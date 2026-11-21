// SPDX-License-Identifier: Apache-2.0

//! The concurrent stress test of the lease and the claim (spec §10.2). Eight tasks, each with its
//! own pinned connection and its own `LeaseKeeper`, run acquire, claim, complete and release on
//! one inbox, while a writer inserts events. A seeded LCG decides when a task expires its own
//! lease from a separate connection between its claim and its complete. Each task records every
//! successful complete. After a drain, the test checks the history against the `run` rows.
//!
//! The binary has `retries = 0` in `rs/.config/nextest.toml`: a race that reds one run must not
//! pass on a retry. Set `LEASE_STRESS_SEED` to replay the expire decisions of a failed run.

mod support;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::application::lease_keeper::{KeeperTiming, LeaseKeeper};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::ids::{EventId, InboxId, ThreadId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::lease::{ClaimError, CompleteError};
use paigasus_agent_host_core::ports::{ClaimRepository, InboxRepository, InsertOutcome, LeaseRepository};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{expire, insert_pending_event, insert_thread, lease_state, snapshot};
use uuid::Uuid;

const WORKERS: usize = 8;
const ROUNDS: usize = 150;
const EVENTS: usize = 200;
const ORG: Uuid = Uuid::from_u128(1);
const INBOX: Uuid = Uuid::from_u128(2);
const FOREIGN_INBOX: Uuid = Uuid::from_u128(3);
const THREAD: Uuid = Uuid::from_u128(10);
const FOREIGN_THREAD: Uuid = Uuid::from_u128(11);
/// The inbox of the sequential overlap phase. It is not the stress inbox, so the history checks
/// see only the stress events.
const SEQ_INBOX: Uuid = Uuid::from_u128(8);
const SEQ_EVENT: Uuid = Uuid::from_u128(0x2000);
/// The statement timeout of one heartbeat. It is shorter than the heartbeat interval (§8.6).
const HB_TIMEOUT: Duration = Duration::from_millis(500);
/// The upper bound of drain rounds. Each round completes one claim.
const DRAIN_ROUNDS: usize = 500;
/// How long a task that expired its own lease waits for another task to take the lease.
const TAKEOVER_WAIT: Duration = Duration::from_millis(100);

fn timing() -> KeeperTiming {
    KeeperTiming::new(Duration::from_secs(3), Duration::from_secs(1)).expect("timing")
}

fn cfg() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 5,
        max_attempts: 5,
        thread_ttl: Duration::from_secs(86400),
    }
}

fn inbox() -> InboxId {
    InboxId::new(ORG, INBOX)
}

/// A 64-bit linear congruential generator (Knuth's MMIX constants).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The seed from `LEASE_STRESS_SEED`, or else from the clock.
fn seed() -> u64 {
    if let Ok(s) = std::env::var("LEASE_STRESS_SEED") {
        return s.parse().expect("LEASE_STRESS_SEED is a u64");
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos();
    u64::try_from(nanos % u128::from(u64::MAX)).expect("fits")
}

/// One successful complete.
#[derive(Debug)]
struct Completed {
    run: Uuid,
    token: i64,
    done: Vec<Uuid>,
}

/// The history of one task.
#[derive(Debug, Default)]
struct Log {
    completes: Vec<Completed>,
    takes: usize,
    /// A complete that lost the lease after the task expired it.
    stale: usize,
    /// A stale complete that started while another task held a live lease with a newer token.
    /// Only the token half of the complete fence refuses it.
    stale_under_new_holder: usize,
    /// A stale complete that started while the lease row still had the own token, expired.
    /// The expiry half of the complete fence refuses it (unless a take came in between).
    stale_own_row: usize,
    /// A claim or a complete that lost the lease with no expire. Only a local deadline under
    /// heavy load can cause it. It is not a safety violation.
    lost_without_expire: usize,
}

/// After an expire, waits up to `TAKEOVER_WAIT` for another task to take the lease. It returns
/// true when the lease row then has a live lease with a token other than `own`. The caller
/// completes after this read, so a true result means that the new holder was live before the
/// stale complete started. (Its lease lives 3 s, far longer than one round.)
async fn wait_for_new_holder(db: &DatabaseConnection, own: i64) -> bool {
    let deadline = std::time::Instant::now() + TAKEOVER_WAIT;
    loop {
        if lease_state(db, INBOX).await.is_some_and(|(t, live)| live && t != own) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn worker(w: usize, store: Arc<PgLeaseStore>, db: DatabaseConnection, seed: u64) -> Log {
    let mut rng = Lcg(seed ^ (u64::try_from(w).expect("w") + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let holder = format!("w{w}");
    let mut log = Log::default();
    for _ in 0..ROUNDS {
        let Some(handle) = store.acquire(&inbox(), &holder, timing().lease_ttl).await.expect("acquire") else {
            tokio::time::sleep(Duration::from_millis(1 + rng.below(3))).await;
            continue;
        };
        log.takes += 1;
        let keeper = LeaseKeeper::start(handle, store.clone(), store.clone(), timing());
        match keeper.claim(&cfg()).await {
            Ok(Some(claim)) => {
                let expired = rng.below(4) == 0;
                let (run, token) = (claim.run.uuid(), claim.lease_token.0);
                // True when another task holds a live lease with a newer token at the stale
                // complete. Then only the token half of the complete fence can refuse it.
                let mut overlap = false;
                // True when the lease row still has the own token, read after the expire and
                // before the complete. Then the expiry half of the fence refuses it.
                let mut own_row = false;
                if expired {
                    expire(&db, INBOX).await;
                    if rng.below(2) == 0 {
                        overlap = wait_for_new_holder(&db, token).await;
                    } else {
                        own_row = lease_state(&db, INBOX).await.is_some_and(|(t, live)| !live && t == token);
                    }
                }
                match keeper.complete(claim).await {
                    Ok(done) => {
                        assert!(!expired, "seed {seed}: worker {w} completed run {run} after its lease was expired");
                        log.completes.push(Completed {
                            run,
                            token,
                            done: done.iter().map(EventId::uuid).collect(),
                        });
                    }
                    Err(CompleteError::LeaseLost) if expired => {
                        log.stale += 1;
                        if overlap {
                            log.stale_under_new_holder += 1;
                        }
                        if own_row {
                            log.stale_own_row += 1;
                        }
                    }
                    Err(CompleteError::LeaseLost) => log.lost_without_expire += 1,
                    Err(e) => panic!("seed {seed}: worker {w} complete: {e}"),
                }
            }
            Ok(None) => {}
            Err(ClaimError::LeaseLost) => log.lost_without_expire += 1,
            Err(e) => panic!("seed {seed}: worker {w} claim: {e}"),
        }
        keeper.release().await.expect("release");
    }
    log
}

/// Inserts `EVENTS` events: a third on the thread of the inbox, a third with no thread, a third
/// on a thread of another inbox. It returns their ids.
async fn writer(repo: PgInboxRepository) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(EVENTS);
    for n in 0..EVENTS {
        let id = Uuid::from_u128(0x1000 + u128::try_from(n).expect("n"));
        let thread = match n % 3 {
            0 => Some(THREAD),
            1 => None,
            _ => Some(FOREIGN_THREAD),
        };
        let event = NewInboxEvent {
            id: EventId::from_uuid(id),
            inbox: inbox(),
            source: "urn:test".to_string(),
            ce_id: id.to_string(),
            ce_type: "agents.x".to_string(),
            thread: thread.map(ThreadId::from_uuid),
            envelope: serde_json::json!({ "data": { "n": n } }),
        };
        let outcome = repo.insert_event(event).await.expect("insert event");
        assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(id) });
        ids.push(id);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    ids
}

/// One fresh worker with no keeper claims and completes until no event is claimable.
async fn drain(store: &PgLeaseStore, db: &DatabaseConnection) -> Vec<Completed> {
    let ttl = Duration::from_secs(60);
    let mut handle = match store.acquire(&inbox(), "drain", ttl).await.expect("drain acquire") {
        Some(handle) => handle,
        None => {
            expire(db, INBOX).await;
            store.acquire(&inbox(), "drain", ttl).await.expect("drain acquire").expect("a take after expire")
        }
    };
    let mut completes = Vec::new();
    for _ in 0..DRAIN_ROUNDS {
        let Some(claim) = store.claim(&mut handle, &cfg()).await.expect("drain claim") else {
            store.release(handle).await.expect("drain release");
            return completes;
        };
        let (run, token) = (claim.run.uuid(), claim.lease_token.0);
        let done = store.complete(&mut handle, claim).await.expect("drain complete");
        completes.push(Completed {
            run,
            token,
            done: done.iter().map(EventId::uuid).collect(),
        });
    }
    panic!("the drain did not end within {DRAIN_ROUNDS} rounds");
}

/// The sequential overlap phase. A claims under its own keeper, the test expires the lease, B
/// takes it and holds a live lease, and then A completes. Only the token half of the complete
/// fence refuses this complete. It gives one overlap on every run, so the coverage floor does
/// not depend on a random coincidence.
async fn sequential_overlap(url: &str, db: &DatabaseConnection) -> usize {
    insert_inbox(db, ORG, SEQ_INBOX, Uuid::from_u128(12), Uuid::from_u128(13)).await;
    insert_pending_event(db, SEQ_INBOX, SEQ_EVENT, 10).await;
    let seq = InboxId::new(ORG, SEQ_INBOX);
    let ttl = timing().lease_ttl;
    let store_a = Arc::new(PgLeaseStore::new(support::connect_pinned(url).await, HB_TIMEOUT));
    let store_b = PgLeaseStore::new(support::connect_pinned(url).await, HB_TIMEOUT);
    let a = store_a.acquire(&seq, "seq-a", ttl).await.expect("A acquire").expect("A takes the lease");
    let keeper = LeaseKeeper::start(a, store_a.clone(), store_a.clone(), timing());
    let claim = keeper.claim(&cfg()).await.expect("A claim").expect("A claims the event");
    let a_token = claim.lease_token.0;
    expire(db, SEQ_INBOX).await;
    let b = store_b.acquire(&seq, "seq-b", ttl).await.expect("B acquire").expect("B takes the expired lease");
    let state = lease_state(db, SEQ_INBOX).await.expect("lease row");
    assert!(state.1 && state.0 != a_token, "B does not hold a live lease with a new token: {state:?}");
    let before = snapshot(db, SEQ_INBOX).await;
    assert_eq!(keeper.complete(claim).await, Err(CompleteError::LeaseLost), "A completed under the live lease of B");
    assert_eq!(before, snapshot(db, SEQ_INBOX).await, "the stale complete of A changed a row");
    keeper.release().await.expect("A release");
    store_b.release(b).await.expect("B release");
    1
}

async fn rows(db: &DatabaseConnection, sql: &str) -> Vec<sea_orm::QueryResult> {
    db.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string())).await.expect(sql)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_workers_never_complete_one_event_twice() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    let seed = seed();
    eprintln!("lease_stress_pg: seed {seed} (replay with LEASE_STRESS_SEED={seed})");
    insert_inbox(&db, ORG, INBOX, Uuid::from_u128(4), Uuid::from_u128(5)).await;
    insert_inbox(&db, ORG, FOREIGN_INBOX, Uuid::from_u128(6), Uuid::from_u128(7)).await;
    insert_thread(&db, INBOX, THREAD).await;
    insert_thread(&db, FOREIGN_INBOX, FOREIGN_THREAD).await;

    let started = std::time::Instant::now();
    let writer = tokio::spawn(writer(PgInboxRepository::new(db.clone())));
    let mut tasks = Vec::with_capacity(WORKERS);
    for w in 0..WORKERS {
        let store = Arc::new(PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT));
        tasks.push(tokio::spawn(worker(w, store, db.clone(), seed)));
    }
    let inserted = writer.await.expect("writer");
    let mut logs = Vec::with_capacity(WORKERS);
    for task in tasks {
        logs.push(task.await.expect("worker"));
    }
    let drain_store = PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT);
    let drained = drain(&drain_store, &db).await;

    let worker_completes: usize = logs.iter().map(|l| l.completes.len()).sum();
    let sum = |f: fn(&Log) -> usize| logs.iter().map(f).sum::<usize>();
    let (takes, stale, lost) = (sum(|l| l.takes), sum(|l| l.stale), sum(|l| l.lost_without_expire));
    let (overlap, own_row) = (sum(|l| l.stale_under_new_holder), sum(|l| l.stale_own_row));
    eprintln!(
        "lease_stress_pg: seed {seed}, {:?}: takes {takes}, worker completes {worker_completes}, drain completes {}, \
         stale completes {stale} ({overlap} under a new holder, {own_row} on the own expired row), lost with no expire {lost}",
        started.elapsed(),
        drained.len()
    );
    let history: Vec<&Completed> = logs.iter().flat_map(|l| l.completes.iter()).chain(drained.iter()).collect();

    // No event id is in two successful completes.
    let mut done_by: HashMap<Uuid, Uuid> = HashMap::new();
    for c in &history {
        for id in &c.done {
            assert!(done_by.insert(*id, c.run).is_none(), "seed {seed}: event {id} is in two successful completes");
        }
    }
    // The recorded tokens are unique: one lease generation completes at most one run.
    let mut tokens = HashSet::new();
    for c in &history {
        assert!(tokens.insert(c.token), "seed {seed}: token {} completed two runs", c.token);
    }
    // Every recorded run row is `completed` with the recorded token, and no other run of the
    // inbox is `completed`.
    let completed: HashMap<Uuid, i64> = rows(&db, &format!("SELECT id, lease_token FROM run WHERE inbox_id = '{INBOX}' AND status = 'completed'"))
        .await
        .iter()
        .map(|r| (r.try_get("", "id").expect("id"), r.try_get("", "lease_token").expect("lease_token")))
        .collect();
    for c in &history {
        assert_eq!(completed.get(&c.run), Some(&c.token), "seed {seed}: run {} is not completed with token {}", c.run, c.token);
    }
    assert_eq!(completed.len(), history.len(), "seed {seed}: a run is completed that no recorded complete returned");
    // The `done` events are exactly the recorded ones, each marked by the run that returned it.
    let done_rows: HashMap<Uuid, Uuid> = rows(&db, &format!("SELECT id, claimed_run_id FROM inbox_event WHERE inbox_id = '{INBOX}' AND status = 'done'"))
        .await
        .iter()
        .map(|r| (r.try_get("", "id").expect("id"), r.try_get("", "claimed_run_id").expect("claimed_run_id")))
        .collect();
    assert_eq!(done_rows, done_by, "seed {seed}: the done events differ from the recorded completes");
    // Every claimed event is on the thread of its run (AG5).
    let mixed = support::scalar_i64(
        &db,
        &format!(
            "SELECT count(*) AS n FROM inbox_event e JOIN run r ON r.id = e.claimed_run_id \
             WHERE e.inbox_id = '{INBOX}' AND e.thread_id IS DISTINCT FROM r.thread_id"
        ),
    )
    .await;
    assert_eq!(mixed, 0, "seed {seed}: a claimed event is not on the thread of its run (AG5)");
    // After the drain every event is `done` or `dead` (AG2).
    let settled = support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{INBOX}' AND status IN ('done', 'dead')")).await;
    let total = support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{INBOX}'")).await;
    assert_eq!(total, i64::try_from(inserted.len()).expect("count"), "seed {seed}: the inbox holds other events");
    assert_eq!(settled, total, "seed {seed}: an event is not done or dead after the drain (AG2)");

    // The random overlaps depend on timing. The sequential phase adds one on every run.
    let random_overlap = overlap;
    let overlap = overlap + sequential_overlap(&url, &db).await;
    eprintln!("lease_stress_pg: seed {seed}: overlaps {overlap} ({random_overlap} random, 1 sequential)");

    // Coverage: the run did contend.
    assert!(worker_completes >= 1, "seed {seed}: coverage: no worker completed a claim");
    assert!(stale >= 1, "seed {seed}: coverage: no stale complete");
    assert!(overlap >= 1, "seed {seed}: coverage: no stale complete under a live new holder");
    // `own_row` is printed, not asserted: a take can come in before the read, and one run
    // measured only 1. `complete_pg` and `lease_property_pg` pin the expiry half of the fence.
}
