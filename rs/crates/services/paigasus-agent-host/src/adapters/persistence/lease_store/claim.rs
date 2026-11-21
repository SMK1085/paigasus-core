// SPDX-License-Identifier: Apache-2.0

//! The claim of a batch of events (spec §8.4, §8.4.1). One transaction rotates the token, locks
//! the lease row, moves exhausted events to `dead`, resolves the threads, groups one batch and
//! opens one run for it.

use std::collections::{HashMap, HashSet};

use paigasus_agent_host_core::claim::{Claim, ClaimConfig};
use paigasus_agent_host_core::ids::{InboxId, RunId, ThreadId};
use paigasus_agent_host_core::lease::{ClaimError, LeaseHandle};
use paigasus_agent_host_core::run::LeaseToken;
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, TransactionTrait, Value};
use uuid::Uuid;

use super::take::ttl_micros;
use super::{PgLeaseStore, commit_or_poison, lost};
use crate::adapters::id::mint_uuid;
use crate::adapters::persistence::inbox_repository::{event_columns, row_to_event};

/// The `last_error` of an event that a claim moves to `dead`.
pub const DEAD_REASON: &str = "max attempts reached without a commit";

/// `$1` = inbox, `$2` = token. Zero rows means that the lease is lost. A rotation makes every
/// run of the old token stale, so its events are free again.
const ROTATE_SQL: &str = "UPDATE inbox_lease SET token = token + 1 \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp() RETURNING token";

/// `$1` = inbox, `$2` = token. No row means that the lease is lost.
const LOCK_SQL: &str = "SELECT token FROM inbox_lease \
     WHERE inbox_id = $1 AND token = $2 AND expires_at > clock_timestamp() FOR UPDATE";

/// The "free" rule of §8.4 step 4 on the alias `e`, with `$2` = the current token. A run of a
/// crashed worker has an old token, so its events are free with no reaper.
macro_rules! free {
    () => {
        "(e.claimed_run_id IS NULL OR NOT EXISTS (SELECT 1 FROM run r \
          WHERE r.id = e.claimed_run_id AND r.status = 'running' AND r.lease_token = $2))"
    };
}

/// `$1` = inbox, `$2` = token, `$3` = max attempts, `$4` = `DEAD_REASON` (§8.4.1).
const DEAD_SQL: &str = concat!(
    "UPDATE inbox_event e SET status = 'dead', last_error = $4 \
     WHERE e.inbox_id = $1 AND e.status = 'pending' AND e.not_before <= now() AND e.attempts >= $3 AND ",
    free!(),
    " RETURNING e.id"
);

/// `$1` = inbox, `$2` = token, `$3` = max attempts. The claimable events, the oldest first.
const CLAIMABLE_SQL: &str = concat!(
    "SELECT e.id, e.thread_id FROM inbox_event e \
     WHERE e.inbox_id = $1 AND e.status = 'pending' AND e.not_before <= now() AND e.attempts < $3 AND ",
    free!(),
    " ORDER BY e.received_at, e.id"
);

/// `$1` = inbox, `$2` = the start threads. One row per start: the end of its successor chain, or
/// a row with `is_cycle` when the chain has a cycle.
const RESOLVE_SQL: &str = "WITH RECURSIVE chain (start_id, id, successor_id) AS ( \
       SELECT t.id, t.id, t.successor_id FROM thread t WHERE t.inbox_id = $1 AND t.id = ANY($2) \
       UNION ALL \
       SELECT c.start_id, t.id, t.successor_id \
       FROM chain c JOIN thread t ON t.id = c.successor_id AND t.inbox_id = $1 \
     ) CYCLE id SET is_cycle USING path \
     SELECT start_id, id AS end_id, is_cycle FROM chain WHERE successor_id IS NULL OR is_cycle";

/// `$1` = thread, `$2` = thread ttl in microseconds, `$3` = inbox.
const LIVENESS_SQL: &str = "SELECT closed, \
       (last_run_at IS NULL OR last_run_at >= now() - ($2::bigint * interval '1 microsecond')) AS recent, \
       last_run_at IS NULL AS never_run \
     FROM thread WHERE id = $1 AND inbox_id = $3";

/// `$1` = thread, `$2` = inbox, `$3` = predecessor or `NULL`.
const INSERT_THREAD_SQL: &str = "INSERT INTO thread (id, inbox_id, predecessor_id) VALUES ($1, $2, $3)";

/// `$1` = run, `$2` = inbox, `$3` = thread, `$4` = token.
const INSERT_RUN_SQL: &str = "INSERT INTO run (id, inbox_id, thread_id, lease_token, status) VALUES ($1, $2, $3, $4, 'running')";

/// `$1` = run, `$2` = the claimed events.
const INSERT_RUN_EVENT_SQL: &str = "INSERT INTO run_event (run_id, event_id) SELECT $1, unnest($2::uuid[])";

/// `$1` = run, `$2` = thread, `$3` = inbox, `$4` = the claimed events. The events stay `pending`.
const MARK_SQL: &str = concat!(
    "UPDATE inbox_event SET claimed_run_id = $1, thread_id = $2, attempts = attempts + 1 \
     WHERE inbox_id = $3 AND id = ANY($4) RETURNING ",
    event_columns!()
);

fn store(e: impl std::fmt::Display) -> ClaimError {
    ClaimError::Store(e.to_string())
}

fn stmt(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The outcome of the transaction, before the commit.
enum Step {
    /// The lease is lost. Roll back.
    Lost,
    /// Commit. `token` is the current token, `dead` the number of dead moves.
    Commit { token: LeaseToken, dead: u64, claim: Option<Claim> },
}

impl PgLeaseStore {
    /// Claim one batch of events under the lease of `handle` (spec §8.4). `Ok(None)` means that no
    /// event is claimable; the lease is still held. A `COMMIT` error, or a claim future that the
    /// caller drops during the `COMMIT`, leaves the outcome unknown, so it poisons the handle.
    pub async fn claim_events(&self, handle: &mut LeaseHandle, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError> {
        handle.check_claim()?;
        let txn = self.db.begin().await.map_err(store)?;
        match claim_in(&txn, handle, cfg).await? {
            Step::Lost => {
                txn.rollback().await.map_err(store)?;
                lost("claim");
                Err(ClaimError::LeaseLost)
            }
            Step::Commit { token, dead, claim } => {
                // The database may have rotated the token and opened the run. If the commit fails
                // or the caller drops this future during it, the handle keeps the old token and no
                // open claim, so it is poisoned (as in `complete`).
                commit_or_poison(handle, txn.commit()).await.map_err(store)?;
                handle.record_claim(token, claim.as_ref().map(|c| c.run.clone()));
                if dead > 0 {
                    metrics::counter!(names::AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL).increment(dead);
                }
                if claim.is_some() {
                    metrics::counter!(names::AGENT_HOST_CLAIMS_TOTAL).increment(1);
                }
                Ok(claim)
            }
        }
    }
}

/// Steps 1 to 8 of §8.4 in the open transaction.
async fn claim_in(txn: &DatabaseTransaction, handle: &LeaseHandle, cfg: &ClaimConfig) -> Result<Step, ClaimError> {
    let inbox = handle.inbox().uuid();

    // Step 1: rotate the token, except on the first claim after a take.
    let mut token = handle.token();
    if handle.needs_rotation() {
        let Some(row) = txn.query_one_raw(stmt(ROTATE_SQL, [inbox.into(), token.0.into()])).await.map_err(store)? else {
            return Ok(Step::Lost);
        };
        token = LeaseToken(row.try_get("", "token").map_err(store)?);
    }

    // Step 2: lock the lease row and check it.
    if txn.query_one_raw(stmt(LOCK_SQL, [inbox.into(), token.0.into()])).await.map_err(store)?.is_none() {
        return Ok(Step::Lost);
    }

    // Step 3: move exhausted events to `dead`.
    let max_attempts = i64::from(cfg.max_attempts);
    let dead = txn
        .query_all_raw(stmt(DEAD_SQL, [inbox.into(), token.0.into(), max_attempts.into(), DEAD_REASON.into()]))
        .await
        .map_err(store)?
        .len();
    let dead = u64::try_from(dead).unwrap_or(u64::MAX);

    // Step 4: the claimable events, the oldest first.
    let rows = txn.query_all_raw(stmt(CLAIMABLE_SQL, [inbox.into(), token.0.into(), max_attempts.into()])).await.map_err(store)?;
    let mut claimable: Vec<(Uuid, Option<Uuid>)> = Vec::with_capacity(rows.len());
    for row in rows {
        claimable.push((row.try_get("", "id").map_err(store)?, row.try_get("", "thread_id").map_err(store)?));
    }
    let Some(&(_, oldest_thread)) = claimable.first() else {
        return Ok(Step::Commit { token, dead, claim: None });
    };

    // Step 5: resolve each thread to the end of its successor chain.
    let resolved = resolve(txn, handle.inbox(), &claimable).await?;
    let key = |thread: Option<Uuid>| thread.and_then(|t| resolved.get(&t).copied().flatten());

    // Step 6: the group of the oldest event.
    let group = key(oldest_thread);
    let limit = usize::try_from(cfg.batch_limit).unwrap_or(usize::MAX);
    let events: Vec<Uuid> = claimable.iter().filter(|(_, t)| key(*t) == group).map(|(id, _)| *id).take(limit).collect();
    let (thread, fresh_thread) = run_thread(txn, inbox, group, cfg).await?;

    // Step 7: the run and one `run_event` row per event.
    let run = mint_uuid();
    txn.execute_raw(stmt(INSERT_RUN_SQL, [run.into(), inbox.into(), thread.into(), token.0.into()])).await.map_err(store)?;
    txn.execute_raw(stmt(INSERT_RUN_EVENT_SQL, [run.into(), events.clone().into()])).await.map_err(store)?;

    // Step 8: mark the events. They stay `pending`; their thread becomes the run's thread.
    let rows = txn
        .query_all_raw(stmt(MARK_SQL, [run.into(), thread.into(), inbox.into(), events.clone().into()]))
        .await
        .map_err(store)?;
    let mut marked = HashMap::with_capacity(rows.len());
    for row in &rows {
        let event = row_to_event(row, handle.inbox()).map_err(|e| ClaimError::Store(e.0))?;
        marked.insert(event.id.uuid(), event);
    }
    let events = events
        .iter()
        .map(|id| marked.remove(id).ok_or_else(|| ClaimError::Store(format!("claimed event {id} was not marked"))))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Step::Commit {
        token,
        dead,
        claim: Some(Claim {
            run: RunId::new(handle.inbox().org(), run),
            thread: ThreadId::from_uuid(thread),
            lease_token: token,
            fresh_thread,
            events,
        }),
    })
}

/// The resolved thread of each start thread of `claimable`. A start whose chain has a cycle
/// resolves to `None`: it is a data error, logged once per start.
async fn resolve(txn: &DatabaseTransaction, inbox: &InboxId, claimable: &[(Uuid, Option<Uuid>)]) -> Result<HashMap<Uuid, Option<Uuid>>, ClaimError> {
    let starts: Vec<Uuid> = claimable.iter().filter_map(|(_, t)| *t).collect::<HashSet<_>>().into_iter().collect();
    let mut resolved = HashMap::with_capacity(starts.len());
    if starts.is_empty() {
        return Ok(resolved);
    }
    let rows = txn.query_all_raw(stmt(RESOLVE_SQL, [inbox.uuid().into(), starts.into()])).await.map_err(store)?;
    for row in rows {
        let start: Uuid = row.try_get("", "start_id").map_err(store)?;
        let end: Uuid = row.try_get("", "end_id").map_err(store)?;
        let is_cycle: bool = row.try_get("", "is_cycle").map_err(store)?;
        if is_cycle {
            if resolved.insert(start, None) != Some(None) {
                tracing::warn!(inbox = %inbox.uuid(), thread = %start, "the successor chain of a thread has a cycle; its events are claimed as having no thread");
            }
        } else {
            resolved.entry(start).or_insert(Some(end));
        }
    }
    Ok(resolved)
}

/// The thread of the run and its `fresh_thread` flag. A live resolved thread is used as it is. A
/// thread that is not live gets a new successor thread, and no thread gets a new thread.
async fn run_thread(txn: &DatabaseTransaction, inbox: Uuid, resolved: Option<Uuid>, cfg: &ClaimConfig) -> Result<(Uuid, bool), ClaimError> {
    if let Some(thread) = resolved {
        let row = txn
            .query_one_raw(stmt(LIVENESS_SQL, [thread.into(), ttl_micros(cfg.thread_ttl).into(), inbox.into()]))
            .await
            .map_err(store)?
            .ok_or_else(|| ClaimError::Store(format!("resolved thread {thread} is gone")))?;
        let closed: bool = row.try_get("", "closed").map_err(store)?;
        let recent: bool = row.try_get("", "recent").map_err(store)?;
        let never_run: bool = row.try_get("", "never_run").map_err(store)?;
        if !closed && recent {
            return Ok((thread, never_run));
        }
    }
    let new = mint_uuid();
    txn.execute_raw(stmt(INSERT_THREAD_SQL, [new.into(), inbox.into(), resolved.into()])).await.map_err(store)?;
    Ok((new, true))
}
