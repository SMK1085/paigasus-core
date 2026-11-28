// SPDX-License-Identifier: Apache-2.0

//! The model registry (spec §6.2). In production it holds the models of the `[[models]]` list,
//! which `gateway_model::register` adds (SMA-851). A test registers a scripted model. A model
//! can have an output token limit, which `build_agent` sets as the `max_output_tokens` setting.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_helikon::core::{CancellationToken, Model, ModelCapabilities, ModelError, ModelEvent, ModelRequest};

/// A shared model. `LlmAgent` needs a sized model type, so this wrapper holds the trait object
/// and forwards every method of the `Model` trait.
#[derive(Clone)]
pub struct SharedModel(Arc<dyn Model>);

impl SharedModel {
    #[must_use]
    pub fn new(model: impl Model + 'static) -> Self {
        Self(Arc::new(model))
    }
}

#[async_trait]
impl Model for SharedModel {
    async fn invoke(&self, request: ModelRequest, cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
        self.0.invoke(request, cancel).await
    }

    fn capabilities(&self) -> ModelCapabilities {
        self.0.capabilities()
    }

    fn provider(&self) -> &str {
        self.0.provider()
    }

    fn model(&self) -> &str {
        self.0.model()
    }
}

/// An agent names a model that the registry does not hold.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the agent \"{agent}\" names the model \"{model}\", which is not registered")]
pub struct UnknownModel {
    pub agent: String,
    pub model: String,
}

/// The models by name.
#[derive(Clone, Default)]
pub struct ModelRegistry {
    models: HashMap<String, SharedModel>,
    max_output_tokens: HashMap<String, u32>,
}

impl ModelRegistry {
    pub fn register(&mut self, id: impl Into<String>, model: SharedModel) {
        self.models.insert(id.into(), model);
    }

    /// Register a model with the limit of output tokens of one call.
    pub fn register_with_limit(&mut self, id: impl Into<String>, model: SharedModel, max_output_tokens: u32) {
        let id = id.into();
        self.max_output_tokens.insert(id.clone(), max_output_tokens);
        self.models.insert(id, model);
    }

    /// The output token limit of a model. `None` when `register` added it.
    #[must_use]
    pub fn max_output_tokens(&self, id: &str) -> Option<u32> {
        self.max_output_tokens.get(id).copied()
    }

    #[must_use]
    pub fn model(&self, id: &str) -> Option<SharedModel> {
        self.models.get(id).cloned()
    }

    /// Check that every agent names a registered model.
    pub fn check(&self, agents: &[AgentDefinition]) -> Result<(), UnknownModel> {
        match agents.iter().find(|a| !self.models.contains_key(&a.model_id)) {
            Some(a) => Err(UnknownModel {
                agent: a.id.as_prn().canonical(),
                model: a.model_id.clone(),
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use paigasus_agent_host_core::ids::AgentId;

    use super::*;

    /// A model that counts its `invoke` calls.
    struct Counting(Arc<AtomicUsize>);

    #[async_trait]
    impl Model for Counting {
        async fn invoke(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ModelError::Unavailable)
        }

        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities::default()
        }

        fn provider(&self) -> &str {
            "counting"
        }

        fn model(&self) -> &str {
            "counting-1"
        }
    }

    fn def(model: &str) -> AgentDefinition {
        AgentDefinition {
            id: AgentId::new(uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)),
            name: "a".to_owned(),
            model_id: model.to_owned(),
            instructions: String::new(),
            tool_names: vec![],
        }
    }

    #[tokio::test]
    async fn a_shared_model_forwards_invoke() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut registry = ModelRegistry::default();
        registry.register("scripted", SharedModel::new(Counting(count.clone())));
        let model = registry.model("scripted").expect("registered");
        assert!(model.invoke(ModelRequest::default(), CancellationToken::new()).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(model.provider(), "counting");
        assert_eq!(model.model(), "counting-1");
    }

    #[test]
    fn a_limit_is_stored_per_model() {
        let mut registry = ModelRegistry::default();
        registry.register_with_limit("m", SharedModel::new(Counting(Arc::new(AtomicUsize::new(0)))), 512);
        registry.register("n", SharedModel::new(Counting(Arc::new(AtomicUsize::new(0)))));
        assert_eq!(registry.max_output_tokens("m"), Some(512));
        assert_eq!(registry.max_output_tokens("n"), None);
        assert!(registry.model("m").is_some());
    }

    #[test]
    fn the_production_registry_is_empty() {
        assert!(ModelRegistry::default().model("scripted").is_none());
    }

    #[test]
    fn check_names_the_agent_and_the_model() {
        let d = def("gpt");
        let err = ModelRegistry::default().check(std::slice::from_ref(&d)).expect_err("no model");
        assert_eq!(err.agent, d.id.as_prn().canonical());
        assert_eq!(err.model, "gpt");
        let mut registry = ModelRegistry::default();
        registry.register("gpt", SharedModel::new(Counting(Arc::new(AtomicUsize::new(0)))));
        assert_eq!(registry.check(&[d]), Ok(()));
    }
}
