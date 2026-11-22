// SPDX-License-Identifier: Apache-2.0

//! The full commit (Completed, Stopped) and the failure commit (Deterministic) of a run
//! (spec §6.8).
//!
//! One attempt is one transaction at READ COMMITTED, in this step order:
//!
//! 1. lock the lease row (`LOCK_SQL`);
//! 2. move the run out of `running`, fenced by its token, `RETURNING thread_id`;
//! 3. full commit only: `write_session`;
//! 4. full commit only: the thread (`last_run_at`, the successor link); failure commit only: close
//!    the thread when the record asks for it;
//! 5. the memory scope check, then `apply_in` when the memory set is not empty;
//! 6. full commit: the passed events go to `done`; failure commit: the back-off of each event;
//! 7. COMMIT through `commit_or_poison`.
//!
//! The thread of every step after step 2 is the `thread_id` that step 2 returns, never a value of
//! the caller: the schema ties `session_event.run_id` and `thread_id` only to the same inbox.
//!
//! A later timer step (not in SMA-790, spec N4) goes after step 5 and before step 6:
//! `TimerStore::apply_in` runs in its own savepoint and takes the lease row lock that step 1
//! already holds, so the lock order stays the lease row first. A refusal of that step rolls back
//! the whole transaction, as `MemoryRejected` does.

use paigasus_agent_host_core::fault::FaultStage;
use paigasus_agent_host_core::ids::{EventId, InboxId};
use paigasus_agent_host_core::lease::LeaseHandle;
use paigasus_agent_host_core::memory::MemoryCommitSet;
use paigasus_agent_host_core::ports::MemoryApplyError;
use paigasus_agent_host_core::run::{CommitError, Committed, RunClass, RunRecord, RunUsage, replace_nul};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::{LOCK_SQL, LOCK_TIMEOUT_SQL, PgRunCommitter, db_err, stmt};
use crate::adapters::memory_commit::MemoryCommit;
use crate::adapters::persistence::lease_store::{commit_or_poison, lost};
use crate::adapters::persistence::session_store::write_session;

/// The attempts of one commit: the first one and at most 2 retries after a `Store` error.
const STORE_ATTEMPTS: u32 = 3;

/// `$1` = run, `$2` = token, `$3` = status, `$4` = final output, `$5` = error (the stop reason),
/// `$6`..`$9` = the token usage. No row means "already committed or lease lost".
const RUN_DONE_SQL: &str = "UPDATE run SET status = $3, finished_at = now(), final_output = $4, error = $5, \
     input_tokens = $6, output_tokens = $7, cached_input_tokens = $8, reasoning_tokens = $9 \
     WHERE id = $1 AND status = 'running' AND lease_token = $2 RETURNING thread_id";

/// `$1` = run, `$2` = token, `$3` = error. The token columns stay null: the usage of a failed run
/// is not known.
const RUN_FAILED_SQL: &str = "UPDATE run SET status = 'failed', finished_at = now(), error_class = 'deterministic', error = $3 \
     WHERE id = $1 AND status = 'running' AND lease_token = $2 RETURNING thread_id";

/// `$1` = thread, `$2` = inbox. Returns the predecessor of the thread.
const THREAD_SQL: &str = "UPDATE thread SET last_run_at = now() WHERE id = $1 AND inbox_id = $2 RETURNING predecessor_id";

/// `$1` = predecessor, `$2` = inbox, `$3` = the successor. An existing successor stays.
const SUCCESSOR_SQL: &str = "UPDATE thread SET successor_id = $3 WHERE id = $1 AND inbox_id = $2 AND successor_id IS NULL";

/// `$1` = thread, `$2` = inbox. The next claim then starts a fresh thread.
const CLOSE_THREAD_SQL: &str = "UPDATE thread SET closed = true WHERE id = $1 AND inbox_id = $2";

/// `$1` = inbox, `$2` = org, `$3` = agent, `$4` = principal. No row means that the memory set is
/// not of the scope of the inbox.
const SCOPE_SQL: &str = "SELECT id FROM inbox WHERE id = $1 AND org_id = $2 AND agent_id = $3 AND principal_id = $4";

/// `$1` = run, `$2` = the passed events. Only the passed events of this run.
const DONE_SQL: &str = "UPDATE inbox_event SET status = 'done' \
     WHERE id = ANY($2) AND claimed_run_id = $1 AND status = 'pending' RETURNING id, attempts";

/// `$1` = event, `$2` = run, `$3` = delay in microseconds, `$4` = error. The event stays pending.
const RETRY_SQL: &str = "UPDATE inbox_event SET not_before = clock_timestamp() + ($3::bigint * interval '1 microsecond'), last_error = $4 \
     WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending'";

/// The text of the scope refusal.
const SCOPE_REFUSED: &str = "the memory scope is not the scope of the inbox";

impl PgRunCommitter {
    /// Commit the end of a run in one fenced transaction (spec §6.8).
    ///
    /// A `Store` error before COMMIT retries the whole transaction at most 2 times. `LeaseLost`,
    /// `MemoryRejected`, an unencodable text and `OutcomeUnknown` are never retried. The keeper,
    /// not this method, calls `record_complete` on the handle.
    pub async fn commit_record(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError> {
        // A poisoned handle sends no SQL and counts nothing.
        if h.is_poisoned() {
            return Err(CommitError::LeaseLost);
        }
        // A record of another run or of another token is not the claim of this handle.
        if h.open_claim().as_ref() != Some(&r.run) || h.token() != r.token {
            lost("commit");
            return Err(CommitError::LeaseLost);
        }
        let r = without_nul(r);
        let mut attempt = 1;
        loop {
            match self.attempt(h, &r).await {
                Err(e @ CommitError::Store(_)) if !e.is_unencodable() && attempt < STORE_ATTEMPTS => attempt += 1,
                other => return other,
            }
        }
    }

    /// One transaction: steps 1 to 7.
    async fn attempt(&self, h: &mut LeaseHandle, r: &RunRecord) -> Result<Committed, CommitError> {
        let txn = self.db.begin().await.map_err(db_err)?;
        let done = match self.steps(&txn, h.inbox(), r).await {
            Ok(done) => done,
            Err(e) => {
                let _ = txn.rollback().await;
                match &e {
                    CommitError::LeaseLost => lost("commit"),
                    CommitError::MemoryRejected(MemoryApplyError::Conflict { .. }) => {
                        metrics::counter!(names::AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL).increment(1);
                    }
                    _ => {}
                }
                return Err(e);
            }
        };
        self.faults.reach(FaultStage::BeforeCommit).await;
        // Step 7. A failed COMMIT, or a future that the caller drops, leaves the outcome unknown,
        // and the handle is poisoned.
        commit_or_poison(h, txn.commit()).await.map_err(|_| CommitError::OutcomeUnknown)?;
        let mut ids = Vec::with_capacity(done.len());
        for (id, attempts) in done {
            metrics::histogram!(names::AGENT_HOST_EVENT_ATTEMPTS).record(f64::from(attempts));
            ids.push(EventId::from_uuid(id));
        }
        Ok(Committed { run: r.run.clone(), done: ids })
    }

    /// Steps 1 to 6 in the open transaction. It returns the events that went to `done`, with
    /// their attempts.
    async fn steps(&self, txn: &DatabaseTransaction, inbox: &InboxId, r: &RunRecord) -> Result<Vec<(Uuid, i32)>, CommitError> {
        let inbox = inbox.uuid();
        let run = r.run.uuid();
        let token = r.token.0;
        txn.execute_unprepared(LOCK_TIMEOUT_SQL).await.map_err(db_err)?;

        // Step 1: lock the lease row and check the token and the expiry.
        if txn.query_one_raw(stmt(LOCK_SQL, [inbox.into(), token.into()])).await.map_err(db_err)?.is_none() {
            return Err(CommitError::LeaseLost);
        }

        // Step 2: move the run out of `running`, fenced by its token.
        let run_row = match &r.class {
            RunClass::Completed { output, usage } => txn.query_one_raw(run_done(run, token, "completed", output.clone(), None, usage)).await,
            RunClass::Stopped { output, reason, usage } => txn.query_one_raw(run_done(run, token, "stopped", output.clone(), Some(reason.clone()), usage)).await,
            RunClass::Deterministic { error, .. } => txn.query_one_raw(stmt(RUN_FAILED_SQL, [run.into(), token.into(), error.clone().into()])).await,
        }
        .map_err(db_err)?;
        let Some(run_row) = run_row else {
            return Err(CommitError::LeaseLost);
        };
        let thread: Uuid = run_row.try_get("", "thread_id").map_err(db_err)?;

        // Steps 3 and 4.
        match &r.class {
            RunClass::Completed { .. } | RunClass::Stopped { .. } => {
                write_session(txn, inbox, thread, run, &r.nonce, &r.session).await.map_err(db_err)?;
                if let Some(row) = txn.query_one_raw(stmt(THREAD_SQL, [thread.into(), inbox.into()])).await.map_err(db_err)? {
                    let predecessor: Option<Uuid> = row.try_get("", "predecessor_id").map_err(db_err)?;
                    if let Some(predecessor) = predecessor {
                        txn.execute_raw(stmt(SUCCESSOR_SQL, [predecessor.into(), inbox.into(), thread.into()])).await.map_err(db_err)?;
                    }
                }
            }
            RunClass::Deterministic { .. } => {
                if r.close_thread {
                    txn.execute_raw(stmt(CLOSE_THREAD_SQL, [thread.into(), inbox.into()])).await.map_err(db_err)?;
                }
            }
        }

        // Step 5: the memory set.
        self.apply_memory(txn, inbox, &r.memory).await?;

        // Step 6: the events.
        match &r.class {
            RunClass::Completed { .. } | RunClass::Stopped { .. } => {
                let passed: Vec<Uuid> = r.passed.iter().map(EventId::uuid).collect();
                let rows = txn.query_all_raw(stmt(DONE_SQL, [run.into(), passed.into()])).await.map_err(db_err)?;
                rows.iter()
                    .map(|row| Ok((row.try_get::<Uuid>("", "id").map_err(db_err)?, row.try_get::<i32>("", "attempts").map_err(db_err)?)))
                    .collect()
            }
            RunClass::Deterministic { error, retry } => {
                for delay in retry {
                    let micros = i64::try_from(delay.delay.as_micros()).unwrap_or(i64::MAX);
                    txn.execute_raw(stmt(RETRY_SQL, [delay.event.uuid().into(), run.into(), micros.into(), error.clone().into()]))
                        .await
                        .map_err(db_err)?;
                }
                Ok(Vec::new())
            }
        }
    }

    /// Step 5: the scope of the set must be the scope of the inbox row. Then `apply_in`, when the
    /// set has a change or a guard.
    async fn apply_memory(&self, txn: &DatabaseTransaction, inbox: Uuid, set: &MemoryCommitSet) -> Result<(), CommitError> {
        let scope = &set.scope;
        let values = [inbox.into(), scope.org().into(), scope.agent().uuid().into(), scope.principal().uuid().into()];
        if txn.query_one_raw(stmt(SCOPE_SQL, values)).await.map_err(db_err)?.is_none() {
            return Err(CommitError::Store(SCOPE_REFUSED.to_string()));
        }
        if set.is_empty() {
            return Ok(());
        }
        let applied = match &self.memory {
            MemoryCommit::InTransaction(store) => store.apply_in(txn, set, &self.caps).await,
        };
        applied.map_err(|e| match e {
            MemoryApplyError::Conflict { .. } | MemoryApplyError::CapExceeded(_) => CommitError::MemoryRejected(e),
            MemoryApplyError::Store(_) | MemoryApplyError::Partial { .. } => CommitError::Store(e.to_string()),
        })
    }
}

/// The step-2 statement of a full commit.
fn run_done(run: Uuid, token: i64, status: &str, output: Option<String>, reason: Option<String>, usage: &RunUsage) -> sea_orm::Statement {
    let big = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    stmt(
        RUN_DONE_SQL,
        [
            run.into(),
            token.into(),
            status.into(),
            output.into(),
            reason.into(),
            big(usage.input_tokens).into(),
            big(usage.output_tokens).into(),
            big(usage.cached_input_tokens).into(),
            big(usage.reasoning_tokens).into(),
        ],
    )
}

/// The record with U+0000 replaced in the output, the error and the stop reason. `write_session`
/// replaces it in the session rows.
fn without_nul(mut r: RunRecord) -> RunRecord {
    r.class = match r.class {
        RunClass::Completed { output, usage } => RunClass::Completed {
            output: output.as_deref().map(replace_nul),
            usage,
        },
        RunClass::Stopped { output, reason, usage } => RunClass::Stopped {
            output: output.as_deref().map(replace_nul),
            reason: replace_nul(&reason),
            usage,
        },
        RunClass::Deterministic { error, retry } => RunClass::Deterministic { error: replace_nul(&error), retry },
    };
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_agent_host_core::ids::RunId;
    use paigasus_agent_host_core::memory::{MemoryChangeSet, MemoryScope};
    use paigasus_agent_host_core::run::LeaseToken;

    fn record(class: RunClass) -> RunRecord {
        let scope = MemoryScope::new(
            paigasus_agent_host_core::ids::AgentId::new(Uuid::nil(), Uuid::nil()),
            paigasus_agent_host_core::ids::PrincipalId::from_uuid(Uuid::nil()),
        );
        RunRecord {
            run: RunId::new(Uuid::nil(), Uuid::nil()),
            token: LeaseToken(1),
            passed: Vec::new(),
            class,
            session: Vec::new(),
            nonce: "n".into(),
            close_thread: false,
            memory: MemoryCommitSet::new(scope, MemoryChangeSet::new()),
        }
    }

    #[test]
    fn nul_is_replaced_in_the_output_the_reason_and_the_error() {
        let stopped = without_nul(record(RunClass::Stopped {
            output: Some("a\u{0}".into()),
            reason: "b\u{0}".into(),
            usage: RunUsage::default(),
        }));
        assert_eq!(
            stopped.class,
            RunClass::Stopped {
                output: Some("a\u{FFFD}".into()),
                reason: "b\u{FFFD}".into(),
                usage: RunUsage::default(),
            }
        );
        let failed = without_nul(record(RunClass::Deterministic {
            error: "c\u{0}".into(),
            retry: Vec::new(),
        }));
        assert_eq!(
            failed.class,
            RunClass::Deterministic {
                error: "c\u{FFFD}".into(),
                retry: Vec::new(),
            }
        );
    }
}
