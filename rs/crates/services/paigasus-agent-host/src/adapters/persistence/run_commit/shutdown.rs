// SPDX-License-Identifier: Apache-2.0

//! The release of a lease after a shutdown (spec §6.9).
//!
//! A deploy is not a failure, so it must not move events toward `dead` (A7). One transaction:
//! lock the lease row and check the token and the expiry; when the check fails, change nothing.
//! Otherwise discard the open run, give back the attempt of each of its pending events, and
//! expire the lease. A crash or a lost lease keeps the attempt (SMA-787 §8.4.1).

use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::run::CommitError;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};

use super::{PgRunCommitter, abort, db_err, lock_lease, stmt};
use crate::adapters::persistence::lease_store::{DISCARD_RUN_SQL, RELEASE_SQL, commit_or_poison};

/// The operation label of a lost lease.
const OPERATION: &str = "release_after_shutdown";

/// `$1` = run. Only the pending events of the run, and never below zero.
const REFUND_SQL: &str = "UPDATE inbox_event SET attempts = attempts - 1 \
     WHERE claimed_run_id = $1 AND status = 'pending' AND attempts > 0";

impl PgRunCommitter {
    /// Release the lease of `h` after a shutdown. A poisoned handle sends no SQL: its lease
    /// expires on its own. A failed COMMIT is `OutcomeUnknown`.
    pub(super) async fn release_shutdown(&self, mut h: LeaseHandle) -> Result<(), CommitError> {
        if h.is_poisoned() {
            return Ok(());
        }
        let txn = self.db.begin().await.map_err(db_err)?;
        if let Err(e) = release_in(&txn, &h).await {
            return Err(abort(txn, e, OPERATION).await);
        }
        commit_or_poison(&mut h, txn.commit()).await.map_err(|_| CommitError::OutcomeUnknown)
    }
}

/// The statements before COMMIT.
async fn release_in(txn: &DatabaseTransaction, h: &LeaseHandle) -> Result<(), CommitError> {
    let inbox = h.inbox().uuid();
    let token = h.token();
    if !lock_lease(txn, inbox, token).await? {
        return Err(CommitError::LeaseLost);
    }
    if let Some(run) = h.open_claim() {
        let run = run.uuid();
        let discarded = txn.execute_raw(stmt(DISCARD_RUN_SQL, [run.into(), token.0.into()])).await.map_err(db_err)?;
        // A run that is no longer `running` (committed, screened or backed off) is not an open
        // run: its attempts stay.
        if discarded.rows_affected() > 0 {
            txn.execute_raw(stmt(REFUND_SQL, [run.into()])).await.map_err(db_err)?;
        }
    }
    txn.execute_raw(stmt(RELEASE_SQL, [inbox.into(), token.0.into()])).await.map_err(db_err)?;
    Ok(())
}
