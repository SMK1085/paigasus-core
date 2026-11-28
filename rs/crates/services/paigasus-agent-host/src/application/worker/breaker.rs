// SPDX-License-Identifier: Apache-2.0

//! The pause of the worker pool after host faults in a row (SMA-851, part F).
//!
//! A host fault is a fault of the host or of the gateway link, not of the event. When many runs
//! end that way in a row, a claim only burns a run on a link that is down. The breaker counts the
//! faults, starts a pause when the count reaches the limit and doubles the pause for each further
//! fault, up to `HOST_FAULT_PAUSE_CAP`. A completed or stopped run resets it.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use paigasus_observability::names;
use tokio::time::Instant;

/// The longest pause.
pub const HOST_FAULT_PAUSE_CAP: Duration = Duration::from_secs(600);

/// The settings of the breaker. The service config makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFaultSettings {
    /// The number of host faults in a row at which the first pause starts.
    pub pause_after: u32,
    /// The length of the first pause. Each further fault doubles it.
    pub pause: Duration,
}

#[derive(Debug, Default)]
struct State {
    /// The host faults since the last completed or stopped run.
    in_row: u32,
    /// The end of the current pause.
    until: Option<Instant>,
    /// The start of the current or the last pause. A run that started before it is stale.
    pause_started: Option<Instant>,
}

/// The count of host faults in a row and the pause that follows it.
#[derive(Debug)]
pub struct HostFaultBreaker {
    settings: HostFaultSettings,
    state: Mutex<State>,
}

impl HostFaultBreaker {
    #[must_use]
    pub fn new(settings: HostFaultSettings) -> Self {
        // The series exists from boot, so a dashboard shows 0 and not "no data".
        metrics::gauge!(names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED).set(0.0);
        Self {
            settings,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Count one host fault of a run that started at `run_started`. When the count reaches
    /// `pause_after`, start a pause and return its length. Each further fault doubles the pause,
    /// up to `HOST_FAULT_PAUSE_CAP`. A fault of a run that started before the current (or last)
    /// pause does not count: the run was in flight when the pause began, and it must not double
    /// the pause.
    pub fn record_host_fault(&self, run_started: Instant, now: Instant) -> Option<Duration> {
        let mut state = self.state();
        if state.pause_started.is_some_and(|start| run_started < start) {
            return None;
        }
        state.in_row = state.in_row.saturating_add(1);
        let over = state.in_row.checked_sub(self.settings.pause_after)?;
        let factor = 1u32.checked_shl(over).unwrap_or(u32::MAX);
        let pause = self.settings.pause.saturating_mul(factor).min(HOST_FAULT_PAUSE_CAP);
        state.until = Some(now + pause);
        state.pause_started = Some(now);
        metrics::gauge!(names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED).set(1.0);
        tracing::warn!(pause_secs = pause.as_secs(), in_row = state.in_row, "the worker pool pauses after host faults in a row");
        Some(pause)
    }

    /// A completed or stopped run: the count goes to 0 and a pause ends.
    pub fn record_completed(&self) {
        let mut state = self.state();
        state.in_row = 0;
        if state.until.take().is_some() {
            metrics::gauge!(names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED).set(0.0);
        }
    }

    /// The rest of the current pause, or `None`. It sets the gauge to 0 when a pause is over.
    pub fn pause_left(&self, now: Instant) -> Option<Duration> {
        let mut state = self.state();
        let until = state.until?;
        if let Some(left) = until.checked_duration_since(now).filter(|d| !d.is_zero()) {
            return Some(left);
        }
        state.until = None;
        metrics::gauge!(names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED).set(0.0);
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_observability::names;
    use tokio::time::Instant;

    use super::{HostFaultBreaker, HostFaultSettings};
    use crate::application::worker::test_fakes::gauge;

    fn breaker() -> HostFaultBreaker {
        HostFaultBreaker::new(HostFaultSettings {
            pause_after: 3,
            pause: Duration::from_secs(30),
        })
    }

    fn secs(n: u64) -> Option<Duration> {
        Some(Duration::from_secs(n))
    }

    #[tokio::test(start_paused = true)]
    async fn three_host_faults_start_a_pause_of_the_base() {
        let b = breaker();
        let now = Instant::now();
        assert_eq!(b.record_host_fault(now, now), None);
        assert_eq!(b.record_host_fault(now, now), None);
        assert_eq!(b.record_host_fault(now, now), secs(30));
        assert_eq!(b.pause_left(now), secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn each_further_host_fault_doubles_the_pause_to_the_cap() {
        let b = breaker();
        let now = Instant::now();
        let got: Vec<Option<Duration>> = (0..9).map(|_| b.record_host_fault(now, now)).collect();
        assert_eq!(&got[..2], &[None, None]);
        assert_eq!(&got[2..], &[secs(30), secs(60), secs(120), secs(240), secs(480), secs(600), secs(600)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_completed_run_resets_the_count_and_ends_the_pause() {
        let b = breaker();
        let now = Instant::now();
        for _ in 0..3 {
            b.record_host_fault(now, now);
        }
        assert!(b.pause_left(now).is_some());
        b.record_completed();
        assert_eq!(b.pause_left(now), None);
        // The runs below start at or after the old pause began, so they count.
        assert_eq!(b.record_host_fault(now, now), None);
        assert_eq!(b.record_host_fault(now, now), None);
        assert_eq!(b.record_host_fault(now, now), secs(30));
    }

    #[test]
    fn the_pause_gauge_is_zero_from_boot() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let _b = breaker();
        assert_eq!(gauge(&recorder, names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED), Some(0.0));
    }

    #[tokio::test(start_paused = true)]
    async fn the_pause_ends_after_its_length() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let b = breaker();
        let now = Instant::now();
        for _ in 0..3 {
            b.record_host_fault(now, now);
        }
        assert_eq!(gauge(&recorder, names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED), Some(1.0));
        assert_eq!(b.pause_left(now + Duration::from_secs(29)), secs(1));
        assert_eq!(b.pause_left(now + Duration::from_secs(30)), None);
        assert_eq!(gauge(&recorder, names::AGENT_HOST_WORKER_HOST_FAULT_PAUSED), Some(0.0));
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_fault_of_a_run_that_started_before_the_pause_does_not_double_it() {
        let b = breaker();
        let t0 = Instant::now();
        // Runs that started at t0 are in flight. The pause starts 5 s later.
        let pause_start = t0 + Duration::from_secs(5);
        for _ in 0..3 {
            b.record_host_fault(pause_start, pause_start);
        }
        assert_eq!(b.pause_left(pause_start), secs(30));
        // The runs that started at t0 end with a host fault after the pause began.
        let later = pause_start + Duration::from_secs(1);
        for _ in 0..10 {
            assert_eq!(b.record_host_fault(t0, later), None);
        }
        assert_eq!(b.pause_left(later), secs(29), "the pause is not longer");
        // A run that started after the pause began counts and doubles it.
        assert_eq!(b.record_host_fault(later, later), secs(60));
    }
}
