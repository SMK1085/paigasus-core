// SPDX-License-Identifier: Apache-2.0

//! The worker pool (spec §6.1), in the form of `application::scheduler_loop::run_scheduler_loop`
//! (SMA-788). `main` spawns `run_worker_pool` in its `servers` set with a clone of the shutdown
//! watch. The pool starts `concurrency` worker tasks and supervises them. Each task takes a lease
//! with `acquire_next` and serves it with `RunWorker::serve`.
//!
//! No `select!` puts the pool token against `acquire_next` or `serve`: a take that is cut short
//! can drop a lease that the database already gave, and `serve` handles the token itself. Only a
//! sleep is raced with the token, so a shutdown ends a sleep at once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_observability::names;
use tokio::sync::watch;
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

use super::run::{LeaseEnd, RunWorker};

/// The settings of the pool. The service config makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSettings {
    /// The number of worker tasks (`worker.concurrency`).
    pub concurrency: u32,
    /// The sleep after an empty poll, before the jitter (`worker.poll_interval_ms`).
    pub poll_interval: Duration,
    /// The ttl of a taken lease.
    pub lease_ttl: Duration,
}

/// `<HOSTNAME or unknown-host>/<pid>`. The pool appends `/<task index>` (spec §6.1).
#[must_use]
pub fn holder_prefix() -> String {
    let host = std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "unknown-host".to_owned());
    format!("{host}/{}", std::process::id())
}

/// `interval × jitter`, with `jitter` clamped to [0.9, 1.1] (± 10 %). A NaN jitter counts as 1.
#[must_use]
pub fn poll_sleep(interval: Duration, jitter: f64) -> Duration {
    let jitter = if jitter.is_nan() { 1.0 } else { jitter.clamp(0.9, 1.1) };
    interval.mul_f64(jitter)
}

/// Run the pool until the shutdown watch changes or its sender is gone. The pool then cancels its
/// own token and waits for its tasks. It sets no drain timer: `main` gives the server tasks
/// `DRAIN_TIMEOUT` and then aborts them, and the drop of the pool's `JoinSet` aborts the worker
/// tasks.
///
/// A task that panics restarts with the same index and counts one
/// `AGENT_HOST_WORKER_RESTARTS_TOTAL`. A task that ends while the token is live also restarts and
/// counts (it does not do that today: a task ends only on the token).
///
/// # Errors
///
/// None. The task must not end on an error, because `main` treats an ended task as fatal. It
/// returns `Ok(())` only on shutdown.
pub async fn run_worker_pool(worker: Arc<RunWorker>, leases: Arc<dyn LeaseRepository>, settings: PoolSettings, holder_prefix: String, mut shutdown: watch::Receiver<()>) -> anyhow::Result<()> {
    let token = CancellationToken::new();
    let task = WorkerTask {
        worker,
        leases,
        settings,
        token: token.clone(),
    };
    let mut tasks = JoinSet::new();
    let mut index_of: HashMap<Id, u32> = HashMap::new();
    let spawn = |tasks: &mut JoinSet<()>, index_of: &mut HashMap<Id, u32>, index: u32, restarted: bool| {
        let holder = format!("{holder_prefix}/{index}");
        let id = tasks.spawn(task.clone().run(holder, restarted)).id();
        index_of.insert(id, index);
    };
    for index in 0..settings.concurrency {
        spawn(&mut tasks, &mut index_of, index, false);
    }
    loop {
        tokio::select! {
            // A new value and a dropped sender both mean shutdown.
            _ = shutdown.changed() => break,
            Some(joined) = tasks.join_next_with_id() => {
                let (id, panicked) = match joined {
                    Ok((id, ())) => (id, false),
                    Err(e) => (e.id(), e.is_panic()),
                };
                let Some(index) = index_of.remove(&id) else { continue };
                if panicked {
                    tracing::error!(index, "a worker task panicked; the pool restarts it");
                } else {
                    tracing::warn!(index, "a worker task ended while the pool runs; the pool restarts it");
                }
                metrics::counter!(names::AGENT_HOST_WORKER_RESTARTS_TOTAL).increment(1);
                spawn(&mut tasks, &mut index_of, index, true);
            }
        }
    }
    token.cancel();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

/// The shared parts of one worker task.
#[derive(Clone)]
struct WorkerTask {
    worker: Arc<RunWorker>,
    leases: Arc<dyn LeaseRepository>,
    settings: PoolSettings,
    token: CancellationToken,
}

impl WorkerTask {
    /// Step 1 of spec §6.1 and the loop around `serve`. A restarted task first sleeps one poll, so
    /// that a task that panics at once does not restart in a hot loop.
    async fn run(self, holder: String, restarted: bool) {
        if restarted && !self.pause(self.jittered()).await {
            return;
        }
        while !self.token.is_cancelled() {
            let pause = match self.leases.acquire_next(&holder, self.settings.lease_ttl).await {
                Ok(Some(handle)) => match self.worker.serve(handle, &self.token).await {
                    LeaseEnd::Shutdown => return,
                    LeaseEnd::Released => None,
                    LeaseEnd::Dropped => Some(self.settings.poll_interval),
                    // The inbox was a candidate, but its claim found nothing. Without a sleep the
                    // next take could pick the same inbox at once, in a hot loop.
                    LeaseEnd::NothingToClaim => Some(self.jittered()),
                },
                Ok(None) => Some(self.jittered()),
                Err(error) => {
                    tracing::warn!(%holder, %error, "acquire_next failed; the task tries again after one poll");
                    Some(self.jittered())
                }
            };
            if let Some(pause) = pause
                && !self.pause(pause).await
            {
                return;
            }
        }
    }

    fn jittered(&self) -> Duration {
        poll_sleep(self.settings.poll_interval, rand::random_range(0.9..=1.1))
    }

    /// Sleep for `d`, or less when the token is cancelled. It returns false on a cancel.
    async fn pause(&self, d: Duration) -> bool {
        tokio::select! {
            () = self.token.cancelled() => false,
            () = tokio::time::sleep(d) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
    use paigasus_agent_host_core::fault::NoFaults;
    use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, PrincipalId, RunId, ThreadId};
    use paigasus_agent_host_core::inbox::{EventStatus, InboxEvent};
    use paigasus_agent_host_core::lease::LeaseHandle;
    use paigasus_agent_host_core::memory::MemoryCaps;
    use paigasus_agent_host_core::retry::RetryPolicy;
    use paigasus_agent_host_core::run::{LeaseToken, RunUsage};
    use paigasus_observability::names;
    use serde_json::json;
    use tokio::sync::watch;
    use tokio::time::Instant;
    use uuid::Uuid;

    use super::{PoolSettings, holder_prefix, poll_sleep, run_worker_pool};
    use crate::application::lease_keeper::KeeperTiming;
    use crate::application::worker::ports::{RunEnd, RunLimits};
    use crate::application::worker::run::{RunWorker, WorkerPorts, WorkerSettings};
    use crate::application::worker::test_fakes::{FakeClaims, FakeCommitter, FakeLeases, FakeResolver, FakeRunner, FakeScreen, TestMemoryStore, counter, resolved_agent};

    const PREFIX: &str = "host/7";

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn inbox() -> InboxId {
        InboxId::new(u(1), u(2))
    }

    fn handle() -> LeaseHandle {
        LeaseHandle::new(inbox(), LeaseToken(1), format!("{PREFIX}/0"))
    }

    /// A claim of one event.
    fn claim() -> Claim {
        Claim {
            run: RunId::new(u(1), u(200)),
            thread: ThreadId::from_uuid(u(300)),
            lease_token: LeaseToken(1),
            fresh_thread: true,
            events: vec![InboxEvent {
                id: EventId::from_uuid(u(101)),
                inbox: inbox(),
                source: "urn:test".into(),
                ce_id: "1".into(),
                ce_type: "agents.e1".into(),
                thread: None,
                received_at: jiff::Timestamp::UNIX_EPOCH,
                status: EventStatus::Pending,
                attempts: 1,
                not_before: jiff::Timestamp::UNIX_EPOCH,
                claimed_run: None,
                envelope: json!({}),
                data: json!({}),
            }],
        }
    }

    fn worker_settings() -> WorkerSettings {
        WorkerSettings {
            claim: ClaimConfig {
                batch_limit: 20,
                max_attempts: 5,
                thread_ttl: Duration::from_secs(86400),
            },
            timing: KeeperTiming::new(Duration::from_secs(30), Duration::from_secs(10)).expect("timing"),
            limits: RunLimits {
                max_turns: 16,
                timeout: Duration::from_secs(300),
            },
            retry: RetryPolicy {
                base: Duration::from_secs(30),
                max: Duration::from_secs(1800),
            },
            max_runs_per_lease: 8,
            caps: MemoryCaps::default(),
        }
    }

    fn pool_settings(concurrency: u32, poll_interval: Duration) -> PoolSettings {
        PoolSettings {
            concurrency,
            poll_interval,
            lease_ttl: Duration::from_secs(30),
        }
    }

    /// A real `RunWorker` on the fakes. It returns the lease store too, to read its holder log.
    fn worker(leases: FakeLeases, claims: Vec<Claim>, runner: FakeRunner) -> (Arc<RunWorker>, Arc<FakeLeases>) {
        let leases = Arc::new(leases);
        let ports = WorkerPorts {
            leases: leases.clone(),
            claims: Arc::new(FakeClaims::new(claims)),
            committer: Arc::new(FakeCommitter::default()),
            resolver: Arc::new(FakeResolver(Ok(resolved_agent(AgentId::new(u(1), u(5)), PrincipalId::from_uuid(u(6)))))),
            screen: Arc::new(FakeScreen::default()),
            runner: Arc::new(runner),
            memory: Arc::new(TestMemoryStore::default()),
            faults: Arc::new(NoFaults),
            nonce: || "N1".to_owned(),
        };
        (Arc::new(RunWorker::new(ports, worker_settings())), leases)
    }

    fn completed() -> FakeRunner {
        FakeRunner::ending(RunEnd::Completed {
            output: "done".into(),
            usage: RunUsage::default(),
        })
    }

    /// A wait for the pool task, bounded in (paused) time, so that a pool that does not end fails
    /// the test and does not hang it.
    async fn join_within(limit: Duration, task: tokio::task::JoinHandle<anyhow::Result<()>>) {
        tokio::time::timeout(limit, task)
            .await
            .expect("the pool ends within the limit")
            .expect("the pool task does not panic")
            .expect("the pool returns Ok on shutdown");
    }

    #[test]
    fn the_poll_sleep_stays_within_ten_percent() {
        assert_eq!(poll_sleep(Duration::from_millis(1000), 2.0), Duration::from_millis(1100));
        assert_eq!(poll_sleep(Duration::from_millis(1000), 0.0), Duration::from_millis(900));
        assert_eq!(poll_sleep(Duration::from_millis(1000), 1.0), Duration::from_millis(1000));
        assert_eq!(poll_sleep(Duration::from_millis(1000), f64::NAN), Duration::from_millis(1000), "a NaN jitter is no jitter");
    }

    #[test]
    fn the_holder_names_host_and_pid() {
        let prefix = holder_prefix();
        assert!(prefix.ends_with(&format!("/{}", std::process::id())), "{prefix}");
        assert!(!prefix.starts_with('/'), "the host part is never empty: {prefix}");
    }

    /// Review Focus 2: a shutdown ends a poll sleep at once, not at the end of the sleep.
    #[tokio::test(start_paused = true)]
    async fn shutdown_ends_a_poll_sleep_at_once() {
        let (tx, rx) = watch::channel(());
        let (worker, leases) = worker(FakeLeases::new(Ok(())), Vec::new(), completed());
        let started = Instant::now();
        let task = tokio::spawn(run_worker_pool(worker, leases.clone(), pool_settings(1, Duration::from_secs(60)), PREFIX.to_owned(), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(leases.holders().len(), 1, "the task polled once and now sleeps for 60 s");

        tx.send(()).expect("send the shutdown");
        join_within(Duration::from_millis(500), task).await;
        assert!(started.elapsed() < Duration::from_secs(2), "the pool ended after {:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_task_restarts_with_its_index() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let (tx, rx) = watch::channel(());
        let (worker, leases) = worker(FakeLeases::new(Ok(())).with_next(vec![handle()]), vec![claim()], completed().panics_once());
        let task = tokio::spawn(run_worker_pool(worker, leases.clone(), pool_settings(1, Duration::from_secs(1)), PREFIX.to_owned(), rx));
        tokio::time::sleep(Duration::from_secs(10)).await;

        let holders = leases.holders();
        assert!(holders.len() >= 2, "the task acquires again after the panic: {holders:?}");
        assert!(holders.iter().all(|h| h == &format!("{PREFIX}/0")), "the new task keeps index 0: {holders:?}");
        tx.send(()).expect("send the shutdown");
        join_within(Duration::from_secs(5), task).await;
        assert_eq!(counter(&recorder, names::AGENT_HOST_WORKER_RESTARTS_TOTAL, None), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_pool_starts_concurrency_tasks() {
        let (tx, rx) = watch::channel(());
        let (worker, leases) = worker(FakeLeases::new(Ok(())), Vec::new(), completed());
        let task = tokio::spawn(run_worker_pool(worker, leases.clone(), pool_settings(3, Duration::from_secs(1)), PREFIX.to_owned(), rx));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let holders: HashSet<String> = leases.holders().into_iter().collect();
        let want: HashSet<String> = (0..3).map(|i| format!("{PREFIX}/{i}")).collect();
        assert_eq!(holders, want);
        tx.send(()).expect("send the shutdown");
        join_within(Duration::from_secs(5), task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_sender_ends_the_pool() {
        let (tx, rx) = watch::channel(());
        let (worker, leases) = worker(FakeLeases::new(Ok(())), Vec::new(), completed());
        let task = tokio::spawn(run_worker_pool(worker, leases, pool_settings(2, Duration::from_secs(60)), PREFIX.to_owned(), rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        drop(tx);
        join_within(Duration::from_millis(500), task).await;
    }
}
