// SPDX-License-Identifier: Apache-2.0

//! The application service of the agent API (SMA-852 spec §6.2, §6.5, §6.7).
//!
//! The order in every method (spec §6.5):
//! 1. Find the target org: the org of the request, or the row that `registry.get*` reads by id.
//!    A row whose org differs from [`ResourceKey::org`] counts as no row.
//! 2. No row: [`deny_unknown`] with the action of the call (the probe), then `Forbidden`.
//! 3. [`authorize_org`] with the action of the call: the key scope, then the self-query.
//! 4. The page size and the page token.
//! 5. The field rules, then the model, then the tools.
//! 6. The registry call.
//!
//! So a caller without the right gets `Forbidden` also for a request with bad fields, and an
//! unknown id, an id of another org, a key of another org and a missing right give the same answer.

pub mod error;
pub mod page_token;

use std::collections::HashSet;
use std::sync::Arc;

use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::{AgentRegistry, IdSource, RunCapabilities};
use paigasus_agent_host_core::registry::{AGENT_NAME_MAX_CHARS, AgentFields, AgentRecord, INSTRUCTIONS_MAX_BYTES, InboxRecord, PAGE_SIZE_DEFAULT, PAGE_SIZE_MAX, PageRequest};
use uuid::Uuid;

pub use self::error::AdminError;
use crate::adapters::iam::Iam;
use crate::application::auth::{Caller, MANAGE_AGENTS_ACTION, READ_AGENTS_ACTION, authorize_org, deny_unknown};

/// The id of a row that a call names. `org` is the org of the PRN over gRPC, and `None` over
/// REST (a bare uuid). A row of another org is handled exactly as an unknown id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceKey {
    pub id: Uuid,
    pub org: Option<Uuid>,
}

/// The agent API rules over the registry port. The gRPC service and the REST routes call this
/// service. They do not call the port.
pub struct AgentAdmin {
    registry: Arc<dyn AgentRegistry>,
    capabilities: Arc<dyn RunCapabilities>,
    iam: Arc<dyn Iam>,
    ids: Arc<dyn IdSource>,
}

impl AgentAdmin {
    #[must_use]
    pub fn new(registry: Arc<dyn AgentRegistry>, capabilities: Arc<dyn RunCapabilities>, iam: Arc<dyn Iam>, ids: Arc<dyn IdSource>) -> Self {
        Self { registry, capabilities, iam, ids }
    }

    /// The IAM link. The transports authenticate the caller through it.
    #[must_use]
    pub fn iam(&self) -> &Arc<dyn Iam> {
        &self.iam
    }

    /// Create an agent in `org`.
    pub async fn create(&self, caller: &Caller, org: Uuid, fields: AgentFields) -> Result<AgentRecord, AdminError> {
        authorize_org(self.iam.as_ref(), caller, org, MANAGE_AGENTS_ACTION).await?;
        self.check_fields(&fields)?;
        let id = AgentId::new(org, self.ids.new_uuid());
        let record = self.registry.create(&id, &fields).await?;
        log_agent_write("CreateAgent", caller, &record);
        Ok(record)
    }

    /// Read an agent.
    pub async fn get(&self, caller: &Caller, agent: ResourceKey) -> Result<AgentRecord, AdminError> {
        self.authorized_agent(caller, agent, READ_AGENTS_ACTION).await
    }

    /// List the agents of `org`, in order of id. It returns the next page token, or `None` on the
    /// last page.
    pub async fn list(&self, caller: &Caller, org: Uuid, include_archived: bool, page_size: u32, page_token: &str) -> Result<(Vec<AgentRecord>, Option<String>), AdminError> {
        authorize_org(self.iam.as_ref(), caller, org, READ_AGENTS_ACTION).await?;
        let size = page_size_of(page_size)?;
        let after = page_token::decode(page_token, org, include_archived)?;
        let page = self.registry.list(org, include_archived, PageRequest { size, after }).await?;
        let next = page.next_after.map(|last| page_token::encode(org, include_archived, last));
        Ok((page.items, next))
    }

    /// Replace the fields of an agent. `version` is the version that the caller read.
    pub async fn update(&self, caller: &Caller, agent: ResourceKey, version: u64, fields: AgentFields) -> Result<AgentRecord, AdminError> {
        let row = self.authorized_agent(caller, agent, MANAGE_AGENTS_ACTION).await?;
        self.check_fields(&fields)?;
        let record = self.registry.update(&row.definition.id, version, &fields).await?;
        log_agent_write("UpdateAgent", caller, &record);
        Ok(record)
    }

    /// Archive an agent.
    pub async fn archive(&self, caller: &Caller, agent: ResourceKey) -> Result<AgentRecord, AdminError> {
        let row = self.authorized_agent(caller, agent, MANAGE_AGENTS_ACTION).await?;
        let record = self.registry.archive(&row.definition.id).await?;
        log_agent_write("ArchiveAgent", caller, &record);
        Ok(record)
    }

    /// Bind a new inbox of `principal` to an agent.
    pub async fn create_inbox(&self, caller: &Caller, agent: ResourceKey, principal: PrincipalId) -> Result<InboxRecord, AdminError> {
        let row = self.authorized_agent(caller, agent, MANAGE_AGENTS_ACTION).await?;
        let agent_id = &row.definition.id;
        let id = InboxId::new(agent_id.org(), self.ids.new_uuid());
        let record = self.registry.create_inbox(&id, agent_id, &principal).await?;
        tracing::info!(
            action = "CreateInbox",
            principal = %caller.principal.as_prn().canonical(),
            agent = %agent_id.as_prn().canonical(),
            inbox = %record.inbox.id().as_prn().canonical(),
            version = row.version,
            "the agent registry changed"
        );
        Ok(record)
    }

    /// Read an inbox.
    pub async fn get_inbox(&self, caller: &Caller, inbox: ResourceKey) -> Result<InboxRecord, AdminError> {
        let row = self.registry.get_inbox(inbox.id).await?.filter(|r| in_org(inbox, r.inbox.org()));
        let Some(row) = row else {
            return Err(deny_unknown(self.iam.as_ref(), caller, READ_AGENTS_ACTION).await.into());
        };
        authorize_org(self.iam.as_ref(), caller, row.inbox.org(), READ_AGENTS_ACTION).await?;
        Ok(row)
    }

    /// List the inboxes of an agent, in order of id. An unknown agent, or an agent of another org,
    /// gives the probe answer, never an empty page.
    pub async fn list_inboxes(&self, caller: &Caller, agent: ResourceKey, page_size: u32, page_token: &str) -> Result<(Vec<InboxRecord>, Option<String>), AdminError> {
        let row = self.authorized_agent(caller, agent, READ_AGENTS_ACTION).await?;
        let agent_id = &row.definition.id;
        let size = page_size_of(page_size)?;
        let after = page_token::decode(page_token, agent_id.uuid(), false)?;
        let page = self.registry.list_inboxes(agent_id, PageRequest { size, after }).await?;
        let next = page.next_after.map(|last| page_token::encode(agent_id.uuid(), false, last));
        Ok((page.items, next))
    }

    /// Spec §6.5 steps 1 to 3 for an agent by id: read the row (a row of another org is no row),
    /// probe on no row, then authorize `action` on the org of the row.
    async fn authorized_agent(&self, caller: &Caller, agent: ResourceKey, action: &'static str) -> Result<AgentRecord, AdminError> {
        let row = self.registry.get(agent.id).await?.filter(|r| in_org(agent, r.definition.id.org()));
        let Some(row) = row else {
            return Err(deny_unknown(self.iam.as_ref(), caller, action).await.into());
        };
        authorize_org(self.iam.as_ref(), caller, row.definition.id.org(), action).await?;
        Ok(row)
    }

    /// The field rules, then the model, then the tools (spec §6.7). The error names the field,
    /// the model or the tool, never the rest of the caller's value.
    fn check_fields(&self, fields: &AgentFields) -> Result<(), AdminError> {
        let name = &fields.name;
        let name_chars = name.chars().count();
        if !(1..=AGENT_NAME_MAX_CHARS).contains(&name_chars) || name.chars().any(char::is_control) || name.trim() != name {
            return Err(AdminError::InvalidField("name"));
        }
        let instructions = &fields.instructions;
        if !(1..=INSTRUCTIONS_MAX_BYTES).contains(&instructions.len()) || instructions.contains('\0') {
            return Err(AdminError::InvalidField("instructions"));
        }
        let mut seen = HashSet::with_capacity(fields.tool_names.len());
        if !fields.tool_names.iter().all(|t| seen.insert(t.as_str())) {
            return Err(AdminError::InvalidField("tools"));
        }
        if !self.capabilities.has_model(&fields.model_id) {
            return Err(AdminError::UnknownModel(fields.model_id.clone()));
        }
        if let Some(tool) = fields.tool_names.iter().find(|t| !self.capabilities.has_tool(t)) {
            return Err(AdminError::UnknownTool(tool.clone()));
        }
        Ok(())
    }
}

/// Is a row of `row_org` the row that `key` names? A key with no org (REST) names every org.
fn in_org(key: ResourceKey, row_org: Uuid) -> bool {
    key.org.is_none_or(|org| org == row_org)
}

/// The page size of a request: `0` is the default, `1..=PAGE_SIZE_MAX` is kept, and a larger
/// value is [`AdminError::InvalidPagination`].
fn page_size_of(requested: u32) -> Result<u32, AdminError> {
    match requested {
        0 => Ok(PAGE_SIZE_DEFAULT),
        size if size <= PAGE_SIZE_MAX => Ok(size),
        _ => Err(AdminError::InvalidPagination),
    }
}

/// The info log of an agent write (spec §6.5). It never logs the instructions.
fn log_agent_write(action: &'static str, caller: &Caller, record: &AgentRecord) {
    tracing::info!(
        action,
        principal = %caller.principal.as_prn().canonical(),
        agent = %record.definition.id.as_prn().canonical(),
        version = record.version,
        "the agent registry changed"
    );
}

#[cfg(test)]
mod tests;
