// SPDX-License-Identifier: Apache-2.0

//! Shared SQL of the lease tests. The statements live here, never in the adapter.

use std::time::Duration;

use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// The backend pid of the session of `conn` (for example the transaction that holds a row lock).
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn backend_pid<C: ConnectionTrait>(conn: &C) -> i64 {
    conn.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT pg_backend_pid()::bigint AS n".to_owned()))
        .await
        .expect("query the backend pid")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("bigint column named n")
}

/// Waits until a session of the database waits on a lock that the session `holder` holds. It
/// panics after `within`. A lock-wait test calls it before it releases the lock, so a call that
/// starts only after the release cannot pass the test without a wait.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn wait_until_blocked_by(db: &DatabaseConnection, holder: i64, within: Duration) {
    let sql = format!("SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE {holder} = ANY(pg_blocking_pids(pid))");
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let row = db
            .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.clone()))
            .await
            .expect("query pg_stat_activity")
            .expect("row");
        if row.try_get::<i64>("", "n").expect("bigint column named n") > 0 {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "no session waited on the row lock within {within:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Expires the lease of an inbox one second in the past. The row stays.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn expire(db: &DatabaseConnection, inbox: Uuid) {
    db.execute_unprepared(&format!("UPDATE inbox_lease SET expires_at = now() - interval '1 second' WHERE inbox_id = '{inbox}'"))
        .await
        .expect("expire lease");
}

/// The ids of the `done` and `dead` events of an inbox.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn terminal_event_ids(db: &DatabaseConnection, inbox: Uuid) -> std::collections::HashSet<Uuid> {
    let sql = format!("SELECT id FROM inbox_event WHERE inbox_id = '{inbox}' AND status IN ('done', 'dead')");
    db.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect("terminal events")
        .iter()
        .map(|r| r.try_get("", "id").expect("id"))
        .collect()
}

/// Expires the lease of an inbox only while it still has `token`. It returns false when another
/// take already replaced that token, so the caller does not expire the lease of another holder.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn expire_token(db: &DatabaseConnection, inbox: Uuid, token: i64) -> bool {
    db.execute_unprepared(&format!(
        "UPDATE inbox_lease SET expires_at = now() - interval '1 second' WHERE inbox_id = '{inbox}' AND token = {token}"
    ))
    .await
    .expect("expire lease")
    .rows_affected()
        == 1
}

/// Inserts one pending event, received `age_secs` seconds ago and due now.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_pending_event(db: &DatabaseConnection, inbox: Uuid, event: Uuid, age_secs: i64) {
    db.execute_unprepared(&format!(
        "INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, envelope, received_at, not_before) \
         VALUES ('{event}', '{inbox}', 'urn:test', '{event}', 'agents.x', '{{}}'::jsonb, \
                 now() - interval '{age_secs} seconds', now() - interval '{age_secs} seconds')"
    ))
    .await
    .expect("insert pending event");
}

/// The expiry of the lease row, in epoch microseconds.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn expires_us(db: &DatabaseConnection, inbox: Uuid) -> i64 {
    super::scalar_i64(
        db,
        &format!("SELECT (EXTRACT(EPOCH FROM expires_at) * 1000000)::bigint AS n FROM inbox_lease WHERE inbox_id = '{inbox}'"),
    )
    .await
}

/// Inserts one live thread: not closed, never run, with no successor.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_thread(db: &DatabaseConnection, inbox: Uuid, thread: Uuid) {
    db.execute_unprepared(&format!("INSERT INTO thread (id, inbox_id) VALUES ('{thread}', '{inbox}')"))
        .await
        .expect("insert thread");
}

/// Inserts one pending event on a thread, received `age_secs` seconds ago and due now.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_event_on_thread(db: &DatabaseConnection, inbox: Uuid, event: Uuid, age_secs: i64, thread: Uuid) {
    insert_pending_event(db, inbox, event, age_secs).await;
    db.execute_unprepared(&format!("UPDATE inbox_event SET thread_id = '{thread}' WHERE id = '{event}'"))
        .await
        .expect("set event thread");
}

/// The sum of the counter `name` in one snapshot of the recorder. With `label`, only the series
/// that carry that label value count. A snapshot drains the counters, so read each one once.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn counter(recorder: &DebuggingRecorder, name: &str, label: Option<(&str, &str)>) -> u64 {
    recorder
        .snapshotter()
        .snapshot()
        .into_vec()
        .into_iter()
        .filter_map(|(key, _, _, v)| {
            let DebugValue::Counter(n) = v else { return None };
            if key.key().name() != name {
                return None;
            }
            match label {
                Some((k, want)) => key.key().labels().any(|l| l.key() == k && l.value() == want).then_some(n),
                None => Some(n),
            }
        })
        .sum()
}

/// Inserts a thread and a `running` run with the given lease token, as a claim does.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_run(db: &DatabaseConnection, inbox: Uuid, run: Uuid, thread: Uuid, token: i64) {
    insert_thread(db, inbox, thread).await;
    db.execute_unprepared(&format!(
        "INSERT INTO run (id, inbox_id, thread_id, lease_token, status) VALUES ('{run}', '{inbox}', '{thread}', {token}, 'running')"
    ))
    .await
    .expect("insert run");
}

/// The token of the lease row and whether the lease is live on the database clock. `None`: no
/// lease row yet.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn lease_state(db: &DatabaseConnection, inbox: Uuid) -> Option<(i64, bool)> {
    let sql = format!("SELECT token, expires_at > clock_timestamp() AS live FROM inbox_lease WHERE inbox_id = '{inbox}'");
    let row = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql)).await.expect("lease state")?;
    Some((row.try_get("", "token").expect("token"), row.try_get("", "live").expect("live")))
}

/// The thread of the newest run of an inbox, if the inbox has a run.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn newest_run_thread(db: &DatabaseConnection, inbox: Uuid) -> Option<Uuid> {
    let sql = format!("SELECT thread_id FROM run WHERE inbox_id = '{inbox}' ORDER BY started_at DESC, id DESC LIMIT 1");
    let row = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql)).await.expect("newest run")?;
    Some(row.try_get("", "thread_id").expect("thread_id"))
}

/// Every `inbox_event`, `run`, `thread` and `inbox_lease` row of an inbox as one text. Two equal
/// texts mean that no row of these tables changed. The lease row holds the token, the holder and
/// the stored `expires_at` (not a live flag, which changes as time passes), so a stale call that
/// moves the expiry of the current lease also shows.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn snapshot(db: &DatabaseConnection, inbox: Uuid) -> String {
    let mut out = String::new();
    for table in ["inbox_event", "run", "thread", "inbox_lease"] {
        // `inbox_lease` has no `id` column; one row per inbox, so the text order is enough.
        let sql = format!("SELECT coalesce(string_agg(row_to_json(x)::text, '|' ORDER BY row_to_json(x)::text), '') AS s FROM {table} x WHERE x.inbox_id = '{inbox}'");
        let row = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql)).await.expect("snapshot").expect("row");
        out.push_str(&row.try_get::<String>("", "s").expect("text column named s"));
        out.push('\n');
    }
    out
}
