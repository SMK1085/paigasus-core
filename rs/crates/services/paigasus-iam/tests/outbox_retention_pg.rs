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
use paigasus_iam::adapters::persistence::pg_outbox_maintainer::published_sweep_sql;
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
    // Load-bearing for the published sweep's `AND parked = false` clause (round-1 review,
    // Finding 1): every OTHER parked row seeded above has `published_at = None`, so it never
    // matches `published_at IS NOT NULL AND published_at < $1` regardless of `parked = false` —
    // deleting that clause from `published_sweep_sql()` would leave every other assertion in
    // this test green. This is the one state that actually exercises the guard: aged AND
    // published AND parked. `pg_outbox_maintainer.rs`'s own module doc notes a later task's
    // `replay_in` becomes a second writer of `published_at`/`parked`, which is exactly why a row
    // can end up in this state in production and why the guard matters.
    let aged_published_but_parked = seed(&db, 5, Some(now - ChronoDuration::days(30)), true, Some(now - ChronoDuration::days(30))).await;

    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(7, 0)).await;

    assert!(!report.errored, "tick reported an error");
    assert_eq!(report.deleted_published, 1);
    assert_eq!(report.deleted_parked, 0, "parked_days = 0 must never delete a parked row");
    assert!(!exists(&db, aged).await, "the aged published row should have been swept");
    assert!(exists(&db, fresh).await, "a published row inside the window must survive");
    assert!(exists(&db, live).await, "an undrained row must never be swept");
    assert!(exists(&db, parked).await, "a parked row must survive parked_days = 0");
    assert!(
        exists(&db, aged_published_but_parked).await,
        "a parked row must survive the published sweep even when its published_at is also old — \
         `parked = false` is what excludes it, not published_at; deleting `AND parked = false` \
         from published_sweep_sql() must fail this assertion"
    );
    assert_eq!(report.parked_rows, 2, "the backlog gauge must count both parked rows");
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
    // this state should be unreachable in production. NOTE (round-1 review correction): the
    // assertion below holds unconditionally by SQL three-valued logic — `parked_at < $1` is
    // NULL (not true) when `parked_at` is NULL, so the row is excluded from the WHERE clause
    // whether or not `parked_sweep_sql()`'s explicit `parked_at IS NOT NULL` clause is present.
    // That clause is honest defense-in-depth, not something this (or any) test could falsify by
    // deleting it — do not read this assertion as coverage of that specific clause; it documents
    // the intended behavior, which happens to be unfalsifiable-by-construction here.
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

/// Round-1 review, Finding 3: every assertion above reads `SweepReport` fields — which proves
/// `parked_row_count()`/the sweep loops ran and their results reached the struct, but proves
/// NOTHING about whether `tick` actually emits its metrics. No recorder is installed anywhere
/// else in this suite, so `gauge!(...)`/`counter!(...)` are no-ops there; an assertion on
/// `report.parked_rows` would pass identically whether or not the `gauge!` call inside `tick`
/// had been deleted entirely. Mirrors `tests/relay_pg.rs`'s pattern (`paigasus_observability::
/// init` + `handle.render()` + substring/line assertions on the Prometheus exposition) to prove
/// the three metric families this module owns are actually emitted, with the label values
/// `IamConfig`'s consumers (Task 17's alert rules) will depend on by name.
#[tokio::test]
async fn tick_emits_its_metric_families_with_the_expected_labels() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    seed(&db, 300, Some(now - ChronoDuration::days(30)), false, None).await;
    seed(&db, 301, None, true, Some(now - ChronoDuration::days(31))).await;

    let handle = paigasus_observability::init("test-iam-outbox-retention-tick-metrics");
    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(7, 30)).await;
    assert!(!report.errored);
    assert_eq!(report.deleted_published, 1);
    assert_eq!(report.deleted_parked, 1);

    let out = handle.render();
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_retention_ticks_total") && l.contains(r#"result="ok""#)),
        "expected an iam_outbox_retention_ticks_total series labeled result=\"ok\":\n{out}"
    );
    assert!(!out.contains(r#"result="error""#), "a healthy tick must not emit a result=\"error\" series:\n{out}");
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_rows_deleted_total") && l.contains(r#"reason="published""#)),
        "expected an iam_outbox_rows_deleted_total series labeled reason=\"published\":\n{out}"
    );
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_rows_deleted_total") && l.contains(r#"reason="parked""#)),
        "expected an iam_outbox_rows_deleted_total series labeled reason=\"parked\":\n{out}"
    );
    assert!(out.contains("iam_outbox_parked_rows"), "missing the backlog gauge:\n{out}");
}

/// Task 2's (m0009) `ix_event_outbox_published` partial index exists specifically so the
/// published sweep can avoid a full-table scan under production row counts — nothing else in
/// this suite proves the query is even ABLE to use an index. Builds `EXPLAIN` from the REAL
/// `published_sweep_sql()` (round-1 review, Finding 2: a hand-copied SQL string would keep
/// passing against stale text if the real query ever changed) rather than a re-typed copy.
///
/// The assertion is deliberately soft — NOT a sequential scan — rather than pinning the exact
/// index name. Which index the planner picks is its own cost-based decision, sensitive to
/// table shape/statistics/Postgres version/competing indexes; a seq scan is the one plan shape
/// that would actually indicate a regression (the query can't use ANY index). Observed plans
/// while shaping this test's seed data (recorded here, not asserted on, since the plan text
/// itself is planner-version-fragile):
///
/// - One seeded row, no bulk data: `Seq Scan on event_outbox` — reasonable; with hardly any
///   pages to scan, an index's overhead isn't worth it. This is why the test seeds more.
/// - 20k-50k bulk rows, all/mostly aged-and-published: `Index Scan using event_outbox_pkey` —
///   NOT the partial index, still not a seq scan. With `ORDER BY id LIMIT $2`, the pkey (already
///   in `id` order) lets the planner stop after the first 1000 qualifying rows without an
///   explicit sort; that stays cheaper than the partial index for as long as most of the table
///   matches the sweep's predicate.
/// - 50k still-LIVE (`published_at IS NULL`) rows + 1 aged published row (what this test seeds,
///   below): `Index Scan using ix_event_outbox_published` (`Sort` on `id` on top, since that
///   index isn't `id`-ordered). Mirrors a healthy relay, where `event_outbox` is dominated by
///   unpublished traffic the relay hasn't drained yet, not a pile of stale published rows — the
///   partial index wins outright once the aged-published predicate is actually selective.
#[tokio::test]
async fn published_sweep_query_does_not_resort_to_a_sequential_scan() {
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

    // Deliberately explicit, NOT left to autovacuum/autoanalyze: those run on a background timer
    // with no guarantee of completing within this test's lifetime, and are the first thing
    // starved when the host is CPU/IO-contended by many concurrent containers — exactly the
    // `moon ci` condition under heavy parallelism. Without this call the planner's statistics
    // (and therefore its plan choice) would depend on whether a background process happened to
    // win a race against the test, turning the assertion below into a check on scheduler luck
    // rather than on query shape. Do NOT delete this as "redundant" even if it appears to have no
    // effect locally: for THIS query, `ORDER BY id` already anchors the plan to
    // `event_outbox_pkey` robustly enough that removing this call (verified) did not reproduce a
    // Seq Scan even under a heavily CPU/IO-loaded rerun with zero table statistics
    // (`pg_class.reltuples = -1`) — but that robustness is a property of the current query shape,
    // not of having real statistics, and a future edit to the query (e.g. dropping `ORDER BY id`)
    // could reinstate the dependency this call exists to remove.
    db.execute_unprepared(r#"ANALYZE "event_outbox";"#).await.unwrap();

    let stmt = Statement::from_sql_and_values(DbBackend::Postgres, format!("EXPLAIN {}", published_sweep_sql()), [Value::from(now), Value::from(1000i64)]);
    let plan = db
        .query_all(stmt)
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "QUERY PLAN").unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    eprintln!("published sweep query plan:\n{plan}");
    assert!(!plan.contains("Seq Scan"), "the published sweep's query must not fall back to a sequential scan; got:\n{plan}");
}
