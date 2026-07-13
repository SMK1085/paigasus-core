// SPDX-License-Identifier: Apache-2.0

//! Integration tests for Task B8 (SMA-446, Slice B): the outbox relay's transactional
//! `SELECT ... FOR UPDATE SKIP LOCKED` drain, against real Postgres.
//!
//! Three scenarios, mirroring the task brief:
//! 1. a healthy publisher drains N seeded rows in one tick and marks each `published_at`;
//! 2. a publisher that always fails increments `attempts` each tick and parks the row once
//!    `attempts` reaches `max_attempts` (the poison path);
//! 3. `FOR UPDATE SKIP LOCKED` safety: a row locked by another, uncommitted transaction is
//!    skipped by the relay's tick rather than blocking on it or being double-processed — proven
//!    deterministically by holding the lock open across the relay's own tick rather than racing
//!    two ticks concurrently.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — same gating
//! pattern as `tests/outbox_uow_pg.rs`/`tests/uow_mechanism_pg.rs`.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use paigasus_iam::adapters::events::OutboxRelay;
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType, PublishError};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement, TransactionTrait};
use uuid::Uuid;

/// A publisher that always succeeds, counting how many events it has seen — the healthy-drain
/// assertion target for scenario 1.
#[derive(Default)]
struct CountingPublisher {
    count: AtomicUsize,
}

#[async_trait]
impl EventPublisher for CountingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A publisher that always fails — the poison-parking assertion target for scenario 2.
struct FailingPublisher;

#[async_trait]
impl EventPublisher for FailingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        Err(Box::<dyn std::error::Error + Send + Sync>::from("always fails").into())
    }
}

/// Inserts one fresh, unpublished `event_outbox` row with the given `id`/`occurred_at`,
/// bypassing the `Outbox`/`UnitOfWork` ports (this test drives `OutboxRelay` directly against a
/// bare `DatabaseConnection`, mirroring `tests/support/mod.rs`'s direct-entity-insert helpers).
async fn seed_row(db: &DatabaseConnection, id: Uuid, occurred_at: chrono::DateTime<Utc>) -> event_outbox::Model {
    event_outbox::ActiveModel {
        id: Set(id),
        occurred_at: Set(occurred_at),
        event_type: Set(EventType::PrincipalCreated.as_wire().to_string()),
        schema_version: Set(1),
        aggregate_prn: Set(format!("prn:pgs:iam:::principal/{id}")),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
    }
    .insert(db)
    .await
    .expect("seed event_outbox row")
}

/// Scenario 1 — healthy drain: N seeded rows, one relay tick, all get `published_at` set and
/// the `CountingPublisher` saw exactly N.
#[tokio::test]
async fn healthy_publisher_drains_all_rows_in_one_tick() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let now = Utc::now();
    let ids: Vec<Uuid> = (1..=3u128).map(Uuid::from_u128).collect();
    for id in &ids {
        seed_row(&db, *id, now).await;
    }

    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 10, 5);
    let publisher = Arc::new(CountingPublisher::default());
    let report = relay.tick(publisher.as_ref()).await.expect("tick succeeds");

    assert_eq!(report.drained, 3, "all three seeded rows are drained in one tick");
    assert_eq!(report.failures, 0);
    assert_eq!(report.parked, 0);
    assert_eq!(publisher.count.load(Ordering::SeqCst), 3, "the publisher saw all three events");

    for id in &ids {
        let row = event_outbox::Entity::find_by_id(*id).one(&db).await.unwrap().expect("row still present");
        assert!(row.published_at.is_some(), "a successfully published row must have published_at set");
        assert_eq!(row.attempts, 0);
        assert!(!row.parked);
    }
}

/// Scenario 2 — poison parking: a row that always fails to publish accumulates `attempts` one
/// per tick, and is `parked` exactly once `attempts` reaches `max_attempts` — after which a
/// further tick leaves it untouched (parked rows are excluded from the drain query).
#[tokio::test]
async fn failing_publisher_parks_the_row_after_max_attempts() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(1);
    seed_row(&db, id, Utc::now()).await;

    let max_attempts = 3;
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 10, max_attempts);
    let publisher = FailingPublisher;

    for expected_attempts in 1..=max_attempts {
        let report = relay.tick(&publisher).await.expect("tick succeeds even when the publisher fails");
        assert_eq!(report.drained, 1, "the still-unparked row keeps being picked up each tick");
        assert_eq!(report.failures, 1);

        let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().expect("row still present");
        assert_eq!(row.attempts, expected_attempts, "attempts increments by one per failed tick");
        assert_eq!(row.published_at, None, "a failed publish never sets published_at");

        let should_be_parked = expected_attempts >= max_attempts;
        assert_eq!(row.parked, should_be_parked, "parked flips true exactly once attempts reaches max_attempts");
        if should_be_parked {
            assert_eq!(report.parked, 1);
        } else {
            assert_eq!(report.parked, 0);
        }
    }

    // One more tick: the now-parked row is excluded from the drain query entirely (no more
    // wasted publish attempts on a poisoned row), so attempts stays pinned at max_attempts.
    let report = relay.tick(&publisher).await.expect("tick succeeds with nothing left to drain");
    assert_eq!(report.drained, 0, "a parked row is never picked up again");
    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert_eq!(row.attempts, max_attempts, "attempts does not keep climbing once parked");
    assert!(row.parked);
}

/// Scenario 3 — `FOR UPDATE SKIP LOCKED` safety: row A is locked by a separate, uncommitted
/// transaction (standing in for another relay replica mid-tick); row B is not. Running a relay
/// tick concurrently against the SAME database (row A's lock is held open across the relay's own
/// `.await`s — genuinely concurrent access, just driven deterministically from the test rather
/// than via a real `tokio::spawn` race) must process ONLY row B: no blocking on row A's lock, and
/// no double-processing once the lock is later released.
#[tokio::test]
async fn skip_locked_leaves_a_row_held_by_another_transaction_untouched() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let now = Utc::now();
    let row_a = Uuid::from_u128(1); // lower id -> ORDER BY id ASC would pick this first
    let row_b = Uuid::from_u128(2);
    seed_row(&db, row_a, now).await;
    seed_row(&db, row_b, now).await;

    // Open a manual transaction on its own pooled connection and lock row A with a plain
    // `FOR UPDATE`, deliberately NOT committing yet — simulating another relay replica already
    // mid-tick on that row.
    let locking_txn = db.begin().await.expect("begin locking txn");
    locking_txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id FROM "event_outbox" WHERE id = $1 FOR UPDATE"#,
            [row_a.into()],
        ))
        .await
        .expect("lock row A")
        .expect("row A exists");

    // Run one relay tick while row A's lock is still held open.
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 10, 5);
    let publisher = Arc::new(CountingPublisher::default());
    let report = relay.tick(publisher.as_ref()).await.expect("tick succeeds without blocking on the locked row");

    assert_eq!(report.drained, 1, "only the unlocked row (B) is drained; SKIP LOCKED skips row A entirely");
    assert_eq!(publisher.count.load(Ordering::SeqCst), 1);

    let a_after = event_outbox::Entity::find_by_id(row_a).one(&db).await.unwrap().unwrap();
    assert!(a_after.published_at.is_none(), "the locked row must be untouched by the concurrent tick");
    assert_eq!(a_after.attempts, 0);

    let b_after = event_outbox::Entity::find_by_id(row_b).one(&db).await.unwrap().unwrap();
    assert!(b_after.published_at.is_some(), "the unlocked row is drained normally");

    // Release row A's lock and prove a subsequent tick now picks it up exactly once (no
    // double-processing artifact left over from the concurrent tick above).
    locking_txn.commit().await.expect("release row A's lock");
    let report2 = relay.tick(publisher.as_ref()).await.expect("second tick succeeds");
    assert_eq!(report2.drained, 1, "row A is drained exactly once, on the tick after its lock is released");
    assert_eq!(publisher.count.load(Ordering::SeqCst), 2, "row A was published exactly once total, not twice");

    let a_final = event_outbox::Entity::find_by_id(row_a).one(&db).await.unwrap().unwrap();
    assert!(a_final.published_at.is_some());
}

/// SMA-446 Unit 5 (Task A11): `tick()` on a non-empty batch emits the drained/published/
/// failures/parked counters plus the oldest-unpublished-age gauge, alongside the existing
/// `tracing::info!`.
#[tokio::test]
async fn tick_with_a_non_empty_batch_emits_relay_metrics() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(7);
    seed_row(&db, id, Utc::now() - chrono::Duration::seconds(5)).await;

    let handle = paigasus_observability::init("test-iam-relay-tick-metrics");
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 10, 5);
    let publisher = Arc::new(CountingPublisher::default());
    let report = relay.tick(publisher.as_ref()).await.expect("tick succeeds");
    assert_eq!(report.drained, 1);

    let out = handle.render();
    assert!(out.contains("iam_outbox_relay_drained_total"), "missing drained counter:\n{out}");
    assert!(out.contains("iam_outbox_relay_published_total"), "missing published counter:\n{out}");
    assert!(out.contains("iam_outbox_relay_publish_failures_total"), "missing publish_failures counter:\n{out}");
    assert!(out.contains("iam_outbox_relay_parked_total"), "missing parked counter:\n{out}");
    assert!(out.contains("iam_outbox_oldest_unpublished_age_seconds"), "missing oldest-unpublished-age gauge:\n{out}");
}

/// SMA-446 Unit 5 (Task A11): driving the poll loop (`run`) for one tick emits
/// `iam_outbox_relay_ticks_total{result="ok"}` — `ticks_total` is a `run()`-loop counter (one
/// increment per poll interval), not a `tick()`-level one, so this drives a real (short) loop
/// iteration rather than calling `tick()` directly.
#[tokio::test]
async fn run_loop_emits_ticks_total_with_ok_result_label() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    seed_row(&db, Uuid::from_u128(8), Utc::now()).await;

    let handle = paigasus_observability::init("test-iam-relay-run-metrics");
    let relay = OutboxRelay::new(db.clone(), Duration::from_millis(10), 10, 5);
    let publisher: Arc<dyn EventPublisher> = Arc::new(CountingPublisher::default());

    // Let the loop run long enough for at least one poll interval to elapse, then shut it down.
    relay.run(publisher, tokio::time::sleep(Duration::from_millis(300))).await;

    let out = handle.render();
    assert!(out.contains("iam_outbox_relay_ticks_total"), "missing ticks_total counter:\n{out}");
    assert!(out.contains(r#"result="ok""#), "expected a result=\"ok\" labeled series:\n{out}");
}
