// SPDX-License-Identifier: Apache-2.0

//! The in-process `LimitStore` (SMA-677 D1, D4, D10, D13). For development and a single replica:
//! with N replicas behind a balancer every replica keeps its own counts, so the effective limits
//! are about N times the configured ones, and a restart resets every budget (spec § 6).
//!
//! One `std::sync::Mutex` guards both maps. It is held for the arithmetic only and never across
//! an `.await` (`check_and_admit` has none inside), so every key is checked and then committed
//! under one lock (D4). A poisoned lock is recovered: every update is a whole-integer write, so
//! the state stays consistent (D10).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use metrics::counter;
use paigasus_observability::names;
use uuid::Uuid;

use crate::domain::limits::{
    BudgetPeriod, ChargeDropReason, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket, MAX_TOKENS_PER_CHARGE, PeriodKey, SlidingWindow, budget_refusal,
};

/// D13: the sweep runs on the check path, once per this many checks. No background task.
const SWEEP_EVERY: u64 = 1024;
/// D13: two rate windows.
const RATE_IDLE: Duration = Duration::from_secs(120);

struct OrgState {
    window: SlidingWindow,
    used: HashMap<PeriodKey, u64>,
    last_touch: SystemTime,
}

impl OrgState {
    fn new(now: SystemTime) -> Self {
        OrgState {
            window: SlidingWindow::default(),
            used: HashMap::new(),
            last_touch: now,
        }
    }
}

#[derive(Default)]
struct State {
    principals: HashMap<String, (SlidingWindow, SystemTime)>,
    orgs: HashMap<Uuid, OrgState>,
    checks: u64,
}

/// A backward clock step reads as no idle time (D10).
fn idle(last: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(last).unwrap_or(Duration::ZERO)
}

impl State {
    /// D13. See this task's eviction rule in the plan.
    fn sweep(&mut self, now: SystemTime) {
        self.principals.retain(|_, (_, last)| idle(*last, now) < RATE_IDLE);
        self.orgs.retain(|_, org| {
            org.used.retain(|key, _| BudgetPeriod::charge_ttl(*key, now).is_some());
            idle(org.last_touch, now) < RATE_IDLE || !org.used.is_empty()
        });
    }
}

#[derive(Default)]
pub struct MemoryLimitStore {
    state: Mutex<State>,
}

impl MemoryLimitStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl LimitStore for MemoryLimitStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        let mut state = self.lock();
        state.checks = state.checks.wrapping_add(1);
        if state.checks.is_multiple_of(SWEEP_EVERY) {
            state.sweep(now);
        }

        // Check every dimension first (pure reads) …
        let mut failed = Vec::new();
        if let Some(limit) = policy.principal_requests_per_minute {
            let window = state.principals.get(principal).map(|(window, _)| *window).unwrap_or_default();
            if let Err(counts) = window.check(now, limit) {
                failed.push(FailedCheck::PrincipalRate(counts));
            }
        }
        if let (Some(limit), Some(id)) = (policy.org_requests_per_minute, org) {
            let window = state.orgs.get(&id).map(|o| o.window).unwrap_or_default();
            if let Err(counts) = window.check(now, limit) {
                failed.push(FailedCheck::OrgRate(counts));
            }
        }
        let budget = policy.budget.zip(org).map(|(budget, id)| (budget, id, budget.period.key_at(now)));
        if let Some((budget, id, key)) = budget {
            let used = state.orgs.get(&id).and_then(|o| o.used.get(&key)).copied().unwrap_or(0);
            if used >= budget.tokens.get() {
                failed.push(budget_refusal(budget, key));
            }
        }
        if !failed.is_empty() {
            return Ok(LimitDecision::Refused(failed));
        }

        // … then commit every rate key, all or nothing (D4).
        if policy.principal_requests_per_minute.is_some() {
            let entry = state.principals.entry(principal.to_owned()).or_insert((SlidingWindow::default(), now));
            entry.0.commit(now);
            entry.1 = now;
        }
        if let (Some(_), Some(id)) = (policy.org_requests_per_minute, org) {
            let entry = state.orgs.entry(id).or_insert_with(|| OrgState::new(now));
            entry.window.commit(now);
            entry.last_touch = now;
        }
        Ok(LimitDecision::Admit(budget.map(|(_, org, period)| LimitTicket { org, period })))
    }

    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        if tokens == 0 {
            return;
        }
        if BudgetPeriod::charge_ttl(ticket.period, now).is_none() {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::PeriodExpired.as_label()).increment(1);
            return;
        }
        let mut state = self.lock();
        let org = state.orgs.entry(ticket.org).or_insert_with(|| OrgState::new(now));
        let used = org.used.entry(ticket.period).or_insert(0);
        *used = used.saturating_add(tokens.min(MAX_TOKENS_PER_CHARGE));
        org.last_touch = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::sync::Arc;

    fn at(rfc3339: &str) -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
    }

    fn principal_limit(n: u64) -> LimitPolicy {
        LimitPolicy {
            principal_requests_per_minute: NonZeroU64::new(n),
            ..LimitPolicy::default()
        }
    }

    const ORG: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);

    /// D13: the sweep drops an idle principal and keeps an active one.
    #[tokio::test]
    async fn the_sweep_evicts_an_idle_principal_and_keeps_an_active_one() {
        let store = MemoryLimitStore::new();
        let policy = principal_limit(1_000_000);
        store.check_and_admit("idle", None, &policy, at("2026-10-02T12:00:00Z")).await.expect("memory never fails");
        // 1023 more checks: the 1024th check overall runs the sweep, 200 s after "idle" was seen.
        for _ in 0..(SWEEP_EVERY - 1) {
            store.check_and_admit("active", None, &policy, at("2026-10-02T12:03:20Z")).await.expect("memory never fails");
        }
        let state = store.lock();
        assert!(!state.principals.contains_key("idle"), "an idle principal is evicted");
        assert!(state.principals.contains_key("active"), "an active principal stays");
    }

    /// D13/D7: an org whose budget period can still take a late charge is kept.
    #[tokio::test]
    async fn the_sweep_keeps_a_budget_that_can_still_take_a_late_charge() {
        let store = MemoryLimitStore::new();
        store.charge(
            LimitTicket {
                org: ORG,
                period: BudgetPeriod::Daily.key_at(at("2026-10-02T12:00:00Z")),
            },
            5,
            at("2026-10-02T12:00:00Z"),
        );
        let policy = principal_limit(1_000_000);
        for _ in 0..SWEEP_EVERY {
            store.check_and_admit("p", None, &policy, at("2026-10-03T12:00:00Z")).await.expect("memory never fails");
        }
        assert!(store.lock().orgs.contains_key(&ORG), "the day after the period, a late charge can still land");
        for _ in 0..SWEEP_EVERY {
            store.check_and_admit("p", None, &policy, at("2026-10-04T00:00:00Z")).await.expect("memory never fails");
        }
        assert!(!store.lock().orgs.contains_key(&ORG), "a day after the period's end, the org is evicted");
    }

    /// D10: a thread panics while it holds the lock; the store still works.
    #[tokio::test]
    async fn a_poisoned_lock_is_recovered() {
        let store = Arc::new(MemoryLimitStore::new());
        let poisoner = Arc::clone(&store);
        let joined = std::thread::spawn(move || {
            let _held = poisoner.state.lock().expect("not yet poisoned");
            panic!("poison the limit store lock on purpose");
        })
        .join();
        assert!(joined.is_err(), "the thread panicked");
        assert!(store.state.is_poisoned());
        let decision = store.check_and_admit("p", None, &principal_limit(1), at("2026-10-02T12:00:00Z")).await.expect("memory never fails");
        assert_eq!(decision, LimitDecision::Admit(None));
    }

    /// D18 parity: a charge more than a day late is dropped, not booked.
    #[tokio::test]
    async fn a_charge_more_than_a_day_late_is_dropped() {
        let store = MemoryLimitStore::new();
        let october = BudgetPeriod::Monthly.key_at(at("2026-10-15T00:00:00Z"));
        store.charge(LimitTicket { org: ORG, period: october }, 5, at("2026-11-02T00:00:00Z"));
        assert!(!store.lock().orgs.contains_key(&ORG), "nothing is booked");
    }
}
