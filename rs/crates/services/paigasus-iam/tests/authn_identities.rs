// SPDX-License-Identifier: Apache-2.0

//! Integration tests for m0003 + `PgExternalIdentityRepository` — atomic JIT provisioning
//! (D9) and the D7 constraint-name error mapping for external identities.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::persistence::{PgExternalIdentityRepository, PgPrincipalRepository};
use paigasus_iam_core::{
    ConflictKind, Email, ExternalIdentity, ExternalIdentityRepository, Issuer, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

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
            .query_one(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_constraint WHERE conname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing constraint {n}");
    }

    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM pg_indexes WHERE indexname = $1",
            ["ix_external_identity_principal".into()],
        ))
        .await
        .unwrap();
    assert!(row.is_some(), "missing index ix_external_identity_principal");
}
