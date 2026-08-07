// SPDX-License-Identifier: Apache-2.0

//! `DeadLetterService` (SMA-469): the Root-only use case over parked `event_outbox` rows.
//!
//! Root-only-ness lives HERE, not in the Cedar schema — the shared `appliesTo` block does not
//! restrict the three actions, so this service enforces it by always authorizing at
//! `root_prn()`, exactly like `AuditQueryService::list` and `PolicyService::list`.
//!
//! `replay`/`replay_matching`/`discard` drive the mutation and its audit entry through ONE
//! `UnitOfWork` transaction (the `application::roles` reference pattern), so a mid-transaction
//! failure leaves neither. They deliberately do NOT enqueue a domain event: these are
//! operational actions on the queue itself, and an outbox event about outbox operations would
//! be circular.

use std::sync::Arc;

use chrono::Utc;
use metrics::counter;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, BulkReplayRequest, Clock, DeadLetterEntry, DeadLetterFilter, DeadLetters, IdGenerator, UnitOfWork};
use paigasus_kernel::Prn;
use paigasus_observability::names;
use uuid::Uuid;

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;

/// The `[outbox.publisher].duplicate_window_secs` default, mirrored here for the SMA-471 D4
/// `beyond_dedup_window` replay label (see [`DeadLetterService::replay`]'s doc and its call
/// site). This application service has no publisher config of its own, and threading it in
/// would couple the dead-letter surface to a broker it otherwise knows nothing about — so this
/// is a constant, not config.
///
/// **This label is an exposure ESTIMATE, not a verdict**, and it is wrong in both directions by
/// bounded amounts. Read it as "how many replays are plausibly past the window", never as proof
/// about any single event. Three separate approximations stack (the last two were raised by
/// CodeRabbit on PR 112):
///
/// 1. **The window may not be 3600s.** A deployment that widens `duplicate_window_secs` sees
///    over-warning (`"true"` for rows the real, wider window still deduped); one that narrows it
///    — legal, since `IamConfig::validate` only enforces a floor of
///    `max_attempts × poll_interval_secs` — sees under-warning.
/// 2. **`parked_at` is the wrong clock.** JetStream's window starts when it STORED the first
///    publish; `parked_at` is stamped only once the relay exhausts `max_attempts`, i.e. about
///    `max_attempts × poll_interval_secs` later (~5 min at defaults). So the elapsed time this
///    measures is systematically SHORTER than the real one, and the label under-reports `"true"`
///    by roughly that margin.
/// 3. **A parked row may never have been stored at all.** If every attempt failed before
///    reaching the server there is no duplicate to create, yet a long-parked row still labels
///    `"true"`. Harmless over-warning.
///
/// Making this exact needs a `first_attempt_at` column on `event_outbox` (a migration) plus the
/// effective `duplicate_window_secs` plumbed in — deliberately deferred, because nothing branches
/// on this label: it is observability only, and an approximate signal about a real risk beats no
/// signal about it. See SMA-471 §8.
const ASSUMED_DEDUP_WINDOW_SECS: u32 = 3_600;

/// Constructor bag, mirroring `RoleServiceDeps` — keeps `new` from growing a six-argument
/// positional signature.
pub struct DeadLetterDeps {
    pub dead: Arc<dyn DeadLetters>,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
    pub authorize: Authorize,
}

#[derive(Clone)]
pub struct DeadLetterService {
    dead: Arc<dyn DeadLetters>,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
    authorize: Authorize,
}

impl DeadLetterService {
    #[must_use]
    pub fn new(deps: DeadLetterDeps) -> Self {
        DeadLetterService {
            dead: deps.dead,
            uow: deps.uow,
            audit: deps.audit,
            ids: deps.ids,
            clock: deps.clock,
            authorize: deps.authorize,
        }
    }

    /// Builds the committed-outcome audit entry every mutating operation records.
    fn audit_entry(&self, actor: &Prn, action: Action, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: self.clock.now(),
            actor_prn: Some(actor.canonical()),
            action: action.as_wire().to_string(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: Vec::new(),
            detail,
            correlation_id: Some(self.ids.new_correlation_id()),
        }
    }

    pub async fn list(&self, actor: &Prn, filter: DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, TenancyError> {
        self.authorize.check(actor, Action::ListOutboxDeadLetters, &root_prn()).await?;
        Ok(self.dead.list(&filter).await?)
    }

    /// Un-parks one row BY ITS EXISTING ID — `REPLAY_ONE_SQL` never mints a fresh one — so this
    /// is an AT-LEAST-ONCE operation from the broker's point of view: the id is what
    /// `NatsEventPublisher` sends as `Nats-Msg-Id`, and JetStream only deduplicates within its
    /// `duplicate_window_secs` of the FIRST publish. A row parked longer ago than that window
    /// republishes an event the stream may already hold, and any consumer must be built to
    /// tolerate that rather than assume exactly-once delivery. Nothing here prevents a late
    /// replay — minting a fresh id would trade a detectable duplicate for an undetectable one,
    /// and refusing the replay would remove the operator's only recovery tool — so the exposure
    /// is measured instead, via `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL`'s `beyond_dedup_window`
    /// label (SMA-471 D4, see the call site).
    pub async fn replay(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError> {
        self.authorize.check(actor, Action::ReplayOutboxDeadLetter, &root_prn()).await?;
        let tx = self.uow.begin().await?;
        let Some(entry) = self.dead.replay_in(&*tx, id).await? else {
            // Dropping `tx` without committing rolls it back. `None` covers an absent id, a
            // live row, and a row another actor just replayed or discarded — all 404 to the
            // caller (documented in the runbook so an operator does not chase a phantom).
            return Err(TenancyError::NotFound);
        };
        let detail = serde_json::json!({
            "event_id": entry.id.to_string(),
            "event_type": entry.event_type,
            "aggregate_prn": entry.aggregate_prn,
            "attempts": entry.attempts,
            "last_error": entry.last_error,
        });
        let audit = self.audit_entry(actor, Action::ReplayOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        // Counted AFTER the commit, so a rolled-back replay is never counted. (This differs
        // from `PgAuditLog`'s counter, which deliberately fires at insert — see its doc.)
        //
        // SMA-471 D4: replay keeps the row id, so a row parked longer ago than JetStream's
        // duplicate window republishes an event the stream may already hold. Nothing prevents
        // that — refusing the replay would remove the operator's only recovery tool — so it is
        // measured instead.
        //
        // Compared against a CONSTANT rather than `outbox.publisher.duplicate_window_secs`, and
        // measured from `parked_at` rather than from the first publish attempt. Both are
        // approximations with bounded, two-directional error — see
        // `ASSUMED_DEDUP_WINDOW_SECS`'s doc for the full accounting and why it is deferred.
        // The label is observability only; nothing branches on it.
        let beyond_dedup_window = match entry.parked_at {
            Some(parked_at) => {
                if Utc::now().signed_duration_since(parked_at).num_seconds() > i64::from(ASSUMED_DEDUP_WINDOW_SECS) {
                    "true"
                } else {
                    "false"
                }
            }
            None => "unknown",
        };
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "one", "beyond_dedup_window" => beyond_dedup_window).increment(1);
        Ok(entry)
    }

    /// Bulk form of [`Self::replay`]: same at-least-once consequence for the broker on every
    /// row it un-parks — each keeps its existing id, so a row parked longer ago than
    /// JetStream's `duplicate_window_secs` republishes an event the stream may already hold —
    /// but `replay_matching_in` returns only a row COUNT, not the rows themselves, so
    /// `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL`'s `beyond_dedup_window` label is always
    /// `"unknown"` here rather than computed per row (see the call site).
    pub async fn replay_matching(&self, actor: &Prn, req: BulkReplayRequest) -> Result<u64, TenancyError> {
        self.authorize.check(actor, Action::ReplayOutboxDeadLetter, &root_prn()).await?;
        // Validated BEFORE any store access — the explicit row budget is the guard on blast
        // radius, so a request without one must never reach the database.
        if !req.is_valid() {
            return Err(TenancyError::InvalidBulkReplay);
        }
        let tx = self.uow.begin().await?;
        let replayed = self.dead.replay_matching_in(&*tx, &req).await?;
        // `max_rows` records the REQUESTED budget verbatim; `capped_max_rows` records the
        // effective ceiling actually enforced (`BulkReplayRequest::MAX_BULK_REPLAY`, Task 10) —
        // kept as a SEPARATE field, not a silent overwrite of `max_rows`, so a request for e.g.
        // 50_000 rows against the 10_000 cap audits unambiguously as "asked for 50000, allowed up
        // to 10000, replayed 3" rather than reading like a partial failure.
        let detail = serde_json::json!({
            "event_type": req.event_type,
            "parked_from": req.parked_from.map(|t| t.to_rfc3339()),
            "parked_to": req.parked_to.map(|t| t.to_rfc3339()),
            "max_rows": req.max_rows,
            "capped_max_rows": req.capped_max_rows(),
            "replayed": replayed,
        });
        let audit = self.audit_entry(actor, Action::ReplayOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        // Increments by ROWS, not calls — mixing units within one metric family would make
        // `rate()` meaningless.
        //
        // Bulk replay returns a COUNT, not rows, so per-row `parked_at` is unavailable and the
        // window question cannot be answered per event. `"unknown"` rather than guessing — and
        // rather than changing `replay_matching_in`'s signature, which is out of scope here.
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "bulk", "beyond_dedup_window" => "unknown").increment(replayed);
        Ok(replayed)
    }

    pub async fn discard(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError> {
        self.authorize.check(actor, Action::DiscardOutboxDeadLetter, &root_prn()).await?;
        let tx = self.uow.begin().await?;
        let Some(entry) = self.dead.discard_in(&*tx, id).await? else {
            return Err(TenancyError::NotFound);
        };
        // Deliberately LOSSLESS, payload included: a discarded dead letter is gone forever, so
        // this entry is its only remaining trace and the documented reconciliation input for
        // the downstream delivery that will now never happen.
        let detail = serde_json::json!({
            "event_id": entry.id.to_string(),
            "event_type": entry.event_type,
            "schema_version": entry.schema_version,
            "aggregate_prn": entry.aggregate_prn,
            "actor_prn": entry.actor_prn,
            "correlation_id": entry.correlation_id.map(|c| c.to_string()),
            "occurred_at": entry.occurred_at.to_rfc3339(),
            "attempts": entry.attempts,
            "last_error": entry.last_error,
            "payload": entry.payload,
        });
        let audit = self.audit_entry(actor, Action::DiscardOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL).increment(1);
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FailingDeadLetters, FakeAuditLog, FakeAuthorizer, FakeDeadLetters, FakeUnitOfWork, FixedClock, SeqIds};
    use chrono::{DateTime, Utc};
    use paigasus_iam_core::AuditOutcome;
    use std::future::Future;

    fn actor() -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
    }

    fn entry(id: u128) -> DeadLetterEntry {
        DeadLetterEntry {
            id: Uuid::from_u128(id),
            occurred_at: Utc::now(),
            event_type: "iam.principal.created".to_string(),
            schema_version: 1,
            aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
            actor_prn: None,
            payload: serde_json::json!({"kind": "user"}).to_string(),
            correlation_id: None,
            attempts: 5,
            parked_at: Some(Utc::now()),
            last_error: Some("backend error: transport closed".to_string()),
        }
    }

    /// Like [`entry`], but with a caller-chosen `parked_at` — the SMA-471 D4 label tests need to
    /// control how long ago a row parked, which the fixed `Utc::now()` in [`entry`] cannot do.
    fn entry_parked_at(id: u128, parked_at: DateTime<Utc>) -> DeadLetterEntry {
        DeadLetterEntry {
            parked_at: Some(parked_at),
            ..entry(id)
        }
    }

    fn filter() -> DeadLetterFilter {
        DeadLetterFilter {
            event_type: None,
            parked_from: None,
            parked_to: None,
            cursor: None,
            limit: 50,
        }
    }

    fn bulk(max_rows: u64) -> BulkReplayRequest {
        BulkReplayRequest {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows,
        }
    }

    struct Fixture {
        svc: DeadLetterService,
        audit: FakeAuditLog,
        dead: FakeDeadLetters,
        uow: FakeUnitOfWork,
    }

    fn fixture(allow: &[Action]) -> Fixture {
        let authz = FakeAuthorizer::default();
        for a in allow {
            authz.allow(*a, &root_prn());
        }
        let dead = FakeDeadLetters::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let svc = DeadLetterService::new(DeadLetterDeps {
            dead: Arc::new(dead.clone()),
            uow: Arc::new(uow.clone()),
            audit: Arc::new(audit.clone()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(authz)),
        });
        Fixture { svc, audit, dead, uow }
    }

    type MetricsSnapshot = Vec<(metrics_util::CompositeKey, Option<metrics::Unit>, Option<metrics::SharedString>, metrics_util::debugging::DebugValue)>;

    /// Drives `fut` to completion under a `metrics_util::debugging::DebuggingRecorder`, exactly
    /// as `redis_conn.rs`'s `breaker_transitions_emit_the_gauge_and_the_counter` proves a
    /// metric is actually EMITTED rather than merely computed. `DeadLetterService`'s methods are
    /// async, so `fut` is polled to completion on a dedicated current-thread `tokio` runtime
    /// INSIDE the closure `with_local_recorder` runs — `with_local_recorder` only installs the
    /// recorder thread-locally for that closure's OWN thread, so polling `fut` on any OTHER
    /// thread (a multi-thread runtime's worker pool, `#[tokio::test]`'s own driver thread, etc.)
    /// would silently miss every metric it emits. Plain `#[test]`, not `#[tokio::test]`, is what
    /// keeps this thread the only one in play.
    fn with_debug_recorder(fut: impl Future<Output = ()>) -> MetricsSnapshot {
        use metrics_util::debugging::DebuggingRecorder;

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("building a current-thread runtime cannot fail here");
        metrics::with_local_recorder(&recorder, || rt.block_on(fut));
        snapshotter.snapshot().into_vec()
    }

    /// Finds `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL`'s emitted `beyond_dedup_window` label
    /// value in a snapshot from [`with_debug_recorder`]. Reads the label the metric was ACTUALLY
    /// emitted with, not a value recomputed by the test — so a test using this proves the
    /// instrumentation, not just the branch condition.
    fn beyond_dedup_window_label(snapshot: &MetricsSnapshot) -> String {
        let found = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL)
            .expect("SMA-471 D4: iam_outbox_dead_letters_replayed_total was never emitted");
        found
            .0
            .key()
            .labels()
            .find(|l| l.key() == "beyond_dedup_window")
            .expect("SMA-471 D4: beyond_dedup_window label missing from the emitted counter")
            .value()
            .to_string()
    }

    #[tokio::test]
    async fn every_operation_denies_an_unauthorized_actor() {
        let f = fixture(&[]);
        f.dead.seed(entry(1));
        assert!(matches!(f.svc.list(&actor(), filter()).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.replay(&actor(), Uuid::from_u128(1)).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.discard(&actor(), Uuid::from_u128(1)).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.replay_matching(&actor(), bulk(10)).await, Err(TenancyError::Forbidden)));
        assert_eq!(f.audit.0.lock().unwrap().len(), 0, "a denied call must never write an audit entry");
    }

    /// SMA-471 D4: replaying a row parked longer ago than the dedup window republishes an event
    /// JetStream may already hold. The label makes that exposure measurable instead of
    /// theoretical.
    #[test]
    fn replaying_a_long_parked_row_is_labelled_beyond_the_dedup_window() {
        let snapshot = with_debug_recorder(async {
            let f = fixture(&[Action::ReplayOutboxDeadLetter]);
            // Well past the 3_600s (1h) `ASSUMED_DEDUP_WINDOW_SECS` default.
            f.dead.seed(entry_parked_at(1, Utc::now() - chrono::Duration::hours(3)));
            f.svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap();
        });
        assert_eq!(beyond_dedup_window_label(&snapshot), "true");
    }

    /// The other half of the comparison: a row parked well inside the window must NOT be
    /// flagged. A test that only covers the long-parked branch above cannot tell "beyond the
    /// window" from "always true".
    #[test]
    fn replaying_a_freshly_parked_row_is_labelled_within_the_window() {
        let snapshot = with_debug_recorder(async {
            let f = fixture(&[Action::ReplayOutboxDeadLetter]);
            f.dead.seed(entry_parked_at(1, Utc::now() - chrono::Duration::seconds(30)));
            f.svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap();
        });
        assert_eq!(beyond_dedup_window_label(&snapshot), "false");
    }

    /// Pins the comparison's DIRECTION at `dead_letters.rs`'s `beyond_dedup_window` computation:
    /// it is strict `>` against `ASSUMED_DEDUP_WINDOW_SECS`, not `>=`. The two tests above (3h
    /// and 30s) are nowhere near the 3_600s threshold, so flipping `>` to `>=` would pass both
    /// unchanged — neither discriminates the operator. This test targets the boundary itself: a
    /// row parked EXACTLY at the threshold must still read `"false"` (strict `>` requires aging
    /// PAST it, not just reaching it), while a row one second further must read `"true"`.
    ///
    /// Deliberately NOT constructed as `Utc::now() - Duration::seconds(3_600)`: `replay`'s own
    /// `Utc::now()` call happens a small amount of wall-clock time AFTER this line runs (test
    /// setup, `with_debug_recorder`'s runtime construction, `fixture()`, the async dispatch),
    /// so the elapsed time it actually measures is `3_600s + that gap`. `num_seconds()` truncates
    /// towards zero, so if the gap is small the count still reads `3_600` — but "small" is not
    /// "zero", and a slow CI runner could push the gap over a full second, silently reading
    /// `3_601` and making the `"false"` assertion flaky in exactly the direction that would
    /// falsely BLAME a correct implementation.
    ///
    /// Instead each target is offset 500ms INTO its intended integer second (`3_600_500ms` /
    /// `3_601_500ms` ago): `num_seconds()` still truncates to `3_600`/`3_601` respectively as
    /// long as the setup-to-comparison gap stays under ~500ms, which a synchronous, no-I/O,
    /// in-memory test (no real network/disk/broker anywhere on this path) is never remotely
    /// close to — the 500ms margin is roughly three orders of magnitude more slack than this
    /// test's actual overhead needs.
    #[test]
    fn replaying_pins_the_strict_greater_than_at_the_window_boundary() {
        let window_ms = i64::from(ASSUMED_DEDUP_WINDOW_SECS) * 1_000;

        let at_boundary = with_debug_recorder(async {
            let f = fixture(&[Action::ReplayOutboxDeadLetter]);
            f.dead.seed(entry_parked_at(1, Utc::now() - chrono::Duration::milliseconds(window_ms + 500)));
            f.svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap();
        });
        assert_eq!(
            beyond_dedup_window_label(&at_boundary),
            "false",
            "a row parked EXACTLY at the window boundary must not be flagged — the comparison is strict `>`, not `>=`"
        );

        let one_second_past = with_debug_recorder(async {
            let f = fixture(&[Action::ReplayOutboxDeadLetter]);
            f.dead.seed(entry_parked_at(2, Utc::now() - chrono::Duration::milliseconds(window_ms + 1_500)));
            f.svc.replay(&actor(), Uuid::from_u128(2)).await.unwrap();
        });
        assert_eq!(beyond_dedup_window_label(&one_second_past), "true", "one second past the window boundary must flip to true");
    }

    /// `replay_matching_in` returns only a row COUNT, never the rows, so `replay_matching` has
    /// no `parked_at` to compare against the window — this pins that the label degrades to
    /// `"unknown"` rather than silently defaulting to `"false"` (which would UNDER-warn) or
    /// guessing.
    #[test]
    fn bulk_replay_is_labelled_unknown_because_per_row_parked_at_is_unavailable() {
        let snapshot = with_debug_recorder(async {
            let f = fixture(&[Action::ReplayOutboxDeadLetter]);
            f.dead.seed(entry(1));
            f.svc.replay_matching(&actor(), bulk(10)).await.unwrap();
        });
        assert_eq!(beyond_dedup_window_label(&snapshot), "unknown");
    }

    #[tokio::test]
    async fn replay_records_exactly_one_audit_entry_naming_the_event() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        let replayed = f.svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap();
        assert_eq!(replayed.id, Uuid::from_u128(1));

        // Exactly one commit — a service that forgot `tx.commit().await?` would still return
        // `Ok` here (every other fake mutates regardless of commit), so this is the guard that
        // actually catches a silently-dropped, never-committed transaction.
        assert_eq!(f.uow.commits(), 1, "replay must commit exactly once");

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "replay must record exactly one audit entry");
        assert_eq!(entries[0].action, "ReplayOutboxDeadLetter");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);
        // Root-scoped: the entry names the synthetic Root resource, not the replayed event's own
        // aggregate — this IS the Root-only enforcement mechanism (module docs), so the audit
        // trail must actually reflect it.
        assert_eq!(entries[0].resource_prn, Some(root_prn().canonical()));
        assert_eq!(entries[0].detail["event_id"], serde_json::json!(Uuid::from_u128(1).to_string()));
        assert_eq!(entries[0].detail["event_type"], serde_json::json!("iam.principal.created"));
        // The row still exists after a replay, so its payload is not copied.
        assert!(entries[0].detail.get("payload").is_none(), "replay must not duplicate the payload");
    }

    #[tokio::test]
    async fn discard_audit_detail_carries_the_whole_event_including_the_payload() {
        let f = fixture(&[Action::DiscardOutboxDeadLetter]);
        f.dead.seed(entry(1));
        f.svc.discard(&actor(), Uuid::from_u128(1)).await.unwrap();

        assert_eq!(f.uow.commits(), 1, "discard must commit exactly once");

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "DiscardOutboxDeadLetter");
        // A discarded dead letter is gone forever — this entry is its ONLY remaining trace,
        // so it must be lossless.
        assert_eq!(entries[0].detail["payload"], serde_json::json!(serde_json::json!({"kind": "user"}).to_string()));
        assert_eq!(entries[0].detail["aggregate_prn"], serde_json::json!("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa"));
        assert_eq!(entries[0].detail["attempts"], serde_json::json!(5));
        assert_eq!(entries[0].detail["last_error"], serde_json::json!("backend error: transport closed"));
    }

    #[tokio::test]
    async fn replay_and_discard_of_an_unknown_id_are_not_found_and_write_no_audit_entry() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter, Action::DiscardOutboxDeadLetter]);
        assert!(matches!(f.svc.replay(&actor(), Uuid::from_u128(9)).await, Err(TenancyError::NotFound)));
        assert!(matches!(f.svc.discard(&actor(), Uuid::from_u128(9)).await, Err(TenancyError::NotFound)));
        assert_eq!(f.audit.0.lock().unwrap().len(), 0);
        assert_eq!(f.uow.commits(), 0, "a NotFound must never commit — the transaction is dropped, not committed");
    }

    #[tokio::test]
    async fn bulk_replay_rejects_a_missing_max_rows_before_touching_the_store() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        assert!(matches!(f.svc.replay_matching(&actor(), bulk(0)).await, Err(TenancyError::InvalidBulkReplay)));
        assert_eq!(f.dead.replay_matching_calls(), 0, "validation must happen before any store access");
        assert_eq!(f.audit.0.lock().unwrap().len(), 0);
        assert_eq!(f.uow.commits(), 0, "a rejected bulk request must never even open, let alone commit, a transaction");
    }

    #[tokio::test]
    async fn bulk_replay_audits_the_request_and_the_count() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        f.dead.seed(entry(2));
        let n = f.svc.replay_matching(&actor(), bulk(10)).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(f.uow.commits(), 1, "bulk replay must commit exactly once");

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "one bulk call is one audit entry");
        assert_eq!(entries[0].action, "ReplayOutboxDeadLetter");
        assert_eq!(entries[0].detail["replayed"], serde_json::json!(2));
        assert_eq!(entries[0].detail["max_rows"], serde_json::json!(10));
        // The requested budget (10) is well under the 10_000 cap, so both fields agree here —
        // `bulk_replay_audits_a_request_over_the_cap_distinctly_from_max_rows` below is what
        // actually proves the two fields diverge when the cap binds.
        assert_eq!(entries[0].detail["capped_max_rows"], serde_json::json!(10));
    }

    #[tokio::test]
    async fn bulk_replay_audits_a_request_over_the_cap_distinctly_from_max_rows() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        let requested = BulkReplayRequest::MAX_BULK_REPLAY + 1;
        let n = f.svc.replay_matching(&actor(), bulk(requested)).await.unwrap();
        assert_eq!(n, 1, "only the one seeded row exists to replay");

        let entries = f.audit.0.lock().unwrap();
        // `max_rows` is the REQUESTED value, verbatim — never silently overwritten by the cap.
        assert_eq!(entries[0].detail["max_rows"], serde_json::json!(requested));
        // `capped_max_rows` is the effective ceiling actually enforced — distinct from both the
        // request and the (unrelated, coincidentally small) actual replay count.
        assert_eq!(entries[0].detail["capped_max_rows"], serde_json::json!(BulkReplayRequest::MAX_BULK_REPLAY));
    }

    /// Mirrors `roles.rs`'s `a_store_error_mid_txn_rolls_back_and_never_emits_or_bumps_guard_d2`:
    /// a store failure AFTER `uow.begin()` but before `audit.record`/`tx.commit()` must surface
    /// as an error, never a false success, and must leave no audit entry and no commit behind.
    #[tokio::test]
    async fn a_store_error_mid_txn_rolls_back_and_never_audits_or_commits() {
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let authz = FakeAuthorizer::default();
        authz.allow(Action::ReplayOutboxDeadLetter, &root_prn());
        authz.allow(Action::DiscardOutboxDeadLetter, &root_prn());
        let svc = DeadLetterService::new(DeadLetterDeps {
            dead: Arc::new(FailingDeadLetters),
            uow: Arc::new(uow.clone()),
            audit: Arc::new(audit.clone()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(authz)),
        });

        let err = svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal, "a store Backend error surfaces as Internal, never a false success");

        let err = svc.replay_matching(&actor(), bulk(10)).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal);

        let err = svc.discard(&actor(), Uuid::from_u128(2)).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal);

        assert_eq!(audit.0.lock().unwrap().len(), 0, "a mid-txn store failure must never leave an audit entry behind");
        assert_eq!(uow.commits(), 0, "a mid-txn store failure must never commit");
    }
}
