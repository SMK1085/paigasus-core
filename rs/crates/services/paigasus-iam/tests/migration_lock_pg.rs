// SPDX-License-Identifier: Apache-2.0

//! The SMA-559 migration advisory lock, proven against a real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker; the skip-versus-panic decision lives once, in
//! `tests/support/docker.rs::start_or_skip`, reached through `support::start_raw_postgres`.
//!
//! Every test takes the CONTAINER from `start_raw_postgres` and discards its returned handle —
//! that handle is pinned to a single connection, so reusing it for two concurrent migrators
//! would make the second block on the pool rather than on the advisory lock. And nothing here
//! may use `start_migrated_postgres`, which has already migrated.

mod support;

use std::time::Duration;

use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::adapters::persistence::migration::m0008_partition_audit_log::AUDIT_PARTITION_LOCK_KEY;
use paigasus_iam::adapters::persistence::migration_lock::{MIGRATION_LOCK_KEY, MigrationLockError, POLL_BACKOFF, migrate_under_lock};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

/// A fresh multi-connection pool onto the same container. NOT `start_raw_postgres`'s handle.
async fn connect(url: &str) -> DatabaseConnection {
    Database::connect(url.to_string()).await.expect("connect")
}

/// A pool pinned to ONE physical connection, so a session-level advisory lock taken on it stays
/// held across statements.
async fn connect_pinned(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.max_connections(1).min_connections(1);
    Database::connect(opts).await.expect("connect pinned")
}

async fn scalar_bool(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .and_then(|r| r.try_get::<bool>("", "v").ok())
        .expect("a bool column named v")
}

async fn migrations_table_exists(db: &DatabaseConnection) -> bool {
    scalar_bool(db, "SELECT to_regclass('public.seaql_migrations') IS NOT NULL AS v").await
}

async fn applied_migrations(db: &DatabaseConnection) -> i64 {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT count(*)::bigint AS n FROM seaql_migrations".to_string()))
        .await
        .expect("query")
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .expect("a count")
}

/// AC 1 and AC 2: two migrators, one database, no deadlock, one converged schema.
///
/// `tokio::time::timeout` IS the deadlock assertion — the same technique
/// `outbox_retention_concurrency_pg.rs` uses.
#[tokio::test]
async fn two_concurrent_migrators_converge_without_deadlocking() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let a = connect(&url).await;
    let b = connect(&url).await;

    // Guards against someone swapping in `start_migrated_postgres`, which would make every
    // assertion below pass trivially.
    assert!(!migrations_table_exists(&a).await, "the container must start UNMIGRATED — did this switch to start_migrated_postgres?");

    let wait = Duration::from_secs(120);
    let joined = tokio::time::timeout(Duration::from_secs(300), async { tokio::join!(migrate_under_lock(&a, wait), migrate_under_lock(&b, wait)) })
        .await
        .expect("concurrent migrations deadlocked");

    let (ra, rb) = joined;
    let oa = ra.expect("migrator A must exit cleanly");
    let ob = rb.expect("migrator B must exit cleanly");

    assert_eq!(applied_migrations(&a).await as usize, Migrator::migrations().len(), "the schema must be at the tip");
    assert!(
        scalar_bool(&a, "SELECT relkind = 'p' AS v FROM pg_class WHERE relname = 'audit_log'").await,
        "audit_log must be partitioned (m0008)"
    );

    // Exactly one side did the work. This is what distinguishes real serialisation from
    // both-ran-and-one-lost.
    let workers = [&oa, &ob].iter().filter(|o| o.migrations_applied > 0).count();
    assert_eq!(workers, 1, "exactly one migrator should apply migrations, got {oa:?} and {ob:?}");
}

/// The guard is load-bearing: with the lock held, the migration must not proceed AT ALL.
///
/// Delete `pg_try_advisory_xact_lock` from `migrate_under_lock` and `Migrator::up`'s own
/// `install()` creates `seaql_migrations`, failing the second assertion.
#[tokio::test]
async fn a_held_lock_blocks_the_migration_and_leaves_the_database_untouched() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    let db = connect(&url).await;

    // `pg_try_advisory_lock`, not `pg_advisory_lock`: the latter returns void and so cannot
    // assert its own setup, and a holder that silently failed would make this test vacuous.
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let err = migrate_under_lock(&db, Duration::from_secs(1)).await.expect_err("must not migrate while the lock is held");
    assert!(matches!(err, MigrationLockError::Contended { .. }), "expected Contended, got {err:?}");
    assert!(!migrations_table_exists(&db).await, "a blocked migration must leave the database untouched");

    assert!(
        scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually release the lock"
    );

    migrate_under_lock(&db, Duration::from_secs(120)).await.expect("must migrate once the lock is free");
    assert_eq!(applied_migrations(&db).await as usize, Migrator::migrations().len());
}

/// The wait-then-acquire path AC 1 actually asks for.
///
/// Asserts on the OUTCOME, not wall clock: an advisory lock does not block DDL, so with the
/// guard deleted a bare `Migrator::up` would also return `Ok` after the hold elapsed, and a
/// wall-clock assertion would still pass.
///
/// **Both timing assertions are causal, not coincidental** (SMA-582). The earlier version held
/// for a flat 500ms and asserted `waited >= 500ms`, which quietly depended on the waiter reaching
/// its first `pg_try_advisory_xact_lock` within that window. Under this crate's documented Docker
/// contention that is not safe — `outbox_retention_concurrency_pg.rs` records an in-container 5s
/// `lock_timeout` inflating to 21.3s of wall clock under a full-crate run. A stall longer than the
/// hold would let the first poll SUCCEED, and `polls >= 1` would fail for a reason unrelated to
/// the behaviour under test. So:
///
/// * the hold is `2 * POLL_BACKOFF`, giving the first poll a two-backoff window to find the lock
///   held rather than a 500ms one, and
/// * the magnitude assertion is `waited >= POLL_BACKOFF`, which FOLLOWS from having polled at all:
///   `wait` is far larger than one backoff here, so `next_poll` never clamps and each retry costs
///   a full `POLL_BACKOFF`. It no longer races the releaser's timer.
#[tokio::test]
async fn a_waiter_waits_and_then_migrates() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    let db = connect(&url).await;

    // Long enough that a stalled first poll still finds the lock held; derived from the
    // production constant so a backoff change cannot silently invalidate the window.
    let hold = 2 * POLL_BACKOFF;

    assert!(scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await);

    let releaser = tokio::spawn(async move {
        tokio::time::sleep(hold).await;
        assert!(scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await);
    });

    let outcome = migrate_under_lock(&db, Duration::from_secs(30)).await.expect("must succeed once the holder releases");
    releaser.await.expect("releaser");

    assert!(outcome.polls >= 1, "the waiter must have polled at least once, got {outcome:?}");
    assert!(outcome.waited >= POLL_BACKOFF, "a waiter that polled must have slept at least one full backoff, got {outcome:?}");
    assert!(outcome.migrations_applied > 0, "the waiter must have applied the migrations, got {outcome:?}");
}

/// The claim that justifies transaction-scoped over session-scoped: a FAILED migration still
/// releases the lock.
///
/// Release is asserted from an INDEPENDENT session. Re-calling `migrate_under_lock` would fail
/// identically on the still-present conflicting object and so would prove nothing.
#[tokio::test]
async fn a_failed_migration_still_releases_the_lock() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let db = connect(&url).await;
    let observer = connect_pinned(&url).await;

    // m0001's `CREATE TABLE principal` is `.if_not_exists()`, so this seed is silently accepted
    // rather than colliding directly — the migration instead fails later, when a subsequent
    // migration's foreign key references `principal.id`, a column this bogus table lacks
    // (observed as Postgres 42703, "column \"id\" referenced in foreign key constraint does not
    // exist").
    db.execute_unprepared("CREATE TABLE principal (bogus int);").await.expect("seed the conflict");

    let err = migrate_under_lock(&db, Duration::from_secs(10)).await.expect_err("the migration must fail");
    assert!(!matches!(err, MigrationLockError::Contended { .. }), "expected a migration error, not Contended: {err:?}");
    assert!(!migrations_table_exists(&db).await, "the whole migration transaction must roll back");

    assert!(
        scalar_bool(&observer, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "a failed migration must still release the advisory lock"
    );
}

/// Warm boot: the second runner applies nothing. A regression guard, not primary evidence.
#[tokio::test]
async fn a_second_run_against_a_migrated_database_applies_nothing() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let db = connect(&url).await;

    let first = migrate_under_lock(&db, Duration::from_secs(120)).await.expect("first run");
    assert!(first.migrations_applied > 0);

    let second = migrate_under_lock(&db, Duration::from_secs(120)).await.expect("second run");
    assert_eq!(second.migrations_applied, 0, "a warm boot must apply nothing");
    assert_eq!(applied_migrations(&db).await as usize, Migrator::migrations().len());
}

/// Spec §3.3: the interaction this design does NOT fix, pinned so the runbook's single-replica
/// caveat is evidence rather than prose.
///
/// m0008 takes `AUDIT_PARTITION_LOCK_KEY` with a BLOCKING `pg_advisory_xact_lock` under
/// `SET LOCAL lock_timeout = '5s'`. An old replica's `PgPartitionMaintainer` holding that key
/// therefore aborts the entire migration — even though this replica won `MIGRATION_LOCK_KEY`.
#[tokio::test]
async fn a_held_audit_partition_lock_aborts_the_migration() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let maintainer = connect_pinned(&url).await;
    let db = connect(&url).await;

    assert!(
        scalar_bool(&maintainer, &format!("SELECT pg_try_advisory_lock({AUDIT_PARTITION_LOCK_KEY}) AS v")).await,
        "the stand-in maintainer must hold the audit-partition key"
    );

    let started = std::time::Instant::now();
    let err = migrate_under_lock(&db, Duration::from_secs(30)).await.expect_err("m0008 must abort under a held audit-partition lock");
    let elapsed = started.elapsed();
    assert!(
        !matches!(err, MigrationLockError::Contended { .. }),
        "the migration lock was won; the failure must come from m0008: {err:?}"
    );
    // Discriminates from ANY migration failure: this must specifically be m0008's own
    // `SET LOCAL lock_timeout = '5s'` firing on its `pg_advisory_xact_lock`, surfaced by
    // Postgres as SQLSTATE 55P03 ("canceling statement due to lock timeout") — not, say, an
    // unrelated `MigrationLockError::Db` or a different migration error entirely.
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("55P03") || rendered.contains("lock timeout"),
        "expected m0008's lock-timeout error (Postgres 55P03 / \"lock timeout\"), got: {rendered}"
    );
    // The abort should land around m0008's 5s `lock_timeout`, not the full 30s `wait` budget —
    // bounding this catches a regression where the failure comes from some other, slower path.
    assert!(elapsed < Duration::from_secs(15), "expected the lock-timeout abort well under the 30s wait budget, took {elapsed:?}");
    assert!(!migrations_table_exists(&db).await, "the whole migration transaction must roll back");

    assert!(scalar_bool(&maintainer, &format!("SELECT pg_advisory_unlock({AUDIT_PARTITION_LOCK_KEY}) AS v")).await);
}
