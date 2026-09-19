// SPDX-License-Identifier: Apache-2.0

//! Waiting for a racer to reach a lock, rather than sleeping and hoping it got there.
//!
//! SMA-659 added this to `tests/authz_policy_store.rs` for one INSERT-vs-INSERT race. SMA-660
//! moved it here, because five more race tests in this crate need it and one predicate term —
//! the statement the racer blocks INSIDE — differs per site.
//!
//! Every item is `#[allow(dead_code)]`: `support` compiles once per test binary (about 59 of
//! them) and most use none of this, which `clippy --all-targets -D warnings` would otherwise
//! report.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Upper bound on how long a racer may take to block on the peer's uncommitted statement.
/// A LOAD BUDGET, not an expectation: the wait returns on the first observation that it blocked.
#[allow(dead_code)]
pub const RACER_BLOCK_BUDGET: Duration = Duration::from_secs(30);

/// How often [`wait_until_blocked_by`] polls `pg_stat_activity`.
#[allow(dead_code)]
const RACER_BLOCK_POLL: Duration = Duration::from_millis(10);

/// Counts backends blocked by `$1` on a row lock while running a statement that starts with `$2`.
///
/// Three terms, three jobs (SMA-660 spec §3.2). `$1 = ANY(pg_blocking_pids(pid))` answers WHO
/// blocks the racer; its necessity is guarded only synthetically, by guard case 4, not by any
/// V7 measurement — at S1 it is the PREFIX term that discriminates, rejecting a racer blocked by
/// the same peer inside a later statement (measured, V7 S1), and at S2 a missing lock surfaces
/// as a finished racer rather than a wrong-statement block. `btrim(query) ILIKE $2` shows the
/// racer is inside the statement under test rather than an earlier one — `btrim` because the
/// match would otherwise depend on whether a production SQL constant starts its raw string with
/// a newline. `wait_event IN ('transactionid', 'tuple')` covers both forms a row-lock waiter can
/// show: a waiter takes a `tuple` lock before it waits on the holder's transaction id, so a poll
/// can land on either. The `'tuple'` half is NOT verified by any test — see spec D5.
#[allow(dead_code)]
const BLOCKED_STATEMENTS_SQL: &str = "SELECT count(*)::bigint AS n FROM pg_stat_activity \
     WHERE wait_event_type = 'Lock' AND wait_event IN ('transactionid', 'tuple') \
     AND btrim(query) ILIKE $2 AND $1 = ANY(pg_blocking_pids(pid))";

/// Every non-idle client backend but the one running this query, for a deadline message: a stuck
/// pool, a different lock and a wrong predicate each look different here. 200 characters of
/// `query`, not 80: SeaORM emits a long column list before the table name, so 80 cannot tell two
/// SELECTs on different tables apart.
#[allow(dead_code)]
const NON_IDLE_BACKENDS_SQL: &str = "SELECT coalesce(string_agg(format('pid=%s state=%s wait=%s/%s query=%s', \
     pid, state, wait_event_type, wait_event, left(query, 200)), '; ' ORDER BY pid), '(none)') AS dump \
     FROM pg_stat_activity \
     WHERE backend_type = 'client backend' AND state IS DISTINCT FROM 'idle' AND pid <> pg_backend_pid()";

/// The backend pid of the connection that runs `conn` — for a transaction, the backend that holds
/// its locks.
#[allow(dead_code)]
pub async fn backend_pid(conn: &impl ConnectionTrait) -> i32 {
    conn.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT pg_backend_pid() AS pid"))
        .await
        .expect("query pg_backend_pid()")
        .expect("pg_backend_pid() always returns a row")
        .try_get::<i32>("", "pid")
        .expect("pg_backend_pid() is an integer")
}

/// Waits until the racer is inside a statement matching `query_prefix` and blocked by
/// `blocker_pid`, which replaces a fixed sleep that only HOPED it had got there. Each poll is an
/// autocommit statement on `db`: `pg_stat_activity` is a per-transaction snapshot, so a poll
/// inside one transaction would see the same data every time. It identifies a blocked backend
/// only by `(blocker_pid, query_prefix)`, never by "is this specifically `racer`" — `racer` is
/// used only for its `is_finished()` check. Safe at all eight call sites today, since each has
/// exactly one racer; a future test with two racers must not assume this returns `Ok` only when
/// THIS `racer` is the one blocked.
///
/// Checks, in this order, every [`RACER_BLOCK_POLL`]: racer blocked → `Ok`; `racer` finished →
/// `Err` (a blocked racer cannot finish while the peer is uncommitted, so checking "blocked"
/// first never hides a finished one); `budget` elapsed → `Err` with `query_prefix` and a dump of
/// the non-idle backends. Panics with `observer query failed: …` if the poll itself fails, which
/// is neither verdict. `budget` is not a hard wall-clock bound: the deadline is checked only
/// after a poll returns, so with an exhausted connection pool one poll can wait up to the pool's
/// own `acquire_timeout` (30 s) before the check runs and panics with `observer query failed`.
/// The worst case is therefore about `budget` plus that acquire timeout, and it is still bounded.
#[allow(dead_code)]
pub async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, query_prefix: &str, budget: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let blocked = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, BLOCKED_STATEMENTS_SQL, [blocker_pid.into(), query_prefix.into()]))
            .await
            .unwrap_or_else(|e| panic!("observer query failed: {e}"))
            .expect("count(*) always returns a row")
            .try_get::<i64>("", "n")
            .unwrap_or_else(|e| panic!("observer query failed: {e}"));
        if blocked > 0 {
            return Ok(());
        }
        if racer.is_finished() {
            return Err(format!(
                "the racer finished before it blocked on the peer's uncommitted statement (pid {blocker_pid}), so the race never happened"
            ));
        }
        if std::time::Instant::now() >= deadline {
            let dump = match db.query_one_raw(Statement::from_string(DbBackend::Postgres, NON_IDLE_BACKENDS_SQL)).await {
                Ok(Some(row)) => row.try_get::<String>("", "dump").unwrap_or_else(|e| format!("(dump unreadable: {e})")),
                Ok(None) => "(dump returned no row)".to_string(),
                Err(e) => format!("(dump failed: {e})"),
            };
            return Err(format!(
                "the racer did not block on the peer's uncommitted statement (pid {blocker_pid}) within {budget:?} \
                 while running a statement matching {query_prefix:?}; non-idle backends: {dump}"
            ));
        }
        tokio::time::sleep(RACER_BLOCK_POLL).await;
    }
}

/// The race tests' call site for [`wait_until_blocked_by`] with [`RACER_BLOCK_BUDGET`]: panics
/// with the wait's reason when the racer does not block, so the test stops HERE and never reaches
/// a verdict that would be meaningless without the race. When the racer has already finished, its
/// own result is part of the message (through `describe`, since a racer's output can hold a
/// non-`Debug` transaction), so an error or a panic inside the racer is not hidden behind
/// "finished".
#[allow(dead_code)]
pub async fn expect_racer_blocked<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &mut JoinHandle<T>, query_prefix: &str, describe: impl FnOnce(&T) -> String) {
    let Err(reason) = wait_until_blocked_by(db, blocker_pid, racer, query_prefix, RACER_BLOCK_BUDGET).await else {
        return;
    };
    if racer.is_finished() {
        let own = match racer.await {
            Ok(output) => describe(&output),
            Err(join_err) => format!("{join_err:?}"),
        };
        panic!("{reason}; the racer's own result: {own}");
    }
    racer.abort();
    panic!("{reason}");
}
