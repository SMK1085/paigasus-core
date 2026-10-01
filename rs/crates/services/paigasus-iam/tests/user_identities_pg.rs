// SPDX-License-Identifier: Apache-2.0

//! Postgres coverage for SMA-712: `UserIdentityService` over the REAL adapters
//! (`PgIdentityLinkStore`, `SeaOrmUnitOfWork`, `PgAuditLog`). Each write commits with its audit
//! row. A failed audit write rolls the change back. The concurrency cases of spec 5.6 are
//! deterministic: the peer holds an open transaction until the racer is provably blocked on it.
//!
//! Runs against an ephemeral Postgres in Docker (see `tests/support/mod.rs`).

mod support;

use std::sync::Arc;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{external_identity, principal};
use paigasus_iam::adapters::persistence::uow::recover_txn;
use paigasus_iam::adapters::persistence::{PgAuditLog, PgExternalIdentityRepository, PgIdentityLinkStore, PgPrincipalRepository, SeaOrmUnitOfWork};
use paigasus_iam::application::authorize::Authorize;
use paigasus_iam::application::error::TenancyError;
use paigasus_iam::application::user_identities::{UserIdentityDeps, UserIdentityService};
use paigasus_iam_core::{
    AccessRequest, AuditEntry, AuditFilter, AuditLog, Authorizer, AuthzError, Credential, Decision, Effect, Email, ExternalIdentity, ExternalIdentityRepository, IdentityLinkStore, Issuer, Principal,
    PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, Transaction, UnitOfWork, User,
};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, Set, Statement, TransactionTrait};
use uuid::Uuid;

const ISSUER: &str = "https://idp.example.com/realms/main";

/// Allows every request. `user_identities.rs`'s unit tests cover authorization; these tests
/// need a caller that passes the check so they reach the rows.
struct AllowAllAuthorizer;

#[async_trait::async_trait]
impl Authorizer for AllowAllAuthorizer {
    async fn is_authorized(&self, _req: &AccessRequest) -> Result<Decision, AuthzError> {
        Ok(Decision {
            effect: Effect::Allow,
            determining_policies: Vec::new(),
        })
    }
}

/// An audit sink whose in-transaction write always fails.
struct FailingAudit;

#[async_trait::async_trait]
impl AuditLog for FailingAudit {
    async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
        unimplemented!("the identity calls audit inside the transaction")
    }

    async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
    }

    async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        unimplemented!("the identity calls never query")
    }
}

fn service(db: &DatabaseConnection, audit: Arc<dyn AuditLog>) -> UserIdentityService {
    UserIdentityService::new(UserIdentityDeps {
        authorize: Authorize::new(Arc::new(AllowAllAuthorizer)),
        links: Arc::new(PgIdentityLinkStore::new(db.clone())),
        uow: Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        audit,
        issuers: vec![Issuer::parse(ISSUER).unwrap()],
        ids: Arc::new(KernelIdGenerator),
        clock: Arc::new(SystemClock),
    })
}

fn pid(n: u128) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
}

fn actor() -> Prn {
    pid(1).prn().clone()
}

fn operator() -> Credential {
    Credential::Oidc {
        issuer: Issuer::parse(ISSUER).unwrap(),
        subject: "operator-sub".to_string(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
    }
}

async fn seed_user(db: &DatabaseConnection, n: u128, email: &str) -> PrincipalId {
    let id = pid(n);
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse(email).unwrap(), "Seeded".to_string(), None, None, now, now);
    PgPrincipalRepository::new(db.clone()).create_user(&principal, &user).await.unwrap();
    id
}

async fn audit_rows(db: &DatabaseConnection, action: &str, resource: &PrincipalId) -> Vec<AuditEntry> {
    PgAuditLog::new(db.clone())
        .query(&AuditFilter {
            actor_prn: None,
            resource_prn: Some(resource.canonical()),
            action: Some(action.to_string()),
            outcome: None,
            from: None,
            to: None,
            cursor: None,
            limit: 50,
        })
        .await
        .unwrap()
}

async fn email_of(db: &DatabaseConnection, id: &PrincipalId) -> String {
    let (_, user) = PgPrincipalRepository::new(db.clone()).find_user(id).await.unwrap().expect("user row");
    user.email.as_str().to_string()
}

#[tokio::test]
async fn each_write_commits_with_its_audit_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "pg-a@example.com").await;

    let first = svc.link(&actor(), &user, ISSUER, "pg-sub-1", "INC-1: first").await.unwrap();
    let second = svc.link(&actor(), &user, ISSUER, "pg-sub-2", "INC-2: second").await.unwrap();
    assert!(first.changed && second.changed);
    assert_eq!(audit_rows(&db, "LinkExternalIdentity", &user).await.len(), 2);

    let view = svc.find_by_email(&actor(), "pg-a@example.com").await.unwrap();
    let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
    assert_eq!(subjects, vec!["pg-sub-1", "pg-sub-2"], "identities come back in (created_at, id) order");
    assert_eq!(view.status, PrincipalStatus::Active);

    let again = svc.link(&actor(), &user, ISSUER, "pg-sub-1", "retry").await.unwrap();
    assert!(!again.changed);
    assert_eq!(again.value.id, first.value.id);
    assert_eq!(audit_rows(&db, "LinkExternalIdentity", &user).await.len(), 2, "a same-user link writes no audit row");

    let changed = svc.change_email(&actor(), &user, "pg-b@example.com", "INC-3: moved").await.unwrap();
    assert_eq!(changed.user.email.as_str(), "pg-b@example.com");
    assert_eq!(changed.identities.len(), 2);
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["old_email"], "pg-a@example.com");
    assert_eq!(rows[0].detail["new_email"], "pg-b@example.com");

    svc.change_email(&actor(), &user, "pg-b@example.com", "no-op").await.unwrap();
    assert_eq!(audit_rows(&db, "ChangeUserEmail", &user).await.len(), 1, "an unchanged email writes no audit row");

    svc.unlink(&actor(), &operator(), &user, first.value.id, "INC-4: dead sub").await.unwrap();
    let rows = audit_rows(&db, "UnlinkExternalIdentity", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["subject"], "pg-sub-1", "the unlink audit names the deleted row");
    assert_eq!(svc.unlink(&actor(), &operator(), &user, first.value.id, "retry").await.unwrap_err(), TenancyError::NotFound);
}

/// Fix round 1 finding: `find_user_by_email` and `user_view_in` (via `change_email`) must read
/// the REAL principal status. A `Disabled` principal proves neither path hard-codes `Active`.
#[tokio::test]
async fn find_user_by_email_and_change_email_report_a_disabled_principal_status() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let id = pid(10);
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Disabled, now, now);
    let user = User::new(id.clone(), Email::parse("disabled@example.com").unwrap(), "Disabled".to_string(), None, None, now, now);
    PgPrincipalRepository::new(db.clone()).create_user(&principal, &user).await.unwrap();

    let view = svc.find_by_email(&actor(), "disabled@example.com").await.unwrap();
    assert_eq!(view.status, PrincipalStatus::Disabled, "find_user_by_email must read the real principal status, not a hard-coded one");

    // Spec 5.4: a Disabled user may still have its email changed. This return value comes from
    // `user_view_in`, the other `load_view` call site.
    let changed = svc.change_email(&actor(), &id, "disabled-2@example.com", "r").await.unwrap();
    assert_eq!(changed.status, PrincipalStatus::Disabled, "user_view_in must read the real principal status too");
}

/// Fix round 1 finding: `find_user_by_email` must sort identities by `created_at`, not by
/// insertion order (which usually tracks id anyway). This seeds the later-created row FIRST,
/// and gives it the SMALLER id, so an id-only sort would return the wrong order too.
#[tokio::test]
async fn find_user_by_email_orders_identities_by_created_at_not_insertion_order() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "order-created@example.com").await;
    let earlier = Utc::now().trunc_subsecs(6);
    let later = earlier + chrono::Duration::seconds(10);

    // Inserted first, but created_at is LATER and id is SMALLER than the second insert.
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0xB1)),
        principal_id: Set(user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("second-by-time".to_string()),
        created_at: Set(later),
        updated_at: Set(later),
    }
    .insert(&db)
    .await
    .unwrap();
    // Inserted second, but created_at is EARLIER and id is LARGER.
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0xB2)),
        principal_id: Set(user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("first-by-time".to_string()),
        created_at: Set(earlier),
        updated_at: Set(earlier),
    }
    .insert(&db)
    .await
    .unwrap();

    let view = svc.find_by_email(&actor(), "order-created@example.com").await.unwrap();
    let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
    assert_eq!(subjects, vec!["first-by-time", "second-by-time"], "identities must sort by created_at, not insertion order or id order");
}

/// Fix round 1 finding: two identities with the SAME `created_at` must break the tie by `id`
/// ascending. Insertion order is the reverse of id order, so a plain "keep insertion order"
/// bug and a "no tie-break" bug would both fail this.
#[tokio::test]
async fn find_user_by_email_breaks_a_created_at_tie_by_id_ascending() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "order-tie@example.com").await;
    let same = Utc::now().trunc_subsecs(6);

    // Same created_at. The higher id is inserted FIRST.
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0xC2)),
        principal_id: Set(user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("higher-id".to_string()),
        created_at: Set(same),
        updated_at: Set(same),
    }
    .insert(&db)
    .await
    .unwrap();
    // The lower id is inserted SECOND.
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0xC1)),
        principal_id: Set(user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("lower-id".to_string()),
        created_at: Set(same),
        updated_at: Set(same),
    }
    .insert(&db)
    .await
    .unwrap();

    let view = svc.find_by_email(&actor(), "order-tie@example.com").await.unwrap();
    let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
    assert_eq!(subjects, vec!["lower-id", "higher-id"], "a created_at tie must break by id ascending, not insertion order");
}

#[tokio::test]
async fn a_failed_audit_write_rolls_every_change_back() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let good = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let failing = service(&db, Arc::new(FailingAudit));
    let user = seed_user(&db, 10, "rb@example.com").await;
    let kept = good.link(&actor(), &user, ISSUER, "kept-sub", "setup").await.unwrap().value;
    let identities = PgExternalIdentityRepository::new(db.clone());
    let issuer = Issuer::parse(ISSUER).unwrap();

    assert_eq!(failing.link(&actor(), &user, ISSUER, "lost-sub", "r").await.unwrap_err(), TenancyError::Internal);
    assert!(identities.find_by_issuer_subject(&issuer, "lost-sub").await.unwrap().is_none(), "the link must roll back");

    assert_eq!(failing.unlink(&actor(), &operator(), &user, kept.id, "r").await.unwrap_err(), TenancyError::Internal);
    assert!(identities.find_by_issuer_subject(&issuer, "kept-sub").await.unwrap().is_some(), "the unlink must roll back");

    assert_eq!(failing.change_email(&actor(), &user, "rb-new@example.com", "r").await.unwrap_err(), TenancyError::Internal);
    assert_eq!(email_of(&db, &user).await, "rb@example.com", "the email change must roll back");
}

/// Review focus 5: the refused delete is rolled back, so the identity is still there.
#[tokio::test]
async fn an_own_identity_refusal_rolls_the_delete_back() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "self@example.com").await;
    let mine = svc.link(&actor(), &user, ISSUER, "operator-sub", "setup").await.unwrap().value;
    assert_eq!(svc.unlink(&actor(), &operator(), &user, mine.id, "r").await.unwrap_err(), TenancyError::CannotUnlinkOwnIdentity);
    let still = PgExternalIdentityRepository::new(db.clone())
        .find_by_issuer_subject(&Issuer::parse(ISSUER).unwrap(), "operator-sub")
        .await
        .unwrap();
    assert_eq!(still.map(|i| i.id), Some(mine.id));
    assert!(audit_rows(&db, "UnlinkExternalIdentity", &user).await.is_empty());
}

#[tokio::test]
async fn unlink_in_with_another_users_identity_matches_no_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let owner = seed_user(&db, 10, "owner@example.com").await;
    let other = seed_user(&db, 11, "other@example.com").await;
    let owned = svc.link(&actor(), &owner, ISSUER, "owned-sub", "setup").await.unwrap().value;

    let store = PgIdentityLinkStore::new(db.clone());
    let tx = SeaOrmUnitOfWork::new(db.clone()).begin().await.unwrap();
    assert!(store.unlink_in(&*tx, &other, owned.id).await.unwrap().is_none());
    drop(tx);

    assert_eq!(svc.unlink(&actor(), &operator(), &other, owned.id, "r").await.unwrap_err(), TenancyError::NotFound);
    assert!(
        PgExternalIdentityRepository::new(db.clone())
            .find_by_issuer_subject(&owned.issuer, "owned-sub")
            .await
            .unwrap()
            .is_some()
    );
}

/// Review focus 4: a service account has a `principal` row but no `"user"` row.
#[tokio::test]
async fn a_service_account_id_is_not_found_on_every_write() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let sa = pid(20);
    let now = Utc::now().trunc_subsecs(6);
    principal::ActiveModel {
        id: Set(sa.uuid()),
        prn: Set(sa.canonical()),
        kind: Set(PrincipalKind::ServiceAccount.as_str().to_string()),
        status: Set(PrincipalStatus::Active.as_str().to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    assert_eq!(svc.link(&actor(), &sa, ISSUER, "sa-sub", "r").await.unwrap_err(), TenancyError::NotFound);
    assert_eq!(svc.unlink(&actor(), &operator(), &sa, Uuid::from_u128(5), "r").await.unwrap_err(), TenancyError::NotFound);
    assert_eq!(svc.change_email(&actor(), &sa, "sa@example.com", "r").await.unwrap_err(), TenancyError::NotFound);
}

/// Review focus 3: `user_email_key` is exact, so a case variant is a different email.
#[tokio::test]
async fn a_case_variant_of_another_users_email_is_not_a_conflict_in_postgres() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    seed_user(&db, 10, "case@example.com").await;
    let other = seed_user(&db, 11, "other-case@example.com").await;
    let view = svc.change_email(&actor(), &other, "Case@Example.com", "r").await.unwrap();
    assert_eq!(view.user.email.as_str(), "Case@Example.com");
    assert_eq!(svc.change_email(&actor(), &other, "case@example.com", "r").await.unwrap_err(), TenancyError::EmailConflict);
}

/// Spec 5.6, first case, in the order where JIT wins: the peer inserts the pair and holds its
/// transaction open. The link reads nothing, blocks on the unique index, and gets 409 when the
/// peer commits.
#[tokio::test]
async fn a_link_that_loses_the_race_to_a_jit_insert_is_a_conflict() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let target = seed_user(&db, 10, "race-target@example.com").await;
    let jit_user = seed_user(&db, 11, "race-jit@example.com").await;

    let peer = db.begin().await.unwrap();
    let now = Utc::now().trunc_subsecs(6);
    external_identity::ActiveModel {
        id: Set(Uuid::from_u128(0x7171)),
        principal_id: Set(jit_user.uuid()),
        issuer: Set(ISSUER.to_string()),
        subject: Set("race-sub".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&peer)
    .await
    .unwrap();
    let peer_pid = support::race::backend_pid(&peer).await;

    let racer_svc = svc.clone();
    let racer_target = target.clone();
    let mut racer = tokio::spawn(async move { racer_svc.link(&actor(), &racer_target, ISSUER, "race-sub", "race").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "insert into \"external_identity\"%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    assert_eq!(racer.await.unwrap().unwrap_err(), TenancyError::ExternalIdentityConflict);
    assert!(audit_rows(&db, "LinkExternalIdentity", &target).await.is_empty());
}

/// Spec 5.6, second case: two changes to the same email. Only one commits.
#[tokio::test]
async fn two_email_changes_to_the_same_email_let_only_one_commit() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let first = seed_user(&db, 10, "first@example.com").await;
    let second = seed_user(&db, 11, "second@example.com").await;

    let peer = db.begin().await.unwrap();
    peer.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE "user" SET email = $1 WHERE principal_id = $2"#,
        ["taken@example.com".into(), first.uuid().into()],
    ))
    .await
    .unwrap();
    let peer_pid = support::race::backend_pid(&peer).await;

    let racer_svc = svc.clone();
    let racer_user = second.clone();
    let mut racer = tokio::spawn(async move { racer_svc.change_email(&actor(), &racer_user, "taken@example.com", "race").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "update \"user\"%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    assert_eq!(racer.await.unwrap().unwrap_err(), TenancyError::EmailConflict);
    assert_eq!(email_of(&db, &second).await, "second@example.com");
    assert!(audit_rows(&db, "ChangeUserEmail", &second).await.is_empty());
}

/// Spec 5.6, third case: two changes to ONE user. The peer changes A to B and holds the row
/// lock. The racer's change to C blocks on the lock, then reads B from the locked row. So its
/// audit row says old B, new C, and the history A to B to C has no gap.
#[tokio::test]
async fn two_email_changes_to_one_user_serialize_and_the_second_reads_the_first() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "a@example.com").await;

    let store = PgIdentityLinkStore::new(db.clone());
    let peer = SeaOrmUnitOfWork::new(db.clone()).begin().await.unwrap();
    let first = store
        .change_email_in(&*peer, &user, &Email::parse("b@example.com").unwrap(), Utc::now())
        .await
        .unwrap()
        .expect("user row");
    assert!(first.changed);
    assert_eq!(first.value.old.as_str(), "a@example.com");
    let peer_pid = support::race::backend_pid(recover_txn(&*peer).unwrap()).await;

    let racer_svc = svc.clone();
    let racer_user = user.clone();
    let mut racer = tokio::spawn(async move { racer_svc.change_email(&actor(), &racer_user, "c@example.com", "second change").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "select%for update%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    let view = racer.await.unwrap().unwrap();
    assert_eq!(view.user.email.as_str(), "c@example.com");
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["old_email"], "b@example.com", "the second change must read the first change's value from the locked row");
    assert_eq!(rows[0].detail["new_email"], "c@example.com");
}

/// Code review F1 and F13: SMA-649's stored-PRN rule over the real store. A user PRN with a
/// region or an organization slot names a stored user by uuid, but it differs from the stored
/// `principal.prn`, so each write answers `PrnMismatch` and changes nothing. Control: the
/// canonical PRN works, and its audit row names the stored canonical PRN.
#[tokio::test]
async fn a_user_prn_with_a_region_or_an_org_slot_is_a_prn_mismatch_in_postgres() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "forged@example.com").await;
    let held = svc.link(&actor(), &user, ISSUER, "forged-held", "setup").await.unwrap().value;
    let regional = PrincipalId::from_prn(Prn::build("iam", "eu-west-1", None, "principal", user.uuid()).unwrap());
    let org_slot = PrincipalId::from_prn(Prn::build("iam", "", Some(Uuid::from_u128(99)), "principal", user.uuid()).unwrap());

    for forged in [&regional, &org_slot] {
        assert_eq!(svc.link(&actor(), forged, ISSUER, "forged-new", "r").await.unwrap_err(), TenancyError::PrnMismatch, "{forged:?}");
        assert_eq!(svc.unlink(&actor(), &operator(), forged, held.id, "r").await.unwrap_err(), TenancyError::PrnMismatch, "{forged:?}");
        assert_eq!(
            svc.change_email(&actor(), forged, "forged-2@example.com", "r").await.unwrap_err(),
            TenancyError::PrnMismatch,
            "{forged:?}"
        );
    }
    let identities = PgExternalIdentityRepository::new(db.clone());
    let issuer = Issuer::parse(ISSUER).unwrap();
    assert!(identities.find_by_issuer_subject(&issuer, "forged-new").await.unwrap().is_none(), "no forged link");
    assert!(identities.find_by_issuer_subject(&issuer, "forged-held").await.unwrap().is_some(), "no forged unlink");
    assert_eq!(email_of(&db, &user).await, "forged@example.com", "no forged email change");
    assert_eq!(audit_rows(&db, "LinkExternalIdentity", &user).await.len(), 1, "only the setup link is audited");
    assert!(audit_rows(&db, "UnlinkExternalIdentity", &user).await.is_empty());
    assert!(audit_rows(&db, "ChangeUserEmail", &user).await.is_empty());

    let view = svc.change_email(&actor(), &user, "forged-3@example.com", "control").await.unwrap();
    assert_eq!(view.user.principal_id.canonical(), user.canonical());
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].resource_prn.as_deref(), Some(user.canonical().as_str()), "the audit names the stored canonical PRN");
}

/// Code review F2: two links of one pair to the SAME user race. The peer inserts the pair for
/// the target and holds its transaction open. The racer reads nothing, blocks on the unique
/// index, and gets the unique violation when the peer commits. The unique violation aborts the
/// racer's transaction, so the service reads the pair again in a new one. The same user holds
/// it, so the racer answers `changed: false` with the stored identity and writes no audit row.
#[tokio::test]
async fn a_same_user_link_that_loses_the_race_is_an_unchanged_retry() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let target = seed_user(&db, 10, "same-race@example.com").await;

    let store = PgIdentityLinkStore::new(db.clone());
    let peer = SeaOrmUnitOfWork::new(db.clone()).begin().await.unwrap();
    let now = Utc::now().trunc_subsecs(6);
    let held = ExternalIdentity {
        id: Uuid::from_u128(0x7272),
        principal_id: target.clone(),
        issuer: Issuer::parse(ISSUER).unwrap(),
        subject: "same-race-sub".to_string(),
        created_at: now,
        updated_at: now,
    };
    store.link_in(&*peer, &held).await.unwrap();
    let peer_pid = support::race::backend_pid(recover_txn(&*peer).unwrap()).await;

    let racer_svc = svc.clone();
    let racer_target = target.clone();
    let mut racer = tokio::spawn(async move { racer_svc.link(&actor(), &racer_target, ISSUER, "same-race-sub", "race").await });
    support::race::expect_racer_blocked(&db, peer_pid, &mut racer, "insert into \"external_identity\"%", |r| format!("{r:?}")).await;
    peer.commit().await.unwrap();

    let out = racer.await.unwrap().expect("a same-user race is a safe retry, not a 409");
    assert!(!out.changed);
    assert_eq!(out.value, held);
    assert!(audit_rows(&db, "LinkExternalIdentity", &target).await.is_empty(), "the retry writes no audit row");
}

/// Code review F6: a stored email that `Email::parse` refuses (a hand-edited row) must not block
/// the repair. A link locks the row without a parse, and `change_email` accepts the bad old
/// value and records it as it is in the audit `old_email`.
#[tokio::test]
async fn an_unparseable_stored_email_can_be_repaired() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let svc = service(&db, Arc::new(PgAuditLog::new(db.clone())));
    let user = seed_user(&db, 10, "bad-row@example.com").await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"UPDATE "user" SET email = $1 WHERE principal_id = $2"#,
        ["not-an-email".into(), user.uuid().into()],
    ))
    .await
    .unwrap();

    let linked = svc.link(&actor(), &user, ISSUER, "bad-row-sub", "INC-5: link").await.unwrap();
    assert!(linked.changed, "the link lock must not parse the stored email");

    let view = svc.change_email(&actor(), &user, "repaired@example.com", "INC-5: repair").await.unwrap();
    assert_eq!(view.user.email.as_str(), "repaired@example.com");
    let rows = audit_rows(&db, "ChangeUserEmail", &user).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail["old_email"], "not-an-email", "the audit keeps the raw stored value");
    assert_eq!(rows[0].detail["new_email"], "repaired@example.com");
}
