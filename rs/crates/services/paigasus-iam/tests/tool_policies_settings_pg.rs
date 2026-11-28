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
        first_enabled_at: None,
        webhooks: Vec::new(),
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
    repo.upsert_settings_in(&*tx, org, &first, None).await.unwrap();
    tx.commit().await.unwrap();
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.fail_mode, got.updated_by.as_deref()), (ToolPolicyMode::LogOnly, FailMode::Closed, Some("alice")));

    let second = settings(ToolPolicyMode::Enforce, FailMode::Open, "bob");
    let tx = uow.begin().await.unwrap();
    repo.upsert_settings_in(&*tx, org, &second, None).await.unwrap();
    tx.commit().await.unwrap();
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.fail_mode, got.updated_by.as_deref()), (ToolPolicyMode::Enforce, FailMode::Open, Some("bob")));
    assert!(got.updated_at.is_some());
    assert_eq!(row_count(&db, org).await, 1, "an upsert keeps one row");
}

#[tokio::test]
async fn first_enabled_at_round_trips_and_survives_a_later_write() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let write = |s: ToolPolicySettings| {
        let (repo, uow) = (repo.clone(), uow.clone());
        async move {
            let tx = uow.begin().await.unwrap();
            repo.upsert_settings_in(&*tx, org, &s, None).await.unwrap();
            tx.commit().await.unwrap();
        }
    };
    // A row in `off` that was never enabled stores NULL.
    write(settings(ToolPolicyMode::Off, FailMode::Closed, "alice")).await;
    assert_eq!(repo.settings(org).await.unwrap().unwrap().first_enabled_at, None);
    // The first enable stores the time. Postgres keeps microseconds.
    let first = chrono::DateTime::parse_from_rfc3339("2026-11-17T10:00:00.123456Z").unwrap().to_utc();
    write(ToolPolicySettings {
        first_enabled_at: Some(first),
        ..settings(ToolPolicyMode::LogOnly, FailMode::Closed, "alice")
    })
    .await;
    assert_eq!(repo.settings(org).await.unwrap().unwrap().first_enabled_at, Some(first));
    // A later write with another value, and one with none, keep the stored value.
    write(ToolPolicySettings {
        first_enabled_at: Some(Utc::now()),
        ..settings(ToolPolicyMode::Enforce, FailMode::Open, "bob")
    })
    .await;
    write(settings(ToolPolicyMode::Off, FailMode::Open, "bob")).await;
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.first_enabled_at), (ToolPolicyMode::Off, Some(first)));
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
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Enforce, FailMode::Open, "alice"), None).await.unwrap();
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
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Enforce, FailMode::Open, "alice"), None).await.unwrap();
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
    let err = repo.upsert_settings_in(&*tx, org, &ToolPolicySettings::default_for_absent_row(), None).await.unwrap_err();
    assert!(matches!(err, RepositoryError::Backend(_)), "{err:?}");
}

// ---- SMA-856: the webhook opt-in list ----

async fn write_with(repo: &PgToolPolicies, uow: &SeaOrmUnitOfWork, org: Uuid, s: &ToolPolicySettings, webhooks: Option<Vec<String>>) -> Vec<String> {
    let tx = uow.begin().await.unwrap();
    let stored = repo.upsert_settings_in(&*tx, org, s, webhooks).await.unwrap();
    tx.commit().await.unwrap();
    stored
}

fn list(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|s| (*s).to_owned()).collect())
}

#[tokio::test]
async fn the_webhooks_column_defaults_to_an_empty_list() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let stored = write_with(&repo, &uow, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), None).await;
    assert_eq!(stored, Vec::<String>::new());
    assert_eq!(repo.settings(org).await.unwrap().unwrap().webhooks, Vec::<String>::new());
}

#[tokio::test]
async fn a_webhook_list_round_trips() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let s = settings(ToolPolicyMode::Off, FailMode::Closed, "alice");
    assert_eq!(write_with(&repo, &uow, org, &s, list(&["a", "b"])).await, ["a", "b"]);
    assert_eq!(repo.settings(org).await.unwrap().unwrap().webhooks, ["a", "b"]);
    assert_eq!(write_with(&repo, &uow, org, &s, list(&[])).await, Vec::<String>::new());
    assert_eq!(repo.settings(org).await.unwrap().unwrap().webhooks, Vec::<String>::new());
}

#[tokio::test]
async fn an_absent_list_does_not_undo_a_concurrent_write() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    write_with(&repo, &uow, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), list(&["a"])).await;
    // A stale struct (`webhooks` empty) with `None` must keep the stored list.
    let stale = ToolPolicySettings {
        webhooks: Vec::new(),
        ..settings(ToolPolicyMode::LogOnly, FailMode::Open, "bob")
    };
    let stored = write_with(&repo, &uow, org, &stale, None).await;
    assert_eq!(stored, ["a"]);
    let got = repo.settings(org).await.unwrap().unwrap();
    assert_eq!((got.mode, got.webhooks), (ToolPolicyMode::LogOnly, vec!["a".to_owned()]));
}

#[tokio::test]
async fn a_bad_stored_list_is_corrupt() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    write_with(&repo, &uow, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), None).await;
    raw(&db, format!("UPDATE tool_policy_settings SET webhooks = 'not json' WHERE org_id = '{org}'")).await.unwrap();
    let err = repo.settings(org).await.unwrap_err();
    assert!(matches!(err, RepositoryError::Backend(_)), "{err:?}");
}

#[tokio::test]
async fn the_down_migration_drops_the_column() {
    use paigasus_iam::adapters::persistence::Migrator;
    use sea_orm_migration::MigratorTrait;
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    write_with(&repo, &uow, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), list(&["a"])).await;
    // The count depends on the number of later migrations: raise it when one is added.
    Migrator::down(&db, Some(1)).await.expect("m0016 down must succeed");
    let columns = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'tool_policy_settings' AND column_name = 'webhooks'".to_owned(),
        ))
        .await
        .unwrap();
    assert!(columns.is_empty(), "the column is gone");
    assert_eq!(row_count(&db, org).await, 1, "the row stays");
    Migrator::up(&db, Some(1)).await.expect("m0016 must apply again");
    assert_eq!(repo.settings(org).await.unwrap().unwrap().webhooks, Vec::<String>::new());
}

/// One settings write as `put_settings` does it: read the predecessor under the row lock, then
/// replace the list. Returns the predecessor that the audit entry would record.
async fn locked_write(repo: &PgToolPolicies, uow: &SeaOrmUnitOfWork, org: Uuid, new: &str, entered: Option<tokio::sync::oneshot::Sender<()>>) -> Vec<String> {
    let tx = uow.begin().await.unwrap();
    let before = repo.webhooks_for_update_in(&*tx, org).await.unwrap();
    if let Some(entered) = entered {
        let _ = entered.send(());
    }
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), list(&[new]))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    before
}

#[tokio::test]
async fn an_absent_row_has_an_empty_locked_predecessor() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let tx = uow.begin().await.unwrap();
    assert_eq!(repo.webhooks_for_update_in(&*tx, org).await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn two_overlapping_settings_writes_record_a_chain_of_predecessors() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let org = seed_org(&db).await;
    let repo = PgToolPolicies::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    write_with(&repo, &uow, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), list(&["a"])).await;

    // The first writer holds the row lock from its read until its commit.
    let tx = uow.begin().await.unwrap();
    assert_eq!(repo.webhooks_for_update_in(&*tx, org).await.unwrap(), ["a"]);
    // The second writer starts now and must wait for the lock.
    let (entered_tx, mut entered_rx) = tokio::sync::oneshot::channel();
    let second = {
        let (repo, uow) = (repo.clone(), uow.clone());
        tokio::spawn(async move { locked_write(&repo, &uow, org, "c", Some(entered_tx)).await })
    };
    let waited = tokio::time::timeout(std::time::Duration::from_millis(1500), &mut entered_rx).await;
    assert!(waited.is_err(), "the second writer read the list while the first held the lock");
    repo.upsert_settings_in(&*tx, org, &settings(ToolPolicyMode::Off, FailMode::Closed, "alice"), list(&["b"]))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The first writer recorded ["a"]. The second one now sees the list that the first stored.
    let second_before = second.await.unwrap();
    assert_eq!(second_before, ["b"]);
    assert_eq!(repo.settings(org).await.unwrap().unwrap().webhooks, ["c"]);
}
