// SPDX-License-Identifier: Apache-2.0

//! The failure classes of spec §6.7: the end of a Helikon run as a `RunEnd`.
//!
//! The Helikon enums are `#[non_exhaustive]`, so each match has a wildcard arm. The wildcard is
//! transient: a transient end commits nothing, so an unknown error cannot write memory, and
//! `max_attempts` still ends the retries. Every variant that Helikon core 0.8.4 has is named.

use std::time::Duration;

use paigasus_agent_host_core::run::{RunUsage, replace_nul};
use paigasus_helikon::core::{AgentError, ModelError, RunError, RunResult, StopReason, TokenUsage};

use crate::application::worker::ports::RunEnd;

/// Map the result of `TokioRunner::run` to its class.
pub fn classify(result: Result<RunResult, RunError>) -> RunEnd {
    match result {
        Ok(r) => outcome(r),
        Err(e) => run_error(e),
    }
}

/// The core usage of a run. `total_tokens` has no column.
fn usage(u: &TokenUsage) -> RunUsage {
    RunUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cached_input_tokens: u.cached_input_tokens,
        reasoning_tokens: u.reasoning_tokens,
    }
}

fn deterministic(error: String, close_thread: bool) -> RunEnd {
    RunEnd::Deterministic { error, close_thread }
}

fn transient(error: String, retry_after: Option<Duration>) -> RunEnd {
    RunEnd::Transient { error, retry_after }
}

fn outcome(r: RunResult) -> RunEnd {
    let usage = usage(&r.usage);
    match r.stop_reason {
        StopReason::Completed => RunEnd::Completed { output: r.final_output, usage },
        StopReason::Stopped { reason } => RunEnd::Stopped {
            output: r.final_output,
            reason: replace_nul(&reason),
            usage,
        },
        // Not reachable: the run context turns deferral off, so a `Defer` is a deny.
        StopReason::Deferred { .. } => deterministic("the run stopped on a deferred tool call, but deferral is off".to_owned(), false),
        other => transient(replace_nul(&format!("the run stopped for an unknown reason: {other:?}")), None),
    }
}

fn run_error(e: RunError) -> RunEnd {
    let text = replace_nul(&e.to_string());
    match e {
        RunError::Agent(a) => agent_error(a, text),
        RunError::MaxIterations | RunError::Timeout | RunError::InvalidResume(_) => deterministic(text, false),
        // The worker decides between Transient, Discarded and Shutdown.
        RunError::Cancelled => RunEnd::Cancelled,
        // A failed session read reaches here as `Other` (runtime-tokio `load_and_record`).
        RunError::SessionWrite(_) | RunError::Other(_) => transient(text, None),
        _ => transient(text, None),
    }
}

fn agent_error(e: AgentError, text: String) -> RunEnd {
    match e {
        AgentError::Model(m) => model_error(m, text),
        AgentError::MaxTurnsExceeded(_)
        | AgentError::MaxIterationsExceeded { .. }
        | AgentError::Guardrail { .. }
        | AgentError::HookDenied { .. }
        | AgentError::InvalidStructuredOutput { .. }
        | AgentError::MaxAgentDepthExceeded { .. }
        | AgentError::MaxHandoffsExceeded { .. }
        | AgentError::Tool(_)
        | AgentError::InvalidInjection { .. }
        | AgentError::NotImplemented { .. }
        | AgentError::Incomplete { .. } => deterministic(text, false),
        AgentError::Session(_) | AgentError::Other(_) => transient(text, None),
        _ => transient(text, None),
    }
}

fn model_error(e: ModelError, text: String) -> RunEnd {
    match e {
        // A long thread closes, so the next claim starts a fresh one (spec §6.7).
        ModelError::ContextLengthExceeded => deterministic(text, true),
        ModelError::Refused { .. } => deterministic(text, false),
        ModelError::RateLimited { retry_after_ms } => transient(text, retry_after_ms.map(Duration::from_millis)),
        ModelError::Unavailable | ModelError::Transport(_) => transient(text, None),
        ModelError::Other(_) => transient(text, None),
        // `ModelError::Rejected` (new in core 0.8) reaches the wildcard until SMA-851 Task 4.
        _ => transient(text, None),
    }
}

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::run::RunUsage;
    use paigasus_helikon::core::{AgentError, GuardrailKind, InvalidResume, ModelError, RunError, RunResult, SessionError, StopReason, TokenUsage, ToolError};

    use super::classify;
    use crate::application::worker::ports::RunEnd;

    fn ok(stop_reason: StopReason) -> Result<RunResult, RunError> {
        let mut r = RunResult::default();
        r.stop_reason = stop_reason;
        r.final_output = "out".to_owned();
        Ok(r)
    }

    fn agent(e: AgentError) -> RunError {
        RunError::Agent(e)
    }

    fn agent_model(e: ModelError) -> RunError {
        RunError::Agent(AgentError::Model(e))
    }

    /// The class, `close_thread` and `retry_after` of an end.
    fn label(end: &RunEnd) -> String {
        match end {
            RunEnd::Completed { .. } => "completed".to_owned(),
            RunEnd::Stopped { .. } => "stopped".to_owned(),
            RunEnd::Deterministic { close_thread: false, .. } => "deterministic".to_owned(),
            RunEnd::Deterministic { close_thread: true, .. } => "deterministic+close".to_owned(),
            RunEnd::Transient { retry_after: None, .. } => "transient".to_owned(),
            RunEnd::Transient { retry_after: Some(d), .. } => format!("transient+{}", d.as_millis()),
            RunEnd::Cancelled => "cancelled".to_owned(),
        }
    }

    #[test]
    fn every_named_variant_has_its_class() {
        let cases: Vec<(Result<RunResult, RunError>, &str)> = vec![
            (ok(StopReason::Completed), "completed"),
            (ok(StopReason::Stopped { reason: "r".into() }), "stopped"),
            (ok(StopReason::Deferred { calls: vec![] }), "deterministic"),
            (Err(RunError::MaxIterations), "deterministic"),
            (Err(RunError::Timeout), "deterministic"),
            (Err(RunError::InvalidResume(InvalidResume::NoOpenDeferrals)), "deterministic"),
            (Err(agent(AgentError::MaxTurnsExceeded(16))), "deterministic"),
            (Err(agent(AgentError::MaxIterationsExceeded { max: 3 })), "deterministic"),
            (Err(agent(AgentError::Guardrail { kind: GuardrailKind::InputPolicy })), "deterministic"),
            (
                Err(agent(AgentError::HookDenied {
                    event: "OnRunStart".into(),
                    reason: "no".into(),
                })),
                "deterministic",
            ),
            (
                Err(agent(AgentError::InvalidStructuredOutput {
                    schema_errors: vec!["e".into()],
                    final_text: "t".into(),
                })),
                "deterministic",
            ),
            (Err(agent(AgentError::MaxAgentDepthExceeded { depth: 9, max: 8 })), "deterministic"),
            (Err(agent(AgentError::MaxHandoffsExceeded { limit: 4 })), "deterministic"),
            (Err(agent(AgentError::Tool(ToolError::Denied { reason: "no".into() }))), "deterministic"),
            (Err(agent(AgentError::InvalidInjection { reason: "r".into() })), "deterministic"),
            (Err(agent(AgentError::NotImplemented { feature: "f" })), "deterministic"),
            (Err(agent(AgentError::Incomplete { stop_reason: StopReason::Completed })), "deterministic"),
            (Err(agent_model(ModelError::Refused { reason: "policy".into() })), "deterministic"),
            (Err(agent_model(ModelError::ContextLengthExceeded)), "deterministic+close"),
            (Err(agent_model(ModelError::RateLimited { retry_after_ms: Some(5000) })), "transient+5000"),
            (Err(agent_model(ModelError::RateLimited { retry_after_ms: None })), "transient"),
            (Err(agent_model(ModelError::Unavailable)), "transient"),
            (Err(agent_model(ModelError::Transport("x".into()))), "transient"),
            (Err(agent_model(ModelError::Other(anyhow::anyhow!("x")))), "transient"),
            (Err(agent(AgentError::Session(SessionError::Unavailable))), "transient"),
            (Err(agent(AgentError::Other(anyhow::anyhow!("x")))), "transient"),
            (Err(RunError::SessionWrite(SessionError::Unavailable)), "transient"),
            (Err(RunError::Other(anyhow::anyhow!("x"))), "transient"),
            (Err(RunError::Cancelled), "cancelled"),
        ];
        for (input, want) in cases {
            let shown = format!("{input:?}");
            assert_eq!(label(&classify(input)), want, "input: {shown}");
        }
    }

    #[test]
    fn the_usage_is_copied() {
        let mut r = RunResult::default();
        let mut usage = TokenUsage::default();
        usage.input_tokens = 7;
        usage.output_tokens = 3;
        usage.cached_input_tokens = 1;
        usage.reasoning_tokens = 2;
        usage.total_tokens = 10;
        r.usage = usage;
        r.final_output = "done".to_owned();
        let want = RunUsage {
            input_tokens: 7,
            output_tokens: 3,
            cached_input_tokens: 1,
            reasoning_tokens: 2,
        };
        assert_eq!(
            classify(Ok(r)),
            RunEnd::Completed {
                output: "done".to_owned(),
                usage: want
            }
        );
    }

    #[test]
    fn nul_in_an_error_text_is_replaced() {
        let end = classify(Err(agent_model(ModelError::Refused { reason: "a\u{0}".into() })));
        match end {
            RunEnd::Deterministic { error, close_thread: false } => {
                assert!(error.contains('\u{FFFD}'), "{error:?}");
                assert!(!error.contains('\u{0}'), "{error:?}");
            }
            other => panic!("expected a deterministic end, got {other:?}"),
        }
    }
}
