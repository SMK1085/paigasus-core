// SPDX-License-Identifier: Apache-2.0

//! The limit-store contract suite (SMA-677 spec § 5.2). One set of cases that BOTH adapters must
//! pass, through the port only: `tests/limits_store_memory.rs` runs it on the memory store and
//! `tests/limits_store_redis.rs` on a real Redis. A difference between the adapters' arithmetic
//! (D3) or their check-then-commit order (D4) reds one of the two runs.
//!
//! Every case takes its own principal and org ids, so cases can share one Redis. Every instant is
//! fixed; nothing sleeps or polls.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use paigasus_gateway::domain::limits::{Budget, BudgetPeriod, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitTicket, RateCounts};
use uuid::Uuid;

/// How a case reaches one backend.
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// A store on the backend. Redis: a NEW `RedisLimitStore` per call, which models one replica.
    /// Memory: the one shared store.
    async fn store(&self) -> Arc<dyn LimitStore>;
    /// Record a charge and return only when it is recorded (memory: `charge`; Redis:
    /// `apply_charge`, awaited). So no case polls.
    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime);
}

/// A fixed instant from RFC 3339.
pub fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A principal PRN and an org id that no other case uses. A counter, not a random UUID: the
/// gateway's `uuid` dependency has no `v4`/`v7` feature.
pub fn fresh_ids() -> (String, Uuid) {
    let n = u128::from(NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let principal = Uuid::from_u128(0x0190_a1e5_0000_7000_8000_0000_0000_0000 | n);
    let org = Uuid::from_u128(0x0190_a100_0000_7000_8000_0000_0000_0000 | n);
    (format!("prn:pgs:iam:::principal/{principal}"), org)
}

fn nz(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).expect("a non-zero limit")
}

fn rates(principal: Option<u64>, org: Option<u64>) -> LimitPolicy {
    LimitPolicy {
        principal_requests_per_minute: principal.map(nz),
        org_requests_per_minute: org.map(nz),
        budget: None,
    }
}

fn budget(tokens: u64, period: BudgetPeriod) -> LimitPolicy {
    LimitPolicy {
        budget: Some(Budget { tokens: nz(tokens), period }),
        ..LimitPolicy::default()
    }
}

fn rate(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> RateCounts {
    RateCounts { previous, current, elapsed_ms, limit }
}

async fn admit(store: &Arc<dyn LimitStore>, principal: &str, org: Uuid, policy: &LimitPolicy, now: SystemTime) -> LimitDecision {
    store.check_and_admit(principal, Some(org), policy, now).await.expect("the contract suite runs against a healthy store")
}

async fn ticket(store: &Arc<dyn LimitStore>, principal: &str, org: Uuid, policy: &LimitPolicy, now: SystemTime) -> LimitTicket {
    match admit(store, principal, org, policy, now).await {
        LimitDecision::Admit(Some(ticket)) => ticket,
        other => panic!("expected an admission with a ticket, got {other:?}"),
    }
}

const NOON: &str = "2026-10-02T12:00:00Z";

/// D4: an org refusal does not use the principal's quota.
pub async fn org_refusal_does_not_grow_the_principal_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let t = at(NOON);
    let both = rates(Some(2), Some(1));
    assert_eq!(admit(&store, &p, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::OrgRate(rate(0, 1, 0, 1))]));
    // If the refusal had counted the principal, its count would be 2 and this would be refused.
    let principal_only = rates(Some(2), None);
    assert_eq!(admit(&store, &p, org, &principal_only, t).await, LimitDecision::Admit(None));
    assert_eq!(
        admit(&store, &p, org, &principal_only, t).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 2, 0, 2))])
    );
}

/// D4, the reverse: a principal refusal does not use the org's quota.
pub async fn principal_refusal_does_not_grow_the_org_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p1, org) = fresh_ids();
    let (p2, _) = fresh_ids();
    let (p3, _) = fresh_ids();
    let t = at(NOON);
    let both = rates(Some(1), Some(2));
    assert_eq!(admit(&store, &p1, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p1, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 1, 0, 1))]));
    // If the refusal had counted the org, the org would be full and p2 would be refused.
    assert_eq!(admit(&store, &p2, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p3, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::OrgRate(rate(0, 2, 0, 2))]));
}

/// D4: a budget refusal grows neither rate count.
pub async fn budget_refusal_grows_neither_rate_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let t = at(NOON);
    let monthly = budget(10, BudgetPeriod::Monthly);
    let first = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(first, 10, t).await;
    let all = LimitPolicy {
        principal_requests_per_minute: Some(nz(1)),
        org_requests_per_minute: Some(nz(1)),
        ..monthly
    };
    assert_eq!(
        admit(&store, &p, org, &all, t).await,
        LimitDecision::Refused(vec![FailedCheck::Budget {
            period: BudgetPeriod::Monthly,
            resets_at_unix: 1_793_491_200
        }])
    );
    // Both rate counts are still 0: one request passes both limits of 1, the next fails both.
    let rates_only = rates(Some(1), Some(1));
    assert_eq!(admit(&store, &p, org, &rates_only, t).await, LimitDecision::Admit(None));
    assert_eq!(
        admit(&store, &p, org, &rates_only, t).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 1, 0, 1)), FailedCheck::OrgRate(rate(0, 1, 0, 1))])
    );
}

/// D7: used < budget admits; the overshoot of one request is allowed and visible.
pub async fn budget_admits_below_and_refuses_at_the_limit(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let (_, full_org) = fresh_ids();
    let t = at(NOON);
    let monthly = budget(10, BudgetPeriod::Monthly);
    let refused = LimitDecision::Refused(vec![FailedCheck::Budget {
        period: BudgetPeriod::Monthly,
        resets_at_unix: 1_793_491_200,
    }]);

    // 10 of 10 → refused.
    let t1 = ticket(&store, &p, full_org, &monthly, t).await;
    h.charge_now(t1, 10, t).await;
    assert_eq!(admit(&store, &p, full_org, &monthly, t).await, refused);

    // 9 of 10 → admitted; its charge of 50 is recorded (59 > 10), so the next one is refused.
    let t2 = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(t2, 9, t).await;
    let t3 = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(t3, 50, t).await;
    assert_eq!(admit(&store, &p, org, &monthly, t).await, refused);
}

/// D6/D7: each period starts at zero, and a late charge lands in its ticket's period.
pub async fn a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(h: &dyn Harness) {
    let store = h.store().await;
    let rows = [
        (BudgetPeriod::Daily, "2026-10-02T23:59:59Z", "2026-10-03T00:00:00Z", 1_790_985_600_i64),
        (BudgetPeriod::Weekly, "2026-10-04T23:59:59Z", "2026-10-05T00:00:00Z", 1_791_158_400),
        (BudgetPeriod::Monthly, "2026-10-31T23:59:59Z", "2026-11-01T00:00:00Z", 1_793_491_200),
    ];
    for (period, before, after, old_reset) in rows {
        let (p, org) = fresh_ids();
        let policy = budget(10, period);
        let old = ticket(&store, &p, org, &policy, at(before)).await;
        // The request ends after the period rolled; its charge goes to the OLD period.
        h.charge_now(old, 10, at(after)).await;
        match admit(&store, &p, org, &policy, at(after)).await {
            LimitDecision::Admit(Some(new)) => assert_eq!(new.period, period.key_at(at(after)), "{period:?}: the new period starts at zero"),
            other => panic!("{period:?}: the new period must admit, got {other:?}"),
        }
        assert_eq!(
            admit(&store, &p, org, &policy, at(before)).await,
            LimitDecision::Refused(vec![FailedCheck::Budget { period, resets_at_unix: old_reset }]),
            "{period:?}: the old period holds the late charge"
        );
    }
}

/// D3 at the window edge: 59.999 s and 60.000 s, the same on both adapters.
pub async fn the_window_edge_gives_the_d3_estimate(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let two = rates(Some(2), None);
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:00:00.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:00:00.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(
        admit(&store, &p, org, &two, at("2026-10-02T12:00:59.999Z")).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 2, 59_999, 2))])
    );
    assert_eq!(
        admit(&store, &p, org, &two, at("2026-10-02T12:01:00.000Z")).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(2, 0, 0, 2))])
    );
    // 2 × 30000 + 0 + 60000 = 120000 ≤ 120000.
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:01:30.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(
        admit(&store, &p, org, &two, at("2026-10-02T12:01:30.000Z")).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(2, 1, 30_000, 2))])
    );
}

/// D4 under concurrency: 64 tasks over 4 stores (4 replicas on Redis), limit 10, one instant.
pub async fn concurrent_admissions_never_pass_the_limit(h: &dyn Harness) {
    let mut stores = Vec::new();
    for _ in 0..4 {
        stores.push(h.store().await);
    }
    let (p, _) = fresh_ids();
    let policy = rates(Some(10), None);
    let t = at(NOON);
    let mut tasks = Vec::new();
    for i in 0..64 {
        let store = Arc::clone(&stores[i % 4]);
        let principal = p.clone();
        tasks.push(tokio::spawn(async move { store.check_and_admit(&principal, None, &policy, t).await.expect("a healthy store") }));
    }
    let mut admitted = 0;
    for task in tasks {
        if matches!(task.await.expect("the task does not panic"), LimitDecision::Admit(_)) {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 10, "exactly the limit is admitted");
}

/// Every case, in order. The Redis binary runs this once per image on one container.
pub async fn run_all(h: &dyn Harness) {
    org_refusal_does_not_grow_the_principal_count(h).await;
    principal_refusal_does_not_grow_the_org_count(h).await;
    budget_refusal_grows_neither_rate_count(h).await;
    budget_admits_below_and_refuses_at_the_limit(h).await;
    a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(h).await;
    the_window_edge_gives_the_d3_estimate(h).await;
    concurrent_admissions_never_pass_the_limit(h).await;
}
