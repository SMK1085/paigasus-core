// SPDX-License-Identifier: Apache-2.0

//! Shared SQL of the timer tests. The statements live here, never in the adapter.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement};
use uuid::Uuid;

/// The text of a note or a name as a SQL string literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Inserts one active `follow_up` row. `next_fire_at_sql` is a SQL expression. Use
/// `date_trunc('second', …)` or a whole-second literal, so that the stored value is a whole second.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_follow_up_row(db: &DatabaseConnection, inbox: Uuid, id: Uuid, origin: &str, next_fire_at_sql: &str, note: &str) {
    db.execute_unprepared(&format!(
        "INSERT INTO timer (id, inbox_id, kind, origin, status, note, next_fire_at) \
         VALUES ('{id}', '{inbox}', 'follow_up', {}, 'active', {}, {next_fire_at_sql})",
        literal(origin),
        literal(note)
    ))
    .await
    .expect("insert follow-up row");
}

/// Inserts one active `schedule` row of origin `agent`. `next_slot` is a civil time such as
/// `2030-01-01T09:00:00`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_schedule_row(db: &DatabaseConnection, inbox: Uuid, id: Uuid, cron: &str, tz: &str, next_slot: &str, next_fire_at_sql: &str) {
    db.execute_unprepared(&format!(
        "INSERT INTO timer (id, inbox_id, kind, origin, status, note, cron, tz, next_slot, next_slot_second, next_fire_at) \
         VALUES ('{id}', '{inbox}', 'schedule', 'agent', 'active', '', {}, {}, {}::timestamp, false, {next_fire_at_sql})",
        literal(cron),
        literal(tz),
        literal(next_slot)
    ))
    .await
    .expect("insert schedule row");
}

/// The mutable state of one timer row.
#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct TimerState {
    pub status: String,
    pub fire_count: i64,
    pub fire_attempts: i32,
    pub next_fire_at_us: i64,
    pub next_slot: Option<String>,
    pub failure: Option<String>,
}

/// The state of a timer row. It panics when the row does not exist.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn timer_state(db: &DatabaseConnection, id: Uuid) -> TimerState {
    let sql = format!(
        "SELECT status, fire_count, fire_attempts, (EXTRACT(EPOCH FROM next_fire_at) * 1000000)::bigint AS next_us, \
         to_char(next_slot, 'YYYY-MM-DD\"T\"HH24:MI:SS') AS next_slot, failure FROM timer WHERE id = '{id}'"
    );
    let row = db
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect("timer state")
        .expect("the timer row exists");
    TimerState {
        status: row.try_get("", "status").expect("status"),
        fire_count: row.try_get("", "fire_count").expect("fire_count"),
        fire_attempts: row.try_get("", "fire_attempts").expect("fire_attempts"),
        next_fire_at_us: row.try_get("", "next_us").expect("next_us"),
        next_slot: row.try_get("", "next_slot").expect("next_slot"),
        failure: row.try_get("", "failure").expect("failure"),
    }
}

/// One `inbox_event` row.
#[derive(Debug)]
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct EventRow {
    pub ce_id: String,
    pub source: String,
    pub ce_type: String,
    pub status: String,
    pub thread_id: Option<Uuid>,
    pub received_us: i64,
    pub envelope: serde_json::Value,
}

/// The events of an inbox, oldest first.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn timer_events(db: &DatabaseConnection, inbox: Uuid) -> Vec<EventRow> {
    let sql = format!(
        "SELECT ce_id, source, ce_type, status, thread_id, (EXTRACT(EPOCH FROM received_at) * 1000000)::bigint AS received_us, \
         envelope::text AS envelope FROM inbox_event WHERE inbox_id = '{inbox}' ORDER BY received_at, ce_id"
    );
    db.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect("timer events")
        .iter()
        .map(|r| EventRow {
            ce_id: r.try_get("", "ce_id").expect("ce_id"),
            source: r.try_get("", "source").expect("source"),
            ce_type: r.try_get("", "ce_type").expect("ce_type"),
            status: r.try_get("", "status").expect("status"),
            thread_id: r.try_get("", "thread_id").expect("thread_id"),
            received_us: r.try_get("", "received_us").expect("received_us"),
            envelope: serde_json::from_str(&r.try_get::<String>("", "envelope").expect("envelope")).expect("envelope JSON"),
        })
        .collect()
}

/// Commits an `inbox_lease` row for an inbox, outside any test transaction. A `FOR UPDATE` of
/// another session finds the row only when it is committed.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn ensure_lease_row(db: &DatabaseConnection, inbox: Uuid) {
    db.execute_unprepared(&format!(
        "INSERT INTO inbox_lease (inbox_id, holder, token, expires_at) VALUES ('{inbox}', 'test', 1, now() + interval '1 hour') ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("insert lease row");
}

/// Takes the `inbox_lease` row lock of an inbox in a test transaction, as the run commit of
/// SMA-790 does. It inserts the row when it is absent.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn lock_lease_row(txn: &DatabaseTransaction, inbox: Uuid) {
    txn.execute_unprepared(&format!(
        "INSERT INTO inbox_lease (inbox_id, holder, token, expires_at) VALUES ('{inbox}', 'test', 1, now() + interval '1 hour') ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("insert lease row");
    txn.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!("SELECT 1 FROM inbox_lease WHERE inbox_id = '{inbox}' FOR UPDATE"),
    ))
    .await
    .expect("lock lease row");
}
