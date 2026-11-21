// SPDX-License-Identifier: Apache-2.0

//! The minimal complete of the Postgres lease adapter against a real database (spec §8.5, G6, §11):
//! the fenced commit, the thread update and the `Queue` mid-run policy.

mod support;

use std::time::Duration;

use metrics_util::debugging::DebuggingRecorder;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::ids::{EventId, InboxId, ThreadId};
use paigasus_agent_host_core::lease::{CompleteError, LeaseHandle};
use paigasus_agent_host_core::ports::{ClaimRepository, LeaseRepository};
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

async fn take_as(store: &PgLeaseStore, holder: &str) -> LeaseHandle {
    store.acquire(&InboxId::new(u(1), u(INBOX)), holder, TTL).await.expect("acquire").expect("some")
}

async fn status_count(db: &DatabaseConnection, status: &str) -> i64 {
    count(db, &format!("SELECT count(*) AS n FROM inbox_event WHERE status = '{status}'")).await
}

/// An old thread (last run two days ago) with one pending event on it.
async fn insert_expired_thread_with_event(db: &DatabaseConnection, thread: Uuid, event: Uuid) {
    insert_thread(db, u(INBOX), thread).await;
    exec(db, &format!("UPDATE thread SET last_run_at = now() - interval '2 days' WHERE id = '{thread}'")).await;
    insert_event_on_thread(db, u(INBOX), event, 10, thread).await;
}

#[tokio::test]
async fn complete_marks_only_the_claimed_events_done() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    let run = claim.run.uuid();
    let mut done = store.complete(&mut handle, claim).await.expect("complete");
    // `UPDATE ... RETURNING` has no defined order, and the port does not promise one.
    done.sort_by_key(EventId::uuid);
    assert_eq!(done, vec![EventId::from_uuid(u(10)), EventId::from_uuid(u(11))]);
    assert_eq!(status_count(&db, "done").await, 2);
    assert_eq!(
        count(&db, &format!("SELECT count(*) AS n FROM run WHERE id = '{run}' AND status = 'completed' AND finished_at IS NOT NULL")).await,
        1
    );
    assert_eq!(handle.open_claim(), None);
}

#[tokio::test]
async fn complete_sets_last_run_at_and_the_successor() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_expired_thread_with_event(&db, u(70), u(10)).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    let new = claim.thread.uuid();
    assert_ne!(new, u(70));
    store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{new}' AND last_run_at IS NOT NULL")).await, 1);
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id = '{new}'", u(70))).await, 1);
}

/// `complete` updates the thread that is stored on the run, not the `thread` field of the
/// `Claim` value. A caller that changes that field to another thread of the inbox cannot set
/// `last_run_at` on it.
#[tokio::test]
async fn complete_updates_the_thread_of_the_run_not_of_the_claim_value() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    insert_thread(&db, u(INBOX), u(72)).await;
    let mut claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    let run_thread = claim.thread.uuid();
    claim.thread = ThreadId::from_uuid(u(72));
    store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{run_thread}' AND last_run_at IS NOT NULL")).await, 1);
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND last_run_at IS NULL", u(72))).await, 1);
}

#[tokio::test]
async fn complete_does_not_overwrite_a_successor() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_expired_thread_with_event(&db, u(70), u(10)).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    insert_thread(&db, u(INBOX), u(71)).await;
    exec(&db, &format!("UPDATE thread SET successor_id = '{}' WHERE id = '{}'", u(71), u(70))).await;
    store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM thread WHERE id = '{}' AND successor_id = '{}'", u(70), u(71))).await, 1);
}

#[tokio::test]
async fn a_stale_complete_is_lease_lost_and_changes_no_row() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    expire(&db, u(INBOX)).await;
    let b = take_as(&store, "w2").await;
    assert_eq!(b.token(), LeaseToken(2));
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.complete(&mut handle, claim).await, Err(CompleteError::LeaseLost));
    assert_eq!(status_count(&db, "pending").await, 1);
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 1);
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, Some(("operation", "complete"))), 1);
}

#[tokio::test]
async fn an_old_claim_on_a_new_handle_is_lease_lost() {
    let Some((_node, db, store, mut a)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let c1 = store.claim(&mut a, &cfg()).await.expect("claim").expect("some");
    expire(&db, u(INBOX)).await;
    let mut b = take_as(&store, "w2").await;
    let c2 = store.claim(&mut b, &cfg()).await.expect("claim").expect("some");
    assert_ne!(c1.run, c2.run);
    assert_eq!(store.complete(&mut b, c1).await, Err(CompleteError::LeaseLost));
    assert_eq!(status_count(&db, "pending").await, 1);
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 2);
    assert_eq!(count(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE claimed_run_id = '{}'", c2.run.uuid())).await, 1);
}

#[tokio::test]
async fn queue_keeps_a_mid_run_event_pending() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    let thread = claim.thread;
    insert_event_on_thread(&db, u(INBOX), u(11), 10, thread.uuid()).await;
    let done = store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(done, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&db, "pending").await, 1);
    let next = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    assert_eq!(next.lease_token, LeaseToken(2));
    assert_eq!(next.thread, thread);
    assert!(!next.fresh_thread);
    assert!(next.events.iter().map(|e| e.id).eq([EventId::from_uuid(u(11))]));
}

#[tokio::test]
async fn a_crash_retry_reports_a_fresh_thread() {
    let Some((_node, db, store, mut a)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let c1 = store.claim(&mut a, &cfg()).await.expect("claim").expect("some");
    expire(&db, u(INBOX)).await;
    let mut b = take_as(&store, "w2").await;
    let c2 = store.claim(&mut b, &cfg()).await.expect("claim").expect("some");
    assert_eq!(c2.thread, c1.thread);
    assert!(c2.fresh_thread);
    assert!(c2.events.iter().map(|e| e.id).eq([EventId::from_uuid(u(10))]));
    assert_eq!(count(&db, &format!("SELECT attempts::bigint AS n FROM inbox_event WHERE id = '{}'", u(10))).await, 2);
}

#[tokio::test]
async fn a_done_event_in_the_claim_is_not_marked_again() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    exec(&db, &format!("UPDATE inbox_event SET status = 'done' WHERE id = '{}'", u(11))).await;
    let done = store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(done, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&db, "done").await, 2);
}

/// The id list is a guard of its own: an event that the run claimed but the claim value does not
/// list stays pending. (`claimed_run_id` alone would mark it.)
#[tokio::test]
async fn complete_marks_only_the_ids_of_the_claim_value() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 20).await;
    insert_pending_event(&db, u(INBOX), u(11), 10).await;
    let mut claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    claim.events.truncate(1);
    let done = store.complete(&mut handle, claim).await.expect("complete");
    assert_eq!(done, vec![EventId::from_uuid(u(10))]);
    assert_eq!(status_count(&db, "pending").await, 1);
}

/// A COMMIT error leaves the state of the complete unknown, so it poisons the handle. A deferred
/// constraint trigger raises at COMMIT, after every statement of the complete succeeded.
#[tokio::test]
async fn a_failed_commit_poisons_the_handle() {
    let Some((_node, db, store, mut handle)) = setup().await else { return };
    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let claim = store.claim(&mut handle, &cfg()).await.expect("claim").expect("some");
    exec(
        &db,
        "CREATE FUNCTION refuse_run() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$",
    )
    .await;
    exec(
        &db,
        "CREATE CONSTRAINT TRIGGER refuse_run AFTER UPDATE ON run DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_run()",
    )
    .await;
    let first = store.complete(&mut handle, claim.clone()).await;
    assert!(matches!(first, Err(CompleteError::Store(_))), "{first:?}");
    assert!(handle.is_poisoned());
    // With the trigger gone, the same complete would commit. The poisoned handle sends no SQL.
    exec(&db, "DROP TRIGGER refuse_run ON run").await;
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    assert_eq!(store.complete(&mut handle, claim).await, Err(CompleteError::LeaseLost));
    assert_eq!(counter(&recorder, names::AGENT_HOST_LEASE_LOST_TOTAL, None), 0, "the poisoned path counts nothing");
    assert_eq!(status_count(&db, "pending").await, 1);
    assert_eq!(count(&db, "SELECT count(*) AS n FROM run WHERE status = 'running'").await, 1);
}
