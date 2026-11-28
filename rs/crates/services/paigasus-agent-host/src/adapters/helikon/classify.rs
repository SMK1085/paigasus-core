// SPDX-License-Identifier: Apache-2.0

//! The failure classes of spec §6.7: the end of a Helikon run as a `RunEnd`.
//!
//! The Helikon enums are `#[non_exhaustive]`, so each match has a wildcard arm. The wildcard is
//! transient: a transient end commits nothing, so an unknown error cannot write memory, and
//! `max_attempts` still ends the retries. Every variant that Helikon core 0.8.4 has is named.
//!
//! A gateway fault (a credential, a missing route, a rate limit, a budget or a failed link) is a
//! host fault: the fault is not in the event, so the attempt goes back (SMA-851 part F).

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

/// A rate limit or budget refusal whose `retry-after` is longer than an in-run retry may wait.
/// The gateway adapter puts it into `ModelError::Other`, so that `RetryingModel` does not sleep
/// for it inside the run (Review Focus 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the gateway refused the call for {retry_after_ms} ms (a rate limit or a budget)")]
pub struct GatewayBackOff {
    pub retry_after_ms: u64,
}

fn host_fault(error: String, retry_after: Option<Duration>) -> RunEnd {
    RunEnd::HostFault { error, retry_after }
}

/// The text of a host fault: the fixed head, then `: {message}` when the error has a message.
fn host_fault_text(head: &str, message: Option<&str>) -> String {
    match message {
        Some(m) => replace_nul(&format!("{head}: {m}")),
        None => replace_nul(head),
    }
}

fn model_error(e: ModelError, text: String) -> RunEnd {
    match e {
        // A long thread closes, so the next claim starts a fresh one (spec §6.7).
        ModelError::ContextLengthExceeded => deterministic(text, true),
        ModelError::Refused { .. } => deterministic(text, false),
        ModelError::Rejected { status, error_type, message } => match status {
            // The gateway refuses the credential of the agent host. A retry cannot help until an
            // operator acts, so the attempt goes back to the event.
            401 | 403 => {
                let head = if error_type.is_empty() {
                    format!("the gateway rejected the agent host credential (status {status})")
                } else {
                    format!("the gateway rejected the agent host credential (status {status}, {error_type})")
                };
                host_fault(host_fault_text(&head, Some(message.as_str()).filter(|m| !m.is_empty())), None)
            }
            404 => {
                let head = if error_type.is_empty() {
                    "the gateway route is off, or the gateway does not know the model (status 404)".to_owned()
                } else {
                    format!("the gateway route is off, or the gateway does not know the model (status 404, {error_type})")
                };
                host_fault(host_fault_text(&head, Some(message.as_str()).filter(|m| !m.is_empty())), None)
            }
            // The request is wrong. The same request fails again.
            400 | 413 => deterministic(text, false),
            _ => transient(text, None),
        },
        ModelError::RateLimited { retry_after_ms } => host_fault(host_fault_text("the gateway rate limit or budget refused the call", None), retry_after_ms.map(Duration::from_millis)),
        ModelError::Unavailable => host_fault(host_fault_text("the gateway link failed", None), None),
        ModelError::Transport(m) => host_fault(host_fault_text("the gateway link failed", Some(m.as_str()).filter(|m| !m.is_empty())), None),
        ModelError::Other(e) => match e.downcast_ref::<GatewayBackOff>() {
            Some(back_off) => host_fault(
                host_fault_text("the gateway rate limit or budget refused the call", None),
                Some(Duration::from_millis(back_off.retry_after_ms)),
            ),
            None => transient(text, None),
        },
        _ => transient(text, None),
    }
}

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::run::RunUsage;
    use paigasus_helikon::core::{AgentError, GuardrailKind, InvalidResume, ModelError, RunError, RunResult, SessionError, StopReason, TokenUsage, ToolError};

    use super::{GatewayBackOff, classify};
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
            RunEnd::HostFault { retry_after: None, .. } => "host_fault".to_owned(),
            RunEnd::HostFault { retry_after: Some(d), .. } => format!("host_fault+{}", d.as_millis()),
            RunEnd::Cancelled => "cancelled".to_owned(),
        }
    }

    fn rejected(status: u16) -> ModelError {
        ModelError::Rejected {
            status,
            error_type: "e".into(),
            message: "m".into(),
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
            (Err(agent_model(ModelError::RateLimited { retry_after_ms: Some(5000) })), "host_fault+5000"),
            (Err(agent_model(ModelError::RateLimited { retry_after_ms: None })), "host_fault"),
            (Err(agent_model(ModelError::Unavailable)), "host_fault"),
            (Err(agent_model(ModelError::Transport("x".into()))), "host_fault"),
            (Err(agent_model(rejected(401))), "host_fault"),
            (Err(agent_model(rejected(403))), "host_fault"),
            (Err(agent_model(rejected(404))), "host_fault"),
            (Err(agent_model(rejected(400))), "deterministic"),
            (Err(agent_model(rejected(413))), "deterministic"),
            (Err(agent_model(rejected(418))), "transient"),
            (Err(agent_model(ModelError::Other(anyhow::Error::new(GatewayBackOff { retry_after_ms: 120_000 })))), "host_fault+120000"),
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
    fn a_credential_rejection_says_so() {
        match classify(Err(agent_model(rejected(401)))) {
            RunEnd::HostFault { error, retry_after: None } => {
                assert!(error.starts_with("the gateway rejected the agent host credential (status 401, e)"), "{error}");
                assert!(error.ends_with(": m"), "{error}");
            }
            other => panic!("expected a host fault, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_message_leaves_no_trailing_separator() {
        let empty = ModelError::Rejected {
            status: 401,
            error_type: String::new(),
            message: String::new(),
        };
        match classify(Err(agent_model(empty))) {
            RunEnd::HostFault { error, .. } => assert_eq!(error, "the gateway rejected the agent host credential (status 401)"),
            other => panic!("expected a host fault, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_route_and_a_failed_link_say_so() {
        match classify(Err(agent_model(rejected(404)))) {
            RunEnd::HostFault { error, .. } => assert!(error.starts_with("the gateway route is off, or the gateway does not know the model (status 404, e)"), "{error}"),
            other => panic!("expected a host fault, got {other:?}"),
        }
        match classify(Err(agent_model(ModelError::Transport("refused".into())))) {
            RunEnd::HostFault { error, .. } => assert_eq!(error, "the gateway link failed: refused"),
            other => panic!("expected a host fault, got {other:?}"),
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
