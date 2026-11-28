// SPDX-License-Identifier: Apache-2.0

//! The Postgres adapter of `AgentRegistry` (SMA-852, spec §6.3 and §6.4).

use async_trait::async_trait;
use jiff::Timestamp;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::inbox::Inbox;
use paigasus_agent_host_core::ports::AgentRegistry;
use paigasus_agent_host_core::registry::{AgentFields, AgentRecord, InboxRecord, PAGE_SIZE_DEFAULT, Page, PageRequest, RegistryError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, SqlErr, Statement, TransactionTrait, Value};
use uuid::Uuid;

use super::time::from_micros;

/// The name of the partial unique index on the name of an active agent.
const NAME_INDEX: &str = "agent_org_name_active";

/// The columns that `row_to_agent` reads. Times cross the boundary as epoch microseconds.
macro_rules! agent_columns {
    () => {
        "id, org_id, name, model, instructions, tools, version, \
         (EXTRACT(EPOCH FROM created_at) * 1000000)::bigint AS created_us, \
         (EXTRACT(EPOCH FROM updated_at) * 1000000)::bigint AS updated_us, \
         (EXTRACT(EPOCH FROM archived_at) * 1000000)::bigint AS archived_us"
    };
}

const INSERT_AGENT_SQL: &str = concat!(
    "INSERT INTO agent (id, org_id, name, model, instructions, tools) VALUES ($2, $1, $3, $4, $5, $6) RETURNING ",
    agent_columns!()
);

const GET_AGENT_SQL: &str = concat!("SELECT ", agent_columns!(), " FROM agent WHERE id = $1");

const GET_AGENT_IN_ORG_SQL: &str = concat!("SELECT ", agent_columns!(), " FROM agent WHERE org_id = $1 AND id = $2");

const LIST_AGENTS_SQL: &str = concat!(
    "SELECT ",
    agent_columns!(),
    " FROM agent WHERE org_id = $1 AND ($2 OR archived_at IS NULL) AND ($3::uuid IS NULL OR id > $3) ORDER BY id LIMIT $4"
);

const UPDATE_AGENT_SQL: &str = concat!(
    "UPDATE agent SET name = $4, model = $5, instructions = $6, tools = $7, version = version + 1, updated_at = now() \
     WHERE org_id = $1 AND id = $2 AND version = $3 AND archived_at IS NULL RETURNING ",
    agent_columns!()
);

const ARCHIVE_AGENT_SQL: &str = concat!("UPDATE agent SET archived_at = COALESCE(archived_at, now()) WHERE org_id = $1 AND id = $2 RETURNING ", agent_columns!());

const CANCEL_TIMERS_SQL: &str = "UPDATE timer SET status = 'cancelled', updated_at = now() \
     WHERE status = 'active' AND inbox_id IN (SELECT id FROM inbox WHERE org_id = $1 AND agent_id = $2)";

const LOCK_AGENT_SQL: &str = "SELECT archived_at IS NOT NULL AS archived FROM agent WHERE org_id = $1 AND id = $2 FOR SHARE";

/// A conflict on `(agent_id, principal_id)` writes nothing, so the transaction stays usable and
/// the code can read the existing inbox. A conflict on the primary key still raises an error.
const INSERT_INBOX_SQL: &str = "INSERT INTO inbox (id, org_id, agent_id, principal_id) VALUES ($1, $2, $3, $4) \
     ON CONFLICT (agent_id, principal_id) DO NOTHING \
     RETURNING (EXTRACT(EPOCH FROM created_at) * 1000000)::bigint AS created_us";

const EXISTING_INBOX_SQL: &str = "SELECT id FROM inbox WHERE agent_id = $1 AND principal_id = $2";

/// The select of `row_to_inbox`.
macro_rules! inbox_select {
    () => {
        "SELECT i.id, i.org_id, i.agent_id, i.principal_id, \
         (EXTRACT(EPOCH FROM i.created_at) * 1000000)::bigint AS created_us, \
         (EXTRACT(EPOCH FROM a.archived_at) * 1000000)::bigint AS archived_us \
         FROM inbox i JOIN agent a ON a.org_id = i.org_id AND a.id = i.agent_id "
    };
}

const GET_INBOX_SQL: &str = concat!(inbox_select!(), "WHERE i.id = $1");

const LIST_INBOXES_SQL: &str = concat!(inbox_select!(), "WHERE i.org_id = $1 AND i.agent_id = $2 AND ($3::uuid IS NULL OR i.id > $3) ORDER BY i.id LIMIT $4");

const ACTIVE_MODELS_SQL: &str = "SELECT id, org_id, model FROM agent WHERE archived_at IS NULL ORDER BY id";

/// The agent registry on a Postgres pool.
#[derive(Debug, Clone)]
pub struct PgAgentRegistry {
    db: DatabaseConnection,
}

impl PgAgentRegistry {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// The active agents with their model ids, for the boot warning.
    pub async fn active_models(&self) -> Result<Vec<(AgentId, String)>, RegistryError> {
        let stmt = Statement::from_string(DatabaseBackend::Postgres, ACTIVE_MODELS_SQL.to_string());
        let rows = self.db.query_all_raw(stmt).await.map_err(store)?;
        rows.iter()
            .map(|row| {
                let id: Uuid = row.try_get("", "id").map_err(store)?;
                let org: Uuid = row.try_get("", "org_id").map_err(store)?;
                let model: String = row.try_get("", "model").map_err(store)?;
                Ok((AgentId::new(org, id), model))
            })
            .collect()
    }
}

fn store(e: impl std::fmt::Display) -> RegistryError {
    RegistryError::Store(e.to_string())
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// True when the error is a unique violation of the index or constraint `name`.
fn is_unique_violation(e: &DbErr, name: &str) -> bool {
    matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(msg)) if msg.contains(name))
}

fn opt_time(us: Option<i64>) -> Result<Option<Timestamp>, RegistryError> {
    us.map(|us| from_micros(us).map_err(store)).transpose()
}

fn row_to_agent(row: &QueryResult) -> Result<AgentRecord, RegistryError> {
    let id: Uuid = row.try_get("", "id").map_err(store)?;
    let org: Uuid = row.try_get("", "org_id").map_err(store)?;
    let version: i64 = row.try_get("", "version").map_err(store)?;
    Ok(AgentRecord {
        definition: AgentDefinition {
            id: AgentId::new(org, id),
            name: row.try_get("", "name").map_err(store)?,
            model_id: row.try_get("", "model").map_err(store)?,
            instructions: row.try_get("", "instructions").map_err(store)?,
            tool_names: row.try_get("", "tools").map_err(store)?,
        },
        version: u64::try_from(version).map_err(|_| RegistryError::Store(format!("version out of range: {version}")))?,
        created_at: from_micros(row.try_get("", "created_us").map_err(store)?).map_err(store)?,
        updated_at: from_micros(row.try_get("", "updated_us").map_err(store)?).map_err(store)?,
        archived_at: opt_time(row.try_get("", "archived_us").map_err(store)?)?,
    })
}

fn row_to_inbox(row: &QueryResult) -> Result<InboxRecord, RegistryError> {
    let id: Uuid = row.try_get("", "id").map_err(store)?;
    let org: Uuid = row.try_get("", "org_id").map_err(store)?;
    let agent: Uuid = row.try_get("", "agent_id").map_err(store)?;
    let principal: Uuid = row.try_get("", "principal_id").map_err(store)?;
    let created_at = from_micros(row.try_get("", "created_us").map_err(store)?).map_err(store)?;
    let inbox = Inbox::new(InboxId::new(org, id), AgentId::new(org, agent), PrincipalId::from_uuid(principal), created_at).map_err(store)?;
    Ok(InboxRecord {
        inbox,
        agent_archived_at: opt_time(row.try_get("", "archived_us").map_err(store)?)?,
    })
}

/// The size of a page, and the SQL limit: one more row than the page, to see whether a next page
/// exists.
fn page_limit(page: PageRequest) -> (usize, i64) {
    let size = if page.size == 0 { PAGE_SIZE_DEFAULT } else { page.size };
    (size as usize, i64::from(size) + 1)
}

/// Cut the extra row off and set `next_after` to the id of the last returned row.
fn finish_page<T>(mut items: Vec<T>, size: usize, id_of: impl Fn(&T) -> Uuid) -> Page<T> {
    let next_after = if items.len() > size {
        items.truncate(size);
        items.last().map(id_of)
    } else {
        None
    };
    Page { items, next_after }
}

#[async_trait]
impl AgentRegistry for PgAgentRegistry {
    async fn create(&self, id: &AgentId, fields: &AgentFields) -> Result<AgentRecord, RegistryError> {
        let values = vec![
            id.org().into(),
            id.uuid().into(),
            fields.name.clone().into(),
            fields.model_id.clone().into(),
            fields.instructions.clone().into(),
            fields.tool_names.clone().into(),
        ];
        match self.db.query_one_raw(stmt(INSERT_AGENT_SQL, values)).await {
            Ok(Some(row)) => row_to_agent(&row),
            Ok(None) => Err(RegistryError::Store("the insert of an agent returned no row".to_string())),
            Err(e) if is_unique_violation(&e, NAME_INDEX) => Err(RegistryError::NameTaken),
            Err(e) => Err(store(e)),
        }
    }

    async fn get(&self, id: Uuid) -> Result<Option<AgentRecord>, RegistryError> {
        let row = self.db.query_one_raw(stmt(GET_AGENT_SQL, vec![id.into()])).await.map_err(store)?;
        row.as_ref().map(row_to_agent).transpose()
    }

    async fn list(&self, org: Uuid, include_archived: bool, page: PageRequest) -> Result<Page<AgentRecord>, RegistryError> {
        let (size, limit) = page_limit(page);
        let rows = self
            .db
            .query_all_raw(stmt(LIST_AGENTS_SQL, vec![org.into(), include_archived.into(), page.after.into(), limit.into()]))
            .await
            .map_err(store)?;
        let items = rows.iter().map(row_to_agent).collect::<Result<Vec<_>, _>>()?;
        Ok(finish_page(items, size, |a| a.definition.id.uuid()))
    }

    async fn update(&self, id: &AgentId, version: u64, fields: &AgentFields) -> Result<AgentRecord, RegistryError> {
        let version = i64::try_from(version).map_err(|_| RegistryError::Invalid("version"))?;
        let tx = self.db.begin().await.map_err(store)?;
        let values = vec![
            id.org().into(),
            id.uuid().into(),
            version.into(),
            fields.name.clone().into(),
            fields.model_id.clone().into(),
            fields.instructions.clone().into(),
            fields.tool_names.clone().into(),
        ];
        match tx.query_one_raw(stmt(UPDATE_AGENT_SQL, values)).await {
            Ok(Some(row)) => {
                let record = row_to_agent(&row)?;
                tx.commit().await.map_err(store)?;
                return Ok(record);
            }
            Ok(None) => {}
            Err(e) if is_unique_violation(&e, NAME_INDEX) => return Err(RegistryError::NameTaken),
            Err(e) => return Err(store(e)),
        }
        // No row matched. Read the row again in the same transaction to tell why.
        let row = tx.query_one_raw(stmt(GET_AGENT_IN_ORG_SQL, vec![id.org().into(), id.uuid().into()])).await.map_err(store)?;
        let current = row.as_ref().map(row_to_agent).transpose()?;
        tx.commit().await.map_err(store)?;
        match current {
            None => Err(RegistryError::NotFound),
            Some(r) if r.archived_at.is_some() => Err(RegistryError::Archived),
            Some(r) => Err(RegistryError::VersionMismatch { current: r.version }),
        }
    }

    async fn archive(&self, id: &AgentId) -> Result<AgentRecord, RegistryError> {
        let tx = self.db.begin().await.map_err(store)?;
        let row = tx.query_one_raw(stmt(ARCHIVE_AGENT_SQL, vec![id.org().into(), id.uuid().into()])).await.map_err(store)?;
        let Some(row) = row else {
            return Err(RegistryError::NotFound);
        };
        let record = row_to_agent(&row)?;
        tx.execute_raw(stmt(CANCEL_TIMERS_SQL, vec![id.org().into(), id.uuid().into()])).await.map_err(store)?;
        tx.commit().await.map_err(store)?;
        Ok(record)
    }

    async fn create_inbox(&self, id: &InboxId, agent: &AgentId, principal: &PrincipalId) -> Result<InboxRecord, RegistryError> {
        // The org check of `Inbox::new` runs before any SQL.
        Inbox::new(id.clone(), agent.clone(), principal.clone(), Timestamp::UNIX_EPOCH).map_err(|_| RegistryError::Invalid("org"))?;
        let tx = self.db.begin().await.map_err(store)?;
        // FOR SHARE conflicts with the archive UPDATE, so a bind cannot commit after an archive.
        let locked = tx.query_one_raw(stmt(LOCK_AGENT_SQL, vec![agent.org().into(), agent.uuid().into()])).await.map_err(store)?;
        let Some(locked) = locked else {
            return Err(RegistryError::NotFound);
        };
        if locked.try_get::<bool>("", "archived").map_err(store)? {
            return Err(RegistryError::Archived);
        }
        let values = vec![id.uuid().into(), agent.org().into(), agent.uuid().into(), principal.uuid().into()];
        let Some(row) = tx.query_one_raw(stmt(INSERT_INBOX_SQL, values)).await.map_err(store)? else {
            let existing = tx.query_one_raw(stmt(EXISTING_INBOX_SQL, vec![agent.uuid().into(), principal.uuid().into()])).await.map_err(store)?;
            let existing: Uuid = existing
                .ok_or_else(|| RegistryError::Store("the inbox of a conflicting bind is gone".to_string()))?
                .try_get("", "id")
                .map_err(store)?;
            return Err(RegistryError::InboxExists(InboxId::new(agent.org(), existing)));
        };
        let created_at = from_micros(row.try_get("", "created_us").map_err(store)?).map_err(store)?;
        tx.commit().await.map_err(store)?;
        let inbox = Inbox::new(id.clone(), agent.clone(), principal.clone(), created_at).map_err(store)?;
        Ok(InboxRecord { inbox, agent_archived_at: None })
    }

    async fn get_inbox(&self, id: Uuid) -> Result<Option<InboxRecord>, RegistryError> {
        let row = self.db.query_one_raw(stmt(GET_INBOX_SQL, vec![id.into()])).await.map_err(store)?;
        row.as_ref().map(row_to_inbox).transpose()
    }

    async fn list_inboxes(&self, agent: &AgentId, page: PageRequest) -> Result<Page<InboxRecord>, RegistryError> {
        let (size, limit) = page_limit(page);
        let rows = self
            .db
            .query_all_raw(stmt(LIST_INBOXES_SQL, vec![agent.org().into(), agent.uuid().into(), page.after.into(), limit.into()]))
            .await
            .map_err(store)?;
        let items = rows.iter().map(row_to_inbox).collect::<Result<Vec<_>, _>>()?;
        Ok(finish_page(items, size, |r| r.inbox.id().uuid()))
    }
}
