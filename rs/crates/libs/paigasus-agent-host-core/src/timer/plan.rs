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
    /// The time that the event carries. It is always the stored `scheduled_at` of the timer, also after a back-off.
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
    let fire_at = timer.scheduled_at();
    let next = match timer.kind() {
        TimerKind::FollowUp => Next::Done,
        TimerKind::Schedule { schedule, next_slot } => {
            let (slot, at) = schedule.next_after(*next_slot, now, bound)?;
            Next::Advance { slot, at }
        }
    };
    Ok(FirePlan { fire_at, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InboxId, TimerId};
    use crate::timer::{BundledTzResolver, CronSchedule, Note, TimerOrigin, TimerParts, TimerStatus, WALK_BOUND};

    fn ts(raw: &str) -> Timestamp {
        raw.parse().unwrap()
    }

    fn timer(kind: TimerKind, scheduled_at: Timestamp, next_fire_at: Timestamp) -> Timer {
        Timer::new(TimerParts {
            id: TimerId::from_uuid(uuid::Uuid::from_u128(7)),
            inbox: InboxId::new(uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)),
            kind,
            thread: None,
            note: Note::parse("").unwrap(),
            origin: TimerOrigin::Agent,
            status: TimerStatus::Active,
            scheduled_at,
            next_fire_at,
        })
    }

    #[test]
    fn a_follow_up_after_a_back_off_fires_at_its_scheduled_instant() {
        let t = timer(TimerKind::FollowUp, ts("2026-03-29T01:00:00Z"), ts("2026-03-29T01:01:00Z"));
        let plan = plan_fire(&t, ts("2026-03-29T01:02:00Z"), WALK_BOUND).unwrap();
        assert_eq!(plan.fire_at, ts("2026-03-29T01:00:00Z"));
        assert_eq!(t.fire_event_id(plan.fire_at), "timer:00000000-0000-0000-0000-000000000007:2026-03-29T01:00:00Z");
    }

    #[test]
    fn a_schedule_after_a_back_off_fires_at_its_slot_and_advances_both_times() {
        let schedule = CronSchedule::parse("0 * * * *", "UTC", &BundledTzResolver::new()).unwrap();
        let (slot, at) = schedule.first_slot(ts("2026-03-29T00:30:00Z"), WALK_BOUND).unwrap();
        let t = timer(TimerKind::Schedule { schedule, next_slot: slot }, at, at + jiff::SignedDuration::from_secs(60));
        let now = at + jiff::SignedDuration::from_secs(61);
        let plan = plan_fire(&t, now, WALK_BOUND).unwrap();
        assert_eq!(plan.fire_at, at, "the event carries the slot and not the retry time");
        let Next::Advance { at: next_at, .. } = plan.next else { panic!("a schedule advances") };
        assert!(next_at > now);
    }

    #[test]
    fn the_constructor_keeps_the_due_time_at_or_after_the_scheduled_instant() {
        let t = timer(TimerKind::FollowUp, ts("2026-03-29T01:00:05.5Z"), ts("2026-03-29T01:00:00Z"));
        assert_eq!(t.scheduled_at(), ts("2026-03-29T01:00:06Z"));
        assert_eq!(t.next_fire_at(), ts("2026-03-29T01:00:06Z"));
    }
}
