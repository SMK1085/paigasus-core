// SPDX-License-Identifier: Apache-2.0

//! The agent resolver of the worker (SMA-852 spec §6.2, §6.8): one query joins the inbox row to
//! its `agent` row. The row of an archived agent does not join, so the answer is `NotFound`.

use async_trait::async_trait;
use paigasus_agent_host_core::agent::{AgentDefinition, ResolvedAgent};
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::{AgentResolver, RepositoryError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// The inbox, joined to the active agent of the same org. The join holds the org, so the same
/// UUID in another org is another agent.
const RESOLVE_SQL: &str = "SELECT a.id AS agent_id, a.name, a.model, a.instructions, a.tools, i.principal_id \
     FROM inbox i JOIN agent a ON a.org_id = i.org_id AND a.id = i.agent_id AND a.archived_at IS NULL \
     WHERE i.id = $1 AND i.org_id = $2";

/// Resolve an inbox to the stored definition of its agent.
pub struct PgAgentResolver {
    db: DatabaseConnection,
}

impl PgAgentResolver {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn store(e: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Store(e.to_string())
}

#[async_trait]
impl AgentResolver for PgAgentResolver {
    async fn resolve(&self, inbox: &InboxId) -> Result<ResolvedAgent, RepositoryError> {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, RESOLVE_SQL, [inbox.uuid().into(), inbox.org().into()]);
        let row = self.db.query_one_raw(stmt).await.map_err(store)?.ok_or(RepositoryError::NotFound)?;
        let agent: Uuid = row.try_get("", "agent_id").map_err(store)?;
        let principal: Uuid = row.try_get("", "principal_id").map_err(store)?;
        Ok(ResolvedAgent {
            definition: AgentDefinition {
                id: AgentId::new(inbox.org(), agent),
                name: row.try_get("", "name").map_err(store)?,
                model_id: row.try_get("", "model").map_err(store)?,
                instructions: row.try_get("", "instructions").map_err(store)?,
                tool_names: row.try_get("", "tools").map_err(store)?,
            },
            principal: PrincipalId::from_uuid(principal),
        })
    }
}
