// SPDX-License-Identifier: Apache-2.0

//! `RunWorker` on the real adapters (spec §6.1, §6.4, §6.8): `PgLeaseStore`, `PgRunCommitter`,
//! `ConfigAgentResolver`, `HelikonAgentRunner` with the scripted model, and `PgMemoryStore`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::agents::ConfigAgentResolver;
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::helikon::scripted::ScriptedModel;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::application::lease_keeper::KeeperTiming;
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{LeaseEnd, RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::fault::NoFaults;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryPath};
use paigasus_agent_host_core::ports::{LeaseRepository, MemoryStore};
use paigasus_agent_host_core::retry::RetryPolicy;
use paigasus_agent_host_core::screen::AllowAll;
use sea_orm::DatabaseConnection;
use serde_json::json;
use support::commit::{committer, scope_of};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::insert_pending_event;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SECRET: &str = "keep-this-secret";

/// One inbox of a configured agent, with one pending event.
struct Fixture {
    org: Uuid,
    inbox: Uuid,
    agent: Uuid,
    principal: Uuid,
    event: Uuid,
}

async fn fixture(db: &DatabaseConnection) -> Fixture {
    let f = Fixture {
        org: Uuid::new_v4(),
        inbox: Uuid::new_v4(),
        agent: Uuid::new_v4(),
        principal: Uuid::new_v4(),
        event: Uuid::new_v4(),
    };
    insert_inbox(db, f.org, f.inbox, f.agent, f.principal).await;
    insert_pending_event(db, f.inbox, f.event, 5).await;
    f
}

/// Store `/instructions.md` in the scope of the inbox, so the run start loads it.
async fn seed_instructions(db: &DatabaseConnection, f: &Fixture) {
    let mut changes = MemoryChangeSet::new();
    changes.record(MemoryChange::Put {
        path: MemoryPath::parse("/instructions.md").expect("path"),
        content: SECRET.to_owned(),
        expected: Expected::Absent,
    });
    let set = MemoryCommitSet::new(scope_of(f.org, f.agent, f.principal), changes);
    PgMemoryStore::new(db.clone()).apply(&set, &MemoryCaps::default()).await.expect("seed the memory");
}

/// A worker on the real adapters. The scripted model makes one memory create, then says `done`.
fn worker(db: &DatabaseConnection, store: Arc<PgLeaseStore>, f: &Fixture) -> RunWorker {
    let model = ScriptedModel::new(vec![vec![(
        "c1".into(),
        "memory".into(),
        json!({"command": "create", "path": "/memories/tasks/t1.md", "file_text": "step 1 done"}),
    )]]);
    let mut models = ModelRegistry::default();
    models.register("scripted", SharedModel::new(model));
    let agent = AgentDefinition {
        id: AgentId::new(f.org, f.agent),
        name: "worker".to_owned(),
        model_id: "scripted".to_owned(),
        instructions: "Use the memory tool.".to_owned(),
        tool_names: vec!["memory".to_owned()],
    };
    let ports = WorkerPorts {
        leases: store.clone(),
        claims: store,
        committer: committer(db),
        resolver: Arc::new(ConfigAgentResolver::new(db.clone(), vec![agent])),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(db.clone(), Arc::new(models))),
        memory: Arc::new(PgMemoryStore::new(db.clone())),
        faults: Arc::new(NoFaults),
        nonce: run_nonce,
    };
    let settings = WorkerSettings {
        claim: ClaimConfig {
            batch_limit: 20,
            max_attempts: 5,
            thread_ttl: Duration::from_secs(86400),
        },
        timing: KeeperTiming::new(Duration::from_secs(30), Duration::from_secs(10)).expect("timing"),
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

/// Take the lease of the inbox and serve it once.
async fn serve_once(db: &DatabaseConnection, f: &Fixture) -> LeaseEnd {
    let store = Arc::new(PgLeaseStore::new(db.clone(), Duration::from_millis(500)));
    let handle = store
        .acquire(&InboxId::new(f.org, f.inbox), "w1", Duration::from_secs(30))
        .await
        .expect("acquire")
        .expect("the lease is free");
    worker(db, store, f).serve(handle, &CancellationToken::new()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_text_never_reaches_session_event() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pg: Docker unavailable");
        return;
    };
    let f = fixture(&db).await;
    seed_instructions(&db, &f).await;

    assert_eq!(serve_once(&db, &f).await, LeaseEnd::Released);

    let done = support::scalar_bool(&db, &format!("SELECT status = 'done' AS v FROM inbox_event WHERE id = '{}'", f.event)).await;
    assert!(done, "the event is not done");
    let rows = support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{}'", f.inbox)).await;
    assert!(rows > 0, "the run wrote no session rows");
    let leaks = support::scalar_i64(
        &db,
        &format!(
            "SELECT count(*) AS n FROM session_event WHERE inbox_id = '{}' \
             AND (payload::text LIKE '%<memory-%' OR payload::text LIKE '%{SECRET}%')",
            f.inbox
        ),
    )
    .await;
    assert_eq!(leaks, 0, "the run-start memory text reached session_event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_spine_run_commits_once() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping worker_pg: Docker unavailable");
        return;
    };
    let f = fixture(&db).await;

    assert_eq!(serve_once(&db, &f).await, LeaseEnd::Released);

    let runs = support::scalar_i64(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{}'", f.inbox)).await;
    assert_eq!(runs, 1);
    let completed = support::scalar_bool(&db, &format!("SELECT status = 'completed' AND final_output = 'done' AS v FROM run WHERE inbox_id = '{}'", f.inbox)).await;
    assert!(completed, "the run is not completed with the output 'done'");
    let event_done = support::scalar_bool(&db, &format!("SELECT status = 'done' AND attempts = 1 AS v FROM inbox_event WHERE id = '{}'", f.event)).await;
    assert!(event_done, "the event is not done after one attempt");
    let file = support::scalar_bool(
        &db,
        &format!(
            "SELECT content = 'step 1 done' AS v FROM memory_file \
             WHERE org_id = '{}' AND agent_id = '{}' AND principal_id = '{}' AND path = '/tasks/t1.md'",
            f.org, f.agent, f.principal
        ),
    )
    .await;
    assert!(file, "the memory file of the run is not stored");
}
