// SPDX-License-Identifier: Apache-2.0

//! Bounded, non-blocking denial-audit buffer + background drain task (SMA-446 Slice A,
//! Task A6). Authorization denials happen on the hot request path, so recording one must
//! never block or fail that path: [`DenialAuditBuffer::push`] only ever takes a
//! `std::sync::Mutex` for the enqueue itself (never `.await`s while holding it) and is
//! bounded — when full, the **oldest** queued entry is dropped (favoring recency) and the
//! [`DenialAuditBuffer::dropped`] counter is bumped so overflow is observable. A separate
//! [`DenialAuditDrain::run`] task wakes on a [`tokio::sync::Notify`] and persists queued
//! entries via the [`AuditLog`] port out of band; a per-entry persistence failure is logged
//! and swallowed (fail-open — a denial-audit hiccup must never turn into a request-path or
//! process failure).
//!
//! [`BufferedDenialAuditSink`] is the [`AuditSink`] that feeds the buffer: it reacts only to
//! [`Effect::Deny`] (allows are not audited in Slice A) and builds an [`AuditEntry`] from the
//! [`AuthzDecisionEvent`] — mirroring `TracingAuditSink`'s doc note (`adapters/authz/audit.rs`),
//! every field copied here (principal/resource PRNs, action, determining policies) is already
//! safe to log, so `detail` is left an empty object rather than carrying anything sensitive.

use async_trait::async_trait;
use paigasus_iam_core::authz::model::AuthzDecisionEvent;
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, AuditSink, Effect, IdGenerator};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// A bounded ring buffer of denial [`AuditEntry`]s awaiting persistence. Cheap to construct
/// (`capacity` items pre-reserved); shared between the [`AuditSink`] producer and the
/// [`DenialAuditDrain`] consumer via `Arc`.
pub struct DenialAuditBuffer {
    queue: Mutex<VecDeque<AuditEntry>>,
    capacity: usize,
    dropped: AtomicU64,
    notify: Notify,
}

impl DenialAuditBuffer {
    /// Builds the buffer plus its paired drain handle. The two are always created together
    /// so a buffer can never exist without something able to drain it.
    #[must_use]
    pub fn new(capacity: usize) -> (Arc<Self>, DenialAuditDrain) {
        let buf = Arc::new(DenialAuditBuffer {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            dropped: AtomicU64::new(0),
            notify: Notify::new(),
        });
        let drain = DenialAuditDrain { buf: Arc::clone(&buf) };
        (buf, drain)
    }

    /// Enqueues `entry`, never blocking and never `.await`ing: the `Mutex` is held only long
    /// enough to push (and, if already at `capacity`, first pop the oldest entry — bumping
    /// [`Self::dropped`]). Wakes one waiting [`DenialAuditDrain::run`] task afterward.
    pub fn push(&self, entry: AuditEntry) {
        {
            let mut q = self.queue.lock().unwrap();
            if q.len() >= self.capacity {
                q.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(entry);
        }
        self.notify.notify_one();
    }

    /// Total entries ever dropped for being enqueued while the buffer was already at
    /// `capacity` (monotonic, never reset).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Empties the queue and returns everything that was in it, in oldest-first order. The
    /// lock is held only for the drain itself, never across the caller's subsequent
    /// (`async`) persistence work.
    fn drain_all(&self) -> Vec<AuditEntry> {
        self.queue.lock().unwrap().drain(..).collect()
    }

    #[cfg(test)]
    pub fn drain_for_test(&self) -> Vec<AuditEntry> {
        self.drain_all()
    }

    #[cfg(test)]
    pub fn len_for_test(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

/// The background task pairing with a [`DenialAuditBuffer`]: wakes whenever the buffer is
/// pushed to (or on a periodic-ish churn via repeated notifications), drains everything
/// queued, and persists each entry via [`AuditLog::record_out_of_band`].
pub struct DenialAuditDrain {
    buf: Arc<DenialAuditBuffer>,
}

impl DenialAuditDrain {
    /// Runs until `shutdown` resolves. Each wake drains the buffer and persists every entry
    /// found; a persistence error is logged (`tracing::warn!`) and swallowed — fail-open, so
    /// a `sink` outage never crashes the drain task or blocks producers. Entries queued
    /// after the final drain (i.e. between the last wake and `shutdown` resolving) are left
    /// unpersisted, same as any other bounded, best-effort audit path.
    pub async fn run(self, sink: Arc<dyn AuditLog>, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = self.buf.notify.notified() => {}
                () = &mut shutdown => break,
            }
            for entry in self.buf.drain_all() {
                if let Err(e) = sink.record_out_of_band(&entry).await {
                    tracing::warn!(
                        error = %e,
                        action = %entry.action,
                        "failed to persist denial audit entry; dropping it (fail-open)"
                    );
                }
            }
        }
    }
}

/// The [`AuditSink`] that feeds a [`DenialAuditBuffer`]: on [`Effect::Deny`] it builds an
/// [`AuditEntry`] and [`DenialAuditBuffer::push`]es it (non-blocking); [`Effect::Allow`] is
/// ignored (Slice A audits denials only — see the module doc).
pub struct BufferedDenialAuditSink {
    buf: Arc<DenialAuditBuffer>,
    ids: Arc<dyn IdGenerator>,
}

impl BufferedDenialAuditSink {
    #[must_use]
    pub fn new(buf: Arc<DenialAuditBuffer>, ids: Arc<dyn IdGenerator>) -> Self {
        Self { buf, ids }
    }
}

#[async_trait]
impl AuditSink for BufferedDenialAuditSink {
    async fn record(&self, ev: &AuthzDecisionEvent) {
        if ev.effect != Effect::Deny {
            return;
        }
        self.buf.push(AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: ev.at,
            actor_prn: Some(ev.principal_prn.clone()),
            action: ev.action.clone(),
            resource_prn: Some(ev.resource_prn.clone()),
            outcome: AuditOutcome::Denied,
            determining_policies: ev.determining_policies.clone(),
            detail: serde_json::json!({}),
            correlation_id: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::SeqIds;
    use chrono::Utc;
    use uuid::Uuid;

    fn entry(action: &str) -> AuditEntry {
        AuditEntry {
            id: Uuid::nil(),
            occurred_at: Utc::now(),
            actor_prn: None,
            action: action.to_string(),
            resource_prn: None,
            outcome: AuditOutcome::Denied,
            determining_policies: vec![],
            detail: serde_json::json!({}),
            correlation_id: None,
        }
    }

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

    #[tokio::test]
    async fn buffer_drops_oldest_when_full_and_counts_drops() {
        let (buf, _drain) = DenialAuditBuffer::new(2);
        buf.push(entry("a"));
        buf.push(entry("b"));
        buf.push(entry("c")); // c evicts a
        assert_eq!(buf.dropped(), 1);
        let drained = buf.drain_for_test(); // test-only helper returning Vec
        assert_eq!(drained.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    }

    #[tokio::test]
    async fn sink_enqueues_denies_and_ignores_allows() {
        let (buf, _d) = DenialAuditBuffer::new(8);
        let sink = BufferedDenialAuditSink::new(buf.clone(), Arc::new(SeqIds::default()));
        sink.record(&event(Effect::Allow)).await;
        sink.record(&event(Effect::Deny)).await;
        assert_eq!(buf.len_for_test(), 1);
    }
}
