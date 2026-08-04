// SPDX-License-Identifier: Apache-2.0

//! `PgPolicyStore` integration test (SMA-444 Task 10): a `system = true` row (the seeding
//! path) is immutable via `put`/`delete` (`AuthzError::SystemImmutable`); a non-system
//! `put`/`delete` succeeds and bumps `policy_gen`; an invalid Cedar source fails schema
//! validation before ever touching the database; `list_all` returns every persisted row.
//!
//! SMA-446 Slice B Task B5 additionally covers `put_in`/`delete_in` directly (the txn-scoped
//! primitives `PolicyService::put`/`delete` actually drive): the SAME same-content-absorbs /
//! different-content-`Conflict` semantics as `put`'s own race tests above, but through a
//! caller-owned `SeaOrmUnitOfWork` transaction and a SAVEPOINT rather than a fresh connection
//! — proving the outer UoW txn survives both a savepoint rollback (absorb) and a savepoint
//! rollback plus `Conflict` (different content); and `put_in`/`Outbox::enqueue`/
//! `AuditLog::record` commit atomically sharing one correlation id, mirroring
//! `tests/authz_role_grants.rs`'s own B4 atomicity proof.
//!
//! SMA-477 adds one guard on the same public path: `put` must never write the reconciliation
//! columns (`content_fingerprint`/`starter_revision`) on either the INSERT or the UPDATE
//! branch — they belong to `SystemPolicyReconciler` alone. The reconciler's own behaviour is
//! covered in `tests/authz_bootstrap.rs`.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`.

mod support;

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox, policy};
use paigasus_iam::adapters::persistence::{PgAuditLog, PgOutbox, PgPolicyStore, SeaOrmUnitOfWork};
use paigasus_iam_core::authz::model::{PolicyKind, root_prn};
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, AuthzError, DomainEvent, EventType, IdGenerator, Outbox, PolicyDocument, PolicyStore, PutOutcome, UnitOfWork};
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, Set, TransactionTrait};

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
        content_fingerprint: NotSet,
        starter_revision: NotSet,
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

/// SMA-477 regression guard: the public `PutPolicy` path must leave `content_fingerprint` and
/// `starter_revision` alone on BOTH the INSERT and the UPDATE branch. `doc_to_model` keeps them
/// `NotSet` deliberately — those columns are `SystemPolicyReconciler`'s alone, and an
/// operator-authored policy carrying a fingerprint would read as this service's own provenance.
/// Without this test, a future edit flipping either `NotSet` to `Set(..)` would pass every other
/// case in the suite.
#[tokio::test]
async fn put_never_writes_the_reconciliation_columns() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = valid_static_doc("operator-authored-policy", false, now);

    // INSERT branch.
    store.put(&doc).await.unwrap();
    let row = policy::Entity::find_by_id("operator-authored-policy".to_string())
        .one(&db)
        .await
        .unwrap()
        .expect("row present after put");
    assert_eq!(row.content_fingerprint, None, "an operator-authored policy must carry no fingerprint");
    assert_eq!(row.starter_revision, None, "an operator-authored policy must carry no starter revision");

    // UPDATE branch — a second `put` with changed content.
    let mut updated = doc.clone();
    updated.description = "updated description".to_string();
    updated.source = r#"permit(principal, action == Pgs::Iam::Action::"CreateOrganization", resource);"#.to_string();
    store.put(&updated).await.unwrap();
    let row = policy::Entity::find_by_id("operator-authored-policy".to_string())
        .one(&db)
        .await
        .unwrap()
        .expect("row present after the update");
    assert_eq!(row.source, updated.source, "the update must actually have landed");
    assert_eq!(row.content_fingerprint, None, "an update must not start stamping a fingerprint either");
    assert_eq!(row.starter_revision, None, "an update must not start stamping a starter revision either");
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

/// Boot-reliability fix (SMA-444 Task 17 review finding): `PgPolicyStore::put`'s existence
/// check and its INSERT aren't atomic, so two replicas booting concurrently against a
/// fresh, unseeded database can both observe `existing == None` for the same starter
/// `policy_id` and both attempt to insert it — the loser must hit a unique-constraint
/// violation and absorb it as an idempotent success (mirroring
/// `bootstrap.rs::seed_role_row`), not fail its replica's `AppState::new`. This covers the
/// SAME-content case (both racers write the IDENTICAL starter policy document); the
/// DIFFERENT-content case is covered separately below
/// (`concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict`).
///
/// Drives two `PgPolicyStore` handles that share the same underlying connection pool
/// (`DatabaseConnection` clones an `Arc`-backed pool handle, so this is a REAL race over
/// the network, not a simulation) at the exact same, previously-absent `policy_id` via
/// `tokio::join!`. Both `put` calls must return `Ok(())` — one takes the genuine INSERT
/// path, the other absorbs the resulting unique-constraint violation — and exactly one row
/// must exist afterward.
#[tokio::test]
async fn concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let store_a = PgPolicyStore::new(db.clone(), Generations::memory());
    let store_b = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = valid_static_doc("racing-policy", false, now);

    let (result_a, result_b) = tokio::join!(store_a.put(&doc), store_b.put(&doc));
    assert!(result_a.is_ok(), "first racer must not fail: {result_a:?}");
    assert!(result_b.is_ok(), "second racer must not fail — the unique-violation loser must absorb, not error: {result_b:?}");

    let all = store_a.list_all().await.unwrap();
    let matches: Vec<_> = all.iter().filter(|d| d.policy_id == "racing-policy").collect();
    assert_eq!(matches.len(), 1, "exactly one row must exist after the race, not zero or two: {matches:?}");
}

/// CodeRabbit review fix (SMA-444): the SAME-content race above absorbs a unique-constraint
/// violation as `Ok(())` because it's genuinely idempotent (both racers wrote the identical
/// starter policy). That absorption is WRONG for the public `PutPolicy` API when two callers
/// race to CREATE the same `policy_id` with DIFFERENT documents — the loser's write would
/// otherwise vanish silently (a lost update) while still reporting success. The loser must
/// instead see `AuthzError::Conflict`.
///
/// A plain `tokio::join!` of two `put()` calls (as above) doesn't reliably land in the
/// genuine INSERT-vs-INSERT race window under load — if racer A's whole `put()` (select,
/// insert, commit) happens to complete before racer B's existence check runs, B just takes
/// the ordinary UPDATE path (`existing = Some`), which unconditionally overwrites and never
/// touches the unique-violation branch this test targets. So this test manufactures the race
/// deterministically instead: it holds racer A's INSERT open in an UNCOMMITTED transaction
/// (so B's existence check still sees no row — MVCC visibility — and B also attempts an
/// INSERT, which Postgres blocks on A's uncommitted row), then commits A once B's `put` (the
/// real, unmodified production method) is in flight, forcing B's blocked INSERT to resolve
/// into a genuine unique-constraint violation.
#[tokio::test]
async fn concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let mut doc_a = valid_static_doc("racing-policy-conflict", false, now);
    doc_a.description = "racer A's document".to_string();
    let mut doc_b = valid_static_doc("racing-policy-conflict", false, now);
    doc_b.source = r#"permit(principal, action == Pgs::Iam::Action::"CreateOrganization", resource);"#.to_string();
    doc_b.description = "racer B's document".to_string();

    // Racer A: insert directly (bypassing `PgPolicyStore::put`, mirroring
    // `seed_system_policy`'s established direct-entity pattern) and hold the transaction
    // open — not yet committed, so not yet visible to any other transaction.
    let txn_a = db.begin().await.unwrap();
    policy::ActiveModel {
        policy_id: Set(doc_a.policy_id.clone()),
        kind: Set("static".to_string()),
        source: Set(doc_a.source.clone()),
        description: Set(Some(doc_a.description.clone())),
        system: Set(doc_a.system),
        created_at: Set(doc_a.created_at),
        updated_at: Set(doc_a.updated_at),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(&txn_a)
    .await
    .unwrap();

    // Racer B: the real `PgPolicyStore::put`, spawned so it runs concurrently with the test
    // body. Its existence check sees no row yet (A's insert is uncommitted) and it attempts
    // its own INSERT, which Postgres blocks pending A's transaction outcome.
    let store_b = PgPolicyStore::new(db.clone(), Generations::memory());
    let put_b = tokio::spawn(async move { store_b.put(&doc_b).await });

    // Give racer B's task time to actually reach (and block inside) its own INSERT before A
    // commits — comfortably longer than a local Postgres round-trip even under load.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    txn_a.commit().await.unwrap();

    let result_b = put_b.await.unwrap();
    assert!(
        matches!(&result_b, Err(AuthzError::Conflict(id)) if id == "racing-policy-conflict"),
        "the losing racer must see AuthzError::Conflict, not a silent Ok that hides its discarded write: {result_b:?}"
    );

    // The stored row belongs to A (the transaction that actually committed) — a single,
    // non-corrupted row, untouched by B's discarded write.
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let all = store.list_all().await.unwrap();
    let matches: Vec<_> = all.iter().filter(|d| d.policy_id == "racing-policy-conflict").collect();
    assert_eq!(matches.len(), 1, "exactly one row must exist after the race: {matches:?}");
    assert_eq!(matches[0].description, "racer A's document", "the stored row must be racer A's — the transaction that committed");
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

/// SMA-446 Slice B Task B5 — the crux, at the store level: a same-content unique-violation
/// race, replayed through `put_in` on a caller-owned UoW transaction, must absorb into
/// `PutOutcome::AbsorbedIdempotent` via a SAVEPOINT rather than aborting the whole `tx` —
/// proven by writing something else on the SAME outer `tx` afterward and committing
/// successfully (savepoint isolation, not "this transaction is now aborted"). Manufactures
/// the race deterministically, exactly like
/// `concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict` below, but
/// with IDENTICAL content this time (the cold-boot `reconcile_starter` race, SMA-444 Task 17
/// follow-up): a plain pre-`commit`-then-`put_in` sequence would just take the ordinary
/// UPDATE path (`existing = Some`) and never touch the savepoint-INSERT branch this test
/// targets, so racer A's INSERT must still be uncommitted when racer B's `put_in` runs its
/// own existence check.
#[tokio::test]
async fn put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let doc = valid_static_doc("savepoint-absorb", false, now);

    // Racer A: insert directly (bypassing `PgPolicyStore`) and hold the transaction open
    // uncommitted — so racer B's existence check still sees no row (MVCC visibility) and
    // itself attempts an INSERT, which Postgres blocks on A's uncommitted row.
    let txn_a = db.begin().await.unwrap();
    policy::ActiveModel {
        policy_id: Set(doc.policy_id.clone()),
        kind: Set("static".to_string()),
        source: Set(doc.source.clone()),
        description: Set(Some(doc.description.clone())),
        system: Set(doc.system),
        created_at: Set(doc.created_at),
        updated_at: Set(doc.updated_at),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(&txn_a)
    .await
    .unwrap();

    // Racer B: `put_in` on a caller-owned UoW transaction, spawned to run concurrently, with
    // the SAME content as A.
    let gens = Generations::memory();
    let store = PgPolicyStore::new(db.clone(), gens);
    let store_b = store.clone();
    let doc_b = doc.clone();
    let uow_b = SeaOrmUnitOfWork::new(db.clone());
    let put_b = tokio::spawn(async move {
        let tx = uow_b.begin().await.expect("begin");
        let outcome = store_b.put_in(&*tx, &doc_b).await;
        (tx, outcome)
    });

    // Give racer B's task time to actually reach (and block inside) its own savepoint INSERT
    // before A commits — comfortably longer than a local Postgres round-trip even under load.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    txn_a.commit().await.unwrap();

    let (tx_b, outcome_b) = put_b.await.unwrap();
    let outcome = outcome_b.unwrap();
    assert!(matches!(outcome, PutOutcome::AbsorbedIdempotent), "expected AbsorbedIdempotent, got {outcome:?}");

    // The outer UoW txn must still be usable after the savepoint rollback — write something
    // else on it and commit successfully.
    let sibling = valid_static_doc("savepoint-absorb-sibling", false, now);
    let sibling_outcome = store.put_in(&*tx_b, &sibling).await.unwrap();
    assert!(matches!(sibling_outcome, PutOutcome::Inserted), "expected Inserted, got {sibling_outcome:?}");
    tx_b.commit().await.expect("outer txn must still be able to commit after the savepoint rollback");

    let all = store.list_all().await.unwrap();
    assert!(all.iter().any(|d| d.policy_id == "savepoint-absorb-sibling"), "the outer txn's later write must have committed");
    assert_eq!(
        all.iter().filter(|d| d.policy_id == "savepoint-absorb").count(),
        1,
        "the absorbed put must not have created a duplicate row"
    );
}

/// SMA-446 Slice B Task B5's other half of the crux: a DIFFERENT-content race through
/// `put_in` must still surface `AuthzError::Conflict` (never a silent absorb), and — same as
/// the same-content case above — the caller's outer UoW `tx` must remain usable afterward,
/// proving the savepoint rollback isolated the failed INSERT without aborting `tx` at the
/// Postgres level. Manufactures the race deterministically exactly like
/// `concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict` above:
/// holds racer A's INSERT open uncommitted (so racer B's existence check sees no row and
/// itself attempts an INSERT, which Postgres blocks on A's uncommitted row) inside racer B's
/// own savepoint, then commits A once B's `put_in` is in flight, forcing B's blocked INSERT
/// to resolve into a genuine unique-constraint violation.
#[tokio::test]
async fn put_in_surfaces_a_different_content_savepoint_conflict_and_the_outer_txn_stays_usable() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let mut doc_a = valid_static_doc("savepoint-conflict", false, now);
    doc_a.description = "racer A's document".to_string();
    let mut doc_b = valid_static_doc("savepoint-conflict", false, now);
    doc_b.source = r#"permit(principal, action == Pgs::Iam::Action::"CreateOrganization", resource);"#.to_string();
    doc_b.description = "racer B's document".to_string();

    // Racer A: insert directly (bypassing `PgPolicyStore`, mirroring `seed_system_policy`'s
    // established direct-entity pattern) and hold the transaction open uncommitted.
    let txn_a = db.begin().await.unwrap();
    policy::ActiveModel {
        policy_id: Set(doc_a.policy_id.clone()),
        kind: Set("static".to_string()),
        source: Set(doc_a.source.clone()),
        description: Set(Some(doc_a.description.clone())),
        system: Set(doc_a.system),
        created_at: Set(doc_a.created_at),
        updated_at: Set(doc_a.updated_at),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(&txn_a)
    .await
    .unwrap();

    // Racer B: `put_in` on a caller-owned UoW transaction, spawned to run concurrently.
    let gens = Generations::memory();
    let store = PgPolicyStore::new(db.clone(), gens);
    let store_b = store.clone();
    let uow_b = SeaOrmUnitOfWork::new(db.clone());
    let put_b = tokio::spawn(async move {
        let tx = uow_b.begin().await.expect("begin");
        let outcome = store_b.put_in(&*tx, &doc_b).await;
        (tx, outcome)
    });

    // Give racer B's task time to actually reach (and block inside) its own savepoint INSERT
    // before A commits — comfortably longer than a local Postgres round-trip even under load.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    txn_a.commit().await.unwrap();

    let (tx_b, outcome_b) = put_b.await.unwrap();
    assert!(
        matches!(&outcome_b, Err(AuthzError::Conflict(id)) if id == "savepoint-conflict"),
        "the losing racer must see AuthzError::Conflict via the savepoint path, not a silent absorb: {outcome_b:?}"
    );

    // Savepoint isolation: B's outer UoW txn must still be usable after the savepoint
    // rollback — write something else on it and commit successfully.
    let sibling = valid_static_doc("savepoint-conflict-sibling", false, now);
    let sibling_outcome = store.put_in(&*tx_b, &sibling).await.unwrap();
    assert!(matches!(sibling_outcome, PutOutcome::Inserted), "expected Inserted, got {sibling_outcome:?}");
    tx_b.commit().await.expect("outer txn must still be able to commit after the conflicting savepoint rollback");

    let all = store.list_all().await.unwrap();
    assert!(
        all.iter().any(|d| d.policy_id == "savepoint-conflict-sibling"),
        "the outer txn's later write must have committed after the conflict"
    );
    let matches: Vec<_> = all.iter().filter(|d| d.policy_id == "savepoint-conflict").collect();
    assert_eq!(matches.len(), 1, "exactly one row must exist for the conflicted id: {matches:?}");
    assert_eq!(matches[0].description, "racer A's document", "the stored row must be racer A's — the transaction that committed");
}

/// A rejected `put_in` on an existing system row must never even open a savepoint — proven
/// by writing something else on the same outer `tx` afterward and committing successfully
/// (the rejection is a pure read-then-reject, no write of ours ever touched `tx`).
#[tokio::test]
async fn put_in_on_an_existing_system_policy_is_rejected_before_any_write() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    seed_system_policy(&db, "system-policy-put-in", now).await;

    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.expect("begin");
    let edit_attempt = valid_static_doc("system-policy-put-in", true, now);

    let err = store.put_in(&*tx, &edit_attempt).await.unwrap_err();
    assert!(matches!(&err, AuthzError::SystemImmutable(id) if id == "system-policy-put-in"), "expected SystemImmutable, got {err:?}");

    let sibling = valid_static_doc("system-policy-put-in-sibling", false, now);
    let outcome = store.put_in(&*tx, &sibling).await.unwrap();
    assert!(matches!(outcome, PutOutcome::Inserted), "expected Inserted, got {outcome:?}");
    tx.commit().await.expect("outer txn must still be usable after the rejected system-row edit");
}

/// SMA-477 D6: the starter ids are reserved even before they are seeded. Without this an
/// operator could occupy one, and a row that is not `system = true` would then be exempt from
/// boot-time convergence forever.
#[tokio::test]
async fn put_rejects_a_reserved_starter_policy_id() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let now = Utc::now();

    let doc = PolicyDocument {
        policy_id: "org_admin".to_string(),
        kind: PolicyKind::Static,
        source: "permit(principal, action, resource);".to_string(),
        description: String::new(),
        system: false,
        created_at: now,
        updated_at: now,
    };

    // Rejected on a FRESH database, i.e. before the id is seeded — the check is on the id, not
    // on any stored row's `system` flag.
    let err = store.put(&doc).await.expect_err("a reserved starter id must be rejected");
    assert!(matches!(&err, AuthzError::SystemImmutable(id) if id == "org_admin"), "got {err:?}");

    let forbid = PolicyDocument {
        policy_id: "forbid-archived-writes".to_string(),
        ..doc.clone()
    };
    assert!(matches!(store.put(&forbid).await, Err(AuthzError::SystemImmutable(_))));

    // An operator's own id is unaffected.
    let ok = PolicyDocument {
        policy_id: "operator-policy".to_string(),
        ..doc
    };
    store.put(&ok).await.expect("a non-reserved id must still be accepted");
}

/// SMA-446 Task B5 — the UoW reference pattern's atomicity proof at the store level (mirrors
/// `tests/authz_role_grants.rs::grant_in_enqueue_and_record_commit_atomically_sharing_correlation_id`):
/// `PgPolicyStore::put_in` + `PgOutbox::enqueue` + `PgAuditLog::record`, driven through the
/// SAME `SeaOrmUnitOfWork` transaction and committed together, land as three durable rows —
/// `policy`/`event_outbox`/`audit_log` — sharing the ONE correlation id `PolicyService`
/// mints per mutation. `put_in` itself must NOT bump `policy_gen` — that is the caller's
/// (`PolicyService`'s) own awaited, post-commit responsibility, never the store's.
#[tokio::test]
async fn put_in_enqueue_and_record_commit_atomically_sharing_correlation_id() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let gens = Generations::memory();
    let store = PgPolicyStore::new(db.clone(), gens.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new();
    let audit = PgAuditLog::new(db.clone());

    let doc = valid_static_doc("atomic-put", false, now);
    let corr = KernelIdGenerator.new_correlation_id();
    let event = DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::PolicyPut,
        schema_version: 1,
        aggregate_prn: format!("policy/{}", doc.policy_id),
        actor_prn: None,
        occurred_at: now,
        payload: serde_json::json!({"policy_id": doc.policy_id, "kind": "static"}),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: now,
        actor_prn: None,
        action: "PutPolicy".to_string(),
        resource_prn: Some(root_prn().canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({"policy_id": doc.policy_id}),
        correlation_id: Some(corr),
    };

    let before = gens.policy_gen().await.unwrap();
    let tx = uow.begin().await.unwrap();
    let outcome = store.put_in(&*tx, &doc).await.unwrap();
    assert!(matches!(outcome, PutOutcome::Inserted), "expected Inserted, got {outcome:?}");
    outbox.enqueue(&*tx, &event).await.unwrap();
    audit.record(&*tx, &entry).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().is_some(),
        "the committed policy row must be visible"
    );
    let outbox_row = event_outbox::Entity::find_by_id(event.id).one(&db).await.unwrap().expect("outbox row present");
    let audit_row = audit_log::Entity::find_by_id(entry.id).one(&db).await.unwrap().expect("audit row present");
    assert_eq!(outbox_row.correlation_id, Some(corr));
    assert_eq!(audit_row.correlation_id, Some(corr));
    assert_eq!(
        outbox_row.correlation_id, audit_row.correlation_id,
        "the outbox event and the audit entry must share one correlation id"
    );

    assert_eq!(
        gens.policy_gen().await.unwrap(),
        before,
        "put_in must never bump policy_gen itself — that is the caller's own post-commit responsibility"
    );
}
