// SPDX-License-Identifier: Apache-2.0

//! The limits of timers and the changes that a run buffers.

use super::{Timer, TimerOrigin};
use crate::ids::TimerId;

/// The limits of timers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerCaps {
    /// The shortest time between two firings of a schedule.
    pub min_schedule_interval_secs: u64,
    /// The most active timers of one inbox.
    pub max_active_per_inbox: u32,
    /// The furthest a timer may fire in the future, in days.
    pub max_horizon_days: u32,
    /// The shortest delay of a follow-up.
    pub min_follow_up_delay_secs: u64,
    /// The most timers that the agent may make in one day.
    pub max_agent_timers_created_per_day: u32,
}

impl Default for TimerCaps {
    fn default() -> Self {
        TimerCaps {
            min_schedule_interval_secs: 900,
            max_active_per_inbox: 50,
            max_horizon_days: 366,
            min_follow_up_delay_secs: 60,
            max_agent_timers_created_per_day: 100,
        }
    }
}

/// The timer changes of one run. The run commit writes them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerChanges {
    /// The timers to add.
    pub inserts: Vec<Timer>,
    /// The timers to cancel, with the origin that asks for the cancel.
    pub cancels: Vec<(TimerId, TimerOrigin)>,
}

impl TimerChanges {
    /// True when there is no insert and no cancel.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.cancels.is_empty()
    }
}

/// The result of one cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The timer is now cancelled.
    Cancelled,
    /// The timer was not active, or it is not of this origin.
    NotActive,
}

/// The result of an apply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// The outcome of each cancel, in order.
    pub cancels: Vec<(TimerId, CancelOutcome)>,
}

/// The error of an apply. No text holds a note.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// A timer belongs to another inbox.
    #[error("timer {} belongs to another inbox", id.uuid())]
    InboxMismatch {
        /// The timer.
        id: TimerId,
    },
    /// An insert is not valid.
    #[error("timer {} is not a valid insert", id.uuid())]
    InvalidInsert {
        /// The timer.
        id: TimerId,
    },
    /// The apply would exceed the active timer limit.
    #[error("active timer limit exceeded: {before} before, {after} after, limit {limit}")]
    CapExceeded {
        /// The number of active timers before the apply.
        before: u32,
        /// The number of active timers after the apply.
        after: u32,
        /// The limit.
        limit: u32,
    },
    /// The apply would exceed the daily creation budget.
    #[error("daily timer budget exceeded: {created} made, limit {limit}")]
    BudgetExceeded {
        /// The number of timers made in the day, with the new ones.
        created: u32,
        /// The limit.
        limit: u32,
    },
    /// The inbox has no `inbox_lease` row. This is a bug: the run commit holds that row.
    #[error("the inbox has no lease row")]
    LeaseMissing,
    /// The store failed. The text is the cause.
    #[error("store error: {0}")]
    Store(String),
}
