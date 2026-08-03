// SPDX-License-Identifier: Apache-2.0

//! `OutboxRelay`: the background drain that turns committed `event_outbox` rows into calls on
//! an injected [`EventPublisher`] (SMA-446, Slice B Task B8).
//!
//! **Multi-replica safety is the point.** Each tick runs entirely on one transaction:
//! 1. `SELECT * FROM event_outbox WHERE published_at IS NULL AND parked = false ORDER BY id
//!    FOR UPDATE SKIP LOCKED LIMIT batch_size` — locks the selected rows for the duration of the
//!    transaction, and `SKIP LOCKED` means a row already locked by ANOTHER relay replica's
//!    concurrent tick is silently excluded from this one's result set rather than blocking on
//!    it. Two replicas' `run` loops therefore never grab the same row.
//! 2. Each locked row is handed to `publisher.publish`; the outcome is written back on the SAME
//!    transaction (`published_at` on success, `attempts += 1` and — once `attempts` reaches
//!    `max_attempts` — `parked = true` on failure).
//! 3. `commit` releases the row locks.
//!
//! Verified against the vendored `sea-orm 1.1.20` (via `sea-query 0.32.7`, its lock-clause
//! backend): `QuerySelect::lock_with_behavior(LockType, LockBehavior)` and
//! `sea_orm::sea_query::{LockType::Update, LockBehavior::SkipLocked}` exist and map directly to
//! `FOR UPDATE SKIP LOCKED` — no raw-`Statement` fallback needed.
//!
//! [`OutboxRelay::run`] mirrors `PolicySnapshot::spawn_reload`'s shutdown-watch pattern
//! (`tokio::select!` racing a poll-interval sleep against a caller-supplied shutdown future) —
//! unlike `spawn_reload`, `run` does not `tokio::spawn` itself; the composition root (a later
//! task, B9) spawns it exactly like it already spawns the denial-audit drain in `main.rs`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use metrics::{counter, gauge};
#[cfg(test)]
use paigasus_iam_core::PublishError;
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use paigasus_observability::names;
use sea_orm::sea_query::{LockBehavior, LockType};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait};

use crate::adapters::persistence::entities::event_outbox;

/// Per-tick telemetry, both logged (`tracing::info!`) and returned so callers/tests can assert
/// on it directly instead of scraping log output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// Rows locked and processed this tick (published + failed, including newly parked).
    pub drained: u64,
    /// Of `drained`, how many failed to publish (a subset that includes `parked`).
    pub failures: u64,
    /// Of `failures`, how many hit `max_attempts` and were parked this tick.
    pub parked: u64,
    /// Age (seconds) of the oldest row in this tick's batch, if the batch was non-empty — a
    /// cheap staleness signal (no extra query: derived from the already-fetched rows).
    pub oldest_unpublished_age_secs: Option<i64>,
}

/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`.
///
/// `PublishError::Backend`'s `Display` is the static string `"backend error"` — thiserror's
/// `#[from]` makes the boxed cause the variant's `source()` rather than part of its message
/// (`paigasus_iam_core::ports`), so `to_string()` alone tells an operator nothing about WHY a
/// publish failed. Since the parked row's `last_error` (SMA-469) and the `error!`/`warn!` lines
/// below all render this string, the chain walk is what makes any of them informative.
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// Reconstructs a [`DomainEvent`] from a persisted `event_outbox` row for handing to
/// [`EventPublisher::publish`]. Returns `Err` (a human-readable reason) for a malformed row — an
/// unrecognized `event_type` wire string, an out-of-range `schema_version`, or invalid `payload`
/// JSON — which should never happen for a row this relay's own writer (`PgOutbox::enqueue`, B2)
/// ever produced, but a defensive check here costs nothing next to a `panic!`/`unwrap()` that
/// would wedge the relay on a single corrupted row. The relay treats this exactly like a failed
/// `publish` call (counts against `attempts`, eventually parks).
fn row_to_domain_event(row: &event_outbox::Model) -> Result<DomainEvent, String> {
    let event_type = EventType::parse(&row.event_type).ok_or_else(|| format!("unrecognized event_type wire string {:?}", row.event_type))?;
    let schema_version = u16::try_from(row.schema_version).map_err(|_| format!("schema_version {} out of u16 range", row.schema_version))?;
    let payload = serde_json::from_str(&row.payload).map_err(|e| format!("invalid payload json: {e}"))?;
    Ok(DomainEvent {
        id: row.id,
        event_type,
        schema_version,
        aggregate_prn: row.aggregate_prn.clone(),
        actor_prn: row.actor_prn.clone(),
        occurred_at: row.occurred_at,
        payload,
        correlation_id: row.correlation_id,
    })
}

/// The transactional-outbox relay: polls `event_outbox` for unpublished, unparked rows and
/// drains them through an injected [`EventPublisher`]. `Clone`: `DatabaseConnection` is an
/// `Arc`-backed pool handle (mirrors every other adapter in this crate), so the composition
/// root can hold a relay handle inside a `#[derive(Clone)]` service if it ever needs to.
#[derive(Clone)]
pub struct OutboxRelay {
    db: DatabaseConnection,
    poll_interval: Duration,
    batch_size: u64,
    max_attempts: i32,
}

impl OutboxRelay {
    #[must_use]
    pub fn new(db: DatabaseConnection, poll_interval: Duration, batch_size: u64, max_attempts: i32) -> Self {
        OutboxRelay {
            db,
            poll_interval,
            batch_size,
            max_attempts,
        }
    }

    /// Runs exactly one drain tick (the transactional-outbox pattern described in the module
    /// docs) and returns its [`TickReport`]. Public (not just used internally by [`Self::run`])
    /// so tests can drive individual, deterministic ticks rather than racing the poll loop.
    pub async fn tick(&self, publisher: &dyn EventPublisher) -> Result<TickReport, DbErr> {
        let txn = self.db.begin().await?;

        let rows = event_outbox::Entity::find()
            .filter(event_outbox::Column::PublishedAt.is_null())
            .filter(event_outbox::Column::Parked.eq(false))
            .order_by_asc(event_outbox::Column::Id)
            .limit(self.batch_size)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .all(&txn)
            .await?;

        let mut report = TickReport {
            drained: rows.len() as u64,
            oldest_unpublished_age_secs: rows.first().map(|r| Utc::now().signed_duration_since(r.occurred_at).num_seconds()),
            ..TickReport::default()
        };

        for row in rows {
            let outcome = match row_to_domain_event(&row) {
                Ok(ev) => publisher.publish(&ev).await.map_err(|e| describe_error(&e)),
                Err(reason) => Err(reason),
            };

            let mut active = row.clone().into_active_model();
            match outcome {
                Ok(()) => {
                    active.published_at = Set(Some(Utc::now()));
                }
                Err(reason) => {
                    report.failures += 1;
                    let attempts = row.attempts + 1;
                    active.attempts = Set(attempts);
                    if attempts >= self.max_attempts {
                        active.parked = Set(true);
                        report.parked += 1;
                        tracing::error!(id = %row.id, event_type = %row.event_type, attempts, reason = %reason, "outbox event parked after max attempts (poison)");
                    } else {
                        tracing::warn!(id = %row.id, event_type = %row.event_type, attempts, reason = %reason, "outbox event publish failed; will retry");
                    }
                }
            }
            active.update(&txn).await?;
        }

        txn.commit().await?;

        tracing::info!(
            drained = report.drained,
            failures = report.failures,
            parked = report.parked,
            oldest_unpublished_age_secs = report.oldest_unpublished_age_secs,
            "outbox relay tick"
        );

        // SMA-446 Unit 5 (Task A11): per-tick relay counters/gauge, alongside the log line
        // above. `parked_total` is a COUNTER of newly-parked-this-tick rows (not a gauge —
        // the currently-parked-row-count is a derivable Prometheus query, `sum(increase(...))`,
        // not a separate series here).
        counter!(names::IAM_OUTBOX_RELAY_DRAINED_TOTAL).increment(report.drained);
        counter!(names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL).increment(report.failures);
        counter!(names::IAM_OUTBOX_RELAY_PUBLISHED_TOTAL).increment(report.drained.saturating_sub(report.failures));
        counter!(names::IAM_OUTBOX_RELAY_PARKED_TOTAL).increment(report.parked);
        gauge!(names::IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS).set(report.oldest_unpublished_age_secs.unwrap_or(0) as f64);

        Ok(report)
    }

    /// Runs one drain [`Self::tick`] and records its outcome on the `ticks_total{result}`
    /// run-loop counter (`result="ok"` on success; `result="error"` plus a `tracing::warn!`
    /// on a DB-level tick error). This is the exact body [`Self::run`] executes per poll
    /// interval, factored out so `run`'s only remaining logic is the `select!` shutdown loop.
    /// Intended for `run` and tests only — production callers should use [`Self::run`]; it is
    /// `pub` for the same reason [`Self::tick`] is: to let tests assert the ok/error tick
    /// counters deterministically without racing the poll loop (SMA-465).
    pub async fn tick_and_record(&self, publisher: &dyn EventPublisher) {
        match self.tick(publisher).await {
            Ok(_) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "ok").increment(1);
            }
            Err(err) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "error").increment(1);
                tracing::warn!(error = %err, "outbox relay tick failed; retrying next interval");
            }
        }
    }

    /// Runs the relay loop until `shutdown` resolves: sleep `poll_interval`, tick, repeat —
    /// mirrors `PolicySnapshot::spawn_reload`'s `tokio::select!` shutdown-watch exactly (sleep
    /// races shutdown first, so the very first tick runs after one poll interval, not
    /// immediately). A tick-level error (e.g. a dropped connection) is logged and the loop keeps
    /// going; per-row publish failures never reach here — [`Self::tick`] already turns those
    /// into `attempts`/`parked` bookkeeping on the same transaction.
    pub async fn run<S>(self, publisher: Arc<dyn EventPublisher>, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {
                    self.tick_and_record(publisher.as_ref()).await;
                }
                () = &mut shutdown => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `row_to_domain_event` unit tests (SMA-446 Slice B Task B8, review finding, Fix 3): the
    //! three malformed-row error branches its own doc comment enumerates — an unrecognized
    //! `event_type` wire string, invalid `payload` JSON, and an out-of-range `schema_version`
    //! — each return `Err`, plus a well-formed row maps to the exact `DomainEvent` its fields
    //! describe. No DB needed: `event_outbox::Model` is hand-built, never persisted.
    use super::*;
    use uuid::Uuid;

    /// A well-formed `event_outbox::Model` — every malformed-row test below starts from this
    /// and corrupts exactly the one field its case is about.
    fn base_model() -> event_outbox::Model {
        event_outbox::Model {
            id: Uuid::from_u128(1),
            occurred_at: Utc::now(),
            event_type: EventType::PrincipalCreated.as_wire().to_string(),
            schema_version: 1,
            aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
            actor_prn: Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb".to_string()),
            payload: serde_json::json!({"kind": "user"}).to_string(),
            correlation_id: Some(Uuid::from_u128(2)),
            published_at: None,
            attempts: 0,
            parked: false,
            parked_at: None,
            last_error: None,
        }
    }

    #[test]
    fn rejects_unrecognized_event_type() {
        let mut row = base_model();
        row.event_type = "iam.nope.happened".to_string();
        let err = row_to_domain_event(&row).unwrap_err();
        assert!(err.contains("unrecognized event_type"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_invalid_payload_json() {
        let mut row = base_model();
        row.payload = "{not valid json".to_string();
        let err = row_to_domain_event(&row).unwrap_err();
        assert!(err.contains("invalid payload json"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_out_of_range_schema_version() {
        let mut negative = base_model();
        negative.schema_version = -1;
        let err = row_to_domain_event(&negative).unwrap_err();
        assert!(err.contains("schema_version"), "unexpected error: {err}");

        let mut too_large = base_model();
        too_large.schema_version = i32::from(u16::MAX) + 1;
        let err = row_to_domain_event(&too_large).unwrap_err();
        assert!(err.contains("schema_version"), "unexpected error: {err}");
    }

    #[test]
    fn maps_a_well_formed_row_to_the_matching_domain_event() {
        let row = base_model();
        let ev = row_to_domain_event(&row).unwrap();
        assert_eq!(ev.id, row.id);
        assert_eq!(ev.event_type, EventType::PrincipalCreated);
        assert_eq!(ev.schema_version, 1);
        assert_eq!(ev.aggregate_prn, row.aggregate_prn);
        assert_eq!(ev.actor_prn, row.actor_prn);
        assert_eq!(ev.occurred_at, row.occurred_at);
        assert_eq!(ev.payload, serde_json::json!({"kind": "user"}));
        assert_eq!(ev.correlation_id, row.correlation_id);
    }

    /// A publish failure must carry its whole `source()` chain into the reason string —
    /// `PublishError::Backend`'s own `Display` is the static "backend error" and renders
    /// nothing about what actually failed (`ports.rs`).
    #[test]
    fn describe_error_walks_the_full_source_chain_without_duplicating_levels() {
        #[derive(Debug, thiserror::Error)]
        #[error("transport closed")]
        struct Inner;

        #[derive(Debug, thiserror::Error)]
        #[error("publish failed")]
        struct Outer(#[source] Inner);

        let err = PublishError::from(Box::new(Outer(Inner)) as Box<dyn std::error::Error + Send + Sync>);
        assert_eq!(describe_error(&err), "backend error: publish failed: transport closed");
    }

    #[test]
    fn describe_error_of_a_sourceless_error_is_just_its_display() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope")]
        struct Bare;
        assert_eq!(describe_error(&Bare), "nope");
    }
}
