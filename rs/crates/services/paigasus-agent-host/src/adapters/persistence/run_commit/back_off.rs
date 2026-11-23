// SPDX-License-Identifier: Apache-2.0

//! The back-off after a transient failure (spec §6.9).
//!
//! One transaction: lock the lease row; move the run to `failed` with the error class
//! `transient`; give each event of the back-off its delay and the error; COMMIT through
//! `commit_or_poison`.

use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::run::{BackOff, CommitError, replace_nul};
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::{PgRunCommitter, abort, check_handle, db_err, lock_lease, retry_events, stmt};
use crate::adapters::persistence::lease_store::commit_or_poison;

/// The operation label of a lost lease.
const OPERATION: &str = "back_off";

/// `$1` = run, `$2` = token, `$3` = error. No row means that the lease is lost.
const RUN_TRANSIENT_SQL: &str = "UPDATE run SET status = 'failed', finished_at = now(), error_class = 'transient', error = $3 \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

impl PgRunCommitter {
    /// Fail a run as transient and return its events to the inbox with a delay, in one fenced
    /// transaction. A failed COMMIT is `OutcomeUnknown` and poisons the handle.
    pub(super) async fn back_off_run(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError> {
        check_handle(h, &b.run, b.token, OPERATION)?;
        let txn = self.db.begin().await.map_err(db_err)?;
        if let Err(e) = back_off_in(&txn, h.inbox().uuid(), &b).await {
            return Err(abort(txn, e, OPERATION).await);
        }
        commit_or_poison(h, txn.commit()).await.map_err(|_| CommitError::OutcomeUnknown)
    }
}

/// The statements before COMMIT.
async fn back_off_in(txn: &DatabaseTransaction, inbox: Uuid, b: &BackOff) -> Result<(), CommitError> {
    if !lock_lease(txn, inbox, b.token).await? {
        return Err(CommitError::LeaseLost);
    }
    let run = b.run.uuid();
    let error = replace_nul(&b.error);
    let failed = txn.execute_raw(stmt(RUN_TRANSIENT_SQL, [run.into(), b.token.0.into(), error.clone().into()])).await.map_err(db_err)?;
    if failed.rows_affected() == 0 {
        return Err(CommitError::LeaseLost);
    }
    retry_events(txn, run, &b.retry, &error).await
}
