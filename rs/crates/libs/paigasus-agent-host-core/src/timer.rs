// SPDX-License-Identifier: Apache-2.0

//! Timers.

use jiff::Timestamp;

use crate::ids::{InboxId, ThreadId, TimerId};

/// The kind of a timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerKind {
    FollowUp { due_at: Timestamp },
    Schedule { cron: String, tz: String },
}

/// A timer of an inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timer {
    pub id: TimerId,
    pub inbox: InboxId,
    pub kind: TimerKind,
    pub thread: Option<ThreadId>,
    pub fire_seq: u64,
}

impl Timer {
    /// The identity of the event that one firing of the timer makes.
    /// The same firing always gives the same identity.
    #[must_use]
    pub fn fire_event_id(&self) -> String {
        format!("timer:{}:{}", self.id.uuid(), self.fire_seq)
    }
}
