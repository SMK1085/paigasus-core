// SPDX-License-Identifier: Apache-2.0

//! The run buffer of the timer tools: follow-ups, list and cancel. A check reads the store fresh at
//! each call and adds the changes that the run buffered. `seal` gives the changes to the run commit.

use std::sync::Arc;
use std::time::Duration;

use jiff::civil;
use jiff::{SignedDuration, Timestamp};

use super::text::refusal_text;
use super::tool::TimerToolName;
use super::{
    CIVIL_GAP_WINDOW_DAYS, CronSchedule, CronScheduleError, CronSyntaxError, Note, NoteError, Timer, TimerCaps, TimerChanges, TimerKind, TimerOrigin, TimerParts, TimerStatus, WALK_BOUND,
    checked_round_up_to_second,
};
use crate::ids::{InboxId, ThreadId, TimerId};
use crate::ports::{IdSource, RepositoryError, RunClock, TimerRepository, TzResolver};

const SECS_PER_DAY: i64 = 86_400;

/// The ports that a timer workspace needs.
#[derive(Clone)]
pub struct TimerDeps {
    /// The read side of the timer store.
    pub store: Arc<dyn TimerRepository>,
    /// The time zone resolver.
    pub tz: Arc<dyn TzResolver>,
    /// The source of new IDs.
    pub ids: Arc<dyn IdSource>,
    /// The clock of the run.
    pub clock: Arc<dyn RunClock>,
}

/// The due time of a follow-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUpDue {
    /// An absolute time.
    At(Timestamp),
    /// A number of seconds after the time of the call.
    InSeconds(i64),
}

/// A follow-up that the workspace buffered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FollowUpSet {
    /// The ID of the new timer.
    pub id: TimerId,
    /// The due time, rounded up to a whole second.
    pub due: Timestamp,
    /// True when the due time is later than the life of the conversation.
    pub beyond_thread_ttl: bool,
}

/// A schedule that the workspace buffered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleSet {
    /// The ID of the new timer.
    pub id: TimerId,
    /// The schedule.
    pub schedule: CronSchedule,
    /// The next three slots: the civil time in the zone of the schedule, and the instant.
    pub next: Vec<(civil::DateTime, Timestamp)>,
}

/// An active timer for a list. `buffered` is true for a timer that this run made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedTimer {
    /// The timer.
    pub timer: Timer,
    /// True when the run buffered the timer and the store does not hold it yet.
    pub buffered: bool,
}

/// A refusal of a workspace command. The `Display` text is for the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerRefusal {
    /// The workspace is sealed.
    Sealed,
    /// The due time is not after the time now.
    NotInFuture {
        /// The due time.
        due: Timestamp,
        /// The time now.
        now: Timestamp,
    },
    /// The delay is shorter than the minimum.
    TooSoon {
        /// The delay in seconds.
        secs: i64,
        /// The minimum in seconds.
        min: u64,
    },
    /// The delay is longer than the horizon.
    TooLate {
        /// The delay in whole days.
        days: i64,
        /// The maximum in days.
        max: u32,
    },
    /// The note is not valid.
    Note(NoteError),
    /// The inbox would have too many active timers.
    CountCap {
        /// The number of active timers after the change.
        after: u32,
        /// The limit.
        limit: u32,
    },
    /// The agent would make too many timers in 24 hours.
    Budget {
        /// The number of timers made in 24 hours, with the new one.
        after: u32,
        /// The limit.
        limit: u32,
    },
    /// No timer of the agent has this ID in this inbox.
    UnknownTimer {
        /// The text that the model gave.
        id: String,
    },
    /// The timer is not active.
    NotActive {
        /// The timer.
        id: TimerId,
        /// The status of the timer.
        status: TimerStatus,
    },
    /// The cron text is not accepted.
    Cron {
        /// The cron text that the model gave.
        cron: String,
        /// The reason.
        error: CronSyntaxError,
    },
    /// The time zone name is not known.
    UnknownZone {
        /// The name that the model gave.
        name: String,
    },
    /// Two slots of the schedule are closer than the minimum.
    IntervalTooShort {
        /// The cron text.
        cron: String,
        /// The gap in seconds.
        gap: u64,
        /// The minimum in seconds.
        min: u64,
    },
    /// The schedule has no slot in the horizon.
    NoSlot {
        /// The cron text.
        cron: String,
        /// The horizon in days.
        days: u32,
    },
    /// The arguments of a tool call are not valid.
    Arguments {
        /// The tool.
        tool: TimerToolName,
        /// What is wrong.
        detail: String,
    },
    /// The store failed.
    Store {
        /// The cause. It is for the log, not for the model.
        cause: String,
    },
}

impl TimerRefusal {
    /// The cause of a store failure, for the log. Other refusals have no cause.
    #[must_use]
    pub fn cause(&self) -> Option<&str> {
        match self {
            TimerRefusal::Store { cause } => Some(cause),
            _ => None,
        }
    }
}

impl std::fmt::Display for TimerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&refusal_text(self))
    }
}

impl std::error::Error for TimerRefusal {}

impl From<RepositoryError> for TimerRefusal {
    fn from(e: RepositoryError) -> Self {
        TimerRefusal::Store { cause: e.to_string() }
    }
}

impl From<NoteError> for TimerRefusal {
    fn from(e: NoteError) -> Self {
        TimerRefusal::Note(e)
    }
}

/// The timers of one run: a buffer of inserts and cancels over the store.
pub struct TimerWorkspace {
    inbox: InboxId,
    thread: ThreadId,
    caps: TimerCaps,
    thread_ttl: Duration,
    deps: TimerDeps,
    inserts: Vec<Timer>,
    cancels: Vec<(TimerId, TimerOrigin)>,
    sealed: bool,
}

impl TimerWorkspace {
    /// Make a workspace for one run.
    #[must_use]
    pub fn new(inbox: InboxId, thread: ThreadId, caps: TimerCaps, thread_ttl: Duration, deps: TimerDeps) -> Self {
        TimerWorkspace {
            inbox,
            thread,
            caps,
            thread_ttl,
            deps,
            inserts: Vec::new(),
            cancels: Vec::new(),
            sealed: false,
        }
    }

    /// True after `seal`.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Stop the workspace and give the buffered changes. Every later call gives `Sealed`.
    pub fn seal(&mut self) -> TimerChanges {
        self.sealed = true;
        TimerChanges {
            inserts: std::mem::take(&mut self.inserts),
            cancels: std::mem::take(&mut self.cancels),
        }
    }

    /// The life of a conversation.
    pub(super) fn thread_ttl(&self) -> Duration {
        self.thread_ttl
    }

    fn check_open(&self) -> Result<(), TimerRefusal> {
        if self.sealed { Err(TimerRefusal::Sealed) } else { Ok(()) }
    }

    /// The one check for a new timer: the count cap, then the daily budget. Every command that adds
    /// a timer calls it. The count is a fresh read of the active IDs, minus the buffered cancels
    /// that are in that set, plus the buffered inserts. A cancel of a timer that is not active now
    /// is not subtracted.
    async fn check_room_for_insert(&self, now: Timestamp) -> Result<(), TimerRefusal> {
        let ids = self.deps.store.active_agent_ids(&self.inbox).await?;
        let cancelled = self.cancels.iter().filter(|(id, _)| ids.contains(id)).count();
        let count = ids.len().saturating_sub(cancelled).saturating_add(self.inserts.len());
        let after = u32::try_from(count.saturating_add(1)).unwrap_or(u32::MAX);
        if after > self.caps.max_active_per_inbox {
            return Err(TimerRefusal::CountCap {
                after,
                limit: self.caps.max_active_per_inbox,
            });
        }
        let since = now.checked_sub(SignedDuration::from_hours(24)).unwrap_or(Timestamp::MIN);
        let created = self.deps.store.count_agent_created_since(&self.inbox, since).await?;
        let after = created.saturating_add(u32::try_from(self.inserts.len()).unwrap_or(u32::MAX)).saturating_add(1);
        if after > self.caps.max_agent_timers_created_per_day {
            return Err(TimerRefusal::Budget {
                after,
                limit: self.caps.max_agent_timers_created_per_day,
            });
        }
        Ok(())
    }

    /// Buffer a follow-up. The checks run in this order: the time is in the future, the delay is not
    /// too short, the delay is not too long, the note, the count cap, the budget.
    pub async fn schedule_follow_up(&mut self, due: FollowUpDue, note: &str, new_thread: bool) -> Result<FollowUpSet, TimerRefusal> {
        self.check_open()?;
        let now = self.deps.clock.now();
        let raw = match due {
            FollowUpDue::At(t) => t,
            FollowUpDue::InSeconds(n) => match now.checked_add(SignedDuration::from_secs(n)) {
                Ok(t) => t,
                Err(_) if n < 0 => return Err(TimerRefusal::NotInFuture { due: Timestamp::MIN, now }),
                Err(_) => {
                    return Err(TimerRefusal::TooLate {
                        days: n / SECS_PER_DAY,
                        max: self.caps.max_horizon_days,
                    });
                }
            },
        };
        let Some(due) = checked_round_up_to_second(raw) else {
            return Err(TimerRefusal::TooLate {
                days: raw.duration_since(now).as_secs() / SECS_PER_DAY,
                max: self.caps.max_horizon_days,
            });
        };
        if due <= now {
            return Err(TimerRefusal::NotInFuture { due, now });
        }
        let delta = due.duration_since(now);
        let min = SignedDuration::try_from(Duration::from_secs(self.caps.min_follow_up_delay_secs)).unwrap_or(SignedDuration::MAX);
        if delta < min {
            return Err(TimerRefusal::TooSoon {
                secs: delta.as_secs(),
                min: self.caps.min_follow_up_delay_secs,
            });
        }
        let horizon = SignedDuration::from_secs(i64::from(self.caps.max_horizon_days) * SECS_PER_DAY);
        if delta > horizon {
            return Err(TimerRefusal::TooLate {
                days: delta.as_secs() / SECS_PER_DAY,
                max: self.caps.max_horizon_days,
            });
        }
        let note = Note::parse(note)?;
        self.check_room_for_insert(now).await?;
        let beyond_thread_ttl = SignedDuration::try_from(self.thread_ttl).map(|ttl| delta > ttl).unwrap_or(false);
        let id = TimerId::from_uuid(self.deps.ids.new_uuid());
        self.inserts.push(Timer::new(TimerParts {
            id,
            inbox: self.inbox.clone(),
            kind: TimerKind::FollowUp,
            thread: if new_thread { None } else { Some(self.thread) },
            note,
            origin: TimerOrigin::Agent,
            status: TimerStatus::Active,
            next_fire_at: due,
        }));
        Ok(FollowUpSet { id, due, beyond_thread_ttl })
    }

    /// Buffer a schedule. The checks run in this order: the cron text and the zone, the gap between
    /// slots, the first slot inside the horizon, the note, the count cap, the budget. Any failure
    /// of a walk is a `NoSlot` refusal.
    pub async fn create_schedule(&mut self, cron: &str, tz: &str, note: &str) -> Result<ScheduleSet, TimerRefusal> {
        self.check_open()?;
        let now = self.deps.clock.now();
        let schedule = CronSchedule::parse(cron, tz, &*self.deps.tz).map_err(|e| match e {
            CronScheduleError::Syntax(error) => TimerRefusal::Cron { cron: cron.to_string(), error },
            CronScheduleError::UnknownZone { name } => TimerRefusal::UnknownZone { name },
        })?;
        let no_slot = || TimerRefusal::NoSlot {
            cron: cron.to_string(),
            days: self.caps.max_horizon_days,
        };
        let from = schedule.time_zone().to_datetime(now);
        if let Some(gap) = schedule.min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, self.caps.min_schedule_interval_secs).map_err(|_| no_slot())? {
            return Err(TimerRefusal::IntervalTooShort {
                cron: cron.to_string(),
                gap,
                min: self.caps.min_schedule_interval_secs,
            });
        }
        let (first, first_at) = schedule.first_slot(now, WALK_BOUND).map_err(|_| no_slot())?;
        let horizon = now
            .checked_add(SignedDuration::from_secs(i64::from(self.caps.max_horizon_days) * SECS_PER_DAY))
            .unwrap_or(Timestamp::MAX);
        if first_at > horizon {
            return Err(no_slot());
        }
        let note = Note::parse(note)?;
        self.check_room_for_insert(now).await?;
        let mut next = vec![(first.civil, first_at)];
        let (mut slot, mut at) = (first, first_at);
        for _ in 0..2 {
            let (s, t) = schedule.next_after(slot, at, WALK_BOUND).map_err(|_| no_slot())?;
            next.push((s.civil, t));
            (slot, at) = (s, t);
        }
        let id = TimerId::from_uuid(self.deps.ids.new_uuid());
        self.inserts.push(Timer::new(TimerParts {
            id,
            inbox: self.inbox.clone(),
            kind: TimerKind::Schedule {
                schedule: schedule.clone(),
                next_slot: first,
            },
            thread: None,
            note,
            origin: TimerOrigin::Agent,
            status: TimerStatus::Active,
            next_fire_at: first_at,
        }));
        Ok(ScheduleSet { id, schedule, next })
    }

    /// List the active timers of the agent: the stored ones without the buffered cancels, and the
    /// buffered inserts, ordered by the next firing.
    pub async fn list(&mut self) -> Result<Vec<ListedTimer>, TimerRefusal> {
        self.check_open()?;
        let stored = self.deps.store.list_active(&self.inbox, TimerOrigin::Agent).await?;
        let mut out: Vec<ListedTimer> = stored
            .into_iter()
            .filter(|v| !self.cancels.iter().any(|(id, _)| *id == v.timer.id()))
            .map(|v| ListedTimer { timer: v.timer, buffered: false })
            .collect();
        out.extend(self.inserts.iter().cloned().map(|timer| ListedTimer { timer, buffered: true }));
        out.sort_by_key(|l| l.timer.next_fire_at());
        Ok(out)
    }

    /// Cancel a timer of the agent. A text that is not an ID, a timer of another inbox, an unknown
    /// timer and a timer of the system all give the same refusal.
    pub async fn cancel(&mut self, timer_id: &str) -> Result<TimerId, TimerRefusal> {
        self.check_open()?;
        let unknown = || TimerRefusal::UnknownTimer { id: timer_id.to_string() };
        let id = TimerId::parse(timer_id).map_err(|_| unknown())?;
        if let Some(pos) = self.inserts.iter().position(|t| t.id() == id) {
            self.inserts.remove(pos);
            return Ok(id);
        }
        let view = match self.deps.store.get(&self.inbox, id).await {
            Ok(view) => view.ok_or_else(unknown)?,
            Err(read_err) => {
                // A row that does not decode makes `get` fail. The agent must still be able to
                // cancel such an active timer, so the cancel works from the active agent IDs.
                let active = self.deps.store.active_agent_ids(&self.inbox).await.map_err(|_| read_err.clone())?;
                if !active.contains(&id) {
                    return Err(read_err.into());
                }
                if !self.cancels.iter().any(|(c, _)| *c == id) {
                    self.cancels.push((id, TimerOrigin::Agent));
                }
                return Ok(id);
            }
        };
        if view.timer.origin() != TimerOrigin::Agent {
            return Err(unknown());
        }
        if view.timer.status() != TimerStatus::Active {
            return Err(TimerRefusal::NotActive { id, status: view.timer.status() });
        }
        if !self.cancels.iter().any(|(c, _)| *c == id) {
            self.cancels.push((id, TimerOrigin::Agent));
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use uuid::Uuid;

    use super::super::fake::{FakeClock, InMemoryTimerRepository, SeqIds};
    use super::super::{BundledTzResolver, TimerToolName, TimerView, follow_up_set_text, list_text, run_tool, schedule_set_text};
    use super::*;
    use serde_json::{Value, json};

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn start() -> Timestamp {
        ts("2026-06-01T00:00:00Z")
    }

    fn inbox_x() -> InboxId {
        InboxId::new(Uuid::from_u128(1), Uuid::from_u128(2))
    }

    fn inbox_y() -> InboxId {
        InboxId::new(Uuid::from_u128(1), Uuid::from_u128(3))
    }

    fn thread() -> ThreadId {
        ThreadId::from_uuid(Uuid::from_u128(77))
    }

    struct Rig {
        store: InMemoryTimerRepository,
        clock: FakeClock,
    }

    fn rig_with(caps: TimerCaps, ttl: Duration) -> (TimerWorkspace, Rig) {
        let store = InMemoryTimerRepository::default();
        let clock = FakeClock::new(start());
        let deps = TimerDeps {
            store: Arc::new(store.clone()),
            tz: Arc::new(BundledTzResolver::new()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(clock.clone()),
        };
        (TimerWorkspace::new(inbox_x(), thread(), caps, ttl, deps), Rig { store, clock })
    }

    fn rig(caps: TimerCaps) -> (TimerWorkspace, Rig) {
        rig_with(caps, Duration::from_secs(86_400))
    }

    fn seed(rig: &Rig, inbox: InboxId, n: u128, origin: TimerOrigin, status: TimerStatus, created_at: Timestamp) -> TimerId {
        let id = TimerId::from_uuid(Uuid::from_u128(0x9000 + n));
        let timer = Timer::new(TimerParts {
            id,
            inbox,
            kind: TimerKind::FollowUp,
            thread: None,
            note: Note::parse("stored").unwrap(),
            origin,
            status,
            next_fire_at: start() + SignedDuration::from_secs(3600 * (n as i64 + 1)),
        });
        rig.store.seed(TimerView {
            timer,
            fire_count: 0,
            last_fired_at: None,
            created_at,
        });
        id
    }

    fn old() -> Timestamp {
        start() - SignedDuration::from_hours(100)
    }

    fn secs(n: i64) -> SignedDuration {
        SignedDuration::from_secs(n)
    }

    #[tokio::test]
    async fn follow_up_in_seconds_uses_the_call_time() {
        let (mut ws, rig) = rig(TimerCaps::default());
        rig.clock.advance(secs(900));
        let set = ws.schedule_follow_up(FollowUpDue::InSeconds(120), "n", false).await.unwrap();
        assert_eq!(set.due, start() + secs(1020));
    }

    #[tokio::test]
    async fn fraction_is_rounded_up() {
        let (mut ws, rig) = rig(TimerCaps::default());
        rig.clock.advance(SignedDuration::from_millis(500));
        let set = ws.schedule_follow_up(FollowUpDue::InSeconds(60), "n", false).await.unwrap();
        assert_eq!(set.due, start() + secs(61));
    }

    #[tokio::test]
    async fn follow_up_limits() {
        let caps = TimerCaps {
            min_follow_up_delay_secs: 60,
            max_horizon_days: 366,
            ..TimerCaps::default()
        };
        let (mut ws, _rig) = rig(caps);
        let now = start();
        let day = SignedDuration::from_hours(24);
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(59), "n", false).await, Err(TimerRefusal::TooSoon { secs: 59, min: 60 }));
        assert!(ws.schedule_follow_up(FollowUpDue::InSeconds(60), "n", false).await.is_ok());
        assert!(ws.schedule_follow_up(FollowUpDue::At(now + day * 366), "n", false).await.is_ok());
        assert_eq!(
            ws.schedule_follow_up(FollowUpDue::At(now + day * 366 + secs(1)), "n", false).await,
            Err(TimerRefusal::TooLate { days: 366, max: 366 })
        );
        assert_eq!(
            ws.schedule_follow_up(FollowUpDue::At(now - secs(1)), "n", false).await,
            Err(TimerRefusal::NotInFuture { due: now - secs(1), now })
        );
    }

    #[tokio::test]
    async fn in_seconds_edges() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        let now = start();
        for n in [-1, 0] {
            assert!(matches!(ws.schedule_follow_up(FollowUpDue::InSeconds(n), "n", false).await, Err(TimerRefusal::NotInFuture { .. })));
        }
        assert!(matches!(
            ws.schedule_follow_up(FollowUpDue::InSeconds(i64::MAX), "n", false).await,
            Err(TimerRefusal::TooLate { max: 366, .. })
        ));
        assert!(matches!(ws.schedule_follow_up(FollowUpDue::InSeconds(i64::MIN), "n", false).await, Err(TimerRefusal::NotInFuture { now: n, .. }) if n == now));
        // The last second before the maximum cannot be rounded up. It is a refusal, not a panic.
        assert!(matches!(
            ws.schedule_follow_up(FollowUpDue::At(Timestamp::MAX), "n", false).await,
            Err(TimerRefusal::TooLate { max: 366, .. })
        ));
        assert!(matches!(
            ws.schedule_follow_up(FollowUpDue::At(Timestamp::MAX - SignedDuration::from_millis(500)), "n", false).await,
            Err(TimerRefusal::TooLate { max: 366, .. })
        ));
        assert!(ws.seal().is_empty());
    }

    #[tokio::test]
    async fn note_is_checked() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        let long = "x".repeat(1001);
        assert_eq!(
            ws.schedule_follow_up(FollowUpDue::InSeconds(120), &long, false).await,
            Err(TimerRefusal::Note(NoteError::TooLong { chars: 1001 }))
        );
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a\0b", false).await, Err(TimerRefusal::Note(NoteError::Nul)));
    }

    #[tokio::test]
    async fn thread_rules() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a", false).await.unwrap();
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "b", true).await.unwrap();
        let changes = ws.seal();
        assert_eq!(changes.inserts[0].thread(), Some(thread()));
        assert_eq!(changes.inserts[1].thread(), None);
        assert_eq!(changes.inserts[0].origin(), TimerOrigin::Agent);
        assert_eq!(changes.inserts[0].status(), TimerStatus::Active);
        assert_eq!(changes.inserts[0].id(), TimerId::from_uuid(Uuid::from_u128(0x1000)));
    }

    #[tokio::test]
    async fn beyond_ttl_text() {
        let (mut ws, _rig) = rig_with(TimerCaps::default(), Duration::from_secs(86_400));
        let ttl = Duration::from_secs(86_400);
        let far = ws.schedule_follow_up(FollowUpDue::InSeconds(2 * 86_400), "n", false).await.unwrap();
        assert!(far.beyond_thread_ttl);
        let text = follow_up_set_text(&far, ttl);
        assert!(text.contains("will be gone"), "{text}");
        assert!(text.contains("more than 24 hours"), "{text}");
        let near = ws.schedule_follow_up(FollowUpDue::InSeconds(3600), "n", false).await.unwrap();
        assert!(!near.beyond_thread_ttl);
        let text = follow_up_set_text(&near, ttl);
        assert!(text.contains("the note must hold"), "{text}");
        assert!(!text.contains("will be gone"), "{text}");
        assert!(text.starts_with("Follow-up 00000000-0000-0000-0000-000000001001 is set for 2026-06-01T01:00:00Z (UTC)."), "{text}");
    }

    #[tokio::test]
    async fn count_cap_at_and_past_the_limit() {
        let caps = TimerCaps {
            max_active_per_inbox: 2,
            ..TimerCaps::default()
        };
        let (mut ws, _rig) = rig(caps);
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a", false).await.unwrap();
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "b", false).await.unwrap();
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await, Err(TimerRefusal::CountCap { after: 3, limit: 2 }));

        let (mut ws, rig) = rig(caps);
        let a = seed(&rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Active, old());
        seed(&rig, inbox_x(), 1, TimerOrigin::Agent, TimerStatus::Active, old());
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await, Err(TimerRefusal::CountCap { after: 3, limit: 2 }));
        ws.cancel(&a.uuid().to_string()).await.unwrap();
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await.unwrap();
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "d", false).await, Err(TimerRefusal::CountCap { after: 3, limit: 2 }));
    }

    #[tokio::test]
    async fn stale_cancel_is_not_subtracted() {
        let caps = TimerCaps {
            max_active_per_inbox: 2,
            ..TimerCaps::default()
        };
        let (mut ws, rig) = rig(caps);
        let a = seed(&rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Active, old());
        seed(&rig, inbox_x(), 1, TimerOrigin::Agent, TimerStatus::Active, old());
        ws.cancel(&a.uuid().to_string()).await.unwrap();
        rig.store.set_status(a, TimerStatus::Done);
        // The store now holds one active timer (B). The cancel of A is stale and frees no place.
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await.unwrap();
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "d", false).await, Err(TimerRefusal::CountCap { after: 3, limit: 2 }));
    }

    #[tokio::test]
    async fn budget_at_and_past_the_limit() {
        let caps = TimerCaps {
            max_agent_timers_created_per_day: 3,
            ..TimerCaps::default()
        };
        let (mut ws, rig) = rig(caps);
        seed(&rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Cancelled, start() - SignedDuration::from_hours(1));
        seed(&rig, inbox_x(), 1, TimerOrigin::Agent, TimerStatus::Done, start() - SignedDuration::from_hours(25));
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a", false).await.unwrap();
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "b", false).await.unwrap();
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await, Err(TimerRefusal::Budget { after: 4, limit: 3 }));
    }

    #[tokio::test]
    async fn cancel_isolation() {
        let (mut ws, rig) = rig(TimerCaps::default());
        let other = seed(&rig, inbox_y(), 0, TimerOrigin::Agent, TimerStatus::Active, old());
        let system = seed(&rig, inbox_x(), 1, TimerOrigin::System, TimerStatus::Active, old());
        let unknown = Uuid::from_u128(0xdead).to_string();
        for raw in [other.uuid().to_string(), unknown, "not-a-uuid".to_string(), system.uuid().to_string()] {
            let err = ws.cancel(&raw).await.unwrap_err();
            assert_eq!(err, TimerRefusal::UnknownTimer { id: raw.clone() });
            assert_eq!(err.to_string(), format!("Error: No timer {raw} exists in this inbox."));
        }
        assert!(ws.seal().is_empty());
    }

    #[tokio::test]
    async fn cancel_rules() {
        let (mut ws, rig) = rig(TimerCaps::default());
        let set = ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a", false).await.unwrap();
        assert_eq!(ws.cancel(&set.id.uuid().to_string()).await, Ok(set.id));
        let done = seed(&rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Done, old());
        let err = ws.cancel(&done.uuid().to_string()).await.unwrap_err();
        assert_eq!(err, TimerRefusal::NotActive { id: done, status: TimerStatus::Done });
        assert_eq!(err.to_string(), format!("Error: Timer {} is done, so it cannot be cancelled.", done.uuid()));
        let live = seed(&rig, inbox_x(), 1, TimerOrigin::Agent, TimerStatus::Active, old());
        ws.cancel(&live.uuid().to_string()).await.unwrap();
        ws.cancel(&live.uuid().to_string()).await.unwrap();
        let changes = ws.seal();
        assert!(changes.inserts.is_empty());
        assert_eq!(changes.cancels, vec![(live, TimerOrigin::Agent)]);
    }

    #[tokio::test]
    async fn list_merges_the_buffer() {
        let (mut ws, rig) = rig(TimerCaps::default());
        assert_eq!(list_text(&ws.list().await.unwrap()), "There are no active timers.");
        let a = seed(&rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Active, old());
        let b = seed(&rig, inbox_x(), 1, TimerOrigin::Agent, TimerStatus::Active, old());
        seed(&rig, inbox_x(), 2, TimerOrigin::System, TimerStatus::Active, old());
        ws.cancel(&a.uuid().to_string()).await.unwrap();
        let c = ws.schedule_follow_up(FollowUpDue::InSeconds(120), "mine", false).await.unwrap();
        let listed = ws.list().await.unwrap();
        let ids: Vec<(TimerId, bool)> = listed.iter().map(|l| (l.timer.id(), l.buffered)).collect();
        assert_eq!(ids, vec![(c.id, true), (b, false)]);
        let text = list_text(&listed);
        assert!(text.lines().next().unwrap().ends_with("Note: mine (set in this run)"), "{text}");
        assert!(text.contains("follow-up, due 2026-06-01T02:00:00Z (UTC). Note: stored"), "{text}");
    }

    #[tokio::test]
    async fn seal_blocks_every_call() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        assert!(!ws.is_sealed());
        ws.seal();
        assert!(ws.is_sealed());
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "n", false).await, Err(TimerRefusal::Sealed));
        assert_eq!(ws.list().await, Err(TimerRefusal::Sealed));
        assert_eq!(ws.cancel("x").await, Err(TimerRefusal::Sealed));
        assert_eq!(TimerRefusal::Sealed.to_string(), "The run is ending. Timers are read-only now.");
    }

    fn at_time(rig: &Rig, t: &str) {
        rig.clock.advance(ts(t).duration_since(rig.clock.now()));
    }

    async fn schedule_refusal(ws: &mut TimerWorkspace, cron: &str, tz: &str) -> TimerRefusal {
        ws.create_schedule(cron, tz, "n").await.unwrap_err()
    }

    #[tokio::test]
    async fn schedule_shows_three_slots() {
        let (mut ws, rig) = rig(TimerCaps::default());
        at_time(&rig, "2026-03-28T10:00:00Z");
        let set = ws.create_schedule("0 9 * * *", "Europe/Berlin", "daily").await.unwrap();
        let instants: Vec<Timestamp> = set.next.iter().map(|(_, t)| *t).collect();
        assert_eq!(instants, vec![ts("2026-03-29T07:00:00Z"), ts("2026-03-30T07:00:00Z"), ts("2026-03-31T07:00:00Z")]);
        let civils: Vec<civil::DateTime> = set.next.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            civils,
            vec![
                civil::date(2026, 3, 29).at(9, 0, 0, 0),
                civil::date(2026, 3, 30).at(9, 0, 0, 0),
                civil::date(2026, 3, 31).at(9, 0, 0, 0)
            ]
        );
        let text = schedule_set_text(&set);
        assert!(text.contains("Europe/Berlin") && text.contains("UTC"), "{text}");
        assert!(text.contains("- 2026-03-29 09:00 Europe/Berlin (2026-03-29 07:00 UTC)"), "{text}");
        let changes = ws.seal();
        let t = &changes.inserts[0];
        assert_eq!(t.next_fire_at(), ts("2026-03-29T07:00:00Z"));
        assert_eq!(t.thread(), None);
        assert!(matches!(t.kind(), TimerKind::Schedule { .. }));
    }

    #[tokio::test]
    async fn schedule_refusals() {
        let (mut ws, r1) = rig(TimerCaps::default());
        at_time(&r1, "2026-03-28T10:00:00Z");
        let e = schedule_refusal(&mut ws, "0 9 L * *", "Europe/Berlin").await;
        assert!(matches!(e, TimerRefusal::Cron { .. }));
        assert!(e.to_string().contains('L'), "{e}");
        assert_eq!(schedule_refusal(&mut ws, "0 9 * * *", "Mars/Olympus").await, TimerRefusal::UnknownZone { name: "Mars/Olympus".into() });
        assert_eq!(
            schedule_refusal(&mut ws, "*/14 * * * *", "Europe/Berlin").await,
            TimerRefusal::IntervalTooShort {
                cron: "*/14 * * * *".into(),
                gap: 840,
                min: 900
            }
        );
        assert_eq!(schedule_refusal(&mut ws, "0 0 30 2 *", "UTC").await, TimerRefusal::NoSlot { cron: "0 0 30 2 *".into(), days: 366 });
        assert_eq!(
            schedule_refusal(&mut ws, "0 0 30 2 *", "UTC").await.to_string(),
            "Error: \"0 0 30 2 *\" has no slot in the next 366 days."
        );
        assert!(ws.seal().is_empty());

        let caps = TimerCaps {
            min_schedule_interval_secs: 7200,
            ..TimerCaps::default()
        };
        let (mut ws, r2) = rig(caps);
        at_time(&r2, "2026-03-28T10:00:00Z");
        ws.create_schedule("0 */2 * * *", "Europe/Berlin", "n").await.unwrap();
        let caps = TimerCaps {
            min_schedule_interval_secs: 86_400,
            ..TimerCaps::default()
        };
        let (mut ws, r3) = rig(caps);
        at_time(&r3, "2026-03-28T10:00:00Z");
        ws.create_schedule("0 9 * * *", "Europe/Berlin", "n").await.unwrap();
    }

    #[tokio::test]
    async fn schedule_beyond_the_horizon_is_no_slot() {
        let caps = TimerCaps {
            max_horizon_days: 30,
            ..TimerCaps::default()
        };
        let (mut ws, _rig) = rig(caps);
        // The next 1 January is more than 30 days after the clock (2026-06-01).
        assert_eq!(schedule_refusal(&mut ws, "0 0 1 1 *", "UTC").await, TimerRefusal::NoSlot { cron: "0 0 1 1 *".into(), days: 30 });
    }

    #[tokio::test]
    async fn schedule_counts_against_cap_and_budget() {
        let caps = TimerCaps {
            max_active_per_inbox: 2,
            ..TimerCaps::default()
        };
        let (mut ws, store_rig) = rig(caps);
        seed(&store_rig, inbox_x(), 0, TimerOrigin::Agent, TimerStatus::Active, old());
        ws.create_schedule("0 9 * * *", "UTC", "a").await.unwrap();
        assert_eq!(schedule_refusal(&mut ws, "0 9 * * *", "UTC").await, TimerRefusal::CountCap { after: 3, limit: 2 });
        // A follow-up and a schedule share the same cap.
        assert_eq!(ws.schedule_follow_up(FollowUpDue::InSeconds(120), "c", false).await, Err(TimerRefusal::CountCap { after: 3, limit: 2 }));

        let caps = TimerCaps {
            max_agent_timers_created_per_day: 2,
            ..TimerCaps::default()
        };
        let (mut ws, _rig) = rig(caps);
        ws.schedule_follow_up(FollowUpDue::InSeconds(120), "a", false).await.unwrap();
        ws.create_schedule("0 9 * * *", "UTC", "b").await.unwrap();
        assert_eq!(schedule_refusal(&mut ws, "0 9 * * *", "UTC").await, TimerRefusal::Budget { after: 3, limit: 2 });
    }

    #[tokio::test]
    async fn at_forms() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        for at in ["2026-11-22T09:00", "2026-11-22"] {
            let reply = run_tool(&mut ws, TimerToolName::ScheduleFollowUp, json!({"at": at, "note": "n"})).await;
            assert!(reply.refused, "{at}");
            assert!(reply.text.starts_with("Error: at must be an RFC 3339 time with an offset"), "{}", reply.text);
            assert!(reply.text.contains("RFC 3339") && reply.text.contains("with an offset"));
        }
        let reply = run_tool(&mut ws, TimerToolName::ScheduleFollowUp, json!({"at": "2026-11-22T09:00:00.25Z", "note": "n"})).await;
        assert!(!reply.refused, "{}", reply.text);
        assert_eq!(ws.seal().inserts[0].next_fire_at(), ts("2026-11-22T09:00:01Z"));
    }

    #[tokio::test]
    async fn argument_types() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        let f = TimerToolName::ScheduleFollowUp;
        let cases = [
            (f, json!({"at": "2026-11-22T09:00:00Z", "in_seconds": 60, "note": "n"})),
            (f, json!({"note": "n"})),
            (f, json!({"in_seconds": 60.5, "note": "n"})),
            (f, json!({"in_seconds": "60", "note": "n"})),
            (f, json!({"in_seconds": 60})),
            (f, json!({"in_seconds": 60, "note": "n", "new_thread": "yes"})),
            (f, json!({"at": 5, "note": "n"})),
            (f, json!("not an object")),
            (TimerToolName::CreateSchedule, json!({"cron": "0 9 * * *", "tz": "UTC"})),
            (TimerToolName::CreateSchedule, json!({"cron": 5, "tz": "UTC", "note": "n"})),
            (TimerToolName::CancelTimer, json!({"timer_id": 5})),
            (TimerToolName::CancelTimer, json!({})),
        ];
        for (tool, args) in cases {
            let reply = run_tool(&mut ws, tool, args.clone()).await;
            assert!(reply.refused, "{args}");
            assert!(reply.text.starts_with("Error:"), "{}", reply.text);
            assert!(reply.text.contains(&format!("{} takes:", tool.as_str())), "{}", reply.text);
            assert_eq!(reply.cause, None);
        }
        assert!(ws.seal().is_empty());
    }

    #[tokio::test]
    async fn a_null_argument_is_an_absent_argument() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        let f = TimerToolName::ScheduleFollowUp;
        let reply = run_tool(&mut ws, f, json!({"at": "2026-11-22T09:00:00Z", "in_seconds": null, "note": "n"})).await;
        assert!(!reply.refused, "{}", reply.text);
        let reply = run_tool(&mut ws, f, json!({"at": null, "in_seconds": 120, "note": "n", "new_thread": null})).await;
        assert!(!reply.refused, "{}", reply.text);
        // A null that stands for a missing required argument is still a refusal.
        let reply = run_tool(&mut ws, f, json!({"at": null, "in_seconds": null, "note": "n"})).await;
        assert!(reply.refused && reply.text.contains("give either at or in_seconds"), "{}", reply.text);
        let reply = run_tool(&mut ws, TimerToolName::CancelTimer, json!({"timer_id": null})).await;
        assert!(reply.refused && reply.text.contains("timer_id is missing"), "{}", reply.text);
    }

    #[tokio::test]
    async fn list_timers_accepts_null_and_empty_arguments() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        for args in [Value::Null, json!({})] {
            let reply = run_tool(&mut ws, TimerToolName::ListTimers, args.clone()).await;
            assert!(!reply.refused, "{args}: {}", reply.text);
        }
        // Another tool still needs an object.
        let reply = run_tool(&mut ws, TimerToolName::CancelTimer, Value::Null).await;
        assert!(reply.refused);
    }

    #[tokio::test]
    async fn every_tool_through_run_tool() {
        let (mut ws, _rig) = rig(TimerCaps::default());
        let set = run_tool(&mut ws, TimerToolName::ScheduleFollowUp, json!({"in_seconds": 120, "note": "n", "extra": 1})).await;
        assert!(!set.refused && set.text.starts_with("Follow-up "), "{}", set.text);
        let id = set.text.split_whitespace().nth(1).unwrap().to_string();
        let sched = run_tool(&mut ws, TimerToolName::CreateSchedule, json!({"cron": "0 9 * * *", "tz": "Europe/Berlin", "note": "n"})).await;
        assert!(!sched.refused && sched.text.starts_with("Schedule "), "{}", sched.text);
        let list = run_tool(&mut ws, TimerToolName::ListTimers, json!({})).await;
        assert!(!list.refused && list.text.lines().count() == 2, "{}", list.text);
        let cancel = run_tool(&mut ws, TimerToolName::CancelTimer, json!({"timer_id": id})).await;
        assert!(!cancel.refused && cancel.text.ends_with("is cancelled."), "{}", cancel.text);
        assert_eq!(TimerToolName::ALL.map(TimerToolName::as_str), ["schedule_follow_up", "create_schedule", "list_timers", "cancel_timer"]);
    }

    #[tokio::test]
    async fn run_tool_refusal_carries_the_store_cause() {
        let deps = TimerDeps {
            store: Arc::new(FailingStore),
            tz: Arc::new(BundledTzResolver::new()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FakeClock::new(start())),
        };
        let mut ws = TimerWorkspace::new(inbox_x(), thread(), TimerCaps::default(), Duration::from_secs(60), deps);
        let reply = run_tool(&mut ws, TimerToolName::ListTimers, json!({})).await;
        assert!(reply.refused && reply.cause.is_some_and(|c| c.contains("boom")));
        assert!(!reply.text.contains("boom"));
    }

    struct FailingStore;

    #[async_trait]
    impl TimerRepository for FailingStore {
        async fn list_active(&self, _: &InboxId, _: TimerOrigin) -> Result<Vec<TimerView>, RepositoryError> {
            Err(RepositoryError::Store("boom".into()))
        }
        async fn get(&self, _: &InboxId, _: TimerId) -> Result<Option<TimerView>, RepositoryError> {
            Err(RepositoryError::Store("boom".into()))
        }
        async fn active_agent_ids(&self, _: &InboxId) -> Result<Vec<TimerId>, RepositoryError> {
            Err(RepositoryError::Store("boom".into()))
        }
        async fn count_agent_created_since(&self, _: &InboxId, _: Timestamp) -> Result<u32, RepositoryError> {
            Err(RepositoryError::Store("boom".into()))
        }
    }

    /// A store whose `get` fails (a row that does not decode) while the active IDs read works.
    struct UndecodableStore {
        active: Vec<TimerId>,
    }

    #[async_trait]
    impl TimerRepository for UndecodableStore {
        async fn list_active(&self, _: &InboxId, _: TimerOrigin) -> Result<Vec<TimerView>, RepositoryError> {
            Ok(Vec::new())
        }
        async fn get(&self, _: &InboxId, _: TimerId) -> Result<Option<TimerView>, RepositoryError> {
            Err(RepositoryError::Store("the row does not decode".into()))
        }
        async fn active_agent_ids(&self, _: &InboxId) -> Result<Vec<TimerId>, RepositoryError> {
            Ok(self.active.clone())
        }
        async fn count_agent_created_since(&self, _: &InboxId, _: Timestamp) -> Result<u32, RepositoryError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn cancel_works_for_an_active_timer_whose_row_does_not_decode() {
        let known = TimerId::from_uuid(Uuid::from_u128(0xabc));
        let deps = TimerDeps {
            store: Arc::new(UndecodableStore { active: vec![known] }),
            tz: Arc::new(BundledTzResolver::new()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FakeClock::new(start())),
        };
        let mut ws = TimerWorkspace::new(inbox_x(), thread(), TimerCaps::default(), Duration::from_secs(60), deps);
        assert_eq!(ws.cancel(&known.uuid().to_string()).await, Ok(known));
        let other = Uuid::from_u128(0xdef).to_string();
        let r = ws.cancel(&other).await;
        assert!(matches!(r, Err(TimerRefusal::Store { .. })), "{r:?}");
        assert_eq!(ws.seal().cancels, vec![(known, TimerOrigin::Agent)]);
    }

    #[tokio::test]
    async fn store_error_is_a_refusal_with_a_cause() {
        let deps = TimerDeps {
            store: Arc::new(FailingStore),
            tz: Arc::new(BundledTzResolver::new()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FakeClock::new(start())),
        };
        let mut ws = TimerWorkspace::new(inbox_x(), thread(), TimerCaps::default(), Duration::from_secs(60), deps);
        let errs = [
            ws.schedule_follow_up(FollowUpDue::InSeconds(120), "n", false).await.unwrap_err(),
            ws.list().await.unwrap_err(),
            ws.cancel(&Uuid::from_u128(5).to_string()).await.unwrap_err(),
        ];
        for e in errs {
            assert_eq!(e.to_string(), "The timer store is not available now. Try again later.");
            assert!(e.cause().is_some_and(|c| c.contains("boom")));
            assert!(!e.to_string().contains("boom"));
        }
        assert_eq!(TimerRefusal::Sealed.cause(), None);
    }
}
