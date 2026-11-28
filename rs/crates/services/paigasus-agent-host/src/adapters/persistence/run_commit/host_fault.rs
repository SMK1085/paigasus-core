// SPDX-License-Identifier: Apache-2.0

//! The back-off after a host fault (SMA-851 D1).
//!
//! A host fault is not the fault of the event: a bad key or a gateway outage must not move events
//! toward `dead`. One transaction: lock the lease row; move the run to `failed` with the error
//! class `host_fault`; give each event of the back-off its delay and the error; give back the
//! attempt of each of these events; COMMIT through `commit_or_poison`.

use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::run::{BackOff, CommitError, replace_nul};
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::{PgRunCommitter, abort, check_handle, db_err, lock_lease, retry_events, stmt};
use crate::adapters::persistence::lease_store::commit_or_poison;

/// The operation label of a lost lease.
const OPERATION: &str = "host_fault";

/// `$1` = run, `$2` = token, `$3` = error. No row means that the lease is lost.
const RUN_HOST_FAULT_SQL: &str = "UPDATE run SET status = 'failed', finished_at = now(), error_class = 'host_fault', error = $3 \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

/// `$1` = event, `$2` = run. The attempt of the claim goes back, as `REFUND_SQL` does after a shutdown.
const HOST_FAULT_REFUND_SQL: &str = "UPDATE inbox_event SET attempts = attempts - 1 \
     WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending' AND attempts > 0";

impl PgRunCommitter {
    /// Fail a run as `host_fault`, delay its events and give back the attempt of each, in one
    /// fenced transaction. A failed COMMIT is `OutcomeUnknown` and poisons the handle.
    pub(super) async fn host_fault_run(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        check_handle(h, &b.run, b.token, OPERATION)?;
        let txn = self.db.begin().await.map_err(db_err)?;
        if let Err(e) = host_fault_in(&txn, h.inbox().uuid(), &b).await {
            return Err(abort(txn, e, OPERATION).await);
        }
        commit_or_poison(h, txn.commit()).await.map_err(|_| CommitError::OutcomeUnknown)
    }
}

/// The statements before COMMIT.
async fn host_fault_in(txn: &DatabaseTransaction, inbox: Uuid, b: &BackOff) -> Result<(), CommitError> {
    if !lock_lease(txn, inbox, b.token).await? {
        return Err(CommitError::LeaseLost);
    }
    let run = b.run.uuid();
    let error = replace_nul(&b.error);
    let failed = txn.execute_raw(stmt(RUN_HOST_FAULT_SQL, [run.into(), b.token.0.into(), error.clone().into()])).await.map_err(db_err)?;
    if failed.rows_affected() == 0 {
        return Err(CommitError::LeaseLost);
    }
    retry_events(txn, run, &b.retry, &error).await?;
    for delay in &b.retry {
        txn.execute_raw(stmt(HOST_FAULT_REFUND_SQL, [delay.event.uuid().into(), run.into()])).await.map_err(db_err)?;
    }
    Ok(())
}
