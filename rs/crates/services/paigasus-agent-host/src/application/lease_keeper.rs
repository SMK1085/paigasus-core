// SPDX-License-Identifier: Apache-2.0

//! The `LeaseKeeper` (spec §8.2, §8.6).
//!
//! The keeper owns one `LeaseHandle` behind one mutex. It is the only writer of the lease row of
//! that handle: the heartbeat, the claim with its token rotation, the completion and the release
//! each hold the mutex for the whole repository call. So a heartbeat never runs during a claim,
//! and it always sends the current token.
//!
//! The heartbeat runs in a task on absolute monotonic instants. The keeper cancels its token when
//! the lease is lost, or when its local deadline passes with no successful heartbeat. The local
//! deadline can only stop a worker early (AG4): the database clock still decides the lease.

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::lease::{ClaimError, CompleteError, HeartbeatError, LeaseHandle};
use paigasus_agent_host_core::ports::{ClaimRepository, LeaseRepository, StoreError};
use paigasus_observability::names;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use tokio_util::sync::CancellationToken;

/// The timing of a keeper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeeperTiming {
    /// The time to live that each heartbeat sets.
    pub lease_ttl: Duration,
    /// The time between two heartbeats. It is also the margin of the local deadline and the
    /// timeout of one heartbeat call.
    pub heartbeat_interval: Duration,
}

/// The timing of a keeper is not valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeeperTimingError {
    /// The heartbeat interval is zero.
    #[error("the heartbeat interval is zero")]
    ZeroInterval,
    /// The lease ttl is shorter than three heartbeat intervals.
    #[error("the lease ttl is shorter than three heartbeat intervals")]
    TtlTooShort,
}

impl KeeperTiming {
    /// Check the timing. The interval must not be zero, and `ttl` must be at least three intervals.
    pub fn new(lease_ttl: Duration, heartbeat_interval: Duration) -> Result<Self, KeeperTimingError> {
        if heartbeat_interval.is_zero() {
            return Err(KeeperTimingError::ZeroInterval);
        }
        match heartbeat_interval.checked_mul(3) {
            Some(min) if lease_ttl >= min => Ok(Self { lease_ttl, heartbeat_interval }),
            _ => Err(KeeperTimingError::TtlTooShort),
        }
    }

    /// The time from a sent heartbeat to the local deadline: the ttl less one interval.
    fn deadline_after(&self) -> Duration {
        self.lease_ttl.saturating_sub(self.heartbeat_interval)
    }
}

/// The handle, shared between the keeper and its heartbeat task. `release` takes it out.
type SharedHandle = Arc<Mutex<Option<LeaseHandle>>>;

/// The owner of one lease handle and its heartbeat. It is the guard of spec §8.6: dropping it
/// (also during a panic unwind) aborts the heartbeat task, so the lease expires.
pub struct LeaseKeeper {
    handle: SharedHandle,
    leases: Arc<dyn LeaseRepository>,
    claims: Arc<dyn ClaimRepository>,
    cancel: CancellationToken,
    heartbeat: JoinHandle<()>,
}

impl LeaseKeeper {
    /// Take ownership of a handle and start its heartbeat. The first local deadline counts from
    /// now. It needs a Tokio runtime.
    pub fn start(handle: LeaseHandle, leases: Arc<dyn LeaseRepository>, claims: Arc<dyn ClaimRepository>, timing: KeeperTiming) -> LeaseKeeper {
        let started = Instant::now();
        let shared: SharedHandle = Arc::new(Mutex::new(Some(handle)));
        let cancel = CancellationToken::new();
        let heartbeat = tokio::spawn(heartbeat_loop(shared.clone(), leases.clone(), timing, cancel.clone(), started));
        LeaseKeeper {
            handle: shared,
            leases,
            claims,
            cancel,
            heartbeat,
        }
    }

    /// The token that the keeper cancels when the lease is lost. Give it to the run.
    #[must_use]
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Claim a batch of events under the mutex. After the lease is lost, it gives `LeaseLost`
    /// with no store call.
    pub async fn claim(&self, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> {
        let mut guard = self.handle.lock().await;
        let Some(handle) = guard.as_mut().filter(|_| !self.cancel.is_cancelled()) else {
            return Err(ClaimError::LeaseLost);
        };
        let result = self.claims.claim(handle, cfg).await;
        if matches!(result, Err(ClaimError::LeaseLost)) {
            self.lose();
        }
        result
    }

    /// Complete a claim under the mutex. After the lease is lost, it gives `LeaseLost` with no
    /// store call.
    pub async fn complete(&self, claim: Claim) -> Result<Vec<EventId>, CompleteError> {
        let mut guard = self.handle.lock().await;
        let Some(handle) = guard.as_mut().filter(|_| !self.cancel.is_cancelled()) else {
            return Err(CompleteError::LeaseLost);
        };
        let result = self.claims.complete(handle, claim).await;
        if matches!(result, Err(CompleteError::LeaseLost)) {
            self.lose();
        }
        result
    }

    /// Stop the heartbeat, then release the lease under the mutex.
    pub async fn release(self) -> Result<(), StoreError> {
        self.heartbeat.abort();
        let mut guard = self.handle.lock().await;
        match guard.take() {
            Some(handle) => self.leases.release(handle).await,
            None => Ok(()),
        }
    }

    /// The lease is lost: cancel the run and stop the heartbeat. The caller holds the mutex, so
    /// a heartbeat that waits for it sees the cancellation first.
    fn lose(&self) {
        self.cancel.cancel();
        self.heartbeat.abort();
    }
}

impl Drop for LeaseKeeper {
    fn drop(&mut self) {
        self.heartbeat.abort();
    }
}

/// The result of one heartbeat attempt.
enum Beat {
    /// The store extended the lease.
    Renewed,
    /// A store error or a timeout. The local deadline does not move.
    Failed,
    /// The lease is lost, or the handle is poisoned.
    Lost,
    /// The keeper stopped: cancelled or released.
    Stopped,
}

async fn heartbeat_loop(handle: SharedHandle, leases: Arc<dyn LeaseRepository>, timing: KeeperTiming, cancel: CancellationToken, started: Instant) {
    let mut deadline = started + timing.deadline_after();
    let mut next = started + timing.heartbeat_interval;
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            () = sleep_until(deadline) => return lose_at_deadline(&cancel),
            () = sleep_until(next) => {}
        }
        let sent = Instant::now();
        next = sent + timing.heartbeat_interval;
        // The wait for the mutex counts against the deadline too.
        let outcome = tokio::select! {
            biased;
            () = sleep_until(deadline) => return lose_at_deadline(&cancel),
            outcome = beat(&handle, leases.as_ref(), &timing, &cancel) => outcome,
        };
        match outcome {
            Beat::Renewed => deadline = sent + timing.deadline_after(),
            Beat::Failed => {}
            Beat::Lost => {
                cancel.cancel();
                return;
            }
            Beat::Stopped => return,
        }
    }
}

/// One heartbeat. It holds the mutex for the whole call.
async fn beat(handle: &SharedHandle, leases: &dyn LeaseRepository, timing: &KeeperTiming, cancel: &CancellationToken) -> Beat {
    let guard = handle.lock().await;
    // A claim or a completion that lost the lease cancels while it holds this mutex.
    if cancel.is_cancelled() {
        return Beat::Stopped;
    }
    let Some(handle) = guard.as_ref() else {
        return Beat::Stopped;
    };
    if handle.is_poisoned() {
        return Beat::Lost;
    }
    match timeout(timing.heartbeat_interval, leases.heartbeat(handle, timing.lease_ttl)).await {
        Ok(Ok(())) => Beat::Renewed,
        Ok(Err(HeartbeatError::LeaseLost)) => Beat::Lost,
        Ok(Err(HeartbeatError::Store(e))) => {
            tracing::warn!(inbox = %handle.inbox().uuid(), error = %e, "a lease heartbeat failed; the local deadline stays");
            Beat::Failed
        }
        Err(_) => {
            tracing::warn!(inbox = %handle.inbox().uuid(), "a lease heartbeat timed out; the local deadline stays");
            Beat::Failed
        }
    }
}

/// The local deadline passed with no successful heartbeat.
fn lose_at_deadline(cancel: &CancellationToken) {
    metrics::counter!(names::AGENT_HOST_LEASE_LOST_TOTAL, "operation" => "deadline").increment(1);
    cancel.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_agent_host_core::ids::{InboxId, RunId, ThreadId};
    use paigasus_agent_host_core::lease::HeartbeatError;
    use paigasus_agent_host_core::run::LeaseToken;
    use paigasus_observability::names;
    use uuid::Uuid;

    const TTL: Duration = Duration::from_secs(30);
    const INTERVAL: Duration = Duration::from_secs(10);

    /// What the fake claim does.
    #[derive(Clone, Copy)]
    enum ClaimMode {
        /// Hold the call for this long, with `in_claim` set.
        Hold(Duration),
        /// Rotate the token from 1 to 2.
        Rotate,
        /// Poison the handle and return a store error.
        Poison,
        /// Return `LeaseLost`.
        Lost,
    }

    /// A fake of both ports. It records each heartbeat.
    struct Fake {
        heartbeat_error: Option<HeartbeatError>,
        heartbeat_sleep: Duration,
        claim_mode: ClaimMode,
        complete_lost: bool,
        heartbeats: AtomicUsize,
        tokens: StdMutex<Vec<i64>>,
        in_claim: AtomicBool,
        overlap: AtomicBool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                heartbeat_error: None,
                heartbeat_sleep: Duration::ZERO,
                claim_mode: ClaimMode::Rotate,
                complete_lost: false,
                heartbeats: AtomicUsize::new(0),
                tokens: StdMutex::new(Vec::new()),
                in_claim: AtomicBool::new(false),
                overlap: AtomicBool::new(false),
            }
        }

        fn heartbeats(&self) -> usize {
            self.heartbeats.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl LeaseRepository for Fake {
        async fn acquire_next(&self, _holder: &str, _ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
            unreachable!("the keeper never acquires")
        }

        async fn acquire(&self, _inbox: &InboxId, _holder: &str, _ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
            unreachable!("the keeper never acquires")
        }

        async fn heartbeat(&self, handle: &LeaseHandle, _ttl: Duration) -> Result<(), HeartbeatError> {
            if self.in_claim.load(Ordering::SeqCst) {
                self.overlap.store(true, Ordering::SeqCst);
            }
            self.heartbeats.fetch_add(1, Ordering::SeqCst);
            self.tokens.lock().expect("tokens").push(handle.token().0);
            if !self.heartbeat_sleep.is_zero() {
                tokio::time::sleep(self.heartbeat_sleep).await;
            }
            match &self.heartbeat_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        async fn release(&self, _handle: LeaseHandle) -> Result<(), StoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ClaimRepository for Fake {
        async fn claim(&self, handle: &mut LeaseHandle, _cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> {
            match self.claim_mode {
                ClaimMode::Hold(d) => {
                    self.in_claim.store(true, Ordering::SeqCst);
                    tokio::time::sleep(d).await;
                    self.in_claim.store(false, Ordering::SeqCst);
                    Ok(None)
                }
                ClaimMode::Rotate => {
                    handle.record_claim(LeaseToken(2), None);
                    Ok(None)
                }
                ClaimMode::Poison => {
                    handle.poison();
                    Err(ClaimError::Store("commit failed".into()))
                }
                ClaimMode::Lost => Err(ClaimError::LeaseLost),
            }
        }

        async fn complete(&self, _handle: &mut LeaseHandle, _claim: Claim) -> Result<Vec<EventId>, CompleteError> {
            if self.complete_lost { Err(CompleteError::LeaseLost) } else { Ok(Vec::new()) }
        }
    }

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn handle() -> LeaseHandle {
        LeaseHandle::new(InboxId::new(u(1), u(2)), LeaseToken(1), "w".into())
    }

    fn cfg() -> ClaimConfig {
        ClaimConfig {
            batch_limit: 20,
            max_attempts: 5,
            thread_ttl: Duration::from_secs(86400),
        }
    }

    fn a_claim() -> Claim {
        Claim {
            run: RunId::new(u(1), u(3)),
            thread: ThreadId::from_uuid(u(4)),
            lease_token: LeaseToken(1),
            fresh_thread: true,
            events: Vec::new(),
        }
    }

    fn start(fake: &Arc<Fake>) -> LeaseKeeper {
        let timing = KeeperTiming::new(TTL, INTERVAL).expect("timing");
        LeaseKeeper::start(handle(), fake.clone(), fake.clone(), timing)
    }

    async fn sleep_ms(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    fn deadline_count(recorder: &DebuggingRecorder) -> u64 {
        recorder
            .snapshotter()
            .snapshot()
            .into_vec()
            .into_iter()
            .filter_map(|(key, _, _, v)| {
                let DebugValue::Counter(n) = v else { return None };
                (key.key().name() == names::AGENT_HOST_LEASE_LOST_TOTAL && key.key().labels().any(|l| l.key() == "operation" && l.value() == "deadline")).then_some(n)
            })
            .sum()
    }

    #[test]
    fn the_keeper_token_is_the_helikon_token() {
        // Compiles only while the two paths name one type.
        fn same(t: tokio_util::sync::CancellationToken) -> paigasus_helikon::core::CancellationToken {
            t
        }
        let t = CancellationToken::new();
        assert!(!same(t).is_cancelled());
    }

    #[test]
    fn keeper_timing_needs_three_intervals_in_the_ttl() {
        assert!(KeeperTiming::new(Duration::from_secs(30), Duration::from_secs(10)).is_ok());
        assert_eq!(KeeperTiming::new(Duration::from_secs(29), Duration::from_secs(10)), Err(KeeperTimingError::TtlTooShort));
        assert_eq!(KeeperTiming::new(Duration::from_secs(30), Duration::ZERO), Err(KeeperTimingError::ZeroInterval));
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_lost_heartbeat_cancels() {
        let fake = Arc::new(Fake {
            heartbeat_error: Some(HeartbeatError::LeaseLost),
            ..Fake::new()
        });
        let keeper = start(&fake);
        sleep_ms(9_900).await;
        assert!(!keeper.cancellation().is_cancelled());
        sleep_ms(200).await;
        assert!(keeper.cancellation().is_cancelled());
        assert_eq!(fake.heartbeats(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn store_errors_cancel_at_the_local_deadline_not_before() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let fake = Arc::new(Fake {
            heartbeat_error: Some(HeartbeatError::Store("down".into())),
            ..Fake::new()
        });
        let keeper = start(&fake);
        // ttl 30 s, margin one 10 s interval: the deadline is 20 s after the start.
        sleep_ms(19_900).await;
        assert!(!keeper.cancellation().is_cancelled());
        sleep_ms(200).await;
        assert!(keeper.cancellation().is_cancelled());
        assert_eq!(deadline_count(&recorder), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_heartbeat_counts_against_the_deadline() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let fake = Arc::new(Fake {
            heartbeat_sleep: Duration::from_secs(25),
            ..Fake::new()
        });
        let keeper = start(&fake);
        sleep_ms(19_900).await;
        assert!(!keeper.cancellation().is_cancelled());
        sleep_ms(200).await;
        assert!(keeper.cancellation().is_cancelled());
        assert_eq!(deadline_count(&recorder), 1);
        assert_eq!(fake.heartbeats(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_heartbeat_never_runs_during_a_claim() {
        let fake = Arc::new(Fake {
            claim_mode: ClaimMode::Hold(Duration::from_secs(15)),
            ..Fake::new()
        });
        let keeper = start(&fake);
        sleep_ms(1_000).await;
        // The claim holds the handle from 1 s to 16 s. The heartbeat at 10 s must wait.
        assert_eq!(keeper.claim(&cfg()).await, Ok(None));
        sleep_ms(1_000).await;
        assert!(fake.heartbeats() >= 1, "the waiting heartbeat must run after the claim");
        assert!(!fake.overlap.load(Ordering::SeqCst), "a heartbeat ran during the claim");
        assert!(!keeper.cancellation().is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn a_heartbeat_sends_the_token_after_a_rotation() {
        let fake = Arc::new(Fake::new());
        let keeper = start(&fake);
        sleep_ms(10_100).await;
        assert_eq!(keeper.claim(&cfg()).await, Ok(None));
        sleep_ms(10_000).await;
        assert_eq!(*fake.tokens.lock().expect("tokens"), vec![1, 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_keeper_stops_the_heartbeats() {
        let fake = Arc::new(Fake::new());
        let keeper = start(&fake);
        sleep_ms(10_100).await;
        let before = fake.heartbeats();
        assert_eq!(before, 1);
        drop(keeper);
        sleep_ms(100_000).await;
        assert_eq!(fake.heartbeats(), before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_poisoned_handle_stops_the_heartbeats_and_cancels() {
        let fake = Arc::new(Fake {
            claim_mode: ClaimMode::Poison,
            ..Fake::new()
        });
        let keeper = start(&fake);
        assert!(matches!(keeper.claim(&cfg()).await, Err(ClaimError::Store(_))));
        sleep_ms(10_100).await;
        assert_eq!(fake.heartbeats(), 0);
        assert!(keeper.cancellation().is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_lost_claim_cancels_the_keeper_at_once() {
        let fake = Arc::new(Fake {
            claim_mode: ClaimMode::Lost,
            ..Fake::new()
        });
        let keeper = start(&fake);
        assert_eq!(keeper.claim(&cfg()).await, Err(ClaimError::LeaseLost));
        assert!(keeper.cancellation().is_cancelled());
        // The keeper serves the dead handle no more: no heartbeat and no store call.
        assert_eq!(keeper.complete(a_claim()).await, Err(CompleteError::LeaseLost));
        sleep_ms(100_000).await;
        assert_eq!(fake.heartbeats(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_lost_complete_cancels_the_keeper_at_once() {
        let fake = Arc::new(Fake { complete_lost: true, ..Fake::new() });
        let keeper = start(&fake);
        assert_eq!(keeper.complete(a_claim()).await, Err(CompleteError::LeaseLost));
        assert!(keeper.cancellation().is_cancelled());
        assert_eq!(keeper.claim(&cfg()).await, Err(ClaimError::LeaseLost));
        sleep_ms(100_000).await;
        assert_eq!(fake.heartbeats(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn release_stops_the_heartbeats() {
        let fake = Arc::new(Fake::new());
        let keeper = start(&fake);
        sleep_ms(10_100).await;
        keeper.release().await.expect("release");
        sleep_ms(100_000).await;
        assert_eq!(fake.heartbeats(), 1);
    }
}
