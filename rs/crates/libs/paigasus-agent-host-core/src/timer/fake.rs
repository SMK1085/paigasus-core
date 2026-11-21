// SPDX-License-Identifier: Apache-2.0

//! In-memory fakes for the timer ports and the clock, ID and zone ports. For tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

use super::{TimerOrigin, TimerStatus, TimerView};
use crate::ids::{InboxId, TimerId};
use crate::ports::{IdSource, RepositoryError, RunClock, TimerRepository, TzError, TzResolver};

/// An in-memory `TimerRepository`. A clone shares the same data.
#[derive(Clone, Default)]
pub struct InMemoryTimerRepository {
    views: Arc<Mutex<Vec<TimerView>>>,
}

impl InMemoryTimerRepository {
    /// Add a view. A view with the same timer ID replaces the old one.
    pub fn seed(&self, view: TimerView) {
        let mut views = self.views.lock().unwrap();
        views.retain(|v| v.timer.id() != view.timer.id());
        views.push(view);
    }

    /// Change the status of a stored timer. An unknown ID changes nothing.
    pub fn set_status(&self, id: TimerId, status: TimerStatus) {
        let mut views = self.views.lock().unwrap();
        for v in views.iter_mut().filter(|v| v.timer.id() == id) {
            let parts = super::TimerParts {
                id,
                inbox: v.timer.inbox().clone(),
                kind: v.timer.kind().clone(),
                thread: v.timer.thread(),
                note: v.timer.note().clone(),
                origin: v.timer.origin(),
                status,
                scheduled_at: v.timer.scheduled_at(),
                next_fire_at: v.timer.next_fire_at(),
            };
            v.timer = super::Timer::new(parts);
        }
    }
}

#[async_trait]
impl TimerRepository for InMemoryTimerRepository {
    async fn list_active(&self, inbox: &InboxId, origin: TimerOrigin) -> Result<Vec<TimerView>, RepositoryError> {
        let views = self.views.lock().unwrap();
        let mut out: Vec<TimerView> = views
            .iter()
            .filter(|v| v.timer.inbox() == inbox && v.timer.origin() == origin && v.timer.status() == TimerStatus::Active)
            .cloned()
            .collect();
        out.sort_by_key(|v| v.timer.next_fire_at());
        Ok(out)
    }

    async fn get(&self, inbox: &InboxId, id: TimerId) -> Result<Option<TimerView>, RepositoryError> {
        let views = self.views.lock().unwrap();
        Ok(views.iter().find(|v| v.timer.inbox() == inbox && v.timer.id() == id).cloned())
    }

    async fn active_agent_ids(&self, inbox: &InboxId) -> Result<Vec<TimerId>, RepositoryError> {
        let views = self.views.lock().unwrap();
        Ok(views
            .iter()
            .filter(|v| v.timer.inbox() == inbox && v.timer.origin() == TimerOrigin::Agent && v.timer.status() == TimerStatus::Active)
            .map(|v| v.timer.id())
            .collect())
    }

    async fn count_agent_created_since(&self, inbox: &InboxId, since: Timestamp) -> Result<u32, RepositoryError> {
        let views = self.views.lock().unwrap();
        let n = views
            .iter()
            .filter(|v| v.timer.inbox() == inbox && v.timer.origin() == TimerOrigin::Agent && v.created_at > since)
            .count();
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

/// A clock that only moves when a test moves it.
#[derive(Clone)]
pub struct FakeClock {
    now: Arc<Mutex<Timestamp>>,
}

impl FakeClock {
    /// Make a clock at `t`.
    #[must_use]
    pub fn new(t: Timestamp) -> Self {
        FakeClock { now: Arc::new(Mutex::new(t)) }
    }

    /// Move the clock forward by `d`.
    pub fn advance(&self, d: SignedDuration) {
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(d).expect("the fake clock stays in range");
    }
}

impl RunClock for FakeClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }
}

/// An ID source with a known sequence. The UUID number `n` is `0x1000 + n`, and `n` starts at 0.
#[derive(Default)]
pub struct SeqIds {
    next: AtomicU64,
}

impl IdSource for SeqIds {
    fn new_uuid(&self) -> Uuid {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        Uuid::from_u128(0x1000 + u128::from(n))
    }
}

/// A zone resolver over a fixed list of zones.
#[derive(Clone, Debug)]
pub struct MapTzResolver {
    zones: HashMap<String, TimeZone>,
}

impl MapTzResolver {
    /// Make a resolver that knows only `entries`.
    #[must_use]
    pub fn new(entries: &[(&str, TimeZone)]) -> Self {
        MapTzResolver {
            zones: entries.iter().map(|(n, z)| ((*n).to_string(), z.clone())).collect(),
        }
    }
}

impl TzResolver for MapTzResolver {
    fn resolve(&self, name: &str) -> Result<TimeZone, TzError> {
        self.zones.get(name).cloned().ok_or_else(|| TzError::Unknown { name: name.to_string() })
    }
}
