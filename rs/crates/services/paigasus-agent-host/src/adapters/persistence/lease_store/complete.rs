// SPDX-License-Identifier: Apache-2.0

//! The minimal commit of a claim (spec §8.5, until SMA-790). One transaction locks the lease row,
//! completes the run, marks only the claimed events `done` and updates the thread.

use async_trait::async_trait;
use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::lease::{ClaimError, CompleteError, LeaseHandle};
use paigasus_agent_host_core::ports::ClaimRepository;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, TransactionTrait, Value};
use uuid::Uuid;

use super::{PgLeaseStore, lost};

/// `$1` = inbox, `$2` = token. No row means that the lease is lost.
const LOCK_SQL: &str = "SELECT token FROM inbox_lease \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp() FOR UPDATE";

/// `$1` = run, `$2` = token. Zero rows means that the lease is lost.
const COMPLETE_RUN_SQL: &str = "UPDATE run SET status = 'completed', finished_at = now() \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

/// `$1` = run, `$2` = the claimed events. Only the claimed events, never other pending events (G6).
const DONE_SQL: &str = "UPDATE inbox_event SET status = 'done' \
     WHERE id = ANY($2) AND claimed_run_id = $1 AND status = 'pending' RETURNING id";

/// `$1` = thread, `$2` = inbox. Returns the predecessor of the thread.
const THREAD_SQL: &str = "UPDATE thread SET last_run_at = now() WHERE id = $1 AND inbox_id = $2 RETURNING predecessor_id";

/// `$1` = predecessor, `$2` = inbox, `$3` = the successor. An existing successor stays.
const SUCCESSOR_SQL: &str = "UPDATE thread SET successor_id = $3 WHERE id = $1 AND inbox_id = $2 AND successor_id IS NULL";

fn store(e: impl std::fmt::Display) -> CompleteError {
    CompleteError::Store(e.to_string())
}

fn stmt(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[async_trait]
impl ClaimRepository for PgLeaseStore {
    async fn claim(&self, handle: &mut LeaseHandle, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> {
        self.claim_events(handle, cfg).await
    }

    async fn complete(&self, handle: &mut LeaseHandle, claim: Claim) -> Result<Vec<EventId>, CompleteError> {
        // A poisoned handle sends no SQL and counts nothing.
        if handle.is_poisoned() {
            return Err(CompleteError::LeaseLost);
        }
        // A claim of another run or of another token is not the claim of this handle.
        if handle.open_claim().as_ref() != Some(&claim.run) || claim.lease_token != handle.token() {
            lost("complete");
            return Err(CompleteError::LeaseLost);
        }
        let txn = self.db.begin().await.map_err(store)?;
        let Some(done) = complete_in(&txn, handle, &claim).await? else {
            txn.rollback().await.map_err(store)?;
            lost("complete");
            return Err(CompleteError::LeaseLost);
        };
        if let Err(e) = txn.commit().await {
            handle.poison();
            return Err(store(e));
        }
        handle.record_complete();
        Ok(done.into_iter().map(EventId::from_uuid).collect())
    }
}

/// Steps 1 to 4 of §8.5 in the open transaction. `None` means that the lease is lost.
async fn complete_in(txn: &DatabaseTransaction, handle: &LeaseHandle, claim: &Claim) -> Result<Option<Vec<Uuid>>, CompleteError> {
    let inbox = handle.inbox().uuid();
    let run = claim.run.uuid();
    let token = claim.lease_token.0;

    // Step 1: lock the lease row and check it.
    if txn.query_one_raw(stmt(LOCK_SQL, [inbox.into(), token.into()])).await.map_err(store)?.is_none() {
        return Ok(None);
    }
    // Step 2: complete the run, fenced by its token.
    if txn.execute_raw(stmt(COMPLETE_RUN_SQL, [run.into(), token.into()])).await.map_err(store)?.rows_affected() == 0 {
        return Ok(None);
    }
    // Step 3: mark the claimed events done.
    let claimed: Vec<Uuid> = claim.events.iter().map(|e| e.id.uuid()).collect();
    let rows = txn.query_all_raw(stmt(DONE_SQL, [run.into(), claimed.into()])).await.map_err(store)?;
    let mut done = Vec::with_capacity(rows.len());
    for row in rows {
        done.push(row.try_get::<Uuid>("", "id").map_err(store)?);
    }
    // Step 4: the thread.
    let thread = claim.thread.uuid();
    if let Some(row) = txn.query_one_raw(stmt(THREAD_SQL, [thread.into(), inbox.into()])).await.map_err(store)? {
        let predecessor: Option<Uuid> = row.try_get("", "predecessor_id").map_err(store)?;
        if let Some(predecessor) = predecessor {
            txn.execute_raw(stmt(SUCCESSOR_SQL, [predecessor.into(), inbox.into(), thread.into()])).await.map_err(store)?;
        }
    }
    Ok(Some(done))
}
