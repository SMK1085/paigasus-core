// SPDX-License-Identifier: Apache-2.0

//! The tool policy settings of `PgToolPolicies`, migration m0014 and the `starter` origin
//! against real Postgres (SMA-775). Each test starts its own container and seeds one
//! organization.
//!
//! Runs against an ephemeral Postgres in Docker. The Docker gating goes through the shared
//! policy (`support::start_migrated_postgres` calls `start_or_skip` of `paigasus-test-docker`,
//! SMA-538); this file does not restate it and does not read `CI`. A container failure with a
//! reachable daemon is a hard failure. An unreachable daemon skips locally, and the
//! `tests/docker_preflight.rs` canary makes that skip visible.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::persistence::{PgToolPolicies, SeaOrmUnitOfWork};
use paigasus_iam_core::tool_policy::{FailMode, ToolPolicyMode, ToolPolicyRepository, ToolPolicySetKey, ToolPolicySettings, ToolPolicyVersion, VersionOrigin, VersionStatus};
use paigasus_iam_core::{RepositoryError, UnitOfWork};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    support::seed_org_ref(db).await.resource_uuid()
}

async fn raw(db: &DatabaseConnection, sql: String) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.map(|_| ())
}

fn settings(mode: ToolPolicyMode, fail_mode: FailMode, by: &str) -> ToolPolicySettings {
    ToolPolicySettings {
        mode,
        fail_mode,
        updated_at: Some(Utc::now()),
        updated_by: Some(by.to_string()),
    }
}

async fn row_count(db: &DatabaseConnection, org: Uuid) -> i64 {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT count(*) AS n FROM tool_policy_settings WHERE org_id = '{org}'"),
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<i64>("", "n").unwrap()
}

#[tokio::test]
async fn settings_are_none_for_an_org_with_no_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    assert_eq!(repo.settings(org).await.unwrap(), None);
}

#[tokio::test]
async fn upsert_inserts_then_updates_one_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let first = settings(ToolPolicyMode::LogOnly, FailMode::Closed, "alice");
    let tx = uow.begin().await.unwrap();
    repo.upsert_settings_in(&*tx, org, &first).await.unwrap();
    tx.commit().await.unwrap();
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.fail_mode, got.updated_by.as_deref()), (ToolPolicyMode::LogOnly, FailMode::Closed, Some("alice")));

    let second = settings(ToolPolicyMode::Enforce, FailMode::Open, "bob");
    let tx = uow.begin().await.unwrap();
    repo.upsert_settings_in(&*tx, org, &second).await.unwrap();
    tx.commit().await.unwrap();
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.fail_mode, got.updated_by.as_deref()), (ToolPolicyMode::Enforce, FailMode::Open, Some("bob")));
    assert!(got.updated_at.is_some());
    assert_eq!(row_count(&db, org).await, 1, "an upsert keeps one row");
}

#[tokio::test]
async fn an_unknown_mode_is_refused_by_the_check() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let err = raw(
        &db,
        format!("INSERT INTO tool_policy_settings (org_id, mode, fail_mode, updated_at, updated_by) VALUES ('{org}', 'strict', 'open', now(), 't')"),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("violates check constraint"), "{err}");
    let err = raw(
        &db,
        format!("INSERT INTO tool_policy_settings (org_id, mode, fail_mode, updated_at, updated_by) VALUES ('{org}', 'off', 'maybe', now(), 't')"),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("violates check constraint"), "{err}");
}

#[tokio::test]
async fn deleting_the_org_cascades_to_its_settings() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.unwrap();
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Enforce, FailMode::Open, "alice")).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(row_count(&db, org).await, 1);
    raw(&db, format!("DELETE FROM organization WHERE id = '{org}'")).await.unwrap();
    assert_eq!(row_count(&db, org).await, 0);
}

#[tokio::test]
async fn a_starter_origin_version_round_trips() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let key = ToolPolicySetKey::Tenant(org);
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let v = ToolPolicyVersion {
        key,
        version: 1,
        source: "// starter".into(),
        compiled: vec![1, 2, 3],
        compiler_generation: 1,
        schema_revision: 1,
        catalog_revision: Some(0),
        status: VersionStatus::Live,
        origin: VersionOrigin::Starter,
        created_by: "tester".into(),
        created_at: Utc::now(),
    };
    let tx = uow.begin().await.unwrap();
    repo.lock_state_in(&*tx, &key).await.unwrap();
    repo.insert_live_version_in(&*tx, &v).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(repo.live(&key).await.unwrap().unwrap().origin, VersionOrigin::Starter);
}

#[tokio::test]
async fn an_upsert_rolls_back_with_its_transaction() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.unwrap();
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Enforce, FailMode::Open, "alice")).await.unwrap();
    drop(tx);
    assert_eq!(repo.settings(org).await.unwrap(), None);
}

#[tokio::test]
async fn an_upsert_without_actor_or_time_is_an_error() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.unwrap();
    let err = repo.upsert_settings_in(&*tx, org, &ToolPolicySettings::default_for_absent_row()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::Backend(_)), "{err:?}");
}
