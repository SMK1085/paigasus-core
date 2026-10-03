// SPDX-License-Identifier: Apache-2.0

//! The charge guard (SMA-677 § 4.6, A4). One guard per admitted request. Its `Drop` charges the
//! request at most once, on the stream path and on the non-stream path, also when the client
//! disconnects (axum then drops the handler future or the response body, and the guard with it).
//! `Drop` never panics (D10): a panic during unwinding aborts the process.

use std::sync::Arc;

use metrics::counter;
use paigasus_observability::{RequestIds, names};
use uuid::Uuid;

use crate::application::limits::Limits;
use crate::domain::limits::{LimitTicket, MAX_TOKENS_PER_CHARGE};

/// The `source` label of `gateway_tokens_charged_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeSource {
    Reported,
    Estimated,
}

impl ChargeSource {
    pub const ALL: [ChargeSource; 2] = [ChargeSource::Reported, ChargeSource::Estimated];

    pub fn as_label(self) -> &'static str {
        match self {
            ChargeSource::Reported => "reported",
            ChargeSource::Estimated => "estimated",
        }
    }
}

/// Why a guard has no ticket: no budget applies, or the store failed (fail-open, D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTicket {
    NoBudget,
    StoreUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Admitted,
    Sent,
    NoCharge,
}

pub struct ChargeGuard {
    limits: Arc<Limits>,
    ticket: Option<LimitTicket>,
    org: Option<Uuid>,
    no_ticket: NoTicket,
    request_estimate: u64,
    completion_estimate: u64,
    reported: Option<u64>,
    phase: Phase,
    ids: Option<RequestIds>,
}

impl ChargeGuard {
    pub(crate) fn new(limits: Arc<Limits>, ticket: Option<LimitTicket>, org: Option<Uuid>, no_ticket: NoTicket) -> Self {
        ChargeGuard {
            limits,
            ticket,
            org,
            no_ticket,
            request_estimate: 0,
            completion_estimate: 0,
            reported: None,
            phase: Phase::Admitted,
            ids: None,
        }
    }

    pub fn has_ticket(&self) -> bool {
        self.ticket.is_some()
    }

    /// D14: the request-side estimate, charged whenever the request was sent and no usage arrives.
    pub fn set_request_estimate(&mut self, tokens: u64) {
        self.request_estimate = tokens;
    }

    /// The ids for the `chat completion metered` line. Read by the handler, where they exist; the
    /// stream runs with `current_ids() == None` (SMA-504 § 4.3 situation 3).
    pub fn set_ids(&mut self, ids: Option<RequestIds>) {
        self.ids = ids;
    }

    /// Just before the egress call.
    pub fn mark_sent(&mut self) {
        self.phase = Phase::Sent;
    }

    /// A connect failure or a non-`2xx` answer: no tokens (A4).
    pub fn mark_no_charge(&mut self) {
        self.phase = Phase::NoCharge;
    }

    /// A `2xx` answer with `usage.total_tokens`.
    pub fn set_reported(&mut self, total_tokens: u64) {
        self.reported = Some(total_tokens);
    }

    /// D14: the completion estimate of a non-stream `2xx` body without usage.
    pub fn set_completion_estimate(&mut self, tokens: u64) {
        self.completion_estimate = tokens;
    }

    /// D14: the stream so far: one token per complete `data:` record, and the last usage seen.
    pub fn set_stream_progress(&mut self, completion_records: u64, reported: Option<u64>) {
        self.completion_estimate = completion_records;
        if reported.is_some() {
            self.reported = reported;
        }
    }

    /// The tokens and source the drop would charge now (A4, D23).
    pub fn tokens(&self) -> (u64, ChargeSource) {
        match (self.phase, self.reported) {
            (Phase::Admitted | Phase::NoCharge, _) => (0, ChargeSource::Estimated),
            (Phase::Sent, Some(total)) => (total.min(MAX_TOKENS_PER_CHARGE), ChargeSource::Reported),
            (Phase::Sent, None) => (self.request_estimate.saturating_add(self.completion_estimate).min(MAX_TOKENS_PER_CHARGE), ChargeSource::Estimated),
        }
    }
}

/// Manual: `Limits` holds trait objects and has no `Debug`. `Result::expect_err` on
/// `admit`'s result needs one.
impl std::fmt::Debug for ChargeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChargeGuard")
            .field("ticket", &self.ticket)
            .field("org", &self.org)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl Drop for ChargeGuard {
    /// Never panics: no `unwrap`, no `expect`, and `LimitStore::charge` must not panic.
    fn drop(&mut self) {
        let (tokens, source) = self.tokens();
        let outcome = if tokens == 0 {
            "zero"
        } else if self.ticket.is_some() {
            "charged"
        } else {
            match self.no_ticket {
                NoTicket::NoBudget => "no_budget",
                NoTicket::StoreUnavailable => "store_unavailable",
            }
        };
        if tokens > 0
            && let Some(ticket) = self.ticket.take()
        {
            counter!(names::GATEWAY_TOKENS_CHARGED_TOTAL, "source" => source.as_label()).increment(tokens);
            self.limits.store().charge(ticket, tokens, self.limits.clock().now());
        }
        let org = self.org.map(|org| org.to_string()).unwrap_or_default();
        let (request_id, correlation_id) = self.ids.map(|ids| (ids.request_id.to_string(), ids.correlation_id.to_string())).unwrap_or_default();
        // The per-org spend record for both paths, fail-open spend included (§ 4.6).
        if tokens > 0 {
            tracing::info!(org = %org, tokens, source = source.as_label(), outcome, request_id = %request_id, correlation_id = %correlation_id, "chat completion metered");
        } else {
            tracing::debug!(org = %org, tokens, source = source.as_label(), outcome, request_id = %request_id, correlation_id = %correlation_id, "chat completion metered");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::limits::MemoryLimitStore;
    use crate::domain::limits::{BudgetPeriod, LimitDecision, LimitPolicy, LimitRules, LimitStore, LimitStoreError, LimitTicket};
    use crate::test_support::{FixedClock, ORG_A, ScriptedStore, at, counter};
    use metrics_util::debugging::DebuggingRecorder;
    use std::num::NonZeroU64;

    fn ticket() -> LimitTicket {
        LimitTicket {
            org: ORG_A,
            period: BudgetPeriod::Monthly.key_at(at("2026-10-02T12:00:00Z")),
        }
    }

    fn admit() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    fn guard_with(store: Arc<dyn LimitStore>, ticket: Option<LimitTicket>, no_ticket: NoTicket) -> ChargeGuard {
        let limits = Arc::new(Limits::new(LimitRules::default(), store, Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
        ChargeGuard::new(limits, ticket, Some(ORG_A), no_ticket)
    }

    #[test]
    fn a_reported_usage_is_charged_once_and_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let store = ScriptedStore::new(admit);
        metrics::with_local_recorder(&recorder, || {
            let mut guard = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
            guard.set_request_estimate(9);
            guard.mark_sent();
            guard.set_reported(42);
        });
        assert_eq!(store.charges(), vec![42]);
        assert_eq!(counter(&snapshotter, names::GATEWAY_TOKENS_CHARGED_TOTAL, &[("source", "reported")]), Some(42));
    }

    #[test]
    fn the_tokens_follow_the_a4_rules() {
        let store = ScriptedStore::new(admit);
        let mut g = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        g.set_request_estimate(10);
        assert_eq!(g.tokens(), (0, ChargeSource::Estimated), "never sent: zero");
        g.mark_sent();
        assert_eq!(g.tokens(), (10, ChargeSource::Estimated), "a timeout or a disconnect: the request estimate");
        g.set_completion_estimate(5);
        assert_eq!(g.tokens(), (15, ChargeSource::Estimated), "a 2xx without usage: request + completion");
        g.set_stream_progress(7, None);
        assert_eq!(g.tokens(), (17, ChargeSource::Estimated), "the stream's record count replaces the completion estimate");
        g.set_stream_progress(8, Some(30));
        assert_eq!(g.tokens(), (30, ChargeSource::Reported), "a usage record wins");
        g.mark_no_charge();
        assert_eq!(g.tokens(), (0, ChargeSource::Estimated), "a connect failure or a non-2xx: zero");
        drop(g);
        assert!(store.charges().is_empty(), "zero tokens charge nothing");
    }

    #[test]
    fn a_guard_without_a_ticket_never_charges() {
        let store = ScriptedStore::new(admit);
        let mut guard = guard_with(store.clone(), None, NoTicket::StoreUnavailable);
        guard.mark_sent();
        guard.set_reported(50);
        drop(guard);
        assert!(store.charges().is_empty(), "A12: fail-open requests are not charged");
    }

    /// Review Focus 5.
    #[test]
    fn an_absurd_usage_is_clamped_to_the_charge_cap() {
        let store = ScriptedStore::new(admit);
        let mut guard = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        guard.mark_sent();
        guard.set_reported(u64::MAX);
        drop(guard);
        assert_eq!(store.charges(), vec![MAX_TOKENS_PER_CHARGE]);
        let mut estimated = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        estimated.set_request_estimate(u64::MAX);
        estimated.mark_sent();
        estimated.set_completion_estimate(u64::MAX);
        assert_eq!(estimated.tokens(), (MAX_TOKENS_PER_CHARGE, ChargeSource::Estimated), "saturating, then clamped");
    }

    #[test]
    fn the_metered_line_names_the_outcome_and_the_ids() {
        let (logs, _g) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let ids = paigasus_observability::RequestIds {
            request_id: uuid::Uuid::from_u128(1),
            correlation_id: uuid::Uuid::from_u128(2),
        };
        let store = ScriptedStore::new(admit);
        let mut charged = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        charged.set_ids(Some(ids));
        charged.mark_sent();
        charged.set_reported(42);
        drop(charged);
        let mut fail_open = guard_with(store.clone(), None, NoTicket::StoreUnavailable);
        fail_open.set_request_estimate(3);
        fail_open.mark_sent();
        drop(fail_open);
        let mut zero = guard_with(store, Some(ticket()), NoTicket::NoBudget);
        zero.mark_sent();
        zero.mark_no_charge();
        drop(zero);
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("chat completion metered")).collect();
        assert_eq!(lines.len(), 2, "a zero-token line is debug, below INFO: {text}");
        assert!(
            lines[0].contains("outcome=\"charged\"") && lines[0].contains("tokens=42") && lines[0].contains("source=\"reported\""),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("00000000-0000-0000-0000-000000000001") && lines[0].contains("00000000-0000-0000-0000-000000000002"),
            "the ids: {}",
            lines[0]
        );
        assert!(lines[0].contains("0190a100-0000-7000-8000-0000000000a1"), "the org: {}", lines[0]);
        assert!(lines[1].contains("outcome=\"store_unavailable\"") && lines[1].contains("tokens=3"), "{}", lines[1]);
    }

    /// Spec § 5.8, memory: a guard dropped on a plain thread charges at once (no spawn).
    #[tokio::test]
    async fn a_memory_charge_from_a_plain_thread_applies_at_once() {
        let store = Arc::new(MemoryLimitStore::new());
        let t = at("2026-10-02T12:00:00Z");
        let policy = LimitPolicy {
            budget: Some(crate::domain::limits::Budget {
                tokens: NonZeroU64::new(10).expect("non-zero"),
                period: BudgetPeriod::Monthly,
            }),
            ..LimitPolicy::default()
        };
        let LimitDecision::Admit(Some(issued)) = store.check_and_admit("p", Some(ORG_A), &policy, t).await.expect("memory never fails") else {
            panic!("a budget admission issues a ticket");
        };
        let mut guard = guard_with(store.clone(), Some(issued), NoTicket::NoBudget);
        guard.mark_sent();
        guard.set_reported(10);
        std::thread::spawn(move || drop(guard)).join().expect("Drop does not panic");
        assert!(
            matches!(store.check_and_admit("p", Some(ORG_A), &policy, t).await.expect("memory never fails"), LimitDecision::Refused(_)),
            "the 10 tokens are booked"
        );
    }
}
