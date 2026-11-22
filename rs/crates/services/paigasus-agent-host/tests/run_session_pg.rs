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
use paigasus_agent_host::adapters::helikon::session::{PgRunSession, to_session_row};
use paigasus_agent_host::adapters::persistence::session_store::write_session;
use paigasus_agent_host_core::ids::ThreadId;
use paigasus_agent_host_core::run::SessionRow;
use paigasus_helikon::core::{ConversationSnapshot, SequenceId, Session, SessionError, SessionEvent};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, SqlErr, Statement, TransactionTrait};
use serde_json::json;
use support::inbox::{insert_inbox, migrated_postgres};
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
