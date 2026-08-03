// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgOutboxMaintainer` (SMA-469) against real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — the same
//! gating pattern as `tests/relay_pg.rs`.
//!
//! Task 6's unit tests (`pg_outbox_maintainer.rs`'s own `#[cfg(test)]` module) only cover SQL
//! string shape and date arithmetic (`cutoff`/`sweep_step`) — pure functions, no database. THESE
//! tests are the entire behavioral proof that the sweep deletes the right rows, and only the
//! right rows, against a real Postgres.

mod support;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{OutboxRetentionPolicy, PgOutboxMaintainer};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, PaginatorTrait, Set, Statement, Value};
use uuid::Uuid;

fn policy(published_days: u32, parked_days: u32) -> OutboxRetentionPolicy {
    OutboxRetentionPolicy {
        enabled: true,
        published_days,
        parked_days,
        batch_size: 1000,
        max_batches_per_tick: 50,
    }
}

/// Seeds one row in a chosen lifecycle state. `published_at`/`parked_at` are set explicitly so
/// a test can age a row without waiting.
async fn seed(db: &DatabaseConnection, id: u128, published_at: Option<DateTime<Utc>>, parked: bool, parked_at: Option<DateTime<Utc>>) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(published_at),
        attempts: Set(0),
        parked: Set(parked),
        parked_at: Set(parked_at),
        last_error: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

async fn exists(db: &DatabaseConnection, id: Uuid) -> bool {
    event_outbox::Entity::find_by_id(id).one(db).await.unwrap().is_some()
}

#[tokio::test]
async fn sweeps_aged_published_rows_and_leaves_live_and_parked_rows_alone() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    let aged = seed(&db, 1, Some(now - ChronoDuration::days(30)), false, None).await;
    let fresh = seed(&db, 2, Some(now - ChronoDuration::hours(1)), false, None).await;
    let live = seed(&db, 3, None, false, None).await;
    let parked = seed(&db, 4, None, true, Some(now - ChronoDuration::days(30))).await;

    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(7, 0)).await;

    assert!(!report.errored, "tick reported an error");
    assert_eq!(report.deleted_published, 1);
    assert_eq!(report.deleted_parked, 0, "parked_days = 0 must never delete a parked row");
    assert!(!exists(&db, aged).await, "the aged published row should have been swept");
    assert!(exists(&db, fresh).await, "a published row inside the window must survive");
    assert!(exists(&db, live).await, "an undrained row must never be swept");
    assert!(exists(&db, parked).await, "a parked row must survive parked_days = 0");
    assert_eq!(report.parked_rows, 1, "the backlog gauge must count the parked row");
}

#[tokio::test]
async fn sweeps_aged_parked_rows_only_when_parked_days_is_set_and_park_time_is_known() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    let aged_parked = seed(&db, 10, None, true, Some(now - ChronoDuration::days(60))).await;
    let fresh_parked = seed(&db, 11, None, true, Some(now - ChronoDuration::days(1))).await;
    // A row parked with an UNKNOWN park time must never be swept — m0009 backfills these, so
    // this state should be unreachable in production, which is exactly why the guard is tested.
    let unknown_parked = seed(&db, 12, None, true, None).await;

    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(0, 30)).await;

    assert!(!report.errored);
    assert_eq!(report.deleted_parked, 1);
    assert_eq!(report.deleted_published, 0, "published_days = 0 must never delete a published row");
    assert!(!exists(&db, aged_parked).await);
    assert!(exists(&db, fresh_parked).await);
    assert!(exists(&db, unknown_parked).await, "a parked row with NULL parked_at must never be swept");
}

#[tokio::test]
async fn disabled_retention_deletes_nothing_but_still_refreshes_the_backlog_gauge() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    let aged = seed(&db, 20, Some(now - ChronoDuration::days(30)), false, None).await;
    seed(&db, 21, None, true, Some(now - ChronoDuration::days(90))).await;

    let mut p = policy(7, 30);
    p.enabled = false;
    let report = PgOutboxMaintainer::new(db.clone()).tick(now, p).await;

    assert!(!report.errored);
    assert_eq!(report.deleted_published, 0);
    assert_eq!(report.deleted_parked, 0);
    assert!(exists(&db, aged).await, "enabled = false must delete nothing");
    assert_eq!(report.parked_rows, 1, "the gauge must still be refreshed when deletion is disabled");
}

#[tokio::test]
async fn honors_batch_size_and_max_batches_per_tick_across_passes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    for i in 0..10u128 {
        seed(&db, 100 + i, Some(now - ChronoDuration::days(30)), false, None).await;
    }

    let maintainer = PgOutboxMaintainer::new(db.clone());

    // batch_size 3, capped at 2 passes => exactly 6 rows this tick, in 2 passes.
    let capped = OutboxRetentionPolicy {
        enabled: true,
        published_days: 7,
        parked_days: 0,
        batch_size: 3,
        max_batches_per_tick: 2,
    };
    let first = maintainer.tick(now, capped).await;
    assert_eq!(first.deleted_published, 6, "cap must bound a tick to batch_size * max_batches");
    assert_eq!(first.passes_published, 2, "batching must actually happen, not one big delete");
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), 4);

    // A subsequent tick resumes and drains the rest.
    let second = maintainer.tick(now, capped).await;
    assert_eq!(second.deleted_published, 4);
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), 0);
}

/// Task 2's (m0009) `ix_event_outbox_published` partial index exists specifically so the
/// published sweep's inner `SELECT` can use it instead of a full-table scan under production
/// row counts — nothing else in this suite proves the query is even ABLE to use it. This
/// mirrors (rather than imports — `published_sweep_sql` is private to `pg_outbox_maintainer`)
/// the exact inner `SELECT` the published sweep issues and inspects the chosen plan.
///
/// A one-row table does NOT exercise this: manually confirmed (not committed) that on a single
/// seeded row Postgres reasonably prefers a `Seq Scan` over `event_outbox` — with hardly any
/// pages to scan, the pkey/partial-index overhead isn't worth it, a legitimate planner cost
/// decision rather than a bug. Bulk-inserting 50k rows still did not flip it as long as they
/// were mostly-aged published rows: with `ORDER BY id LIMIT $2`, `event_outbox_pkey` (already
/// in `id` order) lets the planner stop after the first 1000 qualifying rows without an
/// explicit sort, which stays cheaper than the partial index for as long as most of the table
/// matches the sweep's predicate. The realistic case — and the one this test seeds — is the
/// opposite: a healthy relay drains published rows fast, so `event_outbox` is dominated by
/// still-live (`published_at IS NULL`) rows that never even enter `ix_event_outbox_published`
/// (its own `WHERE published_at IS NOT NULL`), while the few aged rows in the tiny backlog
/// this sweep targets do. Under that realistic skew the partial index wins outright.
#[tokio::test]
async fn published_sweep_select_plan_uses_the_partial_index_under_a_realistic_backlog() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    seed(&db, 200, Some(now - ChronoDuration::days(30)), false, None).await;
    // A healthy-relay-shaped table: 50k still-live rows (never published) dwarfing the one
    // aged, published row above — mirrors a production backlog, where the vast majority of
    // `event_outbox` is unpublished traffic the relay hasn't drained yet, not a pile of stale
    // published rows.
    db.execute_unprepared(
        r#"INSERT INTO "event_outbox" (id, occurred_at, event_type, schema_version, aggregate_prn, payload, published_at, attempts, parked)
           SELECT gen_random_uuid(), now(), 'iam.principal.created', 1,
                  'prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa', '{}',
                  NULL, 0, false
           FROM generate_series(1, 50000)"#,
    )
    .await
    .unwrap();

    // Give the planner real statistics to work with rather than the post-migration defaults.
    db.execute_unprepared(r#"ANALYZE "event_outbox";"#).await.unwrap();

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"EXPLAIN SELECT id FROM "event_outbox"
             WHERE published_at IS NOT NULL AND published_at < $1 AND parked = false
             ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED"#,
        [Value::from(now), Value::from(1000i64)],
    );
    let plan = db
        .query_all(stmt)
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "QUERY PLAN").unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    eprintln!("published sweep query plan:\n{plan}");
    assert!(
        plan.contains("ix_event_outbox_published"),
        "expected the published sweep's SELECT to use ix_event_outbox_published; got:\n{plan}"
    );
}
