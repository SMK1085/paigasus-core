// SPDX-License-Identifier: Apache-2.0

//! The limit-store contract suite (spec § 5.2) on `MemoryLimitStore`.

mod support;

use std::sync::Arc;
use std::time::SystemTime;

use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::{LimitStore, LimitTicket};
use support::limits_contract::{self as contract, Harness};

struct MemoryHarness {
    store: Arc<MemoryLimitStore>,
}

#[async_trait::async_trait]
impl Harness for MemoryHarness {
    async fn store(&self) -> Arc<dyn LimitStore> {
        self.store.clone()
    }

    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        // Synchronous: the memory adapter applies a charge under its lock and returns (D16).
        self.store.charge(ticket, tokens, now);
    }
}

fn harness() -> MemoryHarness {
    MemoryHarness {
        store: Arc::new(MemoryLimitStore::new()),
    }
}

#[tokio::test]
async fn org_refusal_does_not_grow_the_principal_count() {
    contract::org_refusal_does_not_grow_the_principal_count(&harness()).await;
}

#[tokio::test]
async fn principal_refusal_does_not_grow_the_org_count() {
    contract::principal_refusal_does_not_grow_the_org_count(&harness()).await;
}

#[tokio::test]
async fn budget_refusal_grows_neither_rate_count() {
    contract::budget_refusal_grows_neither_rate_count(&harness()).await;
}

#[tokio::test]
async fn budget_admits_below_and_refuses_at_the_limit() {
    contract::budget_admits_below_and_refuses_at_the_limit(&harness()).await;
}

#[tokio::test]
async fn a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period() {
    contract::a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(&harness()).await;
}

#[tokio::test]
async fn the_window_edge_gives_the_d3_estimate() {
    contract::the_window_edge_gives_the_d3_estimate(&harness()).await;
}

#[tokio::test]
async fn the_clamp_boundary_refuses_at_59_999_and_admits_at_60_000() {
    contract::the_clamp_boundary_refuses_at_59_999_and_admits_at_60_000(&harness()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admissions_never_pass_the_limit() {
    contract::concurrent_admissions_never_pass_the_limit(&harness()).await;
}
