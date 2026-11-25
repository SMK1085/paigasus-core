// SPDX-License-Identifier: Apache-2.0
//! The tool-policy side of the gateway core (SMA-752): the mapping from gateway-core request and
//! call types into the kernel's tool-policy types, the per-request policy snapshot, and the call
//! decisions. No I/O.

mod calls;
mod history;
mod mapping;
mod plan;
mod raw;
mod rewrite;
mod snapshot;

pub use calls::{CallOutcome, CallScan, PolicyBasis, decide_calls, decide_calls_with};
pub use history::{FieldPlan, HistoryPolicy, MAX_PREDICATE_DEPTH, MalformedCondition, ReadPlan, ReadSet, map_conditions};

#[cfg(test)]
pub(crate) use mapping::test_plan;
pub use mapping::{call_fields, call_findings, call_schema, ingress, invocation, name_collisions};
pub use plan::{ChoiceRewrite, PlanItem, RefusalCause, RequestPlan, ResultTarget, ScreenRecord, plan_request};
pub use rewrite::{result_notice, rewrite_request};
pub use snapshot::{OnUnavailable, PolicySet, PolicySnapshot, SeenHosts, TenantMode, TenantPart};

/// The floor that ships with the file source (D15). This is the one copy in this crate
/// (preflight F17). The gateway's `tool-policies/floor.cedar` holds the same policy.
#[cfg(test)]
pub(crate) const SHIPPED_FLOOR: &str = r#"@id("floor-name-collision") forbid(principal, action, resource) when { context.findings.contains("name_collision") };"#;
