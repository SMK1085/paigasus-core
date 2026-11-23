// SPDX-License-Identifier: Apache-2.0

//! The screen-out of the events that the screen flagged (spec §6.3, §6.9).
//!
//! One transaction: lock the lease row; move each flagged event of the run to `dead` with its
//! screen error; when every event is flagged, move the run to `screened`; COMMIT through
//! `commit_or_poison`. The thread is not updated, because no run happened on it.

use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::run::{CommitError, ScreenOut, ScreenResult, replace_nul};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::{PgRunCommitter, abort, check_handle, db_err, lock_lease, stmt};
use crate::adapters::persistence::lease_store::commit_or_poison;

/// The operation label of a lost lease.
const OPERATION: &str = "screen_out";

/// `$1` = event, `$2` = run, `$3` = the screen error. Only a pending event of this run.
const SCREEN_EVENT_SQL: &str = "UPDATE inbox_event SET status = 'dead', last_error = $3 \
     WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending' RETURNING attempts";

/// `$1` = run, `$2` = token. No row means that the lease is lost.
const SCREEN_RUN_SQL: &str = "UPDATE run SET status = 'screened', finished_at = now() \
     WHERE id = $1 AND status = 'running' AND lease_token = $2";

impl PgRunCommitter {
    /// Close the flagged events of a run in one fenced transaction. A failed COMMIT is
    /// `OutcomeUnknown` and poisons the handle.
    pub(super) async fn screen_out_events(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError> {
        check_handle(h, &s.run, s.token, OPERATION)?;
        let txn = self.db.begin().await.map_err(db_err)?;
        let attempts = match screen_in(&txn, h.inbox().uuid(), &s).await {
            Ok(attempts) => attempts,
            Err(e) => return Err(abort(txn, e, OPERATION).await),
        };
        commit_or_poison(h, txn.commit()).await.map_err(|_| CommitError::OutcomeUnknown)?;
        let dead = u64::try_from(attempts.len()).unwrap_or(u64::MAX);
        if dead > 0 {
            metrics::counter!(names::AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL).increment(dead);
            metrics::counter!(names::AGENT_HOST_EVENTS_SCREENED_TOTAL).increment(dead);
        }
        for n in attempts {
            metrics::histogram!(names::AGENT_HOST_EVENT_ATTEMPTS).record(f64::from(n));
        }
        Ok(if s.all { ScreenResult::AllScreened } else { ScreenResult::Continue })
    }
}

/// The statements before COMMIT. It returns the attempts of each event that went to `dead`.
async fn screen_in(txn: &DatabaseTransaction, inbox: Uuid, s: &ScreenOut) -> Result<Vec<i32>, CommitError> {
    if !lock_lease(txn, inbox, s.token).await? {
        return Err(CommitError::LeaseLost);
    }
    let run = s.run.uuid();
    let mut attempts = Vec::with_capacity(s.flagged.len());
    for f in &s.flagged {
        let error = replace_nul(&f.last_error());
        if let Some(row) = txn.query_one_raw(stmt(SCREEN_EVENT_SQL, [f.event.uuid().into(), run.into(), error.into()])).await.map_err(db_err)? {
            attempts.push(row.try_get::<i32>("", "attempts").map_err(db_err)?);
        }
    }
    if s.all {
        let done = txn.execute_raw(stmt(SCREEN_RUN_SQL, [run.into(), s.token.0.into()])).await.map_err(db_err)?;
        if done.rows_affected() == 0 {
            return Err(CommitError::LeaseLost);
        }
    }
    Ok(attempts)
}
