// SPDX-License-Identifier: Apache-2.0

//! The Helikon adapter: the only place where the agent host meets the agent runtime.

pub mod classify;
pub mod memory_tool;
pub mod model_registry;
pub mod runner;
pub mod scripted;
pub mod session;
pub mod timer_tools;

use std::sync::Arc;

use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::memory::MemoryWorkspace;
use paigasus_helikon::core::LlmAgent;
use tokio::sync::Mutex;

use self::memory_tool::MemoryTool;
use self::model_registry::SharedModel;

/// The tool name that gives an agent the `memory` tool.
pub const MEMORY_TOOL_NAME: &str = "memory";

/// Build the Helikon agent for a stored agent definition.
///
/// The name and the instructions come from the definition: the system role carries only the
/// instructions (spec §6.4). The agent gets the `memory` tool over `ws` only when its tool names
/// hold `memory`. No other tool name is mapped (timers are out of scope, spec N4). The caller
/// resolves `model_id` through the model registry.
pub fn build_agent(def: &AgentDefinition, model: SharedModel, ws: Arc<Mutex<MemoryWorkspace>>) -> LlmAgent<(), SharedModel, String> {
    let builder = LlmAgent::builder::<()>().name(def.name.clone()).model(model).instructions(def.instructions.clone());
    let builder = if def.tool_names.iter().any(|t| t == MEMORY_TOOL_NAME) {
        builder.tool(MemoryTool::new(ws))
    } else {
        builder
    };
    builder.build()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_agent_host_core::agent::AgentDefinition;
    use paigasus_agent_host_core::ids::AgentId;
    use paigasus_agent_host_core::memory::conformance::fresh_scope;
    use paigasus_agent_host_core::memory::fake::InMemoryMemoryStore;
    use paigasus_agent_host_core::memory::{MemoryCaps, MemoryWorkspace};
    use paigasus_helikon::core::{Agent, CancellationToken, HookRegistry, MemorySession, RunContext, TracerHandle};
    use tokio::sync::Mutex;

    use super::build_agent;
    use super::model_registry::SharedModel;
    use super::scripted::ScriptedModel;

    fn def(tool_names: &[&str]) -> AgentDefinition {
        let id = AgentId::parse("prn:pgs:agents::00000000-0000-0000-0000-000000000001:agent/00000000-0000-0000-0000-000000000002").expect("the PRN is canonical");
        AgentDefinition {
            id,
            name: "triage".to_owned(),
            model_id: "m".to_owned(),
            instructions: "Be brief.".to_owned(),
            tool_names: tool_names.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    fn workspace() -> Arc<Mutex<MemoryWorkspace>> {
        Arc::new(Mutex::new(MemoryWorkspace::new(Arc::new(InMemoryMemoryStore::default()), fresh_scope(), MemoryCaps::default())))
    }

    fn model() -> SharedModel {
        SharedModel::new(ScriptedModel::new(vec![]))
    }

    #[test]
    fn build_agent_maps_the_name_and_the_instructions() {
        let agent = build_agent(&def(&["t"]), model(), workspace());
        assert_eq!(Agent::name(&agent), "triage");
        let ctx = RunContext::<()>::new(Arc::new(()), Arc::new(MemorySession::new()), HookRegistry::new(), TracerHandle::default(), CancellationToken::new());
        assert_eq!(agent.instructions.render(&ctx), "Be brief.");
        assert!(agent.tools.is_empty(), "an unknown tool name maps to no tool");
    }

    #[test]
    fn the_memory_tool_name_adds_the_memory_tool() {
        let agent = build_agent(&def(&["memory"]), model(), workspace());
        let names: Vec<&str> = agent.tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["memory"]);
    }
}
