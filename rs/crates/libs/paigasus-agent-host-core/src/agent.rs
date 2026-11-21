// SPDX-License-Identifier: Apache-2.0

//! The agent definition and the resolved agent.

use crate::ids::{AgentId, PrincipalId};

/// The stored definition of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: AgentId,
    pub name: String,
    pub model_id: String,
    pub instructions: String,
    pub tool_names: Vec<String>,
}

/// An agent definition together with the principal that the agent acts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgent {
    pub definition: AgentDefinition,
    pub principal: PrincipalId,
}
