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

use super::{PgLeaseStore, commit_or_poison, lost};

/// `$1` = inbox, `$2` = token. No row means that the lease is lost.
const LOCK_SQL: &str = "SELECT token FROM inbox_lease \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp() FOR UPDATE";

/// `$1` = run, `$2` = token. No row means that the lease is lost. It returns the thread that is
/// stored on the run: step 4 updates that thread, not the `thread` field of the `Claim` value.
const COMPLETE_RUN_SQL: &str = "UPDATE run SET status = 'completed', finished_at = now() \
     WHERE id = $1 AND status = 'running' AND lease_token = $2 RETURNING thread_id";

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
        // A failed commit, or a commit future that the caller drops, leaves the state of the run
        // unknown: the database may have completed it. The handle is then poisoned.
        commit_or_poison(handle, txn.commit()).await.map_err(store)?;
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
    let Some(run_row) = txn.query_one_raw(stmt(COMPLETE_RUN_SQL, [run.into(), token.into()])).await.map_err(store)? else {
        return Ok(None);
    };
    let thread: Uuid = run_row.try_get("", "thread_id").map_err(store)?;
    // Step 3: mark the claimed events done.
    let claimed: Vec<Uuid> = claim.events.iter().map(|e| e.id.uuid()).collect();
    let rows = txn.query_all_raw(stmt(DONE_SQL, [run.into(), claimed.into()])).await.map_err(store)?;
    let mut done = Vec::with_capacity(rows.len());
    for row in rows {
        done.push(row.try_get::<Uuid>("", "id").map_err(store)?);
    }
    // Step 4: the thread of the run. A `Claim` value whose `thread` was changed cannot move
    // `last_run_at` or a successor link onto another thread.
    if let Some(row) = txn.query_one_raw(stmt(THREAD_SQL, [thread.into(), inbox.into()])).await.map_err(store)? {
        let predecessor: Option<Uuid> = row.try_get("", "predecessor_id").map_err(store)?;
        if let Some(predecessor) = predecessor {
            txn.execute_raw(stmt(SUCCESSOR_SQL, [predecessor.into(), inbox.into(), thread.into()])).await.map_err(store)?;
        }
    }
    Ok(Some(done))
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_agent_host_core::ids::InboxId;
    use paigasus_agent_host_core::run::LeaseToken;
    use std::time::Duration;

    fn handle() -> LeaseHandle {
        LeaseHandle::new(InboxId::new(Uuid::nil(), Uuid::nil()), LeaseToken(1), "h".to_owned())
    }

    #[tokio::test]
    async fn a_commit_that_the_caller_drops_poisons_the_handle() {
        let mut h = handle();
        let pending = std::future::pending::<Result<(), String>>();
        let outcome = tokio::time::timeout(Duration::from_millis(20), commit_or_poison(&mut h, pending)).await;
        assert!(outcome.is_err(), "the commit never returns");
        assert!(h.is_poisoned());
    }

    #[tokio::test]
    async fn a_failed_commit_poisons_and_a_good_commit_does_not() {
        let mut h = handle();
        assert_eq!(commit_or_poison(&mut h, async { Ok::<(), String>(()) }).await, Ok(()));
        assert!(!h.is_poisoned());
        assert!(commit_or_poison(&mut h, async { Err::<(), String>("x".into()) }).await.is_err());
        assert!(h.is_poisoned());
    }
}
