// SPDX-License-Identifier: Apache-2.0

//! The §6.3 disjointness claim, proven CONCURRENTLY (SMA-469).
//!
//! `PgOutboxMaintainer`'s sweep predicates are subsets of the exact complement of the relay's
//! poll predicate (`published_at IS NULL AND parked = false`), so no row is ever visible to
//! both. `tests/outbox_retention_pg.rs` asserts that against statically seeded rows, which
//! cannot prove a claim about concurrency. This holds a relay-style `FOR UPDATE` lock open
//! across a real sweep tick and asserts the sweep neither blocks on it nor deletes the row —
//! the same hold-open technique `tests/relay_pg.rs` uses for its own `SKIP LOCKED` scenario.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same gating
//! pattern as `tests/relay_pg.rs`/`tests/outbox_retention_pg.rs`.

mod support;

use chrono::{Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{OutboxRetentionPolicy, PgOutboxMaintainer};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, DbBackend, EntityTrait, Set, Statement, TransactionTrait};
use uuid::Uuid;

/// Holds a relay-style `SELECT ... FOR UPDATE` lock open on `live`, across a real sweep tick,
/// and proves two things at once:
///
/// 1. the sweep does not BLOCK on the held lock (`tokio::time::timeout` is the assertion — see
///    below), and
/// 2. the sweep still deletes `aged` — the row it IS entitled to touch — proving the sweep had
///    genuine work to do while the lock was held rather than trivially completing because there
///    was nothing to sweep at all.
#[tokio::test]
async fn a_sweep_neither_blocks_on_nor_deletes_a_row_the_relay_holds_locked() {
    let Some((node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox concurrency test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    // The row the "relay" is mid-tick on: unpublished, unparked — invisible to both sweeps.
    let live = Uuid::from_u128(1);
    // The row retention is entitled to delete, seeded aged-published.
    let aged = Uuid::from_u128(2);
    for (id, published) in [(live, None), (aged, Some(now - ChronoDuration::days(30)))] {
        event_outbox::ActiveModel {
            id: Set(id),
            occurred_at: Set(now),
            event_type: Set("iam.principal.created".to_string()),
            schema_version: Set(1),
            aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
            actor_prn: Set(None),
            payload: Set(serde_json::json!({}).to_string()),
            correlation_id: Set(None),
            published_at: Set(published),
            attempts: Set(0),
            parked: Set(false),
            parked_at: Set(None),
            last_error: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    // A SECOND, independent connection (own pool, own physical session) holds the relay's row
    // lock open for the whole sweep — a genuinely separate session, not just a second pooled
    // connection borrowed from `db`'s own pool, mirroring `tests/relay_pg.rs`'s hold-open
    // technique but against a distinct connection so there is no ambiguity about which session
    // the lock lives on.
    let port = support::docker::mapped_port(&node, 5432, "postgres (lock holder)").await;
    let holder = Database::connect(format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let held = holder.begin().await.unwrap();
    held.execute_raw(Statement::from_string(DbBackend::Postgres, format!(r#"SELECT id FROM "event_outbox" WHERE id = '{live}' FOR UPDATE"#)))
        .await
        .unwrap();

    // The sweep must complete promptly. This `timeout` IS the "does not block" assertion: the
    // published sweep's `SELECT ... FOR UPDATE SKIP LOCKED` subquery is executed against a real
    // Postgres connection while `live`'s row lock is held open on a separate session. If the
    // sweep's predicate ever overlapped the relay's poll predicate closely enough for `live` to
    // become a lock CANDIDATE for a plain (non-`SKIP LOCKED`) statement, that statement would
    // block waiting for `held` to commit/rollback, and this `timeout` would fire instead of the
    // `tick` future ever resolving — a hang turned into a clean, attributable failure rather
    // than a silently slow machine.
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        PgOutboxMaintainer::new(db.clone()).tick(
            now,
            OutboxRetentionPolicy {
                enabled: true,
                published_days: 7,
                parked_days: 0,
                batch_size: 100,
                max_batches_per_tick: 10,
            },
        ),
    )
    .await
    .expect("the sweep blocked on a row the relay holds locked (timed out after 10s) — the sweep and relay predicates are not disjoint");

    assert!(!report.errored);
    assert_eq!(report.deleted_published, 1, "the aged published row is still swept while the relay holds another row locked");

    held.rollback().await.unwrap();

    assert!(
        event_outbox::Entity::find_by_id(live).one(&db).await.unwrap().is_some(),
        "the relay's in-flight row must never be swept"
    );
    assert!(event_outbox::Entity::find_by_id(aged).one(&db).await.unwrap().is_none());
}

/// SMA-469 round-2 review, Finding 1: unlike the two delete passes (bounded since round 1 by
/// `sweep_pass`'s `SET LOCAL lock_timeout`/`statement_timeout` transaction), the parked-row
/// backlog gauge used to run via a bare, unscoped `self.parked_row_count().await` outside any
/// such transaction. That mattered more than it looked: the gauge query is the ONE statement
/// `tick` issues unconditionally, including when `[outbox.retention].enabled = false` — so on a
/// deployment that has paused deletion, an `ACCESS EXCLUSIVE` lock on `event_outbox` (exactly
/// what m0009's own migration body takes for its whole duration, or what a manual
/// `ALTER`/`VACUUM FULL` takes) would have nothing bounded left to fall back on: it would queue
/// this query indefinitely, wedging the tick and — since `run`'s `tokio::select!` only observes
/// shutdown BETWEEN ticks — blocking graceful shutdown too.
///
/// Holds an `ACCESS EXCLUSIVE` table lock open on a second, independent connection/session across
/// a real tick (mirroring the hold-open technique above, but a TABLE-level lock rather than a
/// ROW-level one — `SKIP LOCKED` has no opinion on the former) and proves the tick backs off
/// within its bounded `lock_timeout` instead of hanging.
#[tokio::test]
async fn a_tick_does_not_hang_when_an_access_exclusive_lock_blocks_the_gauge_query() {
    let Some((node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox gauge-timeout test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    // Row content is irrelevant to this scenario — an ACCESS EXCLUSIVE lock blocks the gauge's
    // `SELECT count(*)` regardless of which rows exist or match the retention policy.
    event_outbox::ActiveModel {
        id: Set(Uuid::from_u128(1)),
        occurred_at: Set(now),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({}).to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
        parked_at: Set(None),
        last_error: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    // A SECOND, independent connection (own pool, own physical session — not one borrowed from
    // `db`'s own pool) takes the SAME lock mode m0009's migration body holds, and keeps it open
    // for the whole tick.
    let port = support::docker::mapped_port(&node, 5432, "postgres (lock holder)").await;
    let holder = Database::connect(format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let held = holder.begin().await.unwrap();
    held.execute_unprepared(r#"LOCK TABLE "event_outbox" IN ACCESS EXCLUSIVE MODE"#).await.unwrap();

    // Deletion disabled: with retention paused, the gauge query is the ONLY statement this tick
    // issues — precisely the production scenario Finding 1 describes, where an operator who has
    // paused deletion loses the backlog gauge with no bounded delete pass left to fall back on.
    let policy = OutboxRetentionPolicy {
        enabled: false,
        published_days: 7,
        parked_days: 30,
        batch_size: 100,
        max_batches_per_tick: 10,
    };

    // The gauge query's `SET LOCAL lock_timeout` is 5s (`SWEEP_LOCK_TIMEOUT`). 45s gives roughly a
    // 9x margin over that — comfortably absorbing this host's documented Docker-VM contention
    // (many parallel testcontainers can inflate an in-container 5s `lock_timeout` to 20s+ of wall
    // clock; observed once at 21.3s under a full `--test-threads=4` crate run) — while staying far
    // shorter than a genuine hang, so a regression that reintroduces an unscoped gauge query still
    // fails this test cleanly via the `.expect` message below, attributably, rather than stalling
    // the suite indefinitely or reading as an ambiguous "slow machine".
    let report = tokio::time::timeout(std::time::Duration::from_secs(45), PgOutboxMaintainer::new(db.clone()).tick(now, policy))
        .await
        .expect(
            "tick blocked for over 45s: the parked-row gauge query is not timeout-scoped and is \
             queueing indefinitely behind the ACCESS EXCLUSIVE table lock, rather than backing off \
             within its own 5s lock_timeout",
        );

    assert!(
        report.errored,
        "a tick whose gauge query is blocked by a table-level lock must report errored = true \
         (the lock_timeout tripping is a DbErr, not a silent success)"
    );

    held.rollback().await.unwrap();
}
