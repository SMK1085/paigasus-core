// SPDX-License-Identifier: Apache-2.0

//! The worker pool on the real adapters (spec §6.1, §6.14): the candidate filter, the fairness of
//! `max_runs_per_lease`, a shutdown in the model loop, and the inbox lag gauge.

mod support;

use std::sync::Arc;
use std::time::Duration;

use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use paigasus_agent_host::adapters::agents::ConfigAgentResolver;
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::helikon::scripted::{CancelWaitingModel, ScriptedModel};
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::application::lease_keeper::KeeperTiming;
use paigasus_agent_host::application::worker::pool::{PoolSettings, run_worker_pool};
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::fault::NoFaults;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_agent_host_core::retry::RetryPolicy;
use paigasus_agent_host_core::screen::AllowAll;
use paigasus_observability::names;
use sea_orm::DatabaseConnection;
use support::commit::committer;
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{insert_pending_event, lease_state};
use tokio::sync::watch;
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(30);
const HB_TIMEOUT: Duration = Duration::from_millis(500);

/// One org with two inboxes: X of agent A and Y of agent B.
struct Two {
    org: Uuid,
    x: Uuid,
    y: Uuid,
    a: Uuid,
    b: Uuid,
}

async fn two_inboxes(db: &DatabaseConnection) -> Two {
    let t = Two {
        org: Uuid::new_v4(),
        x: Uuid::new_v4(),
        y: Uuid::new_v4(),
        a: Uuid::new_v4(),
        b: Uuid::new_v4(),
    };
    insert_inbox(db, t.org, t.x, t.a, Uuid::new_v4()).await;
    insert_inbox(db, t.org, t.y, t.b, Uuid::new_v4()).await;
    t
}

fn definition(org: Uuid, agent: Uuid) -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new(org, agent),
        name: "pool".to_owned(),
        model_id: "scripted".to_owned(),
        instructions: "Be brief.".to_owned(),
        tool_names: Vec::new(),
    }
}

/// A worker on the real adapters, with `model` registered as `scripted`.
fn worker(db: &DatabaseConnection, store: Arc<PgLeaseStore>, agents: Vec<AgentDefinition>, model: SharedModel, batch_limit: u32, max_runs_per_lease: u32) -> RunWorker {
    let mut models = ModelRegistry::default();
    models.register("scripted", model);
    let ports = WorkerPorts {
        leases: store.clone(),
        claims: store,
        committer: committer(db),
        resolver: Arc::new(ConfigAgentResolver::new(db.clone(), agents)),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(db.clone(), Arc::new(models))),
        memory: Arc::new(PgMemoryStore::new(db.clone())),
        faults: Arc::new(NoFaults),
        nonce: run_nonce,
    };
    let settings = WorkerSettings {
        claim: ClaimConfig {
            batch_limit,
            max_attempts: 5,
            thread_ttl: Duration::from_secs(86400),
        },
        timing: KeeperTiming::new(TTL, Duration::from_secs(10)).expect("timing"),
        limits: RunLimits {
            max_turns: 16,
            timeout: Duration::from_secs(60),
        },
        retry: RetryPolicy {
            base: Duration::from_secs(30),
            max: Duration::from_secs(1800),
        },
        max_runs_per_lease,
        caps: MemoryCaps::default(),
    };
    RunWorker::new(ports, settings)
}

fn pool_settings() -> PoolSettings {
    PoolSettings {
        concurrency: 1,
        poll_interval: Duration::from_millis(100),
        lease_ttl: TTL,
    }
}

/// Poll `sql` (a boolean column `v`) until it is true, for at most `limit`.
async fn wait_until(db: &DatabaseConnection, sql: &str, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    while !support::scalar_bool(db, sql).await {
        assert!(tokio::time::Instant::now() < deadline, "the condition did not hold within {limit:?}: {sql}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The value of the gauge `name` in one snapshot.
fn gauge(recorder: &DebuggingRecorder, name: &str) -> Option<f64> {
    recorder.snapshotter().snapshot().into_vec().into_iter().find_map(|(key, _, _, v)| match v {
        DebugValue::Gauge(g) if key.key().name() == name => Some(g.into_inner()),
        _ => None,
    })
}

#[tokio::test]
async fn an_inbox_with_no_configured_agent_is_never_taken() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    // Y has the older event, so without the filter Y is the first candidate.
    insert_pending_event(&db, t.x, Uuid::new_v4(), 5).await;
    insert_pending_event(&db, t.y, Uuid::new_v4(), 50).await;

    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving(vec![AgentId::new(t.org, t.a)]);
    let first = store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate");
    assert_eq!(first.inbox(), &InboxId::new(t.org, t.x));
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "Y has no configured agent");
    assert_eq!(lease_state(&db, t.y).await, None, "Y has no lease row");
}

#[tokio::test]
async fn a_store_that_serves_no_agent_takes_nothing() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 5).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "the default store serves no agent");
    assert_eq!(lease_state(&db, t.x).await, None);
}

/// F2: the receive order is X1 < Y1 < X2 < X3. With `max_runs_per_lease = 1`, the release after X1
/// lets Y1 (then the oldest due event) win, so Y runs while X still has pending events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_runs_per_lease_gives_a_second_inbox_a_turn() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    for (inbox, age) in [(t.x, 40), (t.y, 30), (t.x, 20), (t.x, 10)] {
        insert_pending_event(&db, inbox, Uuid::new_v4(), age).await;
    }
    let agents = vec![AgentId::new(t.org, t.a), AgentId::new(t.org, t.b)];
    let store = Arc::new(PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving(agents));
    let model = SharedModel::new(ScriptedModel::new(Vec::new()));
    let w = worker(&db, store.clone(), vec![definition(t.org, t.a), definition(t.org, t.b)], model, 1, 1);
    let (tx, rx) = watch::channel(());
    let pool = tokio::spawn(run_worker_pool(Arc::new(w), store, pool_settings(), "test".to_owned(), rx));

    let all_done = format!("SELECT count(*) = 4 AS v FROM inbox_event WHERE inbox_id IN ('{}', '{}') AND status = 'done'", t.x, t.y);
    wait_until(&db, &all_done, Duration::from_secs(60)).await;
    tx.send(()).expect("send the shutdown");
    tokio::time::timeout(Duration::from_secs(10), pool)
        .await
        .expect("the pool ends within 10 s")
        .expect("the pool task does not panic")
        .expect("the pool returns Ok");

    assert_eq!(support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{}'", t.y)).await, 1);
    // Concurrency 1 serves one run at a time: an X run that started after the Y run finished
    // means that X still had a pending event when Y was done.
    let later_x_runs = support::scalar_i64(
        &db,
        &format!(
            "SELECT count(*) AS n FROM run x WHERE x.inbox_id = '{}' \
             AND x.started_at > (SELECT finished_at FROM run WHERE inbox_id = '{}')",
            t.x, t.y
        ),
    )
    .await;
    assert!(later_x_runs >= 1, "X drained before Y had a turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_shutdown_in_the_model_loop_refunds_the_attempt() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    let event = Uuid::new_v4();
    insert_pending_event(&db, t.x, event, 5).await;
    let store = Arc::new(PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving(vec![AgentId::new(t.org, t.a)]));
    let w = worker(&db, store.clone(), vec![definition(t.org, t.a)], SharedModel::new(CancelWaitingModel), 20, 8);
    let (tx, rx) = watch::channel(());
    let pool = tokio::spawn(run_worker_pool(Arc::new(w), store, pool_settings(), "test".to_owned(), rx));

    let running = format!("SELECT count(*) = 1 AS v FROM run WHERE inbox_id = '{}' AND status = 'running'", t.x);
    wait_until(&db, &running, Duration::from_secs(30)).await;
    tx.send(()).expect("send the shutdown");
    tokio::time::timeout(Duration::from_secs(10), pool)
        .await
        .expect("the pool ends within 10 s")
        .expect("the pool task does not panic")
        .expect("the pool returns Ok");

    let discarded = support::scalar_bool(&db, &format!("SELECT bool_and(status = 'discarded') AND count(*) = 1 AS v FROM run WHERE inbox_id = '{}'", t.x)).await;
    assert!(discarded, "the run is not discarded");
    let refunded = support::scalar_bool(&db, &format!("SELECT status = 'pending' AND attempts = 0 AS v FROM inbox_event WHERE id = '{event}'")).await;
    assert!(refunded, "the attempt is not refunded");
    let (_, live) = lease_state(&db, t.x).await.expect("a lease row");
    assert!(!live, "the lease is still live");
}

#[tokio::test]
async fn the_lag_gauge_reads_the_oldest_due_event() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let t = two_inboxes(&db).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 30).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving(vec![AgentId::new(t.org, t.a)]);

    store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate");
    let lag = gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS).expect("the lag gauge is set");
    assert!((29.0..120.0).contains(&lag), "lag {lag}");

    // X now has a live lease, so no candidate is left: the gauge reads 0.
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
    assert_eq!(gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS), Some(0.0));
}
