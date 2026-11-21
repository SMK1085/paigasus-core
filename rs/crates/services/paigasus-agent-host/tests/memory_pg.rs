// SPDX-License-Identifier: Apache-2.0

//! The Postgres memory store against a real Postgres (spec §6.8): the conformance suite, the
//! schema guards, the caller-owned transaction of `apply_in`, and the scope row lock.

mod support;

use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host::adapters::persistence::Migrator;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host_core::memory::conformance::{self, fresh_scope};
use paigasus_agent_host_core::memory::{
    Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion, MemoryWorkspace, execute, parse_command,
};
use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryStore, MemoryUsage};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait, Value};
use sea_orm_migration::MigratorTrait;
use serde_json::json;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// A migrated Postgres: the container, its URL and a pool.
async fn migrated() -> Option<(ContainerAsync<Postgres>, String, DatabaseConnection)> {
    let (node, url) = support::start_raw_postgres().await?;
    let db = support::connect(&url).await;
    migrate_under_lock(&db, Duration::from_secs(60)).await.expect("migrate");
    Some((node, url, db))
}

fn p(raw: &str) -> MemoryPath {
    MemoryPath::parse(raw).expect("a valid test path")
}

fn put_new(path: &str, content: &str) -> MemoryChange {
    MemoryChange::Put {
        path: p(path),
        content: content.to_string(),
        expected: Expected::Absent,
    }
}

fn put_over(path: &str, content: &str, version: &str) -> MemoryChange {
    MemoryChange::Put {
        path: p(path),
        content: content.to_string(),
        expected: Expected::Version(MemoryVersion::new(version)),
    }
}

fn commit_set(scope: &MemoryScope, changes: Vec<MemoryChange>) -> MemoryCommitSet {
    let mut cs = MemoryChangeSet::new();
    for c in changes {
        cs.record(c);
    }
    MemoryCommitSet::new(scope.clone(), cs)
}

fn scope_values(scope: &MemoryScope) -> Vec<Value> {
    vec![scope.org().into(), scope.agent().uuid().into(), scope.principal().uuid().into()]
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Commit the scope row outside any test transaction.
async fn insert_scope_row(db: &DatabaseConnection, scope: &MemoryScope) {
    db.execute_raw(stmt(
        "INSERT INTO memory_scope (org_id, agent_id, principal_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        scope_values(scope),
    ))
    .await
    .expect("insert the scope row");
}

#[tokio::test]
async fn postgres_passes_the_conformance_suite() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    conformance::run_all(&PgMemoryStore::new(db)).await;
}

/// The row holds all three scope columns, and `size_bytes` is the UTF-8 byte length.
#[tokio::test]
async fn raw_row_has_all_scope_columns() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db.clone());
    let s = fresh_scope();
    // "é" is two bytes, so the byte length (6) differs from the character length (5).
    store.apply(&commit_set(&s, vec![put_new("/notes/a.md", "héllo")]), &MemoryCaps::default()).await.expect("put");

    let row = db
        .query_one_raw(stmt("SELECT org_id, agent_id, principal_id, path, size_bytes FROM memory_file WHERE org_id = $1", vec![s.org().into()]))
        .await
        .expect("query")
        .expect("one row");
    assert_eq!(row.try_get::<uuid::Uuid>("", "org_id").expect("org_id"), s.org());
    assert_eq!(row.try_get::<uuid::Uuid>("", "agent_id").expect("agent_id"), s.agent().uuid());
    assert_eq!(row.try_get::<uuid::Uuid>("", "principal_id").expect("principal_id"), s.principal().uuid());
    assert_eq!(row.try_get::<String>("", "path").expect("path"), "/notes/a.md");
    assert_eq!(row.try_get::<i32>("", "size_bytes").expect("size_bytes"), 6);
}

/// The `CHECK` refuses `.` and `..` segments. A valid path and a segment with inner dots pass, so
/// the refusals are not from the foreign key or from a broken insert.
#[tokio::test]
async fn check_refuses_traversal_rows() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let s = fresh_scope();
    insert_scope_row(&db, &s).await;
    let insert = |path: &str| {
        let mut values = scope_values(&s);
        values.push(path.into());
        stmt("INSERT INTO memory_file (org_id, agent_id, principal_id, path, content) VALUES ($1, $2, $3, $4, 'x')", values)
    };
    for good in ["/a/b", "/a..b"] {
        db.execute_raw(insert(good)).await.unwrap_or_else(|e| panic!("{good} must pass the CHECK: {e}"));
    }
    for bad in ["/../x", "/a/./b", "/a/..", "/."] {
        let err = db.execute_raw(insert(bad)).await.expect_err(bad).to_string();
        assert!(err.contains("check constraint"), "{bad} must fail the CHECK, got: {err}");
    }
}

/// `path` sorts in byte order because its collation is `C`.
#[tokio::test]
async fn path_column_collation_is_c() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let row = db
        .query_one_raw(stmt(
            "SELECT collation_name::text AS c FROM information_schema.columns WHERE table_name = 'memory_file' AND column_name = 'path'",
            vec![],
        ))
        .await
        .expect("query")
        .expect("the path column exists");
    assert_eq!(row.try_get::<String>("", "c").expect("collation"), "C");
}

/// `apply_in` writes in the caller's transaction. When the caller rolls back, nothing is written.
#[tokio::test]
async fn apply_in_rolls_back_with_the_caller() {
    let Some((_node, url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db.clone());
    let s = fresh_scope();
    let txn = db.begin().await.expect("begin");
    store.apply_in(&txn, &commit_set(&s, vec![put_new("/a", "1")]), &MemoryCaps::default()).await.expect("apply_in");
    txn.rollback().await.expect("rollback");

    let fresh = PgMemoryStore::new(support::connect(&url).await);
    assert_eq!(fresh.get(&s, &p("/a")).await.expect("get"), None);
    assert_eq!(fresh.usage(&s).await.expect("usage"), MemoryUsage::default());
}

/// A set with no change and no guard takes no scope row and no lock: a run with no memory change
/// does not wait for a console write of the same scope.
#[tokio::test]
async fn a_set_with_no_change_and_no_guard_takes_no_scope_row_and_no_lock() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db.clone());
    let s = fresh_scope();
    let empty = commit_set(&s, vec![]);
    store.apply(&empty, &MemoryCaps::default()).await.expect("apply");
    let txn = db.begin().await.expect("begin");
    store.apply_in(&txn, &empty, &MemoryCaps::default()).await.expect("apply_in");
    txn.commit().await.expect("commit");
    let rows = db
        .query_one_raw(stmt(
            "SELECT count(*)::int AS n FROM memory_scope WHERE org_id = $1 AND agent_id = $2 AND principal_id = $3",
            scope_values(&s),
        ))
        .await
        .expect("count")
        .expect("row");
    assert_eq!(rows.try_get::<i32>("", "n").expect("n"), 0);

    // With the scope row locked FOR UPDATE by another transaction, an empty apply does not wait.
    insert_scope_row(&db, &s).await;
    let b = db.begin().await.expect("begin B");
    b.query_one_raw(stmt(
        "SELECT 1 AS v FROM memory_scope WHERE org_id = $1 AND agent_id = $2 AND principal_id = $3 FOR UPDATE",
        scope_values(&s),
    ))
    .await
    .expect("lock in B")
    .expect("the scope row exists");
    tokio::time::timeout(Duration::from_secs(2), store.apply(&empty, &MemoryCaps::default()))
        .await
        .expect("an empty apply does not wait for the lock")
        .expect("apply");
    b.commit().await.expect("commit B");
}

/// A set with a guard and no change is applied: it takes the scope row and the lock.
#[tokio::test]
async fn a_guard_only_set_takes_the_scope_lock() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db.clone());
    let s = fresh_scope();
    let mut guarded = commit_set(&s, vec![]);
    guarded.guards.insert(MemoryDir::parse("/tasks").expect("a valid dir"));
    store.apply(&guarded, &MemoryCaps::default()).await.expect("a guard on an empty scope passes");
    let rows = db
        .query_one_raw(stmt(
            "SELECT count(*)::int AS n FROM memory_scope WHERE org_id = $1 AND agent_id = $2 AND principal_id = $3",
            scope_values(&s),
        ))
        .await
        .expect("count")
        .expect("row");
    assert_eq!(rows.try_get::<i32>("", "n").expect("n"), 1);
}

/// A directory rename conflicts when another writer adds a file under the directory after the
/// model viewed it.
#[tokio::test]
async fn a_directory_rename_conflicts_on_a_file_added_before_the_commit() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = Arc::new(PgMemoryStore::new(db));
    let s = fresh_scope();
    store.apply(&commit_set(&s, vec![put_new("/tasks/a.md", "a")]), &MemoryCaps::default()).await.expect("seed");
    let mut ws = MemoryWorkspace::new(store.clone(), s.clone(), MemoryCaps::default());
    execute(&mut ws, parse_command(json!({"command": "view", "path": "/memories/tasks"})).expect("parse"))
        .await
        .expect("view");
    execute(
        &mut ws,
        parse_command(json!({"command": "rename", "old_path": "/memories/tasks", "new_path": "/memories/done"})).expect("parse"),
    )
    .await
    .expect("rename");
    store.apply(&commit_set(&s, vec![put_new("/tasks/b.md", "b")]), &MemoryCaps::default()).await.expect("direct write");
    match ws.commit().await {
        Err(MemoryApplyError::Conflict { path }) => assert_eq!(path.as_str(), "/tasks/b.md"),
        other => panic!("expected a conflict at /tasks/b.md, got {other:?}"),
    }
    assert!(store.get(&s, &p("/tasks/a.md")).await.expect("get").is_some());
    assert!(store.get(&s, &p("/tasks/b.md")).await.expect("get").is_some());
    assert!(store.get(&s, &p("/done/a.md")).await.expect("get").is_none());
}

/// A conflict in `apply_in` rolls back to its savepoint. The caller's transaction stays usable,
/// and no part of the refused change set is written.
#[tokio::test]
async fn apply_in_conflict_keeps_the_caller_usable() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db.clone());
    let s = fresh_scope();
    let caps = MemoryCaps::default();
    store.apply(&commit_set(&s, vec![put_new("/a", "old")]), &caps).await.expect("setup");
    let current = store.get(&s, &p("/a")).await.expect("get").expect("exists").version;
    let stale = (current.as_str().parse::<i64>().expect("a numeric version") + 1_000_000).to_string();

    let txn = db.begin().await.expect("begin");
    // `/b` is written first, so a missing rollback to the savepoint would leave it behind.
    let result = store.apply_in(&txn, &commit_set(&s, vec![put_new("/b", "new"), put_over("/a", "new", &stale)]), &caps).await;
    match result {
        Err(MemoryApplyError::Conflict { path }) => assert_eq!(path.as_str(), "/a"),
        other => panic!("expected a conflict at /a, got {other:?}"),
    }
    let one = txn.query_one_raw(stmt("SELECT 1::int AS v", vec![])).await.expect("the caller's transaction is usable").expect("row");
    assert_eq!(one.try_get::<i32>("", "v").expect("v"), 1);
    txn.commit().await.expect("commit");

    assert_eq!(store.get(&s, &p("/b")).await.expect("get"), None);
    assert_eq!(store.get(&s, &p("/a")).await.expect("get").expect("exists").content, "old");
    assert_eq!(store.usage(&s).await.expect("usage"), MemoryUsage { files: 1, bytes: 3 });
}

/// A version that is not a number is a conflict, never a panic.
#[tokio::test]
async fn non_numeric_version_is_a_conflict() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let store = PgMemoryStore::new(db);
    let s = fresh_scope();
    let caps = MemoryCaps::default();
    store.apply(&commit_set(&s, vec![put_new("/a", "old")]), &caps).await.expect("setup");
    for change in [
        put_over("/a", "new", "not-a-number"),
        MemoryChange::Delete {
            path: p("/a"),
            expected: MemoryVersion::new("99999999999999999999"),
        },
    ] {
        match store.apply(&commit_set(&s, vec![change]), &caps).await {
            Err(MemoryApplyError::Conflict { path }) => assert_eq!(path.as_str(), "/a"),
            other => panic!("expected a conflict at /a, got {other:?}"),
        }
    }
    assert_eq!(store.get(&s, &p("/a")).await.expect("get").expect("exists").content, "old");
}

/// `apply` waits for the scope row lock.
///
/// Transaction B holds `FOR KEY SHARE` on the committed scope row. That mode conflicts with
/// `FOR UPDATE` only. The foreign key check of an insert into `memory_file` (`FOR KEY SHARE`) and
/// the `UPDATE` of the scope counts (`FOR NO KEY UPDATE`) do not conflict with it. So `apply`
/// waits here only because of its own `SELECT … FOR UPDATE`. If B held `FOR UPDATE`, the final
/// `UPDATE` would wait too, and the test would pass with the adapter's lock removed.
#[tokio::test]
async fn scope_lock_serializes_apply() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    let s = fresh_scope();
    insert_scope_row(&db, &s).await;

    let b = db.begin().await.expect("begin B");
    b.query_one_raw(stmt(
        "SELECT 1 AS v FROM memory_scope WHERE org_id = $1 AND agent_id = $2 AND principal_id = $3 FOR KEY SHARE",
        scope_values(&s),
    ))
    .await
    .expect("lock in B")
    .expect("the scope row exists");
    let b_pid = b
        .query_one_raw(stmt("SELECT pg_backend_pid() AS v", Vec::new()))
        .await
        .expect("the pid of B")
        .expect("a row")
        .try_get::<i32>("", "v")
        .expect("an int column named v");

    let store = PgMemoryStore::new(db.clone());
    let set = commit_set(&s, vec![put_new("/a", "1")]);
    let task = tokio::spawn(async move { store.apply(&set, &MemoryCaps::default()).await });

    // A row lock wait shows in `pg_locks` as an ungranted lock of another session that B blocks.
    // The query of that session must be the adapter's `SELECT … FROM memory_scope … FOR UPDATE`.
    // B's own session and this poll are not that session. The poll stops after 10 s.
    let waiting = stmt(
        "SELECT EXISTS (SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
         WHERE NOT l.granted AND l.pid <> $1 AND $1 = ANY (pg_blocking_pids(l.pid)) \
         AND a.query LIKE '%FROM memory_scope%FOR UPDATE%') AS v",
        vec![b_pid.into()],
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seen = db
            .query_one_raw(waiting.clone())
            .await
            .expect("read pg_locks")
            .expect("a row")
            .try_get::<bool>("", "v")
            .expect("a bool column named v");
        if seen {
            break;
        }
        assert!(!task.is_finished(), "apply finished without a wait for the scope row lock of B");
        assert!(tokio::time::Instant::now() < deadline, "apply did not wait for the scope row lock of B within 10 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!task.is_finished(), "apply must wait for the scope row lock of B");

    b.commit().await.expect("commit B");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("apply finishes within 5 s after B commits")
        .expect("the task does not panic")
        .expect("apply succeeds");
}

/// The `down` of m0002 drops both tables and the sequence, and leaves m0001 in place.
#[tokio::test]
async fn m0002_down_drops_its_objects() {
    let Some((_node, _url, db)) = migrated().await else {
        eprintln!("skipping memory_pg: Docker unavailable");
        return;
    };
    Migrator::down(&db, Some(1)).await.expect("down m0002");
    for object in ["memory_file", "memory_scope", "memory_file_version_seq"] {
        let sql = format!("SELECT to_regclass('{object}') IS NULL AS v");
        assert!(support::scalar_bool(&db, &sql).await, "{object} must be dropped");
    }
    assert!(support::scalar_bool(&db, "SELECT to_regclass('inbox') IS NOT NULL AS v").await, "m0001 must stay");
}
