// SPDX-License-Identifier: Apache-2.0

//! The in-process crash matrix of the run commit (spec §6.11). Two workers A and B serve the same
//! inbox with the real adapters. A runs a script that creates `/a.md`, B one that creates `/b.md`.
//! A `FaultPoint` double stops worker A at one stage: it tells the test, then waits on a pending
//! future (or on a gate) and never panics. The test aborts A's task (stages 1 to 3) or holds the
//! lease row lock while A commits (stage 4). B takes the inbox after the ttl. Then a late commit
//! of A's record through a raw handle must give `LeaseLost` and change no row.
//!
//! A last test covers the token term of the commit's step 1 alone: A's lease expires, B takes the
//! inbox with a new token, and A's late commit must give `LeaseLost`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use metrics_util::debugging::DebuggingRecorder;
use paigasus_agent_host::adapters::agents::PgAgentResolver;
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::helikon::scripted::ScriptedModel;
use paigasus_agent_host::adapters::memory_commit::MemoryCommit;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::run_commit::PgRunCommitter;
use paigasus_agent_host::application::lease_keeper::KeeperTiming;
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{LeaseEnd, RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::fault::{FaultPoint, FaultStage, NoFaults};
use paigasus_agent_host_core::ids::{EventId, InboxId, RunId};
use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryPath};
use paigasus_agent_host_core::ports::{ClaimRepository, LeaseRepository, RunCommitter};
use paigasus_agent_host_core::retry::RetryPolicy;
use paigasus_agent_host_core::run::{BackOff, CommitError, Committed, LeaseToken, RunClass, RunRecord, RunUsage, ScreenOut, ScreenResult, SessionRow};
use paigasus_agent_host_core::screen::AllowAll;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::json;
use support::commit::{committer, inbox_scope};
use support::inbox::{insert_agent, insert_inbox, migrated_postgres};
use support::lease::{counter, expire, expires_us, insert_pending_event, snapshot};
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::{Notify, Semaphore};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(3);
const HEARTBEAT: Duration = Duration::from_secs(1);
/// The statement timeout of one heartbeat. It is shorter than the heartbeat interval.
const HB_TIMEOUT: Duration = Duration::from_millis(500);
/// The longest wait for worker A to reach the stage of its double.
const REACH_BUDGET: Duration = Duration::from_secs(30);
/// The longest wait for a task to end after an abort, or for A to end after stage 4.
const END_BUDGET: Duration = Duration::from_secs(15);
/// The longest wait for B to take the inbox after the crash.
const TAKEOVER_BUDGET: Duration = Duration::from_secs(15);
/// The longest time of one serve of worker B.
const SERVE_BUDGET: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(200);

/// How the double waits at its stage.
#[derive(Debug, Clone, Copy)]
enum Mode {
    /// Wait for ever. The test aborts the task.
    Hang,
    /// Wait for one permit from the test.
    Gate,
}

/// Reaches `stage`, tells the test, then waits. It never panics: a panic inside a transaction
/// drop can abort the process (spec §6.11).
struct StageDouble {
    stage: FaultStage,
    mode: Mode,
    reached: Notify,
    gate: Semaphore,
}

impl StageDouble {
    fn new(stage: FaultStage, mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            stage,
            mode,
            reached: Notify::new(),
            gate: Semaphore::new(0),
        })
    }
}

#[async_trait]
impl FaultPoint for StageDouble {
    async fn reach(&self, stage: FaultStage) {
        if stage != self.stage {
            return;
        }
        // `notify_one` keeps a permit when the test does not wait yet.
        self.reached.notify_one();
        match self.mode {
            Mode::Hang => std::future::pending::<()>().await,
            Mode::Gate => {
                if let Ok(permit) = self.gate.acquire().await {
                    permit.forget();
                }
            }
        }
    }
}

/// One migrated Postgres with one inbox of one agent and two pending events.
struct World {
    _node: ContainerAsync<Postgres>,
    url: String,
    db: DatabaseConnection,
    org: Uuid,
    inbox: Uuid,
    agent: Uuid,
    principal: Uuid,
    events: [Uuid; 2],
}

impl World {
    fn inbox_id(&self) -> InboxId {
        InboxId::new(self.org, self.inbox)
    }
}

async fn world() -> Option<World> {
    let (node, url, db) = migrated_postgres().await?;
    let w = World {
        _node: node,
        url,
        db,
        org: Uuid::new_v4(),
        inbox: Uuid::new_v4(),
        agent: Uuid::new_v4(),
        principal: Uuid::new_v4(),
        events: [Uuid::new_v4(), Uuid::new_v4()],
    };
    insert_agent(&w.db, w.org, w.agent, "scripted", "Use the memory tool.", &["memory"]).await;
    insert_inbox(&w.db, w.org, w.inbox, w.agent, w.principal).await;
    insert_pending_event(&w.db, w.inbox, w.events[0], 20).await;
    insert_pending_event(&w.db, w.inbox, w.events[1], 10).await;
    Some(w)
}

fn timing() -> KeeperTiming {
    KeeperTiming::new(TTL, HEARTBEAT).expect("timing")
}

fn claim_config() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 20,
        max_attempts: 5,
        thread_ttl: Duration::from_secs(86400),
    }
}

/// A committer on `db` with the in-transaction memory store and the fault point `faults`.
fn committer_with(db: &DatabaseConnection, faults: Arc<dyn FaultPoint>) -> Arc<PgRunCommitter> {
    Arc::new(PgRunCommitter::new(
        db.clone(),
        MemoryCommit::InTransaction(PgMemoryStore::new(db.clone())),
        MemoryCaps::default(),
        faults,
    ))
}

/// A `RunCommitter` that records the result of each `commit` of the committer it wraps. Stage 4
/// uses it to prove that A's commit reached the store and got `LeaseLost` there. The keeper also
/// gives `LeaseLost` with no store call after its local deadline, and that would not test step 1.
struct RecordingCommitter {
    inner: Arc<PgRunCommitter>,
    commits: std::sync::Mutex<Vec<Result<(), CommitError>>>,
    /// The time at which each store commit returned.
    ended: std::sync::Mutex<Vec<std::time::Instant>>,
}

impl RecordingCommitter {
    fn results(&self) -> Vec<Result<(), CommitError>> {
        self.commits.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn ended(&self) -> Vec<std::time::Instant> {
        self.ended.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl RunCommitter for RecordingCommitter {
    async fn commit(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError> {
        let result = self.inner.commit(h, r).await;
        self.ended.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(std::time::Instant::now());
        self.commits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(result.as_ref().map(|_| ()).map_err(Clone::clone));
        result
    }

    async fn screen_out(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError> {
        self.inner.screen_out(h, s).await
    }

    async fn back_off(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        self.inner.back_off(h, b).await
    }

    async fn release_after_shutdown(&self, h: LeaseHandle) -> Result<(), CommitError> {
        self.inner.release_after_shutdown(h).await
    }
}

/// A worker of the agent of the inbox. Its scripted model creates `/memories/<file>`, then says
/// `done`.
fn worker(w: &World, store: Arc<PgLeaseStore>, committer: Arc<dyn RunCommitter>, file: &str, faults: Arc<dyn FaultPoint>) -> RunWorker {
    let model = ScriptedModel::new(vec![vec![(
        "c1".into(),
        "memory".into(),
        json!({"command": "create", "path": format!("/memories/{file}"), "file_text": format!("written by {file}")}),
    )]]);
    let mut models = ModelRegistry::default();
    models.register("scripted", SharedModel::new(model));
    let ports = WorkerPorts {
        leases: store.clone(),
        claims: store,
        committer,
        resolver: Arc::new(PgAgentResolver::new(w.db.clone())),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(w.db.clone(), Arc::new(models))),
        memory: Arc::new(PgMemoryStore::new(w.db.clone())),
        faults,
        nonce: run_nonce,
    };
    let settings = WorkerSettings {
        claim: claim_config(),
        timing: timing(),
        limits: RunLimits {
            max_turns: 16,
            timeout: Duration::from_secs(60),
        },
        retry: RetryPolicy {
            base: Duration::from_secs(30),
            max: Duration::from_secs(1800),
        },
        max_runs_per_lease: 8,
        caps: MemoryCaps::default(),
    };
    RunWorker::new(ports, settings)
}

/// Where the double of worker A sits.
#[derive(Debug, Clone, Copy)]
enum Site {
    /// In the fault point of the worker (stages `AfterRun`, `AfterCommit`).
    Worker,
    /// In the fault point of A's committer (stage `BeforeCommit`).
    Committer,
}

/// Worker A, started in its own task.
struct WorkerA {
    task: tokio::task::JoinHandle<LeaseEnd>,
    token: LeaseToken,
    commits: Arc<RecordingCommitter>,
    /// The backend pid of A's commit connection (pinned, so one backend).
    commit_pid: i64,
}

/// Take the inbox for A and start A's serve in a task, with `double` at `site`. Then wait until A
/// reaches the stage of the double.
async fn start_a(w: &World, double: &Arc<StageDouble>, site: Site) -> WorkerA {
    let store = Arc::new(PgLeaseStore::new(support::connect_pinned(&w.url).await, HB_TIMEOUT));
    let commit_db = support::connect_pinned(&w.url).await;
    let commit_pid = support::scalar_i64(&commit_db, "SELECT pg_backend_pid()::bigint AS n").await;
    let as_fault: Arc<dyn FaultPoint> = double.clone();
    let (worker_faults, committer_faults): (Arc<dyn FaultPoint>, Arc<dyn FaultPoint>) = match site {
        Site::Worker => (as_fault, Arc::new(NoFaults)),
        Site::Committer => (Arc::new(NoFaults), as_fault),
    };
    let commits = Arc::new(RecordingCommitter {
        inner: committer_with(&commit_db, committer_faults),
        commits: std::sync::Mutex::new(Vec::new()),
        ended: std::sync::Mutex::new(Vec::new()),
    });
    let a = worker(w, store.clone(), commits.clone(), "a.md", worker_faults);
    let handle = store.acquire(&w.inbox_id(), "a", TTL).await.expect("A acquire").expect("A takes the lease");
    let token = handle.token();
    let task = tokio::spawn(async move { a.serve(handle, &CancellationToken::new()).await });
    timeout(REACH_BUDGET, double.reached.notified())
        .await
        .unwrap_or_else(|_| panic!("worker A did not reach {:?} within {REACH_BUDGET:?}", double.stage));
    WorkerA { task, token, commits, commit_pid }
}

/// Abort A's task while it waits at its stage.
async fn abort_a(a: WorkerA) {
    a.task.abort();
    let joined = timeout(END_BUDGET, a.task).await.expect("the task of A ends after the abort");
    assert!(joined.expect_err("the task of A is aborted").is_cancelled());
}

/// B polls until it takes the inbox, then serves it once with no fault. It returns B's token and
/// the end of the serve.
async fn take_over(w: &World) -> (LeaseToken, LeaseEnd) {
    let store = Arc::new(PgLeaseStore::new(support::connect_pinned(&w.url).await, HB_TIMEOUT));
    let commit_db = support::connect_pinned(&w.url).await;
    let b = worker(w, store.clone(), committer_with(&commit_db, Arc::new(NoFaults)), "b.md", Arc::new(NoFaults));
    let started = tokio::time::Instant::now();
    let handle = loop {
        if let Some(h) = store.acquire(&w.inbox_id(), "b", TTL).await.expect("B acquire") {
            break h;
        }
        assert!(started.elapsed() < TAKEOVER_BUDGET, "B did not take the inbox within {TAKEOVER_BUDGET:?}");
        tokio::time::sleep(POLL).await;
    };
    let token = handle.token();
    let end = timeout(SERVE_BUDGET, b.serve(handle, &CancellationToken::new())).await.expect("B ends its serve");
    (token, end)
}

/// The run of the inbox that has `token`.
async fn run_of(w: &World, token: LeaseToken) -> Uuid {
    let sql = format!("SELECT id FROM run WHERE inbox_id = '{}' AND lease_token = {}", w.inbox, token.0);
    let row =
        w.db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
            .await
            .expect("read the run")
            .unwrap_or_else(|| panic!("no run has the token {}", token.0));
    row.try_get("", "id").expect("id")
}

async fn n(w: &World, sql: &str) -> i64 {
    support::scalar_i64(&w.db, sql).await
}

/// The number of memory files at `path` in the scope of the inbox.
async fn files(w: &World, path: &str) -> i64 {
    n(
        w,
        &format!(
            "SELECT count(*) AS n FROM memory_file WHERE org_id = '{}' AND agent_id = '{}' AND principal_id = '{}' AND path = '{path}'",
            w.org, w.agent, w.principal
        ),
    )
    .await
}

/// The checks after B took over from a crashed A that did not commit (stages 1, 2 and 4).
async fn assert_b_committed(w: &World, a_token: LeaseToken, b_token: LeaseToken, b_end: LeaseEnd) {
    assert_eq!(b_end, LeaseEnd::Released, "B serves the events of A");
    let a_run = run_of(w, a_token).await;
    let b_run = run_of(w, b_token).await;
    assert_eq!(files(w, "/a.md").await, 0, "the run of A that never committed wrote /a.md");
    assert_eq!(files(w, "/b.md").await, 1, "B's /b.md is not stored once");
    let inbox = w.inbox;
    assert!(
        n(w, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}'")).await > 0,
        "B wrote no session rows"
    );
    assert_eq!(
        n(w, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}' AND run_id <> '{b_run}'")).await,
        0,
        "a session row is not of B's run"
    );
    assert_eq!(
        n(w, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND attempts = 2")).await,
        2,
        "each event has 2 attempts"
    );
    assert!(
        support::scalar_bool(&w.db, &format!("SELECT status = 'running' AS v FROM run WHERE id = '{a_run}'")).await,
        "A's run is not running"
    );
    assert_eq!(
        n(
            w,
            &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND status = 'done' AND claimed_run_id = '{b_run}'")
        )
        .await,
        2,
        "the events are not done by B's run"
    );
    assert_eq!(n(w, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}'")).await, 2, "a run other than A's and B's");
}

/// Every `session_event` and `memory_file` row of the world as one text.
async fn other_rows(w: &World) -> String {
    let mut out = String::new();
    for (table, filter) in [("session_event", format!("x.inbox_id = '{}'", w.inbox)), ("memory_file", format!("x.org_id = '{}'", w.org))] {
        let sql = format!("SELECT coalesce(string_agg(row_to_json(x)::text, '|' ORDER BY row_to_json(x)::text), '') AS s FROM {table} x WHERE {filter}");
        let row = w.db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql)).await.expect("rows").expect("row");
        out.push_str(&row.try_get::<String>("", "s").expect("text column named s"));
        out.push('\n');
    }
    out
}

/// A's full record: A's run, A's token, both events, one session row and a memory set that puts
/// `/a.md`.
async fn a_record(w: &World, a_token: LeaseToken) -> RunRecord {
    let run = RunId::new(w.org, run_of(w, a_token).await);
    let mut changes = MemoryChangeSet::new();
    changes.record(MemoryChange::Put {
        path: MemoryPath::parse("/a.md").expect("path"),
        content: "the late commit of A".to_owned(),
        expected: Expected::Absent,
    });
    RunRecord {
        run,
        token: a_token,
        passed: w.events.iter().copied().map(EventId::from_uuid).collect(),
        class: RunClass::Completed {
            output: Some("late".to_owned()),
            usage: RunUsage::default(),
        },
        session: vec![SessionRow {
            kind: "assistant_message".into(),
            payload: json!({"type": "assistant_message", "content": [{"type": "text", "text": "late"}]}),
        }],
        nonce: "0123456789abcdef0123456789abcdef".to_owned(),
        close_thread: false,
        memory: MemoryCommitSet::new(inbox_scope(&w.db, &w.inbox_id()).await, changes),
    }
}

/// Commit A's record through `handle`. It must give `LeaseLost`, change no row of `run`,
/// `inbox_event`, `thread`, `inbox_lease`, `session_event` and `memory_file`, and count one lost
/// lease of the operation `commit`.
async fn assert_late_commit_is_lost(w: &World, handle: &mut LeaseHandle, record: RunRecord) {
    let before = (snapshot(&w.db, w.inbox).await, other_rows(w).await);
    let recorder = DebuggingRecorder::new();
    let res = {
        let _guard = metrics::set_default_local_recorder(&recorder);
        committer(&w.db).commit(handle, record).await
    };
    assert_eq!(res.map(|_| ()), Err(CommitError::LeaseLost), "the late commit of A");
    assert_eq!(before, (snapshot(&w.db, w.inbox).await, other_rows(w).await), "the late commit of A changed a row");
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "commit"))), 1);
}

/// The late commit of a crashed A, through a raw handle that holds A's token and A's run.
async fn assert_raw_late_commit_is_lost(w: &World, a_token: LeaseToken) {
    let record = a_record(w, a_token).await;
    let mut raw = LeaseHandle::new(w.inbox_id(), a_token, "a".to_owned());
    raw.record_claim(a_token, Some(record.run.clone()));
    assert_late_commit_is_lost(w, &mut raw, record).await;
}

/// Stages 1 and 2: A stops at `stage`, the test aborts it, and B takes over.
async fn crash_before_the_commit(stage: FaultStage, site: Site) {
    let Some(w) = world().await else { return };
    let double = StageDouble::new(stage, Mode::Hang);
    let a = start_a(&w, &double, site).await;
    let a_token = a.token;
    abort_a(a).await;
    let (b_token, b_end) = take_over(&w).await;
    assert_b_committed(&w, a_token, b_token, b_end).await;
    assert_raw_late_commit_is_lost(&w, a_token).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stage_1_after_the_tool_call() {
    crash_before_the_commit(FaultStage::AfterRun, Site::Worker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stage_2_inside_the_commit() {
    crash_before_the_commit(FaultStage::BeforeCommit, Site::Committer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stage_3_after_the_commit() {
    let Some(w) = world().await else { return };
    let double = StageDouble::new(FaultStage::AfterCommit, Mode::Hang);
    let a = start_a(&w, &double, Site::Worker).await;
    let a_token = a.token;
    abort_a(a).await;
    let (_b_token, b_end) = take_over(&w).await;

    assert_eq!(b_end, LeaseEnd::NothingToClaim, "B claims nothing after A committed");
    let a_run = run_of(&w, a_token).await;
    let inbox = w.inbox;
    assert_eq!(files(&w, "/a.md").await, 1, "A's /a.md is not stored once");
    assert_eq!(files(&w, "/b.md").await, 0, "B ran after A committed");
    assert!(
        n(&w, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}'")).await > 0,
        "A wrote no session rows"
    );
    assert_eq!(
        n(&w, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}' AND run_id <> '{a_run}'")).await,
        0,
        "a session row is not of A's run"
    );
    assert_eq!(
        n(&w, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND attempts = 1")).await,
        2,
        "each event has 1 attempt"
    );
    assert!(
        support::scalar_bool(&w.db, &format!("SELECT status = 'completed' AS v FROM run WHERE id = '{a_run}'")).await,
        "A's run is not completed"
    );
    assert_eq!(
        n(
            &w,
            &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND status = 'done' AND claimed_run_id = '{a_run}'")
        )
        .await,
        2,
        "the events are not done by A's run"
    );
    assert_eq!(n(&w, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}'")).await, 1, "B made a run");

    assert_raw_late_commit_is_lost(&w, a_token).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stage_4_heartbeat_partition() {
    let Some(w) = world().await else { return };
    let double = StageDouble::new(FaultStage::AfterRun, Mode::Gate);
    let a = start_a(&w, &double, Site::Worker).await;
    let a_token = a.token;

    // Wait for a fresh heartbeat of A, so the keeper's local deadline (TTL - interval after that
    // heartbeat) is far away when A's commit takes the keeper mutex. Then A's commit reaches the
    // store, and `commits` below proves it.
    let before = expires_us(&w.db, w.inbox).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while expires_us(&w.db, w.inbox).await == before {
        assert!(tokio::time::Instant::now() < deadline, "A sent no heartbeat within 5 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The partition: the test holds the lease row lock, so A's heartbeats and A's step 1 wait.
    // The test only locks the row and does not change it, so Postgres does not check the `WHERE`
    // of a waiting `FOR UPDATE` again (ADR-0028 (b)). The test releases the lock TTL + 1 s after it
    // opens the gate. A's commit starts its wait after the gate, so the wait stays under the 5 s
    // `lock_timeout` of the commit, and A's step 1 must read the expiry after the wait.
    let txn = w.db.begin().await.expect("begin");
    let locked = txn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT token FROM inbox_lease WHERE inbox_id = '{}' FOR UPDATE", w.inbox),
        ))
        .await
        .expect("lock the lease row");
    assert!(locked.is_some(), "no lease row");
    let gate_released = tokio::time::Instant::now();
    double.gate.add_permits(1);
    // Guard: A's commit waits on the row lock of the test before the lease expires. A commit that
    // starts after the release reads an expired lease with no wait, and `ended[0] >= released`
    // below cannot tell the two apart. The pid is of A's commit connection, not of its heartbeat.
    let waiting = format!("SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE pid = {} AND wait_event_type = 'Lock'", a.commit_pid);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while n(&w, &waiting).await == 0 {
        assert!(tokio::time::Instant::now() < deadline, "A's commit did not wait on the lease row lock within 2 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep_until(gate_released + TTL + Duration::from_secs(1)).await;
    let released = std::time::Instant::now();
    txn.commit().await.expect("commit the lock");

    let a_end = timeout(END_BUDGET, a.task).await.expect("A ends after the partition").expect("the task of A");
    // Guard: A's commit waited for the lock of the test. A commit that ended before the release did
    // not wait, so it would not test the expiry check after a lock wait.
    let ended = a.commits.ended();
    assert_eq!(ended.len(), 1, "A made one store commit");
    assert!(ended[0] >= released, "A's commit ended before the test released the lock, so it did not wait");
    assert_eq!(a_end, LeaseEnd::Dropped, "A's commit after the expiry is not LeaseLost");
    assert_eq!(
        a.commits.results(),
        vec![Err(CommitError::LeaseLost)],
        "A's commit did not reach the store, or the store did not refuse it"
    );
    let (b_token, b_end) = take_over(&w).await;
    assert_b_committed(&w, a_token, b_token, b_end).await;
    assert_raw_late_commit_is_lost(&w, a_token).await;
}

/// The token term of step 1 alone. A's lease expires, and B takes the inbox with a new token and
/// claims again, so the lease row is live again. A's late commit, through A's own handle, must give
/// `LeaseLost`, write nothing and count one lost lease of `commit`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_commit_after_a_takeover_is_lease_lost() {
    let Some(w) = world().await else { return };
    let store_a = PgLeaseStore::new(support::connect_pinned(&w.url).await, HB_TIMEOUT);
    let store_b = PgLeaseStore::new(support::connect_pinned(&w.url).await, HB_TIMEOUT);
    let mut a = store_a.acquire(&w.inbox_id(), "a", TTL).await.expect("A acquire").expect("A takes the lease");
    let a_claim = store_a.claim(&mut a, &claim_config()).await.expect("A claim").expect("A claims events");
    assert_eq!(a_claim.events.len(), 2);

    expire(&w.db, w.inbox).await;
    let mut b = store_b.acquire(&w.inbox_id(), "b", TTL).await.expect("B acquire").expect("B takes the expired lease");
    assert!(b.token().0 > a.token().0, "B's take has a new token");
    let b_claim = store_b.claim(&mut b, &claim_config()).await.expect("B claim").expect("B claims the events again");
    assert_eq!(b_claim.events.len(), 2);
    let live = support::scalar_bool(&w.db, &format!("SELECT expires_at > clock_timestamp() AS v FROM inbox_lease WHERE inbox_id = '{}'", w.inbox)).await;
    assert!(live, "B's lease is not live, so the late commit would not test the token term");

    let record = a_record(&w, a_claim.lease_token).await;
    assert_eq!(record.run, a_claim.run);
    assert_late_commit_is_lost(&w, &mut a, record).await;
    // B has no heartbeat, so a lease that is live now was live during the late commit. Then the
    // `LeaseLost` came from the token term, not from the expiry.
    let still_live = support::scalar_bool(&w.db, &format!("SELECT expires_at > clock_timestamp() AS v FROM inbox_lease WHERE inbox_id = '{}'", w.inbox)).await;
    assert!(still_live, "B's lease expired before the late commit ended, so the commit did not test the token term");
    store_b.release(b).await.expect("B release");
}
