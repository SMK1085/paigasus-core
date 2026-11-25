// SPDX-License-Identifier: Apache-2.0
//! The gateway's tool-call interception core (SMA-749, ADR-0027).
//!
//! A synchronous state machine with no I/O and no async runtime (spec D1). The gateway feeds it
//! upstream bytes, decisions and instants, and performs the [`Action`]s it returns.

mod anthropic;
mod body;
mod decider;
pub mod decision_log;
mod interceptor;
mod json;
mod limits;
mod openai;
mod policy;
mod protocol;
mod report;
mod responses;
mod screen;
mod session;
mod sse;
mod turn;
mod types;

pub use body::{BodyCalls, parse_body};
pub use decider::{AllowAll, DecideFuture, DecidedCall, DecisionContext, ToolCallDecider};
pub use interceptor::{Action, DeciderError, Interceptor, Metric};
pub use json::Malformed;
pub use limits::{Limits, OnFailure};
pub use policy::{
    CallOutcome, CallScan, ChoiceRewrite, FieldPlan, HistoryPolicy, MAX_PREDICATE_DEPTH, MalformedCondition, OnUnavailable, PlanItem, PolicyBasis, PolicySet, PolicySnapshot, ReadPlan, ReadSet,
    RefusalCause, RequestPlan, ResultTarget, ScreenRecord, SeenHosts, TenantMode, TenantPart, call_findings, call_schema, decide_calls, decide_calls_with, ingress, invocation, map_conditions,
    name_collisions, plan_request, result_notice, rewrite_request,
};
pub use report::{ReportDecision, ReportEntry, RequestAction, RequestEntry, RequestReason, notice_line, notice_ref, reason_label, report_header, request_comments, sanitize_name};
pub use screen::{RequestScreen, ScreenError, screen_request, screens, seen_hosts};
pub use session::{SessionContext, StoreBudget};
pub use turn::{LatestTurn, MAX_LINK_IDS, PriorCall, ResultShape, ToolChoice, ToolDefinition, ToolResultRef, ToolTurn, TurnFacts, parse_request};
pub use types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall, ToolCallKind};
