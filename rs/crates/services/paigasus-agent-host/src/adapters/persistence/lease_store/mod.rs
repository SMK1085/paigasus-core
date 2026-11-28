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
use paigasus_agent_host_core::run::LeaseToken;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait};
use uuid::Uuid;

pub use claim::DEAD_REASON;
use take::{take, ttl_micros};

/// The number of candidate inboxes that `acquire_next` locks.
pub const CANDIDATE_LIMIT: u64 = 8;

/// The join that keeps an inbox of an active agent whose model this host serves (SMA-852 §6.8).
/// `$alias` is the alias of the `inbox` table, and `$models` is the placeholder of the model ids.
/// The join reads `agent` and takes no lock on it: the `FOR NO KEY UPDATE OF i` clause of
/// `CANDIDATES_SQL` names the inbox alias only. An archived agent, or an agent whose model is not
/// in the list, leaves its inboxes out, and their events wait.
macro_rules! active_agent_join {
    ($alias:literal, $models:literal) => {
        concat!(
            "JOIN agent a ON a.org_id = ",
            $alias,
            ".org_id AND a.id = ",
            $alias,
            ".agent_id AND a.archived_at IS NULL AND a.model = ANY(",
            $models,
            "::text[])"
        )
    };
}

/// The age in seconds, on the database clock, of the oldest due pending event of every inbox of an
/// active agent with a served model (SMA-790 §6.14, SMA-852 §6.8), or 0 when there is none. It has
/// no lease and no lock filter: an event in a leased inbox, or in an inbox that another acquirer
/// holds locked, still counts. So a backlog behind a long or stuck run shows. The argument is the
/// placeholder of the model ids. The sub-select is not correlated, so Postgres runs it once (an
/// InitPlan).
macro_rules! lag_select {
    ($models:literal) => {
        concat!(
            "(SELECT COALESCE(EXTRACT(EPOCH FROM now() - min(e.received_at))::float8, 0) \
              FROM inbox_event e JOIN inbox j ON j.id = e.inbox_id ",
            active_agent_join!("j", $models),
            " WHERE e.status = 'pending' AND e.not_before <= now())"
        )
    };
}

/// The number of `pending` events of the inboxes whose agent is archived (SMA-852 §6.8). No worker
/// takes these inboxes, so the events stay. The sub-select is not correlated. The alias `g` keeps
/// it apart from the alias `a` of `active_agent_join!`.
macro_rules! archived_pending_select {
    () => {
        "(SELECT count(*) FROM inbox_event e JOIN inbox j ON j.id = e.inbox_id \
          JOIN agent g ON g.org_id = j.org_id AND g.id = j.agent_id AND g.archived_at IS NOT NULL \
          WHERE e.status = 'pending')::bigint"
    };
}

/// `$1` = `CANDIDATE_LIMIT` and `$2` = the model ids that this host serves. An inbox of an active
/// agent with a served model, with a pending event that is due and with no live lease, the oldest
/// event first (SMA-790 §6.1). Each row carries `lag`, the value of `lag_select!`, and
/// `archived_pending`, the value of `archived_pending_select!`; both are the same in every row
/// (§6.14). `FOR NO KEY UPDATE OF i` locks the inbox row only, never the `agent` row. It does not
/// conflict with the `FOR KEY SHARE` lock of an `inbox_event` insert, so ingress never waits for an
/// acquirer. The filter has no `attempts < max_attempts` term: an inbox with only exhausted events
/// must still be taken, so that its claim moves them to `dead` (spec §8.4.1).
pub const CANDIDATES_SQL: &str = concat!(
    "SELECT i.id, i.org_id, ",
    lag_select!("$2"),
    " AS lag, ",
    archived_pending_select!(),
    " AS archived_pending \
     FROM inbox i ",
    active_agent_join!("i", "$2"),
    " CROSS JOIN LATERAL (SELECT min(e.received_at) AS oldest FROM inbox_event e \
                         WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now()) d \
     WHERE d.oldest IS NOT NULL \
       AND NOT EXISTS (SELECT 1 FROM inbox_lease l WHERE l.inbox_id = i.id AND l.expires_at >= now()) \
     ORDER BY d.oldest \
     LIMIT $1 \
     FOR NO KEY UPDATE OF i SKIP LOCKED"
);

/// `$1` = the model ids. The lag and the archived-pending count when `CANDIDATES_SQL` gives no row.
const LAG_SQL: &str = concat!("SELECT ", lag_select!("$1"), " AS lag, ", archived_pending_select!(), " AS archived_pending");

/// `$1` = inbox, `$2` = token, `$3` = ttl in microseconds. Zero rows means that the lease is lost.
/// It runs after `lock_live_lease`, so it does not wait on the row lock, and its expiry term reads
/// the clock after the lock is held.
const HEARTBEAT_SQL: &str = "UPDATE inbox_lease SET expires_at = clock_timestamp() + ($3::bigint * interval '1 microsecond') \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp()";

/// `$1` = inbox, `$2` = token. A release expires the lease one microsecond in the past. The row
/// stays, so the next token is larger.
///
/// The expiry term can be evaluated before a lock wait. That is harmless here: a release only moves
/// `expires_at` into the past, so a late release changes nothing that a fence reads. A take changes
/// the row, so Postgres checks the token term again after the wait.
pub(crate) const RELEASE_SQL: &str = "UPDATE inbox_lease SET expires_at = clock_timestamp() - interval '1 microsecond' \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp()";

/// `$1` = run, `$2` = token. A release discards the open run of the handle. It has no expiry term.
/// The token term is checked again after a lock wait, because a take changes the row (see
/// `RELEASE_SQL`).
pub(crate) const DISCARD_RUN_SQL: &str = "UPDATE run SET status = 'discarded', finished_at = now() \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

/// `$1` = inbox, `$2` = token. Locks the lease row of the token. It checks no expiry: a
/// `FOR UPDATE` that waits on a row lock checks its `WHERE` again only when the other transaction
/// changed the row, so an expiry term here can be read before the wait (ADR-0028 (b)).
const LOCK_TOKEN_SQL: &str = "SELECT token FROM inbox_lease WHERE inbox_id = $1 AND token = $2 FOR UPDATE";

/// `$1` = inbox, `$2` = token. The expiry check. It runs after `LOCK_TOKEN_SQL` in the same
/// transaction, so `clock_timestamp()` is read after the lock is held.
const LIVE_SQL: &str = "SELECT expires_at > clock_timestamp() AS live FROM inbox_lease WHERE inbox_id = $1 AND token = $2";

/// Lock the lease row of `token`, then check the expiry on the database clock with a new
/// statement. `false` means that the lease is lost: another token, or an expired lease, also when
/// no other worker has taken it yet (ADR-0028 (b)). The caller holds the row lock until its
/// transaction ends.
pub(crate) async fn lock_live_lease(txn: &DatabaseTransaction, inbox: Uuid, token: LeaseToken) -> Result<bool, DbErr> {
    let values = || [inbox.into(), token.0.into()];
    if txn.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres, LOCK_TOKEN_SQL, values())).await?.is_none() {
        return Ok(false);
    }
    match txn.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres, LIVE_SQL, values())).await? {
        Some(row) => row.try_get("", "live"),
        None => Ok(false),
    }
}

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
    /// The model ids whose agents' inboxes `acquire_next` takes (SMA-852 §6.8).
    models: Vec<String>,
}

impl PgLeaseStore {
    /// `heartbeat_statement_timeout` bounds one heartbeat statement in the database. The store
    /// serves no model, so `acquire_next` takes nothing until `serving_models` names the models.
    #[must_use]
    pub fn new(db: DatabaseConnection, heartbeat_statement_timeout: Duration) -> Self {
        Self {
            db,
            heartbeat_statement_timeout,
            models: Vec::new(),
        }
    }

    /// The same store, serving `models`: `acquire_next` takes only an inbox of an active agent
    /// whose model is one of them. An inbox of an archived agent, or of an agent with another model,
    /// is never taken, and its events wait (SMA-852 §6.8). An empty list takes nothing.
    #[must_use]
    pub fn serving_models(self, models: Vec<String>) -> Self {
        Self { models, ..self }
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
        if self.models.is_empty() {
            return Ok(None);
        }
        let models = self.models.clone();
        let txn = self.db.begin().await.map_err(store)?;
        let limit = i64::try_from(CANDIDATE_LIMIT).unwrap_or(i64::MAX);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, CANDIDATES_SQL, [limit.into(), models.clone().into()]);
        let rows = txn.query_all_raw(stmt).await.map_err(store)?;
        // Every candidate row holds the same lag and the same archived-pending count. With no
        // candidate, one small statement reads them.
        let (lag, archived_pending) = match rows.first() {
            Some(row) => (row.try_get::<f64>("", "lag").map_err(store)?, row.try_get::<i64>("", "archived_pending").map_err(store)?),
            None => {
                let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, LAG_SQL, [models.into()]);
                match txn.query_one_raw(stmt).await.map_err(store)? {
                    Some(row) => (row.try_get::<f64>("", "lag").map_err(store)?, row.try_get::<i64>("", "archived_pending").map_err(store)?),
                    None => (0.0, 0),
                }
            }
        };
        metrics::gauge!(names::AGENT_HOST_INBOX_LAG_SECONDS).set(lag);
        #[allow(clippy::cast_precision_loss, reason = "a count of events; a gauge is an f64")]
        metrics::gauge!(names::AGENT_HOST_ARCHIVED_PENDING_EVENTS).set(archived_pending as f64);
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

    /// It has no agent or model filter: the caller names the inbox. The resolver answers `NotFound`
    /// for an inbox of an archived agent, so a run on it ends there (SMA-852 §6.8).
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
        // Lock the row, then check the expiry: a heartbeat that waited past the expiry does not
        // revive the lease (ADR-0028 (c)).
        if !lock_live_lease(&txn, handle.inbox().uuid(), handle.token()).await.map_err(to_store)? {
            txn.rollback().await.map_err(to_store)?;
            lost("heartbeat");
            return Err(HeartbeatError::LeaseLost);
        }
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
