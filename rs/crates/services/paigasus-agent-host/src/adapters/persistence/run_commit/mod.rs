// SPDX-License-Identifier: Apache-2.0

//! The Postgres run committer (spec §6.7, §6.8).
//!
//! Every write of a run end goes through one fenced transaction: the lease row is locked first,
//! then the run row is moved out of `running` with its token. Every COMMIT goes through
//! `commit_or_poison`. The `Drop` of a `DatabaseTransaction` can panic (see `migration_lock.rs`),
//! so every error path rolls back explicitly.

mod back_off;
mod commit;
mod host_fault;
mod screen;
mod shutdown;

use std::sync::Arc;

use async_trait::async_trait;
use paigasus_agent_host_core::fault::FaultPoint;
use paigasus_agent_host_core::ids::RunId;
use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::ports::RunCommitter;
use paigasus_agent_host_core::run::{BackOff, CommitError, Committed, EventDelay, LeaseToken, RunRecord, ScreenOut, ScreenResult, UNENCODABLE_PREFIX};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, RuntimeErr, Statement, Value};
use uuid::Uuid;

use crate::adapters::memory_commit::MemoryCommit;
use crate::adapters::persistence::lease_store::{lock_live_lease, lost};

/// The lock wait of one commit statement. A wait that ends with this timeout is a `Store` error.
const LOCK_TIMEOUT_SQL: &str = "SET LOCAL lock_timeout = '5s'";

/// The SQLSTATEs of a text that Postgres cannot store: `22P05` (untranslatable character) and
/// `22021` (a byte sequence that is not valid in the encoding).
const UNENCODABLE_CODES: [&str; 2] = ["22P05", "22021"];

/// `$1` = event, `$2` = run, `$3` = delay in microseconds, `$4` = error. The event stays pending.
const RETRY_SQL: &str = "UPDATE inbox_event SET not_before = clock_timestamp() + ($3::bigint * interval '1 microsecond'), last_error = $4 \
     WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending'";

/// The run committer on a Postgres pool.
pub struct PgRunCommitter {
    db: DatabaseConnection,
    memory: MemoryCommit,
    caps: MemoryCaps,
    faults: Arc<dyn FaultPoint>,
}

impl PgRunCommitter {
    /// Build a committer. `memory` decides how the memory changes of a run commit.
    #[must_use]
    pub fn new(db: DatabaseConnection, memory: MemoryCommit, caps: MemoryCaps, faults: Arc<dyn FaultPoint>) -> Self {
        PgRunCommitter { db, memory, caps, faults }
    }
}

#[async_trait]
impl RunCommitter for PgRunCommitter {
    async fn commit(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError> {
        self.commit_record(h, r).await
    }

    async fn screen_out(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError> {
        self.screen_out_events(h, s).await
    }

    async fn back_off(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        self.back_off_run(h, b).await
    }

    async fn host_fault(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        self.host_fault_run(h, b).await
    }

    async fn release_after_shutdown(&self, h: LeaseHandle) -> Result<(), CommitError> {
        self.release_shutdown(h).await
    }
}

/// The check of a handle before a fenced write of `operation`. A poisoned handle sends no SQL and
/// counts nothing. A run or a token that is not the open claim of the handle is a lost lease.
fn check_handle(h: &LeaseHandle, run: &RunId, token: LeaseToken, operation: &'static str) -> Result<(), CommitError> {
    if h.is_poisoned() {
        return Err(CommitError::LeaseLost);
    }
    if h.open_claim().as_ref() != Some(run) || h.token() != token {
        lost(operation);
        return Err(CommitError::LeaseLost);
    }
    Ok(())
}

/// Step 1 of every fenced transaction: set the lock wait, then lock the lease row of the token
/// and check the expiry after the lock is held (`lock_live_lease`). `false` means that the lease
/// is lost.
async fn lock_lease(txn: &DatabaseTransaction, inbox: Uuid, token: LeaseToken) -> Result<bool, CommitError> {
    txn.execute_unprepared(LOCK_TIMEOUT_SQL).await.map_err(db_err)?;
    lock_live_lease(txn, inbox, token).await.map_err(db_err)
}

/// Roll back after a failed step of `operation`, and return its error. A `LeaseLost` counts one
/// lost lease.
async fn abort(txn: DatabaseTransaction, e: CommitError, operation: &'static str) -> CommitError {
    let _ = txn.rollback().await;
    if matches!(e, CommitError::LeaseLost) {
        lost(operation);
    }
    e
}

/// The back-off of each event of `retry`: it stays pending, is due after its delay and keeps
/// `error` as its `last_error`.
async fn retry_events(txn: &DatabaseTransaction, run: Uuid, retry: &[EventDelay], error: &str) -> Result<(), CommitError> {
    for delay in retry {
        let micros = i64::try_from(delay.delay.as_micros()).unwrap_or(i64::MAX);
        txn.execute_raw(stmt(RETRY_SQL, [delay.event.uuid().into(), run.into(), micros.into(), error.into()]))
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

fn stmt(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The SQLSTATE of a database error, when the error has one.
fn sqlstate(err: &DbErr) -> Option<String> {
    let (DbErr::Query(RuntimeErr::SqlxError(e)) | DbErr::Exec(RuntimeErr::SqlxError(e))) = err else {
        return None;
    };
    e.as_database_error().and_then(|d| d.code()).map(|c| c.into_owned())
}

/// A database error as a commit error. A text that the store cannot encode gets the
/// `UNENCODABLE_PREFIX` and its SQLSTATE, so the caller can tell it from a transient error.
fn db_err(err: DbErr) -> CommitError {
    match sqlstate(&err) {
        Some(code) if UNENCODABLE_CODES.contains(&code.as_str()) => CommitError::Store(format!("{UNENCODABLE_PREFIX}{code}")),
        _ => CommitError::Store(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_with_no_sqlstate_is_a_plain_store_error() {
        let e = db_err(DbErr::Custom("connection reset".into()));
        assert_eq!(e, CommitError::Store("Custom Error: connection reset".into()));
        assert!(!e.is_unencodable());
    }
}
