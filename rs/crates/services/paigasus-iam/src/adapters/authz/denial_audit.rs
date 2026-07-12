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
use std::time::Duration;
use tokio::sync::Notify;

/// Ceiling on a single [`AuditLog::record_out_of_band`] call inside [`DenialAuditDrain::
/// drain_into`]. A hung Postgres call must never park the drain forever: the same
/// `drain_into` call also runs as [`DenialAuditDrain::run`]'s final shutdown flush, and an
/// unbounded hang there would stall graceful shutdown (the server `JoinSet` in `main.rs`
/// never completing). A timeout is treated the same as any other persistence failure here —
/// logged and swallowed (fail-open).
const AUDIT_PERSIST_TIMEOUT: Duration = Duration::from_secs(5);

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
        // Defend against a misconfigured `capacity: 0`, which would otherwise make every
        // `push` immediately evict the entry it just enqueued (silently discarding all
        // denial audits without ever counting them past the very first `dropped()` bump).
        let capacity = capacity.max(1);
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
    /// a `sink` outage never crashes the drain task or blocks producers. On `shutdown`, the
    /// loop breaks *without* draining (there may be no pending notification to wake it if the
    /// last push landed after the final regular wake), so a final `drain_all` + persist pass
    /// runs once more before `run` returns — otherwise entries buffered right at shutdown
    /// would be silently lost (not even reflected in [`DenialAuditBuffer::dropped`]).
    pub async fn run(self, sink: Arc<dyn AuditLog>, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = self.buf.notify.notified() => {}
                () = &mut shutdown => break,
            }
            Self::drain_into(&self.buf, &sink).await;
        }
        // Final drain: same fail-open, per-entry error handling as every regular wake, so a
        // graceful shutdown never drops a denial that was already buffered.
        Self::drain_into(&self.buf, &sink).await;
    }

    /// Drains everything currently queued and persists each entry via
    /// [`AuditLog::record_out_of_band`], logging and swallowing per-entry failures
    /// (fail-open) — including a call that doesn't return within [`AUDIT_PERSIST_TIMEOUT`],
    /// so a hung backend can never park the drain (see that const's doc). Shared by the
    /// regular wake loop and the final shutdown drain in [`Self::run`] so both paths behave
    /// identically.
    async fn drain_into(buf: &DenialAuditBuffer, sink: &Arc<dyn AuditLog>) {
        for entry in buf.drain_all() {
            match tokio::time::timeout(AUDIT_PERSIST_TIMEOUT, sink.record_out_of_band(&entry)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        action = %entry.action,
                        "failed to persist denial audit entry; dropping it (fail-open)"
                    );
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        timeout_secs = AUDIT_PERSIST_TIMEOUT.as_secs(),
                        action = %entry.action,
                        "timed out persisting denial audit entry; dropping it (fail-open)"
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
    use paigasus_iam_core::Transaction;
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

    /// Test-only [`AuditLog`] fake that captures every persisted entry (in call order) instead
    /// of writing anywhere, so a test can assert on exactly what `DenialAuditDrain::run`
    /// persisted.
    #[derive(Default)]
    struct CapturingAuditLog {
        recorded: Mutex<Vec<AuditEntry>>,
    }

    #[async_trait]
    impl AuditLog for CapturingAuditLog {
        async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), paigasus_iam_core::RepositoryError> {
            self.recorded.lock().unwrap().push(e.clone());
            Ok(())
        }

        async fn record(&self, _tx: &dyn Transaction, e: &AuditEntry) -> Result<(), paigasus_iam_core::RepositoryError> {
            self.recorded.lock().unwrap().push(e.clone());
            Ok(())
        }

        async fn query(&self, _f: &paigasus_iam_core::AuditFilter) -> Result<Vec<AuditEntry>, paigasus_iam_core::RepositoryError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn run_drains_buffer_on_shutdown_before_returning() {
        let (buf, drain) = DenialAuditBuffer::new(8);
        buf.push(entry("a"));
        buf.push(entry("b"));

        // `push` calls `Notify::notify_one`, which stores a single wake permit even though
        // nothing is waiting yet. Consume it here so `run`'s very first `select!` has no
        // pending notification and only `shutdown` is ready — otherwise the notify branch
        // could win that race non-deterministically, drain the buffer through the "normal"
        // wake path, and mask whether the shutdown path drains on its own.
        buf.notify.notified().await;

        let sink = Arc::new(CapturingAuditLog::default());
        let handle = tokio::spawn(drain.run(sink.clone(), std::future::ready(())));
        handle.await.expect("drain task must not panic");

        let recorded = sink.recorded.lock().unwrap();
        assert_eq!(recorded.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    /// Test-only [`AuditLog`] fake whose `record_out_of_band` never returns — simulates a
    /// hung Postgres call, to verify [`DenialAuditDrain::drain_into`]'s [`AUDIT_PERSIST_TIMEOUT`]
    /// fires instead of parking the drain forever.
    struct HangingAuditLog;

    #[async_trait]
    impl AuditLog for HangingAuditLog {
        async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), paigasus_iam_core::RepositoryError> {
            std::future::pending().await
        }

        async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), paigasus_iam_core::RepositoryError> {
            std::future::pending().await
        }

        async fn query(&self, _f: &paigasus_iam_core::AuditFilter) -> Result<Vec<AuditEntry>, paigasus_iam_core::RepositoryError> {
            Ok(vec![])
        }
    }

    // `start_paused = true` freezes the clock and auto-advances it once every other task is
    // idle — so this exercises the real `AUDIT_PERSIST_TIMEOUT` (5s) deterministically and
    // fast, instead of either a flaky short-timeout stand-in or an actually-slow real sleep.
    #[tokio::test(start_paused = true)]
    async fn drain_into_times_out_a_hung_persist_call_instead_of_hanging() {
        let (buf, _drain) = DenialAuditBuffer::new(8);
        buf.push(entry("a"));
        let sink: Arc<dyn AuditLog> = Arc::new(HangingAuditLog);

        // Without the timeout this await would never return (`HangingAuditLog` never
        // resolves) and the test would hang until the harness's own timeout killed it.
        DenialAuditDrain::drain_into(&buf, &sink).await;

        // The entry was still drained out of the queue even though persisting it never
        // succeeded — the timeout path drops it and moves on, same as any other
        // persistence failure (fail-open).
        assert_eq!(buf.len_for_test(), 0);
    }
}
