// SPDX-License-Identifier: Apache-2.0

//! The Postgres lease adapter against a real database (spec §8.2, §8.3): the take, the fencing
//! token, the heartbeat, the release and `acquire_next`.

mod support;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::lease_store::{CANDIDATE_LIMIT, CANDIDATES_SQL, PgLeaseStore};
use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, RunId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::lease::HeartbeatError;
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, LeaseRepository};
use paigasus_agent_host_core::run::LeaseToken;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{backend_pid, expire, expires_us, insert_pending_event, insert_run, wait_until_blocked_by};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(30);
const HB_TIMEOUT: Duration = Duration::from_secs(5);

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn inbox_id(n: u128) -> InboxId {
    InboxId::new(u(1), u(n))
}

async fn count(db: &sea_orm::DatabaseConnection, sql: &str) -> i64 {
    support::scalar_i64(db, sql).await
}

/// The value of a counter with the label `operation`, from one snapshot.
fn lost_counter(recorder: &DebuggingRecorder, operation: &str) -> u64 {
    recorder
        .snapshotter()
        .snapshot()
        .into_vec()
        .into_iter()
        .filter_map(|(key, _, _, v)| {
            let DebugValue::Counter(n) = v else { return None };
            let op = key.key().labels().find(|l| l.key() == "operation")?.value().to_string();
            (key.key().name() == names::AGENT_HOST_LEASE_LOST_TOTAL && op == operation).then_some(n)
        })
        .sum()
}

#[tokio::test]
async fn the_first_take_of_a_new_inbox_has_token_1() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    assert_eq!(count(&db, "SELECT count(*) AS n FROM inbox_lease").await, 0);
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    assert_eq!(handle.token(), LeaseToken(1));
    assert_eq!(handle.holder(), "w1");
    assert_eq!(count(&db, "SELECT count(*) AS n FROM inbox_lease").await, 1);
}

#[tokio::test]
async fn a_second_take_while_live_is_none() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db, HB_TIMEOUT);
    let _first = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    assert!(store.acquire(&inbox_id(2), "w2", TTL).await.expect("acquire").is_none());
}

#[tokio::test]
async fn a_take_after_expiry_increments_the_token() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let _first = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    expire(&db, u(2)).await;
    let second = store.acquire(&inbox_id(2), "w2", TTL).await.expect("acquire").expect("some");
    assert_eq!(second.token(), LeaseToken(2));
    assert_eq!(count(&db, "SELECT count(*) AS n FROM inbox_lease WHERE holder = 'w2' AND token = 2").await, 1);
}

#[tokio::test]
async fn tokens_never_repeat_after_release() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db, HB_TIMEOUT);
    let first = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    assert_eq!(first.token(), LeaseToken(1));
    store.release(first).await.expect("release");
    let second = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    assert_eq!(second.token(), LeaseToken(2));
}

#[tokio::test]
async fn heartbeat_extends_the_lease() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&inbox_id(2), "w1", Duration::from_secs(10)).await.expect("acquire").expect("some");
    let before = expires_us(&db, u(2)).await;
    store.heartbeat(&handle, Duration::from_secs(60)).await.expect("heartbeat");
    assert!(expires_us(&db, u(2)).await > before);
}

#[tokio::test]
async fn heartbeat_after_expiry_is_lease_lost() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    expire(&db, u(2)).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.heartbeat(&handle, TTL).await, Err(HeartbeatError::LeaseLost));
    assert_eq!(lost_counter(&recorder, "heartbeat"), 1);
}

/// ADR-0028 (c): a heartbeat after `expires_at` does not revive the lease. Another transaction
/// holds the lease row with `FOR UPDATE` past the expiry and does not change the row. The
/// heartbeat waits for the lock (its statement timeout is longer than the wait), then it must give
/// `LeaseLost`, and `expires_at` must not move.
#[tokio::test]
async fn a_heartbeat_that_waits_past_the_expiry_is_lease_lost() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let ttl = Duration::from_secs(2);
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&inbox_id(2), "w1", ttl).await.expect("acquire").expect("some");
    let txn = db.begin().await.expect("begin");
    txn.execute_unprepared(&format!("SELECT token FROM inbox_lease WHERE inbox_id = '{}' FOR UPDATE", u(2)))
        .await
        .expect("lock the lease row");
    let locked_at = tokio::time::Instant::now();
    let holder_pid = backend_pid(&txn).await;
    let before = expires_us(&db, u(2)).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let beat = async {
        let result = store.heartbeat(&handle, Duration::from_secs(60)).await;
        (result, Instant::now())
    };
    let holder = async {
        // Guard: the heartbeat waits on the row lock before the release. A heartbeat that starts
        // after the release ends after it too, so the end time alone cannot prove a wait.
        wait_until_blocked_by(&db, holder_pid, Duration::from_secs(2)).await;
        tokio::time::sleep_until(locked_at + ttl + Duration::from_secs(1)).await;
        // The time before the commit is sent: the server can release the lock before the
        // client sees the commit reply, so a time taken after it can be later than the end of a
        // call that waited.
        let release = Instant::now();
        txn.commit().await.expect("commit the lock");
        release
    };
    let ((result, beat_done), released) = tokio::time::timeout(Duration::from_secs(15), async { tokio::join!(beat, holder) }).await.expect("the heartbeat ends");
    assert!(beat_done >= released, "the heartbeat did not wait for the row lock, so this test proves nothing");
    assert_eq!(result, Err(HeartbeatError::LeaseLost), "a heartbeat after the expiry revived the lease");
    assert_eq!(expires_us(&db, u(2)).await, before, "the heartbeat moved expires_at");
    assert_eq!(lost_counter(&recorder, "heartbeat"), 1);
}

#[tokio::test]
async fn heartbeat_with_a_stale_token_is_lease_lost() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let a = store.acquire(&inbox_id(2), "a", TTL).await.expect("acquire").expect("some");
    expire(&db, u(2)).await;
    let _b = store.acquire(&inbox_id(2), "b", TTL).await.expect("acquire").expect("some");
    let before = expires_us(&db, u(2)).await;
    assert_eq!(store.heartbeat(&a, Duration::from_secs(3600)).await, Err(HeartbeatError::LeaseLost));
    assert_eq!(expires_us(&db, u(2)).await, before);
}

#[tokio::test]
async fn a_database_error_is_store_not_lease_lost() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    db.close().await.expect("close");
    assert!(matches!(store.heartbeat(&handle, TTL).await, Err(HeartbeatError::Store(_))));
}

#[tokio::test]
async fn a_poisoned_handle_heartbeats_lease_lost_with_no_counter() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db, HB_TIMEOUT);
    let mut handle = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    handle.poison();
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.heartbeat(&handle, TTL).await, Err(HeartbeatError::LeaseLost));
    assert_eq!(lost_counter(&recorder, "heartbeat"), 0);
    store.release(handle).await.expect("release of a poisoned handle");
}

#[tokio::test]
async fn acquire_next_takes_the_inbox_with_the_oldest_pending_event() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(10), u(3), u(4)).await;
    insert_inbox(&db, u(1), u(20), u(30), u(4)).await;
    // The older event is in the inbox with the higher id, whose row and event are inserted last.
    // So only the ORDER BY of the query puts it first: id order and heap order both say u(10).
    insert_pending_event(&db, u(10), u(101), 9).await;
    insert_pending_event(&db, u(20), u(201), 10).await;
    // Set both times from ONE database time, so a slow second insert cannot make the event of
    // inbox 20 the newer one.
    db.execute_unprepared(&format!(
        "WITH t AS (SELECT now() AS t0) UPDATE inbox_event e SET received_at = t.t0 - CASE e.id WHEN '{}' THEN interval '9 seconds' ELSE interval '10 seconds' END, \
         not_before = t.t0 - interval '10 seconds' FROM t WHERE e.id IN ('{}', '{}')",
        u(101),
        u(101),
        u(201)
    ))
    .await
    .expect("set the event times");
    let store = PgLeaseStore::new(db, HB_TIMEOUT).serving(vec![AgentId::new(u(1), u(3)), AgentId::new(u(1), u(30))]);
    let first = store.acquire_next("w", TTL).await.expect("acquire_next").expect("some");
    assert_eq!(first.inbox(), &inbox_id(20));
    let second = store.acquire_next("w", TTL).await.expect("acquire_next").expect("some");
    assert_eq!(second.inbox(), &inbox_id(10));
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
}

#[tokio::test]
async fn acquire_next_skips_an_inbox_with_a_live_lease() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(10), u(3), u(4)).await;
    insert_pending_event(&db, u(10), u(101), 5).await;
    let store = PgLeaseStore::new(db, HB_TIMEOUT).serving(vec![AgentId::new(u(1), u(3))]);
    let _held = store.acquire(&inbox_id(10), "w1", TTL).await.expect("acquire").expect("some");
    assert!(store.acquire_next("w2", TTL).await.expect("acquire_next").is_none());
}

#[tokio::test]
async fn acquire_next_skips_an_inbox_whose_events_wait_on_not_before() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(10), u(3), u(4)).await;
    insert_pending_event(&db, u(10), u(101), 5).await;
    db.execute_unprepared("UPDATE inbox_event SET not_before = now() + interval '1 hour'").await.expect("update");
    let store = PgLeaseStore::new(db, HB_TIMEOUT).serving(vec![AgentId::new(u(1), u(3))]);
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acquire_next_on_many_inboxes_gives_one_holder_each() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    for n in 0..20u128 {
        insert_inbox(&db, u(1), u(100 + n), u(500 + n), u(4)).await;
        insert_pending_event(&db, u(100 + n), u(1000 + n), 5).await;
    }
    let mut tasks = Vec::new();
    for w in 0..8 {
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let conn = support::connect_pinned(&url).await;
            let store = PgLeaseStore::new(conn, HB_TIMEOUT).serving((0..20u128).map(|n| AgentId::new(u(1), u(500 + n))).collect());
            let mut taken = Vec::new();
            while let Some(handle) = store.acquire_next(&format!("w{w}"), Duration::from_secs(60)).await.expect("acquire_next") {
                taken.push(handle.inbox().uuid());
            }
            taken
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.expect("task"));
    }
    let distinct: HashSet<Uuid> = all.iter().copied().collect();
    assert_eq!(all.len(), 20, "every inbox is taken once: {all:?}");
    assert_eq!(distinct.len(), 20, "no inbox is taken twice");
}

#[tokio::test]
async fn an_open_acquirer_does_not_block_an_insert() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(10), u(3), u(4)).await;
    insert_pending_event(&db, u(10), u(101), 5).await;
    let conn1 = support::connect_pinned(&url).await;
    conn1.execute_unprepared("BEGIN").await.expect("begin");
    let limit = i64::try_from(CANDIDATE_LIMIT).expect("limit");
    let rows = conn1
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            CANDIDATES_SQL,
            [limit.into(), vec![u(1)].into(), vec![u(3)].into()],
        ))
        .await
        .expect("candidates");
    assert_eq!(rows.len(), 1, "the inbox is a candidate, and its row is now locked");

    let repo = PgInboxRepository::new(support::connect(&url).await);
    let event = NewInboxEvent {
        id: EventId::from_uuid(u(102)),
        inbox: inbox_id(10),
        source: "urn:test".to_string(),
        ce_id: "c2".to_string(),
        ce_type: "agents.x".to_string(),
        thread: None,
        envelope: serde_json::json!({}),
    };
    let outcome = tokio::time::timeout(Duration::from_secs(2), repo.insert_event(event)).await;
    conn1.execute_unprepared("ROLLBACK").await.expect("rollback");
    let outcome = outcome.expect("the insert must not wait for the open acquirer").expect("insert");
    assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(u(102)) });
}

#[tokio::test]
async fn release_closes_an_open_claim_and_expires_the_lease() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let mut handle = store.acquire(&inbox_id(2), "w1", TTL).await.expect("acquire").expect("some");
    insert_run(&db, u(2), u(50), u(60), handle.token().0).await;
    handle.record_claim(handle.token(), Some(RunId::new(u(1), u(50))));
    store.release(handle).await.expect("release");
    assert!(support::scalar_bool(&db, "SELECT (status = 'discarded') AS v FROM run WHERE id = '00000000-0000-0000-0000-000000000032'").await);
    assert!(support::scalar_bool(&db, "SELECT (finished_at IS NOT NULL) AS v FROM run WHERE id = '00000000-0000-0000-0000-000000000032'").await);
    assert!(support::scalar_bool(&db, "SELECT (expires_at < now()) AS v FROM inbox_lease WHERE inbox_id = '00000000-0000-0000-0000-000000000002'").await);
}

/// A release with a stale handle leaves the live lease of the new holder alone, and it leaves the
/// run of the new holder alone.
#[tokio::test]
async fn a_stale_release_leaves_the_new_lease_alone() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let mut a = store.acquire(&inbox_id(2), "a", TTL).await.expect("acquire").expect("some");
    assert_eq!(a.token(), LeaseToken(1));
    insert_run(&db, u(2), u(50), u(60), 1).await;
    a.record_claim(a.token(), Some(RunId::new(u(1), u(50))));
    expire(&db, u(2)).await;
    let mut b = store.acquire(&inbox_id(2), "b", TTL).await.expect("acquire").expect("some");
    assert_eq!(b.token(), LeaseToken(2));
    insert_run(&db, u(2), u(51), u(61), 2).await;
    b.record_claim(b.token(), Some(RunId::new(u(1), u(51))));
    let before = expires_us(&db, u(2)).await;
    store.release(a).await.expect("stale release");
    assert_eq!(expires_us(&db, u(2)).await, before, "the stale release moved the lease of b");
    assert_eq!(count(&db, "SELECT count(*) AS n FROM inbox_lease WHERE token = 2 AND expires_at > clock_timestamp()").await, 1);
    let run_b = format!("SELECT count(*) AS n FROM run WHERE id = '{}' AND status = 'running' AND finished_at IS NULL", u(51));
    assert_eq!(count(&db, &run_b).await, 1, "the stale release changed the run of b");
}
