// SPDX-License-Identifier: Apache-2.0

//! The session store and `PgRunSession` against a real Postgres (spec §6.6).
//!
//! The Helikon sessions testkit runs in two setups: `PgRunSession` as it is (the appends stay in
//! the buffer), and a write-through wrapper whose `append` calls `write_session` in its own
//! transaction under a per-thread advisory lock and reads back through a new `PgRunSession`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use paigasus_agent_host::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::helikon::scripted::ScriptedModel;
use paigasus_agent_host::adapters::helikon::session::{PgRunSession, to_session_row};
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::session_store::write_session;
use paigasus_agent_host::application::worker::ports::{AgentRunner, RunEnd, RunLimits, RunSpec, UserTurn};
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::{AgentId, ThreadId};
use paigasus_agent_host_core::memory::conformance::fresh_scope;
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryWorkspace};
use paigasus_agent_host_core::run::SessionRow;
use paigasus_helikon::core::{ContentPart, ConversationSnapshot, SequenceId, Session, SessionError, SessionEvent};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, SqlErr, Statement, TransactionTrait};
use serde_json::json;
use support::inbox::{insert_inbox, migrated_postgres};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// One inbox, one thread and one `running` run of that thread, all with fresh ids.
struct Fixture {
    inbox: Uuid,
    thread: Uuid,
    run: Uuid,
}

async fn fixture(db: &DatabaseConnection) -> Fixture {
    let (inbox, thread, run) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    insert_inbox(db, Uuid::new_v4(), inbox, Uuid::new_v4(), Uuid::new_v4()).await;
    db.execute_unprepared(&format!("INSERT INTO thread (id, inbox_id) VALUES ('{thread}', '{inbox}')"))
        .await
        .expect("insert thread");
    db.execute_unprepared(&format!(
        "INSERT INTO run (id, inbox_id, thread_id, lease_token, status) VALUES ('{run}', '{inbox}', '{thread}', 1, 'running')"
    ))
    .await
    .expect("insert run");
    Fixture { inbox, thread, run }
}

fn row(n: usize) -> SessionRow {
    SessionRow {
        kind: "user_message".into(),
        payload: json!({"type": "user_message", "content": [{"type": "text", "text": format!("m{n}")}], "ts": "2026-11-22T00:00:00Z"}),
    }
}

async fn sequences(db: &DatabaseConnection, thread: Uuid) -> Vec<i64> {
    let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, "SELECT sequence FROM session_event WHERE thread_id = $1 ORDER BY sequence", [thread.into()]);
    db.query_all_raw(stmt)
        .await
        .expect("read sequences")
        .iter()
        .map(|r| r.try_get::<i64>("", "sequence").expect("sequence"))
        .collect()
}

/// The second setup of spec §6.6: every `append` is its own committed `write_session`.
struct WriteThrough {
    db: DatabaseConnection,
    fx: Fixture,
}

impl WriteThrough {
    fn reader(&self) -> PgRunSession {
        PgRunSession::new(self.db.clone(), ThreadId::from_uuid(self.fx.thread), false)
    }
}

#[async_trait]
impl Session for WriteThrough {
    async fn append(&self, events: &[SessionEvent]) -> Result<(), SessionError> {
        let rows: Vec<SessionRow> = events.iter().map(to_session_row).collect::<Result<_, _>>().map_err(SessionError::backend)?;
        let txn = self.db.begin().await.map_err(SessionError::backend)?;
        let lock = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))",
            [self.fx.thread.to_string().into()],
        );
        let written = async {
            txn.execute_raw(lock).await?;
            write_session(&txn, self.fx.inbox, self.fx.thread, self.fx.run, "N", &rows).await
        }
        .await;
        match written {
            Ok(_) => txn.commit().await.map_err(SessionError::backend),
            Err(e) => {
                txn.rollback().await.map_err(SessionError::backend)?;
                Err(SessionError::backend(e))
            }
        }
    }

    async fn events(&self, since: Option<SequenceId>) -> Result<Vec<SessionEvent>, SessionError> {
        self.reader().events(since).await
    }

    async fn snapshot(&self) -> Result<ConversationSnapshot, SessionError> {
        self.reader().snapshot().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_testkit_passes_against_the_buffered_session() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    paigasus_helikon_sessions_testkit::run_all(|| {
        let db = db.clone();
        async move { Arc::new(PgRunSession::new(db, ThreadId::from_uuid(Uuid::new_v4()), true)) as Arc<dyn Session> }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_testkit_passes_against_the_write_through_session() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    paigasus_helikon_sessions_testkit::run_all(|| {
        let db = db.clone();
        async move {
            let fx = fixture(&db).await;
            Arc::new(WriteThrough { db, fx }) as Arc<dyn Session>
        }
    })
    .await;
}

#[tokio::test]
async fn write_session_numbers_from_the_last_sequence() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let fx = fixture(&db).await;
    for rows in [vec![row(0), row(1)], vec![row(2), row(3), row(4)]] {
        let txn = db.begin().await.expect("begin");
        let n = write_session(&txn, fx.inbox, fx.thread, fx.run, "N", &rows).await.expect("write");
        assert_eq!(n, rows.len());
        txn.commit().await.expect("commit");
    }
    assert_eq!(sequences(&db, fx.thread).await, vec![0, 1, 2, 3, 4]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_writer_of_one_number_fails() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let fx = fixture(&db).await;
    let first = db.begin().await.expect("begin first");
    write_session(&first, fx.inbox, fx.thread, fx.run, "N", &[row(0)]).await.expect("first write");

    // The second writer reads `next = 0` (the first row is not committed) and then waits on the
    // primary key of the first, uncommitted row.
    let second = db.begin().await.expect("begin second");
    let (inbox, thread, run) = (fx.inbox, fx.thread, fx.run);
    let writer = tokio::spawn(async move {
        let result = write_session(&second, inbox, thread, run, "N", &[row(1)]).await;
        second.rollback().await.expect("rollback second");
        result
    });
    wait_for_a_lock_waiter(&db).await;
    first.commit().await.expect("commit first");

    let err: DbErr = writer.await.expect("join").expect_err("the second writer must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))), "expected 23505, got {err:?}");
    assert_eq!(sequences(&db, fx.thread).await, vec![0]);
}

/// Wait until one backend waits on a lock: then the second writer has read `next` and waits on
/// the insert.
async fn wait_for_a_lock_waiter(db: &DatabaseConnection) {
    for _ in 0..500 {
        if support::scalar_bool(db, "SELECT count(*) > 0 AS v FROM pg_stat_activity WHERE wait_event_type = 'Lock'").await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the second writer never waited on a lock");
}

#[tokio::test]
async fn an_unreadable_row_is_a_session_error() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let fx = fixture(&db).await;
    db.execute_unprepared(&format!(
        "INSERT INTO session_event (inbox_id, thread_id, sequence, run_id, kind, payload) \
         VALUES ('{}', '{}', 0, '{}', 'from_the_future', '{{\"type\": \"from_the_future\"}}')",
        fx.inbox, fx.thread, fx.run
    ))
    .await
    .expect("insert row");
    let err = PgRunSession::new(db, ThreadId::from_uuid(fx.thread), false).events(None).await.unwrap_err();
    assert!(matches!(err, SessionError::Backend(_)), "{err:?}");
}

fn buffered(text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        content: vec![ContentPart::Text { text: text.into() }],
        ts: jiff::Timestamp::from_second(1_700_000_000).expect("ts"),
    }
}

fn texts(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| match e {
            SessionEvent::UserMessage { content, .. } => match content.first() {
                Some(ContentPart::Text { text }) => text.clone(),
                other => panic!("expected a text part, got {other:?}"),
            },
            other => panic!("expected a user message, got {other:?}"),
        })
        .collect()
}

/// The path of every run after the first on a thread: committed rows and a buffer together.
/// Positions start at 0 over the combined list and `since` is exclusive, so `SequenceId(1)` (the
/// last committed row) gives the two buffered events.
#[tokio::test]
async fn the_buffer_follows_the_committed_events() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let fx = fixture(&db).await;
    let txn = db.begin().await.expect("begin");
    write_session(&txn, fx.inbox, fx.thread, fx.run, "N", &[row(0), row(1)]).await.expect("write");
    txn.commit().await.expect("commit");

    let session = PgRunSession::new(db, ThreadId::from_uuid(fx.thread), false);
    session.append(&[buffered("b0"), buffered("b1")]).await.expect("append");

    assert_eq!(texts(&session.events(None).await.expect("events")), ["m0", "m1", "b0", "b1"]);
    assert_eq!(texts(&session.events(Some(SequenceId(1))).await.expect("events since 1")), ["b0", "b1"]);
    assert_eq!(texts(&session.events(Some(SequenceId(2))).await.expect("events since 2")), ["b1"]);
    assert_eq!(session.snapshot().await.expect("snapshot").messages.len(), 4);

    assert_eq!(texts(&session.take_buffer()), ["b0", "b1"]);
    assert_eq!(texts(&session.events(None).await.expect("events after take")), ["m0", "m1"]);
}

/// An agent with the `memory` tool and the scripted model.
fn scripted_agent() -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new(Uuid::new_v4(), Uuid::new_v4()),
        name: "worker".to_owned(),
        model_id: "scripted".to_owned(),
        instructions: "Use the memory tool.".to_owned(),
        tool_names: vec!["memory".to_owned()],
    }
}

/// A runner whose registry holds `model` as `scripted`.
fn runner(db: &DatabaseConnection, model: ScriptedModel) -> HelikonAgentRunner {
    let mut models = ModelRegistry::default();
    models.register("scripted", SharedModel::new(model));
    HelikonAgentRunner::new(db.clone(), Arc::new(models))
}

/// A run spec on `thread` with a fresh workspace. It also returns the workspace.
fn run_spec(db: &DatabaseConnection, thread: Uuid, fresh_thread: bool, memory: Option<&str>) -> (RunSpec, Arc<tokio::sync::Mutex<MemoryWorkspace>>) {
    let store = Arc::new(PgMemoryStore::new(db.clone()));
    let workspace = Arc::new(tokio::sync::Mutex::new(MemoryWorkspace::new(store, fresh_scope(), MemoryCaps::default())));
    let spec = RunSpec {
        agent: scripted_agent(),
        thread: ThreadId::from_uuid(thread),
        fresh_thread,
        turn: UserTurn {
            memory: memory.map(str::to_owned),
            events: "<event-N n=\"0\">{}</event-N>".to_owned(),
        },
        workspace: workspace.clone(),
        cancel: CancellationToken::new(),
        limits: RunLimits {
            max_turns: 16,
            timeout: Duration::from_secs(60),
        },
    };
    (spec, workspace)
}

/// The number of text parts of a stored `user_message` row.
fn text_parts(row: &SessionRow) -> usize {
    row.payload["content"].as_array().expect("content").iter().filter(|p| p["type"] == "text").count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scripted_run_returns_its_session_with_the_memory_part() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let model = ScriptedModel::new(vec![vec![(
        "c1".into(),
        "memory".into(),
        json!({"command": "create", "path": "/memories/tasks/t1.md", "file_text": "step 1 done"}),
    )]]);
    let (spec, workspace) = run_spec(&db, Uuid::new_v4(), true, Some("<memory-N>notes</memory-N>"));
    let out = runner(&db, model).run(spec).await;

    match &out.end {
        RunEnd::Completed { output, .. } => assert_eq!(output, "done"),
        other => panic!("expected a completed run, got {other:?}"),
    }
    let first = out.session.first().expect("the session holds the user turn");
    assert_eq!(first.kind, "user_message");
    // `write_session` removes the memory part only at the write (spec §6.4).
    assert_eq!(text_parts(first), 2, "{:?}", first.payload);
    assert!(out.session.iter().any(|r| r.kind == "tool_called"), "the memory call is in the session: {:?}", out.session);
    assert_eq!(workspace.lock().await.take_changes().changes.len(), 1, "the memory create is buffered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_without_memory_text_has_one_user_text_part() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let (spec, _workspace) = run_spec(&db, Uuid::new_v4(), true, None);
    let out = runner(&db, ScriptedModel::new(vec![])).run(spec).await;
    assert!(matches!(out.end, RunEnd::Completed { .. }), "{:?}", out.end);
    assert_eq!(text_parts(out.session.first().expect("the user turn")), 1);
}

/// Review Focus 5 (F13): Helikon reports a failed session read as `RunError::Other`, so an
/// unreadable committed row ends the run as transient, before the model loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_read_error_is_transient() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let fx = fixture(&db).await;
    db.execute_unprepared(&format!(
        "INSERT INTO session_event (inbox_id, thread_id, sequence, run_id, kind, payload) \
         VALUES ('{}', '{}', 0, '{}', 'from_the_future', '{{\"type\": \"from_the_future\"}}')",
        fx.inbox, fx.thread, fx.run
    ))
    .await
    .expect("insert row");
    let (spec, _workspace) = run_spec(&db, fx.thread, false, None);
    let out = runner(&db, ScriptedModel::new(vec![])).run(spec).await;
    match &out.end {
        RunEnd::Transient { error, retry_after: None } => assert!(error.contains("from_the_future"), "the read error is the cause: {error}"),
        other => panic!("expected a transient end, got {other:?}"),
    }
    assert!(out.session.is_empty(), "{:?}", out.session);
}

#[tokio::test]
async fn an_unregistered_model_is_transient() {
    let Some((_node, _url, db)) = migrated_postgres().await else {
        eprintln!("skipping run_session_pg: Docker unavailable");
        return;
    };
    let (spec, _workspace) = run_spec(&db, Uuid::new_v4(), true, None);
    let out = HelikonAgentRunner::new(db.clone(), Arc::new(ModelRegistry::default())).run(spec).await;
    assert_eq!(
        out.end,
        RunEnd::Transient {
            error: "the model of the agent is not registered".to_owned(),
            retry_after: None
        }
    );
    assert!(out.session.is_empty());
}
