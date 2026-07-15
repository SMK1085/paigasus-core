// SPDX-License-Identifier: Apache-2.0

//! `PgPartitionMaintainer` (SMA-467): the background task that keeps `audit_log`'s monthly leaf
//! partitions created ahead of time and drops aged-out ones per an outcome-aware retention policy.
//!
//! Mirrors `OutboxRelay` (`adapters::events::relay`): a `tick` does one unit of work and returns a
//! report; `run` is the `tokio::select!` shutdown-watch loop the composition root spawns.
//!
//! **Never stalls audit inserts.** Each DDL op runs in its OWN short transaction that first takes
//! `pg_advisory_xact_lock(AUDIT_PARTITION_LOCK_KEY)` (one replica does DDL at a time; same key as
//! m0008 so the swap and a tick never overlap) and `SET LOCAL lock_timeout` so a CREATE/DROP that
//! would block behind live-insert locks on the parent BACKS OFF (errors, retried next tick) instead
//! of queueing and stalling all audit writes (hence mutations, since committed audit rows are
//! in-txn). `prune` is attempted independently of `ensure_partitions_ahead`, so a create-ahead
//! failure (e.g. a polluted default) can't wedge retention.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};

use super::migration::m0008_partition_audit_log::AUDIT_PARTITION_LOCK_KEY;

/// The retention knobs a tick needs, decoupled from `config::RetentionConfig` (`enabled`/
/// `interval_secs` live in the loop, not a tick). `Copy` so tests/`run` pass it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub ahead_months: u32,
    pub denied_months: u32,
    pub committed_months: u32,
}

/// Per-tick outcome, returned (and logged) so tests can assert without scraping logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceReport {
    pub created: u64,
    pub dropped_denied: u64,
    pub dropped_committed: u64,
    pub errored: bool,
}

const LOCK_TIMEOUT: &str = "5s";

#[derive(Clone)]
pub struct PgPartitionMaintainer {
    db: DatabaseConnection,
}

impl PgPartitionMaintainer {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgPartitionMaintainer { db }
    }

    /// One maintenance unit: create-ahead, then (independently) prune. Errors are logged + counted,
    /// never propagated — the loop keeps running.
    pub async fn tick(&self, now: DateTime<Utc>, policy: RetentionPolicy) -> MaintenanceReport {
        let mut report = MaintenanceReport::default();

        match self.ensure_partitions_ahead(now, policy.ahead_months).await {
            Ok(n) => report.created = n,
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "audit partition create-ahead failed; will retry next tick");
            }
        }
        // prune runs regardless of create-ahead's outcome (independence, §5.1).
        match self.prune(now, policy).await {
            Ok((d, c)) => {
                report.dropped_denied = d;
                report.dropped_committed = c;
            }
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "audit partition prune failed; will retry next tick");
            }
        }

        // Refresh the default-partition-rows gauge (the "create-ahead fell behind" signal).
        match self.default_partition_rows().await {
            Ok(rows) => gauge!(names::IAM_AUDIT_DEFAULT_PARTITION_ROWS).set(rows as f64),
            Err(e) => tracing::warn!(error = %e, "audit default-partition-rows gauge query failed"),
        }

        counter!(names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL, "result" => if report.errored { "error" } else { "ok" }).increment(1);
        counter!(names::IAM_AUDIT_PARTITIONS_CREATED_TOTAL).increment(report.created);
        counter!(names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL, "outcome" => "denied").increment(report.dropped_denied);
        counter!(names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL, "outcome" => "committed").increment(report.dropped_committed);
        tracing::info!(
            created = report.created,
            dropped_denied = report.dropped_denied,
            dropped_committed = report.dropped_committed,
            errored = report.errored,
            "audit partition maintenance tick"
        );
        report
    }

    /// `CREATE TABLE IF NOT EXISTS` a monthly leaf for both outcome subtrees for each month in
    /// `[now, now + ahead_months]`. Each CREATE is its own locked, lock_timeout'd transaction.
    ///
    /// `created` only counts leaves that did NOT already exist. The existing leaves for each
    /// subtree are fetched ONCE up front (`child_leaves`) rather than re-derived from the CREATE's
    /// own outcome, because `CREATE TABLE IF NOT EXISTS` succeeds silently whether or not it
    /// actually created anything — counting every successful CREATE (the previous behavior) made
    /// `iam_audit_partitions_created_total` climb by `(ahead_months+1)*2` on EVERY tick forever,
    /// even once every target leaf already existed. The `IF NOT EXISTS` CREATE is still issued
    /// (kept for safety against a concurrent create race), just no longer unconditionally counted.
    async fn ensure_partitions_ahead(&self, now: DateTime<Utc>, ahead_months: u32) -> Result<u64, DbErr> {
        let mut created = 0;
        let mut existing: [(&str, HashSet<String>); 2] = [
            ("committed", self.child_leaves("committed").await?.into_iter().collect()),
            ("denied", self.child_leaves("denied").await?.into_iter().collect()),
        ];
        let mut ym = (now.year(), now.month());
        for _ in 0..=ahead_months {
            for (sub, leaves) in &mut existing {
                let leaf_name = format!("audit_log_{sub}_{:04}_{:02}", ym.0, ym.1);
                if !leaves.contains(&leaf_name) {
                    let ddl = month_leaf_ddl(sub, ym.0, ym.1);
                    self.run_ddl(&ddl).await?;
                    leaves.insert(leaf_name);
                    created += 1;
                }
            }
            ym = add_one_month(ym);
        }
        Ok(created)
    }

    /// Drop denied leaves older than `denied_months` and (only if `committed_months > 0`) committed
    /// leaves older than `committed_months`. Enumerates ACTUAL child leaves from the catalog and
    /// drops those whose parsed `YYYY_MM` is strictly before the cutoff month. Never drops a
    /// `*_default`. Returns `(dropped_denied, dropped_committed)`.
    async fn prune(&self, now: DateTime<Utc>, policy: RetentionPolicy) -> Result<(u64, u64), DbErr> {
        let mut dropped = (0u64, 0u64);
        for (sub, months, slot) in [("denied", policy.denied_months, 0usize), ("committed", policy.committed_months, 1usize)] {
            if months == 0 {
                continue; // 0 = never drop this outcome
            }
            let cutoff = subtract_months((now.year(), now.month()), months);
            for leaf in self.child_leaves(sub).await? {
                if let Some(ym) = parse_leaf_month(&leaf, sub)
                    && ym < cutoff
                {
                    self.run_ddl(&format!("DROP TABLE IF EXISTS {leaf};")).await?;
                    if slot == 0 { dropped.0 += 1 } else { dropped.1 += 1 }
                }
            }
        }
        Ok(dropped)
    }

    /// Names of the concrete leaf partitions under `audit_log_<sub>` (excludes the default, whose
    /// name is `audit_log_<sub>_default` and never parses to a month).
    async fn child_leaves(&self, sub: &str) -> Result<Vec<String>, DbErr> {
        let stmt = Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT c.relname AS name FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'audit_log_{sub}'::regclass"),
        );
        let rows = self.db.query_all(stmt).await?;
        Ok(rows.iter().filter_map(|r| r.try_get::<String>("", "name").ok()).collect())
    }

    async fn default_partition_rows(&self) -> Result<i64, DbErr> {
        let stmt = Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT (SELECT count(*) FROM audit_log_committed_default) + (SELECT count(*) FROM audit_log_denied_default) + (SELECT count(*) FROM audit_log_other) AS n".to_string(),
        );
        Ok(self.db.query_one(stmt).await?.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0))
    }

    /// Run one DDL statement in its own transaction under the advisory lock + a bounded
    /// `lock_timeout` (so it backs off rather than stalling live inserts).
    async fn run_ddl(&self, ddl: &str) -> Result<(), DbErr> {
        let txn = self.db.begin().await?;
        txn.execute_unprepared("SET LOCAL TimeZone = 'UTC';").await?;
        txn.execute_unprepared(&format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}';")).await?;
        txn.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({AUDIT_PARTITION_LOCK_KEY});")).await?;
        txn.execute_unprepared(ddl).await?;
        txn.commit().await
    }

    /// The shutdown-watch loop (mirrors `OutboxRelay::run`): sleep `interval`, tick, repeat until
    /// `shutdown` resolves.
    pub async fn run<S>(self, policy: RetentionPolicy, interval: Duration, shutdown: S)
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

fn month_leaf_ddl(sub: &str, year: i32, month1: u32) -> String {
    let (ny, nm) = add_one_month((year, month1));
    format!(
        "CREATE TABLE IF NOT EXISTS audit_log_{sub}_{year:04}_{month1:02} \
         PARTITION OF audit_log_{sub} \
         FOR VALUES FROM (TIMESTAMPTZ '{year:04}-{month1:02}-01 00:00:00+00') \
         TO (TIMESTAMPTZ '{ny:04}-{nm:02}-01 00:00:00+00');"
    )
}

fn add_one_month((y, m): (i32, u32)) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

/// `(y, m)` shifted back by `n` months.
fn subtract_months((y, m): (i32, u32), n: u32) -> (i32, u32) {
    let total = (y * 12 + (m as i32 - 1)) - n as i32;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

/// Parse `audit_log_<sub>_YYYY_MM` → `(YYYY, MM)`; `None` for the default (`…_default`) or any
/// non-month child.
fn parse_leaf_month(name: &str, sub: &str) -> Option<(i32, u32)> {
    let suffix = name.strip_prefix(&format!("audit_log_{sub}_"))?;
    let (y, m) = suffix.split_once('_')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtract_months_crosses_year_boundary() {
        assert_eq!(subtract_months((2026, 7), 3), (2026, 4));
        assert_eq!(subtract_months((2026, 2), 3), (2025, 11));
        assert_eq!(subtract_months((2026, 1), 1), (2025, 12));
    }
    #[test]
    fn parse_leaf_month_ignores_default() {
        assert_eq!(parse_leaf_month("audit_log_denied_2026_07", "denied"), Some((2026, 7)));
        assert_eq!(parse_leaf_month("audit_log_denied_default", "denied"), None);
    }
}
