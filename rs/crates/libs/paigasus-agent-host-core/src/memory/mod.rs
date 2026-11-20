// SPDX-License-Identifier: Apache-2.0

//! The memory store value types: scope, paths, versions and the change set.

mod caps;
mod change;
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod path;
mod workspace;

pub use caps::{CapError, MemoryCaps};
pub use change::{Expected, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryVersion};
pub use path::{MemoryDir, MemoryPath, ParsedToolPath, PathError, TOOL_ROOT, parse_tool_path};
pub use workspace::{MemoryWorkspace, WorkspaceError};

use crate::ids::{AgentId, PrincipalId};
use uuid::Uuid;

/// The owner of a set of memory files: one agent and one principal. The scope comes from the
/// resolved agent. No tool argument and no path can carry it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemoryScope {
    agent: AgentId,
    principal: PrincipalId,
}

impl MemoryScope {
    /// Build a scope from an agent and a principal.
    #[must_use]
    pub fn new(agent: AgentId, principal: PrincipalId) -> Self {
        MemoryScope { agent, principal }
    }

    /// The agent of the scope.
    #[must_use]
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// The principal of the scope.
    #[must_use]
    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    /// The organization of the scope. It is the organization of the agent.
    #[must_use]
    pub fn org(&self) -> Uuid {
        self.agent.org()
    }
}
