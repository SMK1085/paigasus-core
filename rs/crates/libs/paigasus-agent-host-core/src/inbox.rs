// SPDX-License-Identifier: Apache-2.0

//! The inbox and its events.

use jiff::Timestamp;
use uuid::Uuid;

use crate::error::DomainError;
use crate::ids::{AgentId, EventId, InboxId, PrincipalId, RunId, ThreadId};

/// An inbox. It belongs to one agent and one principal, in one organization.
///
/// `Inbox::new` is the only constructor, because it enforces the organization invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    id: InboxId,
    agent: AgentId,
    principal: PrincipalId,
    created_at: Timestamp,
}

impl Inbox {
    /// Make an inbox. The organization of the agent must be the organization of the inbox.
    pub fn new(id: InboxId, agent: AgentId, principal: PrincipalId, created_at: Timestamp) -> Result<Inbox, DomainError> {
        if id.org() != agent.org() {
            return Err(DomainError::OrgMismatch {
                inbox_org: id.org(),
                agent_org: agent.org(),
            });
        }
        Ok(Inbox { id, agent, principal, created_at })
    }

    /// The identity of the inbox.
    #[must_use]
    pub fn id(&self) -> &InboxId {
        &self.id
    }

    /// The agent that owns the inbox.
    #[must_use]
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// The principal of the inbox.
    #[must_use]
    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    /// The time the inbox was created.
    #[must_use]
    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// The organization of the inbox. It comes from the inbox PRN.
    #[must_use]
    pub fn org(&self) -> Uuid {
        self.id.org()
    }
}

/// The processing state of an inbox event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Pending,
    Done,
    Dead,
}

/// An event in an inbox.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxEvent {
    pub id: EventId,
    pub inbox: InboxId,
    pub source: String,
    pub ce_id: String,
    pub ce_type: String,
    pub thread: Option<ThreadId>,
    pub received_at: Timestamp,
    pub status: EventStatus,
    pub attempts: u32,
    pub not_before: Timestamp,
    pub claimed_run: Option<RunId>,
    /// The full CloudEvents envelope as JSON.
    pub envelope: serde_json::Value,
    /// The `data` member of the envelope, or `Null`.
    pub data: serde_json::Value,
}

/// An event to insert into an inbox. The store sets the status, the attempts and the times.
#[derive(Debug, Clone, PartialEq)]
pub struct NewInboxEvent {
    pub id: EventId,
    pub inbox: InboxId,
    pub source: String,
    pub ce_id: String,
    pub ce_type: String,
    pub thread: Option<ThreadId>,
    pub envelope: serde_json::Value,
}
