// SPDX-License-Identifier: Apache-2.0

//! Outbox messages.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{InboxId, OutboxMessageId, RunId, ThreadId};

/// The payload of an outbox message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutboxPayload {
    Message { text: String },
}

/// A message that a run puts in the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    pub id: OutboxMessageId,
    pub inbox: InboxId,
    pub thread: ThreadId,
    pub run: RunId,
    pub payload: OutboxPayload,
    pub created_at: Timestamp,
}
