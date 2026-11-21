// SPDX-License-Identifier: Apache-2.0

//! The Postgres scheduler (SMA-788 spec §11.1). One short transaction fires one due row.
//!
//! The transaction locks the row with `FOR UPDATE OF t SKIP LOCKED`. It never locks an `inbox`
//! row, so ingress does not wait for a fire (AG7). It inserts the event and updates the timer in
//! the same transaction, so a crash between the two leaves nothing, and the next pass fires the
//! same event again with the same `ce_id` (AG2, G7). A bad row gets a back-off in a new
//! transaction and never stops the pass (AG6). A transient database error leaves the row
//! unchanged and stops the pass (AG8). `DatabaseTransaction::drop` can panic, so every path ends
//! with an explicit commit or rollback.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::ports::{InsertOutcome, RepositoryError, Scheduler, TzResolver};
use paigasus_agent_host_core::timer::{FirePlan, Next, PlanError, Timer, WalkError, plan_fire};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult, RuntimeErr, Statement, TransactionTrait, Value};
use uuid::Uuid;

use super::inbox_repository::insert_event_in;
use super::time::from_micros;
use super::timer_rows::{TIMER_COLUMNS, civil_param, row_to_view};
use crate::adapters::id::mint_uuid;
use crate::adapters::timer_event::build_fire_event;

// A panic in `plan_fire` must not end the pass: `catch_unwind` turns it into a permanent error of
// one row. With `panic = "abort"` there is no unwind, so the guard would not work (spec §11.1).
#[cfg(not(panic = "unwind"))]
compile_error!("paigasus-agent-host needs panic = \"unwind\": the scheduler catches a panic in plan_fire for one row (SMA-788 spec §11.1 step 3)");

/// The longest back-off of a bad row.
const BACKOFF_CAP: Duration = Duration::from_secs(3600);

/// The settings of the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerSettings {
    /// `SET LOCAL statement_timeout` of each transaction.
    pub statement_timeout: Duration,
    /// The failed attempts after which a timer goes to `failed`.
    pub max_fire_attempts: u32,
    /// The back-off after the first failed attempt. It doubles for each next attempt.
    pub backoff_base: Duration,
    /// The most steps of the walk to the next slot.
    pub walk_bound: u32,
}

/// A hook that runs after the event insert and before the timer update, in the same transaction.
/// It exists for tests that hold or crash a fire transaction.
#[async_trait]
pub trait AfterInsertHook: Send + Sync {
    /// Called with the open fire transaction.
    async fn after_insert(&self, txn: &DatabaseTransaction);
}

/// The class of a database error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// The same row fails again on the next attempt (SQLSTATE class 22 or 23).
    Permanent,
    /// The cause is outside the row. The row stays as it is.
    Transient,
}

/// The back-off after `attempts` failed attempts: `base * 2^(attempts - 1)`, at most one hour.
#[must_use]
pub fn backoff(base: Duration, attempts: u32) -> Duration {
    let factor = 2u32.saturating_pow(attempts.saturating_sub(1));
    base.saturating_mul(factor).min(BACKOFF_CAP)
}

/// The SQLSTATE of a database error, when the error has one.
fn sqlstate(err: &DbErr) -> Option<String> {
    let (DbErr::Query(RuntimeErr::SqlxError(e)) | DbErr::Exec(RuntimeErr::SqlxError(e))) = err else {
        return None;
    };
    match e.as_ref() {
        sea_orm::sqlx::Error::Database(db) => db.code().map(|c| c.into_owned()),
        _ => None,
    }
}

/// Class 22 (data exception) and class 23 (integrity violation) are permanent. Every other error
/// is transient.
#[must_use]
pub fn classify(err: &DbErr) -> ErrorClass {
    match sqlstate(err) {
        Some(code) if code.starts_with("22") || code.starts_with("23") => ErrorClass::Permanent,
        _ => ErrorClass::Transient,
    }
}

/// Why one row did not fire.
#[derive(Debug)]
enum Fail {
    /// The row fails again. `category` is a fixed text for the `failure` column.
    Permanent { category: String },
    /// The cause is outside the row.
    Transient(String),
}

impl Fail {
    fn permanent(category: &str) -> Fail {
        Fail::Permanent { category: category.to_string() }
    }

    /// The failure of a statement on a locked row.
    fn db(err: &DbErr) -> Fail {
        match classify(err) {
            ErrorClass::Permanent => {
                let code: String = sqlstate(err).unwrap_or_default().chars().filter(char::is_ascii_alphanumeric).collect();
                Fail::Permanent {
                    category: format!("database error SQLSTATE {code}"),
                }
            }
            ErrorClass::Transient => Fail::Transient(err.to_string()),
        }
    }
}

/// What the scheduler knows about the locked row before it decodes the row.
#[derive(Debug, Clone)]
struct Ctx {
    id: Uuid,
    kind: String,
    origin: String,
    attempts: i64,
}

/// A failure, with the row when the row is known.
struct Failure {
    ctx: Option<Ctx>,
    fail: Fail,
}

impl Failure {
    fn transient(err: &DbErr) -> Failure {
        Failure {
            ctx: None,
            fail: Fail::Transient(err.to_string()),
        }
    }
}

/// One fired row.
struct Fired {
    ctx: Ctx,
    kind: &'static str,
    origin: &'static str,
    duplicate: bool,
}

/// Run `f`. A panic in it is a permanent failure of the row. The payload is not read: it can hold
/// the note.
fn guarded<T>(f: impl FnOnce() -> Result<T, Fail>) -> Result<T, Fail> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(Fail::permanent("panic in plan_fire")))
}

/// The scheduler on a Postgres pool.
#[derive(Clone)]
pub struct PgScheduler {
    db: DatabaseConnection,
    tz: Arc<dyn TzResolver>,
    settings: SchedulerSettings,
    hook: Option<Arc<dyn AfterInsertHook>>,
}

impl std::fmt::Debug for PgScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgScheduler").field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl PgScheduler {
    /// Build a scheduler on a pool. The schema of m0004 must exist.
    #[must_use]
    pub fn new(db: DatabaseConnection, tz: Arc<dyn TzResolver>, settings: SchedulerSettings) -> Self {
        PgScheduler { db, tz, settings, hook: None }
    }

    /// Set a test hook that runs after the event insert of every fire.
    #[must_use]
    pub fn with_after_insert_hook(mut self, hook: Arc<dyn AfterInsertHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// Fire the oldest due row that no other transaction holds. `false` means there is no row.
    async fn fire_one(&self) -> Result<bool, RepositoryError> {
        let txn = self.db.begin().await.map_err(|e| transient(&e.to_string()))?;
        let failure = match self.attempt(&txn).await {
            Ok(None) => {
                txn.rollback().await.map_err(|e| transient(&e.to_string()))?;
                return Ok(false);
            }
            Ok(Some(fired)) => match txn.commit().await {
                Ok(()) => {
                    metrics::counter!(
                        paigasus_observability::names::AGENT_HOST_TIMER_FIRED_TOTAL,
                        "kind" => fired.kind,
                        "origin" => fired.origin,
                        "outcome" => if fired.duplicate { "duplicate" } else { "inserted" }
                    )
                    .increment(1);
                    return Ok(true);
                }
                Err(e) => Failure {
                    ctx: Some(fired.ctx),
                    fail: Fail::db(&e),
                },
            },
            Err(failure) => {
                // Roll back first. Errors of the rollback itself do not change the answer.
                let _ = txn.rollback().await;
                failure
            }
        };
        match failure {
            Failure {
                ctx: Some(ctx),
                fail: Fail::Permanent { category },
            } => {
                metrics::counter!(paigasus_observability::names::AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL, "class" => "permanent").increment(1);
                // `false` means another worker handled the row. The row is still processed.
                self.back_off(&ctx, &category).await?;
                Ok(true)
            }
            Failure {
                fail: Fail::Permanent { category }, ..
            } => Err(transient(&category)),
            Failure { fail: Fail::Transient(message), .. } => Err(transient(&message)),
        }
    }

    /// The work of one fire inside the transaction. The caller commits or rolls back.
    async fn attempt(&self, txn: &DatabaseTransaction) -> Result<Option<Fired>, Failure> {
        let ms = self.settings.statement_timeout.as_millis().max(1);
        txn.execute_unprepared(&format!("SET LOCAL statement_timeout = '{ms}ms'")).await.map_err(|e| Failure::transient(&e))?;
        let now = db_now_in(txn).await.map_err(|e| Failure::transient(&e))?;
        let sql = format!(
            "SELECT {TIMER_COLUMNS}, i.org_id FROM timer t JOIN inbox i ON i.id = t.inbox_id \
             WHERE t.status = 'active' AND t.next_fire_at <= now() ORDER BY t.next_fire_at LIMIT 1 FOR UPDATE OF t SKIP LOCKED"
        );
        let Some(row) = txn.query_one_raw(stmt(&sql, vec![])).await.map_err(|e| Failure::transient(&e))? else {
            return Ok(None);
        };
        let (ctx, org) = read_ctx(&row).map_err(|e| Failure::transient(&e))?;
        let fail = |fail: Fail| Failure { ctx: Some(ctx.clone()), fail };

        let (timer, plan, event) = guarded(|| {
            let view = row_to_view(&row, org, self.tz.as_ref()).map_err(|e| Fail::permanent(e.category))?;
            let plan = plan_fire(&view.timer, now, self.settings.walk_bound).map_err(plan_category)?;
            let event = build_fire_event(&view.timer, plan.fire_at, now, EventId::from_uuid(mint_uuid())).map_err(|_| Fail::permanent("event build failed"))?;
            Ok((view.timer, plan, event))
        })
        .map_err(fail)?;

        let duplicate = match insert_event_in(txn, &event).await.map_err(|e| fail(Fail::db(&e)))? {
            InsertOutcome::Inserted { .. } => false,
            InsertOutcome::Duplicate { payload_matches, .. } => {
                if !payload_matches {
                    tracing::warn!(timer_id = %ctx.id, "a timer event exists with the same key and another payload");
                }
                true
            }
        };
        if let Some(hook) = &self.hook {
            hook.after_insert(txn).await;
        }
        update_fired(txn, &timer, &plan).await.map_err(|e| fail(Fail::db(&e)))?;
        Ok(Some(Fired {
            kind: timer.kind().as_str(),
            origin: timer.origin().as_str(),
            ctx,
            duplicate,
        }))
    }

    /// Put a bad row back by the back-off, or to `failed` at the last attempt. It runs in a new
    /// transaction, after the fire transaction rolled back. It returns `false` when another worker
    /// already handled the row since this worker read `ctx.attempts`.
    async fn back_off(&self, ctx: &Ctx, category: &str) -> Result<bool, RepositoryError> {
        let txn = self.db.begin().await.map_err(|e| transient(&e.to_string()))?;
        match self.back_off_in(&txn, ctx, category).await {
            Ok(status) => {
                txn.commit().await.map_err(|e| transient(&e.to_string()))?;
                match status.as_deref() {
                    None => return Ok(false),
                    Some("failed") => {
                        metrics::counter!(paigasus_observability::names::AGENT_HOST_TIMER_FAILED_TOTAL, "kind" => ctx.kind.clone(), "origin" => ctx.origin.clone()).increment(1);
                        tracing::warn!(timer_id = %ctx.id, category, "a timer is failed after too many attempts");
                    }
                    Some(_) => tracing::info!(timer_id = %ctx.id, category, "a timer fire failed and is backed off"),
                }
                Ok(true)
            }
            Err(e) => {
                let _ = txn.rollback().await;
                Err(transient(&e.to_string()))
            }
        }
    }

    /// Back off one row from the `fire_attempts` value that a fire read. It returns `false` when
    /// the row changed since that read, so no second attempt is counted. It exists for the test of
    /// that race.
    ///
    /// # Errors
    /// A database error.
    pub async fn back_off_after_read(&self, id: Uuid, attempts_read: i64, category: &str) -> Result<bool, RepositoryError> {
        let row = self
            .db
            .query_one_raw(stmt("SELECT kind, origin FROM timer WHERE id = $1", vec![id.into()]))
            .await
            .map_err(|e| transient(&e.to_string()))?
            .ok_or_else(|| RepositoryError::Store("the timer does not exist".to_string()))?;
        let ctx = Ctx {
            id,
            kind: row.try_get("", "kind").map_err(|e| transient(&e.to_string()))?,
            origin: row.try_get("", "origin").map_err(|e| transient(&e.to_string()))?,
            attempts: attempts_read,
        };
        self.back_off(&ctx, category).await
    }

    /// The UPDATE matches only while `fire_attempts` still has the value that the fire read. The
    /// wait comes from that same value. No row means that another worker handled the row.
    async fn back_off_in(&self, txn: &DatabaseTransaction, ctx: &Ctx, category: &str) -> Result<Option<String>, DbErr> {
        let ms = self.settings.statement_timeout.as_millis().max(1);
        txn.execute_unprepared(&format!("SET LOCAL statement_timeout = '{ms}ms'")).await?;
        let now = db_now_in(txn).await?;
        let attempts = u32::try_from(ctx.attempts.saturating_add(1).max(1)).unwrap_or(u32::MAX);
        let wait = backoff(self.settings.backoff_base, attempts);
        let next = round_up_to_second(now).saturating_add(i64::try_from(wait.as_secs()).unwrap_or(i64::MAX));
        let row = txn
            .query_one_raw(stmt(
                "UPDATE timer SET fire_attempts = fire_attempts + 1, updated_at = now(), \
                 status = CASE WHEN fire_attempts + 1 >= $3 THEN 'failed' ELSE status END, \
                 failure = CASE WHEN fire_attempts + 1 >= $3 THEN $4::text ELSE failure END, \
                 next_fire_at = CASE WHEN fire_attempts + 1 >= $3 THEN next_fire_at ELSE to_timestamp($2::bigint) END \
                 WHERE id = $1 AND status = 'active' AND fire_attempts = $5::bigint RETURNING status",
                vec![
                    ctx.id.into(),
                    next.into(),
                    i64::from(self.settings.max_fire_attempts).into(),
                    failure_text(category).into(),
                    ctx.attempts.into(),
                ],
            ))
            .await?;
        row.map(|r| r.try_get::<String>("", "status")).transpose()
    }
}

#[async_trait]
impl Scheduler for PgScheduler {
    async fn fire_due(&self, limit: u32) -> Result<usize, RepositoryError> {
        let mut processed = 0;
        for _ in 0..limit {
            if !self.fire_one().await? {
                break;
            }
            processed += 1;
        }
        Ok(processed)
    }

    async fn due_lag(&self) -> Result<Duration, RepositoryError> {
        let row = self
            .db
            .query_one_raw(stmt(
                "SELECT COALESCE((EXTRACT(EPOCH FROM (now() - min(next_fire_at))) * 1000000)::bigint, 0) AS n \
                 FROM timer WHERE status = 'active' AND next_fire_at <= now()",
                vec![],
            ))
            .await
            .map_err(|e| RepositoryError::Store(e.to_string()))?
            .ok_or_else(|| RepositoryError::Store("the lag query returned no row".to_string()))?;
        let us: i64 = row.try_get("", "n").map_err(|e| RepositoryError::Store(e.to_string()))?;
        Ok(Duration::from_micros(u64::try_from(us).unwrap_or(0)))
    }
}

/// Count a transient error and build the pass error.
fn transient(message: &str) -> RepositoryError {
    metrics::counter!(paigasus_observability::names::AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL, "class" => "transient").increment(1);
    RepositoryError::Store(message.to_string())
}

fn plan_category(e: PlanError) -> Fail {
    Fail::permanent(match e {
        PlanError::Walk(WalkError::BoundExceeded { .. }) => "walk bound exceeded",
        PlanError::Walk(WalkError::Tz(_)) => "unknown time zone",
        PlanError::Walk(WalkError::Cron(_)) => "invalid cron",
    })
}

/// The `failure` text: a fixed category, at most 500 characters, no control character.
fn failure_text(category: &str) -> String {
    category.chars().filter(|c| !c.is_control()).take(500).collect()
}

fn round_up_to_second(t: Timestamp) -> i64 {
    t.as_second() + i64::from(t.subsec_nanosecond() > 0)
}

/// The id, kind, origin, attempts and org of a locked row, read as plain columns.
fn read_ctx(row: &QueryResult) -> Result<(Ctx, Uuid), DbErr> {
    let ctx = Ctx {
        id: row.try_get("", "id")?,
        kind: row.try_get("", "kind")?,
        origin: row.try_get("", "origin")?,
        attempts: row.try_get("", "fire_attempts")?,
    };
    Ok((ctx, row.try_get("", "org_id")?))
}

async fn db_now_in(txn: &DatabaseTransaction) -> Result<Timestamp, DbErr> {
    let row = txn
        .query_one_raw(stmt("SELECT (EXTRACT(EPOCH FROM now()) * 1000000)::bigint AS n", vec![]))
        .await?
        .ok_or_else(|| DbErr::Custom("SELECT now() returned no row".to_string()))?;
    let us: i64 = row.try_get("", "n")?;
    from_micros(us).map_err(|e| DbErr::Custom(e.0))
}

/// Write the state after a firing.
async fn update_fired(txn: &DatabaseTransaction, timer: &Timer, plan: &FirePlan) -> Result<(), DbErr> {
    let id: Value = timer.id().uuid().into();
    match plan.next {
        Next::Done => {
            txn.execute_raw(stmt(
                "UPDATE timer SET status = 'done', last_fired_at = now(), fire_count = fire_count + 1, fire_attempts = 0, updated_at = now() WHERE id = $1",
                vec![id],
            ))
            .await?;
        }
        Next::Advance { slot, at } => {
            txn.execute_raw(stmt(
                "UPDATE timer SET next_slot = $2::timestamp, next_slot_second = $3, next_fire_at = to_timestamp($4::bigint), \
                 last_fired_at = now(), fire_count = fire_count + 1, fire_attempts = 0, updated_at = now() WHERE id = $1",
                vec![id, civil_param(slot.civil).into(), slot.second.into(), at.as_second().into()],
            ))
            .await?;
        }
    }
    Ok(())
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_curve() {
        let base = Duration::from_secs(1);
        let secs: Vec<u64> = (1..=5).map(|n| backoff(base, n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16]);
        assert_eq!(backoff(base, 12).as_secs(), 2048);
        assert_eq!(backoff(base, 13).as_secs(), 3600, "capped at one hour");
        assert_eq!(backoff(base, u32::MAX).as_secs(), 3600, "no overflow");
        assert_eq!(backoff(Duration::MAX, u32::MAX), BACKOFF_CAP);
        assert_eq!(backoff(base, 0).as_secs(), 1, "no attempt counts as the first");
    }

    #[test]
    fn a_custom_db_error_is_transient() {
        assert_eq!(classify(&DbErr::Custom("x".to_string())), ErrorClass::Transient);
    }

    #[test]
    fn a_panic_is_a_permanent_failure_of_the_row() {
        let out: Result<(), Fail> = guarded(|| panic!("boom SECRET-NOTE"));
        assert!(matches!(out, Err(Fail::Permanent { ref category }) if category == "panic in plan_fire"));
        assert!(guarded(|| Ok(1)).is_ok());
    }

    #[test]
    fn the_failure_text_has_no_control_character_and_is_short() {
        let text = failure_text(&format!("a\nb\u{7}{}", "x".repeat(900)));
        assert_eq!(text.chars().count(), 500);
        assert!(!text.chars().any(char::is_control));
    }

    #[test]
    fn a_time_is_rounded_up_to_the_second() {
        assert_eq!(round_up_to_second(Timestamp::from_microsecond(1_500_000).expect("ts")), 2);
        assert_eq!(round_up_to_second(Timestamp::from_microsecond(2_000_000).expect("ts")), 2);
    }
}
