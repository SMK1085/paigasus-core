// SPDX-License-Identifier: Apache-2.0

//! The plan of one firing: the time of the event and the next state of the timer.

use jiff::Timestamp;

use super::{Slot, Timer, TimerKind, WalkError};

/// What follows a firing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// The timer has no next firing.
    Done,
    /// The timer fires again at `at`, for the slot `slot`.
    Advance {
        /// The slot of the next firing.
        slot: Slot,
        /// The instant of the next firing.
        at: Timestamp,
    },
}

/// The plan of one firing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirePlan {
    /// The time that the event carries. It is always the stored `next_fire_at` of the timer.
    pub fire_at: Timestamp,
    /// The state of the timer after the firing.
    pub next: Next,
}

/// The reason that a firing cannot be planned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The walk to the next slot failed.
    #[error("the walk to the next slot failed: {0}")]
    Walk(#[from] WalkError),
}

/// Plan the firing of a due timer. The caller passes only due timers, so `now` is at or after
/// `timer.next_fire_at()`.
///
/// The walk starts at the stored slot and not at the stored instant. So a change of the zone rules
/// cannot make a slot fire twice.
///
/// # Errors
/// `Walk` when the walk to the next slot fails.
pub fn plan_fire(timer: &Timer, now: Timestamp, bound: u32) -> Result<FirePlan, PlanError> {
    let fire_at = timer.next_fire_at();
    let next = match timer.kind() {
        TimerKind::FollowUp => Next::Done,
        TimerKind::Schedule { schedule, next_slot } => {
            let (slot, at) = schedule.next_after(*next_slot, now, bound)?;
            Next::Advance { slot, at }
        }
    };
    Ok(FirePlan { fire_at, next })
}
