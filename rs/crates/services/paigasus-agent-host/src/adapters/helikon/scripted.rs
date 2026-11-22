// SPDX-License-Identifier: Apache-2.0

//! A scripted Helikon model. The production registry is empty (spec §6.2) and never holds it.
//! The tests register it, and the `test-model` feature reuses it.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use futures::stream::BoxStream;
use paigasus_helikon::core::{CancellationToken, FinishReason, Model, ModelCapabilities, ModelError, ModelEvent, ModelRequest};
use serde_json::Value;

/// One tool call of a scripted turn: `(call_id, tool, args)`.
pub type ScriptedCall = (String, String, Value);

/// A scripted model. Each `invoke` emits the tool calls of the next turn, then ends the turn with
/// `ToolCalls`. After the last turn it emits the final text `done`.
pub struct ScriptedModel {
    turns: Mutex<VecDeque<Vec<ScriptedCall>>>,
}

impl ScriptedModel {
    /// A model with one turn per entry.
    #[must_use]
    pub fn new(turns: Vec<Vec<ScriptedCall>>) -> Self {
        Self { turns: Mutex::new(turns.into()) }
    }
}

#[async_trait]
impl Model for ScriptedModel {
    async fn invoke(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
        // `pop_front` cannot leave the queue half-changed, so a poisoned lock is still usable.
        let turn = self.turns.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        let events: Vec<ModelEvent> = match turn {
            Some(calls) => {
                let mut events: Vec<ModelEvent> = calls
                    .into_iter()
                    .map(|(call_id, tool, args)| ModelEvent::ToolCallDelta {
                        call_id,
                        name: Some(tool),
                        args_delta: args.to_string(),
                    })
                    .collect();
                events.push(ModelEvent::Finish { reason: FinishReason::ToolCalls });
                events
            }
            None => vec![ModelEvent::TokenDelta { text: "done".to_string() }, ModelEvent::Finish { reason: FinishReason::Stop }],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::default()
    }
}
