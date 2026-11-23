// SPDX-License-Identifier: Apache-2.0

//! The screen of inbound events.

use async_trait::async_trait;

use crate::inbox::InboxEvent;

/// The verdict of a screen for one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenVerdict {
    Pass,
    Flag { detector: String, reason: String },
}

/// The screen could not reach a verdict.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("screen error: {0}")]
pub struct ScreenError(pub String);

/// A screen that checks an event before a run uses it.
#[async_trait]
pub trait EventScreen: Send + Sync {
    /// Screen one event.
    async fn screen(&self, event: &InboxEvent) -> Result<ScreenVerdict, ScreenError>;
}

/// The screen that passes every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

#[async_trait]
impl EventScreen for AllowAll {
    async fn screen(&self, _event: &InboxEvent) -> Result<ScreenVerdict, ScreenError> {
        Ok(ScreenVerdict::Pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, InboxId};
    use crate::inbox::EventStatus;
    use jiff::Timestamp;
    use uuid::Uuid;

    #[tokio::test]
    async fn allow_all_passes_every_event() {
        let e = InboxEvent {
            id: EventId::from_uuid(Uuid::from_u128(1)),
            inbox: InboxId::new(Uuid::from_u128(2), Uuid::from_u128(3)),
            source: "test".into(),
            ce_id: "1".into(),
            ce_type: "t".into(),
            thread: None,
            received_at: Timestamp::UNIX_EPOCH,
            status: EventStatus::Pending,
            attempts: 0,
            not_before: Timestamp::UNIX_EPOCH,
            claimed_run: None,
            envelope: serde_json::Value::Null,
            data: serde_json::Value::Null,
        };
        assert_eq!(AllowAll.screen(&e).await, Ok(ScreenVerdict::Pass));
    }
}
