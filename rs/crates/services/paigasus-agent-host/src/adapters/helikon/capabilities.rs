// SPDX-License-Identifier: Apache-2.0

//! What this host can run (SMA-852 spec §6.2, §6.7): the models of the registry and the tools
//! that `build_agent` maps.

use std::sync::Arc;

use paigasus_agent_host_core::ports::RunCapabilities;

use super::MEMORY_TOOL_NAME;
use super::model_registry::ModelRegistry;

/// The tool names that an agent can list. `build_agent` maps each one to a tool.
pub const KNOWN_TOOLS: &[&str] = &[MEMORY_TOOL_NAME];

/// The `RunCapabilities` adapter over the model registry and [`KNOWN_TOOLS`].
pub struct HelikonCapabilities {
    models: Arc<ModelRegistry>,
}

impl HelikonCapabilities {
    #[must_use]
    pub fn new(models: Arc<ModelRegistry>) -> Self {
        Self { models }
    }
}

impl RunCapabilities for HelikonCapabilities {
    fn has_model(&self, model: &str) -> bool {
        self.models.model(model).is_some()
    }

    fn has_tool(&self, tool: &str) -> bool {
        KNOWN_TOOLS.contains(&tool)
    }

    /// The ids of the models, sorted.
    fn model_ids(&self) -> Vec<String> {
        self.models.ids()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::helikon::model_registry::SharedModel;
    use crate::adapters::helikon::scripted::ScriptedModel;

    fn capabilities(ids: &[&str]) -> HelikonCapabilities {
        let mut models = ModelRegistry::default();
        for id in ids {
            models.register(*id, SharedModel::new(ScriptedModel::new(vec![])));
        }
        HelikonCapabilities::new(Arc::new(models))
    }

    #[test]
    fn has_model_is_true_only_for_a_registered_id() {
        let caps = capabilities(&["m1"]);
        assert!(caps.has_model("m1"));
        assert!(!caps.has_model("m2"));
        assert!(!caps.has_model(""));
    }

    #[test]
    fn has_tool_knows_memory_only() {
        let caps = capabilities(&[]);
        assert!(caps.has_tool("memory"));
        assert!(!caps.has_tool("shell"));
        assert!(!caps.has_tool("Memory"));
    }

    #[test]
    fn model_ids_are_sorted() {
        let caps = capabilities(&["zeta", "alpha", "mid"]);
        assert_eq!(caps.model_ids(), ["alpha", "mid", "zeta"]);
        assert!(capabilities(&[]).model_ids().is_empty());
    }
}
