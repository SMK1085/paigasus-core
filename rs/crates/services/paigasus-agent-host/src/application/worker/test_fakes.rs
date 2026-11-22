// SPDX-License-Identifier: Apache-2.0

//! Fakes of every port of the run worker, for the unit tests of `worker::run` (and of the pool).

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use paigasus_agent_host_core::agent::{AgentDefinition, ResolvedAgent};
use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, PrincipalId};
use paigasus_agent_host_core::inbox::InboxEvent;
use paigasus_agent_host_core::lease::{ClaimError, HeartbeatError, LeaseHandle};
use paigasus_agent_host_core::memory::fake::InMemoryMemoryStore;
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope};
use paigasus_agent_host_core::ports::{
    AgentResolver, ClaimRepository, LeaseRepository, MemoryApplyError, MemoryEntry, MemoryFile, MemoryStore, MemoryStoreError, MemoryUsage, RepositoryError, RunCommitter, StoreError,
};
use paigasus_agent_host_core::run::{BackOff, CommitError, Committed, RunRecord, ScreenOut, ScreenResult};
use paigasus_agent_host_core::screen::{EventScreen, ScreenError, ScreenVerdict};
use tokio::sync::{Notify, Semaphore};

use super::ports::{AgentRunner, RunEnd, RunOutput, RunSpec, UserTurn};

fn lock<T>(m: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The lease store. Each heartbeat gives `heartbeat`. `acquire_next` and `acquire` give `None`.
pub(crate) struct FakeLeases {
    pub heartbeat: Result<(), HeartbeatError>,
    releases: AtomicUsize,
}

impl FakeLeases {
    pub fn new(heartbeat: Result<(), HeartbeatError>) -> Self {
        Self {
            heartbeat,
            releases: AtomicUsize::new(0),
        }
    }

    /// The number of `release` calls.
    pub fn releases(&self) -> usize {
        self.releases.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LeaseRepository for FakeLeases {
    async fn acquire_next(&self, _holder: &str, _ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
        Ok(None)
    }

    async fn acquire(&self, _inbox: &InboxId, _holder: &str, _ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
        Ok(None)
    }

    async fn heartbeat(&self, _handle: &LeaseHandle, _ttl: Duration) -> Result<(), HeartbeatError> {
        self.heartbeat.clone()
    }

    async fn release(&self, _handle: LeaseHandle) -> Result<(), StoreError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The claim store: a queue of claim results. An empty queue gives `Ok(None)`.
pub(crate) struct FakeClaims {
    queue: StdMutex<VecDeque<Result<Option<Claim>, ClaimError>>>,
}

impl FakeClaims {
    pub fn new(claims: Vec<Claim>) -> Self {
        Self {
            queue: StdMutex::new(claims.into_iter().map(|c| Ok(Some(c))).collect()),
        }
    }
}

#[async_trait]
impl ClaimRepository for FakeClaims {
    async fn claim(&self, _handle: &mut LeaseHandle, _cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> {
        lock(&self.queue).pop_front().unwrap_or(Ok(None))
    }
}

/// One call of the committer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CommitterCall {
    Commit(Box<RunRecord>),
    ScreenOut(ScreenOut),
    BackOff(BackOff),
    ReleaseAfterShutdown,
}

/// The committer. It records every call. `commit` takes its results from a queue (an empty queue
/// gives `Ok`). `screen_out` gives `AllScreened` when `all` is set, else `Continue`, unless
/// `screen_out_result` is set. With a gate, `commit` signals `entered` and waits for one permit.
#[derive(Default)]
pub(crate) struct FakeCommitter {
    calls: StdMutex<Vec<CommitterCall>>,
    pub commit_results: StdMutex<VecDeque<Result<Committed, CommitError>>>,
    pub screen_out_result: Option<Result<ScreenResult, CommitError>>,
    pub gate: Option<Semaphore>,
    pub entered: Notify,
}

impl FakeCommitter {
    pub fn with_commit_results(results: Vec<Result<Committed, CommitError>>) -> Self {
        Self {
            commit_results: StdMutex::new(results.into()),
            ..Self::default()
        }
    }

    /// A committer whose `commit` waits for `open_gate`.
    pub fn gated() -> Self {
        Self {
            gate: Some(Semaphore::new(0)),
            ..Self::default()
        }
    }

    pub fn open_gate(&self) {
        self.gate.as_ref().expect("a gated committer").add_permits(1);
    }

    pub fn calls(&self) -> Vec<CommitterCall> {
        lock(&self.calls).clone()
    }

    pub fn commits(&self) -> Vec<RunRecord> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                CommitterCall::Commit(r) => Some(*r),
                _ => None,
            })
            .collect()
    }

    pub fn back_offs(&self) -> Vec<BackOff> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                CommitterCall::BackOff(b) => Some(b),
                _ => None,
            })
            .collect()
    }

    pub fn screen_outs(&self) -> Vec<ScreenOut> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                CommitterCall::ScreenOut(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn record(&self, call: CommitterCall) {
        lock(&self.calls).push(call);
    }
}

#[async_trait]
impl RunCommitter for FakeCommitter {
    async fn commit(&self, _h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError> {
        let run = r.run.clone();
        let done = r.passed.clone();
        self.record(CommitterCall::Commit(Box::new(r)));
        if let Some(gate) = &self.gate {
            self.entered.notify_one();
            gate.acquire().await.expect("the gate is open").forget();
        }
        lock(&self.commit_results).pop_front().unwrap_or(Ok(Committed { run, done }))
    }

    async fn screen_out(&self, _h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError> {
        let all = s.all;
        self.record(CommitterCall::ScreenOut(s));
        self.screen_out_result.clone().unwrap_or(Ok(if all { ScreenResult::AllScreened } else { ScreenResult::Continue }))
    }

    async fn back_off(&self, _h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        self.record(CommitterCall::BackOff(b));
        Ok(())
    }

    async fn release_after_shutdown(&self, _h: LeaseHandle) -> Result<(), CommitError> {
        self.record(CommitterCall::ReleaseAfterShutdown);
        Ok(())
    }
}

/// The resolver: one fixed result.
pub(crate) struct FakeResolver(pub Result<ResolvedAgent, RepositoryError>);

#[async_trait]
impl AgentResolver for FakeResolver {
    async fn resolve(&self, _inbox: &InboxId) -> Result<ResolvedAgent, RepositoryError> {
        self.0.clone()
    }
}

/// The screen: it flags the chosen events, or fails for every event.
#[derive(Default)]
pub(crate) struct FakeScreen {
    pub flag: HashSet<EventId>,
    pub fail: bool,
}

#[async_trait]
impl EventScreen for FakeScreen {
    async fn screen(&self, event: &InboxEvent) -> Result<ScreenVerdict, ScreenError> {
        if self.fail {
            return Err(ScreenError("the detector is down".into()));
        }
        Ok(if self.flag.contains(&event.id) {
            ScreenVerdict::Flag {
                detector: "pii".into(),
                reason: "an email".into(),
            }
        } else {
            ScreenVerdict::Pass
        })
    }
}

/// The runner. It counts its calls and keeps each user turn. It writes `write` into the workspace
/// (when set), waits for the run token (when `wait_for_cancel` is set), and then gives `output`.
pub(crate) struct FakeRunner {
    pub output: RunOutput,
    pub wait_for_cancel: bool,
    pub write: Option<(&'static str, &'static str)>,
    calls: AtomicUsize,
    turns: StdMutex<Vec<UserTurn>>,
}

impl FakeRunner {
    pub fn new(output: RunOutput) -> Self {
        Self {
            output,
            wait_for_cancel: false,
            write: None,
            calls: AtomicUsize::new(0),
            turns: StdMutex::new(Vec::new()),
        }
    }

    /// A runner that ends with `end` and no session rows.
    pub fn ending(end: RunEnd) -> Self {
        Self::new(RunOutput { end, session: Vec::new() })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn turns(&self) -> Vec<UserTurn> {
        lock(&self.turns).clone()
    }
}

#[async_trait]
impl AgentRunner for FakeRunner {
    async fn run(&self, spec: RunSpec) -> RunOutput {
        self.calls.fetch_add(1, Ordering::SeqCst);
        lock(&self.turns).push(spec.turn.clone());
        if let Some((path, content)) = self.write {
            let path = MemoryPath::parse(path).expect("a test path");
            spec.workspace.lock().await.write(&path, content.to_owned()).await.expect("the workspace write");
        }
        if self.wait_for_cancel {
            spec.cancel.cancelled().await;
        }
        self.output.clone()
    }
}

/// A memory store over `InMemoryMemoryStore` whose `get` can sleep first, or fail.
#[derive(Default)]
pub(crate) struct TestMemoryStore {
    pub inner: InMemoryMemoryStore,
    pub get_delay: Duration,
    pub get_fails: bool,
}

#[async_trait]
impl MemoryStore for TestMemoryStore {
    async fn get(&self, scope: &MemoryScope, path: &MemoryPath) -> Result<Option<MemoryFile>, MemoryStoreError> {
        if !self.get_delay.is_zero() {
            tokio::time::sleep(self.get_delay).await;
        }
        if self.get_fails {
            return Err(MemoryStoreError::Store("connection reset".into()));
        }
        self.inner.get(scope, path).await
    }

    async fn list(&self, scope: &MemoryScope, dir: &MemoryDir) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        self.inner.list(scope, dir).await
    }

    async fn usage(&self, scope: &MemoryScope) -> Result<MemoryUsage, MemoryStoreError> {
        self.inner.usage(scope).await
    }

    async fn apply(&self, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        self.inner.apply(set, caps).await
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> Result<(), MemoryStoreError> {
        self.inner.delete_scope(scope).await
    }
}

/// A resolved agent with no tools.
pub(crate) fn resolved_agent(agent: AgentId, principal: PrincipalId) -> ResolvedAgent {
    ResolvedAgent {
        definition: AgentDefinition {
            id: agent,
            name: "triage".into(),
            model_id: "scripted".into(),
            instructions: "Be brief.".into(),
            tool_names: Vec::new(),
        },
        principal,
    }
}

/// The sum of a counter, with one label value or with any label.
pub(crate) fn counter(recorder: &DebuggingRecorder, name: &str, label: Option<(&str, &str)>) -> u64 {
    recorder
        .snapshotter()
        .snapshot()
        .into_vec()
        .into_iter()
        .filter_map(|(key, _, _, v)| {
            let DebugValue::Counter(n) = v else { return None };
            if key.key().name() != name {
                return None;
            }
            match label {
                Some((k, want)) => key.key().labels().any(|l| l.key() == k && l.value() == want).then_some(n),
                None => Some(n),
            }
        })
        .sum()
}
