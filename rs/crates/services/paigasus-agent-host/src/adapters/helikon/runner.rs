// SPDX-License-Identifier: Apache-2.0

//! `HelikonAgentRunner`: the `AgentRunner` port over `TokioRunner` (spec §6.5).

use std::sync::Arc;

use async_trait::async_trait;
use paigasus_agent_host_core::run::{SessionRow, replace_nul};
use paigasus_helikon::core::{AgentInput, ContentPart, HookRegistry, Item, RunConfig, RunContext, Runner, Session, SessionEvent, TracerHandle};
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
        let agent = build_agent(&spec.agent, model, self.models.max_output_tokens(&spec.agent.model_id), spec.workspace);
        let session = Arc::new(PgRunSession::new(self.db.clone(), spec.thread, spec.fresh_thread));
        let ctx = RunContext::new(Arc::new(()), session.clone() as Arc<dyn Session>, HookRegistry::new(), TracerHandle::default(), spec.cancel).with_deferred_tool_calls(false);
        let mut input = AgentInput::new();
        input.messages = vec![user_message(spec.turn)];
        let mut config = RunConfig::new().with_timeout(spec.limits.timeout);
        config.max_turns = spec.limits.max_turns;

        let end = classify(TokioRunner.run(&agent, ctx, input, config).await);
        finish(end, &session.take_buffer(), to_session_row)
    }
}

/// The output of a run. Only a full commit (Completed, Stopped) writes session rows (spec §6.7,
/// §6.8), so only those ends carry rows, and only for them a serialize error makes the end
/// transient. Every other end carries no rows, and its class does not change. `to_row` is a
/// parameter so that a test can make a serialize error: a real `SessionEvent` does not fail.
fn finish(end: RunEnd, events: &[SessionEvent], to_row: impl Fn(&SessionEvent) -> Result<SessionRow, serde_json::Error>) -> RunOutput {
    if !matches!(end, RunEnd::Completed { .. } | RunEnd::Stopped { .. }) {
        return RunOutput { end, session: Vec::new() };
    }
    match events.iter().map(to_row).collect::<Result<Vec<SessionRow>, _>>() {
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

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::run::{RunUsage, SessionRow};
    use paigasus_helikon::core::{ContentPart, SessionEvent};

    use super::finish;
    use crate::adapters::helikon::session::to_session_row;
    use crate::application::worker::ports::RunEnd;

    fn event() -> SessionEvent {
        SessionEvent::UserMessage {
            content: vec![ContentPart::Text { text: "e".into() }],
            ts: jiff::Timestamp::from_second(1_700_000_000).expect("ts"),
        }
    }

    /// A row conversion that always fails, as a serialize error would.
    fn failing(_: &SessionEvent) -> Result<SessionRow, serde_json::Error> {
        Err(serde_json::from_str::<u8>("x").expect_err("not a number"))
    }

    fn completed() -> RunEnd {
        RunEnd::Completed {
            output: "done".into(),
            usage: RunUsage::default(),
        }
    }

    #[test]
    fn a_deterministic_end_keeps_its_class_and_has_no_rows() {
        let end = RunEnd::Deterministic {
            error: "max turns (1) exceeded".into(),
            close_thread: false,
        };
        let out = finish(end.clone(), &[event()], failing);
        assert_eq!(out.end, end);
        assert!(out.session.is_empty());
    }

    #[test]
    fn a_transient_or_cancelled_end_has_no_rows() {
        for end in [RunEnd::Transient { error: "x".into(), retry_after: None }, RunEnd::Cancelled] {
            let out = finish(end.clone(), &[event()], to_session_row);
            assert_eq!(out.end, end);
            assert!(out.session.is_empty());
        }
    }

    #[test]
    fn a_completed_end_carries_its_rows() {
        let out = finish(completed(), &[event()], to_session_row);
        assert_eq!(out.end, completed());
        assert_eq!(out.session.len(), 1);
        assert_eq!(out.session[0].kind, "user_message");
    }

    #[test]
    fn a_serialize_error_makes_a_completed_end_transient() {
        let out = finish(completed(), &[event()], failing);
        match out.end {
            RunEnd::Transient { error, retry_after: None } => assert!(error.starts_with("the session of the run cannot be serialized"), "{error}"),
            other => panic!("expected a transient end, got {other:?}"),
        }
        assert!(out.session.is_empty());
    }
}
