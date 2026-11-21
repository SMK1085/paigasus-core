// SPDX-License-Identifier: Apache-2.0

//! The Postgres timer store (SMA-788 spec §7.3 and §8).
//!
//! Every read has `WHERE t.inbox_id = $1` (AG4). `apply_in` runs in the run transaction of the
//! caller. It takes the `inbox_lease` row lock itself, and it never locks an `inbox` row. The
//! `Drop` of a `DatabaseTransaction` can panic, so every error path rolls back explicitly.

use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;
use paigasus_agent_host_core::ids::{InboxId, TimerId};
use paigasus_agent_host_core::ports::{RepositoryError, TimerRepository, TzResolver};
use paigasus_agent_host_core::timer::{ApplyError, ApplyReport, CancelOutcome, Timer, TimerCaps, TimerChanges, TimerKind, TimerOrigin, TimerStatus, TimerView};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait, Value};

use super::time::{from_micros, to_micros};
use super::timer_rows::{RowError, TIMER_COLUMNS, civil_param, row_to_view};

/// The Postgres timer store.
#[derive(Clone)]
pub struct PgTimerStore {
    db: DatabaseConnection,
    tz: Arc<dyn TzResolver>,
}

impl std::fmt::Debug for PgTimerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTimerStore").finish_non_exhaustive()
    }
}

impl PgTimerStore {
    /// Build a store on a pool. The schema of m0004 must exist.
    #[must_use]
    pub fn new(db: DatabaseConnection, tz: Arc<dyn TzResolver>) -> Self {
        PgTimerStore { db, tz }
    }

    /// The database clock, from `SELECT now()`.
    pub async fn db_now(&self) -> Result<Timestamp, RepositoryError> {
        let row = self
            .db
            .query_one_raw(stmt("SELECT (EXTRACT(EPOCH FROM now()) * 1000000)::bigint AS n", vec![]))
            .await
            .map_err(repo_err)?
            .ok_or_else(|| RepositoryError::Store("SELECT now() returned no row".to_string()))?;
        let us: i64 = row.try_get("", "n").map_err(repo_err)?;
        from_micros(us).map_err(|e| RepositoryError::Store(e.0))
    }

    /// Apply the timer changes of a run inside the transaction that the caller owns (spec §8).
    ///
    /// It opens a savepoint on `txn`. On success it releases the savepoint, and the changes
    /// commit with the caller's transaction. On any error it rolls back to the savepoint and
    /// returns the error, so `txn` stays usable and holds no part of the change set. It never
    /// commits or rolls back `txn`. It expects READ COMMITTED.
    pub async fn apply_in(&self, txn: &DatabaseTransaction, inbox: &InboxId, caps: &TimerCaps, changes: TimerChanges) -> Result<ApplyReport, ApplyError> {
        let savepoint = txn.begin().await.map_err(store_err)?;
        match apply_body(&savepoint, inbox, caps, changes).await {
            Ok(report) => {
                savepoint.commit().await.map_err(store_err)?;
                // Count and log after the release: a refused apply rolls the run back, and the
                // next attempt would count the same cancel again.
                for (id, outcome) in &report.cancels {
                    if *outcome == CancelOutcome::NotActive {
                        tracing::info!(timer_id = %id.uuid(), "a timer cancel found the timer not active");
                        metrics::counter!(paigasus_observability::names::AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL).increment(1);
                    }
                }
                Ok(report)
            }
            Err(e) => {
                let _ = savepoint.rollback().await;
                Err(e)
            }
        }
    }

    fn views(&self, inbox: &InboxId, rows: &[sea_orm::QueryResult]) -> Result<Vec<TimerView>, RepositoryError> {
        rows.iter().map(|r| row_to_view(r, inbox.org(), self.tz.as_ref()).map_err(row_err)).collect()
    }
}

#[async_trait]
impl TimerRepository for PgTimerStore {
    async fn list_active(&self, inbox: &InboxId, origin: TimerOrigin) -> Result<Vec<TimerView>, RepositoryError> {
        let rows = self
            .db
            .query_all_raw(stmt(
                &format!("SELECT {TIMER_COLUMNS} FROM timer t WHERE t.inbox_id = $1 AND t.origin = $2 AND t.status = 'active' ORDER BY t.next_fire_at, t.id"),
                vec![inbox.uuid().into(), origin.as_str().into()],
            ))
            .await
            .map_err(repo_err)?;
        self.views(inbox, &rows)
    }

    async fn get(&self, inbox: &InboxId, id: TimerId) -> Result<Option<TimerView>, RepositoryError> {
        let rows = self
            .db
            .query_all_raw(stmt(
                &format!("SELECT {TIMER_COLUMNS} FROM timer t WHERE t.inbox_id = $1 AND t.id = $2"),
                vec![inbox.uuid().into(), id.uuid().into()],
            ))
            .await
            .map_err(repo_err)?;
        Ok(self.views(inbox, &rows)?.into_iter().next())
    }

    async fn active_agent_ids(&self, inbox: &InboxId) -> Result<Vec<TimerId>, RepositoryError> {
        let rows = self
            .db
            .query_all_raw(stmt(
                "SELECT t.id FROM timer t WHERE t.inbox_id = $1 AND t.origin = 'agent' AND t.status = 'active' ORDER BY t.next_fire_at, t.id",
                vec![inbox.uuid().into()],
            ))
            .await
            .map_err(repo_err)?;
        rows.iter().map(|r| Ok(TimerId::from_uuid(r.try_get("", "id").map_err(repo_err)?))).collect()
    }

    async fn count_agent_created_since(&self, inbox: &InboxId, since: Timestamp) -> Result<u32, RepositoryError> {
        let row = self
            .db
            .query_one_raw(stmt(
                "SELECT count(*)::bigint AS n FROM timer t WHERE t.inbox_id = $1 AND t.origin = 'agent' \
                 AND t.created_at > 'epoch'::timestamptz + $2::bigint * interval '1 microsecond'",
                vec![inbox.uuid().into(), to_micros(since).into()],
            ))
            .await
            .map_err(repo_err)?
            .ok_or_else(|| RepositoryError::Store("the count query returned no row".to_string()))?;
        let n: i64 = row.try_get("", "n").map_err(repo_err)?;
        u32::try_from(n).map_err(|e| RepositoryError::Store(e.to_string()))
    }
}

/// The work of `apply_in` on the savepoint. The caller releases or rolls it back.
async fn apply_body(txn: &DatabaseTransaction, inbox: &InboxId, caps: &TimerCaps, changes: TimerChanges) -> Result<ApplyReport, ApplyError> {
    // The lease row lock is the fence of the run commit (spec §8). It does not lock `inbox`.
    txn.query_one_raw(stmt("SELECT 1 FROM inbox_lease WHERE inbox_id = $1 FOR UPDATE", vec![inbox.uuid().into()]))
        .await
        .map_err(store_err)?;

    for t in &changes.inserts {
        if t.inbox() != inbox {
            return Err(ApplyError::InboxMismatch { id: t.id() });
        }
        if t.status() != TimerStatus::Active {
            return Err(ApplyError::InvalidInsert { id: t.id() });
        }
    }

    let before = active_agent_count(txn, inbox).await?;

    let mut report = ApplyReport::default();
    for (id, origin) in &changes.cancels {
        let done = txn
            .query_one_raw(stmt(
                "UPDATE timer SET status = 'cancelled', updated_at = now() \
                 WHERE id = $1 AND inbox_id = $2 AND origin = $3 AND status = 'active' RETURNING id",
                vec![id.uuid().into(), inbox.uuid().into(), origin.as_str().into()],
            ))
            .await
            .map_err(store_err)?;
        let outcome = if done.is_some() { CancelOutcome::Cancelled } else { CancelOutcome::NotActive };
        report.cancels.push((*id, outcome));
    }

    for t in &changes.inserts {
        insert_timer(txn, t).await.map_err(store_err)?;
    }

    let after = active_agent_count(txn, inbox).await?;
    if after > caps.max_active_per_inbox && after > before {
        return Err(ApplyError::CapExceeded {
            before,
            after,
            limit: caps.max_active_per_inbox,
        });
    }

    if changes.inserts.iter().any(|t| t.origin() == TimerOrigin::Agent) {
        let row = txn
            .query_one_raw(stmt(
                "SELECT count(*)::bigint AS n FROM timer WHERE inbox_id = $1 AND origin = 'agent' \
                 AND created_at > clock_timestamp() - interval '24 hours'",
                vec![inbox.uuid().into()],
            ))
            .await
            .map_err(store_err)?
            .ok_or_else(|| ApplyError::Store("the budget query returned no row".to_string()))?;
        let created = u32::try_from(row.try_get::<i64>("", "n").map_err(store_err)?).map_err(store_err)?;
        if created > caps.max_agent_timers_created_per_day {
            return Err(ApplyError::BudgetExceeded {
                created,
                limit: caps.max_agent_timers_created_per_day,
            });
        }
    }
    Ok(report)
}

async fn active_agent_count(txn: &DatabaseTransaction, inbox: &InboxId) -> Result<u32, ApplyError> {
    let row = txn
        .query_one_raw(stmt(
            "SELECT count(*)::bigint AS n FROM timer WHERE inbox_id = $1 AND origin = 'agent' AND status = 'active'",
            vec![inbox.uuid().into()],
        ))
        .await
        .map_err(store_err)?
        .ok_or_else(|| ApplyError::Store("the count query returned no row".to_string()))?;
    u32::try_from(row.try_get::<i64>("", "n").map_err(store_err)?).map_err(store_err)
}

async fn insert_timer(txn: &DatabaseTransaction, t: &Timer) -> Result<(), DbErr> {
    let (cron, tz, slot, second): (Option<String>, Option<String>, Option<String>, Option<bool>) = match t.kind() {
        TimerKind::FollowUp => (None, None, None, None),
        TimerKind::Schedule { schedule, next_slot } => (
            Some(schedule.text().to_string()),
            Some(schedule.tz_name().to_string()),
            Some(civil_param(next_slot.civil)),
            Some(next_slot.second),
        ),
    };
    let values: Vec<Value> = vec![
        t.id().uuid().into(),
        t.inbox().uuid().into(),
        t.kind().as_str().into(),
        t.origin().as_str().into(),
        t.thread().map(|th| th.uuid()).into(),
        cron.into(),
        tz.into(),
        slot.into(),
        second.into(),
        t.note().as_str().into(),
        t.next_fire_at().as_second().into(),
    ];
    txn.execute_raw(stmt(
        "INSERT INTO timer (id, inbox_id, kind, origin, status, thread_id, cron, tz, next_slot, next_slot_second, note, next_fire_at) \
         VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8::timestamp, $9, $10, to_timestamp($11::bigint))",
        values,
    ))
    .await?;
    Ok(())
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn repo_err(e: impl ToString) -> RepositoryError {
    RepositoryError::Store(e.to_string())
}

fn row_err(e: RowError) -> RepositoryError {
    RepositoryError::Store(e.reason)
}

fn store_err(e: impl ToString) -> ApplyError {
    ApplyError::Store(e.to_string())
}
