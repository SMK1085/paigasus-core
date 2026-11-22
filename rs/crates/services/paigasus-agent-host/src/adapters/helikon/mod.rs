// SPDX-License-Identifier: Apache-2.0

//! The Helikon adapter: the only place where the agent host meets the agent runtime.

pub mod memory_tool;
pub mod session;
pub mod timer_tools;

use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_helikon::core::{LlmAgent, Model};

/// Build the Helikon agent for a stored agent definition.
///
/// The name and the instructions come from the definition. The tool names are not mapped yet
/// (SMA-790). The caller supplies the model: the agent host does not resolve `model_id` here.
pub fn build_agent<M: Model + 'static>(def: &AgentDefinition, model: M) -> LlmAgent<(), M, String> {
    LlmAgent::builder::<()>().name(def.name.clone()).model(model).instructions(def.instructions.clone()).build()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use paigasus_agent_host_core::agent::AgentDefinition;
    use paigasus_agent_host_core::ids::AgentId;
    use paigasus_helikon::core::{Agent, CancellationToken, HookRegistry, MemorySession, Model, ModelCapabilities, ModelError, ModelEvent, ModelRequest, RunContext, TracerHandle};

    use super::build_agent;

    /// A model that no test calls. `invoke` always fails.
    struct NoModel;

    #[async_trait]
    impl Model for NoModel {
        async fn invoke(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
            Err(ModelError::Unavailable)
        }

        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities::default()
        }
    }

    #[test]
    fn build_agent_maps_the_name_and_the_instructions() {
        let id = AgentId::parse("prn:pgs:agents::00000000-0000-0000-0000-000000000001:agent/00000000-0000-0000-0000-000000000002").expect("the PRN is canonical");
        let def = AgentDefinition {
            id,
            name: "triage".to_owned(),
            model_id: "m".to_owned(),
            instructions: "Be brief.".to_owned(),
            tool_names: vec!["t".to_owned()],
        };
        let agent = build_agent(&def, NoModel);
        assert_eq!(Agent::name(&agent), "triage");
        let ctx = RunContext::<()>::new(Arc::new(()), Arc::new(MemorySession::new()), HookRegistry::new(), TracerHandle::default(), CancellationToken::new());
        assert_eq!(agent.instructions.render(&ctx), "Be brief.");
    }
}
