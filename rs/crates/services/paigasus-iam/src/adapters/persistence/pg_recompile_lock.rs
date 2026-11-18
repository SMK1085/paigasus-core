// SPDX-License-Identifier: Apache-2.0

//! The advisory lock of the tool policy re-compile job (SMA-781).
//!
//! One transaction holds `pg_try_advisory_xact_lock`. Postgres frees the lock when the
//! transaction ends, so no unlock call exists to miss. The lock does not block a tool policy
//! write: a writer locks the `tool_policy_state` row, not this key.
//!
//! **Release the guard with `release`.** `DatabaseTransaction::Drop` calls
//! `start_rollback().expect(..)` and can panic under connection contention (see
//! `migration_lock.rs`). A dropped guard still frees the lock, but only treat that as a backstop.

use async_trait::async_trait;
use paigasus_iam_core::RepositoryError;
use paigasus_iam_core::tool_policy::{RecompileLock, RecompileLockGuard};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement, TransactionTrait};

use super::map_err;

/// One key for the re-compile job. Distinct from `MIGRATION_LOCK_KEY` and
/// `AUDIT_PARTITION_LOCK_KEY`.
pub const TOOL_POLICY_RECOMPILE_LOCK_KEY: i64 = 5_580_781;

/// Takes the re-compile lock on a Postgres transaction.
pub struct PgRecompileLock {
    db: DatabaseConnection,
}

impl PgRecompileLock {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

struct PgRecompileGuard {
    tx: DatabaseTransaction,
}

#[async_trait]
impl RecompileLock for PgRecompileLock {
    async fn try_acquire(&self) -> Result<Option<Box<dyn RecompileLockGuard>>, RepositoryError> {
        let tx = self.db.begin().await.map_err(map_err)?;
        let row = tx
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT pg_try_advisory_xact_lock({TOOL_POLICY_RECOMPILE_LOCK_KEY}) AS locked"),
            ))
            .await;
        let locked = match row {
            Ok(row) => row.and_then(|r| r.try_get::<bool>("", "locked").ok()).unwrap_or(false),
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(map_err(e));
            }
        };
        if !locked {
            // Explicit rollback: a drop can panic.
            let _ = tx.rollback().await;
            return Ok(None);
        }
        Ok(Some(Box::new(PgRecompileGuard { tx })))
    }
}

#[async_trait]
impl RecompileLockGuard for PgRecompileGuard {
    async fn release(self: Box<Self>) -> Result<(), RepositoryError> {
        self.tx.commit().await.map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::persistence::migration::m0008_partition_audit_log::AUDIT_PARTITION_LOCK_KEY;
    use crate::adapters::persistence::migration_lock::MIGRATION_LOCK_KEY;

    #[test]
    fn the_recompile_lock_key_differs_from_the_other_advisory_keys() {
        assert_ne!(TOOL_POLICY_RECOMPILE_LOCK_KEY, MIGRATION_LOCK_KEY);
        assert_ne!(TOOL_POLICY_RECOMPILE_LOCK_KEY, AUDIT_PARTITION_LOCK_KEY);
    }

    #[test]
    fn the_guard_can_move_into_a_spawned_task() {
        fn assert_send_static<T: Send + 'static>() {}
        assert_send_static::<Box<dyn RecompileLockGuard>>();
    }
}
