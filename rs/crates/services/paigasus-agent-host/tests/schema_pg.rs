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
async fn down_drops_every_table_of_m0003_in_dependency_order() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    // m0004 (the timer table, SMA-788) is newer than m0003, so two steps down reach m0003.
    Migrator::down(&db, Some(2)).await.expect("down of m0004 and m0003");
    for table in ["thread", "inbox_lease", "run", "run_event", "inbox_event"] {
        assert!(support::scalar_bool(&db, &format!("SELECT to_regclass('{table}') IS NULL AS v")).await, "{table} must be gone");
    }
    assert!(support::scalar_bool(&db, "SELECT to_regclass('memory_file') IS NOT NULL AS v").await, "m0002 must stay");
    assert!(support::scalar_bool(&db, "SELECT to_regclass('inbox') IS NOT NULL AS v").await, "m0001 must stay");
}

#[tokio::test]
async fn m0004_creates_the_timer_table() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    for object in ["timer", "timer_due", "timer_inbox_active", "timer_inbox_agent_created"] {
        assert!(support::scalar_bool(&db, &format!("SELECT to_regclass('{object}') IS NOT NULL AS v")).await, "{object} must exist");
    }
}

/// An insert into `timer` for inbox 1 with the given columns. `extra` is a list of
/// `(column, SQL literal)` pairs that come on top of the valid base row.
async fn insert_timer(db: &DatabaseConnection, tid: u128, base: &[(&str, &str)], extra: &[(&str, &str)]) -> Result<(), sea_orm::DbErr> {
    let mut cols: Vec<(String, String)> = base.iter().map(|(c, v)| ((*c).to_string(), (*v).to_string())).collect();
    for (c, v) in extra {
        cols.retain(|(k, _)| k != c);
        cols.push(((*c).to_string(), (*v).to_string()));
    }
    let names: Vec<&str> = cols.iter().map(|(c, _)| c.as_str()).collect();
    let values: Vec<&str> = cols.iter().map(|(_, v)| v.as_str()).collect();
    db.execute_unprepared(&format!(
        "INSERT INTO timer (id, inbox_id, {}) VALUES ('{}', '{}', {})",
        names.join(", "),
        id(tid),
        id(1),
        values.join(", ")
    ))
    .await
    .map(|_| ())
}

/// A valid `follow_up` row.
const FOLLOW_UP: [(&str, &str); 6] = [
    ("kind", "'follow_up'"),
    ("origin", "'agent'"),
    ("status", "'active'"),
    ("note", "'n'"),
    ("scheduled_at", "now()"),
    ("next_fire_at", "now()"),
];

/// The extra columns that make the base row a valid `schedule`.
const SCHEDULE: [(&str, &str); 5] = [
    ("kind", "'schedule'"),
    ("cron", "'0 * * * *'"),
    ("tz", "'Europe/Berlin'"),
    ("next_slot", "'2026-01-01 10:00:00'"),
    ("next_slot_second", "false"),
];

fn assert_check_violation(r: Result<(), sea_orm::DbErr>, what: &str) {
    let err = r.expect_err(what);
    let code = match &err {
        sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) | sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e)) => e.as_database_error().and_then(|d| d.code().map(|c| c.to_string())),
        _ => None,
    };
    assert_eq!(code.as_deref(), Some("23514"), "{what}: expected SQLSTATE 23514, got {err:?}");
}

#[tokio::test]
async fn timer_checks_hold() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    // The two valid shapes are accepted.
    insert_timer(&db, 5000, &FOLLOW_UP, &[]).await.expect("valid follow_up");
    insert_timer(&db, 5001, &FOLLOW_UP, &SCHEDULE).await.expect("valid schedule");
    insert_timer(&db, 5002, &FOLLOW_UP, &[("note", &format!("'{}'", "é".repeat(1000)))])
        .await
        .expect("1000 two-byte characters are accepted");

    assert_check_violation(insert_timer(&db, 5010, &FOLLOW_UP, &[("kind", "'schedule'")]).await, "schedule with no cron");
    assert_check_violation(insert_timer(&db, 5011, &FOLLOW_UP, &[("cron", "'0 * * * *'")]).await, "follow_up with a cron");
    let no_tz: Vec<(&str, &str)> = SCHEDULE.iter().filter(|(c, _)| *c != "tz").copied().collect();
    assert_check_violation(insert_timer(&db, 5012, &FOLLOW_UP, &no_tz).await, "cron without tz");
    assert_check_violation(
        insert_timer(
            &db,
            5013,
            &FOLLOW_UP,
            &[("kind", "'schedule'"), ("cron", "'0 * * * *'"), ("tz", "'UTC'"), ("next_slot", "'2026-01-01 10:00:00'")],
        )
        .await,
        "next_slot without next_slot_second",
    );
    assert_check_violation(insert_timer(&db, 5014, &FOLLOW_UP, &[("note", &format!("'{}'", "é".repeat(1001)))]).await, "note of 1001 characters");
    assert_check_violation(
        insert_timer(&db, 5015, &FOLLOW_UP, &[("failure", &format!("'{}'", "x".repeat(501)))]).await,
        "failure of 501 characters",
    );
    assert_check_violation(insert_timer(&db, 5016, &FOLLOW_UP, &[("status", "'paused'")]).await, "status paused");
    for (n, col, lit) in [
        (5017, "next_fire_at", "'-infinity'"),
        (5018, "next_fire_at", "'infinity'"),
        (5022, "scheduled_at", "'-infinity'"),
        (5023, "scheduled_at", "'infinity'"),
        (5019, "last_fired_at", "'-infinity'"),
        (5020, "last_fired_at", "'infinity'"),
    ] {
        assert_check_violation(insert_timer(&db, n, &FOLLOW_UP, &[(col, lit)]).await, &format!("{col} {lit}"));
    }
    insert_timer(&db, 5021, &FOLLOW_UP, &[("last_fired_at", "now()")]).await.expect("a finite last_fired_at");
    // `next_fire_at` is the due time. It must not be before `scheduled_at`.
    assert_check_violation(
        insert_timer(&db, 5024, &FOLLOW_UP, &[("scheduled_at", "now() + interval '1 minute'"), ("next_fire_at", "now()")]).await,
        "next_fire_at before scheduled_at",
    );
    insert_timer(&db, 5025, &FOLLOW_UP, &[("scheduled_at", "now()"), ("next_fire_at", "now() + interval '1 minute'")])
        .await
        .expect("a due time after the scheduled instant (a back-off)");
    let no_scheduled: Vec<(&str, &str)> = FOLLOW_UP.iter().filter(|(c, _)| *c != "scheduled_at").copied().collect();
    let err = insert_timer(&db, 5026, &no_scheduled, &[]).await.expect_err("scheduled_at is required");
    assert!(
        err.to_string().contains("scheduled_at") && err.to_string().contains("null value"),
        "expected a NOT NULL error on scheduled_at, got {err:?}"
    );
}

#[tokio::test]
async fn timer_thread_must_be_of_the_inbox() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    two_inboxes(&db).await;
    insert_thread(&db, id(300), id(2)).await;
    let thread = format!("'{}'", id(300));
    let err = insert_timer(&db, 5100, &FOLLOW_UP, &[("thread_id", &thread)])
        .await
        .expect_err("a thread of inbox B must be refused for inbox A");
    assert!(matches!(err.sql_err(), Some(SqlErr::ForeignKeyConstraintViolation(_))), "expected 23503, got {err:?}");
    insert_thread(&db, id(301), id(1)).await;
    let own = format!("'{}'", id(301));
    insert_timer(&db, 5101, &FOLLOW_UP, &[("thread_id", &own)]).await.expect("a thread of the same inbox is accepted");
}

#[tokio::test]
async fn m0004_down_drops_only_the_timer_table() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping schema test: Docker unavailable");
        return;
    };
    Migrator::down(&db, Some(1)).await.expect("down of m0004");
    assert!(support::scalar_bool(&db, "SELECT to_regclass('timer') IS NULL AS v").await, "timer must be gone");
    assert!(support::scalar_bool(&db, "SELECT to_regclass('inbox_event') IS NOT NULL AS v").await, "m0003 must stay");
}
