// SPDX-License-Identifier: Apache-2.0

//! The agent resolver of the worker (spec §6.2): the `[[agents]]` list of the configuration.
//! The inbox row gives the agent UUID and the principal. The full `AgentId` (the organization
//! and the UUID) then selects the definition.

use std::collections::HashMap;

use async_trait::async_trait;
use paigasus_agent_host_core::agent::{AgentDefinition, ResolvedAgent};
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::{AgentResolver, RepositoryError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// Resolve an inbox to a configured agent.
pub struct ConfigAgentResolver {
    db: DatabaseConnection,
    agents: HashMap<AgentId, AgentDefinition>,
}

impl ConfigAgentResolver {
    #[must_use]
    pub fn new(db: DatabaseConnection, agents: Vec<AgentDefinition>) -> Self {
        Self {
            db,
            agents: agents.into_iter().map(|def| (def.id.clone(), def)).collect(),
        }
    }

    /// The agents that this host serves.
    #[must_use]
    pub fn served(&self) -> Vec<AgentId> {
        self.agents.keys().cloned().collect()
    }
}

#[async_trait]
impl AgentResolver for ConfigAgentResolver {
    async fn resolve(&self, inbox: &InboxId) -> Result<ResolvedAgent, RepositoryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT agent_id, principal_id FROM inbox WHERE id = $1 AND org_id = $2",
            [inbox.uuid().into(), inbox.org().into()],
        );
        let row = self.db.query_one_raw(stmt).await.map_err(|e| RepositoryError::Store(e.to_string()))?.ok_or(RepositoryError::NotFound)?;
        let agent: Uuid = row.try_get("", "agent_id").map_err(|e| RepositoryError::Store(e.to_string()))?;
        let principal: Uuid = row.try_get("", "principal_id").map_err(|e| RepositoryError::Store(e.to_string()))?;
        // The key holds the organization too: the same UUID in another organization is another agent.
        let definition = self.agents.get(&AgentId::new(inbox.org(), agent)).ok_or(RepositoryError::NotFound)?.clone();
        Ok(ResolvedAgent {
            definition,
            principal: PrincipalId::from_uuid(principal),
        })
    }
}
