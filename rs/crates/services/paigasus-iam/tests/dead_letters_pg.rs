// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgDeadLetters` (SMA-469) against real Postgres. Docker gating
//! mirrors `tests/relay_pg.rs`: hard failure in CI, skip on a Docker-less laptop.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::events::OutboxRelay;
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{PgDeadLetters, SeaOrmUnitOfWork};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter, DeadLetters, DomainEvent, EventPublisher, PublishError, UnitOfWork};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

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

/// Seeds a parked row (the dead-letter state) with a chosen event type and park time.
/// `actor_prn`/`correlation_id` are left `None` here — fine for the tests that only read the
/// result back through `PgDeadLetters::list` (SeaORM's entity API, `model_to_entry`), which
/// extracts every field by compile-checked struct access. It is NOT fine for a test that reads
/// back through `replay_in`/`discard_in` (`row_to_entry`'s raw `RETURNING *` extraction) — see
/// [`seed_parked_with_details`].
async fn seed_parked(db: &DatabaseConnection, id: u128, event_type: &str, parked_ago_days: i64) -> Uuid {
    seed_parked_inner(db, id, event_type, parked_ago_days, None, None).await
}

/// Like [`seed_parked`], but also stamps `actor_prn`/`correlation_id` with real, non-null
/// values. Required for any test that reads its result back through `PgDeadLetters::
/// row_to_entry` (`replay_in`/`discard_in`): that function extracts columns BY NAME
/// (`r.try_get("", "actor_prn")` etc.) from a raw `RETURNING *` row, and `sea_orm`'s
/// `TryGetable for Option<T>` maps a MISSING (e.g. typo'd) column to `Ok(None)`, never an
/// error. Seeding `None` there and asserting `None` back — as a naive test would — would pass
/// even against a broken column name; only a non-null seed plus an exact non-null assertion on
/// the value actually returned by `replay_in`/`discard_in` exercises the extraction.
async fn seed_parked_with_details(db: &DatabaseConnection, id: u128, event_type: &str, parked_ago_days: i64, actor_prn: &str, correlation_id: Uuid) -> Uuid {
    seed_parked_inner(db, id, event_type, parked_ago_days, Some(actor_prn.to_string()), Some(correlation_id)).await
}

async fn seed_parked_inner(db: &DatabaseConnection, id: u128, event_type: &str, parked_ago_days: i64, actor_prn: Option<String>, correlation_id: Option<Uuid>) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set(event_type.to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(actor_prn),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(correlation_id),
        published_at: Set(None),
        attempts: Set(5),
        parked: Set(true),
        parked_at: Set(Some(Utc::now() - ChronoDuration::days(parked_ago_days))),
        last_error: Set(Some("backend error: transport closed".to_string())),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

fn filter() -> DeadLetterFilter {
    DeadLetterFilter {
        event_type: None,
        parked_from: None,
        parked_to: None,
        cursor: None,
        limit: 50,
    }
}

#[tokio::test]
async fn lists_only_parked_rows_newest_first_and_pages_by_keyset() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());

    let a = seed_parked(&db, 1, "iam.principal.created", 3).await;
    let b = seed_parked(&db, 2, "iam.role.granted", 2).await;
    let c = seed_parked(&db, 3, "iam.principal.created", 1).await;
    // A live row must never appear in the dead-letter list.
    event_outbox::ActiveModel {
        id: Set(Uuid::from_u128(4)),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set("{}".to_string()),
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

    let all = dead.list(&filter()).await.unwrap();
    assert_eq!(all.len(), 3, "only parked rows are dead letters");
    assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b, a], "newest (highest v7 id) first");
    assert_eq!(all[0].last_error.as_deref(), Some("backend error: transport closed"));
    assert_eq!(all[0].attempts, 5);

    // event_type filter
    let typed = dead
        .list(&DeadLetterFilter {
            event_type: Some("iam.role.granted".to_string()),
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(typed.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b]);

    // parked_at range filter — the axis that answers "what parked during last night's outage".
    let recent = dead
        .list(&DeadLetterFilter {
            parked_from: Some(Utc::now() - ChronoDuration::days(2) - ChronoDuration::hours(1)),
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(recent.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b]);

    // keyset paging
    let page1 = dead.list(&DeadLetterFilter { limit: 2, ..filter() }).await.unwrap();
    assert_eq!(page1.len(), 2);
    let page2 = dead
        .list(&DeadLetterFilter {
            cursor: Some(page1.last().unwrap().id),
            limit: 2,
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(page2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![a]);

    // `parked_to` (the upper bound) is otherwise never exercised against a real database — only
    // `parked_from` is used above, and nothing in the repo ever sets `parked_to` to anything but
    // `None`. Seed one more row parked "now" (too recent for the window below) and bound the
    // query on BOTH ends, proving the upper bound actually excludes it rather than merely being
    // accepted and ignored.
    let d = seed_parked(&db, 5, "iam.principal.created", 0).await;
    let bounded = dead
        .list(&DeadLetterFilter {
            parked_from: Some(Utc::now() - ChronoDuration::days(2) - ChronoDuration::hours(1)),
            parked_to: Some(Utc::now() - ChronoDuration::hours(12)),
            ..filter()
        })
        .await
        .unwrap();
    assert!(!bounded.iter().any(|e| e.id == d), "a row parked too recently must be excluded by parked_to");
    assert_eq!(
        bounded.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![c, b],
        "parked_to must narrow the same window `recent` used, minus the too-recent row"
    );
}

#[tokio::test]
async fn replay_returns_the_row_to_the_live_queue_and_the_relay_publishes_it() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let actor_prn = "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb";
    let correlation_id = Uuid::from_u128(999_999);
    let id = seed_parked_with_details(&db, 10, "iam.principal.created", 1, actor_prn, correlation_id).await;

    let tx = uow.begin().await.unwrap();
    let replayed = dead.replay_in(&*tx, id).await.unwrap().expect("replay must return the affected row");
    tx.commit().await.unwrap();
    assert_eq!(replayed.id, id);
    // Non-null-values trap coverage: `actor_prn`/`correlation_id`/`last_error` are untouched by
    // `REPLAY_ONE_SQL` (only `parked`/`attempts`/`parked_at` are written), so the exact,
    // non-null values below can only come from `row_to_entry` correctly reading those three
    // columns off the `RETURNING *` row — a typo'd column name would silently yield `None`
    // instead of failing (sea_orm's `TryGetable for Option<T>` treats a missing column as
    // `Ok(None)`), which a bare "some row came back" assertion would not catch.
    assert_eq!(replayed.actor_prn.as_deref(), Some(actor_prn), "row_to_entry must extract actor_prn, not silently None it");
    assert_eq!(replayed.correlation_id, Some(correlation_id), "row_to_entry must extract correlation_id, not silently None it");
    assert_eq!(
        replayed.last_error.as_deref(),
        Some("backend error: transport closed"),
        "row_to_entry must extract last_error, not silently None it"
    );

    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert!(!row.parked, "replay must un-park the row");
    assert_eq!(row.attempts, 0, "replay must reset the attempt count");
    assert!(row.parked_at.is_none(), "replay must clear the park time");
    assert_eq!(
        row.last_error.as_deref(),
        Some("backend error: transport closed"),
        "replay must PRESERVE last_error so a re-parked row keeps its original evidence"
    );

    // The whole point: the relay's very next tick actually publishes it.
    let publisher = Arc::new(CountingPublisher::default());
    let report = OutboxRelay::new(db.clone(), Duration::from_secs(60), 100, 5).tick(publisher.as_ref()).await.unwrap();
    assert_eq!(report.drained, 1, "a replayed row must be visible to the relay's poll");
    assert_eq!(report.failures, 0);
    assert_eq!(publisher.count.load(Ordering::SeqCst), 1);
    assert!(event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap().published_at.is_some());
}

#[tokio::test]
async fn discard_removes_the_row_and_returns_its_full_contents() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let actor_prn = "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000cc";
    let correlation_id = Uuid::from_u128(888_888);
    let id = seed_parked_with_details(&db, 20, "iam.principal.created", 1, actor_prn, correlation_id).await;

    let tx = uow.begin().await.unwrap();
    let discarded = dead.discard_in(&*tx, id).await.unwrap().expect("discard must return the deleted row");
    tx.commit().await.unwrap();

    // The returned contents ARE the discarded event's only remaining trace — the service
    // copies them into an audit entry, so they must be complete. Also the non-null-values
    // trap coverage for `row_to_entry` (see the replay test's comment above): assert exact,
    // non-null values for actor_prn/correlation_id/parked_at/last_error rather than merely
    // "a row came back".
    assert_eq!(discarded.id, id);
    assert_eq!(discarded.event_type, "iam.principal.created");
    assert_eq!(discarded.payload, serde_json::json!({"kind": "user"}).to_string());
    assert_eq!(discarded.actor_prn.as_deref(), Some(actor_prn), "row_to_entry must extract actor_prn, not silently None it");
    assert_eq!(discarded.correlation_id, Some(correlation_id), "row_to_entry must extract correlation_id, not silently None it");
    assert!(discarded.parked_at.is_some(), "row_to_entry must extract parked_at, not silently None it");
    assert_eq!(discarded.last_error.as_deref(), Some("backend error: transport closed"));
    assert!(event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().is_none());
}

#[tokio::test]
async fn replay_and_discard_of_a_non_parked_id_return_none() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    // A live row: reachable by id, but NOT a dead letter — this surface must not touch it.
    let live = Uuid::from_u128(30);
    event_outbox::ActiveModel {
        id: Set(live),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set("{}".to_string()),
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

    let tx = uow.begin().await.unwrap();
    assert!(dead.replay_in(&*tx, live).await.unwrap().is_none(), "a live row is not a dead letter");
    assert!(dead.discard_in(&*tx, live).await.unwrap().is_none(), "a live row is not a dead letter");
    assert!(dead.replay_in(&*tx, Uuid::from_u128(999)).await.unwrap().is_none(), "an absent id yields None");
    tx.commit().await.unwrap();

    assert!(event_outbox::Entity::find_by_id(live).one(&db).await.unwrap().is_some(), "the live row must survive untouched");
}

#[tokio::test]
async fn bulk_replay_honors_its_filter_cap_and_ascending_selection_order() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let mut created = Vec::new();
    for i in 0..5u128 {
        created.push(seed_parked(&db, 100 + i, "iam.principal.created", 1).await);
    }
    // Seeded BELOW the matching set's ids (50 < 100..104), not above (a prior version of this
    // test used 200): the subquery is `ORDER BY id ASC LIMIT max_rows`, so with `max_rows = 2`
    // it would pick `other` FIRST if the `event_type` filter were silently dropped — at id 200
    // it would never be selected regardless of the filter (ids 100/101 are always the two
    // lowest), making the filter assertion below pass even with no filter applied at all. At id
    // 50 the filter is load-bearing: only the WHERE clause, not the ordering, keeps `other` out
    // of the replayed set. (Confirmed by mutation-testing `filter_clauses`: removing its
    // `event_type` clause turns this test red; restoring it turns it back green — see the task
    // report.)
    let other = seed_parked(&db, 50, "iam.role.granted", 1).await;

    // Capped at 2: must replay the two OLDEST (lowest v7 ids) of the matching set, so repeated
    // calls walk the backlog forward instead of re-selecting the same newest slice.
    let tx = uow.begin().await.unwrap();
    let n = dead
        .replay_matching_in(
            &*tx,
            &BulkReplayRequest {
                event_type: Some("iam.principal.created".to_string()),
                parked_from: None,
                parked_to: None,
                max_rows: 2,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 2);

    for (i, id) in created.iter().enumerate() {
        let row = event_outbox::Entity::find_by_id(*id).one(&db).await.unwrap().unwrap();
        let should_be_replayed = i < 2;
        assert_eq!(row.parked, !should_be_replayed, "the two oldest matching rows must be the ones replayed (index {i})");
        // A bulk-replayed row that keeps `attempts`/`parked_at` from its prior park would
        // re-park on its very first subsequent failure — silently defeating the mass-outage
        // recovery bulk replay exists for. Mirrors the single-row `replay_in` test's assertions.
        if should_be_replayed {
            assert_eq!(row.attempts, 0, "bulk replay must reset the attempt count (index {i})");
            assert!(row.parked_at.is_none(), "bulk replay must clear the park time (index {i})");
        } else {
            assert_eq!(row.attempts, 5, "an unreplayed row's attempts must be untouched (index {i})");
            assert!(row.parked_at.is_some(), "an unreplayed row's parked_at must be untouched (index {i})");
        }
    }
    let other_row = event_outbox::Entity::find_by_id(other).one(&db).await.unwrap().unwrap();
    assert!(other_row.parked, "a row outside the event_type filter must not be replayed");
    assert_eq!(other_row.attempts, 5, "a row outside the filter must be completely untouched");
}

#[tokio::test]
async fn bulk_replay_honors_a_parked_at_time_window() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    // Ids deliberately do NOT correlate with `parked_ago_days` (unlike the cap/order test's
    // monotonic 100..104 ids above), and `max_rows` is set well above the matching-row count —
    // so nothing here can be explained by `ORDER BY id ASC LIMIT max_rows` selecting a prefix.
    // Only the `parked_from`/`parked_to` WHERE clauses can produce the right in/out split.
    let too_old = seed_parked(&db, 500, "iam.principal.created", 10).await; // parked 10d ago: before `from`
    let in_window_a = seed_parked(&db, 501, "iam.principal.created", 4).await; // parked 4d ago: inside
    let too_new = seed_parked(&db, 502, "iam.principal.created", 0).await; // parked "now": after `to`
    let in_window_b = seed_parked(&db, 499, "iam.principal.created", 2).await; // parked 2d ago: inside, LOWEST id of the four

    let tx = uow.begin().await.unwrap();
    let n = dead
        .replay_matching_in(
            &*tx,
            &BulkReplayRequest {
                event_type: None,
                parked_from: Some(Utc::now() - ChronoDuration::days(5)),
                parked_to: Some(Utc::now() - ChronoDuration::days(1)),
                // Uncapped relative to the 4-row matching set: the window, not the cap or the
                // id order, must be what explains the result below.
                max_rows: 10,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 2, "only the two rows parked inside the window are replayed");

    for (id, expect_replayed, label) in [
        (too_old, false, "too_old"),
        (in_window_a, true, "in_window_a"),
        (too_new, false, "too_new"),
        (in_window_b, true, "in_window_b"),
    ] {
        let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(row.parked, !expect_replayed, "{label}: the parked_at window, not id order, must decide replay");
        if expect_replayed {
            assert_eq!(row.attempts, 0, "{label}: bulk replay must reset the attempt count");
            assert!(row.parked_at.is_none(), "{label}: bulk replay must clear the park time");
        }
    }
}
