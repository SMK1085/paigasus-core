// SPDX-License-Identifier: Apache-2.0

//! Serialises `Migrator::up` across concurrently starting replicas (SMA-559).
//!
//! `sea-orm-migration` does not serialise concurrent `up()`. On Postgres it runs the whole
//! migration set in ONE transaction (`exec_with_connection`, `migrator.rs:261-273`), so a
//! half-applied migration is already impossible — but two migrating transactions can still
//! deadlock, and the loser can fail its boot on a duplicate-object error. This module takes a
//! transaction-scoped advisory lock on the very transaction `Migrator::up` runs in, so Postgres
//! releases it on commit or rollback and no unlock path exists to be missed.
//!
//! **This key serialises migration runs against each other and NOTHING ELSE.** A migration doing
//! DDL on a table a background maintainer also touches still needs that maintainer's own key —
//! see `AUDIT_PARTITION_LOCK_KEY` and spec §2.1. Do not read this module as licence to drop a
//! hand-rolled advisory lock from a future migration.

use std::time::{Duration, Instant};

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait};
use sea_orm_migration::MigratorTrait;

use super::Migrator;

/// Namespaces a whole migration RUN against another run. Must never collide with
/// `AUDIT_PARTITION_LOCK_KEY` (5_580_467).
pub const MIGRATION_LOCK_KEY: i64 = 5_580_559;

/// Fixed backoff between acquisition attempts. No jitter: every loser simply waits and then
/// finds nothing to do, so staggering them buys nothing (contrast `OutboxConfig::wake_debounce_ms`,
/// where the herd is real).
pub const POLL_BACKOFF: Duration = Duration::from_secs(1);

/// At most one "still waiting" line per this interval.
pub const LOG_THROTTLE: Duration = Duration::from_secs(15);

/// The image's `HEALTHCHECK --start-period`, mirrored here so boot can warn when the configured
/// wait exceeds what the container tolerates. `ci/images/run.sh`'s `assert_pins` asserts this
/// constant and `rs/Dockerfile` agree.
pub const IMAGE_START_PERIOD_SECS: u64 = 180;

/// Budget for the migration itself, on top of the lock wait. `IMAGE_START_PERIOD_SECS` is
/// `lock_wait_secs` default + this.
pub const MIGRATION_BUDGET_SECS: u64 = 60;

/// What to do after a failed acquisition attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Sleep this long, then try again. Already clamped to the remaining budget.
    Retry(Duration),
    /// The budget is spent.
    GiveUp,
}

/// The sole authority for backoff and give-up.
///
/// Extracted from the async loop so the timing decision is testable without Docker — the same
/// discipline `docker::env_flag` and `PgPartitionMaintainer::tick`'s `MaintenanceReport` follow.
///
/// Note the do-while property ("always attempt at least once") is NOT expressed here: it belongs
/// to `migrate_under_lock`'s loop structure, because this function is only ever consulted AFTER
/// an attempt has already failed.
pub fn next_poll(elapsed: Duration, wait: Duration) -> Poll {
    let remaining = wait.saturating_sub(elapsed);
    if remaining.is_zero() {
        return Poll::GiveUp;
    }
    // Clamped, so the wait is honoured exactly rather than overshot by up to one interval.
    Poll::Retry(POLL_BACKOFF.min(remaining))
}

/// What a `migrate_under_lock` call actually did.
///
/// Returned rather than discarded because a `Result<(), _>` makes the wait unobservable: a test
/// measuring wall clock around the call cannot tell "waited for the lock" from "ran the
/// migration", so it would pass with the lock deleted. An advisory lock does not block DDL, so
/// that is not a theoretical concern. `PgPartitionMaintainer::tick` returns a `MaintenanceReport`
/// for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationLockOutcome {
    /// Time spent waiting for the lock, excluding the migration itself.
    pub waited: Duration,
    /// Failed acquisition attempts before the successful one. `0` = uncontended.
    pub polls: u32,
    /// Migrations actually applied. `0` on a warm boot.
    pub migrations_applied: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationLockError {
    #[error(
        "timed out after {waited:?} waiting for the migration advisory lock (key {key}); another \
         replica may still be migrating, or a lock may be stranded by a hard-killed pod — see \
         docs/ops/RUNBOOK-containers.md"
    )]
    Contended { waited: Duration, key: i64 },
    #[error("database error while acquiring the migration advisory lock: {0}")]
    Db(#[source] DbErr),
    #[error("migration failed under the advisory lock: {0}")]
    Migrate(#[source] DbErr),
}

/// How many migrations `seaql_migrations` records, or `0` when the table does not exist yet.
///
/// Two statements rather than one `COALESCE`: Postgres plans a whole statement up front, so a
/// `SELECT count(*) FROM seaql_migrations` guarded by a subquery still fails on a fresh database
/// where `Migrator::up`'s own `install()` has not run yet.
async fn applied_count<C: ConnectionTrait>(db: &C) -> Result<u64, DbErr> {
    let present = db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT to_regclass('public.seaql_migrations') IS NOT NULL AS present".to_string(),
        ))
        .await?
        .and_then(|r| r.try_get::<bool>("", "present").ok())
        .unwrap_or(false);
    if !present {
        return Ok(0);
    }
    let row = db
        .query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT count(*)::bigint AS n FROM seaql_migrations".to_string()))
        .await?;
    Ok(row.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0).max(0) as u64)
}

/// Run `Migrator::up` under a transaction-scoped advisory lock, waiting up to `wait` to acquire it.
///
/// **Production code must never call `Migrator::up` bare.** There is exactly one production call
/// site (`main.rs`) and this is it; the remaining call sites are integration tests that
/// deliberately drive migrations step by step.
///
/// The loop is do-while: an attempt always happens before any give-up decision, so even the
/// smallest legal `wait` still asks Postgres once.
pub async fn migrate_under_lock(db: &DatabaseConnection, wait: Duration) -> Result<MigrationLockOutcome, MigrationLockError> {
    let start = Instant::now();
    let mut polls: u32 = 0;
    let mut last_log = start;

    loop {
        // A failure here aborts boot rather than counting against the budget: it means the pool
        // is unusable, which no amount of waiting fixes.
        let txn = db.begin().await.map_err(MigrationLockError::Db)?;

        let acquired = txn
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT pg_try_advisory_xact_lock({MIGRATION_LOCK_KEY}) AS locked"),
            ))
            .await
            .map_err(MigrationLockError::Db)?
            .and_then(|r| r.try_get::<bool>("", "locked").ok())
            .unwrap_or(false);

        if acquired {
            let before = applied_count(&txn).await.map_err(MigrationLockError::Db)?;
            // Rollback is EXPLICIT on every path we own. `DatabaseTransaction::Drop` calls
            // `start_rollback().expect(..)`, which panics if the connection mutex is contended —
            // turning a migration error into a panic in the composition root. (sea-orm-migration
            // drops its own inner savepoint transaction on error; that residual is not ours.)
            if let Err(e) = Migrator::up(&txn, None).await {
                let _ = txn.rollback().await;
                return Err(MigrationLockError::Migrate(e));
            }
            let after = applied_count(&txn).await.map_err(MigrationLockError::Db)?;
            txn.commit().await.map_err(MigrationLockError::Db)?;
            return Ok(MigrationLockOutcome {
                waited: start.elapsed(),
                polls,
                migrations_applied: after.saturating_sub(before),
            });
        }

        let _ = txn.rollback().await;

        match next_poll(start.elapsed(), wait) {
            Poll::GiveUp => {
                let waited = start.elapsed();
                // `main.rs` surfaces a boot error only through a bare `eprintln!`, which bypasses
                // `paigasus_logging` — without this the structured pipeline gets the waiting
                // lines and then silence at the moment that actually matters.
                tracing::error!(?waited, polls, key = MIGRATION_LOCK_KEY, "gave up waiting for the migration advisory lock");
                return Err(MigrationLockError::Contended { waited, key: MIGRATION_LOCK_KEY });
            }
            Poll::Retry(backoff) => {
                if last_log.elapsed() >= LOG_THROTTLE || polls == 0 {
                    tracing::info!(
                        elapsed = ?start.elapsed(),
                        remaining = ?wait.saturating_sub(start.elapsed()),
                        key = MIGRATION_LOCK_KEY,
                        "another replica is migrating; waiting for the migration advisory lock"
                    );
                    last_log = Instant::now();
                }
                polls = polls.saturating_add(1);
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_wait_retries_at_the_full_backoff() {
        assert_eq!(next_poll(Duration::ZERO, Duration::from_secs(1)), Poll::Retry(Duration::from_secs(1)));
    }

    /// The clamp: without it the loop overshoots the budget by up to one backoff interval, and
    /// the "deadline is honoured exactly" claim in the module doc is false.
    #[test]
    fn a_partial_budget_clamps_the_backoff_to_what_remains() {
        assert_eq!(next_poll(Duration::from_millis(900), Duration::from_secs(1)), Poll::Retry(Duration::from_millis(100)));
    }

    #[test]
    fn an_exactly_spent_budget_gives_up() {
        assert_eq!(next_poll(Duration::from_secs(1), Duration::from_secs(1)), Poll::GiveUp);
    }

    /// `saturating_sub`, not `checked_sub` + unwrap: an overshoot past the budget is normal (the
    /// sleep and the round-trip both take time) and must not panic.
    #[test]
    fn an_overshot_budget_gives_up_rather_than_panicking() {
        assert_eq!(next_poll(Duration::from_secs(2), Duration::from_secs(1)), Poll::GiveUp);
    }
}
