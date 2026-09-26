// SPDX-License-Identifier: Apache-2.0

//! Integration tests for m0003 + `PgExternalIdentityRepository` — atomic JIT provisioning
//! (D9) and the D7 constraint-name error mapping for external identities.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::{external_identity, principal, user};
use paigasus_iam::adapters::persistence::{PgExternalIdentityRepository, PgPrincipalRepository};
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam_core::{
    ConflictKind, Email, ExternalIdentity, ExternalIdentityRepository, Issuer, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Set, Statement, TransactionTrait};

/// Builds a fresh (principal, user, external_identity) triple with distinct minted ids —
/// callers override email/issuer/subject as needed for each test's scenario.
fn build_triple(seed: u64, entropy: u8, email: &str, issuer: &str, subject: &str) -> (Principal, User, ExternalIdentity) {
    let uuid = mint_uuid7(seed, [entropy; 10]);
    let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap());
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse(email).unwrap(), "Test User".into(), None, None, now, now);
    let identity_id = mint_uuid7(seed, [entropy.wrapping_add(1); 10]);
    let identity = ExternalIdentity {
        id: identity_id,
        principal_id: id,
        issuer: Issuer::parse(issuer).unwrap(),
        subject: subject.into(),
        created_at: now,
        updated_at: now,
    };
    (principal, user, identity)
}

/// AC — `provision` writes principal + user + external_identity atomically, and
/// `find_by_issuer_subject` reconstructs the persisted `ExternalIdentity`.
#[tokio::test]
async fn provision_creates_principal_user_and_identity_atomically() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let (principal, user, identity) = build_triple(1_700_000_100_000, 0x10, "jit@example.com", "https://idp.example.com/realm", "sub-1");

    let repo = PgExternalIdentityRepository::new(db.clone());
    repo.provision(&principal, &user, &identity).await.unwrap();

    let found = repo.find_by_issuer_subject(&identity.issuer, &identity.subject).await.unwrap().expect("identity present");
    assert_eq!(found, identity);

    let principals = PgPrincipalRepository::new(db);
    let (got_p, got_u) = principals.find_user(&principal.id).await.unwrap().expect("user row present");
    assert_eq!(got_p, principal);
    assert_eq!(got_u, user);
}

/// AC — a second `provision` reusing the same `(issuer, subject)` (but a fresh principal and
/// email) must fail with `Conflict(ExternalIdentityExists)` and roll back completely: no
/// orphan principal is left behind (D9).
#[tokio::test]
async fn duplicate_identity_conflicts() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let (first_principal, first_user, first_identity) = build_triple(1_700_000_200_000, 0x20, "first@example.com", "https://idp.example.com/realm", "dup-sub");
    let repo = PgExternalIdentityRepository::new(db.clone());
    repo.provision(&first_principal, &first_user, &first_identity).await.unwrap();

    let (second_principal, second_user, mut second_identity) = build_triple(1_700_000_200_100, 0x21, "second@example.com", "https://idp.example.com/realm", "dup-sub");
    // Same (issuer, subject) as `first_identity`, deliberately.
    second_identity.issuer = first_identity.issuer.clone();
    second_identity.subject = first_identity.subject.clone();

    let result = repo.provision(&second_principal, &second_user, &second_identity).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists))),
        "expected Conflict(ExternalIdentityExists), got {result:?}"
    );

    // No orphan: the second principal must not exist.
    let principals = PgPrincipalRepository::new(db);
    assert!(
        principals.find_principal(&second_principal.id).await.unwrap().is_none(),
        "second principal was orphaned despite the failed transaction"
    );
}

/// AC — a second `provision` with a fresh identity but a colliding email must fail with
/// `Conflict(EmailTaken)`, rolling back the principal AND leaving no dangling identity row
/// (D9 atomicity spans all three inserts).
#[tokio::test]
async fn email_conflict_rolls_back_everything() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let (first_principal, first_user, first_identity) = build_triple(1_700_000_300_000, 0x30, "shared@example.com", "https://idp.example.com/realm", "sub-a");
    let repo = PgExternalIdentityRepository::new(db.clone());
    repo.provision(&first_principal, &first_user, &first_identity).await.unwrap();

    let (second_principal, mut second_user, second_identity) = build_triple(1_700_000_300_100, 0x31, "unused@example.com", "https://idp.example.com/realm", "sub-b");
    // Same email as `first_user` — the `user` insert must fail, rolling back the principal
    // and the external_identity insert alongside it.
    second_user.email = Email::parse("shared@example.com").unwrap();

    let result = repo.provision(&second_principal, &second_user, &second_identity).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::EmailTaken))),
        "expected Conflict(EmailTaken), got {result:?}"
    );

    let principals = PgPrincipalRepository::new(db.clone());
    assert!(
        principals.find_principal(&second_principal.id).await.unwrap().is_none(),
        "second principal was orphaned despite the failed transaction"
    );

    let identity_absent = repo.find_by_issuer_subject(&second_identity.issuer, &second_identity.subject).await.unwrap();
    assert!(identity_absent.is_none(), "second identity was orphaned despite the failed transaction");
}

/// Schema-level test — asserts the exact constraint/index names the D7 error mapping and
/// FK depend on (mirrors `tenancy_schema.rs` for m0002).
#[tokio::test]
async fn constraint_names_are_stable() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    for n in ["uq_external_identity_issuer_subject", "fk_external_identity_principal"] {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_constraint WHERE conname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing constraint {n}");
    }

    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM pg_indexes WHERE indexname = $1",
            ["ix_external_identity_principal".into()],
        ))
        .await
        .unwrap();
    assert!(row.is_some(), "missing index ix_external_identity_principal");
}

/// SMA-698 T3 (spec 4.6, D1). In Postgres, `provision` inserts the `user` row before the
/// `external_identity` row, and `user.email` is unique. So when two first logins of ONE identity
/// race, the loser fails on `user_email_key` (`EmailTaken`) first. The loser must still resolve
/// to the winner's principal and write no JIT failure line.
///
/// Deterministic, as SMA-660 asks of every race test here: a `tokio::join!` of two `resolve`
/// calls also passes when one call commits before the other starts, with no race at all. The
/// winner is one open transaction that holds all three rows. The loser is the real
/// `AppState.authn.resolve`. The winner commits only when the loser is provably blocked inside its
/// `user` insert on the winner's uncommitted email.
#[tokio::test]
async fn lost_first_login_race_on_the_email_resolves_to_the_winner() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.expect("AppState::new");
    let email = "t3-racer@example.com";
    let subject = "t3-race-subject";
    let (winner_principal, winner_user, winner_identity) = build_triple(1_700_000_400_000, 0x40, email, &idp.issuer, subject);

    // The winner: all three rows in one open transaction, in `provision`'s order.
    let winner = db.begin().await.unwrap();
    principal::ActiveModel {
        id: Set(winner_principal.id.uuid()),
        prn: Set(winner_principal.id.canonical()),
        kind: Set(winner_principal.kind.as_str().to_string()),
        status: Set(winner_principal.status.as_str().to_string()),
        created_at: Set(winner_principal.created_at),
        updated_at: Set(winner_principal.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    user::ActiveModel {
        principal_id: Set(winner_user.principal_id.uuid()),
        email: Set(winner_user.email.as_str().to_string()),
        display_name: Set(winner_user.display_name.clone()),
        locale: Set(winner_user.locale.clone()),
        timezone: Set(winner_user.timezone.clone()),
        created_at: Set(winner_user.created_at),
        updated_at: Set(winner_user.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    external_identity::ActiveModel {
        id: Set(winner_identity.id),
        principal_id: Set(winner_identity.principal_id.uuid()),
        issuer: Set(winner_identity.issuer.as_str().to_string()),
        subject: Set(winner_identity.subject.clone()),
        created_at: Set(winner_identity.created_at),
        updated_at: Set(winner_identity.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    let winner_pid = support::race::backend_pid(&winner).await;

    // The loser: the real use case. Its lookup cannot see the uncommitted winner, so it provisions.
    let token = idp.bearer(subject, Some(email), "paigasus", 3600);
    let loser_state = state.clone();
    let mut loser = tokio::spawn(async move { loser_state.authn.resolve(&token, Provisioning::Enabled).await });

    support::race::expect_racer_blocked(&db, winner_pid, &mut loser, "insert into \"user\"%", |r| format!("{r:?}")).await;
    winner.commit().await.unwrap();

    let resolved = loser.await.unwrap().expect("the loser of a first-login race must resolve to the winner");
    assert_eq!(resolved.principal_id, winner_principal.id, "the loser resolves to the winner's principal");
    let users = user::Entity::find().filter(user::Column::Email.eq(email)).count(&db).await.unwrap();
    assert_eq!(users, 1, "exactly one user owns the email after the race");
    let text = logs.text();
    assert_eq!(
        text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).count(),
        0,
        "a lost race writes no JIT failure line:\n{text}"
    );
}
