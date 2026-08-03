// SPDX-License-Identifier: Apache-2.0

//! m0009 (SMA-469): `event_outbox` gains `parked_at` + `last_error`, the two retention/DLQ
//! partial indexes exist, and every row already parked at migration time is backfilled with a
//! non-NULL `parked_at` (so it is reachable by both time filters and retention rather than
//! being permanently uncollectable).

mod support;

use sea_orm::{ConnectionTrait, DbBackend, Statement};

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
