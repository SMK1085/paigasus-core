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
use paigasus_iam_core::{RepositoryError, UnitOfWork};
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
