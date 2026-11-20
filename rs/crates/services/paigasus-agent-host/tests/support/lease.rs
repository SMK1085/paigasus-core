// SPDX-License-Identifier: Apache-2.0

//! Shared SQL of the lease tests. The statements live here, never in the adapter.

use sea_orm::{ConnectionTrait, DatabaseConnection};
use uuid::Uuid;

/// Expires the lease of an inbox one second in the past. The row stays.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn expire(db: &DatabaseConnection, inbox: Uuid) {
    db.execute_unprepared(&format!("UPDATE inbox_lease SET expires_at = now() - interval '1 second' WHERE inbox_id = '{inbox}'"))
        .await
        .expect("expire lease");
}

/// Inserts one pending event, received `age_secs` seconds ago and due now.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_pending_event(db: &DatabaseConnection, inbox: Uuid, event: Uuid, age_secs: i64) {
    db.execute_unprepared(&format!(
        "INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, envelope, received_at, not_before) \
         VALUES ('{event}', '{inbox}', 'urn:test', '{event}', 'agents.x', '{{}}'::jsonb, \
                 now() - interval '{age_secs} seconds', now() - interval '{age_secs} seconds')"
    ))
    .await
    .expect("insert pending event");
}

/// The expiry of the lease row, in epoch microseconds.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn expires_us(db: &DatabaseConnection, inbox: Uuid) -> i64 {
    super::scalar_i64(
        db,
        &format!("SELECT (EXTRACT(EPOCH FROM expires_at) * 1000000)::bigint AS n FROM inbox_lease WHERE inbox_id = '{inbox}'"),
    )
    .await
}
