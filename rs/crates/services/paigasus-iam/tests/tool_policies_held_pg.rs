// SPDX-License-Identifier: Apache-2.0

//! Held tool policy versions and the shared version counter against real Postgres (SMA-781,
//! migration m0015, plan H1 to H8). Each test starts its own container and seeds organizations.
//!
//! Runs against an ephemeral Postgres in Docker. A missing Docker daemon is a hard failure when
//! `PAIGASUS_REQUIRE_DOCKER=1` or `CI` is set.

mod support;

use chrono::{DateTime, Utc};
use paigasus_iam::adapters::persistence::migration::m0015_tool_policy_held_versions::M0015_BACKFILL_SQL;
use paigasus_iam::adapters::persistence::{PgRecompileLock, PgToolPolicies, SeaOrmUnitOfWork};
use paigasus_iam_core::tool_policy::{HeldStatus, HeldVersion, RecompileLock, ReportRejection, ToolPolicyRepository, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus, failed_report};
use paigasus_iam_core::{ConflictKind, RepositoryError, UnitOfWork};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    support::seed_org_ref(db).await.resource_uuid()
}

async fn raw(db: &DatabaseConnection, sql: String) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.map(|_| ())
}

async fn scalar(db: &DatabaseConnection, sql: String, col: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", col)
        .unwrap()
}

fn is_check_violation(e: &sea_orm::DbErr) -> bool {
    e.to_string().contains("violates check constraint")
}

fn live(key: ToolPolicySetKey, n: u64, origin: VersionOrigin) -> ToolPolicyVersion {
    ToolPolicyVersion {
        key,
        version: n,
        source: format!("// v{n}"),
        compiled: vec![1, 2, 3, n as u8],
        compiler_generation: 1,
        schema_revision: 1,
        catalog_revision: match key {
            ToolPolicySetKey::Tenant(_) => Some(0),
            ToolPolicySetKey::Floor => None,
        },
        status: VersionStatus::Live,
        origin,
        created_by: "tester".into(),
        created_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

fn held(key: ToolPolicySetKey, n: u64, base: u64) -> HeldVersion {
    HeldVersion {
        key,
        version: n,
        base_version: base,
        source: format!("// held {n}"),
        compiled: Some(vec![9, n as u8]),
        compiler_generation: 2,
        schema_revision: 1,
        catalog_revision: match key {
            ToolPolicySetKey::Tenant(_) => Some(0),
            ToolPolicySetKey::Floor => None,
        },
        status: HeldStatus::Held,
        report: failed_report(vec![ReportRejection {
            policy_id: Some("p".into()),
            line: Some(4),
            column: Some(2),
            code: "c".into(),
            message: "m".into(),
        }]),
        created_by: "job".into(),
        created_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

struct Env {
    _node: testcontainers_modules::testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    db: DatabaseConnection,
    repo: PgToolPolicies,
    uow: SeaOrmUnitOfWork,
}

async fn env() -> Option<Env> {
    let (node, db) = support::start_migrated_postgres().await?;
    Some(Env {
        _node: node,
        repo: PgToolPolicies::new(db.clone()),
        uow: SeaOrmUnitOfWork::new(db.clone()),
        db,
    })
}

impl Env {
    async fn put_live(&self, v: &ToolPolicyVersion) -> Result<(), RepositoryError> {
        let tx = self.uow.begin().await.unwrap();
        self.repo.lock_state_in(&*tx, &v.key).await.unwrap();
        match self.repo.insert_live_version_in(&*tx, v).await {
            Ok(()) => {
                tx.commit().await.unwrap();
                Ok(())
            }
            Err(e) => {
                drop(tx);
                Err(e)
            }
        }
    }

    async fn put_held(&self, h: &HeldVersion) -> Result<(), RepositoryError> {
        let tx = self.uow.begin().await.unwrap();
        self.repo.lock_state_in(&*tx, &h.key).await.unwrap();
        match self.repo.insert_held_in(&*tx, h).await {
            Ok(()) => {
                tx.commit().await.unwrap();
                Ok(())
            }
            Err(e) => {
                drop(tx);
                Err(e)
            }
        }
    }
}

#[tokio::test]
async fn m0015_backfills_last_version_from_the_stored_versions() {
    let Some(e) = env().await else { return };
    let org = seed_org(&e.db).await;
    let empty_org = seed_org(&e.db).await;
    let key = ToolPolicySetKey::Tenant(org);
    for n in 1..=3 {
        e.put_live(&live(key, n, VersionOrigin::Save)).await.unwrap();
    }
    e.repo.state(&ToolPolicySetKey::Tenant(empty_org)).await.unwrap();
    let tx = e.uow.begin().await.unwrap();
    e.repo.lock_state_in(&*tx, &ToolPolicySetKey::Tenant(empty_org)).await.unwrap();
    tx.commit().await.unwrap();
    raw(&e.db, "UPDATE tool_policy_state SET last_version = 0".into()).await.unwrap();
    raw(&e.db, M0015_BACKFILL_SQL.to_owned()).await.unwrap();
    assert_eq!(e.repo.state(&key).await.unwrap().last_version, 3);
    assert_eq!(e.repo.state(&ToolPolicySetKey::Tenant(empty_org)).await.unwrap().last_version, 0);
    assert_eq!(e.repo.state(&ToolPolicySetKey::Floor).await.unwrap().last_version, 0);
}

#[tokio::test]
async fn a_held_row_takes_the_next_number_and_the_next_save_skips_it() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    e.put_held(&held(key, 2, 1)).await.unwrap();
    let st = e.repo.state(&key).await.unwrap();
    assert_eq!((st.live_version, st.last_version, st.next_version()), (1, 2, 3));
    assert_eq!(e.repo.held(&key).await.unwrap().unwrap().version, 2);
    e.put_live(&live(key, 3, VersionOrigin::Save)).await.unwrap();
    let st = e.repo.state(&key).await.unwrap();
    assert_eq!((st.live_version, st.last_version), (3, 3));
    assert!(e.repo.held(&key).await.unwrap().is_none(), "the new live version discards the held row");
    assert_eq!(
        scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'discarded' AND version = 2".into(), "n").await,
        1
    );
}

#[tokio::test]
async fn a_held_row_with_a_stale_number_is_a_version_conflict() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    for n in [1, 3] {
        let err = e.put_held(&held(key, n, 1)).await.unwrap_err();
        assert!(matches!(err, RepositoryError::Conflict(ConflictKind::ToolPolicyVersion)), "{err:?}");
    }
    assert!(e.repo.held(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn a_new_live_version_discards_the_held_row_from_every_origin() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    let mut next = 2;
    for origin in [
        VersionOrigin::Save,
        VersionOrigin::CatalogChange,
        VersionOrigin::Starter,
        VersionOrigin::Recompile,
        VersionOrigin::Accept,
    ] {
        e.put_held(&held(key, next, next - 1)).await.unwrap();
        assert!(e.repo.held(&key).await.unwrap().is_some());
        e.put_live(&live(key, next + 1, origin)).await.unwrap();
        assert!(e.repo.held(&key).await.unwrap().is_none(), "{origin:?} must discard the held row");
        assert_eq!(e.repo.live(&key).await.unwrap().unwrap().origin, origin);
        next += 2;
    }
}

#[tokio::test]
async fn insert_held_discards_an_older_held_row_first() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    e.put_held(&held(key, 2, 1)).await.unwrap();
    e.put_held(&held(key, 3, 1)).await.unwrap();
    let all = e
        .repo
        .list_held(
            Some(match key {
                ToolPolicySetKey::Tenant(o) => o,
                ToolPolicySetKey::Floor => unreachable!(),
            }),
            50,
            0,
        )
        .await
        .unwrap();
    assert_eq!(all.iter().map(|h| h.version).collect::<Vec<_>>(), vec![3]);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'held'".into(), "n").await, 1);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'discarded'".into(), "n").await, 1);
}

/// The query applies the page in SQL: the floor first, then tenants by org UUID, with LIMIT and
/// OFFSET. Two tenants, so a limit of 1 and an offset of 1 each select one row.
#[tokio::test]
async fn list_held_pages_in_sql_with_the_floor_first() {
    let Some(e) = env().await else { return };
    let mut orgs = [seed_org(&e.db).await, seed_org(&e.db).await];
    for key in orgs.iter().map(|o| ToolPolicySetKey::Tenant(*o)).chain([ToolPolicySetKey::Floor]) {
        e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
        e.put_held(&held(key, 2, 1)).await.unwrap();
    }
    orgs.sort();
    let keys = |rows: Vec<HeldVersion>| rows.into_iter().map(|r| r.key).collect::<Vec<_>>();
    let all = keys(e.repo.list_held(None, 10, 0).await.unwrap());
    assert_eq!(all, vec![ToolPolicySetKey::Floor, ToolPolicySetKey::Tenant(orgs[0]), ToolPolicySetKey::Tenant(orgs[1])]);
    assert_eq!(keys(e.repo.list_held(None, 1, 0).await.unwrap()), vec![ToolPolicySetKey::Floor]);
    assert_eq!(keys(e.repo.list_held(None, 1, 1).await.unwrap()), vec![ToolPolicySetKey::Tenant(orgs[0])]);
    assert_eq!(keys(e.repo.list_held(None, 5, 2).await.unwrap()), vec![ToolPolicySetKey::Tenant(orgs[1])]);
    assert!(e.repo.list_held(None, 5, 3).await.unwrap().is_empty());
    // A tenant list: limit 1 gives the one row, offset 1 gives none.
    assert_eq!(keys(e.repo.list_held(Some(orgs[1]), 1, 0).await.unwrap()), vec![ToolPolicySetKey::Tenant(orgs[1])]);
    assert!(e.repo.list_held(Some(orgs[1]), 1, 1).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_held_row_stores_no_compiled_bytes() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    let mut h = held(key, 2, 1);
    h.compiled = None;
    e.put_held(&h).await.unwrap();
    assert_eq!(e.repo.held(&key).await.unwrap().unwrap(), h);
}

#[tokio::test]
async fn a_live_row_without_compiled_bytes_is_refused_by_the_check() {
    let Some(e) = env().await else { return };
    let org = seed_org(&e.db).await;
    let err = raw(
        &e.db,
        format!(
            "INSERT INTO tool_policy_set_version (kind, org_id, version, source, compiled, compiler_generation, schema_revision, catalog_revision, status, origin, created_by, created_at) \
             VALUES ('tenant', '{org}', 1, 's', NULL, 1, 1, 0, 'live', 'save', 't', now())"
        ),
    )
    .await
    .unwrap_err();
    assert!(is_check_violation(&err) && err.to_string().contains("ck_tool_policy_version_compiled"), "{err}");
}

#[tokio::test]
async fn a_held_row_without_a_base_version_is_refused_by_the_check() {
    let Some(e) = env().await else { return };
    let org = seed_org(&e.db).await;
    for (base, report) in [("NULL", "'{}'::jsonb"), ("1", "NULL")] {
        let err = raw(
            &e.db,
            format!(
                "INSERT INTO tool_policy_set_version (kind, org_id, version, source, compiled, compiler_generation, schema_revision, catalog_revision, status, origin, base_version, report, created_by, created_at) \
                 VALUES ('tenant', '{org}', 2, 's', '\\x00', 1, 1, 0, 'held', 'recompile', {base}, {report}, 't', now())"
            ),
        )
        .await
        .unwrap_err();
        assert!(is_check_violation(&err) && err.to_string().contains("ck_tool_policy_version_held_fields"), "{err}");
    }
}

#[tokio::test]
async fn the_held_report_round_trips_through_jsonb() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    let mut h = held(key, 2, 1);
    h.report.policies.added = vec!["a".into()];
    h.report.schema.changed = vec!["Pgs::x".into(), "Pgs::y".into()];
    e.put_held(&h).await.unwrap();
    assert_eq!(e.repo.held(&key).await.unwrap().unwrap().report, h.report);
}

#[tokio::test]
async fn version_and_live_never_return_a_held_or_discarded_row() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    e.put_held(&held(key, 2, 1)).await.unwrap();
    e.put_held(&held(key, 3, 1)).await.unwrap();
    assert!(e.repo.version(&key, 2).await.unwrap().is_none(), "discarded");
    assert!(e.repo.version(&key, 3).await.unwrap().is_none(), "held");
    assert_eq!(e.repo.live(&key).await.unwrap().unwrap().version, 1);
    assert_eq!(e.repo.version(&key, 1).await.unwrap().unwrap().version, 1);
}

#[tokio::test]
async fn stale_live_versions_lists_the_floor_first_then_tenants() {
    let Some(e) = env().await else { return };
    let mut orgs = vec![seed_org(&e.db).await, seed_org(&e.db).await, seed_org(&e.db).await];
    for org in &orgs {
        e.put_live(&live(ToolPolicySetKey::Tenant(*org), 1, VersionOrigin::Save)).await.unwrap();
    }
    e.put_live(&live(ToolPolicySetKey::Floor, 1, VersionOrigin::Save)).await.unwrap();
    // One tenant is current: generation 2 and schema revision 1 are the running values.
    let current = orgs[0];
    raw(&e.db, format!("UPDATE tool_policy_set_version SET compiler_generation = 2 WHERE org_id = '{current}'"))
        .await
        .unwrap();
    let stale = e.repo.stale_live_versions(2, 1).await.unwrap();
    orgs.sort();
    let mut want = vec![ToolPolicySetKey::Floor];
    want.extend(orgs.iter().filter(|o| **o != current).map(|o| ToolPolicySetKey::Tenant(*o)));
    assert_eq!(stale.iter().map(|v| v.key).collect::<Vec<_>>(), want);
    // A schema revision difference alone is stale too.
    assert_eq!(e.repo.stale_live_versions(1, 2).await.unwrap().len(), 4);
}

#[tokio::test]
async fn max_stored_revisions_reads_every_status() {
    let Some(e) = env().await else { return };
    assert_eq!(e.repo.max_stored_revisions().await.unwrap(), (0, 0));
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    let mut h = held(key, 2, 1);
    h.compiler_generation = 9;
    h.schema_revision = 4;
    e.put_held(&h).await.unwrap();
    e.put_held(&held(key, 3, 1)).await.unwrap(); // discards the generation 9 row
    assert_eq!(e.repo.max_stored_revisions().await.unwrap(), (9, 4), "a discarded row counts");
}

#[tokio::test]
async fn proposal_exists_sees_held_and_discarded_rows_only_for_the_same_base() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    assert!(!e.repo.proposal_exists(&key, 1, 2, 1).await.unwrap());
    e.put_held(&held(key, 2, 1)).await.unwrap();
    assert!(e.repo.proposal_exists(&key, 1, 2, 1).await.unwrap(), "held");
    assert!(!e.repo.proposal_exists(&key, 2, 2, 1).await.unwrap(), "other base");
    assert!(!e.repo.proposal_exists(&key, 1, 3, 1).await.unwrap(), "other generation");
    assert!(!e.repo.proposal_exists(&key, 1, 2, 2).await.unwrap(), "other schema revision");
    let tx = e.uow.begin().await.unwrap();
    assert!(e.repo.discard_held_in(&*tx, &key, 2).await.unwrap());
    tx.commit().await.unwrap();
    assert!(e.repo.proposal_exists(&key, 1, 2, 1).await.unwrap(), "discarded");
    let other = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    assert!(!e.repo.proposal_exists(&other, 1, 2, 1).await.unwrap(), "other set");
}

#[tokio::test]
async fn discard_held_of_an_absent_number_is_false_and_changes_nothing() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    e.put_held(&held(key, 2, 1)).await.unwrap();
    let tx = e.uow.begin().await.unwrap();
    assert!(!e.repo.discard_held_in(&*tx, &key, 7).await.unwrap());
    assert!(!e.repo.discard_held_in(&*tx, &key, 1).await.unwrap(), "a live number is no held row");
    tx.commit().await.unwrap();
    assert_eq!(e.repo.held(&key).await.unwrap().unwrap().version, 2);
    assert_eq!(e.repo.live(&key).await.unwrap().unwrap().version, 1);
}

#[tokio::test]
async fn the_next_version_skips_a_number_that_an_old_replica_wrote() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    // An old replica sets live_version and leaves last_version behind.
    raw(&e.db, "UPDATE tool_policy_state SET live_version = 5, last_version = 4 WHERE kind = 'tenant'".into())
        .await
        .unwrap();
    assert_eq!(e.repo.state(&key).await.unwrap().next_version(), 6);
    e.put_live(&live(key, 6, VersionOrigin::Save)).await.unwrap();
    let st = e.repo.state(&key).await.unwrap();
    assert_eq!((st.live_version, st.last_version), (6, 6));
}

#[tokio::test]
async fn a_rolled_back_held_insert_leaves_no_row_and_the_old_counter() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    let tx = e.uow.begin().await.unwrap();
    e.repo.lock_state_in(&*tx, &key).await.unwrap();
    e.repo.insert_held_in(&*tx, &held(key, 2, 1)).await.unwrap();
    drop(tx); // an uncommitted transaction rolls back
    assert!(e.repo.held(&key).await.unwrap().is_none());
    assert_eq!(e.repo.state(&key).await.unwrap().last_version, 1);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE version = 2".into(), "n").await, 0);
}

/// `down` removes the held and discarded rows above the live version, so a PR 2 save
/// (`live_version + 1`) does not hit the unique index.
#[tokio::test]
async fn down_removes_held_rows_above_the_live_version_and_a_pr2_save_succeeds() {
    use paigasus_iam::adapters::persistence::Migrator;
    use sea_orm_migration::MigratorTrait;
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    e.put_live(&live(key, 1, VersionOrigin::Save)).await.unwrap();
    e.put_held(&held(key, 2, 1)).await.unwrap(); // becomes discarded
    e.put_held(&held(key, 3, 1)).await.unwrap(); // stays held
    // The count depends on the number of later migrations: raise it when one is added.
    Migrator::down(&e.db, Some(2)).await.unwrap(); // m0016, then m0015
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version".into(), "n").await, 1, "only the live row stays");
    raw(&e.db, "UPDATE tool_policy_set_version SET status = 'superseded'".into()).await.unwrap();
    let org = match key {
        ToolPolicySetKey::Tenant(o) => o,
        ToolPolicySetKey::Floor => unreachable!(),
    };
    raw(
        &e.db,
        format!(
            "INSERT INTO tool_policy_set_version (kind, org_id, version, source, compiled, compiler_generation, schema_revision, catalog_revision, status, origin, created_by, created_at) \
             VALUES ('tenant', '{org}', 2, 's', '\\x00', 1, 1, 0, 'live', 'save', 't', now())"
        ),
    )
    .await
    .expect("a PR 2 save takes live_version + 1");
}

/// Postgres frees the lock when the dropped guard's rollback reaches the connection. Poll a
/// short time, because the drop path sends the rollback without waiting.
async fn acquire_within(lock: &PgRecompileLock) -> bool {
    for _ in 0..50 {
        if let Some(g) = lock.try_acquire().await.unwrap() {
            g.release().await.unwrap();
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn a_second_try_acquire_gets_none_until_the_first_guard_is_released() {
    let Some(e) = env().await else { return };
    let lock = PgRecompileLock::new(e.db.clone());
    let first = lock.try_acquire().await.unwrap().expect("the first acquire gets the lock");
    assert!(lock.try_acquire().await.unwrap().is_none(), "a second holder must not get the lock");
    first.release().await.unwrap();
    let again = lock.try_acquire().await.unwrap();
    assert!(again.is_some(), "the lock is free after release");
    again.unwrap().release().await.unwrap();
}

/// Backstop only: the job releases explicitly. A drop must still free the lock.
#[tokio::test]
async fn a_dropped_guard_frees_the_lock() {
    let Some(e) = env().await else { return };
    let lock = PgRecompileLock::new(e.db.clone());
    let guard = lock.try_acquire().await.unwrap().expect("acquire");
    assert!(lock.try_acquire().await.unwrap().is_none());
    drop(guard);
    assert!(acquire_within(&lock).await, "a dropped guard must free the lock");
}

#[tokio::test]
async fn the_lock_does_not_block_a_tool_policy_write() {
    let Some(e) = env().await else { return };
    let key = ToolPolicySetKey::Tenant(seed_org(&e.db).await);
    let lock = PgRecompileLock::new(e.db.clone());
    let guard = lock.try_acquire().await.unwrap().expect("acquire");
    tokio::time::timeout(std::time::Duration::from_secs(10), e.put_live(&live(key, 1, VersionOrigin::Save)))
        .await
        .expect("a write must not wait for the re-compile lock")
        .unwrap();
    assert_eq!(e.repo.state(&key).await.unwrap().live_version, 1);
    guard.release().await.unwrap();
}

/// A compiler that accepts every source. The Cedar text is the source plus `suffix`, so a
/// non-empty `suffix` makes an output that differs from the stored one. With a `gate`, each
/// compile signals `started` and then waits for a permit of `release`.
struct GatedCompiler {
    generation: u32,
    suffix: &'static str,
    gate: Option<std::sync::Arc<CompileGate>>,
}

/// Holds a compile until the test lets it go.
struct CompileGate {
    started: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl paigasus_iam_core::tool_policy::PolicyCompiler for GatedCompiler {
    fn generation(&self) -> u32 {
        self.generation
    }

    async fn compile(&self, req: &paigasus_iam_core::tool_policy::CompileRequest) -> Result<paigasus_iam_core::tool_policy::CompileOutcome, paigasus_iam_core::tool_policy::CompilerFailure> {
        if let Some(gate) = &self.gate {
            gate.started.notify_one();
            gate.release.acquire().await.expect("the gate semaphore stays open").forget();
        }
        Ok(paigasus_iam_core::tool_policy::CompileOutcome::Accepted(paigasus_iam_core::tool_policy::CompiledOutput {
            cedar_policies: format!("{}{}", req.source, self.suffix),
            cedar_schema_json: req.schema_json.clone(),
            conditions: Vec::new(),
        }))
    }
}

/// An authorizer that allows every request.
struct AllowAll;

#[async_trait::async_trait]
impl paigasus_iam_core::Authorizer for AllowAll {
    async fn is_authorized(&self, _req: &paigasus_iam_core::AccessRequest) -> Result<paigasus_iam_core::Decision, paigasus_iam_core::AuthzError> {
        Ok(paigasus_iam_core::Decision {
            effect: paigasus_iam_core::Effect::Allow,
            determining_policies: vec![],
        })
    }
}

/// A service on the Postgres adapters, with its own `PgRecompileLock` (so its lock is its own
/// connection, like another replica) and an allow-all authorizer.
fn service_with(db: &DatabaseConnection, compiler: GatedCompiler) -> paigasus_iam::adapters::http::ToolPolicySvc {
    use paigasus_iam::adapters::authz::Generations;
    use paigasus_iam::adapters::clock::SystemClock;
    use paigasus_iam::adapters::id::KernelIdGenerator;
    use paigasus_iam::adapters::persistence::{PgAuditLog, PgOrganizationRepository, PgOutbox};
    use paigasus_iam::application::authorize::Authorize;
    use paigasus_iam::application::tool_policies::{ToolPolicyLimits, ToolPolicyService, ToolPolicyServiceDeps};
    use paigasus_iam_core::tool_policy::CatalogLimits;

    ToolPolicyService::new(ToolPolicyServiceDeps {
        repo: std::sync::Arc::new(PgToolPolicies::new(db.clone())),
        orgs: std::sync::Arc::new(PgOrganizationRepository::new(db.clone(), Generations::memory())),
        compiler: std::sync::Arc::new(compiler),
        authorize: Authorize::new(std::sync::Arc::new(AllowAll)),
        uow: std::sync::Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        audit: std::sync::Arc::new(PgAuditLog::new(db.clone())),
        outbox: std::sync::Arc::new(PgOutbox::new(false)),
        recompile_lock: std::sync::Arc::new(PgRecompileLock::new(db.clone())),
        limits: ToolPolicyLimits {
            catalog: CatalogLimits::default(),
            max_source_bytes: 65_536,
            max_window_ms: 3_600_000,
            max_catalog_response_bytes: 2_097_152,
            history_conditions: false,
        },
        ids: KernelIdGenerator,
        clock: SystemClock,
    })
}

/// Two replicas start the job on one stale floor at the same time. One runs and writes the held
/// row; the other sees the lock and does nothing. A gate fixes the order, not a delay: run `a`
/// holds the lock inside its compile until run `b` has returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_runs_write_one_held_row() {
    use paigasus_iam::application::tool_policies::{RecompileOutcome, RecompileRun, WriteResult};

    let Some(e) = env().await else { return };
    // Generation 1 saves the floor.
    let saver = service_with(
        &e.db,
        GatedCompiler {
            generation: 1,
            suffix: "",
            gate: None,
        },
    );
    let actor = paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap();
    let saved = saver.put_floor(&actor, "// floor\n", 0).await.expect("save the floor");
    assert!(matches!(saved, WriteResult::Accepted(_)));
    // Generation 2 makes the floor stale and compiles it to another output.
    let gate = std::sync::Arc::new(CompileGate {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let changed = |gate| GatedCompiler {
        generation: 2,
        suffix: "// changed\n",
        gate,
    };
    let a = service_with(&e.db, changed(Some(gate.clone())));
    let b = service_with(&e.db, changed(None));
    let run_b = async {
        // Run `a` is in its compile, so it holds the lock.
        tokio::time::timeout(std::time::Duration::from_secs(60), gate.started.notified())
            .await
            .expect("run a must reach its compile");
        let rb = b.run_recompile_at_boot(std::future::pending()).await;
        gate.release.add_permits(1);
        rb
    };
    let (ra, rb) = tokio::join!(a.run_recompile_at_boot(std::future::pending()), run_b);
    let (ra, rb) = (ra.expect("run a"), rb.expect("run b"));
    assert_eq!(rb, RecompileRun::AlreadyRunning, "run b starts while run a holds the lock");
    let RecompileRun::Ran(results) = ra else { panic!("expected run a to run, got {ra:?}") };
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].outcome, RecompileOutcome::HeldDifferent { .. }), "{:?}", results[0].outcome);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'held'".into(), "n").await, 1);
    // The lock is free again.
    let lock = PgRecompileLock::new(e.db.clone());
    assert!(acquire_within(&lock).await);
}

/// The output of an echo compiler for `source`, at schema revision 1: the current schema without
/// the two attributes that revision 2 added (SMA-855 spec section 6.13.1).
fn revision_1_output(key: ToolPolicySetKey, source: &str) -> paigasus_iam_core::tool_policy::CompiledOutput {
    use paigasus_iam_core::tool_policy::{CompiledOutput, base_schema_json, tenant_schema_json};
    let mut schema = match key {
        ToolPolicySetKey::Tenant(_) => tenant_schema_json(&[]),
        ToolPolicySetKey::Floor => base_schema_json(),
    };
    let gateway = &mut schema["Pgs::Gateway"];
    for action in gateway["actions"].as_object_mut().unwrap().values_mut() {
        assert!(action["appliesTo"]["context"]["attributes"].as_object_mut().unwrap().remove("max_severity").is_some());
    }
    assert!(gateway["commonTypes"]["CallInfo"]["attributes"].as_object_mut().unwrap().remove("hosts").is_some());
    CompiledOutput {
        cedar_policies: source.to_owned(),
        cedar_schema_json: schema,
        conditions: Vec::new(),
    }
}

/// A live version 1 at compiler generation 2 and schema revision 1, with the revision-1 output
/// of `source` as its compiled bytes: a set that the release before SMA-855 saved.
fn revision_1_live(key: ToolPolicySetKey, source: &str) -> ToolPolicyVersion {
    use paigasus_proto::paigasus::iam::v1 as pb;
    use prost::Message;
    let out = revision_1_output(key, source);
    let (kind, org_id) = match key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    let compiled = pb::CompiledToolPolicySet {
        kind: kind as i32,
        org_id,
        version: 1,
        compiler_generation: 2,
        schema_revision: 1,
        catalog_revision: 0,
        cedar_policies: out.cedar_policies,
        cedar_schema: out.cedar_schema_json.to_string(),
        conditions: Vec::new(),
        max_window_ms: 3_600_000,
        catalog_names: Vec::new(),
    }
    .encode_to_vec();
    ToolPolicyVersion {
        source: source.to_owned(),
        compiled,
        compiler_generation: 2,
        schema_revision: 1,
        ..live(key, 1, VersionOrigin::Save)
    }
}

/// SMA-855 spec section 7.5 and D28: the release bumps the compiler generation (2 to 3) and the
/// schema revision (1 to 2), and the schema only adds attributes. One boot run then writes an
/// identical live version for every set. No held row exists, every set is current, and a
/// catalog write succeeds.
#[tokio::test]
async fn an_additive_revision_and_generation_bump_keeps_every_live_set_current() {
    use paigasus_iam::application::error::TenancyError;
    use paigasus_iam::application::tool_policies::{RecompileOutcome, RecompileRun, SetRecompile, WriteResult};
    use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, TOOL_SCHEMA_REVISION, ToolKind};
    use paigasus_proto::paigasus::iam::v1 as pb;
    use prost::Message;

    const FLOOR: &str = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n";
    assert_eq!(TOOL_SCHEMA_REVISION, 2, "this test is the revision-2 rollout");

    let Some(e) = env().await else { return };
    let mut orgs = [seed_org(&e.db).await, seed_org(&e.db).await];
    orgs.sort();
    e.put_live(&revision_1_live(ToolPolicySetKey::Floor, FLOOR)).await.unwrap();
    for org in orgs {
        e.put_live(&revision_1_live(ToolPolicySetKey::Tenant(org), STARTER_TOOL_POLICY_SOURCE)).await.unwrap();
    }

    let svc = service_with(
        &e.db,
        GatedCompiler {
            generation: 3,
            suffix: "",
            gate: None,
        },
    );
    let actor = paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap();
    let search = || Some(serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}));

    // The control: before the run, every set is stale, so a catalog write is refused.
    let refused = svc.put_tool(&actor, orgs[0], "search", ToolKind::Function, search(), 0).await;
    assert!(matches!(refused, Err(TenancyError::ToolPolicyRecompilePending)), "{refused:?}");

    let run = svc.run_recompile_at_boot(std::future::pending()).await.expect("the run");
    let identical = |key| SetRecompile {
        key,
        base_version: 1,
        outcome: RecompileOutcome::Identical { version: 2 },
    };
    assert_eq!(
        run,
        RecompileRun::Ran(vec![
            identical(ToolPolicySetKey::Floor),
            identical(ToolPolicySetKey::Tenant(orgs[0])),
            identical(ToolPolicySetKey::Tenant(orgs[1]))
        ])
    );

    assert_eq!(
        scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status IN ('held', 'discarded')".into(), "n").await,
        0
    );
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'live'".into(), "n").await, 3);
    assert_eq!(
        scalar(
            &e.db,
            "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'live' AND version = 2 AND compiler_generation = 3 AND schema_revision = 2".into(),
            "n"
        )
        .await,
        3
    );
    assert!(e.repo.stale_live_versions(3, TOOL_SCHEMA_REVISION).await.unwrap().is_empty(), "every live set is current");
    for key in [ToolPolicySetKey::Floor, ToolPolicySetKey::Tenant(orgs[0]), ToolPolicySetKey::Tenant(orgs[1])] {
        let v = e.repo.live(&key).await.unwrap().expect("a live version");
        assert_eq!((v.version, v.origin, v.compiler_generation, v.schema_revision), (2, VersionOrigin::Recompile, 3, 2), "{key:?}");
        let set = pb::CompiledToolPolicySet::decode(v.compiled.as_slice()).expect("decode");
        assert_eq!((set.compiler_generation, set.schema_revision), (3, 2), "{key:?}");
        let schema: serde_json::Value = serde_json::from_str(&set.cedar_schema).unwrap();
        assert!(
            schema["Pgs::Gateway"]["commonTypes"]["CallInfo"]["attributes"]["hosts"].is_object(),
            "{key:?} stores the revision-2 schema"
        );
    }

    // A catalog write now succeeds.
    let written = svc.put_tool(&actor, orgs[0], "search", ToolKind::Function, search(), 0).await.expect("the catalog write");
    assert!(matches!(written, WriteResult::Accepted(_)), "{written:?}");
}
