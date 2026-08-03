// SPDX-License-Identifier: Apache-2.0

//! `PgOutboxMaintainer` (SMA-469): the background sweep that bounds `event_outbox`'s growth.
//!
//! Mirrors `PgPartitionMaintainer` (`pg_partition_maintainer`, SMA-467): a `tick` does one unit
//! of work and returns a report; `run` is the `tokio::select!` shutdown-watch loop the
//! composition root spawns. It is deliberately NOT folded into `OutboxRelay::tick` — that
//! would couple a 5-second hot loop to an hourly bulk `DELETE`, make tick latency lumpy, and
//! (decisively) muddy `iam_outbox_relay_ticks_total`, so a retention failure would red the
//! relay's own liveness signal.
//!
//! **Retention and the relay cannot contend, by construction.** The relay's poll predicate is
//! `published_at IS NULL AND parked = false`; both sweep predicates are subsets of its exact
//! complement, so no row is ever visible to both. That claim covers relay-vs-retention ONLY —
//! it says nothing about replay vs. retention or replay vs. replay, which `PgDeadLetters`
//! handles with its own `FOR UPDATE SKIP LOCKED`.
//!
//! `SKIP LOCKED` here lets two maintainer replicas partition the work rather than block. Note
//! the consequence: a pass can return fewer than `batch_size` rows because a PEER replica
//! holds them, so `passes < max_batches_per_tick` does NOT prove the backlog is drained. The
//! next tick resumes.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, Value};

/// The sweep knobs a tick needs, decoupled from `config::OutboxRetentionConfig`
/// (`interval_secs` lives in the loop, not a tick). `Copy` so tests/`run` pass it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxRetentionPolicy {
    /// `false` = perform no deletions. The tick still runs, because it is what refreshes the
    /// `iam_outbox_parked_rows` backlog gauge.
    pub enabled: bool,
    /// Delete published rows older than this. `0` = never.
    pub published_days: u32,
    /// Delete parked rows whose `parked_at` is older than this. `0` = never.
    pub parked_days: u32,
    pub batch_size: u64,
    pub max_batches_per_tick: u32,
}

/// Per-tick outcome, returned (and logged) so tests can assert without scraping logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted_published: u64,
    pub deleted_parked: u64,
    /// Delete passes actually issued. Exists so a test can prove BATCHING happened — with
    /// totals alone, one pass of 2000 and two passes of 1000 are indistinguishable.
    pub passes_published: u32,
    pub passes_parked: u32,
    pub parked_rows: u64,
    pub errored: bool,
}

/// `now` shifted back by `days`, or `None` if that would overflow `DateTime`'s representable
/// range. A checked subtraction is deliberate: chrono's `Sub` impl PANICS on overflow, and
/// `published_days`/`parked_days` are operator-supplied `u32`s that `IamConfig::validate`
/// intentionally leaves un-range-checked (`0` must stay legal as the "never" sentinel) — so a
/// fat-fingered huge value must degrade gracefully here, not kill the spawned maintainer task.
fn cutoff(now: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>> {
    now.checked_sub_signed(chrono::Duration::days(i64::from(days)))
}

/// One sweep step's outcome, decided BEFORE any database access — pure and unit-testable in
/// isolation from `tick`. `days == 0` is the policy's "never" sentinel, enforced HERE (not
/// inside `cutoff`, which has no opinion on `0`); a `days` large enough to overflow `cutoff`'s
/// range degrades to `Overflow` rather than propagating a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepStep {
    /// `days == 0`: this bucket's retention is off; skip silently, no error.
    Disabled,
    /// `cutoff` overflowed `DateTime`'s range for this `days` value. Skip the step, but the
    /// caller must still mark the tick `errored` — a silently skipped sweep would otherwise look
    /// like a quiet no-op instead of the misconfiguration it is.
    Overflow,
    /// Sweep everything older than this cutoff.
    Run(DateTime<Utc>),
}

fn sweep_step(now: DateTime<Utc>, days: u32) -> SweepStep {
    if days == 0 {
        return SweepStep::Disabled;
    }
    match cutoff(now, days) {
        Some(cut) => SweepStep::Run(cut),
        None => SweepStep::Overflow,
    }
}

/// `$1` = cutoff timestamp, `$2` = batch size.
///
/// `pub` (but `#[doc(hidden)]`, not part of the crate's real public API) so
/// `tests/outbox_retention_pg.rs` can `EXPLAIN` the EXACT statement the sweep issues, rather
/// than a hand-copied string that could silently drift from this one (SMA-469 round-1 review,
/// Finding 2).
#[doc(hidden)]
pub fn published_sweep_sql() -> &'static str {
    r#"DELETE FROM "event_outbox" WHERE id IN (
         SELECT id FROM "event_outbox"
         WHERE published_at IS NOT NULL AND published_at < $1 AND parked = false
         ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
       )"#
}

/// `$1` = cutoff timestamp, `$2` = batch size.
fn parked_sweep_sql() -> &'static str {
    r#"DELETE FROM "event_outbox" WHERE id IN (
         SELECT id FROM "event_outbox"
         WHERE parked = true AND parked_at IS NOT NULL AND parked_at < $1
         ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
       )"#
}

#[derive(Clone)]
pub struct PgOutboxMaintainer {
    db: DatabaseConnection,
}

impl PgOutboxMaintainer {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgOutboxMaintainer { db }
    }

    /// One maintenance unit: sweep published, then (independently) sweep parked, then always
    /// refresh the backlog gauge. Errors are logged + counted, never propagated — the loop
    /// keeps running, exactly like `PgPartitionMaintainer::tick`.
    pub async fn tick(&self, now: DateTime<Utc>, policy: OutboxRetentionPolicy) -> SweepReport {
        let mut report = SweepReport::default();

        if policy.enabled {
            match sweep_step(now, policy.published_days) {
                SweepStep::Disabled => {}
                SweepStep::Overflow => {
                    report.errored = true;
                    tracing::warn!(
                        published_days = policy.published_days,
                        "outbox published_days cutoff overflowed DateTime's representable range; skipping the published sweep this tick"
                    );
                }
                SweepStep::Run(cut) => match self.sweep(published_sweep_sql(), cut, policy).await {
                    Ok((n, passes)) => {
                        report.deleted_published = n;
                        report.passes_published = passes;
                    }
                    Err((n, passes, e)) => {
                        // A pass that errors aborts only ITS OWN step's loop; the rows already
                        // deleted are still reported.
                        report.deleted_published = n;
                        report.passes_published = passes;
                        report.errored = true;
                        tracing::warn!(error = %e, "outbox published-row sweep failed; will retry next tick");
                    }
                },
            }
        }

        // Runs regardless of the published sweep's outcome (independence — one step failing
        // must not wedge the other, mirroring `PgPartitionMaintainer`'s prune-after-create).
        if policy.enabled {
            match sweep_step(now, policy.parked_days) {
                SweepStep::Disabled => {}
                SweepStep::Overflow => {
                    report.errored = true;
                    tracing::warn!(
                        parked_days = policy.parked_days,
                        "outbox parked_days cutoff overflowed DateTime's representable range; skipping the parked sweep this tick"
                    );
                }
                SweepStep::Run(cut) => match self.sweep(parked_sweep_sql(), cut, policy).await {
                    Ok((n, passes)) => {
                        report.deleted_parked = n;
                        report.passes_parked = passes;
                    }
                    Err((n, passes, e)) => {
                        report.deleted_parked = n;
                        report.passes_parked = passes;
                        report.errored = true;
                        tracing::warn!(error = %e, "outbox parked-row sweep failed; will retry next tick");
                    }
                },
            }
        }

        // ALWAYS — including when `enabled = false`. This gauge is the dead-letter backlog
        // signal, and losing it because deletion was paused would blind the operator exactly
        // when they are most likely to have paused it.
        match self.parked_row_count().await {
            Ok(n) => {
                report.parked_rows = n;
                gauge!(names::IAM_OUTBOX_PARKED_ROWS).set(n as f64);
            }
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "outbox parked-row gauge query failed");
            }
        }

        counter!(names::IAM_OUTBOX_RETENTION_TICKS_TOTAL, "result" => if report.errored { "error" } else { "ok" }).increment(1);
        counter!(names::IAM_OUTBOX_ROWS_DELETED_TOTAL, "reason" => "published").increment(report.deleted_published);
        counter!(names::IAM_OUTBOX_ROWS_DELETED_TOTAL, "reason" => "parked").increment(report.deleted_parked);
        tracing::info!(
            deleted_published = report.deleted_published,
            deleted_parked = report.deleted_parked,
            passes_published = report.passes_published,
            passes_parked = report.passes_parked,
            parked_rows = report.parked_rows,
            errored = report.errored,
            "outbox retention tick"
        );
        report
    }

    /// Batched delete: repeat `sql` until a pass affects fewer than `batch_size` rows or
    /// `max_batches_per_tick` is reached. Each pass is its OWN autocommit statement — never one
    /// long transaction holding locks across the whole sweep. On error, returns what was
    /// deleted before it, so a partial sweep is still reported honestly.
    async fn sweep(&self, sql: &str, cutoff: DateTime<Utc>, policy: OutboxRetentionPolicy) -> Result<(u64, u32), (u64, u32, DbErr)> {
        // The loop's `affected < policy.batch_size` exit condition can never fire when
        // `batch_size == 0` (`0 < 0` is false), so a directly-constructed zero-batch policy
        // would otherwise burn every `max_batches_per_tick` pass on no-op round-trips.
        // `IamConfig::validate` rejects `batch_size == 0` today, but `OutboxRetentionPolicy` and
        // `tick` are `pub` and constructible without going through config validation — enforce
        // the invariant here too rather than relying on a caller to have checked it.
        if policy.batch_size == 0 {
            return Ok((0, 0));
        }

        // `batch_size` is `u64` in the policy but Postgres `LIMIT` takes a signed integer;
        // `try_from` + clamp to `i64::MAX` rather than a wrapping `as` cast (mirrors `main.rs`'s
        // `max_attempts` u32->i32 narrowing) — a wrapped-negative `LIMIT` is rejected by
        // Postgres, so every subsequent pass would then error forever: one bad config value
        // permanently breaking the sweep, rather than just running with a very large limit.
        let batch_limit = i64::try_from(policy.batch_size).unwrap_or(i64::MAX);

        let mut total = 0u64;
        let mut passes = 0u32;
        while passes < policy.max_batches_per_tick {
            let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, [Value::from(cutoff), Value::from(batch_limit)]);
            let affected = match self.db.execute(stmt).await {
                Ok(r) => r.rows_affected(),
                Err(e) => return Err((total, passes, e)),
            };
            passes += 1;
            total += affected;
            if affected < policy.batch_size {
                break;
            }
        }
        Ok((total, passes))
    }

    async fn parked_row_count(&self) -> Result<u64, DbErr> {
        let stmt = Statement::from_string(DbBackend::Postgres, r#"SELECT count(*) AS n FROM "event_outbox" WHERE parked = true"#.to_string());
        let row = self.db.query_one(stmt).await?;
        // `count(*)` always returns exactly one row, so `row` being `None` is unreachable in
        // practice — but a genuine column-decode failure MUST be propagated, not swallowed. The
        // previous `.and_then(|r| r.try_get(...).ok()).unwrap_or(0)` turned a broken read into a
        // fabricated "0 parked rows": a silent all-clear on the dead-letter backlog alarm.
        let n: i64 = match row {
            Some(r) => r.try_get("", "n")?,
            None => 0,
        };
        Ok(n.max(0) as u64)
    }

    /// The shutdown-watch loop (mirrors `PgPartitionMaintainer::run`/`OutboxRelay::run`):
    /// sleep `interval`, tick, repeat until `shutdown` resolves.
    pub async fn run<S>(self, policy: OutboxRetentionPolicy, interval: Duration, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => { self.tick(Utc::now(), policy).await; }
                () = &mut shutdown => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_sweep_sql_excludes_parked_rows_and_bounds_the_batch() {
        let sql = published_sweep_sql();
        assert!(sql.contains("published_at IS NOT NULL"), "{sql}");
        // Redundant TODAY (the relay sets published_at or parked, never both) but nothing
        // ENFORCES that, and `replay_in` is now a second writer of these columns. §3.3's
        // promise that parked rows never age out by default must hold structurally.
        assert!(sql.contains("parked = false"), "published sweep must never touch a parked row: {sql}");
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
        assert!(sql.contains("LIMIT $2"), "{sql}");
    }

    #[test]
    fn parked_sweep_sql_requires_a_known_park_time() {
        let sql = parked_sweep_sql();
        assert!(sql.contains("parked = true"), "{sql}");
        assert!(sql.contains("parked_at IS NOT NULL"), "a row with an unknown park time must never be swept: {sql}");
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
    }

    #[test]
    fn each_sweep_sql_binds_cutoff_then_batch_size_exactly_once() {
        // A swapped $1/$2 binding (cutoff where the batch size is expected, or vice versa) is
        // invisible to the substring checks above — pin the exact shape instead.
        for sql in [published_sweep_sql(), parked_sweep_sql()] {
            assert_eq!(sql.matches("$1").count(), 1, "{sql}");
            assert_eq!(sql.matches("$2").count(), 1, "{sql}");
            assert!(sql.find("$1").unwrap() < sql.find("$2").unwrap(), "$1 (cutoff) must precede $2 (batch size): {sql}");
        }
    }

    #[test]
    fn cutoff_subtracts_whole_days() {
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(cutoff(now, 7), Some(DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z").unwrap().with_timezone(&Utc)));
    }

    #[test]
    fn cutoff_of_zero_days_is_now_not_a_never_sentinel() {
        // `cutoff` has no opinion on `0` — the "0 = never" sentinel is enforced by `sweep_step`
        // (and, through it, `tick`), not here.
        let now = Utc::now();
        assert_eq!(cutoff(now, 0), Some(now));
    }

    #[test]
    fn cutoff_does_not_panic_on_an_overflowing_days_value() {
        // `published_days`/`parked_days` are un-range-checked `u32`s from config (Finding I1) —
        // a fat-fingered huge value must degrade to `None`, not panic chrono's `Sub` impl.
        let now = Utc::now();
        assert_eq!(cutoff(now, u32::MAX), None);
    }

    #[test]
    fn sweep_step_treats_zero_as_disabled_and_overflow_as_an_error_signal() {
        let now = Utc::now();
        assert_eq!(sweep_step(now, 0), SweepStep::Disabled);
        assert_eq!(sweep_step(now, u32::MAX), SweepStep::Overflow);
        assert!(matches!(sweep_step(now, 7), SweepStep::Run(_)));
    }
}
