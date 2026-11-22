// SPDX-License-Identifier: Apache-2.0

//! The Postgres adapter of `LeaseRepository` and of the claim (spec §8.2, §8.3, §8.4). Every
//! time that decides that a lease is valid is the database clock (AG4).

mod claim;
mod take;

use std::time::Duration;

use async_trait::async_trait;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
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

/// The age in seconds, on the database clock, of the oldest due pending event of every inbox of a
/// served agent (SMA-790 §6.14), or 0 when there is none. It has no lease and no lock filter: an
/// event in a leased inbox, or in an inbox that another acquirer holds locked, still counts. So a
/// backlog behind a long or stuck run shows. The arguments are the placeholders of the org ids and
/// the agent ids. The sub-select is not correlated, so Postgres runs it once (an InitPlan).
macro_rules! lag_select {
    ($orgs:literal, $agents:literal) => {
        concat!(
            "(SELECT COALESCE(EXTRACT(EPOCH FROM now() - min(e.received_at))::float8, 0) \
              FROM inbox_event e JOIN inbox j ON j.id = e.inbox_id \
              WHERE e.status = 'pending' AND e.not_before <= now() \
                AND (j.org_id, j.agent_id) IN (SELECT * FROM unnest(",
            $orgs,
            "::uuid[], ",
            $agents,
            "::uuid[])))"
        )
    };
}

/// `$1` = `CANDIDATE_LIMIT`, `$2` = the org ids and `$3` = the agent ids of the served agents (two
/// arrays of the same length, one pair per agent). An inbox of a served agent with a pending event
/// that is due and with no live lease, the oldest event first (SMA-790 §6.1). Each row carries
/// `lag`, the value of `lag_select!`, which is the same in every row (§6.14). `FOR NO KEY UPDATE`
/// does not conflict with the `FOR KEY SHARE` lock of an `inbox_event` insert, so ingress never
/// waits for an acquirer. The filter has no `attempts < max_attempts` term: an inbox with only
/// exhausted events must still be taken, so that its claim moves them to `dead` (spec §8.4.1).
pub const CANDIDATES_SQL: &str = concat!(
    "SELECT i.id, i.org_id, ",
    lag_select!("$2", "$3"),
    " AS lag \
     FROM inbox i \
     CROSS JOIN LATERAL (SELECT min(e.received_at) AS oldest FROM inbox_event e \
                         WHERE e.inbox_id = i.id AND e.status = 'pending' AND e.not_before <= now()) d \
     WHERE d.oldest IS NOT NULL \
       AND NOT EXISTS (SELECT 1 FROM inbox_lease l WHERE l.inbox_id = i.id AND l.expires_at >= now()) \
       AND (i.org_id, i.agent_id) IN (SELECT * FROM unnest($2::uuid[], $3::uuid[])) \
     ORDER BY d.oldest \
     LIMIT $1 \
     FOR NO KEY UPDATE OF i SKIP LOCKED"
);

/// `$1` = the org ids, `$2` = the agent ids. The lag when `CANDIDATES_SQL` gives no row.
const LAG_SQL: &str = concat!("SELECT ", lag_select!("$1", "$2"), " AS lag");

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
    /// The agents whose inboxes `acquire_next` takes (SMA-790 §6.1).
    agents: Vec<AgentId>,
}

impl PgLeaseStore {
    /// `heartbeat_statement_timeout` bounds one heartbeat statement in the database. The store
    /// serves no agent, so `acquire_next` takes nothing until `serving` names the agents.
    #[must_use]
    pub fn new(db: DatabaseConnection, heartbeat_statement_timeout: Duration) -> Self {
        Self {
            db,
            heartbeat_statement_timeout,
            agents: Vec::new(),
        }
    }

    /// The same store, serving `agents`: `acquire_next` takes only an inbox of one of them. An
    /// inbox with no configured agent is never taken, and its events wait (SMA-790 §6.1, A7).
    #[must_use]
    pub fn serving(self, agents: Vec<AgentId>) -> Self {
        Self { agents, ..self }
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
        if self.agents.is_empty() {
            return Ok(None);
        }
        let orgs: Vec<Uuid> = self.agents.iter().map(AgentId::org).collect();
        let agents: Vec<Uuid> = self.agents.iter().map(AgentId::uuid).collect();
        let txn = self.db.begin().await.map_err(store)?;
        let limit = i64::try_from(CANDIDATE_LIMIT).unwrap_or(i64::MAX);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, CANDIDATES_SQL, [limit.into(), orgs.clone().into(), agents.clone().into()]);
        let rows = txn.query_all_raw(stmt).await.map_err(store)?;
        // Every candidate row holds the same lag. With no candidate, one small statement reads it.
        let lag = match rows.first() {
            Some(row) => row.try_get::<f64>("", "lag").map_err(store)?,
            None => {
                let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, LAG_SQL, [orgs.into(), agents.into()]);
                match txn.query_one_raw(stmt).await.map_err(store)? {
                    Some(row) => row.try_get::<f64>("", "lag").map_err(store)?,
                    None => 0.0,
                }
            }
        };
        metrics::gauge!(names::AGENT_HOST_INBOX_LAG_SECONDS).set(lag);
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
