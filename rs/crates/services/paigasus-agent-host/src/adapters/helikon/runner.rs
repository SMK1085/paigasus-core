// SPDX-License-Identifier: Apache-2.0

//! `HelikonAgentRunner`: the `AgentRunner` port over `TokioRunner` (spec §6.5).

use std::sync::Arc;

use async_trait::async_trait;
use paigasus_agent_host_core::run::{SessionRow, replace_nul};
use paigasus_helikon::core::{AgentInput, ContentPart, HookRegistry, Item, RunConfig, RunContext, Runner, Session, TracerHandle};
use paigasus_helikon::runtime_tokio::TokioRunner;
use sea_orm::DatabaseConnection;

use super::build_agent;
use super::classify::classify;
use super::model_registry::ModelRegistry;
use super::session::{PgRunSession, to_session_row};
use crate::application::worker::ports::{AgentRunner, RunEnd, RunOutput, RunSpec, UserTurn};

/// The error text when the registry does not hold the model of the agent.
pub const MODEL_NOT_REGISTERED: &str = "the model of the agent is not registered";

/// Runs one agent turn through Helikon, with a `PgRunSession` on the thread of the run.
pub struct HelikonAgentRunner {
    db: DatabaseConnection,
    models: Arc<ModelRegistry>,
}

impl HelikonAgentRunner {
    #[must_use]
    pub fn new(db: DatabaseConnection, models: Arc<ModelRegistry>) -> Self {
        Self { db, models }
    }
}

/// The user turn as one user message: the memory text (when present), then the events.
fn user_message(turn: UserTurn) -> Item {
    let mut content = Vec::with_capacity(2);
    if let Some(memory) = turn.memory {
        content.push(ContentPart::Text { text: memory });
    }
    content.push(ContentPart::Text { text: turn.events });
    Item::UserMessage { content }
}

#[async_trait]
impl AgentRunner for HelikonAgentRunner {
    async fn run(&self, spec: RunSpec) -> RunOutput {
        let Some(model) = self.models.model(&spec.agent.model_id) else {
            return RunOutput {
                end: RunEnd::Transient {
                    error: MODEL_NOT_REGISTERED.to_owned(),
                    retry_after: None,
                },
                session: Vec::new(),
            };
        };
        let agent = build_agent(&spec.agent, model, spec.workspace);
        let session = Arc::new(PgRunSession::new(self.db.clone(), spec.thread, spec.fresh_thread));
        let ctx = RunContext::new(Arc::new(()), session.clone() as Arc<dyn Session>, HookRegistry::new(), TracerHandle::default(), spec.cancel).with_deferred_tool_calls(false);
        let mut input = AgentInput::new();
        input.messages = vec![user_message(spec.turn)];
        let mut config = RunConfig::new().with_timeout(spec.limits.timeout);
        config.max_turns = spec.limits.max_turns;

        let end = classify(TokioRunner.run(&agent, ctx, input, config).await);
        match session.take_buffer().iter().map(to_session_row).collect::<Result<Vec<SessionRow>, _>>() {
            Ok(rows) => RunOutput { end, session: rows },
            Err(e) => RunOutput {
                end: RunEnd::Transient {
                    error: replace_nul(&format!("the session of the run cannot be serialized: {e}")),
                    retry_after: None,
                },
                session: Vec::new(),
            },
        }
    }
}
