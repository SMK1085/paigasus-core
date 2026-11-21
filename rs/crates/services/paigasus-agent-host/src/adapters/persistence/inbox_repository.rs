// SPDX-License-Identifier: Apache-2.0

//! The Postgres adapter of `InboxRepository` (spec §7.2).

use async_trait::async_trait;
use paigasus_agent_host_core::error::DomainError;
use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, PrincipalId, RunId, ThreadId};
use paigasus_agent_host_core::inbox::{EventStatus, Inbox, InboxEvent, NewInboxEvent};
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, RepositoryError, StoreError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement, Value};
use uuid::Uuid;

use super::time::from_micros;

/// One statement. The thread subselect keeps a thread of another inbox out of the foreign key:
/// the row then gets a `NULL` thread. A conflict on the dedup key writes nothing.
const INSERT_SQL: &str = "INSERT INTO inbox_event (id, inbox_id, source, ce_id, ce_type, thread_id, envelope) \
     VALUES ($1, $2, $3, $4, $5, (SELECT t.id FROM thread t WHERE t.id = $6 AND t.inbox_id = $2), $7::jsonb) \
     ON CONFLICT (inbox_id, source, ce_id) DO NOTHING \
     RETURNING id";

/// The stored row of a key. `data` is `NULL` for a missing key and for JSON `null`.
const STORED_SQL: &str = "SELECT id, ce_type, (envelope -> 'data')::text AS data FROM inbox_event \
     WHERE inbox_id = $1 AND source = $2 AND ce_id = $3";

const INBOX_SQL: &str = "SELECT org_id, agent_id, principal_id, \
     (EXTRACT(EPOCH FROM created_at) * 1000000)::bigint AS created_us FROM inbox WHERE id = $1";

/// The columns that `row_to_event` reads, as a literal for `concat!`, for a `SELECT` or a
/// `RETURNING` on `inbox_event`. Times cross the boundary as epoch microseconds.
macro_rules! event_columns {
    () => {
        "id, source, ce_id, ce_type, thread_id, status, attempts::bigint AS attempts, claimed_run_id, \
         envelope::text AS envelope, \
         (EXTRACT(EPOCH FROM received_at) * 1000000)::bigint AS received_us, \
         (EXTRACT(EPOCH FROM not_before) * 1000000)::bigint AS not_before_us"
    };
}
pub(crate) use event_columns;

/// Map a row with the `event_columns!()` of an event of `inbox` to the domain event.
pub(crate) fn row_to_event(row: &QueryResult, inbox: &InboxId) -> Result<InboxEvent, StoreError> {
    let err = |e: DbErr| StoreError(e.to_string());
    let status: String = row.try_get("", "status").map_err(err)?;
    let status = match status.as_str() {
        "pending" => EventStatus::Pending,
        "done" => EventStatus::Done,
        "dead" => EventStatus::Dead,
        other => return Err(StoreError(format!("unknown inbox event status {other:?}"))),
    };
    let attempts: i64 = row.try_get("", "attempts").map_err(err)?;
    let attempts = u32::try_from(attempts).map_err(|_| StoreError(format!("attempts out of range: {attempts}")))?;
    let thread: Option<Uuid> = row.try_get("", "thread_id").map_err(err)?;
    let run: Option<Uuid> = row.try_get("", "claimed_run_id").map_err(err)?;
    let envelope: String = row.try_get("", "envelope").map_err(err)?;
    let envelope: serde_json::Value = serde_json::from_str(&envelope).map_err(|e| StoreError(e.to_string()))?;
    let data = envelope.get("data").cloned().unwrap_or(serde_json::Value::Null);
    Ok(InboxEvent {
        id: EventId::from_uuid(row.try_get("", "id").map_err(err)?),
        inbox: inbox.clone(),
        source: row.try_get("", "source").map_err(err)?,
        ce_id: row.try_get("", "ce_id").map_err(err)?,
        ce_type: row.try_get("", "ce_type").map_err(err)?,
        thread: thread.map(ThreadId::from_uuid),
        received_at: from_micros(row.try_get("", "received_us").map_err(err)?)?,
        status,
        attempts,
        not_before: from_micros(row.try_get("", "not_before_us").map_err(err)?)?,
        claimed_run: run.map(|r| RunId::new(inbox.org(), r)),
        envelope,
        data,
    })
}

/// The inbox repository on a Postgres pool.
#[derive(Debug, Clone)]
pub struct PgInboxRepository {
    db: DatabaseConnection,
}

impl PgInboxRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn store(e: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Store(e.to_string())
}

fn db_err(e: DbErr) -> RepositoryError {
    store(e)
}

fn domain_err(e: DomainError) -> RepositoryError {
    store(e)
}

/// JSON `null` and an absent value are the same: both become `None`.
fn normalize(v: Option<serde_json::Value>) -> Option<serde_json::Value> {
    v.filter(|v| !v.is_null())
}

#[async_trait]
impl InboxRepository for PgInboxRepository {
    async fn get_by_uuid(&self, id: Uuid) -> Result<Option<Inbox>, RepositoryError> {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, INBOX_SQL, [id.into()]);
        let Some(row) = self.db.query_one_raw(stmt).await.map_err(db_err)? else {
            return Ok(None);
        };
        let org: Uuid = row.try_get("", "org_id").map_err(db_err)?;
        let agent: Uuid = row.try_get("", "agent_id").map_err(db_err)?;
        let principal: Uuid = row.try_get("", "principal_id").map_err(db_err)?;
        let created_us: i64 = row.try_get("", "created_us").map_err(db_err)?;
        let created_at = from_micros(created_us).map_err(store)?;
        let inbox = Inbox::new(InboxId::new(org, id), AgentId::new(org, agent), PrincipalId::from_uuid(principal), created_at).map_err(domain_err)?;
        Ok(Some(inbox))
    }

    async fn insert_event(&self, event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError> {
        let inbox = event.inbox.uuid();
        let thread: Option<Uuid> = event.thread.as_ref().map(|t| t.uuid());
        let values: Vec<Value> = vec![
            event.id.uuid().into(),
            inbox.into(),
            event.source.clone().into(),
            event.ce_id.clone().into(),
            event.ce_type.clone().into(),
            thread.into(),
            event.envelope.to_string().into(),
        ];
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, INSERT_SQL, values);
        if let Some(row) = self.db.query_one_raw(stmt).await.map_err(db_err)? {
            let id: Uuid = row.try_get("", "id").map_err(db_err)?;
            return Ok(InsertOutcome::Inserted { id: EventId::from_uuid(id) });
        }

        // No row: a stored row holds the key. Compare its type and data in Rust.
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, STORED_SQL, [inbox.into(), event.source.clone().into(), event.ce_id.clone().into()]);
        let row = self
            .db
            .query_one_raw(stmt)
            .await
            .map_err(db_err)?
            .ok_or_else(|| RepositoryError::Store("the stored row of a duplicate key is gone".to_string()))?;
        let id: Uuid = row.try_get("", "id").map_err(db_err)?;
        let stored_type: String = row.try_get("", "ce_type").map_err(db_err)?;
        let stored_data: Option<String> = row.try_get("", "data").map_err(db_err)?;
        let stored_data = stored_data.map(|s| serde_json::from_str::<serde_json::Value>(&s)).transpose().map_err(store)?;
        let new_data = event.envelope.get("data").cloned();
        let payload_matches = stored_type == event.ce_type && normalize(stored_data) == normalize(new_data);
        Ok(InsertOutcome::Duplicate {
            id: EventId::from_uuid(id),
            payload_matches,
        })
    }
}
