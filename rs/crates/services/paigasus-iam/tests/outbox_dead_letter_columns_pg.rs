// SPDX-License-Identifier: Apache-2.0

//! m0009 (SMA-469): `event_outbox` gains `parked_at` + `last_error`, and the two retention/DLQ
//! partial indexes exist (`m0009_adds_columns_and_partial_indexes`).
//!
//! `backfill_stamps_parked_at_from_now_not_occurred_at` separately proves the single most
//! consequential correctness property of the migration: a row already parked at migration time
//! is backfilled with `parked_at = now()`, never copied from `occurred_at`. Stamping `now()`
//! makes the row reachable by both time filters and retention going forward; copying
//! `occurred_at` would make an already-old parked event instantly eligible for deletion by a
//! retention window it predates. A bare `parked_at IS NOT NULL` check cannot tell these apart —
//! both leave it non-NULL — so this test compares the backfilled `parked_at` against the seeded
//! row's own `occurred_at` directly.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::persistence::Migrator;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

#[tokio::test]
async fn m0009_adds_columns_and_partial_indexes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping m0009 test: Docker unavailable");
        return;
    };

    let cols = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'event_outbox'".to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "column_name").unwrap())
        .collect::<Vec<_>>();
    assert!(cols.contains(&"parked_at".to_string()), "missing parked_at: {cols:?}");
    assert!(cols.contains(&"last_error".to_string()), "missing last_error: {cols:?}");

    let idx = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes WHERE tablename = 'event_outbox'".to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "indexname").unwrap())
        .collect::<Vec<_>>();
    assert!(idx.contains(&"ix_event_outbox_published".to_string()), "missing published index: {idx:?}");
    assert!(idx.contains(&"ix_event_outbox_parked".to_string()), "missing parked index: {idx:?}");
}

/// The backfill's most consequential property, and the one a bare `IS NOT NULL` check cannot
/// catch (see module doc): `parked_at` must be stamped from `now()` at migration time, never
/// copied from `occurred_at`. Seeds a row directly into the plain, pre-m0009 `event_outbox`
/// shape (m0001..m0008 — `parked_at`/`last_error` do not exist yet) that is ALREADY `parked`
/// with an `occurred_at` 90 days in the past, then applies m0009 and asserts the backfilled
/// `parked_at` lands near the migration time — at least 80 days after `occurred_at`. If a future
/// edit swapped `now()` for `occurred_at` in the migration's `UPDATE`, `parked_at` would equal
/// `occurred_at` exactly and this assertion would fail; it would not under the correct `now()`
/// backfill.
#[tokio::test]
async fn backfill_stamps_parked_at_from_now_not_occurred_at() {
    let Some((_node, db)) = support::start_raw_postgres().await else {
        eprintln!("skipping m0009 backfill test: Docker unavailable");
        return;
    };

    // Plain, pre-m0009 `event_outbox` shape (m0001..m0008).
    Migrator::up(&db, Some(8)).await.expect("m0001..m0008 must apply");

    let id = Uuid::from_u128(1);
    let occurred_at = Utc::now() - chrono::Duration::days(90);
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO event_outbox (id, occurred_at, event_type, aggregate_prn, payload, parked) VALUES ($1, $2, $3, $4, $5, true)",
        [
            id.into(),
            occurred_at.into(),
            "iam.principal.created".into(),
            "prn:pgs:iam:::principal/00000000-0000-0000-0000-000000000001".into(),
            "{}".into(),
        ],
    ))
    .await
    .expect("raw historical insert into the plain (pre-m0009) event_outbox, already parked");

    // Apply m0009: the backfill must stamp this already-parked row's parked_at.
    Migrator::up(&db, None).await.expect("m0009 must apply and backfill parked_at for the pre-existing parked row");

    let seeded = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT parked_at, occurred_at FROM event_outbox WHERE id = $1",
            [id.into()],
        ))
        .await
        .unwrap()
        .expect("seeded row must still exist after m0009");
    let parked_at: chrono::DateTime<Utc> = seeded.try_get("", "parked_at").expect("parked_at must be non-NULL after the backfill");
    let read_occurred_at: chrono::DateTime<Utc> = seeded.try_get("", "occurred_at").unwrap();

    assert!(
        parked_at > read_occurred_at + chrono::Duration::days(80),
        "parked_at ({parked_at}) must be stamped from now() (the migration time, ~90 days after occurred_at here), not copied from \
         occurred_at ({read_occurred_at}) — a bare IS NOT NULL check cannot catch a now() -> occurred_at regression, this comparison can"
    );
}
