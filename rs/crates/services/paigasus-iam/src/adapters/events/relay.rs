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
use metrics::{counter, gauge, histogram};
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

/// Byte bound on a stored `last_error` (SMA-469). Deliberately a BYTE bound, not a char count:
/// 1024 four-byte chars would be 4KB, past Postgres's ~2KB TOAST threshold, so a pathological
/// publisher error string could bloat the row it is meant to describe.
const MAX_ERROR_BYTES: usize = 1024;

/// Bounds `s` to [`MAX_ERROR_BYTES`], cutting on a char boundary and marking the elision.
fn truncate_error(s: &str) -> String {
    if s.len() <= MAX_ERROR_BYTES {
        return s.to_string();
    }
    let end = s.char_indices().map(|(i, _)| i).take_while(|i| *i <= MAX_ERROR_BYTES).last().unwrap_or(0);
    format!("{}…", &s[..end])
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

/// Which rows a tick may drain (SMA-489 D13).
///
/// The distinction exists to keep retry cadence pinned to `poll_interval_secs`. `tick`
/// increments `attempts` once per tick for every row it locks, and nothing throttles how often
/// a *nudged* tick happens — so if nudged ticks drained everything, a failing row would burn
/// its retry budget at the COMMIT rate. At 2 mutations/s a row would reach the default
/// `max_attempts = 60` in ~30 s instead of ~5 min, dead-lettering the in-flight backlog on a
/// routine broker restart, and voiding `IamConfig::validate`'s
/// `duplicate_window_secs > max_attempts × poll_interval_secs` dedup floor while leaving that
/// check passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickMode {
    /// Every unpublished, unparked row — the poll tick's mode, and the pre-SMA-489 behaviour.
    All,
    /// Only never-attempted rows (`attempts = 0`) — every nudge- and backlog-driven tick.
    ///
    /// A row that has failed once is invisible to nudged ticks and is retried only by the poll.
    /// Side benefit: fresh events are no longer head-of-line blocked behind a poison row on the
    /// nudge path.
    Fresh,
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
    wake_debounce: Duration,
}

impl OutboxRelay {
    #[must_use]
    pub fn new(db: DatabaseConnection, poll_interval: Duration, batch_size: u64, max_attempts: i32) -> Self {
        OutboxRelay {
            db,
            poll_interval,
            batch_size,
            max_attempts,
            wake_debounce: Duration::from_millis(200),
        }
    }

    /// Overrides the D14 nudge debounce (default 200 ms). Builder rather than a `new` parameter
    /// so the existing four-argument call sites across the test suite stay untouched.
    #[must_use]
    pub fn with_wake_debounce(mut self, d: Duration) -> Self {
        self.wake_debounce = d;
        self
    }

    /// Runs exactly one drain tick over EVERY eligible row and returns its [`TickReport`].
    /// Equivalent to `tick_with(publisher, TickMode::All)`; kept as-is so existing callers and
    /// tests are unaffected.
    pub async fn tick(&self, publisher: &dyn EventPublisher) -> Result<TickReport, DbErr> {
        self.tick_with(publisher, TickMode::All).await
    }

    /// [`Self::tick`], restricted to `mode`'s row set (SMA-489 D13).
    pub async fn tick_with(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr> {
        let txn = self.db.begin().await?;

        let mut query = event_outbox::Entity::find()
            .filter(event_outbox::Column::PublishedAt.is_null())
            .filter(event_outbox::Column::Parked.eq(false));
        if mode == TickMode::Fresh {
            query = query.filter(event_outbox::Column::Attempts.eq(0));
        }
        let rows = query
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
                    let published_at = Utc::now();
                    // SMA-489: the only end-to-end proof the nudge works in production.
                    // `oldest_unpublished_age_seconds` cannot serve — it is reset to 0 on every
                    // empty tick, and the nudge makes empty ticks common.
                    histogram!(names::IAM_OUTBOX_PUBLISH_LAG_SECONDS).record(published_at.signed_duration_since(row.occurred_at).num_milliseconds() as f64 / 1000.0);
                    active.published_at = Set(Some(published_at));
                }
                Err(reason) => {
                    report.failures += 1;
                    let attempts = row.attempts + 1;
                    active.attempts = Set(attempts);
                    // SMA-469: recorded on EVERY failed attempt, not only at parking — an
                    // operator watching `attempts` climb wants the current reason, and the
                    // dead-letter surface reads this column.
                    active.last_error = Set(Some(truncate_error(&reason)));
                    if attempts >= self.max_attempts {
                        active.parked = Set(true);
                        // `[outbox.retention].parked_days` measures from HERE, never from
                        // `occurred_at` (m0009's module doc).
                        active.parked_at = Set(Some(Utc::now()));
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

    /// Runs one [`Self::tick_with`] and records its outcome on the `ticks_total{result}`
    /// run-loop counter. Returns `tick_with`'s own `Result` so [`Self::run`]'s backlog
    /// continuation (SMA-489 D9) can read `drained`/`failures` and so an `Err` ends a
    /// continuation run instead of hot-looping a broken database.
    pub async fn tick_and_record(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr> {
        match self.tick_with(publisher, mode).await {
            Ok(report) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "ok").increment(1);
                Ok(report)
            }
            Err(err) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "error").increment(1);
                tracing::warn!(error = %err, "outbox relay tick failed; retrying next interval");
                Err(err)
            }
        }
    }

    /// Runs the relay loop until `shutdown` resolves.
    ///
    /// Three things can start a tick: the `poll_interval` timer (draining every eligible row,
    /// `TickMode::All`), a `wake` permit from the `PgOutboxListener` (SMA-489), or SMA-489 D9's
    /// backlog continuation. The latter two use `TickMode::Fresh` so retry cadence stays pinned
    /// to `poll_interval` (D13).
    ///
    /// **Shutdown is checked BETWEEN ticks, never raced AROUND one.** Racing `shutdown` against
    /// the tick itself would cancel it mid-flight, rolling back a transaction whose events the
    /// publisher may already have accepted — SMA-471 D3's unbounded-republish gap, on every
    /// graceful shutdown.
    ///
    /// SOUNDNESS: `S: Future` is not `FusedFuture`, and polling a completed future is a contract
    /// violation. This shape is sound only because EVERY path that observes `shutdown` ready
    /// breaks the loop immediately. Preserve that if you restructure, or switch to a
    /// `CancellationToken`/`watch::Receiver`, which are poll-after-ready safe.
    pub async fn run<S>(self, publisher: Arc<dyn EventPublisher>, wake: Arc<tokio::sync::Notify>, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);

        // SMA-489 D12: prime every label value at zero. A metrics-rs series first appears
        // already at 1, so an `increase()` rule could never fire on a label's first occurrence.
        for source in ["notify", "poll", "backlog"] {
            counter!(names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL, "source" => source).increment(0);
        }

        'outer: loop {
            // `biased` so a ready shutdown always beats a ready notify permit. Without it the
            // choice is random, an extra tick can run after shutdown, and the tests that assert
            // otherwise become flaky. It costs nothing: the tick is not inside this select.
            let mut source = tokio::select! {
                biased;
                () = &mut shutdown => break 'outer,
                () = wake.notified() => "notify",
                () = tokio::time::sleep(self.poll_interval) => "poll",
            };
            let mut mode = if source == "poll" { TickMode::All } else { TickMode::Fresh };

            loop {
                counter!(names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL, "source" => source).increment(1);
                let Ok(report) = self.tick_and_record(publisher.as_ref(), mode).await else {
                    break; // already logged and counted; never hot-loop a broken database
                };
                // D9: continue only on a FULL batch that made progress. `drained > failures`
                // rather than `failures == 0` so one poison row cannot disable the continuation.
                if report.drained < self.batch_size || report.drained <= report.failures {
                    break;
                }
                // Poll shutdown WITHOUT cancelling anything, then keep draining.
                let stopping = std::future::poll_fn(|cx| std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_ready())).await;
                if stopping {
                    break 'outer;
                }
                source = "backlog";
                mode = TickMode::Fresh;
            }

            // D14: floor the nudge-driven tick rate. The poll arm is already bounded.
            if source != "poll" {
                let jitter = 0.75 + rand::random::<f64>() * 0.5;
                let delay = self.wake_debounce.mul_f64(jitter);
                tokio::select! {
                    biased;
                    () = &mut shutdown => break 'outer,
                    () = tokio::time::sleep(delay) => {}
                }
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

    #[test]
    fn truncate_error_leaves_a_short_string_untouched() {
        assert_eq!(truncate_error("boom"), "boom");
    }

    #[test]
    fn truncate_error_leaves_a_string_exactly_at_the_bound_untouched() {
        // Exactly MAX_ERROR_BYTES bytes (1-byte-per-char ASCII): the `s.len() <= MAX_ERROR_BYTES`
        // fast path must return it verbatim, with no elision marker appended.
        let exact = "a".repeat(MAX_ERROR_BYTES);
        let out = truncate_error(&exact);
        assert_eq!(out, exact, "a string exactly at the bound must be returned unchanged");
        assert!(!out.ends_with('…'), "must not append an elision marker when nothing was cut");
    }

    #[test]
    fn truncate_error_bounds_a_long_string_of_four_byte_chars_without_panicking() {
        // 700 four-byte chars = 2800 bytes, comfortably over the 1024-byte bound and past
        // Postgres's ~2KB TOAST threshold — the reason the bound is in BYTES, not chars.
        // NOTE: this does NOT discriminate the correct implementation from a naive
        // `&s[..MAX_ERROR_BYTES]` slice — 4 evenly divides 1024, so byte offset 1024 is always a
        // valid boundary for this data either way. Kept as a smoke test; the char-boundary
        // guard itself is exercised by `truncate_error_cuts_before_a_split_multibyte_char_not_through_it`.
        let long = "😀".repeat(700);
        let out = truncate_error(&long);
        assert!(out.len() <= MAX_ERROR_BYTES + '…'.len_utf8(), "not bounded: {} bytes", out.len());
        assert!(out.ends_with('…'), "expected an elision marker");
        assert!(out.trim_end_matches('…').chars().all(|c| c == '😀'));
    }

    #[test]
    fn truncate_error_cuts_before_a_split_multibyte_char_not_through_it() {
        // '€' is 3 bytes, and 1024 mod 3 == 1, so byte offset MAX_ERROR_BYTES (1024) lands ONE
        // BYTE INTO the 342nd '€' (its 3-byte span is [1023, 1026)) rather than on a boundary.
        // A naive `&s[..MAX_ERROR_BYTES]` slice would panic here; the correct implementation
        // must instead cut at 1023 — the last char boundary <= MAX_ERROR_BYTES — giving up one
        // trailing byte rather than splitting a character. This is the discriminating case the
        // 4-byte-char test above cannot provide.
        let long = "€".repeat(700); // 2100 bytes, well past MAX_ERROR_BYTES
        let out = truncate_error(&long);
        assert!(out.len() <= MAX_ERROR_BYTES + '…'.len_utf8(), "not bounded: {} bytes", out.len());
        assert!(out.ends_with('…'), "expected an elision marker");
        let prefix = out.trim_end_matches('…');
        assert!(prefix.chars().all(|c| c == '€'), "prefix must be whole '€' chars, got: {prefix:?}");
        assert_eq!(prefix.len(), 1023, "must give up the trailing partial byte rather than split a character");
    }

    /// The D9 continuation predicate, isolated. The mixed case is what discriminates
    /// `drained > failures` from the naive `failures == 0`: a single poison row sits at a fixed
    /// FIFO position and reappears in every batch until it parks 60 attempts later, so
    /// `failures == 0` would leave the continuation dead exactly when a deep backlog needs it.
    #[test]
    fn continuation_predicate_requires_a_full_batch_that_made_progress() {
        let batch = 100u64;
        let should_continue = |drained: u64, failures: u64| drained == batch && drained > failures;

        assert!(should_continue(100, 0), "full batch, all published");
        assert!(should_continue(100, 99), "full batch, one row published — still progress");
        assert!(!should_continue(100, 100), "full batch, nothing published — would hot-loop");
        assert!(!should_continue(99, 0), "partial batch — queue is drained");
    }
}
