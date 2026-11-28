// SPDX-License-Identifier: Apache-2.0

//! `RunWorker`: one taken lease, its claims and the end of every run (spec §6.1, §6.3 to §6.9).

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::fault::FaultStage;
use paigasus_agent_host_core::ids::InboxId;
use paigasus_agent_host_core::inbox::InboxEvent;
use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::memory::layout::{render_run_start, run_start_context};
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryCommitSet, MemoryScope, MemoryWorkspace};
use paigasus_agent_host_core::ports::{AgentResolver, ClaimRepository, EventScreen, FaultPoint, LeaseRepository, MemoryStore, RunCommitter};
use paigasus_agent_host_core::render;
use paigasus_agent_host_core::retry::{Jitter, RetryPolicy};
use paigasus_agent_host_core::run::{BackOff, CommitError, EventDelay, FlaggedEvent, RunClass, RunRecord, ScreenOut, ScreenResult, SessionRow};
use paigasus_agent_host_core::screen::ScreenVerdict;
use paigasus_observability::names;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::breaker::{HostFaultBreaker, HostFaultSettings};
use super::ports::{AgentRunner, RunEnd, RunLimits, RunSpec, UserTurn};
use crate::application::lease_keeper::{KeeperTiming, LeaseKeeper};

/// The longest delay of a host fault, `retry_after` included.
pub const HOST_FAULT_DELAY_CAP: Duration = Duration::from_secs(86_400);

/// The settings of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSettings {
    pub claim: ClaimConfig,
    pub timing: KeeperTiming,
    pub limits: RunLimits,
    pub retry: RetryPolicy,
    /// The runs of one lease before the worker releases it (spec §6.1 step 5).
    pub max_runs_per_lease: u32,
    pub caps: MemoryCaps,
    /// The pause of the pool after host faults in a row.
    pub host_fault: HostFaultSettings,
}

/// The ports of a worker.
#[derive(Clone)]
pub struct WorkerPorts {
    pub leases: Arc<dyn LeaseRepository>,
    pub claims: Arc<dyn ClaimRepository>,
    pub committer: Arc<dyn RunCommitter>,
    pub resolver: Arc<dyn AgentResolver>,
    pub screen: Arc<dyn EventScreen>,
    pub runner: Arc<dyn AgentRunner>,
    pub memory: Arc<dyn MemoryStore>,
    pub faults: Arc<dyn FaultPoint>,
    /// The source of the random run nonce (spec §6.4). The composition root passes
    /// `adapters::helikon::memory_tool::run_nonce`, so the application layer names no adapter.
    pub nonce: fn() -> String,
}

/// How the worker ended one lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseEnd {
    /// The worker served at least one claim and released the lease.
    Released,
    /// The first claim found nothing, and the worker released the lease.
    NothingToClaim,
    /// The worker dropped the keeper with no release: an error, a lost lease or an unknown
    /// outcome. The lease expires.
    Dropped,
    /// The shutdown token stopped the worker before a claim or during a run. The worker released
    /// the lease after the shutdown.
    Shutdown,
}

/// Serves one lease at a time.
pub struct RunWorker {
    ports: WorkerPorts,
    settings: WorkerSettings,
    breaker: HostFaultBreaker,
}

impl RunWorker {
    #[must_use]
    pub fn new(ports: WorkerPorts, settings: WorkerSettings) -> Self {
        let breaker = HostFaultBreaker::new(settings.host_fault);
        Self { ports, settings, breaker }
    }

    /// The rest of the pause after host faults in a row, or `None`. The pool claims nothing while
    /// it is `Some`.
    #[must_use]
    pub fn pause_left(&self) -> Option<Duration> {
        self.breaker.pause_left(Instant::now())
    }

    /// Steps 2 to 6 of spec §6.1 for one taken lease.
    pub async fn serve(&self, handle: LeaseHandle, shutdown: &CancellationToken) -> LeaseEnd {
        let inbox = handle.inbox().clone();
        let p = &self.ports;
        let keeper = LeaseKeeper::start(handle, p.leases.clone(), p.claims.clone(), p.committer.clone(), self.settings.timing);
        let mut runs: u32 = 0;
        loop {
            // The start of this run, taken before the claim. The duration metric and the host-fault
            // pause (Ruling F2) both use it: a pause that starts after this point does not count
            // the run, however late the run reaches the runner.
            let started = Instant::now();
            // A shutdown takes no new claim. Nothing is claimed, so a plain release is enough.
            if shutdown.is_cancelled() {
                return release(keeper, &inbox, LeaseEnd::Shutdown).await;
            }
            // A pause after host faults takes no new claim, also inside a lease (Ruling F1). Nothing
            // is claimed, so a plain release is enough. The pool then waits out the pause.
            if self.pause_left().is_some() {
                return release(keeper, &inbox, LeaseEnd::Released).await;
            }
            let claim = match keeper.claim(&self.settings.claim).await {
                Ok(Some(claim)) => claim,
                Ok(None) => return release(keeper, &inbox, if runs > 0 { LeaseEnd::Released } else { LeaseEnd::NothingToClaim }).await,
                Err(e) => {
                    tracing::warn!(inbox = %inbox.uuid(), error = %e, "a claim failed; the worker drops the lease");
                    return LeaseEnd::Dropped;
                }
            };
            match self.run_claim(&keeper, &inbox, &claim, shutdown, started).await {
                Step::Next => {}
                Step::Drop => return LeaseEnd::Dropped,
                Step::Shutdown => {
                    if let Err(e) = keeper.release_after_shutdown().await {
                        tracing::warn!(inbox = %inbox.uuid(), error = %e, "the release after a shutdown failed; the lease expires");
                    }
                    return LeaseEnd::Shutdown;
                }
            }
            runs += 1;
            metrics::histogram!(names::AGENT_HOST_RUN_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
            if shutdown.is_cancelled() || runs >= self.settings.max_runs_per_lease {
                return release(keeper, &inbox, LeaseEnd::Released).await;
            }
        }
    }

    /// Step 4 of spec §6.1 for one claim: resolve, screen, run and end the run.
    async fn run_claim(&self, keeper: &LeaseKeeper, inbox: &InboxId, claim: &Claim, shutdown: &CancellationToken, run_started: Instant) -> Step {
        let p = &self.ports;
        let agent = match p.resolver.resolve(inbox).await {
            Ok(agent) => agent,
            // `NotFound` (an unknown inbox, or an agent that is not configured) and `Store` are
            // both transient (spec §6.2, §6.7).
            Err(e) => return self.transient(keeper, claim, &claim.events, e.to_string(), None).await,
        };

        let mut passed: Vec<InboxEvent> = Vec::with_capacity(claim.events.len());
        let mut flagged = Vec::new();
        for event in &claim.events {
            match p.screen.screen(event).await {
                Ok(ScreenVerdict::Pass) => passed.push(event.clone()),
                Ok(ScreenVerdict::Flag { detector, reason }) => flagged.push(FlaggedEvent { event: event.id, detector, reason }),
                // No event passed, so the back-off covers every claimed event (spec §6.3).
                Err(e) => return self.transient(keeper, claim, &claim.events, e.to_string(), None).await,
            }
        }
        if !flagged.is_empty() {
            let out = ScreenOut {
                run: claim.run.clone(),
                token: claim.lease_token,
                flagged,
                all: passed.is_empty(),
            };
            match keeper.screen_out(out).await {
                Ok(ScreenResult::AllScreened) => {
                    count_run("screened");
                    return Step::Next;
                }
                Ok(ScreenResult::Continue) => {}
                Err(CommitError::LeaseLost | CommitError::OutcomeUnknown) => return Step::Drop,
                Err(e) => return self.transient(keeper, claim, &claim.events, e.to_string(), None).await,
            }
        }
        if keeper.is_lost() {
            return Step::Drop;
        }

        let scope = MemoryScope::new(agent.definition.id.clone(), agent.principal.clone());
        let ws = Arc::new(Mutex::new(MemoryWorkspace::new(p.memory.clone(), scope, self.settings.caps)));
        let nonce = (p.nonce)();
        let items = match run_start_context(&mut *ws.lock().await).await {
            Ok(items) => items,
            Err(e) => return self.transient(keeper, claim, &passed, e.to_string(), None).await,
        };
        let turn = UserTurn {
            memory: (!items.is_empty()).then(|| render_run_start(&items, &nonce)),
            events: render::events::events(&passed, &nonce),
        };
        // Review Focus 3: the lease can be lost during the run start. No model call on a lost lease.
        if keeper.is_lost() {
            return Step::Drop;
        }

        // A shutdown during the run start sends no model request. The biased select below polls the
        // run first, so it cannot stop a run that has not started.
        if shutdown.is_cancelled() {
            count_run("shutdown");
            return Step::Shutdown;
        }

        let cancel = keeper.cancellation();
        let spec = RunSpec {
            agent: agent.definition,
            thread: claim.thread,
            fresh_thread: claim.fresh_thread,
            turn,
            workspace: ws.clone(),
            cancel: cancel.clone(),
            limits: self.settings.limits,
        };
        // The run token is a child of the keeper token, and a shutdown cancels it too. A select
        // keeps the same run future, so the run ends in its own way after the cancel.
        let run = p.runner.run(spec);
        tokio::pin!(run);
        let out = tokio::select! {
            biased;
            out = &mut run => out,
            () = shutdown.cancelled() => {
                cancel.cancel();
                run.await
            }
        };
        p.faults.reach(FaultStage::AfterRun).await;

        // A cancel can give any end (runtime-tokio 0.1.26), so the lease and the shutdown token
        // decide first, for every end.
        if keeper.is_lost() {
            count_run("discarded");
            return Step::Drop;
        }
        if shutdown.is_cancelled() {
            count_run("shutdown");
            return Step::Shutdown;
        }

        let ids = passed.iter().map(|e| e.id).collect::<Vec<_>>();
        let memory = ws.lock().await.take_changes();
        let (class, session, close_thread) = match out.end {
            RunEnd::Cancelled => return self.transient(keeper, claim, &passed, "the run was cancelled".to_owned(), None).await,
            RunEnd::Transient { error, retry_after } => return self.transient(keeper, claim, &passed, error, retry_after).await,
            RunEnd::HostFault { error, retry_after } => return self.host_fault(keeper, claim, &passed, error, retry_after, run_started).await,
            RunEnd::Completed { output, usage } => (RunClass::Completed { output: Some(output), usage }, out.session, false),
            RunEnd::Stopped { output, reason, usage } => (RunClass::Stopped { output: Some(output), reason, usage }, out.session, false),
            RunEnd::Deterministic { error, close_thread } => (
                RunClass::Deterministic {
                    error,
                    retry: self.delays(&passed, None),
                },
                Vec::new(),
                close_thread,
            ),
        };
        let full = matches!(class, RunClass::Completed { .. } | RunClass::Stopped { .. });
        // F6: the failure commit after an unencodable full commit keeps the memory changes.
        let fallback = full.then(|| memory.clone());
        let full_record = record(claim, ids.clone(), class, session, nonce.clone(), close_thread, memory);
        match self.commit(keeper, full_record).await {
            Committed::Yes(step) => step,
            Committed::No(e) if e.is_unencodable() => match fallback {
                Some(memory) => {
                    let class = RunClass::Deterministic {
                        error: e.to_string(),
                        retry: self.delays(&passed, None),
                    };
                    let failure_record = record(claim, ids, class, Vec::new(), nonce, false, memory);
                    match self.commit(keeper, failure_record).await {
                        Committed::Yes(step) => step,
                        Committed::No(e) => self.transient(keeper, claim, &passed, e.to_string(), None).await,
                    }
                }
                None => self.transient(keeper, claim, &passed, e.to_string(), None).await,
            },
            Committed::No(e) => self.transient(keeper, claim, &passed, e.to_string(), None).await,
        }
    }

    /// Commit a run record. The commit is not raced with the shutdown token (Review Focus 1). A
    /// lost lease and an unknown outcome drop the keeper. `MemoryRejected` and `Store` errors go
    /// back to the caller, which decides between a failure commit and a back-off.
    async fn commit(&self, keeper: &LeaseKeeper, record: RunRecord) -> Committed {
        let class = match record.class {
            RunClass::Completed { .. } => "completed",
            RunClass::Stopped { .. } => "stopped",
            RunClass::Deterministic { .. } => "deterministic",
        };
        let ends_a_row = matches!(record.class, RunClass::Completed { .. } | RunClass::Stopped { .. });
        match keeper.commit(record).await {
            Ok(_) => {
                count_run(class);
                if ends_a_row {
                    self.breaker.record_completed();
                }
                self.ports.faults.reach(FaultStage::AfterCommit).await;
                Committed::Yes(Step::Next)
            }
            Err(CommitError::LeaseLost) => {
                count_run("discarded");
                Committed::Yes(Step::Drop)
            }
            Err(CommitError::OutcomeUnknown) => {
                metrics::counter!(names::AGENT_HOST_COMMIT_OUTCOME_UNKNOWN_TOTAL).increment(1);
                Committed::Yes(Step::Drop)
            }
            // `MemoryRejected` keeps only its `Display` text, never the path (spec §6.8).
            Err(e) => Committed::No(e),
        }
    }

    /// A transient end: fail the run and delay `targets` (spec §6.9). A failed back-off drops the
    /// keeper, and the events become free by lease expiry.
    async fn transient(&self, keeper: &LeaseKeeper, claim: &Claim, targets: &[InboxEvent], error: String, retry_after: Option<Duration>) -> Step {
        let back_off = BackOff {
            run: claim.run.clone(),
            token: claim.lease_token,
            error,
            retry: self.delays(targets, retry_after),
        };
        match keeper.back_off(back_off).await {
            Ok(()) => {
                count_run("transient");
                Step::Next
            }
            Err(e) => {
                tracing::warn!(run = %claim.run.uuid(), error = %e, "a back-off failed; the worker drops the lease");
                Step::Drop
            }
        }
    }

    /// A host fault end: fail the run as `host_fault`, delay `targets` and give back their
    /// attempts. A failed back-off drops the keeper, and the events become free by lease expiry.
    async fn host_fault(&self, keeper: &LeaseKeeper, claim: &Claim, targets: &[InboxEvent], error: String, retry_after: Option<Duration>, run_started: Instant) -> Step {
        let back_off = BackOff {
            run: claim.run.clone(),
            token: claim.lease_token,
            error: error.clone(),
            retry: self.host_fault_delays(targets, retry_after),
        };
        match keeper.host_fault(back_off).await {
            Ok(()) => {
                count_run("host_fault");
                tracing::error!(run = %claim.run.uuid(), %error, "a host fault ended the run; the attempt is given back");
                self.breaker.record_host_fault(run_started, Instant::now());
                Step::Next
            }
            Err(e) => {
                tracing::warn!(run = %claim.run.uuid(), error = %e, "a host fault back-off failed; the worker drops the lease");
                Step::Drop
            }
        }
    }

    /// The delay of a host fault: at least `retry_after`, which the retry cap does not cut, and at
    /// most `HOST_FAULT_DELAY_CAP`.
    fn host_fault_delays(&self, targets: &[InboxEvent], retry_after: Option<Duration>) -> Vec<EventDelay> {
        targets
            .iter()
            .map(|event| EventDelay {
                event: event.id,
                delay: self
                    .settings
                    .retry
                    .delay(event.attempts, Jitter::new(rand::random_range(0.8..=1.2)), None)
                    .max(retry_after.unwrap_or_default())
                    .min(HOST_FAULT_DELAY_CAP),
            })
            .collect()
    }

    /// The retry delay of each event, from its attempts, with a fresh jitter per event.
    fn delays(&self, targets: &[InboxEvent], retry_after: Option<Duration>) -> Vec<EventDelay> {
        targets
            .iter()
            .map(|event| EventDelay {
                event: event.id,
                delay: self.settings.retry.delay(event.attempts, Jitter::new(rand::random_range(0.8..=1.2)), retry_after),
            })
            .collect()
    }
}

/// What the worker does after one claim.
enum Step {
    /// Go to the next claim.
    Next,
    /// Drop the keeper with no release.
    Drop,
    /// Release the lease after a shutdown.
    Shutdown,
}

/// The result of one commit attempt.
enum Committed {
    /// The commit ended the claim, or the keeper must go.
    Yes(Step),
    /// The store refused the record. The run is not ended yet.
    No(CommitError),
}

fn count_run(class: &'static str) {
    metrics::counter!(names::AGENT_HOST_RUNS_TOTAL, "class" => class).increment(1);
}

/// Release the lease. A failed release drops the lease instead: it then expires.
async fn release(keeper: LeaseKeeper, inbox: &InboxId, end: LeaseEnd) -> LeaseEnd {
    match keeper.release().await {
        Ok(()) => end,
        Err(e) => {
            tracing::warn!(inbox = %inbox.uuid(), error = %e, "a lease release failed; the lease expires");
            LeaseEnd::Dropped
        }
    }
}

/// The record of one run commit.
fn record(claim: &Claim, passed: Vec<paigasus_agent_host_core::ids::EventId>, class: RunClass, session: Vec<SessionRow>, nonce: String, close_thread: bool, memory: MemoryCommitSet) -> RunRecord {
    RunRecord {
        run: claim.run.clone(),
        token: claim.lease_token,
        passed,
        class,
        session,
        nonce,
        close_thread,
        memory,
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
    use paigasus_agent_host_core::lease::{HeartbeatError, LeaseHandle};
    use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryPath, MemoryScope};
    use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryStore, RepositoryError};
    use paigasus_agent_host_core::render;
    use paigasus_agent_host_core::retry::RetryPolicy;
    use paigasus_agent_host_core::run::{CommitError, LeaseToken, RunClass, RunUsage, SessionRow, UNENCODABLE_PREFIX};
    use paigasus_observability::names;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::{LeaseEnd, RunWorker, WorkerPorts, WorkerSettings};
    use crate::application::lease_keeper::KeeperTiming;
    use crate::application::worker::breaker::HostFaultSettings;
    use crate::application::worker::ports::{RunEnd, RunLimits, RunOutput};
    use crate::application::worker::test_fakes::{CommitterCall, Counters, FakeClaims, FakeCommitter, FakeLeases, FakeResolver, FakeRunner, FakeScreen, TestMemoryStore, counter, resolved_agent};

    const NONCE: &str = "N1";

    fn nonce() -> String {
        NONCE.to_owned()
    }

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn inbox() -> InboxId {
        InboxId::new(u(1), u(2))
    }

    fn agent() -> AgentId {
        AgentId::new(u(1), u(5))
    }

    fn principal() -> PrincipalId {
        PrincipalId::from_uuid(u(6))
    }

    fn scope() -> MemoryScope {
        MemoryScope::new(agent(), principal())
    }

    fn event(n: u128) -> InboxEvent {
        InboxEvent {
            id: EventId::from_uuid(u(100 + n)),
            inbox: inbox(),
            source: "urn:test".into(),
            ce_id: n.to_string(),
            ce_type: format!("agents.e{n}"),
            thread: None,
            received_at: jiff::Timestamp::UNIX_EPOCH,
            status: EventStatus::Pending,
            attempts: 1,
            not_before: jiff::Timestamp::UNIX_EPOCH,
            claimed_run: None,
            envelope: json!({}),
            data: json!({"n": n as u64}),
        }
    }

    fn claim(n: u128, events: Vec<InboxEvent>) -> Claim {
        Claim {
            run: RunId::new(u(1), u(200 + n)),
            thread: ThreadId::from_uuid(u(300)),
            lease_token: LeaseToken(1),
            fresh_thread: true,
            events,
        }
    }

    fn two_events() -> Claim {
        claim(1, vec![event(1), event(2)])
    }

    fn settings(max_runs_per_lease: u32) -> WorkerSettings {
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
            max_runs_per_lease,
            caps: MemoryCaps::default(),
            host_fault: HostFaultSettings {
                pause_after: 3,
                pause: Duration::from_secs(30),
            },
        }
    }

    fn completed() -> RunEnd {
        RunEnd::Completed {
            output: "done".into(),
            usage: RunUsage::default(),
        }
    }

    fn session_row() -> SessionRow {
        SessionRow {
            kind: "assistant_message".into(),
            payload: json!({"text": "done"}),
        }
    }

    /// One test setup: the fakes and the worker over them.
    struct Rig {
        leases: Arc<FakeLeases>,
        committer: Arc<FakeCommitter>,
        runner: Arc<FakeRunner>,
        memory: Arc<TestMemoryStore>,
        worker: RunWorker,
    }

    struct Parts {
        heartbeat: Result<(), HeartbeatError>,
        claims: Vec<Claim>,
        committer: FakeCommitter,
        resolver: Result<paigasus_agent_host_core::agent::ResolvedAgent, RepositoryError>,
        screen: FakeScreen,
        runner: FakeRunner,
        memory: TestMemoryStore,
        max_runs_per_lease: u32,
    }

    impl Default for Parts {
        fn default() -> Self {
            Self {
                heartbeat: Ok(()),
                claims: vec![two_events()],
                committer: FakeCommitter::default(),
                resolver: Ok(resolved_agent(agent(), principal())),
                screen: FakeScreen::default(),
                runner: FakeRunner::ending(completed()),
                memory: TestMemoryStore::default(),
                max_runs_per_lease: 8,
            }
        }
    }

    fn rig(parts: Parts) -> Rig {
        let leases = Arc::new(FakeLeases::new(parts.heartbeat));
        let committer = Arc::new(parts.committer);
        let runner = Arc::new(parts.runner);
        let memory = Arc::new(parts.memory);
        let ports = WorkerPorts {
            leases: leases.clone(),
            claims: Arc::new(FakeClaims::new(parts.claims)),
            committer: committer.clone(),
            resolver: Arc::new(FakeResolver(parts.resolver)),
            screen: Arc::new(parts.screen),
            runner: runner.clone(),
            memory: memory.clone(),
            faults: Arc::new(NoFaults),
            nonce,
        };
        Rig {
            leases,
            committer,
            runner,
            memory,
            worker: RunWorker::new(ports, settings(parts.max_runs_per_lease)),
        }
    }

    fn handle() -> LeaseHandle {
        LeaseHandle::new(inbox(), LeaseToken(1), "w".into())
    }

    impl Rig {
        async fn serve(&self) -> LeaseEnd {
            self.worker.serve(handle(), &CancellationToken::new()).await
        }
    }

    /// A join of `serve` with a test step, bounded in (paused) time, so that a wrong worker fails
    /// the test and does not hang it.
    async fn bounded<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(3600), f)
            .await
            .expect("serve and the test step end within one hour of test time")
    }

    async fn seed(store: &TestMemoryStore, path: &str, content: &str) {
        let mut changes = MemoryChangeSet::new();
        changes.record(MemoryChange::Put {
            path: MemoryPath::parse(path).expect("path"),
            content: content.into(),
            expected: Expected::Absent,
        });
        store.apply(&MemoryCommitSet::new(scope(), changes), &MemoryCaps::default()).await.expect("seed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_completed_run_commits_with_session_and_memory() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::new(RunOutput {
            end: completed(),
            session: vec![session_row()],
        });
        runner.write = Some(("/tasks/t1.md", "step 1 done"));
        let r = rig(Parts { runner, ..Parts::default() });
        seed(&r.memory, "/instructions.md", "be kind").await;

        assert_eq!(r.serve().await, LeaseEnd::Released);

        let commits = r.committer.commits();
        assert_eq!(commits.len(), 1, "{:?}", r.committer.calls());
        let record = &commits[0];
        assert_eq!(
            record.class,
            RunClass::Completed {
                output: Some("done".into()),
                usage: RunUsage::default()
            }
        );
        assert_eq!(record.session, vec![session_row()]);
        assert_eq!(record.memory.scope, scope());
        assert_eq!(record.memory.changes.len(), 1, "the memory write of the run is in the commit");
        assert_eq!(record.passed, vec![event(1).id, event(2).id]);
        assert_eq!(record.nonce, NONCE);
        assert_eq!(record.run, two_events().run);
        assert!(!record.close_thread);
        assert!(r.committer.back_offs().is_empty());
        let turn = &r.runner.turns()[0];
        assert!(turn.memory.as_deref().is_some_and(|m| m.starts_with("<memory-N1") && m.contains("be kind")), "{turn:?}");
        assert_eq!(turn.events, render::events::events(&[event(1), event(2)], NONCE));
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "completed"))), 1);
        assert_eq!(r.leases.releases(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_with_no_memory_file_has_no_memory_part() {
        let r = rig(Parts::default());
        assert_eq!(r.serve().await, LeaseEnd::Released);
        assert_eq!(r.runner.turns()[0].memory, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_flagged_event_goes_dead_and_the_rest_runs() {
        let screen = FakeScreen {
            flag: HashSet::from([event(1).id]),
            fail: false,
        };
        let r = rig(Parts { screen, ..Parts::default() });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let outs = r.committer.screen_outs();
        assert_eq!(outs.len(), 1);
        assert!(!outs[0].all);
        assert_eq!(outs[0].flagged.iter().map(|f| f.event).collect::<Vec<_>>(), vec![event(1).id]);
        assert_eq!(outs[0].flagged[0].detector, "pii");
        assert_eq!(r.runner.turns()[0].events, render::events::events(&[event(2)], NONCE));
        assert_eq!(r.committer.commits()[0].passed, vec![event(2).id]);
    }

    #[tokio::test(start_paused = true)]
    async fn all_flagged_makes_no_model_call() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let screen = FakeScreen {
            flag: HashSet::from([event(1).id, event(2).id]),
            fail: false,
        };
        let r = rig(Parts {
            screen,
            claims: vec![two_events(), claim(2, vec![event(3)])],
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let outs = r.committer.screen_outs();
        assert_eq!(outs.len(), 1);
        assert!(outs[0].all);
        assert_eq!(r.runner.calls(), 1, "only the next claim runs");
        assert_eq!(r.runner.turns()[0].events, render::events::events(&[event(3)], NONCE));
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "screened"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_screen_error_backs_off_every_claimed_event() {
        let r = rig(Parts {
            screen: FakeScreen { flag: HashSet::new(), fail: true },
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].retry.iter().map(|d| d.event).collect::<Vec<_>>(), vec![event(1).id, event(2).id]);
        assert_eq!(backs[0].error, "screen error: the detector is down");
        assert_eq!(r.runner.calls(), 0);
        assert!(r.committer.commits().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_resolver_error_is_transient() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let r = rig(Parts {
            resolver: Err(RepositoryError::NotFound),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].retry.len(), 2);
        assert_eq!(backs[0].error, "not found");
        assert_eq!(r.runner.calls(), 0);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "transient"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_memory_store_error_at_run_start_is_transient() {
        let screen = FakeScreen {
            flag: HashSet::from([event(1).id]),
            fail: false,
        };
        let r = rig(Parts {
            screen,
            memory: TestMemoryStore {
                get_fails: true,
                ..TestMemoryStore::default()
            },
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].retry.iter().map(|d| d.event).collect::<Vec<_>>(), vec![event(2).id], "only the passed event backs off");
        assert!(backs[0].error.contains("connection reset"), "{}", backs[0].error);
        assert_eq!(r.runner.calls(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_deterministic_end_commits_memory_and_drops_the_session() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::new(RunOutput {
            end: RunEnd::Deterministic {
                error: "max turns (16) exceeded".into(),
                close_thread: false,
            },
            session: vec![session_row()],
        });
        runner.write = Some(("/tasks/t1.md", "progress"));
        let r = rig(Parts { runner, ..Parts::default() });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let commits = r.committer.commits();
        assert_eq!(commits.len(), 1);
        let RunClass::Deterministic { error, retry } = &commits[0].class else {
            panic!("expected a deterministic class, got {:?}", commits[0].class);
        };
        assert_eq!(error, "max turns (16) exceeded");
        assert_eq!(retry.iter().map(|d| d.event).collect::<Vec<_>>(), vec![event(1).id, event(2).id]);
        // attempts 1: 30 s with a jitter in [0.8, 1.2].
        assert!(retry.iter().all(|d| d.delay >= Duration::from_secs(24) && d.delay <= Duration::from_secs(36)), "{retry:?}");
        assert!(commits[0].session.is_empty());
        assert_eq!(commits[0].memory.changes.len(), 1);
        assert!(!commits[0].close_thread);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "deterministic"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn context_length_closes_the_thread() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::Deterministic {
                error: "the context length is exceeded".into(),
                close_thread: true,
            }),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        assert!(r.committer.commits()[0].close_thread);
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_end_backs_off_with_retry_after() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::Transient {
                error: "rate limited".into(),
                retry_after: Some(Duration::from_secs(600)),
            }),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].error, "rate limited");
        assert_eq!(backs[0].retry.len(), 2);
        assert!(backs[0].retry.iter().all(|d| d.delay == Duration::from_secs(600)), "{:?}", backs[0].retry);
        assert!(r.committer.commits().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_fault_backs_off_with_a_refund_and_counts_host_fault() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::HostFault {
                error: "the gateway link failed".into(),
                retry_after: Some(Duration::from_secs(60)),
            }),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let faults = r.committer.host_faults();
        assert_eq!(faults.len(), 1, "{:?}", r.committer.calls());
        assert!(r.committer.back_offs().is_empty());
        assert!(r.committer.commits().is_empty());
        assert_eq!(faults[0].error, "the gateway link failed");
        assert_eq!(faults[0].retry.len(), 2);
        assert!(faults[0].retry.iter().all(|d| d.delay >= Duration::from_secs(60)), "{:?}", faults[0].retry);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "host_fault"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_fault_delay_keeps_a_retry_after_above_the_retry_cap() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::HostFault {
                error: "x".into(),
                retry_after: Some(Duration::from_secs(3600)),
            }),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let faults = r.committer.host_faults();
        assert_eq!(faults.len(), 1);
        assert!(faults[0].retry.iter().all(|d| d.delay == Duration::from_secs(3600)), "{:?}", faults[0].retry);
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_fault_delay_never_passes_the_cap() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::HostFault {
                error: "x".into(),
                retry_after: Some(Duration::from_secs(30 * 86_400)),
            }),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let faults = r.committer.host_faults();
        assert_eq!(faults.len(), 1);
        assert!(!faults[0].retry.is_empty());
        assert!(faults[0].retry.iter().all(|d| d.delay == Duration::from_secs(86_400)), "{:?}", faults[0].retry);
    }

    /// Ruling F2: a run that took its claim before the pause began does not double the pause, even
    /// when it reaches the runner after the pause began.
    #[tokio::test(start_paused = true)]
    async fn a_run_claimed_before_the_pause_does_not_double_it() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::HostFault { error: "x".into(), retry_after: None }),
            memory: TestMemoryStore {
                get_delay: Duration::from_secs(2),
                ..TestMemoryStore::default()
            },
            ..Parts::default()
        });
        let begin_pause = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = tokio::time::Instant::now();
            for _ in 0..3 {
                r.worker.breaker.record_host_fault(now, now);
            }
        };
        let (end, ()) = bounded(async { tokio::join!(r.serve(), begin_pause) }).await;
        assert_eq!(end, LeaseEnd::Released);
        assert_eq!(r.runner.calls(), 1);
        let left = r.worker.pause_left().expect("the pause runs");
        assert!(left <= Duration::from_secs(30), "the late fault doubled the pause: {left:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_host_fault_drops_the_lease() {
        let mut committer = FakeCommitter::default();
        committer.host_fault_result = Some(Err(CommitError::Store("x".into())));
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::HostFault { error: "x".into(), retry_after: None }),
            committer,
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Dropped);
        assert_eq!(r.committer.host_faults().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_memory_rejection_backs_off_with_no_path_in_the_error() {
        let path = MemoryPath::parse("/people/secret-name.md").expect("path");
        let r = rig(Parts {
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::MemoryRejected(MemoryApplyError::Conflict { path }))]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].error, "memory refused: memory conflict");
        assert_eq!(backs[0].retry.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unencodable_completed_commit_becomes_a_failure_commit() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::new(RunOutput {
            end: completed(),
            session: vec![session_row()],
        });
        runner.write = Some(("/tasks/t1.md", "progress"));
        let r = rig(Parts {
            runner,
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::Store(format!("{UNENCODABLE_PREFIX}22P05")))]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let commits = r.committer.commits();
        assert_eq!(commits.len(), 2, "{:?}", r.committer.calls());
        let RunClass::Deterministic { error, retry } = &commits[1].class else {
            panic!("expected a deterministic second commit, got {:?}", commits[1].class);
        };
        assert!(error.contains(UNENCODABLE_PREFIX), "{error}");
        assert_eq!(retry.len(), 2);
        assert!(commits[1].session.is_empty());
        // F6: the failure commit keeps the memory changes of the run.
        assert_eq!(commits[1].memory, commits[0].memory);
        assert_eq!(commits[1].memory.changes.len(), 1);
        assert!(r.committer.back_offs().is_empty());
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "deterministic"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_unencodable_commit_is_transient() {
        let unencodable = || Err(CommitError::Store(format!("{UNENCODABLE_PREFIX}22021")));
        let r = rig(Parts {
            committer: FakeCommitter::with_commit_results(vec![unencodable(), unencodable()]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        assert_eq!(r.committer.commits().len(), 2);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert!(backs[0].error.contains(UNENCODABLE_PREFIX), "{}", backs[0].error);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unencodable_deterministic_commit_is_transient() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::Deterministic {
                error: "x".into(),
                close_thread: false,
            }),
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::Store(format!("{UNENCODABLE_PREFIX}22P05")))]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        assert_eq!(r.committer.commits().len(), 1);
        assert_eq!(r.committer.back_offs().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_store_error_at_the_commit_is_transient() {
        let r = rig(Parts {
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::Store("connection reset".into()))]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].error, "store error: connection reset");
    }

    #[tokio::test(start_paused = true)]
    async fn outcome_unknown_drops_the_keeper() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let r = rig(Parts {
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::OutcomeUnknown)]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Dropped);
        assert_eq!(r.leases.releases(), 0);
        assert!(r.committer.back_offs().is_empty());
        assert_eq!(counter(&recorder, names::AGENT_HOST_COMMIT_OUTCOME_UNKNOWN_TOTAL, None), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_lost_commit_writes_nothing_more() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let r = rig(Parts {
            committer: FakeCommitter::with_commit_results(vec![Err(CommitError::LeaseLost)]),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Dropped);
        assert_eq!(r.committer.calls().len(), 1);
        assert_eq!(r.leases.releases(), 0);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "discarded"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_lease_after_the_run_writes_nothing() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::ending(RunEnd::Cancelled);
        runner.wait_for_cancel = true;
        let r = rig(Parts {
            heartbeat: Err(HeartbeatError::LeaseLost),
            runner,
            ..Parts::default()
        });
        assert_eq!(bounded(r.serve()).await, LeaseEnd::Dropped);
        assert_eq!(r.runner.calls(), 1);
        assert!(r.committer.calls().is_empty(), "{:?}", r.committer.calls());
        assert_eq!(r.leases.releases(), 0);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "discarded"))), 1);
    }

    /// runtime-tokio can end a cancelled run as `Completed`. A lost lease still writes nothing.
    #[tokio::test(start_paused = true)]
    async fn a_completed_run_with_a_lost_lease_writes_nothing() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::new(RunOutput {
            end: completed(),
            session: vec![session_row()],
        });
        runner.wait_for_cancel = true;
        let r = rig(Parts {
            heartbeat: Err(HeartbeatError::LeaseLost),
            runner,
            ..Parts::default()
        });
        assert_eq!(bounded(r.serve()).await, LeaseEnd::Dropped);
        assert!(r.committer.calls().is_empty(), "{:?}", r.committer.calls());
        // One snapshot for both reads: a snapshot resets every counter.
        let runs = Counters::take(&recorder);
        assert_eq!(runs.get(names::AGENT_HOST_RUNS_TOTAL, Some(("class", "discarded"))), 1);
        assert_eq!(runs.get(names::AGENT_HOST_RUNS_TOTAL, Some(("class", "completed"))), 0);
    }

    /// Review Focus 3 (F14): the lease is lost during the run start. The memory read sleeps past
    /// the local deadline, and the heartbeat at 10 s gives `LeaseLost`.
    #[tokio::test(start_paused = true)]
    async fn a_lease_lost_before_the_model_call_writes_nothing() {
        let r = rig(Parts {
            heartbeat: Err(HeartbeatError::LeaseLost),
            memory: TestMemoryStore {
                get_delay: Duration::from_secs(25),
                ..TestMemoryStore::default()
            },
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Dropped);
        assert_eq!(r.runner.calls(), 0, "the model ran on a lost lease");
        assert!(r.committer.calls().is_empty(), "{:?}", r.committer.calls());
    }

    #[tokio::test(start_paused = true)]
    async fn a_shutdown_in_the_model_loop_refunds_the_attempt() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let mut runner = FakeRunner::ending(RunEnd::Cancelled);
        runner.wait_for_cancel = true;
        let r = rig(Parts { runner, ..Parts::default() });
        let shutdown = CancellationToken::new();
        let stop = async {
            while r.runner.calls() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            shutdown.cancel();
        };
        let (end, ()) = bounded(async { tokio::join!(r.worker.serve(handle(), &shutdown), stop) }).await;
        assert_eq!(end, LeaseEnd::Shutdown);
        assert_eq!(r.committer.calls(), vec![CommitterCall::ReleaseAfterShutdown]);
        assert_eq!(r.leases.releases(), 0);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "shutdown"))), 1);
    }

    /// runtime-tokio can end a cancelled run as `Transient`. A shutdown is still the shutdown
    /// path, with no back-off.
    #[tokio::test(start_paused = true)]
    async fn a_transient_end_after_a_shutdown_is_the_shutdown_path() {
        let mut runner = FakeRunner::ending(RunEnd::Transient {
            error: "the model call was cancelled".into(),
            retry_after: None,
        });
        runner.wait_for_cancel = true;
        let r = rig(Parts { runner, ..Parts::default() });
        let shutdown = CancellationToken::new();
        let stop = async {
            while r.runner.calls() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            shutdown.cancel();
        };
        let (end, ()) = bounded(async { tokio::join!(r.worker.serve(handle(), &shutdown), stop) }).await;
        assert_eq!(end, LeaseEnd::Shutdown);
        assert_eq!(r.committer.calls(), vec![CommitterCall::ReleaseAfterShutdown]);
    }

    /// A shutdown before the first claim takes no claim and sends no model request.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_before_the_claim_claims_nothing() {
        let r = rig(Parts::default());
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        assert_eq!(bounded(r.worker.serve(handle(), &shutdown)).await, LeaseEnd::Shutdown);
        assert_eq!(r.runner.calls(), 0, "the model ran after the shutdown");
        assert!(r.committer.calls().is_empty(), "{:?}", r.committer.calls());
        assert_eq!(r.leases.releases(), 1);
    }

    /// A shutdown during the run start (the memory read) sends no model request. The worker takes
    /// the shutdown path and refunds the attempt.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_during_the_run_start_makes_no_model_call() {
        let recorder = DebuggingRecorder::new();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let r = rig(Parts {
            memory: TestMemoryStore {
                get_delay: Duration::from_secs(2),
                ..TestMemoryStore::default()
            },
            ..Parts::default()
        });
        let shutdown = CancellationToken::new();
        let stop = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            shutdown.cancel();
        };
        let (end, ()) = bounded(async { tokio::join!(r.worker.serve(handle(), &shutdown), stop) }).await;
        assert_eq!(end, LeaseEnd::Shutdown);
        assert_eq!(r.runner.calls(), 0, "the model ran after the shutdown");
        assert_eq!(r.committer.calls(), vec![CommitterCall::ReleaseAfterShutdown]);
        assert_eq!(counter(&recorder, names::AGENT_HOST_RUNS_TOTAL, Some(("class", "shutdown"))), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancel_with_no_shutdown_and_no_lost_lease_is_transient() {
        let r = rig(Parts {
            runner: FakeRunner::ending(RunEnd::Cancelled),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        let backs = r.committer.back_offs();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].error, "the run was cancelled");
        assert_eq!(r.leases.releases(), 1);
    }

    /// Review Focus 1: a shutdown during the commit lets the commit finish, and the worker then
    /// releases the lease with a plain release.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_during_the_commit_keeps_the_commit() {
        let r = rig(Parts {
            committer: FakeCommitter::gated(),
            claims: vec![two_events(), claim(2, vec![event(3)])],
            ..Parts::default()
        });
        let shutdown = CancellationToken::new();
        let stop = async {
            r.committer.entered.notified().await;
            shutdown.cancel();
            r.committer.open_gate();
        };
        let (end, ()) = bounded(async { tokio::join!(r.worker.serve(handle(), &shutdown), stop) }).await;
        assert_eq!(end, LeaseEnd::Released);
        assert_eq!(r.committer.commits().len(), 1, "{:?}", r.committer.calls());
        assert!(!r.committer.calls().contains(&CommitterCall::ReleaseAfterShutdown));
        assert_eq!(r.leases.releases(), 1);
        assert_eq!(r.runner.calls(), 1, "no claim after the shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn max_runs_per_lease_releases_the_lease() {
        let r = rig(Parts {
            claims: vec![claim(1, vec![event(1)]), claim(2, vec![event(2)]), claim(3, vec![event(3)])],
            max_runs_per_lease: 2,
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::Released);
        assert_eq!(r.committer.commits().len(), 2);
        assert_eq!(r.leases.releases(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_first_claim_is_nothing_to_claim() {
        let r = rig(Parts {
            claims: Vec::new(),
            ..Parts::default()
        });
        assert_eq!(r.serve().await, LeaseEnd::NothingToClaim);
        assert_eq!(r.leases.releases(), 1);
        assert_eq!(r.runner.calls(), 0);
    }
}
