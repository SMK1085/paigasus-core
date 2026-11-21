// SPDX-License-Identifier: Apache-2.0

//! Two proofs about the migration bookkeeping against a real Postgres (anti-goal A6):
//! concurrent migrators converge on this crate's own table, and a table of another service in the
//! same schema stays untouched.

mod support;

use std::time::Duration;

use paigasus_agent_host::adapters::persistence::migration_lock::migrate_under_lock;
use paigasus_agent_host::adapters::persistence::{MIGRATION_TABLE, Migrator};
use sea_orm::ConnectionTrait;
use sea_orm_migration::MigratorTrait;

/// Two migrators, one database: no deadlock, one converged schema, and exactly one of them does
/// the work. `tokio::time::timeout` is the deadlock assertion.
#[tokio::test]
async fn two_concurrent_migrators_converge_without_deadlocking() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration isolation test: Docker unavailable");
        return;
    };
    let a = support::connect(&url).await;
    let b = support::connect(&url).await;
    assert!(
        !support::scalar_bool(&a, &format!("SELECT to_regclass('{MIGRATION_TABLE}') IS NOT NULL AS v")).await,
        "the container must start unmigrated"
    );

    let wait = Duration::from_secs(120);
    let (ra, rb) = tokio::time::timeout(Duration::from_secs(300), async { tokio::join!(migrate_under_lock(&a, wait), migrate_under_lock(&b, wait)) })
        .await
        .expect("concurrent migrations deadlocked");
    let (oa, ob) = (ra.expect("migrator A"), rb.expect("migrator B"));

    let n = support::scalar_i64(&a, &format!("SELECT count(*)::bigint AS n FROM {MIGRATION_TABLE}")).await;
    assert_eq!(n as usize, Migrator::migrations().len(), "the schema must be at the tip");
    let workers = [&oa, &ob].iter().filter(|o| o.migrations_applied > 0).count();
    assert_eq!(workers, 1, "exactly one migrator applies migrations, got {oa:?} and {ob:?}");
}

/// A `seaql_migrations` table of another service stays as it is. SeaORM's default table name is
/// that one. If this crate used it, SeaORM would find the foreign version `m9999_other_service`
/// with no migration file and refuse to start.
#[tokio::test]
async fn a_foreign_seaql_migrations_table_is_left_alone() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration isolation test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    db.execute_unprepared("CREATE TABLE public.seaql_migrations (version varchar NOT NULL PRIMARY KEY, applied_at bigint NOT NULL)")
        .await
        .expect("create the foreign table");
    db.execute_unprepared("INSERT INTO public.seaql_migrations (version, applied_at) VALUES ('m9999_other_service', 1)")
        .await
        .expect("seed the foreign row");

    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/readyz", 200, Duration::from_secs(90)).await;

    let rows = support::scalar_i64(&db, "SELECT count(*)::bigint AS n FROM public.seaql_migrations").await;
    assert_eq!(rows, 1, "the foreign table must still hold exactly its one row");
    assert!(
        support::scalar_bool(
            &db,
            "SELECT EXISTS (SELECT 1 FROM public.seaql_migrations WHERE version = 'm9999_other_service' AND applied_at = 1) AS v"
        )
        .await,
        "the foreign row must be unchanged"
    );
    let own = support::scalar_i64(&db, &format!("SELECT count(*)::bigint AS n FROM {MIGRATION_TABLE}")).await;
    assert_eq!(own as usize, Migrator::migrations().len(), "this service must record its own migration in its own table");
}
