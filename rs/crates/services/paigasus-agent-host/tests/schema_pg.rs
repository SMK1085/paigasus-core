// SPDX-License-Identifier: Apache-2.0

//! The inbox tables against a real Postgres (spec §7.1): the dedup key, the composite foreign
//! keys that keep a thread, a run and an event in one inbox, the status check and the indexes.

mod support;

use paigasus_agent_host::adapters::persistence::Migrator;
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlErr};
use sea_orm_migration::MigratorTrait;
use support::inbox::{insert_inbox, migrated_postgres};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// Two inboxes: A (id 1) and B (id 2).
async fn two_inboxes(db: &DatabaseConnection) {
    insert_inbox(db, id(100), id(1), id(10), id(20)).await;
    insert_inbox(db, id(100), id(2), id(11), id(21)).await;
}

async fn insert_thread(db: &DatabaseConnection, thread: Uuid, inbox: Uuid) {
    db.execute_unprepared(&format!("INSERT INTO thread (id, inbox_id) VALUES ('{thread}', '{inbox}')"))
        .await
        .expect("insert thread");
}

async fn insert_run(db: &DatabaseConnection, run: Uuid, inbox: Uuid, thread: Uuid) -> Result<(), sea_orm::DbErr> {
    db.execute_unprepared(&format!(
        "INSERT INTO run (id, inbox_id, thread_id, lease_token, status) VALUES ('{run}', '{inbox}', '{thread}', 1, 'running')"
    ))
    .await
    .map(|_| ())
}

fn event_sql(ev: Uuid, inbox: Uuid, source: &str, ce_id: &str, thread: Option<Uuid>, run: Option<Uuid>, status: &str) -> String {
    let thread = thread.map_or("NULL".to_string(), |t| format!("'{t}'"));
    let run = run.map_or("NULL".to_string(), |r| format!("'{r}'"));
    format!(
        "INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, thread_id, envelope, status, claimed_run_id) \
         VALUES ('{ev}', '{inbox}', '{source}', '{ce_id}', 'agents.x', {thread}, '{{}}', '{status}', {run})"
    )
}

#[tokio::test]
async fn the_dedup_key_refuses_a_second_row() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    db.execute_unprepared(&event_sql(id(1000), id(1), "s", "c", None, None, "pending")).await.expect("first");
    let err = db.execute_unprepared(&event_sql(id(1001), id(1), "s", "c", None, None, "pending")).await.expect_err("second must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))), "expected 23505, got {err:?}");
}

#[tokio::test]
async fn the_same_ce_id_from_two_sources_gives_two_rows() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    db.execute_unprepared(&event_sql(id(1000), id(1), "s1", "c", None, None, "pending")).await.expect("first");
    db.execute_unprepared(&event_sql(id(1001), id(1), "s2", "c", None, None, "pending")).await.expect("second");
}

#[tokio::test]
async fn an_event_cannot_name_a_thread_of_another_inbox() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    insert_thread(&db, id(500), id(2)).await;
    let err = db
        .execute_unprepared(&event_sql(id(1000), id(1), "s", "c", Some(id(500)), None, "pending"))
        .await
        .expect_err("must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::ForeignKeyConstraintViolation(_))), "expected 23503, got {err:?}");
}

#[tokio::test]
async fn a_run_cannot_name_a_thread_of_another_inbox() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    insert_thread(&db, id(500), id(2)).await;
    let err = insert_run(&db, id(600), id(1), id(500)).await.expect_err("must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::ForeignKeyConstraintViolation(_))), "expected 23503, got {err:?}");
}

#[tokio::test]
async fn an_event_cannot_name_a_run_of_another_inbox() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    insert_thread(&db, id(500), id(2)).await;
    insert_run(&db, id(600), id(2), id(500)).await.expect("run in inbox B");
    let err = db
        .execute_unprepared(&event_sql(id(1000), id(1), "s", "c", None, Some(id(600)), "pending"))
        .await
        .expect_err("must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::ForeignKeyConstraintViolation(_))), "expected 23503, got {err:?}");
}

#[tokio::test]
async fn the_status_check_refuses_claimed() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    for (n, status) in ["pending", "done", "dead"].into_iter().enumerate() {
        db.execute_unprepared(&event_sql(id(1000 + n as u128), id(1), "s", &format!("c{n}"), None, None, status))
            .await
            .unwrap_or_else(|e| panic!("status {status} must be accepted: {e:?}"));
    }
    let err = db
        .execute_unprepared(&event_sql(id(2000), id(1), "s", "cx", None, None, "claimed"))
        .await
        .expect_err("claimed must fail");
    assert!(err.to_string().contains("inbox_event_status_check"), "expected the status check, got {err:?}");
}

#[tokio::test]
async fn the_claimable_index_is_partial_on_pending() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    let ok = support::scalar_bool(
        &db,
        "SELECT indexdef LIKE '%WHERE (status = ''pending''::text)' AS v FROM pg_indexes WHERE indexname = 'inbox_event_claimable'",
    )
    .await;
    assert!(ok, "inbox_event_claimable must be partial on status = 'pending'");
}

#[tokio::test]
async fn no_index_names_the_holder_column() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    let n = support::scalar_i64(&db, "SELECT count(*)::bigint AS n FROM pg_indexes WHERE tablename = 'inbox_lease' AND indexdef ~ 'holder'").await;
    assert_eq!(n, 0, "holder is diagnostic only and must not be indexed");
    let all = support::scalar_i64(&db, "SELECT count(*)::bigint AS n FROM pg_indexes WHERE tablename = 'inbox_lease'").await;
    assert!(all >= 1, "the check must see the primary key index of inbox_lease");
}

#[tokio::test]
async fn down_drops_every_table_of_m0002_in_dependency_order() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    Migrator::down(&db, Some(1)).await.expect("down of m0002");
    for table in ["thread", "inbox_lease", "run", "run_event", "inbox_event"] {
        assert!(support::scalar_bool(&db, &format!("SELECT to_regclass('{table}') IS NULL AS v")).await, "{table} must be gone");
    }
    assert!(support::scalar_bool(&db, "SELECT to_regclass('inbox') IS NOT NULL AS v").await, "m0001 must stay");
}
