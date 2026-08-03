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

/// `now` shifted back by `days`.
fn cutoff(now: DateTime<Utc>, days: u32) -> DateTime<Utc> {
    now - chrono::Duration::days(i64::from(days))
}

/// `$1` = cutoff timestamp, `$2` = batch size.
fn published_sweep_sql() -> &'static str {
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

        if policy.enabled && policy.published_days > 0 {
            match self.sweep(published_sweep_sql(), cutoff(now, policy.published_days), policy).await {
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
            }
        }

        // Runs regardless of the published sweep's outcome (independence — one step failing
        // must not wedge the other, mirroring `PgPartitionMaintainer`'s prune-after-create).
        if policy.enabled && policy.parked_days > 0 {
            match self.sweep(parked_sweep_sql(), cutoff(now, policy.parked_days), policy).await {
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
        let mut total = 0u64;
        let mut passes = 0u32;
        while passes < policy.max_batches_per_tick {
            let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, [Value::from(cutoff), Value::from(policy.batch_size as i64)]);
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
        let n = self.db.query_one(stmt).await?.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0);
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
    fn cutoff_subtracts_whole_days() {
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(cutoff(now, 7), DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z").unwrap().with_timezone(&Utc));
    }
}
