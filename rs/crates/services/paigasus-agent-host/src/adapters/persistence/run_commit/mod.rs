// SPDX-License-Identifier: Apache-2.0

//! The Postgres run committer (spec §6.7, §6.8).
//!
//! Every write of a run end goes through one fenced transaction: the lease row is locked first,
//! then the run row is moved out of `running` with its token. Every COMMIT goes through
//! `commit_or_poison`. The `Drop` of a `DatabaseTransaction` can panic (see `migration_lock.rs`),
//! so every error path rolls back explicitly.

mod commit;

use std::sync::Arc;

use paigasus_agent_host_core::fault::FaultPoint;
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::run::{CommitError, UNENCODABLE_PREFIX};
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, RuntimeErr, Statement, Value};

use crate::adapters::memory_commit::MemoryCommit;

/// `$1` = inbox, `$2` = token. The first statement of a commit that locks a row. No row means
/// that the lease is lost.
const LOCK_SQL: &str = "SELECT token FROM inbox_lease \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp() FOR UPDATE";

/// The lock wait of one commit statement. A wait that ends with this timeout is a `Store` error.
const LOCK_TIMEOUT_SQL: &str = "SET LOCAL lock_timeout = '5s'";

/// The SQLSTATEs of a text that Postgres cannot store: `22P05` (untranslatable character) and
/// `22021` (a byte sequence that is not valid in the encoding).
const UNENCODABLE_CODES: [&str; 2] = ["22P05", "22021"];

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
