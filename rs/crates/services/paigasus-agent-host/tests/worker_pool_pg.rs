// SPDX-License-Identifier: Apache-2.0

//! The worker pool on the real adapters (spec §6.1, §6.14): the candidate filter, the fairness of
//! `max_runs_per_lease`, a shutdown in the model loop, and the inbox lag gauge.

mod support;

use std::sync::Arc;
use std::time::Duration;

use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use paigasus_agent_host::adapters::agents::PgAgentResolver;
use paigasus_agent_host::adapters::helikon::capabilities::HelikonCapabilities;
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::helikon::scripted::{CancelWaitingModel, ScriptedModel};
use paigasus_agent_host::adapters::iam::CallerCredential;
use paigasus_agent_host::adapters::id::MintIds;
use paigasus_agent_host::adapters::persistence::agent_registry::PgAgentRegistry;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::application::agents::{AgentAdmin, ResourceKey};
use paigasus_agent_host::application::auth::{Caller, CallerKind};
use paigasus_agent_host::application::lease_keeper::KeeperTiming;
use paigasus_agent_host::application::worker::pool::{PoolSettings, run_worker_pool};
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::fault::NoFaults;
use paigasus_agent_host_core::ids::{InboxId, PrincipalId};
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_agent_host_core::registry::AgentFields;
use paigasus_agent_host_core::retry::RetryPolicy;
use paigasus_agent_host_core::screen::AllowAll;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use support::commit::committer;
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::{insert_pending_event, lease_state};
use support::stub_iam::StubIam;
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

/// The store of the tests: it serves the model `scripted`.
fn store_of(db: &DatabaseConnection) -> PgLeaseStore {
    PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving_models(vec!["scripted".to_owned()])
}

/// A model registry that holds `model` as `scripted`.
fn registry_of(model: SharedModel) -> Arc<ModelRegistry> {
    let mut models = ModelRegistry::default();
    models.register("scripted", model);
    Arc::new(models)
}

/// A worker on the real adapters, with `model` registered as `scripted`.
fn worker(db: &DatabaseConnection, store: Arc<PgLeaseStore>, model: SharedModel, batch_limit: u32, max_runs_per_lease: u32) -> RunWorker {
    worker_with(db, store, registry_of(model), batch_limit, max_runs_per_lease)
}

fn worker_with(db: &DatabaseConnection, store: Arc<PgLeaseStore>, models: Arc<ModelRegistry>, batch_limit: u32, max_runs_per_lease: u32) -> RunWorker {
    let ports = WorkerPorts {
        leases: store.clone(),
        claims: store,
        committer: committer(db),
        resolver: Arc::new(PgAgentResolver::new(db.clone())),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(db.clone(), models)),
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

/// Archive the agent with plain SQL, with no worker involved.
async fn archive(db: &DatabaseConnection, agent: Uuid) {
    db.execute_unprepared(&format!("UPDATE agent SET archived_at = now() WHERE id = '{agent}'")).await.expect("archive");
}

#[tokio::test]
async fn an_inbox_of_an_archived_agent_is_not_taken_and_its_events_stay_pending() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    // X has the older event, so without the filter X is the first candidate.
    let (x_event, y_event) = (Uuid::new_v4(), Uuid::new_v4());
    insert_pending_event(&db, t.x, x_event, 50).await;
    insert_pending_event(&db, t.y, y_event, 5).await;
    archive(&db, t.a).await;

    let store = store_of(&db);
    let first = store.acquire_next("w", TTL).await.expect("acquire_next").expect("Y is a candidate");
    assert_eq!(first.inbox(), &InboxId::new(t.org, t.y));
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "X belongs to an archived agent");
    assert_eq!(lease_state(&db, t.x).await, None, "X has no lease row");
    assert!(
        support::scalar_bool(&db, &format!("SELECT status = 'pending' AS v FROM inbox_event WHERE id = '{x_event}'")).await,
        "the event of X stays pending"
    );
}

#[tokio::test]
async fn an_inbox_of_an_agent_whose_model_is_not_bound_is_not_taken() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 50).await;
    insert_pending_event(&db, t.y, Uuid::new_v4(), 5).await;
    db.execute_unprepared(&format!("UPDATE agent SET model = 'other' WHERE id = '{}'", t.a)).await.expect("set the model");

    let store = store_of(&db);
    let first = store.acquire_next("w", TTL).await.expect("acquire_next").expect("Y is a candidate");
    assert_eq!(first.inbox(), &InboxId::new(t.org, t.y));
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "X has a model that this store does not bind");
    assert_eq!(lease_state(&db, t.x).await, None, "X has no lease row");
}

#[tokio::test]
async fn a_store_that_binds_no_model_takes_nothing() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let t = two_inboxes(&db).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 5).await;
    let store = PgLeaseStore::new(db.clone(), HB_TIMEOUT);
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "the default store binds no model");
    let empty = PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving_models(Vec::new());
    assert!(empty.acquire_next("w", TTL).await.expect("acquire_next").is_none(), "an empty list binds no model");
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
    let store = Arc::new(store_of(&db));
    let model = SharedModel::new(ScriptedModel::new(Vec::new()));
    let w = worker(&db, store.clone(), model, 1, 1);
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
    let store = Arc::new(store_of(&db));
    let model = CancelWaitingModel::default();
    let w = worker(&db, store.clone(), SharedModel::new(model.clone()), 20, 8);
    let (tx, rx) = watch::channel(());
    let pool = tokio::spawn(run_worker_pool(Arc::new(w), store, pool_settings(), "test".to_owned(), rx));

    let running = format!("SELECT count(*) = 1 AS v FROM run WHERE inbox_id = '{}' AND status = 'running'", t.x);
    wait_until(&db, &running, Duration::from_secs(30)).await;
    // The run row commits with the claim, before the model call. Wait for the model call, so the
    // shutdown reaches the run in the model loop.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while model.invocations() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the run did not reach the model within 30 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    assert_eq!(model.invocations(), 1, "one model call, and none after the shutdown");
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
    // Y has an agent with a model that this store does not bind: its older event must not count.
    insert_pending_event(&db, t.y, Uuid::new_v4(), 300).await;
    db.execute_unprepared(&format!("UPDATE agent SET model = 'other' WHERE id = '{}'", t.b)).await.expect("set the model");
    let store = store_of(&db);

    store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate");
    let lag = gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS).expect("the lag gauge is set");
    assert!((29.0..120.0).contains(&lag), "the lag of X only, not of the unbound Y: {lag}");

    // X is leased now, so no candidate is left. Its event is still pending and due, so the gauge
    // still reads its age, not 0.
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
    let lag = gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS).expect("the lag gauge is set");
    assert!((29.0..120.0).contains(&lag), "the backlog of the leased X shows: {lag}");

    // A store that binds a model with no agent reads 0.
    let idle = PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving_models(vec!["no-such-model".to_owned()]);
    assert!(idle.acquire_next("w", TTL).await.expect("acquire_next").is_none());
    assert_eq!(gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS), Some(0.0));
}

/// The takeable event is 30 s old. An archived agent holds a 300 s old event, and an agent with an
/// unbound model holds a 600 s old one. Neither counts, on the path with a candidate row and on the
/// path with none.
#[tokio::test]
async fn both_lag_paths_leave_out_archived_and_unbound_inboxes() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let t = two_inboxes(&db).await;
    let (z_inbox, z_agent) = (Uuid::new_v4(), Uuid::new_v4());
    insert_inbox(&db, t.org, z_inbox, z_agent, Uuid::new_v4()).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 30).await;
    insert_pending_event(&db, t.y, Uuid::new_v4(), 300).await;
    insert_pending_event(&db, z_inbox, Uuid::new_v4(), 600).await;
    archive(&db, t.b).await;
    db.execute_unprepared(&format!("UPDATE agent SET model = 'other' WHERE id = '{z_agent}'")).await.expect("set the model");
    let store = store_of(&db);

    // The path with a candidate row: X is the candidate.
    store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate");
    let lag = gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS).expect("the lag gauge is set");
    assert!((29.0..120.0).contains(&lag), "the candidate path counts the age of X only: {lag}");

    // The path with no candidate row: X is leased, and Y and Z are filtered.
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
    let lag = gauge(&recorder, names::AGENT_HOST_INBOX_LAG_SECONDS).expect("the lag gauge is set");
    assert!((29.0..120.0).contains(&lag), "the no-candidate path counts the age of X only: {lag}");
}

#[tokio::test]
async fn the_archived_pending_gauge_counts_the_pending_events_of_an_archived_agent() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let recorder = DebuggingRecorder::new();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let t = two_inboxes(&db).await;
    insert_pending_event(&db, t.x, Uuid::new_v4(), 30).await;
    let done = Uuid::new_v4();
    for age in [300, 200, 100] {
        insert_pending_event(&db, t.y, Uuid::new_v4(), age).await;
    }
    insert_pending_event(&db, t.y, done, 400).await;
    db.execute_unprepared(&format!("UPDATE inbox_event SET status = 'done' WHERE id = '{done}'"))
        .await
        .expect("close an event");
    let store = store_of(&db);

    // No agent is archived: the gauge is 0, on the candidate path. Y has the older events, so Y
    // is the candidate.
    let first = store.acquire_next("w", TTL).await.expect("acquire_next").expect("Y is a candidate");
    assert_eq!(first.inbox(), &InboxId::new(t.org, t.y));
    assert_eq!(gauge(&recorder, names::AGENT_HOST_ARCHIVED_PENDING_EVENTS), Some(0.0));
    store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate");

    archive(&db, t.b).await;
    // Both inboxes are leased, so this poll has no candidate and reads the gauge on the second path.
    assert!(store.acquire_next("w", TTL).await.expect("acquire_next").is_none());
    assert_eq!(
        gauge(&recorder, names::AGENT_HOST_ARCHIVED_PENDING_EVENTS),
        Some(3.0),
        "the three pending events of Y; the done one does not count"
    );

    // Expire the lease of X, so the next poll has a candidate row again.
    db.execute_unprepared(&format!("UPDATE inbox_lease SET expires_at = now() - interval '1 second' WHERE inbox_id = '{}'", t.x))
        .await
        .expect("expire");
    store.acquire_next("w", TTL).await.expect("acquire_next").expect("X is a candidate again");
    assert_eq!(gauge(&recorder, names::AGENT_HOST_ARCHIVED_PENDING_EVENTS), Some(3.0), "the candidate path reads it too");
}

/// The done-when test (SMA-852): the pool runs with no agent row. An agent that `AgentAdmin`
/// creates later, with its inbox, is served without a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_created_while_the_pool_runs_is_served_without_a_restart() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pool_pg: Docker unavailable");
        return;
    };
    let models = registry_of(SharedModel::new(ScriptedModel::new(Vec::new())));
    let store = Arc::new(PgLeaseStore::new(db.clone(), HB_TIMEOUT).serving_models(models.ids()));
    let w = worker_with(&db, store.clone(), Arc::clone(&models), 20, 8);
    let (tx, rx) = watch::channel(());
    let pool = tokio::spawn(run_worker_pool(Arc::new(w), store, pool_settings(), "test".to_owned(), rx));
    // The pool polls an empty database for a moment.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM agent").await, 0, "the pool started with no agent row");

    let org = Uuid::new_v4();
    let principal = Uuid::new_v4();
    let admin = AgentAdmin::new(
        Arc::new(PgAgentRegistry::new(db.clone())),
        Arc::new(HelikonCapabilities::new(Arc::clone(&models))),
        Arc::new(StubIam::api_key(principal)),
        Arc::new(MintIds),
    );
    let caller = Caller {
        principal: PrincipalId::from_uuid(principal),
        credential: CallerCredential::Bearer(support::stub_iam::TOKEN.to_owned()),
        kind: CallerKind::Oidc { member_orgs: vec![org] },
    };
    let fields = AgentFields {
        name: "late".to_owned(),
        model_id: "scripted".to_owned(),
        instructions: "Be brief.".to_owned(),
        tool_names: Vec::new(),
    };
    let agent = admin.create(&caller, org, fields).await.expect("create the agent");
    let inbox = admin
        .create_inbox(
            &caller,
            ResourceKey {
                id: agent.definition.id.uuid(),
                org: Some(org),
            },
            PrincipalId::from_uuid(principal),
        )
        .await
        .expect("bind the inbox");
    let inbox = inbox.inbox.id().uuid();
    let event = Uuid::new_v4();
    insert_pending_event(&db, inbox, event, 1).await;

    let done = format!("SELECT status = 'done' AS v FROM inbox_event WHERE id = '{event}'");
    wait_until(&db, &done, Duration::from_secs(30)).await;
    tx.send(()).expect("send the shutdown");
    tokio::time::timeout(Duration::from_secs(10), pool)
        .await
        .expect("the pool ends within 10 s")
        .expect("the pool task does not panic")
        .expect("the pool returns Ok");
    assert_eq!(
        support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}' AND status = 'completed'")).await,
        1
    );
    assert_eq!(support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}'")).await, 1, "one run row");
}
