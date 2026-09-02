// SPDX-License-Identifier: Apache-2.0

//! Schema-level tests for m0005 (SMA-445 design §5.1): asserts the `service_account`/`api_key`
//! tables + their SeaORM entities exist with the expected columns, the exact constraint/index
//! names the future D7 error mapping depends on, and that `ck_service_account_owner`/
//! `ck_api_key_scope` actually reject a row that doesn't set exactly one owner/scope target.
//!
//! `PgServiceAccountRepository` end-to-end coverage (Task 9): `create`'s two-insert-one-
//! transaction round-trip, the per-owner unique-name conflict (D7), and `set_principal_
//! status` updating the `principal` row's lifecycle status (D16).
//!
//! `PgApiKeyRepository` end-to-end coverage (Task 10): `issue`/`find_by_id`/`revoke`'s
//! round-trip (including revoke's idempotent-no-op posture), `touch_last_used`'s throttled
//! guarded UPDATE, `list_by_service_account`'s per-SA scoping, and the `uq_api_key_hash` ->
//! `ConflictKind::ApiKeyHashCollision` mapping (D7).

mod support;

use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::persistence::entities::{api_key, principal};
use paigasus_iam::adapters::persistence::{PgApiKeyRepository, PgServiceAccountRepository};
use paigasus_iam_core::{Action, ApiKeyRepository, ApiKeyStatus, Clock, ConflictKind, PrincipalStatus, RepositoryError, ServiceAccountRepository};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, Statement};

/// Querying the empty `api_key` table through its SeaORM entity succeeds — proves the table
/// and every column the entity's `Model` declares actually exist (a missing/mistyped column
/// would fail the `SELECT` this issues, not just panic on a `.unwrap()` of absent data).
#[tokio::test]
async fn m0005_creates_tables() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let n = api_key::Entity::find().count(&db).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn service_accounts_schema_has_named_constraints_and_indexes() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let constraint_names = [
        "fk_service_account_principal",
        "fk_service_account_org",
        "fk_service_account_team",
        "fk_service_account_project",
        "ck_service_account_owner",
        "fk_api_key_service_account",
        "fk_api_key_scope_org",
        "fk_api_key_scope_team",
        "fk_api_key_scope_project",
        "uq_api_key_hash",
        "ck_api_key_scope",
    ];
    for n in constraint_names {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_constraint WHERE conname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing constraint {n}");
    }

    for n in [
        "uq_service_account_org_name",
        "uq_service_account_team_name",
        "uq_service_account_project_name",
        "ix_api_key_service_account",
    ] {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_indexes WHERE indexname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing index {n}");
    }
}

/// AC — `ck_service_account_owner` rejects a row that sets zero or more than one of
/// `owner_org_id`/`owner_team_id`/`owner_project_id`.
#[tokio::test]
async fn service_account_check_rejects_non_single_owner_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
           VALUES ('11111111-1111-1111-1111-111111111111', 'prn:pgs:iam:::principal/11111111-1111-1111-1111-111111111111', 'service_account', 'active', now(), now())"#,
        [],
    ))
    .await
    .unwrap();

    // Zero owner targets set.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "service_account" (principal_id, name, created_at, updated_at)
               VALUES ('11111111-1111-1111-1111-111111111111', 'ci-bot', now(), now())"#,
            [],
        ))
        .await;
    let err = result.expect_err("insert with no owner target set must fail");
    assert!(err.to_string().contains("ck_service_account_owner"), "unexpected error: {err}");

    // Two owner targets set (org + team) — arbitrary uuids, not real rows; the CHECK fires
    // before Postgres would ever evaluate the (absent) FK targets.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "service_account" (principal_id, owner_org_id, owner_team_id, name, created_at, updated_at)
               VALUES ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222',
                       '33333333-3333-3333-3333-333333333333', 'ci-bot', now(), now())"#,
            [],
        ))
        .await;
    let err = result.expect_err("insert with two owner targets set must fail");
    assert!(err.to_string().contains("ck_service_account_owner"), "unexpected error: {err}");
}

/// AC — `ck_api_key_scope` rejects a row that sets zero or more than one of
/// `scope_org_id`/`scope_team_id`/`scope_project_id`.
#[tokio::test]
async fn api_key_check_rejects_non_single_scope_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
           VALUES ('11111111-1111-1111-1111-111111111111', 'prn:pgs:iam:::principal/11111111-1111-1111-1111-111111111111', 'service_account', 'active', now(), now())"#,
        [],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
           VALUES ('22222222-2222-2222-2222-222222222222', 'prn:pgs:iam:::organization/22222222-2222-2222-2222-222222222222', 'acme', 'Acme', 'active', now(), now())"#,
        [],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "service_account" (principal_id, owner_org_id, name, created_at, updated_at)
           VALUES ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222', 'ci-bot', now(), now())"#,
        [],
    ))
    .await
    .unwrap();

    // Zero scope targets set.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "api_key" (id, service_account_id, prefix, key_hash, status, created_at)
               VALUES ('44444444-4444-4444-4444-444444444444', '11111111-1111-1111-1111-111111111111', 'pgs_sk_abc', 'hash-a', 'active', now())"#,
            [],
        ))
        .await;
    let err = result.expect_err("insert with no scope target set must fail");
    assert!(err.to_string().contains("ck_api_key_scope"), "unexpected error: {err}");

    // Two scope targets set (org + team). `scope_team_id` here is an arbitrary uuid, not a
    // real `team` row — Postgres evaluates CHECK constraints (part of ExecConstraints, before
    // the row is written) ahead of the scope FK's AFTER-ROW trigger, so `ck_api_key_scope`
    // fires first regardless.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "api_key" (id, service_account_id, scope_org_id, scope_team_id, prefix, key_hash, status, created_at)
               VALUES ('55555555-5555-5555-5555-555555555555', '11111111-1111-1111-1111-111111111111',
                       '22222222-2222-2222-2222-222222222222', '33333333-3333-3333-3333-333333333333',
                       'pgs_sk_def', 'hash-b', 'active', now())"#,
            [],
        ))
        .await;
    let err = result.expect_err("insert with two scope targets set must fail");
    assert!(err.to_string().contains("ck_api_key_scope"), "unexpected error: {err}");
}

/// AC — `create` inserts `principal` + `service_account` in one transaction (Task 9 brief),
/// and `find` round-trips the name; the `principal` row backing it carries
/// `kind = "service_account"` (D16). `find` also returns the principal's status — freshly
/// created, `Active` (CodeRabbit finding on the SMA-445 PR: read paths must surface status).
#[tokio::test]
async fn create_and_find_service_account() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let repo = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner);

    repo.create(&p, &sa).await.unwrap();

    let got = repo.find(&sa.principal_id).await.unwrap().expect("row present");
    assert_eq!(got.account, sa);
    assert_eq!(got.status, PrincipalStatus::Active);

    let pr = principal::Entity::find_by_id(sa.principal_id.uuid()).one(&db).await.unwrap().expect("principal row present");
    assert_eq!(pr.kind, "service_account");
}

/// AC — two service accounts under the SAME owner with the SAME name conflict
/// (`uq_service_account_org_name`, D7): `Err(RepositoryError::Conflict(ConflictKind::
/// ServiceAccountNameTaken))`, and the second `principal` row must not be orphaned.
#[tokio::test]
async fn duplicate_name_per_owner_conflicts() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let repo = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p1, sa1) = support::sample_sa("dup", owner.clone());
    let (p2, sa2) = support::sample_sa("dup", owner);

    repo.create(&p1, &sa1).await.unwrap();
    let result = repo.create(&p2, &sa2).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::ServiceAccountNameTaken))),
        "expected Conflict(ServiceAccountNameTaken), got {result:?}"
    );

    // The second principal must NOT be orphaned: the transaction rolled back its insert.
    let orphan = principal::Entity::find_by_id(sa2.principal_id.uuid()).one(&db).await.unwrap();
    assert!(orphan.is_none(), "second principal was orphaned despite the failed transaction");
}

/// AC — `set_principal_status` updates the LIFECYCLE status on the `principal` row (D16: the
/// `service_account` table has no `status` column of its own), and `find`'s own returned
/// `status` reflects it immediately (the JOIN read, not a stale/cached value).
#[tokio::test]
async fn set_principal_status_disables() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let repo = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner);
    repo.create(&p, &sa).await.unwrap();

    repo.set_principal_status(&sa.principal_id, PrincipalStatus::Disabled).await.unwrap();

    let pr = principal::Entity::find_by_id(sa.principal_id.uuid()).one(&db).await.unwrap().expect("principal row present");
    assert_eq!(pr.status, "disabled");

    // `find` still round-trips the service_account row itself — status lives on `principal`
    // alone, unaffected on the `service_account` side. Its own `status` now reads `disabled`.
    let got = repo.find(&sa.principal_id).await.unwrap().expect("row present");
    assert_eq!(got.account.name, "ci-bot");
    assert_eq!(got.status, PrincipalStatus::Disabled);
}

/// AC — `list_by_owner` returns only the accounts owned by the queried node, `ORDER BY
/// created_at, id` (rule 9), and respects `limit`/`offset`. Each entry's `status` is its own
/// principal's — freshly created, `Active` (the JOIN read, mirrors `find`'s coverage above).
#[tokio::test]
async fn list_by_owner_scopes_and_paginates() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let repo = PgServiceAccountRepository::new(db.clone());
    let owner_a = support::seed_org_ref(&db).await;
    let owner_b = support::seed_org_ref(&db).await;

    let (pa1, saa1) = support::sample_sa("a-one", owner_a.clone());
    let (pa2, saa2) = support::sample_sa("a-two", owner_a.clone());
    let (pb1, sab1) = support::sample_sa("b-one", owner_b.clone());
    repo.create(&pa1, &saa1).await.unwrap();
    repo.create(&pa2, &saa2).await.unwrap();
    repo.create(&pb1, &sab1).await.unwrap();

    let page = repo.list_by_owner(&owner_a, 10, 0).await.unwrap();
    assert_eq!(page.len(), 2, "only owner_a's accounts");
    assert!(page.iter().all(|sa| sa.account.owner == owner_a));
    assert!(page.iter().all(|sa| sa.status == PrincipalStatus::Active));
    assert_eq!(page[0].account.name, "a-one");
    assert_eq!(page[1].account.name, "a-two");

    // Archiving one account changes only ITS status — the paginated list reflects it, and the
    // sibling account's own status is untouched.
    repo.set_principal_status(&saa1.principal_id, PrincipalStatus::Disabled).await.unwrap();
    let after_archive = repo.list_by_owner(&owner_a, 10, 0).await.unwrap();
    assert_eq!(after_archive[0].status, PrincipalStatus::Disabled, "a-one is now disabled");
    assert_eq!(after_archive[1].status, PrincipalStatus::Active, "a-two is untouched");

    let first_page = repo.list_by_owner(&owner_a, 1, 0).await.unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].account.name, "a-one");
    let second_page = repo.list_by_owner(&owner_a, 1, 1).await.unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].account.name, "a-two");
}

// --- SMA-445 Task 10: `PgApiKeyRepository` ---------------------------------------------

/// AC (Task 10 brief) — `issue` inserts one row; `find_by_id` round-trips the key exactly
/// (including its exact stored hash bytes) with `status = Active`; `revoke` sets `status =
/// Revoked` + `revoked_at`, and revoking an already-revoked key again is a no-op success —
/// `revoked_at` stays pinned to the FIRST revoke, not overwritten by the second call's `now`.
#[tokio::test]
async fn issue_find_revoke() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (key, hash) = support::sample_key(&sa.principal_id, owner);
    repo.issue(&key, &hash).await.unwrap();

    let (got, stored) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(got, key);
    assert_eq!(got.status, ApiKeyStatus::Active);
    assert_eq!(stored, hash);

    let clock = SystemClock;
    let revoke_at = clock.now();
    repo.revoke(key.id, revoke_at).await.unwrap();
    let (after, _) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(after.status, ApiKeyStatus::Revoked);
    assert_eq!(after.revoked_at, Some(revoke_at));

    repo.revoke(key.id, revoke_at + chrono::Duration::seconds(60)).await.unwrap();
    let (still, _) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(still.status, ApiKeyStatus::Revoked);
    assert_eq!(still.revoked_at, Some(revoke_at), "re-revoking must be a no-op, not bump revoked_at");
}

/// AC (Task 10 brief) — `touch_last_used` sets `last_used_at` on the first call (`NULL` ->
/// `now`); an immediate second call within the same throttle window is a no-op (the guarded
/// UPDATE's `last_used_at < now - throttle_secs` is false); a third call past the window
/// updates again. Time-travel via explicit `now` values (no wall-clock sleep) for determinism.
#[tokio::test]
async fn touch_last_used_is_throttled() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (key, hash) = support::sample_key(&sa.principal_id, owner);
    repo.issue(&key, &hash).await.unwrap();

    let clock = SystemClock;
    let t0 = clock.now();
    repo.touch_last_used(key.id, t0, 60).await.unwrap();
    let (after_first, _) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(after_first.last_used_at, Some(t0));

    let t1 = t0 + chrono::Duration::seconds(1);
    repo.touch_last_used(key.id, t1, 60).await.unwrap();
    let (after_second, _) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(after_second.last_used_at, Some(t0), "throttled touch must not bump last_used_at");

    let t2 = t0 + chrono::Duration::seconds(61);
    repo.touch_last_used(key.id, t2, 60).await.unwrap();
    let (after_third, _) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(after_third.last_used_at, Some(t2), "a touch past the throttle window must update");
}

/// AC (Task 10 brief) — `list_by_service_account` returns only the keys issued to that SA
/// (not another's); the returned `ApiKey`s carry no secret material — the domain type has no
/// plaintext/hash field, so the mapping can only ever expose `prefix` (the safe display
/// fragment), never the raw hash bytes.
#[tokio::test]
async fn list_by_service_account_returns_keys_no_secret_leakage() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p1, sa1) = support::sample_sa("bot-1", owner.clone());
    let (p2, sa2) = support::sample_sa("bot-2", owner.clone());
    sar.create(&p1, &sa1).await.unwrap();
    sar.create(&p2, &sa2).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (key1, hash1) = support::sample_key(&sa1.principal_id, owner.clone());
    let (key2, hash2) = support::sample_key(&sa1.principal_id, owner.clone());
    let (other_key, other_hash) = support::sample_key(&sa2.principal_id, owner);
    repo.issue(&key1, &hash1).await.unwrap();
    repo.issue(&key2, &hash2).await.unwrap();
    repo.issue(&other_key, &other_hash).await.unwrap();

    let listed = repo.list_by_service_account(&sa1.principal_id, 10, 0).await.unwrap();
    assert_eq!(listed.len(), 2, "only sa1's keys");
    assert!(listed.iter().all(|k| k.service_account_id == sa1.principal_id));
    assert!(listed.contains(&key1));
    assert!(listed.contains(&key2));

    for k in &listed {
        assert!(!k.prefix.is_empty());
        assert_ne!(k.prefix.as_bytes(), hash1.as_slice());
        assert_ne!(k.prefix.as_bytes(), hash2.as_slice());
    }

    let ids = repo.list_ids_by_service_account(&sa1.principal_id).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&key1.id));
    assert!(ids.contains(&key2.id));
    assert!(!ids.contains(&other_key.id));
}

/// AC (Task 10 brief, D7) — two keys with the SAME `key_hash` conflict on `uq_api_key_hash`:
/// `Err(RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision))`, and the losing row is
/// never written.
#[tokio::test]
async fn duplicate_hash_conflicts() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (key1, hash) = support::sample_key(&sa.principal_id, owner.clone());
    repo.issue(&key1, &hash).await.unwrap();

    let (key2, _distinct_hash) = support::sample_key(&sa.principal_id, owner);
    let result = repo.issue(&key2, &hash).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision))),
        "expected Conflict(ApiKeyHashCollision), got {result:?}"
    );

    let missing = repo.find_by_id(key2.id).await.unwrap();
    assert!(missing.is_none(), "the losing insert must not have written a row");
}

/// AC (Task 10 brief, Minor review finding) — a key issued with a NON-empty `scope_actions`/
/// `scope_roles` round-trips exactly through the JSON-encoded `scope_actions`/`scope_roles`
/// TEXT columns (the empty-vec path is covered by every other test via `sample_key`; this
/// exercises the serialize/deserialize path those never reach).
#[tokio::test]
async fn issue_find_round_trips_non_empty_scope() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (mut key, hash) = support::sample_key(&sa.principal_id, owner);
    key.scope_actions = vec![Action::CreateProject, Action::ListProjects];
    key.scope_roles = vec!["org_admin".to_string(), "project_viewer".to_string()];
    repo.issue(&key, &hash).await.unwrap();

    let (got, _stored) = repo.find_by_id(key.id).await.unwrap().expect("row present");
    assert_eq!(got, key, "the whole key (incl. non-empty scope narrowing) must round-trip");
    assert_eq!(got.scope_actions, vec![Action::CreateProject, Action::ListProjects]);
    assert_eq!(got.scope_roles, vec!["org_admin".to_string(), "project_viewer".to_string()]);
}
