// SPDX-License-Identifier: Apache-2.0

//! The claim of the Postgres lease adapter against a real database (spec §8.4, §8.4.1): the token
//! rotation, the dead events, the thread resolution, the grouping and the poisoned handle.

mod support;

use std::time::Duration;

use metrics_util::debugging::DebuggingRecorder;
use paigasus_agent_host::adapters::persistence::lease_store::{DEAD_REASON, PgLeaseStore};
use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::{EventId, InboxId, ThreadId};
use paigasus_agent_host_core::inbox::EventStatus;
use paigasus_agent_host_core::lease::{ClaimError, LeaseHandle};
use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_agent_host_core::run::LeaseToken;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{counter, expire, insert_event_on_thread, insert_pending_event, insert_thread};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(30);
const HB_TIMEOUT: Duration = Duration::from_secs(5);
const INBOX: u128 = 2;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn cfg() -> ClaimConfig {
    ClaimConfig {
        batch_limit: 20,
        max_attempts: 5,
        thread_ttl: Duration::from_secs(86400),
    }
}

async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
    support::scalar_i64(db, sql).await
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// A migrated database with one inbox, a store and a lease taken by `w1`.
async fn setup() -> Option<(
    testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    DatabaseConnection,
    PgLeaseStore,
    LeaseHandle,
)> {
    let (node, _url, db) = migrated_postgres().await?;
    insert_inbox(&db, u(1), u(INBOX), u(3), u(4)).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    let handle = store.acquire(&InboxId::new(u(1), u(INBOX)), "w1", TTL).await.expect("acquire").expect("some");
    Some((node, db, store, handle))
}

fn ids(claim: &Claim) -> Vec<Uuid> {
    claim.events.iter().map(|e| e.id.uuid()).collect()
}

/// True when the thread exists with exactly this predecessor (`None`: no predecessor).
async fn has_predecessor(db: &DatabaseConnection, thread: Uuid, want: Option<Uuid>) -> bool {
    let cond = want.map_or_else(|| "predecessor_id IS NULL".to_string(), |p| format!("predecessor_id = '{p}'"));
    count(db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{thread}' AND {cond}")).await == 1
}

#[tokio::test]
async fn the_first_claim_uses_the_take_token() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(claim.lease_token, LeaseToken(1));
    assert_eq!(handle.token(), LeaseToken(1));
}

#[tokio::test]
async fn a_second_claim_rotates_the_token() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Ok(None));
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(claim.lease_token, LeaseToken(2));
    assert_eq!(handle.token(), LeaseToken(2));
    assert_eq!(count(&db, &format!("SELECT token AS n FROM inbox_lease WHERE inbox_id = '{}'", u(INBOX))).await, 2);
}

#[tokio::test]
async fn a_claim_on_an_open_claim_is_claim_open() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Err(ClaimError::ClaimOpen));
    assert_eq!(count(&db, &format!("SELECT attempts::bigint AS n FROM inbox_event WHERE id = '{}'", u(10))).await, 1);
}

#[tokio::test]
async fn a_claim_after_expiry_is_lease_lost() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    expire(&db, u(INBOX)).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Err(ClaimError::LeaseLost));
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run").await, 0);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "claim"))), 1);
}

/// The rotation case of the expiry guard: a second claim after expiry is lost too.
#[tokio::test]
async fn a_rotation_after_expiry_is_lease_lost() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Ok(None));
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    expire(&db, u(INBOX)).await;
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Err(ClaimError::LeaseLost));
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run").await, 0);
    assert_eq!(count(&db, &format!("SELECT token AS n FROM inbox_lease WHERE inbox_id = '{}'", u(INBOX))).await, 1);
}

#[tokio::test]
async fn no_claimable_event_is_ok_none_and_keeps_the_lease() {
    let Some((_node, _db, store, mut handle)) = setup().await else { return };
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Ok(None));
    assert_eq!(handle.open_claim(), None);
    store.heartbeat(&handle, TTL).await.expect("heartbeat");
}

#[tokio::test]
async fn events_of_one_thread_are_claimed_together() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_thread(&db, u(INBOX), u(70)).await;
    insert_event_on_thread(&db, u(INBOX), u(10), 100, u(70)).await;
    insert_pending_event(&db, u(INBOX), u(11), 90).await;
    insert_event_on_thread(&db, u(INBOX), u(12), 80, u(70)).await;
    insert_pending_event(&db, u(INBOX), u(13), 70).await;
    insert_event_on_thread(&db, u(INBOX), u(14), 60, u(70)).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(ids(&claim), vec![u(10), u(12), u(14)]);
    assert_eq!(claim.thread, ThreadId::from_uuid(u(70)));
    assert!(claim.fresh_thread);
}

#[tokio::test]
async fn the_batch_limit_caps_the_claim() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    for (i, age) in [(10, 50), (11, 40), (12, 30), (13, 20), (14, 10)] {
        insert_pending_event(&db, u(INBOX), u(i), age).await;
    }
    let cfg = ClaimConfig { batch_limit: 2, ..cfg() };
    let claim = store.claim_events(&mut handle, &cfg).await.expect("claim").expect("some");
    assert_eq!(ids(&claim), vec![u(10), u(11)]);
}

#[tokio::test]
async fn no_thread_events_get_a_new_thread() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    let thread = claim.thread.uuid();
    assert!(has_predecessor(&db, thread, None).await);
    assert!(claim.fresh_thread);
    for event in &claim.events {
        assert_eq!(event.thread, Some(claim.thread));
    }
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE thread_id = '{thread}'")).await, 2);
}

#[tokio::test]
async fn an_expired_thread_gets_a_successor() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_thread(&db, u(INBOX), u(70)).await;
    exec(&db, &format!("UPDATE thread SET last_run_at = now() - interval '2 days' WHERE id = '{}'", u(70))).await;
    insert_event_on_thread(&db, u(INBOX), u(10), 10, u(70)).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    let new = claim.thread.uuid();
    assert_ne!(new, u(70));
    assert!(has_predecessor(&db, new, Some(u(70))).await);
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id IS NULL", u(70))).await, 1);
    assert!(claim.fresh_thread);
}

#[tokio::test]
async fn a_closed_thread_gets_a_successor() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_thread(&db, u(INBOX), u(70)).await;
    exec(&db, &format!("UPDATE thread SET closed = true WHERE id = '{}'", u(70))).await;
    insert_event_on_thread(&db, u(INBOX), u(10), 10, u(70)).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    let new = claim.thread.uuid();
    assert_ne!(new, u(70));
    assert!(has_predecessor(&db, new, Some(u(70))).await);
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id IS NULL", u(70))).await, 1);
}

#[tokio::test]
async fn a_successor_chain_resolves_to_its_end() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_thread(&db, u(INBOX), u(70)).await;
    insert_thread(&db, u(INBOX), u(71)).await;
    exec(&db, &format!("UPDATE thread SET closed = true, successor_id = '{}' WHERE id = '{}'", u(71), u(70))).await;
    insert_event_on_thread(&db, u(INBOX), u(10), 30, u(70)).await;
    insert_event_on_thread(&db, u(INBOX), u(11), 20, u(71)).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(claim.thread, ThreadId::from_uuid(u(71)));
    assert_eq!(ids(&claim), vec![u(10), u(11)]);
}

#[tokio::test]
async fn a_cycle_is_treated_as_no_thread() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_thread(&db, u(INBOX), u(70)).await;
    insert_thread(&db, u(INBOX), u(71)).await;
    exec(&db, &format!("UPDATE thread SET successor_id = '{}' WHERE id = '{}'", u(71), u(70))).await;
    exec(&db, &format!("UPDATE thread SET successor_id = '{}' WHERE id = '{}'", u(70), u(71))).await;
    insert_event_on_thread(&db, u(INBOX), u(10), 30, u(70)).await;
    insert_pending_event(&db, u(INBOX), u(11), 20).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(ids(&claim), vec![u(10), u(11)]);
    let new = claim.thread.uuid();
    assert!(new != u(70) && new != u(71));
    assert!(has_predecessor(&db, new, None).await);
}

#[tokio::test]
async fn not_before_in_the_future_is_not_claimed() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    exec(&db, &format!("UPDATE inbox_event SET not_before = now() + interval '1 hour' WHERE id = '{}'", u(10))).await;
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Ok(None));
}

#[tokio::test]
async fn exhausted_events_move_to_dead() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    exec(&db, &format!("UPDATE inbox_event SET attempts = 5 WHERE id = '{}'", u(10))).await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Ok(None));
    let sql = format!("SELECT count(*) AS n FROM inbox_event WHERE id = '{}' AND status = 'dead' AND last_error = '{DEAD_REASON}'", u(10));
    assert_eq!(count(&db, &sql).await, 1);
    assert_eq!(counter(&recorder, names::AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL, None), 1);
}

#[tokio::test]
async fn a_claim_marks_events_and_writes_run_event_rows() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let claim = store.claim_events(&mut handle, &cfg()).await.expect("claim").expect("some");
    let run = claim.run.uuid();
    assert_eq!(handle.open_claim(), Some(claim.run.clone()));
    for event in &claim.events {
        assert_eq!(event.attempts, 1);
        assert_eq!(event.status, EventStatus::Pending);
        assert_eq!(event.claimed_run.as_ref(), Some(&claim.run));
    }
    let marked = format!("SELECT count(*) AS n FROM inbox_event WHERE attempts = 1 AND status = 'pending' AND claimed_run_id = '{run}'");
    assert_eq!(count(&db, &marked).await, 2);
    let run_row = format!(
        "SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'running' AND lease_token = 1 AND thread_id = '{}'",
        claim.thread.uuid()
    );
    assert_eq!(count(&db, &run_row).await, 1);
    for id in [u(10), u(11)] {
        assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM run_event WHERE run_id = '{run}' AND event_id = '{id}'")).await, 1);
    }
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run_event").await, 2);
    assert!(claim.events.iter().map(|e| e.id).eq([EventId::from_uuid(u(10)), EventId::from_uuid(u(11))]));
}

#[tokio::test]
async fn a_failed_commit_poisons_the_handle() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    exec(
        &db,
        "CREATE FUNCTION refuse_run() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$",
    )
    .await;
    exec(
        &db,
        "CREATE CONSTRAINT TRIGGER refuse_run AFTER INSERT ON run DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_run()",
    )
    .await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let first = store.claim_events(&mut handle, &cfg()).await;
    assert!(matches!(first, Err(ClaimError::Store(_))), "{first:?}");
    assert!(handle.is_poisoned());
    assert_eq!(store.claim_events(&mut handle, &cfg()).await, Err(ClaimError::Poisoned));
    assert_eq!(counter(&recorder, names::AGENT_HOST_CLAIMS_TOTAL, None), 0);
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run").await, 0);
    exec(&db, "DROP TRIGGER refuse_run ON run").await;
}
