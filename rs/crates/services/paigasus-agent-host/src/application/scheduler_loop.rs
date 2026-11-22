// SPDX-License-Identifier: Apache-2.0

//! The scheduler loop (spec §11.3). It calls `Scheduler::fire_due(1)` row by row, so that the
//! shutdown watch is read between two rows. The loop never puts the shutdown arm of a `select!`
//! against a running `fire_due`: a fire that is cut short can drop an open transaction. A fire
//! finishes or rolls back before the loop reads the watch again.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use paigasus_agent_host_core::ports::{RepositoryError, Scheduler};
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

/// The settings of the loop. `TimersConfig::loop_settings` makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopSettings {
    /// The time between two passes.
    pub poll_interval: Duration,
    /// The most rows that one pass processes.
    pub pass_limit: u32,
}

/// Fires up to `pass_limit` rows, one `fire_due(1)` call for each row. It returns the number of
/// rows that the calls processed. A call that gives 0 ends the pass. The pass stops before a
/// call when the shutdown watch has a new value, or when its sender is gone.
///
/// # Errors
///
/// The first error of `fire_due`. The rows that the earlier calls processed stay committed.
pub async fn run_pass(scheduler: &dyn Scheduler, pass_limit: u32, shutdown: &watch::Receiver<()>) -> Result<u32, RepositoryError> {
    let mut processed = 0;
    while processed < pass_limit {
        if shutdown.has_changed().unwrap_or(true) {
            break;
        }
        if scheduler.fire_due(1).await? == 0 {
            break;
        }
        processed += 1;
    }
    Ok(processed)
}

/// Runs a pass at each tick until the shutdown watch changes. A pass that processes exactly
/// `pass_limit` rows is full, and the next pass starts at once. After each pass the loop records
/// the duration, the lag and the Unix time of the end. The scheduler counts its own errors, so
/// the loop only logs one and waits for the next tick.
///
/// # Errors
///
/// None. The task must not end on an error, because `main` treats an ended task as fatal. It
/// returns `Ok(())` only on shutdown.
pub async fn run_scheduler_loop(scheduler: Arc<dyn Scheduler>, settings: LoopSettings, mut shutdown: watch::Receiver<()>) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(settings.poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.changed() => return Ok(()),
        }
        loop {
            let started = Instant::now();
            let result = run_pass(&*scheduler, settings.pass_limit, &shutdown).await;
            metrics::histogram!(names::AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
            // Read the watch before the lag read: a shutdown must not wait for a database call.
            if shutdown.has_changed().unwrap_or(true) {
                return Ok(());
            }
            match scheduler.due_lag().await {
                Ok(lag) => metrics::gauge!(names::AGENT_HOST_SCHEDULER_LAG_SECONDS).set(lag.as_secs_f64()),
                Err(error) => tracing::warn!(%error, "the scheduler could not read the lag"),
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            metrics::gauge!(names::AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS).set(now);
            match result {
                Ok(processed) if processed >= settings.pass_limit => {}
                Ok(_) => break,
                Err(error) => {
                    tracing::warn!(%error, "a scheduler pass failed; the next tick tries again");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_agent_host_core::ports::{RepositoryError, Scheduler};
    use paigasus_observability::names;
    use tokio::sync::watch;

    use super::{LoopSettings, run_pass, run_scheduler_loop};

    /// A scheduler that returns scripted answers. After the script it answers 0.
    struct Fake {
        script: Mutex<VecDeque<Result<usize, RepositoryError>>>,
        calls: Mutex<Vec<tokio::time::Instant>>,
        /// The count of `due_lag` calls.
        lag_reads: Mutex<usize>,
        /// Sent inside the call with this index (1-based).
        shutdown_at: Option<(usize, watch::Sender<()>)>,
    }

    impl Fake {
        fn new(script: Vec<Result<usize, RepositoryError>>) -> Arc<Self> {
            Arc::new(Fake {
                script: Mutex::new(script.into()),
                calls: Mutex::new(Vec::new()),
                lag_reads: Mutex::new(0),
                shutdown_at: None,
            })
        }
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Scheduler for Fake {
        async fn fire_due(&self, limit: u32) -> Result<usize, RepositoryError> {
            assert_eq!(limit, 1, "the loop fires one row per call");
            let index = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(tokio::time::Instant::now());
                calls.len()
            };
            if let Some((at, tx)) = &self.shutdown_at
                && *at == index
            {
                tx.send(()).unwrap();
            }
            self.script.lock().unwrap().pop_front().unwrap_or(Ok(0))
        }
        async fn due_lag(&self) -> Result<Duration, RepositoryError> {
            *self.lag_reads.lock().unwrap() += 1;
            Ok(Duration::from_secs(7))
        }
    }

    fn settings(pass_limit: u32) -> LoopSettings {
        LoopSettings {
            poll_interval: Duration::from_secs(10),
            pass_limit,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_pass_starts_the_next_pass_at_once() {
        let (tx, rx) = watch::channel(());
        let fake = Fake::new(vec![Ok(1), Ok(1), Ok(1), Ok(0)]);
        let task = tokio::spawn(run_scheduler_loop(fake.clone(), settings(2), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        // Pass 1 uses two calls and is full. Pass 2 starts at once: one row, then 0.
        assert_eq!(fake.count(), 4, "the second pass must not wait for a tick");
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.iter().all(|t| *t == calls[0]), "no call waited for a tick");
        drop(calls);
        tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_does_not_end_the_loop() {
        let (tx, rx) = watch::channel(());
        let fake = Fake::new(vec![Err(RepositoryError::Store("down".into())), Ok(0)]);
        let task = tokio::spawn(run_scheduler_loop(fake.clone(), settings(5), rx));
        tokio::time::sleep(Duration::from_secs(25)).await;
        assert!(!task.is_finished(), "an error must not end the loop");
        assert!(fake.count() >= 3, "the loop kept its ticks; calls: {}", fake.count());
        tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_ends_the_loop() {
        let (tx, rx) = watch::channel(());
        let fake = Fake::new(vec![]);
        let task = tokio::spawn(run_scheduler_loop(fake, settings(5), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        tx.send(()).unwrap();
        assert!(task.await.unwrap().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_a_pass_before_the_next_row() {
        let (tx, rx) = watch::channel(());
        let fake = Arc::new(Fake {
            script: Mutex::new(VecDeque::from(vec![Ok(1), Ok(1), Ok(1), Ok(1)])),
            calls: Mutex::new(Vec::new()),
            lag_reads: Mutex::new(0),
            shutdown_at: Some((2, tx)),
        });
        let n = run_pass(&*fake, 5, &rx).await.unwrap();
        assert_eq!(fake.count(), 2, "no third call after the shutdown");
        assert_eq!(n, 2, "both rows that were fired are counted");
    }

    #[tokio::test(start_paused = true)]
    async fn a_shutdown_during_a_pass_ends_the_loop_without_a_lag_read() {
        let (tx, rx) = watch::channel(());
        let fake = Arc::new(Fake {
            script: Mutex::new(VecDeque::from(vec![Ok(1), Ok(1)])),
            calls: Mutex::new(Vec::new()),
            lag_reads: Mutex::new(0),
            shutdown_at: Some((1, tx)),
        });
        let task = tokio::spawn(run_scheduler_loop(fake.clone(), settings(5), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(task.await.unwrap().is_ok());
        assert_eq!(fake.count(), 1, "the pass stops after the shutdown");
        assert_eq!(*fake.lag_reads.lock().unwrap(), 0, "no lag read after a shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn the_gauges_are_set_after_a_pass() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (tx, rx) = watch::channel(());
        let fake = Fake::new(vec![Ok(0)]);
        let task = tokio::spawn(run_scheduler_loop(fake, settings(5), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        tx.send(()).unwrap();
        task.await.unwrap().unwrap();

        let values: Vec<(String, DebugValue)> = snapshotter.snapshot().into_vec().into_iter().map(|(k, _, _, v)| (k.key().name().to_string(), v)).collect();
        let gauge = |name: &str| {
            values.iter().find_map(|(n, v)| match v {
                DebugValue::Gauge(g) if n == name => Some(g.into_inner()),
                _ => None,
            })
        };
        assert_eq!(gauge(names::AGENT_HOST_SCHEDULER_LAG_SECONDS), Some(7.0));
        let last = gauge(names::AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS).expect("the last-pass gauge");
        assert!(last > 1_700_000_000.0, "a Unix time in seconds, got {last}");
        assert!(
            values
                .iter()
                .any(|(n, v)| n == names::AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS && matches!(v, DebugValue::Histogram(h) if !h.is_empty())),
            "the pass duration histogram has a sample"
        );
    }
}
