// SPDX-License-Identifier: Apache-2.0

//! The `LeaseKeeper` on a real `PgLeaseStore` (spec §8.2, §8.6): the heartbeat keeps the lease
//! alive in real time, and it sends the rotated token.

mod support;

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::application::lease_keeper::{KeeperTiming, LeaseKeeper};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::ids::InboxId;
use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_agent_host_core::run::LeaseToken;
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::insert_pending_event;
use uuid::Uuid;

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

#[tokio::test]
async fn a_keeper_survives_a_rotation() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(INBOX), u(3), u(4)).await;
    let timing = KeeperTiming::new(Duration::from_secs(3), Duration::from_secs(1)).expect("timing");
    // The statement timeout is shorter than the heartbeat interval (§8.6).
    let store = Arc::new(PgLeaseStore::new(db.clone(), Duration::from_millis(500)));
    let handle = store.acquire(&InboxId::new(u(1), u(INBOX)), "w1", timing.lease_ttl).await.expect("acquire").expect("some");
    assert_eq!(handle.token(), LeaseToken(1));
    let keeper = LeaseKeeper::start(handle, store.clone(), store.clone(), timing);
    let cancel = keeper.cancellation();

    insert_pending_event(&db, u(INBOX), u(10), 10).await;
    let first = keeper.claim(&cfg()).await.expect("claim 1").expect("some");
    assert_eq!(first.lease_token, LeaseToken(1), "the first claim after a take reuses the take token");
    keeper.complete(first).await.expect("complete 1");

    // Longer than the ttl: only the heartbeats keep the lease.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!cancel.is_cancelled(), "the keeper lost the lease while it slept");

    insert_pending_event(&db, u(INBOX), u(11), 1).await;
    let second = keeper.claim(&cfg()).await.expect("claim 2").expect("some");
    assert_eq!(second.lease_token, LeaseToken(2), "the second claim rotates the token");
    keeper.complete(second).await.expect("complete 2");

    // Longer than the ttl again: the heartbeats now send token 2.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!cancel.is_cancelled(), "the keeper lost the lease after the rotation");
    let token = support::scalar_i64(&db, &format!("SELECT token AS n FROM inbox_lease WHERE inbox_id = '{}'", u(INBOX))).await;
    assert_eq!(token, 2);
    let live = support::scalar_bool(&db, &format!("SELECT expires_at > clock_timestamp() AS v FROM inbox_lease WHERE inbox_id = '{}'", u(INBOX))).await;
    assert!(live, "the lease row is not live after the second sleep");

    keeper.release().await.expect("release");
    assert!(!cancel.is_cancelled());
}
