// SPDX-License-Identifier: Apache-2.0

//! The take of an inbox lease (spec §8.3). One statement works for an inbox that has no lease row
//! yet and for an inbox whose lease expired.

use std::time::Duration;

use paigasus_agent_host_core::ports::StoreError;
use paigasus_agent_host_core::run::LeaseToken;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

/// `$1` = inbox, `$2` = holder, `$3` = ttl in microseconds. The `WHERE` of the conflict branch is
/// what keeps a live lease: without it, a second holder would take a live lease. No row means that
/// another holder has a live lease. The row is never deleted, so a token never repeats.
///
/// The statement uses `clock_timestamp()`, not `now()` (spec §8: a statement that can wait on a
/// lock reads the live clock). The conflict branch can wait for the row lock of another take or
/// of a claim. `now()` is the start of the transaction, so after a long wait the new expiry could
/// already be in the past. With the live clock, the expiry check and the new expiry both read the
/// clock after the lock is granted.
const TAKE_SQL: &str = "INSERT INTO inbox_lease (inbox_id, holder, token, expires_at) \
     VALUES ($1, $2, 1, clock_timestamp() + ($3::bigint * interval '1 microsecond')) \
     ON CONFLICT (inbox_id) DO UPDATE \
       SET token = inbox_lease.token + 1, holder = EXCLUDED.holder, \
           expires_at = clock_timestamp() + ($3::bigint * interval '1 microsecond') \
       WHERE inbox_lease.expires_at < clock_timestamp() \
     RETURNING token";

/// The ttl in whole microseconds, capped at the largest `bigint`.
pub(super) fn ttl_micros(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_micros()).unwrap_or(i64::MAX)
}

/// Run the take on a connection or an open transaction. `None` means that the lease is live.
pub(super) async fn take(conn: &impl ConnectionTrait, inbox: Uuid, holder: &str, ttl: Duration) -> Result<Option<LeaseToken>, StoreError> {
    let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, TAKE_SQL, [inbox.into(), holder.to_string().into(), ttl_micros(ttl).into()]);
    let Some(row) = conn.query_one_raw(stmt).await.map_err(|e| StoreError(e.to_string()))? else {
        return Ok(None);
    };
    let token: i64 = row.try_get("", "token").map_err(|e| StoreError(e.to_string()))?;
    Ok(Some(LeaseToken(token)))
}
