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

use std::time::Duration;

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
