// SPDX-License-Identifier: Apache-2.0

//! The run clock of the timer tools: the database time at the start of the run, plus the
//! monotonic time since then. It never reads the wall clock of the host (spec §10.1).

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use paigasus_agent_host_core::ports::RunClock;
use tokio::time::Instant;

/// The time of the run. The database `now` at the start of the run is the base.
#[derive(Clone, Copy, Debug)]
pub struct RunStartClock {
    db_start: Timestamp,
    started: Instant,
}

impl RunStartClock {
    /// Start the clock. `db_start` is the database time read at the start of the run.
    #[must_use]
    pub fn new(db_start: Timestamp) -> Self {
        RunStartClock { db_start, started: Instant::now() }
    }
}

impl RunClock for RunStartClock {
    fn now(&self) -> Timestamp {
        let elapsed: Duration = self.started.elapsed();
        SignedDuration::try_from(elapsed).ok().and_then(|d| self.db_start.checked_add(d).ok()).unwrap_or(self.db_start)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jiff::Timestamp;
    use paigasus_agent_host_core::ports::RunClock;

    use super::RunStartClock;

    #[tokio::test(start_paused = true)]
    async fn run_start_clock_moves_with_the_run() {
        let start: Timestamp = "2026-06-01T00:00:00Z".parse().unwrap();
        let clock = RunStartClock::new(start);
        assert_eq!(clock.now(), start);
        tokio::time::advance(Duration::from_secs(900)).await;
        let expected: Timestamp = "2026-06-01T00:15:00Z".parse().unwrap();
        assert_eq!(clock.now(), expected);
    }
}
