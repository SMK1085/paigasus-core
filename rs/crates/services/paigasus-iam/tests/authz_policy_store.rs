// SPDX-License-Identifier: Apache-2.0

//! `PgPolicyStore` integration test (SMA-444 Task 10): a `system = true` row (the seeding
//! path) is immutable via `put`/`delete` (`AuthzError::SystemImmutable`); a non-system
//! `put`/`delete` succeeds and bumps `policy_gen`; an invalid Cedar source fails schema
//! validation before ever touching the database; `list_all` returns every persisted row.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`.

mod support;

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::persistence::PgPolicyStore;
use paigasus_iam::adapters::persistence::entities::policy;
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::{AuthzError, PolicyDocument, PolicyStore};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

/// A well-formed, schema-valid static policy document (mirrors `authz::schema`'s own
/// "well-formed" test fixture).
fn valid_static_doc(policy_id: &str, system: bool, now: DateTime<Utc>) -> PolicyDocument {
    PolicyDocument {
        policy_id: policy_id.to_string(),
        kind: PolicyKind::Static,
        source: r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string(),
        description: "test policy".to_string(),
        system,
        created_at: now,
        updated_at: now,
    }
}

/// Inserts a `policy` row directly via the SeaORM entity — bypassing `PgPolicyStore::put`
/// (which itself refuses to mutate an existing `system = true` row) — the seeding-path
/// shape (Task 17) the store must still refuse to edit/delete once persisted.
async fn seed_system_policy(db: &DatabaseConnection, policy_id: &str, now: DateTime<Utc>) {
    policy::ActiveModel {
        policy_id: Set(policy_id.to_string()),
        kind: Set("static".to_string()),
        source: Set(r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string()),
        description: Set(Some("seeded system policy".to_string())),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn put_on_an_existing_system_policy_is_rejected() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    seed_system_policy(&db, "system-policy-put", now).await;

    let store = PgPolicyStore::new(db, Generations::memory());
    let edit_attempt = valid_static_doc("system-policy-put", true, now);

    let err = store.put(&edit_attempt).await.unwrap_err();
    assert!(matches!(&err, AuthzError::SystemImmutable(id) if id == "system-policy-put"), "expected SystemImmutable, got {err:?}");
}

/// A `put` carrying `system: false` for an id that is *already* persisted with
/// `system = true` must still be rejected as `SystemImmutable` — the guard has to read the
/// STORED row's `system` flag, not the incoming `doc`'s. If a future refactor swapped
/// `existing.system` for `doc.system` in the check, this is the bypass it would let through
/// (an attacker simply sets `system: false` on the request to edit a protected row), so this
/// test pins that the stored row wins.
#[tokio::test]
async fn put_with_system_false_cannot_bypass_an_existing_system_true_row() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    seed_system_policy(&db, "system-policy-bypass-attempt", now).await;

    let store = PgPolicyStore::new(db, Generations::memory());
    let mut bypass_attempt = valid_static_doc("system-policy-bypass-attempt", false, now);
    bypass_attempt.source = r#"permit(principal, action == Pgs::Iam::Action::"CreateOrganization", resource);"#.to_string();
    bypass_attempt.description = "attacker-supplied description".to_string();

    let err = store.put(&bypass_attempt).await.unwrap_err();
    assert!(
        matches!(&err, AuthzError::SystemImmutable(id) if id == "system-policy-bypass-attempt"),
        "expected SystemImmutable even though the request claimed system: false, got {err:?}"
    );

    // The stored row must be untouched — not just rejected, but never written.
    let all = store.list_all().await.unwrap();
    let got = all
        .iter()
        .find(|d| d.policy_id == "system-policy-bypass-attempt")
        .expect("seeded system row must survive the rejected put");
    assert!(got.system, "stored row's system flag must remain true");
    assert_eq!(
        got.source, r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#,
        "seeded source must be unchanged, not overwritten by the bypass attempt"
    );
    assert_eq!(got.description, "seeded system policy", "seeded description must be unchanged");
}

#[tokio::test]
async fn put_a_non_system_valid_policy_succeeds_and_bumps_policy_gen() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let store = PgPolicyStore::new(db, Generations::memory());
    let doc = valid_static_doc("non-system-policy", false, now);

    let before = store.policy_gen().await.unwrap();
    store.put(&doc).await.unwrap();
    let after = store.policy_gen().await.unwrap();
    assert_eq!(after, before + 1, "a successful put must bump policy_gen exactly once");

    let all = store.list_all().await.unwrap();
    let got = all.iter().find(|d| d.policy_id == "non-system-policy").expect("row present after put");
    assert!(!got.system);
    assert_eq!(got.source, doc.source);
    assert_eq!(got.description, doc.description);

    // A second `put` on the same (non-system) id is an update, not a conflict, and bumps
    // the generation again.
    let mut updated = doc.clone();
    updated.description = "updated description".to_string();
    // A forged, different `created_at` on the incoming doc must be ignored on the UPDATE
    // path — the stored row's original `created_at` is preserved, not overwritten.
    updated.created_at = now + chrono::Duration::days(1);
    store.put(&updated).await.unwrap();
    assert_eq!(store.policy_gen().await.unwrap(), before + 2);
    let all = store.list_all().await.unwrap();
    let got = all.iter().find(|d| d.policy_id == "non-system-policy").expect("row still present after update");
    assert_eq!(got.description, "updated description");
    assert_eq!(
        got.created_at, doc.created_at,
        "created_at must be preserved from the original row, not overwritten by an update's doc.created_at"
    );
}

#[tokio::test]
async fn put_an_invalid_policy_source_fails_schema_validation_before_touching_the_db() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let store = PgPolicyStore::new(db, Generations::memory());
    let mut doc = valid_static_doc("bad-policy", false, now);
    // Well-formed Cedar syntax, but `NoSuchAction` isn't in the embedded schema's action
    // catalog — a schema-validation failure, not a parse failure.
    doc.source = r#"permit(principal, action == Pgs::Iam::Action::"NoSuchAction", resource);"#.to_string();

    let before = store.policy_gen().await.unwrap();
    let err = store.put(&doc).await.unwrap_err();
    assert!(matches!(err, AuthzError::SchemaValidation(_)), "expected SchemaValidation, got {err:?}");

    // Rejected before ever reaching the database: no row created, no generation bump.
    assert_eq!(store.policy_gen().await.unwrap(), before);
    let all = store.list_all().await.unwrap();
    assert!(!all.iter().any(|d| d.policy_id == "bad-policy"));
}

#[tokio::test]
async fn delete_of_an_existing_system_policy_is_rejected() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    seed_system_policy(&db, "system-policy-delete", now).await;

    let store = PgPolicyStore::new(db, Generations::memory());
    let err = store.delete("system-policy-delete").await.unwrap_err();
    assert!(matches!(&err, AuthzError::SystemImmutable(id) if id == "system-policy-delete"), "expected SystemImmutable, got {err:?}");

    // The row must survive the rejected delete.
    let all = store.list_all().await.unwrap();
    assert!(all.iter().any(|d| d.policy_id == "system-policy-delete"));
}

#[tokio::test]
async fn delete_of_a_non_system_policy_succeeds_and_bumps_policy_gen() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let store = PgPolicyStore::new(db, Generations::memory());
    store.put(&valid_static_doc("deletable-policy", false, now)).await.unwrap();

    let before = store.policy_gen().await.unwrap();
    store.delete("deletable-policy").await.unwrap();
    let after = store.policy_gen().await.unwrap();
    assert_eq!(after, before + 1);

    let all = store.list_all().await.unwrap();
    assert!(!all.iter().any(|d| d.policy_id == "deletable-policy"));
}

/// Deleting a policy id that was never persisted is a no-op success (idempotent DELETE
/// semantics) rather than an error — and must not bump the generation, since nothing
/// changed.
#[tokio::test]
async fn delete_of_an_unknown_policy_id_is_a_noop() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db, Generations::memory());

    let before = store.policy_gen().await.unwrap();
    store.delete("never-existed").await.unwrap();
    assert_eq!(store.policy_gen().await.unwrap(), before);
}

#[tokio::test]
async fn list_all_returns_every_inserted_row() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    seed_system_policy(&db, "system-listed", now).await;
    let store = PgPolicyStore::new(db, Generations::memory());
    store.put(&valid_static_doc("static-listed", false, now)).await.unwrap();

    let all = store.list_all().await.unwrap();
    let ids: Vec<_> = all.iter().map(|d| d.policy_id.as_str()).collect();
    assert!(ids.contains(&"system-listed"), "seeded system row missing from list_all: {ids:?}");
    assert!(ids.contains(&"static-listed"), "put row missing from list_all: {ids:?}");
}
