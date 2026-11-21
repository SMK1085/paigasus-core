// SPDX-License-Identifier: Apache-2.0

//! The one insert path of an event into an inbox (spec §5.2 step 7). The HTTP route and the NATS
//! consumer both call [`ingest`], so the counters mean the same on both channels.

use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, RepositoryError};
use paigasus_observability::names;

/// The channel that delivered an event. It is the value of the `channel` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Http,
    Nats,
}

impl Channel {
    #[must_use]
    pub fn as_label(&self) -> &'static str {
        match self {
            Channel::Http => "http",
            Channel::Nats => "nats",
        }
    }
}

/// Inserts the event and counts the outcome. The counters change only after the repository
/// returns `Ok`. A mismatch is logged with the inbox, `source` and `ce_id`, never with the data.
pub async fn ingest(repo: &dyn InboxRepository, event: NewInboxEvent, channel: Channel) -> Result<InsertOutcome, RepositoryError> {
    let inbox = event.inbox.uuid();
    let source = event.source.clone();
    let ce_id = event.ce_id.clone();
    let outcome = repo.insert_event(event).await?;
    let label = channel.as_label();
    match &outcome {
        InsertOutcome::Inserted { .. } => {
            metrics::counter!(names::AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL, "channel" => label).increment(1);
        }
        InsertOutcome::Duplicate { payload_matches, .. } => {
            metrics::counter!(names::AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL, "channel" => label).increment(1);
            if !payload_matches {
                metrics::counter!(names::AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL, "channel" => label).increment(1);
                tracing::warn!(%inbox, %source, %ce_id, channel = label, "a duplicate event has another type or data than the stored event");
            }
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use async_trait::async_trait;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use paigasus_agent_host_core::ids::{EventId, InboxId};
    use paigasus_agent_host_core::inbox::Inbox;
    use uuid::Uuid;

    /// An in-memory repository that returns a fixed outcome.
    struct Fixed(InsertOutcome);

    #[async_trait]
    impl InboxRepository for Fixed {
        async fn get_by_uuid(&self, _id: Uuid) -> Result<Option<Inbox>, RepositoryError> {
            Ok(None)
        }
        async fn insert_event(&self, _event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError> {
            Ok(self.0.clone())
        }
    }

    fn event() -> NewInboxEvent {
        NewInboxEvent {
            id: EventId::from_uuid(Uuid::from_u128(1)),
            inbox: InboxId::new(Uuid::from_u128(2), Uuid::from_u128(3)),
            source: "urn:test".into(),
            ce_id: "c1".into(),
            ce_type: "agents.x".into(),
            thread: None,
            envelope: serde_json::json!({}),
        }
    }

    /// The counters of one snapshot, keyed by name and `channel` label. A snapshot of the
    /// `DebuggingRecorder` resets its counters, so a test takes exactly one.
    fn counters(snapshotter: &Snapshotter) -> HashMap<(String, String), u64> {
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter_map(|(key, _, _, v)| {
                let DebugValue::Counter(n) = v else { return None };
                let channel = key.key().labels().find(|l| l.key() == "channel")?.value().to_string();
                Some(((key.key().name().to_string(), channel), n))
            })
            .collect()
    }

    /// The value of a counter in a snapshot, or 0 when the series does not exist.
    fn value(snapshot: &HashMap<(String, String), u64>, name: &str, channel: &str) -> u64 {
        snapshot.get(&(name.to_string(), channel.to_string())).copied().unwrap_or(0)
    }

    #[tokio::test]
    async fn ingest_counts_an_insert_per_channel() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Fixed(InsertOutcome::Inserted {
            id: EventId::from_uuid(Uuid::from_u128(1)),
        });
        ingest(&repo, event(), Channel::Http).await.expect("ingest");
        let snap = counters(&snap);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL, "http"), 1);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL, "nats"), 0);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL, "http"), 0);
    }

    #[tokio::test]
    async fn ingest_counts_a_duplicate_and_a_mismatch() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Fixed(InsertOutcome::Duplicate {
            id: EventId::from_uuid(Uuid::from_u128(1)),
            payload_matches: false,
        });
        ingest(&repo, event(), Channel::Nats).await.expect("ingest");
        let snap = counters(&snap);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL, "nats"), 1);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL, "nats"), 1);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL, "nats"), 0);
    }

    #[tokio::test]
    async fn a_matching_duplicate_is_not_a_mismatch() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Fixed(InsertOutcome::Duplicate {
            id: EventId::from_uuid(Uuid::from_u128(1)),
            payload_matches: true,
        });
        ingest(&repo, event(), Channel::Http).await.expect("ingest");
        let snap = counters(&snap);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL, "http"), 1);
        assert_eq!(value(&snap, names::AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL, "http"), 0);
    }
}
