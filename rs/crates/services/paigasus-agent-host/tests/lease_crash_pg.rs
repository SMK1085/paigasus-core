// SPDX-License-Identifier: Apache-2.0

//! The crash test of the lease (spec §10.3). Worker A takes the lease with a real `LeaseKeeper`
//! and a short real ttl, and claims. Then the task of A is aborted (one case) or panics (one
//! case). The heartbeat stops, and the lease expires on the database clock. Worker B takes the
//! lease after the ttl, claims the same events again and completes them. A late complete with the
//! claim of A is `LeaseLost` and changes no row.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::application::lease_keeper::{KeeperTiming, LeaseKeeper};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::ids::InboxId;
use paigasus_agent_host_core::lease::{CompleteError, LeaseHandle};
use paigasus_agent_host_core::ports::{ClaimRepository, LeaseRepository};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{expires_us, insert_pending_event, snapshot};
use tokio::sync::oneshot;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(1);
const INBOX: Uuid = Uuid::from_u128(2);
const E1: Uuid = Uuid::from_u128(10);
const E2: Uuid = Uuid::from_u128(11);
/// The statement timeout of one heartbeat. It is shorter than the heartbeat interval (§8.6).
const HB_TIMEOUT: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(200);
const TAKEOVER_BUDGET: Duration = Duration::from_secs(15);

fn timing() -> KeeperTiming {
    KeeperTiming::new(Duration::from_secs(3), Duration::from_secs(1)).expect("timing")
}

fn cfg() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 20,
        max_attempts: 5,
        thread_ttl: Duration::from_secs(86400),
    }
}

#[derive(Debug, Clone, Copy)]
enum Crash {
    /// The test aborts the task of A while it waits on a pending future.
    Abort,
    /// The task of A panics.
    Panic,
}

async fn a_crashed_worker_is_taken_over(crash: Crash) {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, ORG, INBOX, Uuid::from_u128(3), Uuid::from_u128(4)).await;
    insert_pending_event(&db, INBOX, E1, 20).await;
    insert_pending_event(&db, INBOX, E2, 10).await;
    let inbox = InboxId::new(ORG, INBOX);
    let ttl = timing().lease_ttl;
    let store_a = Arc::new(PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT));
    let store_b = PgLeaseStore::new(support::connect_pinned(&url).await, HB_TIMEOUT);

    // Worker A: take, start the keeper, claim in a task.
    let handle = store_a.acquire(&inbox, "a", ttl).await.expect("A acquire").expect("A takes the lease");
    let a_token = handle.token();
    let keeper = LeaseKeeper::start(handle, store_a.clone(), store_a.clone(), timing());
    let (claim_tx, claim_rx) = oneshot::channel();
    let (crash_tx, crash_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let claim = keeper.claim(&cfg()).await.expect("A claim").expect("A claims events");
        claim_tx.send((claim, a_token)).expect("send the claim of A");
        match crash {
            Crash::Abort => std::future::pending::<()>().await,
            Crash::Panic => {
                let _ = crash_rx.await;
                panic!("worker A crashes mid-run");
            }
        }
        drop(keeper);
    });
    let (a_claim, a_token) = claim_rx.await.expect("the claim of A");
    assert_eq!(a_claim.lease_token, a_token, "the first claim after a take reuses the take token");
    let mut a_ids: Vec<Uuid> = a_claim.events.iter().map(|e| e.id.uuid()).collect();
    a_ids.sort();
    assert_eq!(a_ids, vec![E1, E2]);

    // Negative control: B cannot take the lease of A while it is live, also after one ttl, when
    // only the heartbeats of the keeper keep it.
    assert!(store_b.acquire(&inbox, "b", ttl).await.expect("B acquire").is_none(), "B took the live lease of A");
    tokio::time::sleep(ttl + Duration::from_secs(1)).await;
    assert!(store_b.acquire(&inbox, "b", ttl).await.expect("B acquire").is_none(), "B took the lease while the keeper of A ran");

    // The expiry of A's lease on the database clock, read just before the crash. A heartbeat
    // after this read can only make the real expiry later, so the value is a lower bound. It is
    // read before the crash, so a crash cleanup that shortens `expires_at` cannot lower it.
    let a_expiry_us = expires_us(&db, INBOX).await;

    // The crash.
    match crash {
        Crash::Abort => {
            task.abort();
            assert!(task.await.expect_err("the task of A is aborted").is_cancelled());
        }
        Crash::Panic => {
            crash_tx.send(()).expect("signal the crash");
            assert!(task.await.expect_err("the task of A panics").is_panic());
        }
    }

    // The crash must not shorten the lease: the stored expiry after the crash is not earlier.
    let a_expiry_after_crash_us = expires_us(&db, INBOX).await;
    assert!(a_expiry_after_crash_us >= a_expiry_us, "the crash shortened A's lease by {} us", a_expiry_us - a_expiry_after_crash_us);

    // B polls until the lease of A expires on the database clock.
    let crashed = Instant::now();
    let mut polls = 0usize;
    let mut b = loop {
        if let Some(h) = store_b.acquire(&inbox, "b", ttl).await.expect("B acquire") {
            break h;
        }
        polls += 1;
        assert!(crashed.elapsed() < TAKEOVER_BUDGET, "B did not take the lease within {TAKEOVER_BUDGET:?} after A crashed");
        tokio::time::sleep(POLL).await;
    };
    // The take of B must come after A's expiry on the database clock. The take sets `expires_at`
    // to its clock plus the ttl, so the take time is that value minus the ttl. This does not
    // depend on the number of polls: a test that resumes late can take the lease on its first poll.
    let ttl_us = i64::try_from(ttl.as_micros()).expect("ttl in microseconds");
    let b_take_us = expires_us(&db, INBOX).await - ttl_us;
    assert!(
        b_take_us >= a_expiry_us,
        "B took the lease {} us before A's lease expired: the crash released it, the ttl did not expire it",
        a_expiry_us - b_take_us
    );
    eprintln!("{crash:?}: B took the lease {:?} after the crash, after {polls} polls", crashed.elapsed());
    assert!(b.token().0 > a_token.0, "the take of B has a larger token");

    // B claims the same events on the same thread, as a second attempt.
    let b_claim = store_b.claim(&mut b, &cfg()).await.expect("B claim").expect("B claims events");
    let mut b_ids: Vec<Uuid> = b_claim.events.iter().map(|e| e.id.uuid()).collect();
    b_ids.sort();
    assert_eq!(b_ids, a_ids, "B claims the events of A");
    assert!(b_claim.events.iter().all(|e| e.attempts == 2), "{:?}", b_claim.events.iter().map(|e| e.attempts).collect::<Vec<_>>());
    assert_eq!(b_claim.thread, a_claim.thread);
    assert!(b_claim.fresh_thread);
    let done = store_b.complete(&mut b, b_claim).await.expect("B complete");
    assert_eq!(done.len(), 2);
    let done_count = support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{INBOX}' AND status = 'done'")).await;
    assert_eq!(done_count, 2, "every event is done");

    // A late complete with the claim of A is `LeaseLost` and changes no row.
    let mut late = LeaseHandle::new(inbox.clone(), a_token, "a".to_string());
    late.record_claim(a_token, Some(a_claim.run.clone()));
    let before = snapshot(&db, INBOX).await;
    assert_eq!(store_a.complete(&mut late, a_claim).await, Err(CompleteError::LeaseLost));
    assert_eq!(before, snapshot(&db, INBOX).await, "the late complete of A changed a row");
    store_b.release(b).await.expect("B release");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aborted_worker_is_taken_over_after_the_ttl() {
    a_crashed_worker_is_taken_over(Crash::Abort).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicked_worker_is_taken_over_after_the_ttl() {
    a_crashed_worker_is_taken_over(Crash::Panic).await;
}
