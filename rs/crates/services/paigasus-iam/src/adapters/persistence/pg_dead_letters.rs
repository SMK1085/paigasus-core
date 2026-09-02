// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed [`DeadLetters`] (SMA-469): inspect and retire `event_outbox` rows the relay
//! parked. `parked = true` IS the dead-letter predicate — there is no separate table (a
//! dedicated one would cost a move-on-park inside the relay's transaction, a move-back replay
//! path, and would render the `parked` column vestigial, all to express a set one boolean
//! already expresses).
//!
//! The three mutating methods write on the CALLER's transaction (recovered via
//! [`recover_txn`], exactly like `PgOutbox::enqueue`) so the mutation and its audit entry
//! commit atomically on one `UnitOfWork`.
//!
//! **Every mutating statement carries `AND parked = true`**, so a live or already-published row
//! is untouchable through this surface — these endpoints can never be used to mutate the live
//! queue.
//!
//! They use `RETURNING *` and go through `Statement` + `query_one` (`execute` discards the
//! returned row), so the caller gets the affected row's contents for its audit entry. For
//! `discard_in` that audit entry is the discarded event's ONLY remaining trace.
//!
//! **A caveat for time filters, not a bug:** `parked_from`/`parked_to` (on both `list` and
//! `replay_matching_in`) filter on `parked_at`, and Postgres never evaluates a `NULL` comparison
//! as true — so a row with `parked_at IS NULL` cannot satisfy `>=`/`<=` against ANY bound and is
//! invisible to every time-filtered call. It remains fully visible via an unfiltered `list` and
//! reachable via an unfiltered (or only `event_type`-filtered) `replay_matching_in`, so nothing
//! is permanently lost — but this is easy to mistake for a bug when triaging why a known-parked
//! row didn't show up in a windowed query. `PgOutboxMaintainer`'s parked-row sweep has the exact
//! same blind spot for the exact same reason (it requires `parked_at IS NOT NULL` before
//! comparing to a cutoff): both surfaces inherit it from `event_outbox::Model::parked_at`'s own
//! doc, which notes the column isn't schema-enforced non-NULL for a parked row.

use super::entities::event_outbox;
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterEntry, DeadLetterFilter, DeadLetters, RepositoryError};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter, QueryOrder, QueryResult, QuerySelect, Statement, Value};
use uuid::Uuid;

/// `$1` = id. Note `last_error` is deliberately NOT cleared: clearing it would destroy the
/// evidence chain when a replayed row re-parks for a different reason — the operator would then
/// see only the second failure. `parked_at`/`attempts` DO reset, because they describe the
/// row's current state; the error string is history.
const REPLAY_ONE_SQL: &str = r#"UPDATE "event_outbox" SET parked = false, attempts = 0, parked_at = NULL
                                WHERE id = $1 AND parked = true RETURNING *"#;

/// `$1` = id.
const DISCARD_ONE_SQL: &str = r#"DELETE FROM "event_outbox" WHERE id = $1 AND parked = true RETURNING *"#;

/// Builds `replay_matching_in`'s bulk `UPDATE`. `predicate` is a `filter_clauses` output
/// (`parked = true [AND …]`); `limit_placeholder` is the 1-based index of the `LIMIT` param in
/// the caller's bound `params` vec (i.e. `params.len()` after the limit value itself is pushed).
///
/// Extracted to a plain function — rather than inlined via `format!` inside the `async fn`, as
/// originally written — specifically so it is unit-testable: an integration test cannot
/// distinguish "replayed the right rows" from "replayed the right rows without `SKIP LOCKED`" or
/// "without ascending order", so those properties can only be pinned down here, at the SQL
/// level. Mirrors `pg_outbox_maintainer.rs`'s `published_sweep_sql`/`parked_sweep_sql` split.
///
/// The outer `WHERE id IN (...) AND parked = true` repeats the subquery's own `parked = true`
/// scoping. That repetition isn't required for correctness today — the subquery's
/// `FOR UPDATE` takes the row locks and Postgres re-checks the subquery's qual after locking, so
/// a row that stopped being parked between the snapshot and the lock is already excluded — but
/// it makes the "never touch a live row" guarantee direct here too, rather than the one mutating
/// statement where it held only indirectly through a nested qual.
///
/// The subquery selects `ORDER BY id` ASCENDING (unlike `list`'s `DESC`): when a filter matches
/// more rows than `max_rows`, repeated calls then walk the backlog forward instead of
/// re-selecting the same newest slice.
///
/// **`FOR UPDATE SKIP LOCKED` is required, not an optimization.** Postgres does not guarantee an
/// `UPDATE ... WHERE id IN (SELECT ... ORDER BY ...)` takes row locks in the subquery's order, so
/// two concurrent bulk replays with overlapping filters can deadlock; a non-deadlocking overlap
/// instead blocks the second operator for the whole of the first's transaction — which includes
/// its audit write and commit. `SKIP LOCKED` makes concurrent replays partition rather than
/// collide, and an operator responding to an outage is precisely the person most likely to fire
/// two of these.
fn bulk_replay_sql(predicate: &str, limit_placeholder: usize) -> String {
    format!(
        r#"UPDATE "event_outbox" SET parked = false, attempts = 0, parked_at = NULL
           WHERE id IN (
             SELECT id FROM "event_outbox" WHERE {predicate}
             ORDER BY id LIMIT ${limit_placeholder} FOR UPDATE SKIP LOCKED
           ) AND parked = true"#
    )
}

/// Builds the shared `parked = true [AND …]` predicate, appending each present filter value to
/// `params` and numbering its placeholder from the vec's running length (so a caller that has
/// already bound values gets a correct continuation, not a restarted sequence).
fn filter_clauses(event_type: &Option<String>, parked_from: &Option<DateTime<Utc>>, parked_to: &Option<DateTime<Utc>>, params: &mut Vec<Value>) -> String {
    let mut sql = "parked = true".to_string();
    if let Some(t) = event_type {
        params.push(Value::from(t.clone()));
        sql.push_str(&format!(" AND event_type = ${}", params.len()));
    }
    if let Some(f) = parked_from {
        params.push(Value::from(*f));
        sql.push_str(&format!(" AND parked_at >= ${}", params.len()));
    }
    if let Some(t) = parked_to {
        params.push(Value::from(*t));
        sql.push_str(&format!(" AND parked_at <= ${}", params.len()));
    }
    sql
}

fn model_to_entry(m: event_outbox::Model) -> DeadLetterEntry {
    DeadLetterEntry {
        id: m.id,
        occurred_at: m.occurred_at,
        event_type: m.event_type,
        schema_version: m.schema_version,
        aggregate_prn: m.aggregate_prn,
        actor_prn: m.actor_prn,
        payload: m.payload,
        correlation_id: m.correlation_id,
        attempts: m.attempts.max(0) as u32,
        parked_at: m.parked_at,
        last_error: m.last_error,
    }
}

/// Projects a `RETURNING *` row. Column names mirror `event_outbox`'s schema exactly.
fn row_to_entry(r: &QueryResult) -> Result<DeadLetterEntry, RepositoryError> {
    Ok(DeadLetterEntry {
        id: r.try_get("", "id").map_err(map_err)?,
        occurred_at: r.try_get("", "occurred_at").map_err(map_err)?,
        event_type: r.try_get("", "event_type").map_err(map_err)?,
        schema_version: r.try_get("", "schema_version").map_err(map_err)?,
        aggregate_prn: r.try_get("", "aggregate_prn").map_err(map_err)?,
        actor_prn: r.try_get("", "actor_prn").map_err(map_err)?,
        payload: r.try_get("", "payload").map_err(map_err)?,
        correlation_id: r.try_get("", "correlation_id").map_err(map_err)?,
        attempts: r.try_get::<i32>("", "attempts").map_err(map_err)?.max(0) as u32,
        parked_at: r.try_get("", "parked_at").map_err(map_err)?,
        last_error: r.try_get("", "last_error").map_err(map_err)?,
    })
}

/// `Clone`: `DatabaseConnection` is an `Arc`-backed pool handle, mirroring every other adapter
/// in this module.
#[derive(Clone)]
pub struct PgDeadLetters {
    db: DatabaseConnection,
}

impl PgDeadLetters {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgDeadLetters { db }
    }
}

#[async_trait]
impl DeadLetters for PgDeadLetters {
    /// Keyset paging by `id DESC` (`id < cursor`), mirroring `PgAuditLog::query`. Outbox ids
    /// are UUIDv7 (`KernelIdGenerator::mint`), so id order IS time order — newest first, which
    /// is what an operator inspecting a backlog wants.
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError> {
        let mut q = event_outbox::Entity::find().filter(event_outbox::Column::Parked.eq(true));
        if let Some(t) = &f.event_type {
            q = q.filter(event_outbox::Column::EventType.eq(t.clone()));
        }
        if let Some(from) = f.parked_from {
            q = q.filter(event_outbox::Column::ParkedAt.gte(from));
        }
        if let Some(to) = f.parked_to {
            q = q.filter(event_outbox::Column::ParkedAt.lte(to));
        }
        if let Some(cursor) = f.cursor {
            q = q.filter(event_outbox::Column::Id.lt(cursor));
        }
        let models = q.order_by_desc(event_outbox::Column::Id).limit(f.capped_limit()).all(&self.db).await.map_err(map_err)?;
        Ok(models.into_iter().map(model_to_entry).collect())
    }

    async fn replay_in(&self, tx: &dyn paigasus_iam_core::Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, REPLAY_ONE_SQL, [Value::from(id)]);
        match txn.query_one_raw(stmt).await.map_err(map_err)? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }

    /// See [`bulk_replay_sql`] for the statement itself and why its shape (`SKIP LOCKED`,
    /// ascending order, the doubled `parked = true` scope) is load-bearing.
    async fn replay_matching_in(&self, tx: &dyn paigasus_iam_core::Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError> {
        let txn = recover_txn(tx)?;
        let mut params: Vec<Value> = Vec::new();
        let predicate = filter_clauses(&r.event_type, &r.parked_from, &r.parked_to, &mut params);
        params.push(Value::from(r.capped_max_rows() as i64));
        let sql = bulk_replay_sql(&predicate, params.len());
        let res = txn.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, &sql, params)).await.map_err(map_err)?;
        Ok(res.rows_affected())
    }

    async fn discard_in(&self, tx: &dyn paigasus_iam_core::Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, DISCARD_ONE_SQL, [Value::from(id)]);
        match txn.query_one_raw(stmt).await.map_err(map_err)? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_sql_always_scopes_to_parked_rows() {
        let mut params: Vec<Value> = Vec::new();
        let sql = filter_clauses(&None, &None, &None, &mut params);
        assert_eq!(sql, "parked = true", "an unfiltered request must still be scoped to parked rows");
        assert!(params.is_empty());
    }

    #[test]
    fn filter_sql_binds_each_present_field_positionally() {
        let mut params: Vec<Value> = Vec::new();
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let sql = filter_clauses(&Some("iam.principal.created".to_string()), &Some(from), &Some(to), &mut params);
        assert_eq!(sql, "parked = true AND event_type = $1 AND parked_at >= $2 AND parked_at <= $3");
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn filter_sql_numbers_params_from_the_existing_offset() {
        // This is a property of the helper, not an observed call pattern: `filter_clauses` must
        // stay correct for ANY caller that has already bound some params before calling it.
        // Today `replay_matching_in` is the only caller and it binds nothing beforehand — `list`
        // never calls this helper at all, it builds its own query via the SeaORM entity API —
        // but the offset logic itself must not assume that, so this seeds a prior binding and
        // checks the placeholder continues from it rather than restarting at `$1`.
        let mut params: Vec<Value> = vec![Value::from(1i64)];
        let sql = filter_clauses(&Some("x".to_string()), &None, &None, &mut params);
        assert_eq!(sql, "parked = true AND event_type = $2");
    }

    #[test]
    fn every_mutating_statement_is_scoped_to_parked_rows() {
        // A live or already-published row must be untouchable through this surface.
        assert!(REPLAY_ONE_SQL.contains("parked = true"), "{REPLAY_ONE_SQL}");
        assert!(DISCARD_ONE_SQL.contains("parked = true"), "{DISCARD_ONE_SQL}");
        // Replay must NOT clear last_error: a re-parked row would otherwise lose the original
        // evidence and show only the second failure.
        assert!(!REPLAY_ONE_SQL.contains("last_error = NULL"), "replay must preserve last_error");
    }

    #[test]
    fn bulk_replay_sql_locks_the_subquery_with_skip_locked() {
        let sql = bulk_replay_sql("parked = true", 1);
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
    }

    #[test]
    fn bulk_replay_sql_orders_the_subquery_ascending_not_descending() {
        let sql = bulk_replay_sql("parked = true", 1);
        // A bare `contains("ORDER BY id")` would still pass against a regressed
        // `ORDER BY id DESC` — assert the placeholder immediately follows `id` instead, which a
        // `DESC` regression breaks (it inserts a token between them).
        assert!(sql.contains("ORDER BY id LIMIT $1"), "{sql}");
    }

    #[test]
    fn bulk_replay_sql_scopes_the_outer_update_to_parked_rows_directly() {
        let sql = bulk_replay_sql("parked = true", 1);
        // The subquery's own predicate already starts `parked = true` (a `filter_clauses`
        // output); this asserts the OUTER `UPDATE`'s `WHERE` repeats the scope too, rather than
        // relying solely on the nested qual.
        assert!(sql.trim_end().ends_with("AND parked = true"), "{sql}");
    }

    #[test]
    fn bulk_replay_sql_places_limit_at_the_given_placeholder_index() {
        let sql = bulk_replay_sql("parked = true AND event_type = $1", 2);
        assert!(sql.contains("LIMIT $2"), "{sql}");
    }
}
