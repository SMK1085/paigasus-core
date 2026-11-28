// SPDX-License-Identifier: Apache-2.0

//! The core types of the agent registry (SMA-852). The ports are in `ports.rs`.

use jiff::Timestamp;
use uuid::Uuid;

use crate::agent::AgentDefinition;
use crate::ids::InboxId;
use crate::inbox::Inbox;

/// The maximum length of an agent name, in characters.
pub const AGENT_NAME_MAX_CHARS: usize = 64;
/// The maximum size of the instructions of an agent, in bytes.
pub const INSTRUCTIONS_MAX_BYTES: usize = 32_768;
/// The page size that is used when a request gives none.
pub const PAGE_SIZE_DEFAULT: u32 = 50;
/// The largest page size that a request may ask for.
pub const PAGE_SIZE_MAX: u32 = 100;

/// The fields of an agent that a caller sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFields {
    pub name: String,
    pub model_id: String,
    pub instructions: String,
    pub tool_names: Vec<String>,
}

/// A stored agent with its version and its times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub definition: AgentDefinition,
    pub version: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived_at: Option<Timestamp>,
}

/// An inbox together with the archive time of its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxRecord {
    pub inbox: Inbox,
    pub agent_archived_at: Option<Timestamp>,
}

/// A request for one page of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub size: u32,
    pub after: Option<Uuid>,
}

/// One page of a list. `next_after` is `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_after: Option<Uuid>,
}

/// The error of the agent registry. The text of each variant is fixed and holds no caller input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("not found")]
    NotFound,
    #[error("agent name taken")]
    NameTaken,
    #[error("inbox exists")]
    InboxExists(InboxId),
    #[error("version mismatch")]
    VersionMismatch { current: u64 },
    #[error("agent archived")]
    Archived,
    #[error("invalid field {0}")]
    Invalid(&'static str),
    #[error("unknown model")]
    UnknownModel(String),
    #[error("unknown tool")]
    UnknownTool(String),
    #[error("store error")]
    Store(String),
}
