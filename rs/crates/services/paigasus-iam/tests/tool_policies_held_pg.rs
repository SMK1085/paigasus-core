// SPDX-License-Identifier: Apache-2.0

//! Held tool policy versions and the shared version counter against real Postgres (SMA-781,
//! migration m0015, plan H1 to H8). Each test starts its own container and seeds organizations.
//!
//! Runs against an ephemeral Postgres in Docker. A missing Docker daemon is a hard failure when
//! `PAIGASUS_REQUIRE_DOCKER=1` or `CI` is set.

mod support;

use chrono::{DateTime, Utc};
use paigasus_iam::adapters::persistence::migration::m0015_tool_policy_held_versions::M0015_BACKFILL_SQL;
use paigasus_iam::adapters::persistence::{PgToolPolicies, SeaOrmUnitOfWork};
use paigasus_iam_core::tool_policy::{HeldStatus, HeldVersion, ReportRejection, ToolPolicyRepository, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus, failed_report};
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
        .list_held(Some(match key {
            ToolPolicySetKey::Tenant(o) => o,
            ToolPolicySetKey::Floor => unreachable!(),
        }))
        .await
        .unwrap();
    assert_eq!(all.iter().map(|h| h.version).collect::<Vec<_>>(), vec![3]);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'held'".into(), "n").await, 1);
    assert_eq!(scalar(&e.db, "SELECT count(*) AS n FROM tool_policy_set_version WHERE status = 'discarded'".into(), "n").await, 1);
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
    Migrator::down(&e.db, Some(1)).await.unwrap();
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
