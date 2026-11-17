// SPDX-License-Identifier: Apache-2.0

//! `PgToolPolicies` and migration m0013 against real Postgres (SMA-774, spec sections 4.2, 4.4,
//! 4.5 and 6). Each test starts its own container and seeds one organization.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use chrono::{DateTime, Utc};
use paigasus_iam::adapters::persistence::uow::recover_txn;
use paigasus_iam::adapters::persistence::{PgToolPolicies, SeaOrmUnitOfWork};
use paigasus_iam_core::tool_policy::{ToolEntry, ToolKind, ToolPolicyRepository, ToolPolicySetKey, ToolPolicyVersion, ToolSource, VersionOrigin, VersionStatus, parse_tool_name};
use paigasus_iam_core::{ConflictKind, RepositoryError, UnitOfWork};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, SqlErr, Statement};
use serde_json::json;
use uuid::Uuid;

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    support::seed_org_ref(db).await.resource_uuid()
}

fn tool(name: &str, kind: ToolKind, schema: Option<serde_json::Value>, cedar: serde_json::Value) -> ToolEntry {
    ToolEntry {
        name: parse_tool_name(None, name).unwrap(),
        kind,
        input_schema: schema,
        cedar_input_type: cedar,
        dropped_fields: vec![],
        source: ToolSource::Manual,
    }
}

fn version(key: ToolPolicySetKey, n: u64, status: VersionStatus, at: DateTime<Utc>) -> ToolPolicyVersion {
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
        status,
        origin: VersionOrigin::Save,
        created_by: "tester".into(),
        created_at: at,
    }
}

async fn raw(db: &DatabaseConnection, sql: String) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.map(|_| ())
}

fn raw_version_sql(kind: &str, org: Option<Uuid>, version: i64, status: &str) -> String {
    let org_sql = org.map_or("NULL".to_string(), |o| format!("'{o}'"));
    let rev = if kind == "floor" { "NULL" } else { "0" };
    format!(
        "INSERT INTO tool_policy_set_version (kind, org_id, version, source, compiled, compiler_generation, schema_revision, catalog_revision, status, origin, created_by, created_at) \
         VALUES ('{kind}', {org_sql}, {version}, 's', '\\x00', 1, 1, {rev}, '{status}', 'save', 't', now())"
    )
}

fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

fn is_check_violation(e: &sea_orm::DbErr) -> bool {
    e.to_string().contains("violates check constraint")
}

#[tokio::test]
async fn state_is_zero_for_an_org_with_no_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let st = repo.state(&ToolPolicySetKey::Tenant(org)).await.unwrap();
    assert_eq!((st.catalog_revision, st.live_version), (0, 0));
    assert!(repo.live(&ToolPolicySetKey::Tenant(org)).await.unwrap().is_none());
    assert!(repo.catalog(org).await.unwrap().is_empty());
}

/// `catalog_snapshot` reads the tools and the revision in one snapshot. A writer adds one tool per
/// write, so revision N always holds N tools. Readers run during the writes. Each read must give a
/// matching pair. Two plain reads can give N+1 tools with revision N.
#[tokio::test]
async fn catalog_snapshot_pairs_the_tools_with_their_revision_during_writes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    assert_eq!(repo.catalog_snapshot(org).await.unwrap(), (vec![], 0), "no state row gives revision 0");

    const WRITES: usize = 150;
    let writer = {
        let repo = repo.clone();
        let uow = SeaOrmUnitOfWork::new(db.clone());
        tokio::spawn(async move {
            for i in 0..WRITES {
                let tx = uow.begin().await.unwrap();
                repo.lock_state_in(&*tx, &ToolPolicySetKey::Tenant(org)).await.unwrap();
                repo.apply_catalog_in(&*tx, org, &[tool(&format!("t{i:04}"), ToolKind::Custom, None, json!({}))], &[], Utc::now())
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
        })
    };
    let mut readers = Vec::new();
    for _ in 0..4 {
        let repo = repo.clone();
        readers.push(tokio::spawn(async move {
            let mut reads = 0usize;
            loop {
                let (tools, revision) = repo.catalog_snapshot(org).await.unwrap();
                assert_eq!(tools.len() as u64, revision, "the tools and the revision come from two snapshots");
                reads += 1;
                if revision as usize == WRITES {
                    return reads;
                }
            }
        }));
    }
    writer.await.unwrap();
    let mut total = 0;
    for r in readers {
        total += r.await.unwrap();
    }
    // No read/write ratio is asserted: reader throughput is not a property of correct code. Each
    // reader loops until it sees the final revision, so it made at least one read.
    assert!(total >= 4, "every reader made at least one read ({total} reads)");
    let (tools, revision) = repo.catalog_snapshot(org).await.unwrap();
    assert_eq!((tools.len(), revision), (WRITES, WRITES as u64));
}

#[tokio::test]
async fn apply_catalog_upserts_deletes_and_bumps_the_revision() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let t0 = Utc::now();

    let tx = uow.begin().await.unwrap();
    let rev = repo
        .apply_catalog_in(
            &*tx,
            org,
            &[
                tool("zeta", ToolKind::Function, Some(json!({"type":"object"})), json!({"a": 1})),
                tool("alpha", ToolKind::Custom, None, json!({})),
            ],
            &[],
            t0,
        )
        .await
        .unwrap();
    assert_eq!(rev, 1);
    tx.commit().await.unwrap();

    let cat = repo.catalog(org).await.unwrap();
    let keys: Vec<_> = cat.iter().map(|s| s.entry.name.name_key.clone()).collect();
    assert_eq!(keys, ["alpha", "zeta"], "sorted by name_key");
    assert_eq!(cat[1].entry.input_schema, Some(json!({"type":"object"})));
    assert_eq!(cat[0].entry.kind, ToolKind::Custom);
    assert_eq!(repo.state(&ToolPolicySetKey::Tenant(org)).await.unwrap().catalog_revision, 1);

    let t1 = t0 + chrono::Duration::seconds(60);
    let tx = uow.begin().await.unwrap();
    let rev = repo
        .apply_catalog_in(
            &*tx,
            org,
            &[tool("zeta", ToolKind::Function, Some(json!({"type":"object"})), json!({"a": 2}))],
            &["alpha".to_string()],
            t1,
        )
        .await
        .unwrap();
    assert_eq!(rev, 2);
    tx.commit().await.unwrap();

    let cat = repo.catalog(org).await.unwrap();
    assert_eq!(cat.len(), 1);
    assert_eq!(cat[0].entry.cedar_input_type, json!({"a": 2}));
    assert_eq!(cat[0].created_at.timestamp_micros(), t0.timestamp_micros(), "created_at survives an upsert");
    assert_eq!(cat[0].updated_at.timestamp_micros(), t1.timestamp_micros());
    assert_eq!(repo.state(&ToolPolicySetKey::Tenant(org)).await.unwrap().catalog_revision, 2);
}

#[tokio::test]
async fn insert_live_version_supersedes_the_previous_live_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let key = ToolPolicySetKey::Tenant(org);
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let now = Utc::now();

    for n in [1u64, 2] {
        let tx = uow.begin().await.unwrap();
        repo.lock_state_in(&*tx, &key).await.unwrap();
        repo.insert_live_version_in(&*tx, &version(key, n, VersionStatus::Live, now)).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(repo.state(&key).await.unwrap().live_version, 2);
    let live = repo.live(&key).await.unwrap().unwrap();
    assert_eq!((live.version, live.status), (2, VersionStatus::Live));
    assert_eq!(live.compiled, vec![1, 2, 3, 2]);
    assert_eq!(live.catalog_revision, Some(0));
    let v1 = repo.version(&key, 1).await.unwrap().unwrap();
    assert_eq!(v1.status, VersionStatus::Superseded);
    assert_eq!(v1.origin, VersionOrigin::Save);
    assert!(repo.version(&key, 9).await.unwrap().is_none());
}

#[tokio::test]
async fn the_floor_has_a_version_with_no_catalog_revision() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let key = ToolPolicySetKey::Floor;
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key).await.unwrap();
    repo.insert_live_version_in(&*tx, &version(key, 1, VersionStatus::Live, Utc::now())).await.unwrap();
    tx.commit().await.unwrap();
    let live = repo.live(&key).await.unwrap().unwrap();
    assert_eq!((live.key, live.version, live.catalog_revision), (key, 1, None));
}

#[tokio::test]
async fn a_second_live_row_is_refused_by_the_index() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    raw(&db, raw_version_sql("tenant", Some(org), 1, "live")).await.unwrap();
    let err = raw(&db, raw_version_sql("tenant", Some(org), 2, "live")).await.unwrap_err();
    assert!(is_unique_violation(&err), "{err}");
    assert!(err.to_string().contains("uq_tool_policy_set_version_live"), "{err}");
    // The floor is one set too, although its org_id is NULL.
    raw(&db, raw_version_sql("floor", None, 1, "live")).await.unwrap();
    let err = raw(&db, raw_version_sql("floor", None, 2, "live")).await.unwrap_err();
    assert!(is_unique_violation(&err), "{err}");
}

#[tokio::test]
async fn a_second_held_row_is_refused_by_the_index() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    raw(&db, raw_version_sql("tenant", Some(org), 1, "held")).await.unwrap();
    let err = raw(&db, raw_version_sql("tenant", Some(org), 2, "held")).await.unwrap_err();
    assert!(is_unique_violation(&err), "{err}");
    assert!(err.to_string().contains("uq_tool_policy_set_version_held"), "{err}");
    // The same version number twice is refused by the main index.
    let err = raw(&db, raw_version_sql("tenant", Some(org), 1, "superseded")).await.unwrap_err();
    assert!(err.to_string().contains("uq_tool_policy_set_version"), "{err}");
}

#[tokio::test]
async fn a_floor_row_with_an_org_id_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let err = raw(&db, raw_version_sql("floor", Some(org), 1, "live")).await.unwrap_err();
    assert!(is_check_violation(&err), "{err}");
    let err = raw(&db, format!("INSERT INTO tool_policy_state (set_key, kind, org_id) VALUES ('floor2', 'floor', '{org}')"))
        .await
        .unwrap_err();
    assert!(is_check_violation(&err), "{err}");
    // A tenant row with a wrong set_key is refused too.
    let err = raw(&db, format!("INSERT INTO tool_policy_state (set_key, kind, org_id) VALUES ('tenant:other', 'tenant', '{org}')"))
        .await
        .unwrap_err();
    assert!(is_check_violation(&err), "{err}");
}

#[tokio::test]
async fn a_function_tool_with_no_schema_is_stored_and_a_custom_tool_with_a_schema_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let tx = uow.begin().await.unwrap();
    repo.apply_catalog_in(&*tx, org, &[tool("builtin", ToolKind::Function, None, json!({}))], &[], Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let cat = repo.catalog(org).await.unwrap();
    assert_eq!((cat[0].entry.kind, cat[0].entry.input_schema.clone()), (ToolKind::Function, None));

    let tx = uow.begin().await.unwrap();
    let err = repo
        .apply_catalog_in(&*tx, org, &[tool("bad", ToolKind::Custom, Some(json!({"type":"object"})), json!({}))], &[], Utc::now())
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::Backend(_)), "{err:?}");
    drop(tx);
    assert_eq!(repo.catalog(org).await.unwrap().len(), 1, "the refused row is not stored");
}

#[tokio::test]
async fn version_does_not_return_a_held_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let key = ToolPolicySetKey::Tenant(org);
    raw(&db, raw_version_sql("tenant", Some(org), 3, "held")).await.unwrap();
    raw(&db, raw_version_sql("tenant", Some(org), 4, "discarded")).await.unwrap();
    let repo = PgToolPolicies::new(db.clone());
    assert!(repo.version(&key, 3).await.unwrap().is_none());
    assert!(repo.version(&key, 4).await.unwrap().is_none());
}

#[tokio::test]
async fn lock_state_blocks_a_second_locker_until_commit() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let key = ToolPolicySetKey::Tenant(org);
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    // Make the row first, so the second locker waits in the SELECT … FOR UPDATE.
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key).await.unwrap();
    tx.commit().await.unwrap();

    let first = uow.begin().await.unwrap();
    repo.lock_state_in(&*first, &key).await.unwrap();
    let first_pid = support::race::backend_pid(recover_txn(&*first).unwrap()).await;

    let (db2, repo2) = (db.clone(), repo.clone());
    let mut second = tokio::spawn(async move {
        let tx = SeaOrmUnitOfWork::new(db2).begin().await.unwrap();
        let st = repo2.lock_state_in(&*tx, &key).await.unwrap();
        tx.commit().await.unwrap();
        st.catalog_revision
    });
    support::race::expect_racer_blocked(&db, first_pid, &mut second, "select \"tool_policy_state\"%", |r| format!("{r:?}")).await;

    // The first writer bumps the revision. The second must see it after the lock.
    let rev = repo.apply_catalog_in(&*first, org, &[], &[], Utc::now()).await.unwrap();
    assert_eq!(rev, 1);
    first.commit().await.unwrap();
    assert_eq!(second.await.unwrap(), 1, "the second locker reads the committed state");
}

#[tokio::test]
async fn deleting_an_org_cascades() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let key = ToolPolicySetKey::Tenant(org);
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key).await.unwrap();
    repo.apply_catalog_in(&*tx, org, &[tool("t", ToolKind::Function, None, json!({}))], &[], Utc::now()).await.unwrap();
    repo.insert_live_version_in(&*tx, &version(key, 1, VersionStatus::Live, Utc::now())).await.unwrap();
    tx.commit().await.unwrap();

    raw(&db, format!("DELETE FROM organization WHERE id = '{org}'")).await.unwrap();
    for table in ["tool_catalog_entry", "tool_policy_state", "tool_policy_set_version"] {
        let row = db
            .query_one_raw(Statement::from_string(DbBackend::Postgres, format!("SELECT count(*)::bigint AS n FROM {table} WHERE org_id = '{org}'")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0, "{table}");
    }
}

#[tokio::test]
async fn the_floor_state_row_is_seeded() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let repo = PgToolPolicies::new(db.clone());
    let st = repo.state(&ToolPolicySetKey::Floor).await.unwrap();
    assert_eq!((st.catalog_revision, st.live_version), (0, 0));
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM tool_policy_state WHERE set_key = 'floor' AND kind = 'floor'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1);
}

/// Commits version 1 and 2 of a tenant set, so that the live version is 2.
async fn set_at_version_two(db: &DatabaseConnection) -> (ToolPolicySetKey, PgToolPolicies, SeaOrmUnitOfWork) {
    let org = seed_org(db).await;
    let key = ToolPolicySetKey::Tenant(org);
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    for n in [1u64, 2] {
        let tx = uow.begin().await.unwrap();
        repo.lock_state_in(&*tx, &key).await.unwrap();
        repo.insert_live_version_in(&*tx, &version(key, n, VersionStatus::Live, Utc::now())).await.unwrap();
        tx.commit().await.unwrap();
    }
    (key, repo, uow)
}

async fn assert_unchanged_at_two(repo: &PgToolPolicies, key: &ToolPolicySetKey) {
    assert_eq!(repo.state(key).await.unwrap().live_version, 2);
    let live = repo.live(key).await.unwrap().unwrap();
    assert_eq!((live.version, live.status), (2, VersionStatus::Live));
    assert_eq!(repo.version(key, 1).await.unwrap().unwrap().status, VersionStatus::Superseded);
    assert!(repo.version(key, 3).await.unwrap().is_none());
}

#[tokio::test]
async fn a_stale_writer_gets_the_version_conflict_and_changes_nothing() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (key, repo, uow) = set_at_version_two(&db).await;
    // The stale writer read live_version 1 and wants version 2 again. It does not lock.
    let tx = uow.begin().await.unwrap();
    let err = repo.insert_live_version_in(&*tx, &version(key, 2, VersionStatus::Live, Utc::now())).await.unwrap_err();
    assert!(matches!(err, RepositoryError::Conflict(ConflictKind::ToolPolicyVersion)), "{err:?}");
    drop(tx);
    assert_unchanged_at_two(&repo, &key).await;
}

#[tokio::test]
async fn a_version_that_is_not_the_next_one_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (key, repo, uow) = set_at_version_two(&db).await;
    for n in [4u64, 1] {
        let tx = uow.begin().await.unwrap();
        repo.lock_state_in(&*tx, &key).await.unwrap();
        let err = repo.insert_live_version_in(&*tx, &version(key, n, VersionStatus::Live, Utc::now())).await.unwrap_err();
        assert!(matches!(err, RepositoryError::Conflict(ConflictKind::ToolPolicyVersion)), "version {n}: {err:?}");
        drop(tx);
    }
    assert_unchanged_at_two(&repo, &key).await;
}

#[tokio::test]
async fn insert_live_version_refuses_a_status_that_is_not_live() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (key, repo, uow) = set_at_version_two(&db).await;
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key).await.unwrap();
    let err = repo.insert_live_version_in(&*tx, &version(key, 3, VersionStatus::Superseded, Utc::now())).await.unwrap_err();
    assert!(matches!(err, RepositoryError::Backend(_)), "{err:?}");
    drop(tx);
    assert_unchanged_at_two(&repo, &key).await;
}

/// A version of `key` whose source names its organization, so that a read of the wrong org's row
/// shows in the source.
fn tagged_version(key: ToolPolicySetKey, n: u64, tag: &str) -> ToolPolicyVersion {
    ToolPolicyVersion {
        source: format!("// {tag} v{n}"),
        ..version(key, n, VersionStatus::Live, Utc::now())
    }
}

/// I1 (SMA-774 final review): two organizations hold catalogs and versions side by side. Each
/// read, list and write sees and changes only the rows of its own organization.
#[tokio::test]
async fn two_orgs_never_see_or_change_each_others_rows() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (a, b) = (seed_org(&db).await, seed_org(&db).await);
    let (key_a, key_b) = (ToolPolicySetKey::Tenant(a), ToolPolicySetKey::Tenant(b));
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let now = Utc::now();

    // A: tools "alpha" and "shared", catalog revision 1, versions 1 and 2 (live 2).
    // B: tools "beta" and "shared", catalog revision 1, version 1 (live 1).
    let tx = uow.begin().await.unwrap();
    repo.apply_catalog_in(
        &*tx,
        a,
        &[tool("alpha", ToolKind::Custom, None, json!({})), tool("shared", ToolKind::Custom, None, json!({"org": "a"}))],
        &[],
        now,
    )
    .await
    .unwrap();
    repo.apply_catalog_in(
        &*tx,
        b,
        &[tool("beta", ToolKind::Custom, None, json!({})), tool("shared", ToolKind::Custom, None, json!({"org": "b"}))],
        &[],
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    for (key, tag, n) in [(key_a, "a", 1u64), (key_a, "a", 2), (key_b, "b", 1)] {
        let tx = uow.begin().await.unwrap();
        repo.lock_state_in(&*tx, &key).await.unwrap();
        repo.insert_live_version_in(&*tx, &tagged_version(key, n, tag)).await.unwrap();
        tx.commit().await.unwrap();
    }

    let names = |cat: Vec<paigasus_iam_core::tool_policy::StoredTool>| cat.into_iter().map(|t| (t.entry.name.name_key, t.entry.cedar_input_type)).collect::<Vec<_>>();
    assert_eq!(names(repo.catalog(a).await.unwrap()), [("alpha".to_string(), json!({})), ("shared".to_string(), json!({"org": "a"}))]);
    assert_eq!(names(repo.catalog(b).await.unwrap()), [("beta".to_string(), json!({})), ("shared".to_string(), json!({"org": "b"}))]);
    let (st_a, st_b) = (repo.state(&key_a).await.unwrap(), repo.state(&key_b).await.unwrap());
    assert_eq!((st_a.catalog_revision, st_a.live_version), (1, 2));
    assert_eq!((st_b.catalog_revision, st_b.live_version), (1, 1));
    assert_eq!(repo.live(&key_a).await.unwrap().unwrap().source, "// a v2");
    assert_eq!(repo.live(&key_b).await.unwrap().unwrap().source, "// b v1");
    assert_eq!(repo.version(&key_a, 1).await.unwrap().unwrap().source, "// a v1");
    assert_eq!(repo.version(&key_b, 1).await.unwrap().unwrap().source, "// b v1");
    assert!(repo.version(&key_b, 2).await.unwrap().is_none(), "B must not see A's version 2");

    // A deletes "shared" and upserts "beta". B's "shared" stays, and B's "beta" keeps its value.
    let tx = uow.begin().await.unwrap();
    let rev = repo
        .apply_catalog_in(&*tx, a, &[tool("beta", ToolKind::Custom, None, json!({"org": "a"}))], &["shared".to_string()], now)
        .await
        .unwrap();
    assert_eq!(rev, 2);
    tx.commit().await.unwrap();
    assert_eq!(names(repo.catalog(a).await.unwrap()), [("alpha".to_string(), json!({})), ("beta".to_string(), json!({"org": "a"}))]);
    assert_eq!(names(repo.catalog(b).await.unwrap()), [("beta".to_string(), json!({})), ("shared".to_string(), json!({"org": "b"}))]);
    assert_eq!(repo.state(&key_b).await.unwrap().catalog_revision, 1, "A's write must not bump B's revision");

    // B saves version 2. A's live version 2 stays live, and A's version 1 stays superseded.
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key_b).await.unwrap();
    repo.insert_live_version_in(&*tx, &tagged_version(key_b, 2, "b")).await.unwrap();
    tx.commit().await.unwrap();
    let live_a = repo.live(&key_a).await.unwrap().unwrap();
    assert_eq!((live_a.version, live_a.status, live_a.source.as_str()), (2, VersionStatus::Live, "// a v2"));
    assert_eq!(repo.version(&key_a, 1).await.unwrap().unwrap().status, VersionStatus::Superseded);
    assert_eq!(repo.version(&key_b, 1).await.unwrap().unwrap().status, VersionStatus::Superseded);
    assert_eq!(repo.version(&key_b, 2).await.unwrap().unwrap().source, "// b v2");
    assert_eq!(repo.state(&key_a).await.unwrap().live_version, 2);
    assert_eq!(repo.state(&key_b).await.unwrap().live_version, 2);
}

/// A save through the real service: the new live version, its outbox row and its audit row
/// commit together, and the row carries the four-segment event type (SMA-775).
#[tokio::test]
async fn a_committed_save_leaves_one_outbox_row_with_the_four_segment_subject() {
    use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
    use paigasus_iam_core::PrincipalId;
    use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, tenant_schema_json};
    use paigasus_kernel::Prn;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let dir = tempfile::TempDir::new().expect("tempdir");
    let result = support::accepted_result(STARTER_TOOL_POLICY_SOURCE, &tenant_schema_json(&[]));
    let compiler = support::fake_compiler_script(dir.path(), &result);
    let (state, idp) = support::state_with_tool_policies(db.clone(), true, &compiler).await;
    let token = idp.bearer("tp-pg-admin", Some("tp-pg-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &token).await;
    let actor = Prn::parse(&admin_prn).unwrap();
    let out = state.orgs.create(&PrincipalId::from_prn(actor.clone()), "acme", "Acme").await.expect("create the organization");
    let org = out.organization.id.uuid();
    let svc = state.tool_policies.as_ref().expect("the tool policy service");

    svc.put_set(&actor, org, STARTER_TOOL_POLICY_SOURCE, 0).await.expect("save");

    let rows = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq("iam.policy.tool_set.published"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let corr = rows[0].correlation_id.expect("the event has a correlation id");
    let audit = audit_log::Entity::find().filter(audit_log::Column::CorrelationId.eq(corr)).all(&db).await.unwrap();
    assert_eq!(audit.len(), 1, "the audit row shares the correlation id");
    assert_eq!(audit[0].action, "PutToolPolicySet");

    // An identical save stores nothing and writes no second row.
    svc.put_set(&actor, org, STARTER_TOOL_POLICY_SOURCE, 1).await.expect("identical save");
    let count = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq("iam.policy.tool_set.published"))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 1);
}

/// A compiler that accepts every source: the Cedar text is the source and the schema is the
/// request schema. It waits a short time, so that two writers are both inside the compile step
/// before either of them locks the state.
struct SlowIdentityCompiler;

#[async_trait::async_trait]
impl paigasus_iam_core::tool_policy::PolicyCompiler for SlowIdentityCompiler {
    fn generation(&self) -> u32 {
        1
    }

    async fn compile(&self, req: &paigasus_iam_core::tool_policy::CompileRequest) -> Result<paigasus_iam_core::tool_policy::CompileOutcome, paigasus_iam_core::tool_policy::CompilerFailure> {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        Ok(paigasus_iam_core::tool_policy::CompileOutcome::Accepted(paigasus_iam_core::tool_policy::CompiledOutput {
            cedar_policies: req.source.clone(),
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

/// Two first enables at the same time (SMA-775). Both read live version 0 and both compile the
/// starter. The state lock lets one commit. The other then sees the new state and gets the
/// version conflict. One starter version, one event, one state row at version 1.
///
/// The service is built on the Postgres adapters with an allow-all authorizer. The full
/// `AppState` service cannot run this race: its first authorization of the second writer ends
/// only after the first writer has committed (measured), so the second writer reads version 1
/// and writes no starter. That is a property of the test harness, not of the write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_first_enables_write_one_starter() {
    use paigasus_iam::adapters::authz::Generations;
    use paigasus_iam::adapters::clock::SystemClock;
    use paigasus_iam::adapters::id::KernelIdGenerator;
    use paigasus_iam::adapters::persistence::entities::event_outbox;
    use paigasus_iam::adapters::persistence::{PgAuditLog, PgOrganizationRepository, PgOutbox};
    use paigasus_iam::application::authorize::Authorize;
    use paigasus_iam::application::error::TenancyError;
    use paigasus_iam::application::tool_policies::{ToolPolicyLimits, ToolPolicyService, ToolPolicyServiceDeps};
    use paigasus_iam_core::tool_policy::{CatalogLimits, FailMode, ToolPolicyMode};
    use paigasus_kernel::Prn;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org_ref = support::seed_org_ref(&db).await;
    let org = org_ref.resource_uuid();
    let org_prn = org_ref.canonical();
    let actor = Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap();
    let svc = ToolPolicyService::new(ToolPolicyServiceDeps {
        repo: std::sync::Arc::new(PgToolPolicies::new(db.clone())),
        orgs: std::sync::Arc::new(PgOrganizationRepository::new(db.clone(), Generations::memory())),
        compiler: std::sync::Arc::new(SlowIdentityCompiler),
        authorize: Authorize::new(std::sync::Arc::new(AllowAll)),
        uow: std::sync::Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        audit: std::sync::Arc::new(PgAuditLog::new(db.clone())),
        outbox: std::sync::Arc::new(PgOutbox::new(false)),
        limits: ToolPolicyLimits {
            catalog: CatalogLimits::default(),
            max_source_bytes: 65_536,
            max_window_ms: 3_600_000,
            max_catalog_response_bytes: 2_097_152,
        },
        ids: KernelIdGenerator,
        clock: SystemClock,
    });

    let (a, b) = tokio::join!(
        svc.put_settings(&actor, org, ToolPolicyMode::LogOnly, FailMode::Closed),
        svc.put_settings(&actor, org, ToolPolicyMode::LogOnly, FailMode::Closed),
    );
    let (ok, err) = match (a, b) {
        (Ok(ok), Err(err)) | (Err(err), Ok(ok)) => (ok, err),
        (a, b) => panic!("expected exactly one Ok and one Err, got {a:?} and {b:?}"),
    };
    assert_eq!(ok.starter_version, 1);
    assert_eq!(err, TenancyError::ToolPolicyVersionConflict);

    let count = |sql: String| {
        let db = db.clone();
        async move {
            let row = db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql)).await.unwrap().expect("a row");
            row.try_get_by_index::<i64>(0).unwrap()
        }
    };
    assert_eq!(count(format!("SELECT count(*) FROM tool_policy_set_version WHERE org_id = '{org}'")).await, 1);
    assert_eq!(count(format!("SELECT live_version FROM tool_policy_state WHERE org_id = '{org}'")).await, 1);
    let events = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq("iam.policy.tool_set.published"))
        .filter(event_outbox::Column::AggregatePrn.eq(org_prn))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
}
