// SPDX-License-Identifier: Apache-2.0
//! The gateway's tool-call interception core (SMA-749, ADR-0027).
//!
//! A synchronous state machine with no I/O and no async runtime (spec D1). The gateway feeds it
//! upstream bytes, decisions and instants, and performs the [`Action`]s it returns.

mod anthropic;
mod body;
mod decider;
mod interceptor;
mod json;
mod limits;
mod openai;
mod policy;
mod protocol;
mod report;
mod responses;
mod sse;
mod turn;
mod types;

pub use body::{BodyCalls, parse_body};
pub use decider::{AllowAll, DecideFuture, DecisionContext, ToolCallDecider};
pub use interceptor::{Action, DeciderError, Interceptor, Metric};
pub use json::Malformed;
pub use limits::{Limits, OnFailure};
pub use policy::{
    CallOutcome, ChoiceRewrite, OnUnavailable, PlanItem, PolicyBasis, PolicySet, PolicySnapshot, RequestPlan, ResultTarget, TenantMode, TenantPart, call_findings, decide_calls, ingress, invocation,
    name_collisions, plan_request, result_notice, rewrite_request,
};
pub use report::{ReportDecision, ReportEntry, RequestAction, RequestEntry, RequestReason, notice_line, reason_label, report_header, request_comments, sanitize_name};
pub use turn::{PriorCall, ResultShape, ToolChoice, ToolDefinition, ToolResultRef, ToolTurn, TurnFacts, parse_request};
pub use types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall, ToolCallKind};
