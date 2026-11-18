// SPDX-License-Identifier: Apache-2.0

//! The Redis side of the decision log (SMA-755 § 5.6, § 5.8). `RedisDecisionLog` takes a record, maps it to
//! the proto, encodes it and puts it into a bounded queue. The writer task (`writer`) moves the queue into a
//! Redis stream. The connect loop (`redis_conn`) makes the Redis connection without a block of the boot.

pub mod jetstream;
pub mod proto;
pub mod redis_conn;
pub mod relay;
pub mod supervised;
pub mod writer;

use bytes::Bytes;
use metrics::counter;
use paigasus_gateway_core::decision_log::DecisionRecord;
use paigasus_observability::names;
use prost::Message;
use tokio::sync::mpsc;

use crate::domain::decision_log::{DecisionLog, log_line};

pub use redis_conn::{DECISION_LOG_BREAKER_ROLE, RedisSlot, connect_loop};
pub use relay::{RelaySettings, run_relay};
pub use supervised::run_supervised;
pub use writer::{WriterSettings, run_writer};

/// Why an event did not reach the stream. The labels are the § 5.8 values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    QueueFull,
    Closed,
    RedisUnavailable,
    WriteError,
    BreakerOpen,
    BacklogFull,
    PublishRejected,
    RedisDeleted,
    Shutdown,
}

impl DropReason {
    pub const ALL: [DropReason; 9] = [
        DropReason::QueueFull,
        DropReason::Closed,
        DropReason::RedisUnavailable,
        DropReason::WriteError,
        DropReason::BreakerOpen,
        DropReason::BacklogFull,
        DropReason::PublishRejected,
        DropReason::RedisDeleted,
        DropReason::Shutdown,
    ];

    #[must_use]
    pub fn as_label(self) -> &'static str {
        match self {
            DropReason::QueueFull => "queue_full",
            DropReason::Closed => "closed",
            DropReason::RedisUnavailable => "redis_unavailable",
            DropReason::WriteError => "write_error",
            DropReason::BreakerOpen => "breaker_open",
            DropReason::BacklogFull => "backlog_full",
            DropReason::PublishRejected => "publish_rejected",
            DropReason::RedisDeleted => "redis_deleted",
            DropReason::Shutdown => "shutdown",
        }
    }
}

/// Count `n` dropped events for `reason`.
pub fn count_dropped(reason: DropReason, n: u64) {
    if n > 0 {
        counter!(names::GATEWAY_DECISION_LOG_DROPPED_TOTAL, "reason" => reason.as_label()).increment(n);
    }
}

/// Register every decision-log counter at zero, so that `increase()` sees the first event.
pub fn prime_decision_log_metrics() {
    counter!(names::GATEWAY_DECISION_LOG_RECORDED_TOTAL).increment(0);
    counter!(names::GATEWAY_DECISION_LOG_WRITTEN_TOTAL).increment(0);
    counter!(names::GATEWAY_DECISION_LOG_PUBLISHED_TOTAL).increment(0);
    counter!(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL).increment(0);
    counter!(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL).increment(0);
    for reason in DropReason::ALL {
        counter!(names::GATEWAY_DECISION_LOG_DROPPED_TOTAL, "reason" => reason.as_label()).increment(0);
    }
    for task in ["writer", "relay"] {
        counter!(names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL, "task" => task).increment(0);
    }
}

/// The log of `[decision_log] enabled = true`: the tracing line, and an encoded event in a bounded queue.
#[derive(Debug, Clone)]
pub struct RedisDecisionLog {
    tx: mpsc::Sender<Bytes>,
}

impl RedisDecisionLog {
    /// Make the log and the receiver of its queue. The receiver goes to `run_writer`.
    #[must_use]
    pub fn new(queue_capacity: usize) -> (Self, mpsc::Receiver<Bytes>) {
        let (tx, rx) = mpsc::channel(queue_capacity.max(1));
        (Self { tx }, rx)
    }
}

impl DecisionLog for RedisDecisionLog {
    fn wants_digest(&self) -> bool {
        true
    }

    fn record(&self, record: DecisionRecord) {
        log_line(&record);
        let bytes = Bytes::from(proto::to_event(&record).encode_to_vec());
        counter!(names::GATEWAY_DECISION_LOG_RECORDED_TOTAL).increment(1);
        match self.tx.try_send(bytes) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => count_dropped(DropReason::QueueFull, 1),
            Err(mpsc::error::TrySendError::Closed(_)) => count_dropped(DropReason::Closed, 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_gateway_core::decision_log::{RecordHeader, ReportedCall, build_response};
    use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
    use std::time::SystemTime;
    use uuid::Uuid;

    fn record() -> DecisionRecord {
        let entry = ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision: ReportDecision::Block,
            basis: Some(Basis::Decider),
            code: Some(BlockCode::Policy),
            enforced: Some(true),
            reason: None,
        };
        let ctx = DecisionContext::new("prn:pgs:iam:::user/u", "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", Some("req-1".into()));
        build_response(
            RecordHeader::new(Uuid::nil(), SystemTime::UNIX_EPOCH, Uuid::nil(), 1, None),
            &ctx,
            &ReportedCall::new(entry, None, None),
        )
    }

    type Rows = Vec<(metrics_util::CompositeKey, Option<metrics::Unit>, Option<metrics::SharedString>, metrics_util::debugging::DebugValue)>;

    // `Snapshotter::snapshot` resets the counters it reads, so each test takes one snapshot and reads it as a whole.
    fn dropped(snap: &Rows, reason: &str) -> Option<u64> {
        snapshot_counter(snap, names::GATEWAY_DECISION_LOG_DROPPED_TOTAL, &[("reason", reason)])
    }

    fn snapshot_counter(snap: &Rows, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
        use metrics_util::debugging::DebugValue;
        snap.iter().find_map(|(key, _, _, value)| {
            let key = key.key();
            let have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
            let same = key.name() == name && have.len() == labels.len() && labels.iter().all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v));
            match (same, value) {
                (true, DebugValue::Counter(n)) => Some(*n),
                _ => None,
            }
        })
    }

    #[test]
    fn a_full_queue_counts_queue_full() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            let (log, _rx) = RedisDecisionLog::new(1);
            log.record(record());
            log.record(record());
        });
        let snap = snapshotter.snapshot().into_vec();
        assert_eq!(dropped(&snap, "queue_full"), Some(1));
        assert_eq!(snapshot_counter(&snap, names::GATEWAY_DECISION_LOG_RECORDED_TOTAL, &[]), Some(2));
    }

    #[test]
    fn a_closed_queue_counts_closed() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            let (log, rx) = RedisDecisionLog::new(4);
            drop(rx);
            log.record(record());
        });
        let snap = snapshotter.snapshot().into_vec();
        assert_eq!(dropped(&snap, "closed"), Some(1));
    }

    #[test]
    fn the_encoded_event_holds_no_canary() {
        use paigasus_gateway_core::decision_log::ArgumentDigest;
        // The record type has no field for arguments, only a keyed digest, so a canary in the call input
        // cannot reach the event bytes. The digest is in the event and the canary is not.
        let canary = "s3cr3t-canary";
        let mut r = record();
        r.arguments = Some(ArgumentDigest::new([7; 32], "k1"));
        let bytes = proto::to_event(&r).encode_to_vec();
        assert!(!bytes.windows(canary.len()).any(|w| w == canary.as_bytes()));
        assert!(bytes.windows(32).any(|w| w == [7u8; 32]), "the digest is in the event");
    }

    #[test]
    fn prime_primes_every_reason_at_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, prime_decision_log_metrics);
        let snap = snapshotter.snapshot().into_vec();
        for reason in DropReason::ALL {
            assert_eq!(dropped(&snap, reason.as_label()), Some(0), "{}", reason.as_label());
        }
        for task in ["writer", "relay"] {
            assert_eq!(snapshot_counter(&snap, names::GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL, &[("task", task)]), Some(0));
        }
        assert_eq!(snapshot_counter(&snap, names::GATEWAY_DECISION_LOG_RECORDED_TOTAL, &[]), Some(0));
        assert_eq!(snapshot_counter(&snap, names::GATEWAY_DECISION_LOG_WRITTEN_TOTAL, &[]), Some(0));
        for name in [
            names::GATEWAY_DECISION_LOG_PUBLISHED_TOTAL,
            names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL,
            names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL,
        ] {
            assert_eq!(snapshot_counter(&snap, name, &[]), Some(0), "{name}");
        }
        assert_eq!(snap.len(), 9 + 2 + 2 + 3);
    }

    #[test]
    fn the_labels_are_the_spec_values() {
        let labels: Vec<_> = DropReason::ALL.iter().map(|r| r.as_label()).collect();
        assert_eq!(
            labels,
            [
                "queue_full",
                "closed",
                "redis_unavailable",
                "write_error",
                "breaker_open",
                "backlog_full",
                "publish_rejected",
                "redis_deleted",
                "shutdown"
            ]
        );
    }
}
