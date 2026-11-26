// SPDX-License-Identifier: Apache-2.0
//! The session of one request and its store time budget (SMA-753, spec D4 and D10).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use paigasus_kernel::tool_policy::{Findings, SessionInfo};
use uuid::Uuid;

use crate::decision_log::CallFacts;

/// The store time budget of one request (spec D10 "Deadline"). It reads no clock: callers pass
/// the time they spent.
#[derive(Debug)]
pub struct StoreBudget {
    limit: Duration,
    spent_nanos: AtomicU64,
    failed: AtomicBool,
}

impl StoreBudget {
    /// A budget of `limit`, with nothing spent.
    #[must_use]
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            spent_nanos: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        }
    }

    /// The time left, or `None` when the budget is spent or a store call failed.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        if self.is_failed() {
            return None;
        }
        let spent = Duration::from_nanos(self.spent_nanos.load(Ordering::Acquire));
        self.limit.checked_sub(spent).filter(|d| !d.is_zero())
    }

    /// Add `spent` to the time used. The sum saturates.
    pub fn charge(&self, spent: Duration) {
        let n = u64::try_from(spent.as_nanos()).unwrap_or(u64::MAX);
        let _ = self.spent_nanos.try_update(Ordering::AcqRel, Ordering::Acquire, |v| Some(v.saturating_add(n)));
    }

    /// Record that a store call failed. No time is left after this.
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Whether a store call failed.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// The session of one request (spec D4).
#[non_exhaustive]
#[derive(Debug)]
pub struct SessionContext {
    pub org: Uuid,
    pub session_id: Uuid,
    /// `SessionInfo::new(session_id.to_string(), turn)`.
    pub info: SessionInfo,
    /// Request-level findings: `mixed_sessions`, `forged_history`.
    pub findings: Findings,
    pub budget: StoreBudget,
    /// The decision ids whose pending event a reserve wrote. A call that is not in this set has no
    /// event to settle, so the report site appends it (SMA-753 final review, finding 1).
    reserved: Mutex<HashSet<Uuid>>,
    /// The hold intent (plan decision D-D): the calls of a hold with the decision id the decider
    /// gave each one, in order. The report site claims an entry for a call that has no decision id.
    intent: Mutex<Vec<(CallFacts, Uuid)>>,
}

impl SessionContext {
    #[must_use]
    pub fn new(org: Uuid, session_id: Uuid, turn: i64, findings: Findings, budget: StoreBudget) -> Self {
        Self {
            org,
            session_id,
            info: SessionInfo::new(session_id.to_string(), turn),
            findings,
            budget,
            reserved: Mutex::new(HashSet::new()),
            intent: Mutex::new(Vec::new()),
        }
    }

    /// Record that a reserve wrote the events of `ids`.
    pub fn mark_reserved(&self, ids: &[Uuid]) {
        self.reserved.lock().unwrap_or_else(PoisonError::into_inner).extend(ids.iter().copied());
    }

    /// Add the intent entries of a hold. They go after the entries that are already there.
    pub fn record_intent(&self, entries: Vec<(CallFacts, Uuid)>) {
        self.intent.lock().unwrap_or_else(PoisonError::into_inner).extend(entries);
    }

    /// Take the oldest unclaimed entry with facts equal to `call`, and remove it.
    #[must_use]
    pub fn claim_intent(&self, call: &CallFacts) -> Option<Uuid> {
        let mut intent = self.intent.lock().unwrap_or_else(PoisonError::into_inner);
        let at = intent.iter().position(|(facts, _)| facts == call)?;
        Some(intent.remove(at).1)
    }

    /// Remove the entries with these ids. The decider calls this with its own ids when it returns.
    pub fn forget_intent(&self, ids: &[Uuid]) {
        self.intent.lock().unwrap_or_else(PoisonError::into_inner).retain(|(_, id)| !ids.contains(id));
    }

    /// Whether a reserve wrote the event of `id`.
    #[must_use]
    pub fn is_reserved(&self, id: Uuid) -> bool {
        self.reserved.lock().unwrap_or_else(PoisonError::into_inner).contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::types::ToolCallKind;

    fn facts(name: &str) -> CallFacts {
        CallFacts::of(&crate::types::ToolCall {
            choice: 0,
            position: 0,
            id: Some("c".into()),
            name: name.into(),
            namespace: None,
            kind: ToolCallKind::Function,
            arguments: "{}".into(),
        })
    }

    fn session() -> SessionContext {
        SessionContext::new(Uuid::nil(), Uuid::nil(), 0, Findings::new(), StoreBudget::new(Duration::from_secs(1)))
    }

    #[test]
    fn equal_call_facts_claim_their_intents_in_order() {
        let s = session();
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        s.record_intent(vec![(facts("c0"), a), (facts("c0"), b)]);
        assert_eq!(s.claim_intent(&facts("c1")), None);
        assert_eq!(s.claim_intent(&facts("c0")), Some(a));
        assert_eq!(s.claim_intent(&facts("c0")), Some(b));
        assert_eq!(s.claim_intent(&facts("c0")), None);
    }

    #[test]
    fn a_forgotten_intent_cannot_be_claimed() {
        let s = session();
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        s.record_intent(vec![(facts("c0"), a)]);
        s.record_intent(vec![(facts("c0"), b)]);
        s.forget_intent(&[a]);
        assert_eq!(s.claim_intent(&facts("c0")), Some(b));
        s.forget_intent(&[b]);
        assert_eq!(s.claim_intent(&facts("c0")), None);
    }

    #[test]
    fn the_budget_shrinks_with_each_charge_and_ends_at_zero() {
        let b = StoreBudget::new(Duration::from_millis(100));
        assert_eq!(b.remaining(), Some(Duration::from_millis(100)));
        b.charge(Duration::from_millis(30));
        assert_eq!(b.remaining(), Some(Duration::from_millis(70)));
        b.charge(Duration::from_millis(60));
        assert_eq!(b.remaining(), Some(Duration::from_millis(10)));
        b.charge(Duration::from_millis(10));
        assert_eq!(b.remaining(), None);
        b.charge(Duration::from_millis(500));
        assert_eq!(b.remaining(), None);
        assert!(!b.is_failed());
    }

    #[test]
    fn a_failed_budget_has_no_time_left() {
        let b = StoreBudget::new(Duration::from_secs(1));
        assert!(!b.is_failed());
        b.fail();
        assert!(b.is_failed());
        assert_eq!(b.remaining(), None);
    }
}
