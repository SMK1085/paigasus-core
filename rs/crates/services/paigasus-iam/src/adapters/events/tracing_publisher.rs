// SPDX-License-Identifier: Apache-2.0

//! `TracingEventPublisher`: an [`EventPublisher`] that emits one structured `tracing::info!`
//! per relayed event (SMA-446, Slice B Task B8) — a placeholder sink for local dev / early
//! environments ahead of a real message-bus publisher (a later slice). There is no I/O here
//! that can fail, so `publish` always returns `Ok(())`.

use async_trait::async_trait;
use paigasus_iam_core::{DomainEvent, EventPublisher, PublishError};

/// Publishes every relayed [`DomainEvent`] as one structured `tracing::info!` — event type,
/// aggregate PRN, and correlation id — never fails. Stateless, so a unit struct (mirrors
/// `PgOutbox`'s posture) injected as `Arc<dyn EventPublisher>` by the composition root.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingEventPublisher;

impl TracingEventPublisher {
    #[must_use]
    pub fn new() -> Self {
        TracingEventPublisher
    }
}

#[async_trait]
impl EventPublisher for TracingEventPublisher {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError> {
        tracing::info!(
            event_id = %ev.id,
            event_type = ev.event_type.as_wire(),
            aggregate_prn = %ev.aggregate_prn,
            correlation_id = ?ev.correlation_id,
            "outbox event published"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use paigasus_iam_core::EventType;
    use uuid::Uuid;

    fn sample_event() -> DomainEvent {
        DomainEvent {
            id: Uuid::from_u128(1),
            event_type: EventType::PrincipalCreated,
            schema_version: 1,
            aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-000000000000".to_string(),
            actor_prn: None,
            occurred_at: Utc::now(),
            payload: serde_json::json!({"kind": "user"}),
            correlation_id: Some(Uuid::from_u128(2)),
        }
    }

    #[tokio::test]
    async fn publish_always_returns_ok() {
        let publisher = TracingEventPublisher::new();
        assert!(publisher.publish(&sample_event()).await.is_ok());
    }
}
