// SPDX-License-Identifier: Apache-2.0

//! Timers: the kinds, the status, the note and the `Timer` value.

use jiff::Timestamp;

mod changes;
mod cron;
#[cfg(test)]
mod dst_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod plan;
mod text;
mod tz;
mod workspace;

pub use changes::{ApplyError, ApplyReport, CancelOutcome, TimerCaps, TimerChanges};
pub use cron::{CIVIL_GAP_WINDOW_DAYS, CronSchedule, CronScheduleError, CronSyntaxError, Resolution, WALK_BOUND, WalkError};
pub use plan::{FirePlan, Next, PlanError, plan_fire};
pub use text::{cancelled_text, follow_up_set_text, list_text};
pub use tz::BundledTzResolver;
pub use workspace::{FollowUpDue, FollowUpSet, ListedTimer, TimerDeps, TimerRefusal, TimerWorkspace};

use crate::ids::{InboxId, ThreadId, TimerId};

/// The most characters of a timer note.
pub const NOTE_MAX_CHARS: usize = 1000;

/// One civil time slot of a schedule. `second` is true for the second pass of a repeated hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The civil time of the slot.
    pub civil: jiff::civil::DateTime,
    /// True when the slot is in the second pass of a repeated civil time.
    pub second: bool,
}

/// The kind of a timer.
// The brief fixes the shape `Schedule { schedule: CronSchedule, .. }`. A timer is a rare value.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerKind {
    /// A timer that fires once.
    FollowUp,
    /// A timer that fires on a cron schedule.
    Schedule {
        /// The schedule.
        schedule: CronSchedule,
        /// The slot of the next firing.
        next_slot: Slot,
    },
}

impl TimerKind {
    /// The column value of the kind.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            TimerKind::FollowUp => "follow_up",
            TimerKind::Schedule { .. } => "schedule",
        }
    }
}

/// The status of a timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerStatus {
    /// The timer waits to fire.
    Active,
    /// The timer fired and has no next firing.
    Done,
    /// A cancel ended the timer.
    Cancelled,
    /// The timer cannot fire again.
    Failed,
}

impl TimerStatus {
    /// The column value of the status.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TimerStatus::Active => "active",
            TimerStatus::Done => "done",
            TimerStatus::Cancelled => "cancelled",
            TimerStatus::Failed => "failed",
        }
    }

    /// Read a column value. An unknown value gives `None`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(TimerStatus::Active),
            "done" => Some(TimerStatus::Done),
            "cancelled" => Some(TimerStatus::Cancelled),
            "failed" => Some(TimerStatus::Failed),
            _ => None,
        }
    }
}

/// Who made a timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerOrigin {
    /// The agent made the timer with a tool.
    Agent,
    /// The system made the timer.
    System,
}

impl TimerOrigin {
    /// The column value of the origin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TimerOrigin::Agent => "agent",
            TimerOrigin::System => "system",
        }
    }

    /// Read a column value. An unknown value gives `None`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "agent" => Some(TimerOrigin::Agent),
            "system" => Some(TimerOrigin::System),
            _ => None,
        }
    }
}

/// The error of a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoteError {
    /// The note has more than `NOTE_MAX_CHARS` characters.
    #[error("the note is too long: {chars} characters")]
    TooLong {
        /// The number of characters.
        chars: usize,
    },
    /// The note holds a NUL character.
    #[error("the note holds a NUL character")]
    Nul,
}

/// The note of a timer. It is model text and is not trusted. Do not log it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note(String);

impl Note {
    /// Check a note. An empty note is valid.
    pub fn parse(raw: &str) -> Result<Note, NoteError> {
        let chars = raw.chars().count();
        if chars > NOTE_MAX_CHARS {
            return Err(NoteError::TooLong { chars });
        }
        if raw.contains('\0') {
            return Err(NoteError::Nul);
        }
        Ok(Note(raw.to_string()))
    }

    /// The text of the note.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The parts of a timer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerParts {
    /// The ID of the timer.
    pub id: TimerId,
    /// The inbox of the timer.
    pub inbox: InboxId,
    /// The kind of the timer.
    pub kind: TimerKind,
    /// The thread that the timer fires into.
    pub thread: Option<ThreadId>,
    /// The note of the timer.
    pub note: Note,
    /// Who made the timer.
    pub origin: TimerOrigin,
    /// The status of the timer.
    pub status: TimerStatus,
    /// The time of the next firing.
    pub next_fire_at: Timestamp,
}

/// A timer of an inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timer {
    id: TimerId,
    inbox: InboxId,
    kind: TimerKind,
    thread: Option<ThreadId>,
    note: Note,
    origin: TimerOrigin,
    status: TimerStatus,
    next_fire_at: Timestamp,
}

/// Round a time up to a whole second.
#[must_use]
pub fn round_up_to_second(t: Timestamp) -> Timestamp {
    t.round(jiff::TimestampRound::new().smallest(jiff::Unit::Second).mode(jiff::RoundMode::Ceil))
        .expect("rounding a timestamp up to a second stays in range")
}

/// Format a firing time as `2026-03-29T01:00:00Z`.
#[must_use]
pub fn format_fire_at(t: Timestamp) -> String {
    t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

impl Timer {
    /// Build a timer. It rounds `next_fire_at` up to a whole second.
    #[must_use]
    pub fn new(parts: TimerParts) -> Timer {
        Timer {
            id: parts.id,
            inbox: parts.inbox,
            kind: parts.kind,
            thread: parts.thread,
            note: parts.note,
            origin: parts.origin,
            status: parts.status,
            next_fire_at: round_up_to_second(parts.next_fire_at),
        }
    }

    /// The ID of the timer.
    #[must_use]
    pub fn id(&self) -> TimerId {
        self.id
    }

    /// The inbox of the timer.
    #[must_use]
    pub fn inbox(&self) -> &InboxId {
        &self.inbox
    }

    /// The kind of the timer.
    #[must_use]
    pub fn kind(&self) -> &TimerKind {
        &self.kind
    }

    /// The thread that the timer fires into.
    #[must_use]
    pub fn thread(&self) -> Option<ThreadId> {
        self.thread
    }

    /// The note of the timer.
    #[must_use]
    pub fn note(&self) -> &Note {
        &self.note
    }

    /// Who made the timer.
    #[must_use]
    pub fn origin(&self) -> TimerOrigin {
        self.origin
    }

    /// The status of the timer.
    #[must_use]
    pub fn status(&self) -> TimerStatus {
        self.status
    }

    /// The time of the next firing.
    #[must_use]
    pub fn next_fire_at(&self) -> Timestamp {
        self.next_fire_at
    }

    /// The identity of the event that one firing of the timer makes.
    /// The same firing always gives the same identity.
    #[must_use]
    pub fn fire_event_id(&self, fire_at: Timestamp) -> String {
        format!("timer:{}:{}", self.id.uuid(), format_fire_at(fire_at))
    }
}

/// A timer with its firing history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerView {
    /// The timer.
    pub timer: Timer,
    /// How many times the timer fired.
    pub fire_count: u64,
    /// The time of the last firing.
    pub last_fired_at: Option<Timestamp>,
    /// The time the timer was made.
    pub created_at: Timestamp,
}
