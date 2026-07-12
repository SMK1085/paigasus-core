// SPDX-License-Identifier: Apache-2.0

//! `TracingAuditSink`: the default [`AuditSink`] (ADR-0013, spec §7) — emits one structured
//! `tracing` event per [`AuthzDecisionEvent`], `info` for `Allow` and `warn` for `Deny` (so a
//! log pipeline can alert on denies without a separate field-value filter). This is a
//! log-only sink; M5 plugs in a persistent one (e.g. writing to a `authz_audit_log` table)
//! behind the same [`AuditSink`] port, so no caller needs to change when that lands.
//!
//! **No sensitive material:** [`AuthzDecisionEvent`] carries only PRNs, the action name, the
//! effect, and the determining-policy ids — never a bearer token, claim, or other secret —
//! so every field here is safe to log verbatim.

use async_trait::async_trait;
use paigasus_iam_core::authz::model::AuthzDecisionEvent;
use paigasus_iam_core::{AuditSink, Effect};
use std::sync::Arc;

/// The default [`AuditSink`]: logs every decision via `tracing`, nothing more.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingAuditSink;

#[async_trait]
impl AuditSink for TracingAuditSink {
    async fn record(&self, ev: &AuthzDecisionEvent) {
        match ev.effect {
            Effect::Allow => tracing::info!(
                principal = %ev.principal_prn,
                action = %ev.action,
                resource = %ev.resource_prn,
                effect = ?ev.effect,
                determining_policies = ?ev.determining_policies,
                "authz decision"
            ),
            Effect::Deny => tracing::warn!(
                principal = %ev.principal_prn,
                action = %ev.action,
                resource = %ev.resource_prn,
                effect = ?ev.effect,
                determining_policies = ?ev.determining_policies,
                "authz decision"
            ),
        }
    }
}

/// An [`AuditSink`] that fans one decision event out to several inner sinks in order
/// (SMA-446 Task A12). `CedarAuthorizer` takes exactly one `Arc<dyn AuditSink>`, but Slice A
/// needs a decision recorded to BOTH the log-only [`TracingAuditSink`] AND the persistent
/// [`BufferedDenialAuditSink`](super::denial_audit::BufferedDenialAuditSink); this composes
/// them into that single port. `record` awaits each inner sink sequentially — every Slice-A
/// sink is non-blocking (a `tracing` macro; a bounded, lock-only buffer push), so there is no
/// benefit to `join!`ing them, and sequential keeps the ordering (and any future
/// backpressure) trivially predictable.
pub struct FanOutAuditSink {
    sinks: Vec<Arc<dyn AuditSink>>,
}

impl FanOutAuditSink {
    #[must_use]
    pub fn new(sinks: Vec<Arc<dyn AuditSink>>) -> Self {
        Self { sinks }
    }
}

#[async_trait]
impl AuditSink for FanOutAuditSink {
    async fn record(&self, ev: &AuthzDecisionEvent) {
        for sink in &self.sinks {
            sink.record(ev).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn event(effect: Effect) -> AuthzDecisionEvent {
        AuthzDecisionEvent {
            principal_prn: "prn:pgs:iam:::principal/00000000-0000-7000-8000-000000000001".to_string(),
            action: "GetProject".to_string(),
            resource_prn: "prn:pgs:iam:::project/00000000-0000-7000-8000-000000000002".to_string(),
            effect,
            determining_policies: vec!["some-policy".to_string()],
            at: Utc::now(),
        }
    }

    /// `record` never panics and returns for either effect — the only behavior this sink
    /// has beyond the `tracing` macros themselves (whose output isn't asserted here; the
    /// `info`-on-allow/`warn`-on-deny split is the documented contract, not something a unit
    /// test can observe without a subscriber harness).
    #[tokio::test]
    async fn record_completes_for_both_effects() {
        let sink = TracingAuditSink;
        sink.record(&event(Effect::Allow)).await;
        sink.record(&event(Effect::Deny)).await;
    }

    /// A counting [`AuditSink`] fake: records how many times `record` was called, so a test
    /// can assert `FanOutAuditSink` reached every inner sink exactly once per event.
    #[derive(Default)]
    struct CountingSink {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl AuditSink for CountingSink {
        async fn record(&self, _ev: &AuthzDecisionEvent) {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn fan_out_forwards_each_event_to_every_inner_sink() {
        let a = Arc::new(CountingSink::default());
        let b = Arc::new(CountingSink::default());
        let fan = FanOutAuditSink::new(vec![a.clone() as Arc<dyn AuditSink>, b.clone() as Arc<dyn AuditSink>]);

        fan.record(&event(Effect::Deny)).await;
        fan.record(&event(Effect::Allow)).await;

        assert_eq!(a.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(b.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn fan_out_over_no_sinks_is_a_no_op() {
        // A degenerate empty fan-out must still satisfy the port without panicking.
        let fan = FanOutAuditSink::new(vec![]);
        fan.record(&event(Effect::Deny)).await;
    }
}
