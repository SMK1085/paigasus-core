// SPDX-License-Identifier: Apache-2.0

//! The Postgres adapter of `LeaseRepository` and of the claim (spec §8.2, §8.3, §8.4). Every
//! time that decides that a lease is valid is the database clock (AG4).

mod claim;
mod take;

use std::time::Duration;

use async_trait::async_trait;
use paigasus_agent_host_core::ids::InboxId;
use paigasus_agent_host_core::lease::{HeartbeatError, LeaseHandle};
use paigasus_agent_host_core::ports::{LeaseRepository, StoreError};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use uuid::Uuid;

pub use claim::DEAD_REASON;
use take::{take, ttl_micros};

/// The number of candidate inboxes that `acquire_next` locks.
pub const CANDIDATE_LIMIT: u64 = 8;

/// `$1` = `CANDIDATE_LIMIT`. An inbox with a pending event that is due and with no live lease, the
/// oldest event first. `FOR NO KEY UPDATE` does not conflict with the `FOR KEY SHARE` lock of an
/// `inbox_event` insert, so ingress never waits for an acquirer. The filter has no
/// `attempts < max_attempts` term: an inbox with only exhausted events must still be taken, so that
/// its claim moves them to `dead` (spec §8.4.1).
pub const CANDIDATES_SQL: &str = "SELECT i.id, i.org_id \
     FROM inbox i \
     WHERE EXISTS (SELECT 1 FROM inbox_event e \
                   WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now()) \
       AND NOT EXISTS (SELECT 1 FROM inbox_lease l WHERE l.inbox_id = i.id AND l.expires_at >= now()) \
     ORDER BY (SELECT min(e.received_at) FROM inbox_event e \
               WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now()) \
     LIMIT $1 \
     FOR NO KEY UPDATE OF i SKIP LOCKED";

/// `$1` = inbox, `$2` = token, `$3` = ttl in microseconds. Zero rows means that the lease is lost.
const HEARTBEAT_SQL: &str = "UPDATE inbox_lease SET expires_at = clock_timestamp() + ($3::bigint * interval '1 microsecond') \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp()";

/// `$1` = inbox, `$2` = token. A release expires the lease one microsecond in the past. The row
/// stays, so the next token is larger.
pub(crate) const RELEASE_SQL: &str = "UPDATE inbox_lease SET expires_at = clock_timestamp() - interval '1 microsecond' \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp()";

/// `$1` = run, `$2` = token. A release discards the open run of the handle.
pub(crate) const DISCARD_RUN_SQL: &str = "UPDATE run SET status = 'discarded', finished_at = now() \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

/// Await the commit of the transaction of a claim or of a run commit (spec §8.4, SMA-790 §6.8).
/// The handle is poisoned unless the commit returns `Ok`. This covers a failed commit and a future
/// that the caller drops while the commit is pending: in both cases the database may have
/// committed, so the token or the open claim of the handle may be stale.
pub(crate) async fn commit_or_poison<E>(handle: &mut LeaseHandle, commit: impl std::future::Future<Output = Result<(), E>>) -> Result<(), E> {
    struct PoisonUnlessDisarmed<'a> {
        handle: &'a mut LeaseHandle,
        armed: bool,
    }
    impl Drop for PoisonUnlessDisarmed<'_> {
        fn drop(&mut self) {
            if self.armed {
                self.handle.poison();
            }
        }
    }
    let mut guard = PoisonUnlessDisarmed { handle, armed: true };
    let result = commit.await;
    guard.armed = result.is_err();
    result
}

/// The lease store on a Postgres pool.
#[derive(Debug, Clone)]
pub struct PgLeaseStore {
    db: DatabaseConnection,
    heartbeat_statement_timeout: Duration,
}

impl PgLeaseStore {
    /// `heartbeat_statement_timeout` bounds one heartbeat statement in the database.
    #[must_use]
    pub fn new(db: DatabaseConnection, heartbeat_statement_timeout: Duration) -> Self {
        Self { db, heartbeat_statement_timeout }
    }
}

fn store(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

/// Count one lost lease of `operation`.
pub(crate) fn lost(operation: &'static str) {
    metrics::counter!(names::AGENT_HOST_LEASE_LOST_TOTAL, "operation" => operation).increment(1);
}

#[async_trait]
impl LeaseRepository for PgLeaseStore {
    async fn acquire_next(&self, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
        let txn = self.db.begin().await.map_err(store)?;
        let limit = i64::try_from(CANDIDATE_LIMIT).unwrap_or(i64::MAX);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, CANDIDATES_SQL, [limit.into()]);
        let rows = txn.query_all_raw(stmt).await.map_err(store)?;
        for row in rows {
            let id: Uuid = row.try_get("", "id").map_err(store)?;
            let org: Uuid = row.try_get("", "org_id").map_err(store)?;
            if let Some(token) = take(&txn, id, holder, ttl).await? {
                txn.commit().await.map_err(store)?;
                return Ok(Some(LeaseHandle::new(InboxId::new(org, id), token, holder.to_string())));
            }
        }
        txn.commit().await.map_err(store)?;
        Ok(None)
    }

    async fn acquire(&self, inbox: &InboxId, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError> {
        Ok(take(&self.db, inbox.uuid(), holder, ttl).await?.map(|token| LeaseHandle::new(inbox.clone(), token, holder.to_string())))
    }

    async fn heartbeat(&self, handle: &LeaseHandle, ttl: Duration) -> Result<(), HeartbeatError> {
        // A poisoned handle sends no SQL and counts nothing.
        if handle.is_poisoned() {
            return Err(HeartbeatError::LeaseLost);
        }
        let to_store = |e: sea_orm::DbErr| HeartbeatError::Store(e.to_string());
        let txn = self.db.begin().await.map_err(to_store)?;
        txn.execute_unprepared(&format!("SET LOCAL statement_timeout = '{}ms'", self.heartbeat_statement_timeout.as_millis()))
            .await
            .map_err(to_store)?;
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            HEARTBEAT_SQL,
            [handle.inbox().uuid().into(), handle.token().0.into(), ttl_micros(ttl).into()],
        );
        let result = txn.execute_raw(stmt).await.map_err(to_store)?;
        txn.commit().await.map_err(to_store)?;
        if result.rows_affected() == 0 {
            lost("heartbeat");
            return Err(HeartbeatError::LeaseLost);
        }
        Ok(())
    }

    async fn release(&self, handle: LeaseHandle) -> Result<(), StoreError> {
        // A poisoned handle sends no SQL. Its lease expires on its own.
        if handle.is_poisoned() {
            return Ok(());
        }
        let txn = self.db.begin().await.map_err(store)?;
        if let Some(run) = handle.open_claim() {
            let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, DISCARD_RUN_SQL, [run.uuid().into(), handle.token().0.into()]);
            txn.execute_raw(stmt).await.map_err(store)?;
        }
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, RELEASE_SQL, [handle.inbox().uuid().into(), handle.token().0.into()]);
        txn.execute_raw(stmt).await.map_err(store)?;
        txn.commit().await.map_err(store)?;
        Ok(())
    }
}
