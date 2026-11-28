// SPDX-License-Identifier: Apache-2.0

//! The model registry (spec §6.2). In production it is empty: the real provider is SMA-851.
//! A test registers a scripted model.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
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

/// The models by name.
#[derive(Clone, Default)]
pub struct ModelRegistry {
    models: HashMap<String, SharedModel>,
}

impl ModelRegistry {
    pub fn register(&mut self, id: impl Into<String>, model: SharedModel) {
        self.models.insert(id.into(), model);
    }

    #[must_use]
    pub fn model(&self, id: &str) -> Option<SharedModel> {
        self.models.get(id).cloned()
    }

    /// The ids of the registered models, sorted.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.models.keys().cloned().collect();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn the_production_registry_is_empty() {
        assert!(ModelRegistry::default().model("scripted").is_none());
    }
}
