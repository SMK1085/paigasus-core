// SPDX-License-Identifier: Apache-2.0

//! The sequential property test of the lease and the claim (spec §10.1). Two to four simulated
//! workers, each on its own pinned connection, run generated operations on one inbox, one after
//! another. After every step the test reads the database and checks the invariants. After the
//! sequence, one fresh worker drains the inbox, and every inserted event must be `done` or `dead`
//! (AG2).
//!
//! The binary has `retries = 0` in `rs/.config/nextest.toml`: a failure must red at once, not
//! pass on a retry with a new seed. Commit every seed that proptest writes to
//! `tests/lease_property_pg.proptest-regressions`.

mod support;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::{EventId, InboxId, ThreadId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::lease::{ClaimError, CompleteError, HeartbeatError, LeaseHandle};
use paigasus_agent_host_core::ports::{ClaimRepository, InboxRepository, InsertOutcome, LeaseRepository};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, TestCaseError, TestRunner};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{expire, insert_thread, lease_state, newest_run_thread, snapshot};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(60);
const HB_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_WORKERS: usize = 4;
const CASES: u32 = 64;
const ORG: Uuid = Uuid::from_u128(1);
/// The upper bound of drain rounds. Each round completes one claim, so a drain that needs more
/// rounds does not make progress.
const DRAIN_ROUNDS: usize = 500;

fn cfg() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 3,
        max_attempts: 3,
        thread_ttl: Duration::from_secs(86400),
    }
}

/// A new UUID for each inbox, event and thread of this process.
fn fresh() -> Uuid {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Uuid::from_u128((0xA5u128 << 120) | u128::from(NEXT.fetch_add(1, Ordering::Relaxed)))
}

#[derive(Debug, Clone)]
enum ThreadChoice {
    /// No thread.
    None,
    /// The thread of the newest run of the inbox (no thread when the inbox has no run).
    ExistingOfRun,
    /// A thread of another inbox. The repository stores no thread for it (AG6).
    Foreign,
}

#[derive(Debug, Clone)]
enum Op {
    Acquire(usize),
    Heartbeat(usize),
    Claim(usize),
    /// `old`: complete an older claim (of a replaced or released handle, or already completed)
    /// with the current handle of the worker.
    Complete {
        worker: usize,
        old: bool,
    },
    Release(usize),
    Insert {
        thread: ThreadChoice,
    },
    Expire,
}

fn op(workers: usize) -> impl Strategy<Value = Op> {
    // Half of the operations go to worker 0. A rotation needs one worker to claim, complete and
    // claim again, so a uniform choice of the worker makes rotations rare.
    let w = prop_oneof![1 => Just(0usize), 1 => 0..workers];
    let thread = prop_oneof![Just(ThreadChoice::None), Just(ThreadChoice::ExistingOfRun), Just(ThreadChoice::Foreign)];
    prop_oneof![
        3 => w.clone().prop_map(Op::Acquire),
        1 => w.clone().prop_map(Op::Heartbeat),
        4 => w.clone().prop_map(Op::Claim),
        4 => w.clone().prop_map(|worker| Op::Complete { worker, old: false }),
        1 => w.clone().prop_map(|worker| Op::Complete { worker, old: true }),
        1 => w.prop_map(Op::Release),
        3 => thread.prop_map(|thread| Op::Insert { thread }),
        2 => Just(Op::Expire),
    ]
}

/// The coverage counters over all cases.
#[derive(Default)]
struct Coverage {
    stale_complete: AtomicUsize,
    take_after_expire: AtomicUsize,
    rotation: AtomicUsize,
    old_claim_complete: AtomicUsize,
}

/// What every case shares: the database, one store per worker (each on one pinned connection),
/// the drain store, the repository and the thread of the foreign inbox.
struct Ctx {
    db: DatabaseConnection,
    stores: Vec<PgLeaseStore>,
    drain: PgLeaseStore,
    repo: PgInboxRepository,
    foreign_thread: Uuid,
    coverage: Coverage,
}

/// The state of one simulated worker in one case.
#[derive(Default)]
struct Worker {
    handle: Option<LeaseHandle>,
    claim: Option<Claim>,
    /// The model: true after a successful claim (also an empty one) since the last take. The
    /// next claim must then rotate the token. The test derives this from the operations, not
    /// from the handle.
    claimed_since_take: bool,
}

/// The state of one case.
struct Case<'a> {
    ctx: &'a Ctx,
    inbox: InboxId,
    workers: Vec<Worker>,
    /// Claims that are no longer the open claim of a held handle.
    old_claims: Vec<Claim>,
    /// The ids of every inserted event.
    inserted: Vec<Uuid>,
    /// The `done` ids of every successful complete.
    done: HashSet<Uuid>,
    /// The largest token that an acquire or a claim returned.
    max_token: i64,
    /// True after an `Expire`, until the next successful acquire.
    expired: bool,
}

impl Case<'_> {
    fn id(&self) -> Uuid {
        self.inbox.uuid()
    }

    /// The token of the lease row and whether it is live. `(0, false)` when there is no row.
    async fn lease(&self) -> (i64, bool) {
        lease_state(&self.ctx.db, self.id()).await.unwrap_or((0, false))
    }

    async fn step(&mut self, op: &Op) -> Result<(), TestCaseError> {
        match op {
            Op::Acquire(w) => self.acquire(*w).await,
            Op::Heartbeat(w) => self.heartbeat(*w).await,
            Op::Claim(w) => self.claim(*w).await,
            Op::Complete { worker, old: false } => self.complete(*worker).await,
            Op::Complete { worker, old: true } => self.complete_old(*worker).await,
            Op::Release(w) => self.release(*w).await,
            Op::Insert { thread } => self.insert(thread).await,
            Op::Expire => {
                expire(&self.ctx.db, self.id()).await;
                self.expired = true;
                Ok(())
            }
        }
    }

    async fn acquire(&mut self, w: usize) -> Result<(), TestCaseError> {
        let (token, live) = self.lease().await;
        let had_row = token > 0;
        let got = self.ctx.stores[w].acquire(&self.inbox, &format!("w{w}"), TTL).await.expect("acquire");
        let Some(handle) = got else {
            prop_assert!(live, "acquire returned None on a lease that is not live");
            return Ok(());
        };
        prop_assert!(!live, "acquire took a live lease");
        let t = handle.token().0;
        prop_assert!(t > self.max_token, "take token {} is not larger than {}", t, self.max_token);
        self.max_token = t;
        if self.expired && had_row {
            self.ctx.coverage.take_after_expire.fetch_add(1, Ordering::Relaxed);
        }
        self.expired = false;
        let worker = &mut self.workers[w];
        worker.handle = Some(handle);
        worker.claimed_since_take = false;
        if let Some(claim) = worker.claim.take() {
            self.old_claims.push(claim);
        }
        Ok(())
    }

    async fn heartbeat(&mut self, w: usize) -> Result<(), TestCaseError> {
        let Some(handle) = self.workers[w].handle.as_ref() else { return Ok(()) };
        let (token, live) = self.lease().await;
        let valid = live && token == handle.token().0;
        match self.ctx.stores[w].heartbeat(handle, TTL).await {
            Ok(()) => prop_assert!(valid, "heartbeat succeeded on a lease that is not valid"),
            Err(HeartbeatError::LeaseLost) => prop_assert!(!valid, "heartbeat lost a valid lease"),
            Err(e) => panic!("heartbeat: {e}"),
        }
        Ok(())
    }

    async fn claim(&mut self, w: usize) -> Result<(), TestCaseError> {
        let (token, live) = self.lease().await;
        let max_token = self.max_token;
        let worker = &mut self.workers[w];
        let Some(handle) = worker.handle.as_mut() else { return Ok(()) };
        // The expectations come from the model. The handle must agree with the model.
        let open = worker.claim.is_some();
        let rotates = worker.claimed_since_take;
        prop_assert_eq!(handle.open_claim().is_some(), open, "the handle and the model disagree on the open claim");
        prop_assert_eq!(handle.needs_rotation(), rotates, "the handle and the model disagree on the rotation");
        let before = handle.token().0;
        let valid = live && token == before;
        let result = self.ctx.stores[w].claim(handle, &cfg()).await;
        if open {
            prop_assert_eq!(result, Err(ClaimError::ClaimOpen), "a claim on an open claim");
            return Ok(());
        }
        match result {
            Ok(claim) => {
                prop_assert!(valid, "a claim succeeded on a lease that is not valid");
                let t = handle.token().0;
                if rotates {
                    prop_assert!(t > max_token, "rotated token {} is not larger than {}", t, max_token);
                    self.ctx.coverage.rotation.fetch_add(1, Ordering::Relaxed);
                } else {
                    // R6: the first claim after a take reuses the take token.
                    prop_assert!(t == before && t == max_token, "first claim token {} is not the take token {}", t, before);
                }
                self.max_token = t;
                worker.claimed_since_take = true;
                if let Some(claim) = claim {
                    prop_assert_eq!(claim.lease_token.0, t);
                    worker.claim = Some(claim);
                }
            }
            Err(ClaimError::LeaseLost) => prop_assert!(!valid, "a claim lost a valid lease"),
            Err(e) => panic!("claim: {e}"),
        }
        Ok(())
    }

    async fn complete(&mut self, w: usize) -> Result<(), TestCaseError> {
        let (token, live) = self.lease().await;
        let worker = &mut self.workers[w];
        let (Some(handle), Some(claim)) = (worker.handle.as_mut(), worker.claim.as_ref()) else {
            return Ok(());
        };
        let valid = live && token == claim.lease_token.0;
        let before = snapshot(&self.ctx.db, self.inbox.uuid()).await;
        match self.ctx.stores[w].complete(handle, claim.clone()).await {
            Ok(done) => {
                prop_assert!(valid, "a complete succeeded with a stale token");
                for id in done {
                    prop_assert!(self.done.insert(id.uuid()), "event {} is done twice", id.uuid());
                }
                let claim = worker.claim.take().expect("claim");
                self.old_claims.push(claim);
            }
            Err(CompleteError::LeaseLost) => {
                prop_assert!(!valid, "a complete lost a valid lease");
                prop_assert_eq!(&before, &snapshot(&self.ctx.db, self.inbox.uuid()).await, "a stale complete changed a row");
                self.ctx.coverage.stale_complete.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => panic!("complete: {e}"),
        }
        Ok(())
    }

    async fn complete_old(&mut self, w: usize) -> Result<(), TestCaseError> {
        let Some(claim) = self.old_claims.last().cloned() else { return Ok(()) };
        let Some(handle) = self.workers[w].handle.as_mut() else { return Ok(()) };
        let before = snapshot(&self.ctx.db, self.inbox.uuid()).await;
        let result = self.ctx.stores[w].complete(handle, claim).await;
        prop_assert_eq!(result, Err(CompleteError::LeaseLost), "a complete with an old claim");
        prop_assert_eq!(&before, &snapshot(&self.ctx.db, self.inbox.uuid()).await, "a complete with an old claim changed a row");
        self.ctx.coverage.old_claim_complete.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn release(&mut self, w: usize) -> Result<(), TestCaseError> {
        let worker = &mut self.workers[w];
        let Some(handle) = worker.handle.take() else { return Ok(()) };
        self.ctx.stores[w].release(handle).await.expect("release");
        if let Some(claim) = worker.claim.take() {
            self.old_claims.push(claim);
        }
        Ok(())
    }

    async fn insert(&mut self, choice: &ThreadChoice) -> Result<(), TestCaseError> {
        let thread = match choice {
            ThreadChoice::None => None,
            ThreadChoice::ExistingOfRun => newest_run_thread(&self.ctx.db, self.id()).await,
            ThreadChoice::Foreign => Some(self.ctx.foreign_thread),
        };
        let id = fresh();
        let event = NewInboxEvent {
            id: EventId::from_uuid(id),
            inbox: self.inbox.clone(),
            source: "urn:test".to_string(),
            ce_id: id.to_string(),
            ce_type: "agents.x".to_string(),
            thread: thread.map(ThreadId::from_uuid),
            envelope: serde_json::json!({ "data": { "n": self.inserted.len() } }),
        };
        let outcome = self.ctx.repo.insert_event(event).await.expect("insert event");
        prop_assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(id) });
        self.inserted.push(id);
        Ok(())
    }

    /// The invariants after every step, read from the database.
    async fn check(&self) -> Result<(), TestCaseError> {
        let inbox = self.id();
        let (token, live) = self.lease().await;
        let holders = self.workers.iter().filter_map(|w| w.handle.as_ref()).filter(|h| live && h.token().0 == token).count();
        prop_assert!(holders <= 1, "{} workers hold the current live token", holders);

        // One round trip for the three counts: the running runs with the current live token, the
        // claimed events that are not on the thread of their run (AG5), and the events over
        // `max_attempts`.
        let max = cfg().max_attempts;
        let sql = format!(
            "SELECT \
               (SELECT count(*) FROM run r JOIN inbox_lease l ON l.inbox_id = r.inbox_id \
                WHERE r.inbox_id = '{inbox}' AND r.status = 'running' AND r.lease_token = l.token \
                  AND l.expires_at > clock_timestamp()) AS running, \
               (SELECT count(*) FROM inbox_event e JOIN run r ON r.id = e.claimed_run_id \
                WHERE e.inbox_id = '{inbox}' AND e.thread_id IS DISTINCT FROM r.thread_id) AS mixed, \
               (SELECT count(*) FROM inbox_event WHERE inbox_id = '{inbox}' AND attempts > {max}) AS over"
        );
        let row = self
            .ctx
            .db
            .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
            .await
            .expect("invariant counts")
            .expect("row");
        let get = |column: &str| row.try_get::<i64>("", column).expect("bigint column");
        let (running, mixed, over) = (get("running"), get("mixed"), get("over"));
        prop_assert!(running <= 1, "{} running runs have the current live token", running);
        prop_assert_eq!(mixed, 0, "a claimed event is not on the thread of its run (AG5)");
        prop_assert_eq!(over, 0, "an event has more than max_attempts attempts");
        Ok(())
    }

    /// One fresh worker claims and completes until no event is claimable. Then every inserted
    /// event is `done` or `dead` (AG2).
    async fn drain(&mut self) -> Result<(), TestCaseError> {
        let store = &self.ctx.drain;
        let mut handle = match store.acquire(&self.inbox, "drain", TTL).await.expect("drain acquire") {
            Some(handle) => handle,
            None => {
                expire(&self.ctx.db, self.id()).await;
                store.acquire(&self.inbox, "drain", TTL).await.expect("drain acquire").expect("a take after expire")
            }
        };
        let mut drained = false;
        for _ in 0..DRAIN_ROUNDS {
            let Some(claim) = store.claim(&mut handle, &cfg()).await.expect("drain claim") else {
                drained = true;
                break;
            };
            for id in store.complete(&mut handle, claim).await.expect("drain complete") {
                prop_assert!(self.done.insert(id.uuid()), "event {} is done twice", id.uuid());
            }
            self.check().await?;
        }
        prop_assert!(drained, "the drain did not end within {} rounds", DRAIN_ROUNDS);
        store.release(handle).await.expect("drain release");

        let inbox = self.id();
        let total = count(&self.ctx.db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}'")).await;
        let settled = count(
            &self.ctx.db,
            &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND status IN ('done', 'dead')"),
        )
        .await;
        let inserted = i64::try_from(self.inserted.len()).expect("count");
        prop_assert_eq!(total, inserted, "the inbox holds other events than the inserted ones");
        prop_assert_eq!(settled, inserted, "an inserted event is not done or dead after the drain (AG2)");
        Ok(())
    }
}

async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
    support::scalar_i64(db, sql).await
}

async fn run_case(ctx: &Ctx, workers: usize, ops: &[Op]) -> Result<(), TestCaseError> {
    let inbox = fresh();
    insert_inbox(&ctx.db, ORG, inbox, fresh(), fresh()).await;
    let mut case = Case {
        ctx,
        inbox: InboxId::new(ORG, inbox),
        workers: (0..workers).map(|_| Worker::default()).collect(),
        old_claims: Vec::new(),
        inserted: Vec::new(),
        done: HashSet::new(),
        max_token: 0,
        expired: false,
    };
    for op in ops {
        case.step(op).await?;
        case.check().await?;
    }
    case.drain().await
}

#[test]
fn lease_and_claim_keep_their_invariants() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    // The container drops at the end of this function, and its drop needs a runtime context.
    let _enter = rt.enter();
    let Some((_node, url, db)) = rt.block_on(migrated_postgres()) else { return };
    let ctx = rt.block_on(async {
        let mut stores = Vec::with_capacity(MAX_WORKERS);
        for _ in 0..MAX_WORKERS {
            stores.push(PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT));
        }
        let drain = PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT);
        let foreign_inbox = fresh();
        let foreign_thread = fresh();
        insert_inbox(&db, ORG, foreign_inbox, fresh(), fresh()).await;
        insert_thread(&db, foreign_inbox, foreign_thread).await;
        Ctx {
            repo: PgInboxRepository::new(db.clone()),
            db,
            stores,
            drain,
            foreign_thread,
            coverage: Coverage::default(),
        }
    });

    // A fixed case first. It reaches each required coverage state by construction, so the
    // coverage assertions cannot red by chance. The generated cases still explore at random.
    if let Err(e) = rt.block_on(run_case(&ctx, 2, &fixed_case())) {
        panic!("the fixed case failed: {e}");
    }
    let c = &ctx.coverage;
    for (name, n) in required_coverage(c) {
        assert!(n.load(Ordering::Relaxed) >= 1, "the fixed case reached no {name}");
    }

    let config = Config {
        cases: CASES,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct("tests/lease_property_pg.proptest-regressions"))),
        ..Config::default()
    };
    let strategy = (2usize..=MAX_WORKERS).prop_flat_map(|n| (Just(n), vec(op(n), 1..=40)));
    let mut runner = TestRunner::new(config);
    if let Err(e) = runner.run(&strategy, |(workers, ops)| rt.block_on(run_case(&ctx, workers, &ops))) {
        panic!("{e}");
    }

    // The three counters of spec §10.1. The fixed case already reached each one.
    for (name, n) in required_coverage(c) {
        let n = n.load(Ordering::Relaxed);
        assert!(n >= 1, "coverage: no {name} in {CASES} cases");
        eprintln!("coverage: {name} = {n}");
    }
    // Not required by the spec, and not asserted: the old-claim case stops at the in-memory
    // pre-check of `complete`.
    eprintln!("coverage: complete with an old claim = {}", c.old_claim_complete.load(Ordering::Relaxed));
}

/// The coverage counters that spec §10.1 requires.
fn required_coverage(c: &Coverage) -> [(&'static str, &AtomicUsize); 3] {
    [("stale complete", &c.stale_complete), ("take after expire", &c.take_after_expire), ("rotation", &c.rotation)]
}

/// Worker 0 takes, claims, completes and claims again (a rotation). The lease expires, worker 1
/// takes it (a take after expire), and worker 0 completes its open claim with the stale token (a
/// stale complete).
fn fixed_case() -> Vec<Op> {
    vec![
        Op::Acquire(0),
        Op::Insert { thread: ThreadChoice::None },
        Op::Claim(0),
        Op::Complete { worker: 0, old: false },
        Op::Insert { thread: ThreadChoice::ExistingOfRun },
        Op::Claim(0),
        Op::Expire,
        Op::Acquire(1),
        Op::Complete { worker: 0, old: false },
    ]
}
