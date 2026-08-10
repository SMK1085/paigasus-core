// SPDX-License-Identifier: Apache-2.0

//! SMA-489 integration tests: the commit-nudge's Postgres semantics, D13's retry metering, and
//! D9's backlog continuation, against a real Postgres.
//!
//! `tests/relay_pg.rs` already covers the run loop's *scheduling* against a `Disconnected`
//! database (the notify arm fires, the debounce floors the tick rate, `biased` shutdown wins, the
//! absolute poll deadline is not starved). Nothing there touches a database, so none of it can
//! prove the parts that are only real against Postgres:
//!
//! * `pg_notify` inside the mutation's own transaction is delivered on commit and discarded on
//!   rollback, to a DIFFERENT session (D2) — which is also the cross-replica proof, since another
//!   replica is just another session;
//! * a nudged tick will not touch an already-attempted row, so nudges cannot burn a failing row's
//!   retry budget (D13, AC6) — asserted BOTH at `tick_with` level and through the real `run` loop,
//!   because only the latter pins which `TickMode` the notify path actually chooses;
//! * one wakeup drains a backlog deeper than one batch (D9, AC4) and stops when nothing publishes
//!   (D9, AC5);
//! * `PgOutboxListener` survives an unreachable database (D7, AC8) and a killed backend (D15,
//!   AC8);
//! * `[outbox].wake_on_commit = false` gates the WRITER, not just the listener (D11, AC10);
//! * shutdown never cancels an in-flight tick (D10, AC9);
//! * end to end, a committed mutation is published without waiting out the poll interval (AC1).
//!
//! Backlog rows are seeded by DIRECT entity insert (`seed_row`, copied from `relay_pg.rs`),
//! deliberately NOT through `PgOutbox::enqueue`: enqueue emits one notification per row, so a
//! backlog seeded that way would keep the run loop ticking on its own permits and would pass with
//! D9's continuation deleted.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — same gating
//! pattern as `tests/relay_pg.rs`.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use paigasus_iam::adapters::events::{OutboxRelay, TickMode};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{PgOutbox, PgOutboxListener, SeaOrmUnitOfWork};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType, Outbox, PublishError, UnitOfWork};
use sea_orm::sqlx::postgres::PgListener;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, Set};
use tokio::sync::{Notify, oneshot};
use uuid::Uuid;

// --- helpers -------------------------------------------------------------------------------

/// A publisher that always succeeds, counting how many events it has seen (mirrors
/// `relay_pg.rs::CountingPublisher`, with a `count()` accessor because these tests read it from
/// behind an `Arc` while the relay owns another handle).
#[derive(Default)]
struct CountingPublisher {
    count: AtomicUsize,
}

impl CountingPublisher {
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EventPublisher for CountingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A publisher that always fails AND counts the attempts. The count is what makes the D13 and D9
/// assertions direct: "how many times was an event handed to the publisher", rather than only the
/// `attempts` column it leaves behind.
#[derive(Default)]
struct CountingAlwaysFailingPublisher {
    count: AtomicUsize,
}

impl CountingAlwaysFailingPublisher {
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EventPublisher for CountingAlwaysFailingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Err(Box::<dyn std::error::Error + Send + Sync>::from("always fails").into())
    }
}

/// A publisher that parks inside `publish` until released: it signals `entered` (the tick is now
/// mid-flight, inside the relay's open transaction) and then awaits `gate`. That is what lets the
/// D10 test signal shutdown at a moment when a tick is provably in progress, instead of racing it.
struct BlockingPublisher {
    gate: Arc<Notify>,
    entered: Arc<Notify>,
}

#[async_trait]
impl EventPublisher for BlockingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.entered.notify_one();
        self.gate.notified().await;
        Ok(())
    }
}

/// Inserts one fresh, unpublished `event_outbox` row with the given `id`/`occurred_at`, bypassing
/// the `Outbox`/`UnitOfWork` ports — copied from `relay_pg.rs::seed_row`.
///
/// The bypass is load-bearing here, not incidental. `PgOutbox::enqueue` emits a `pg_notify` per
/// row (D2). Seeding a backlog through it would therefore build the backlog test on top of the
/// very notification mechanism the suite exists to prove, and the moment anyone attaches a
/// `PgOutboxListener` to these fixtures — which is exactly what
/// `a_committed_mutation_is_published_without_waiting_for_the_poll` already does — each seeded row
/// would deliver its own wake permit and `one_wakeup_drains_a_backlog_larger_than_the_batch` would
/// pass with D9's continuation deleted.
///
/// (In the backlog tests as written the `wake` is a bare `Notify` with no listener attached, so a
/// `pg_notify` would currently go nowhere. That is a property of today's wiring, not a guarantee,
/// and it is not what the test should be resting on.) A direct insert emits nothing at all, so the
/// ONE explicit `notify_one` is unambiguously the only thing that can start a tick.
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
        parked_at: Set(None),
        last_error: Set(None),
    }
    .insert(db)
    .await
    .expect("seed event_outbox row")
}

/// A monotonic, process-local event-id source. This crate's `uuid` feature set is deliberately
/// `v4`/`v7`-free (the kernel/wasm rng-free posture — see `paigasus-iam`'s `Cargo.toml` and
/// `support::next_grant_id`'s doc), so test ids are minted from a counter instead. Each test runs
/// against its OWN freshly migrated Postgres, so uniqueness only has to hold per test.
static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1);

/// One minimal, unique `DomainEvent` — same shape as `relay.rs`'s `base_model()` /
/// `relay_pg.rs`'s `seed_row`, so a row written by `PgOutbox::enqueue` and one written by
/// `seed_row` are interchangeable as far as the relay is concerned.
fn sample_event() -> DomainEvent {
    let id = Uuid::from_u128(u128::from(NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed)));
    DomainEvent {
        id,
        event_type: EventType::PrincipalCreated,
        schema_version: 1,
        aggregate_prn: format!("prn:pgs:iam:::principal/{id}"),
        actor_prn: None,
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: None,
    }
}

/// Reads back one `event_outbox` row, which must exist.
async fn row(db: &DatabaseConnection, id: Uuid) -> event_outbox::Model {
    event_outbox::Entity::find_by_id(id).one(db).await.expect("query event_outbox").expect("row present")
}

/// Counts the backends OTHER than this one carrying the `PgOutboxListener`'s `application_name`
/// (`paigasus-iam-outbox-listener`, set in `pg_outbox_listener.rs`'s `connect`) — i.e. the
/// `PgOutboxListener`'s own session, once it has actually connected.
///
/// Matching on `application_name` rather than on the last-run statement text (`LISTEN%`) or the
/// channel name is deliberate. `pg_stat_activity.query` holds each backend's LAST statement, so
/// either of those alternatives could match some OTHER connection that happens to have last run a
/// `LISTEN` on this channel — e.g. `wake_on_commit_false_emits_no_notification` below opens its
/// own bare `PgListener` on the same channel — or the probe/terminate statement itself on
/// whichever SeaORM pool connection last ran it (that string literally contains the channel
/// name), and `pid <> pg_backend_pid()` only excludes the one running right now, not its siblings
/// in the pool. `application_name` identifies exactly this component, independent of what it is
/// doing right now or which other backends are momentarily also `LISTEN`ing.
async fn listening_backends(db: &DatabaseConnection) -> i64 {
    db.query_one(sea_orm::Statement::from_string(
        sea_orm::DbBackend::Postgres,
        "SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE application_name = 'paigasus-iam-outbox-listener' AND pid <> pg_backend_pid()",
    ))
    .await
    .expect("query pg_stat_activity")
    .expect("count() always returns a row")
    .try_get::<i64>("", "n")
    .expect("bigint count")
}

/// Polls `cond` every 25 ms until it holds or `budget` elapses. Used for POSITIVE assertions
/// ("this eventually happens") so a loaded machine costs latency, not a false failure; negative
/// assertions ("this must not happen") deliberately keep their fixed waits.
async fn wait_until(budget: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + budget;
    while !cond() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// --- D2: the notification's transactional semantics -----------------------------------------

/// D2, the load-bearing claim: a notification emitted inside a transaction is delivered ONLY on
/// commit. The listening session is a DIFFERENT connection, which is also what makes this the
/// cross-replica proof — a separate replica is a separate session and the mechanism does not
/// distinguish them.
#[tokio::test]
async fn a_notification_arrives_only_after_the_enqueuing_transaction_commits() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");

    // Still uncommitted: nothing may arrive.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), listener.recv()).await.is_err(),
        "a notification arrived before commit — the whole after-commit guarantee is broken"
    );

    tx.commit().await.expect("commit");

    tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .expect("notification arrives promptly after commit")
        .expect("recv ok");
}

/// D2's other half: a rolled-back mutation must never nudge.
#[tokio::test]
async fn a_rolled_back_mutation_emits_no_notification() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    {
        let tx = uow.begin().await.expect("begin");
        outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
        // dropped without commit -> rollback
    }

    assert!(
        tokio::time::timeout(Duration::from_millis(500), listener.recv()).await.is_err(),
        "a rolled-back mutation nudged the relay"
    );
    assert_eq!(event_outbox::Entity::find().all(&db).await.expect("query").len(), 0, "the rolled-back row must not be visible either");
}

// --- D13: nudges must not burn the retry budget ----------------------------------------------

/// **AC6, the most important test in this file (D13).** A failing row's `attempts` must advance
/// at most once per poll interval no matter how many nudges arrive — otherwise the retry budget
/// burns at the commit rate and `duplicate_window_secs > max_attempts × poll_interval_secs`
/// stops describing reality while still validating (`config.rs`'s
/// `duplicate_window_must_exceed_max_retry_span`).
///
/// Two phases, because they pin two different things and each is separately breakable:
///
/// * **Phase 1** drives `tick_with` directly and proves `TickMode::Fresh`'s row filter really is
///   `attempts = 0`. Deleting the `Attempts.eq(0)` filter in `tick_with` reds this phase.
/// * **Phase 2** drives the REAL `run` loop with a stream of nudges and proves the notify path
///   actually chooses `Fresh`. Phase 1 cannot see that choice at all: switching `run`'s notify arm
///   to `TickMode::All` leaves phase 1 completely green. Phase 2 is what fails.
#[tokio::test]
async fn nudged_ticks_do_not_burn_a_failing_rows_retry_budget() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(1);
    seed_row(&db, id, Utc::now()).await;

    // A 600 s poll interval means the poll arm cannot fire during this test — every tick the run
    // loop performs in phase 2 is nudge- or backlog-driven.
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), 10, 60).with_wake_debounce(Duration::from_millis(1));
    let publisher = Arc::new(CountingAlwaysFailingPublisher::default());

    // Phase 1 — one poll-mode tick takes the row to attempts = 1.
    relay.tick_with(publisher.as_ref(), TickMode::All).await.expect("poll tick");
    assert_eq!(row(&db, id).await.attempts, 1, "one poll tick must record exactly one attempt");

    // Ten nudge-mode ticks must all skip it.
    for _ in 0..10 {
        let report = relay.tick_with(publisher.as_ref(), TickMode::Fresh).await.expect("fresh tick");
        assert_eq!(report.drained, 0, "a nudged tick must not touch an already-attempted row");
    }
    assert_eq!(row(&db, id).await.attempts, 1, "attempts advanced more than once — D13's metering is broken");

    // Phase 2 — the same claim through the real run loop, which is the only place the nudge
    // path's TickMode is actually chosen.
    let after_phase_one = publisher.count();
    let wake = Arc::new(Notify::new());
    let (tx, rx) = oneshot::channel::<()>();
    let runner = relay.clone();
    let run_publisher: Arc<dyn EventPublisher> = publisher.clone();
    let run_wake = wake.clone();
    let handle = tokio::spawn(async move {
        runner
            .run(run_publisher, run_wake, async move {
                let _ = rx.await;
            })
            .await;
    });

    // ~1 s of continuous nudging against a 1 ms debounce: dozens of nudged ticks, none of which
    // may see the already-attempted row.
    for _ in 0..40 {
        wake.notify_one();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(10), handle).await.expect("run exits").expect("no panic");

    assert_eq!(
        publisher.count(),
        after_phase_one,
        "a nudged tick handed the already-attempted row to the publisher — nudges are burning the retry budget"
    );
    let final_row = row(&db, id).await;
    assert_eq!(final_row.attempts, 1, "attempts climbed with the nudge rate rather than the poll rate — D13's metering is broken");
    assert!(!final_row.parked, "the row must be nowhere near max_attempts after a nudge burst");
}

// --- D9: the backlog continuation ------------------------------------------------------------

/// AC4/D9: a full batch that made progress keeps draining without waiting a poll interval.
///
/// The 600 s poll interval and the SINGLE `notify_one` are the discriminators: exactly one wakeup
/// is available, so anything drained past the first batch of 10 can only have come from the
/// continuation loop. Rows are seeded by direct insert precisely so no extra permits exist.
#[tokio::test]
async fn one_wakeup_drains_a_backlog_larger_than_the_batch() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let batch_size = 10u64;
    for i in 1..=25u128 {
        seed_row(&db, Uuid::from_u128(i), Utc::now()).await;
    }

    let wake = Arc::new(Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), batch_size, 60).with_wake_debounce(Duration::from_millis(1));
    let (tx, rx) = oneshot::channel::<()>();
    let publisher = Arc::new(CountingPublisher::default());

    let w = wake.clone();
    let p: Arc<dyn EventPublisher> = publisher.clone();
    let handle = tokio::spawn(async move {
        relay
            .run(p, w, async move {
                let _ = rx.await;
            })
            .await;
    });

    wake.notify_one();

    wait_until(Duration::from_secs(30), || publisher.count() >= 25).await;
    assert_eq!(publisher.count(), 25, "the backlog continuation did not drain past one batch");
    assert_eq!(support::unpublished_count(&db).await, 0, "every seeded row must be marked published");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(10), handle).await.expect("run exits").expect("no panic");
}

/// AC5/D9: when NO row in a batch publishes, the continuation must stop rather than hot-loop.
#[tokio::test]
async fn a_totally_failing_publisher_stops_the_backlog_continuation() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let batch_size = 5u64;
    for i in 1..=20u128 {
        seed_row(&db, Uuid::from_u128(i), Utc::now()).await;
    }

    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), batch_size, 60).with_wake_debounce(Duration::from_millis(1));
    let publisher = Arc::new(CountingAlwaysFailingPublisher::default());
    let wake = Arc::new(Notify::new());
    let (tx, rx) = oneshot::channel::<()>();

    let w = wake.clone();
    let p: Arc<dyn EventPublisher> = publisher.clone();
    let handle = tokio::spawn(async move {
        relay
            .run(p, w, async move {
                let _ = rx.await;
            })
            .await;
    });

    wake.notify_one();

    // Positive half: the one batch does get attempted (polled, so a loaded machine costs latency
    // rather than a false failure).
    wait_until(Duration::from_secs(30), || publisher.count() >= batch_size as usize).await;
    // Negative half: nothing more may follow. A fixed wait is the right instrument here — a
    // hot-looping continuation would blow past `batch_size` within milliseconds.
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(publisher.count(), batch_size as usize, "the continuation kept going with a fully failing publisher");
    assert_eq!(support::unpublished_count(&db).await, 20, "a failing publisher must leave every row unpublished");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(10), handle).await.expect("run exits").expect("no panic");
}

// --- D7/D15: the listener's failure modes ----------------------------------------------------

/// AC8/D7: a listener pointed at an unreachable database must not return, panic, or report
/// connected — it retries forever while delivery stays poll-only.
#[tokio::test]
async fn a_listener_with_an_unreachable_database_keeps_retrying_without_failing() {
    let wake = Arc::new(Notify::new());
    let listener = PgOutboxListener::new("postgres://nobody:nobody@127.0.0.1:1/nonexistent".to_string(), wake, Duration::from_secs(60));
    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        listener
            .run(async move {
                let _ = rx.await;
            })
            .await;
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!handle.is_finished(), "the listener gave up instead of retrying (D7 says never fatal)");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(40), handle).await.expect("listener honours shutdown").expect("no panic");
}

/// AC8/D15: a killed listener backend must be noticed and reconnected, with BOTH the gauge and
/// the reconnect counter moving — the failure mode the original error-driven design would have
/// missed entirely, since sqlx reconnects internally.
#[tokio::test]
async fn a_killed_listener_backend_reconnects_and_still_delivers() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;
    let handle = paigasus_observability::init("test-iam-listener-reconnect");

    let wake = Arc::new(Notify::new());
    let listener = PgOutboxListener::new(url.clone(), wake.clone(), Duration::from_secs(300));
    let (tx, rx) = oneshot::channel::<()>();
    let listen_handle = tokio::spawn(async move {
        listener
            .run(async move {
                let _ = rx.await;
            })
            .await;
    });

    // Wait for the LISTEN to actually be registered before killing it — polled rather than slept,
    // because killing before the subscription exists would test nothing at all.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while listening_backends(&db).await == 0 {
        assert!(std::time::Instant::now() < deadline, "the outbox listener never issued its LISTEN");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Kill the outbox listener's backend specifically (identified by its `application_name`, not
    // our own admin connection). Matching on `application_name` rather than on `LISTEN%` or the
    // channel name means this can ONLY ever hit `PgOutboxListener`'s own session, even once a
    // second listener (of any kind) exists in this file — see `listening_backends` above for the
    // full reasoning, which applies identically here.
    db.execute_unprepared("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = 'paigasus-iam-outbox-listener' AND pid <> pg_backend_pid()")
        .await
        .expect("terminate listener backend");

    // Give the listener time to notice and re-establish.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // A notification after the reconnect must still land.
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let t = uow.begin().await.expect("begin");
    outbox.enqueue(&*t, &sample_event()).await.expect("enqueue");
    t.commit().await.expect("commit");

    let woke = tokio::time::timeout(Duration::from_secs(20), wake.notified()).await;

    // Scrape while the listener is STILL RUNNING. `run` zeroes the connected gauge on its way out
    // (`pg_outbox_listener.rs`, after the `'outer` loop), so a render taken after shutdown reads 0
    // whether the listener recovered or never came back — the gauge assertion below would be
    // asserting the shutdown path, not the reconnect.
    let out = handle.render();

    let _ = tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), listen_handle).await;

    assert!(woke.is_ok(), "the listener never delivered a notification after its backend was killed");
    // Both signals, parsed rather than grepped — see `support::sum_metric_from`'s doc for why a
    // name-contains assertion here is vacuous.
    assert!(
        support::sum_metric_from(&out, "iam_outbox_listener_reconnects_total") >= 1.0,
        "reconnects_total never moved — liveness is not being detected:\n{out}"
    );
    // The gauge is what separates "still down" from "healthy and quiet" for an operator: a
    // reconnect counter that moved proves the loss was NOTICED, but only `connected = 1` proves
    // the listener came back.
    assert_eq!(
        support::sum_metric_from(&out, "iam_outbox_listener_connected"),
        1.0,
        "the connected gauge did not return to 1 after the reconnect:\n{out}"
    );
}

// --- D11: the wake_on_commit escape hatch ----------------------------------------------------

/// AC10/D11: `wake_on_commit = false` must emit NO notification at all — the writer is gated,
/// not only the listener. (It does not disable D9's backlog continuation; that is deliberate
/// and documented on the config field.)
#[tokio::test]
async fn wake_on_commit_false_emits_no_notification() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(false); // the escape hatch
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    assert!(
        tokio::time::timeout(Duration::from_millis(750), listener.recv()).await.is_err(),
        "wake_on_commit = false still emitted a notification — the writer is not gated"
    );

    // ...and the row is still there for the poll to drain.
    let rows = event_outbox::Entity::find().all(&db).await.expect("query");
    assert_eq!(rows.len(), 1, "the outbox row itself must be written regardless of the flag");
}

// --- D10: shutdown never cancels an in-flight tick -------------------------------------------

/// AC9/D10: shutdown must NEVER cancel an in-flight tick. A publisher that blocks until
/// released lets us signal shutdown mid-tick; the tick's transaction must still commit, so
/// `published_at` is stamped. A cancelled tick would roll back and leave it NULL — SMA-471 D3's
/// unbounded-republish gap on every graceful shutdown.
#[tokio::test]
async fn shutdown_during_a_tick_does_not_cancel_it() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(42);
    seed_row(&db, id, Utc::now()).await;

    let gate = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let publisher = Arc::new(BlockingPublisher {
        gate: gate.clone(),
        entered: entered.clone(),
    });

    let wake = Arc::new(Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_millis(50), 10, 5);
    let (tx, rx) = oneshot::channel::<()>();
    let run = tokio::spawn({
        let p: Arc<dyn EventPublisher> = publisher.clone();
        let w = wake.clone();
        async move {
            relay
                .run(p, w, async move {
                    let _ = rx.await;
                })
                .await;
        }
    });

    // The tick is inside publish() — its transaction is open and the row is locked.
    tokio::time::timeout(Duration::from_secs(30), entered.notified()).await.expect("the poll tick reached publish()");
    let _ = tx.send(()); // shutdown NOW, mid-tick
    tokio::time::sleep(Duration::from_millis(200)).await;
    gate.notify_one(); // let the publish finish

    tokio::time::timeout(Duration::from_secs(10), run).await.expect("run exits").expect("no panic");

    let after = row(&db, id).await;
    assert!(after.published_at.is_some(), "the in-flight tick was cancelled by shutdown — its transaction rolled back");
    assert_eq!(after.attempts, 0, "a successfully published row must not have recorded a failed attempt");
}

// --- AC1: the end-to-end nudge ---------------------------------------------------------------

/// AC1: end to end, with a live listener and a 600 s poll interval, a committed mutation is
/// published without waiting out the poll.
///
/// The 600 s poll interval is the real discriminator — a publish at all proves the nudge path
/// carried it. The wall-clock bound is deliberately loose (5 s, vs. the ~10 ms this takes on an
/// idle machine) so a loaded CI box cannot red it while still being three orders of magnitude
/// below the poll interval it is contrasted against.
#[tokio::test]
async fn a_committed_mutation_is_published_without_waiting_for_the_poll() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let wake = Arc::new(Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), 100, 60).with_wake_debounce(Duration::from_millis(1));
    let publisher = Arc::new(CountingPublisher::default());
    let (tx_relay, rx_relay) = oneshot::channel::<()>();
    let (tx_listen, rx_listen) = oneshot::channel::<()>();

    let w = wake.clone();
    let p: Arc<dyn EventPublisher> = publisher.clone();
    let relay_handle = tokio::spawn(async move {
        relay
            .run(p, w, async move {
                let _ = rx_relay.await;
            })
            .await;
    });
    let listener = PgOutboxListener::new(url, wake.clone(), Duration::from_secs(60));
    let listen_handle = tokio::spawn(async move {
        listener
            .run(async move {
                let _ = rx_listen.await;
            })
            .await;
    });

    // Wait for the LISTEN to be established — POLLED, not slept. Postgres does not queue
    // notifications for an absent listener, so a fixed 500 ms that a loaded machine overran would
    // lose the notification outright and red this test on a timing accident rather than a
    // regression. Same reasoning as the killed-backend test above.
    let listen_deadline = std::time::Instant::now() + Duration::from_secs(30);
    while listening_backends(&db).await == 0 {
        assert!(std::time::Instant::now() < listen_deadline, "the outbox listener never issued its LISTEN");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let started = std::time::Instant::now();
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    wait_until(Duration::from_secs(30), || publisher.count() > 0).await;
    let elapsed = started.elapsed();
    assert_eq!(publisher.count(), 1, "the event was never published");
    assert!(elapsed < Duration::from_secs(5), "published after {elapsed:?}, expected well under the 600s poll interval");

    let _ = tx_relay.send(());
    let _ = tx_listen.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), relay_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), listen_handle).await;
}
