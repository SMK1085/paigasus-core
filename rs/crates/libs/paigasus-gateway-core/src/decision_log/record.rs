// SPDX-License-Identifier: Apache-2.0
//! The record types (SMA-755 § 5.1). A record never holds raw arguments (A1).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use paigasus_kernel::CedarUid;
use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect, PolicyRef, SessionInfo};
use uuid::Uuid;

use crate::policy::{PolicyBasis, TenantMode};
use crate::report::ReportEntry;
use crate::types::{BlockCode, ToolCall, ToolCallKind};

/// Which side of the exchange the decision applied to.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Request,
    Response,
}

/// What the gateway did, as the report says.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedEffect {
    Allow,
    Observe,
    Block,
}

/// How the decision was reached.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordBasis {
    Policy,
    ErroredForbid,
    Unavailable,
    OnFailure,
}

impl RecordBasis {
    /// The stable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RecordBasis::Policy => "policy",
            RecordBasis::ErroredForbid => "errored_forbid",
            RecordBasis::Unavailable => "unavailable",
            RecordBasis::OnFailure => "on_failure",
        }
    }
}

impl From<PolicyBasis> for RecordBasis {
    /// The decider's basis is the record basis. `OnFailure` has no decider origin.
    fn from(b: PolicyBasis) -> Self {
        match b {
            PolicyBasis::Policy => RecordBasis::Policy,
            PolicyBasis::ErroredForbid => RecordBasis::ErroredForbid,
            PolicyBasis::Unavailable => RecordBasis::Unavailable,
        }
    }
}

/// Whether the gateway block stopped the call (A5).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prevention {
    NotApplicable,
    Prevented,
    ProviderMayRun,
}

/// What the gateway did to the request. Named apart from `report::RequestAction` (P-6).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedAction {
    RemoveDefinition,
    ReplaceResult,
    ResetToolChoice,
    Refuse,
}

/// The tool of a record. `kind` and `call_id` are empty when the core did not know the call.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolRef {
    /// `sanitize_name` of the display name.
    pub name: String,
    pub namespace: String,
    /// `normalise` of the display name, cut at `MAX_FIELD_BYTES`.
    pub action_id: String,
    /// A cut shortened one of the fields.
    pub truncated: bool,
    pub kind: String,
    pub call_id: String,
}

/// A keyed digest of the arguments. Never a plain hash (A1).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentDigest {
    pub hmac_sha256: [u8; 32],
    pub key_id: String,
}

impl ArgumentDigest {
    #[must_use]
    pub fn new(hmac_sha256: [u8; 32], key_id: impl Into<String>) -> Self {
        Self { hmac_sha256, key_id: key_id.into() }
    }
}

/// The Cedar detail of one decision.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedarDetail {
    /// The kernel verdict, before the interceptor.
    pub effect: Effect,
    pub determining: Vec<PolicyRef>,
    pub errored: Vec<PolicyRef>,
    pub floor_denied: bool,
    /// The final finding set, from the kernel.
    pub findings: BTreeSet<String>,
    pub declared: Declared,
    pub action_group: ActionGroup,
    pub decide_duration: Duration,
}

impl CedarDetail {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        effect: Effect,
        determining: Vec<PolicyRef>,
        errored: Vec<PolicyRef>,
        floor_denied: bool,
        findings: BTreeSet<String>,
        declared: Declared,
        action_group: ActionGroup,
        decide_duration: Duration,
    ) -> Self {
        Self {
            effect,
            determining,
            errored,
            floor_denied,
            findings,
            declared,
            action_group,
            decide_duration,
        }
    }
}

/// What the decider knows about one call, for the record.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionDetail {
    pub basis: RecordBasis,
    pub cedar: Option<CedarDetail>,
    pub arguments: Option<ArgumentDigest>,
}

impl DecisionDetail {
    #[must_use]
    pub fn new(basis: RecordBasis, cedar: Option<CedarDetail>, arguments: Option<ArgumentDigest>) -> Self {
        Self { basis, cedar, arguments }
    }
}

/// The facts of a call that a record needs. It copies no arguments.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFacts {
    pub kind: ToolCallKind,
    pub id: Option<String>,
    pub namespace: Option<String>,
    pub name: String,
}

impl CallFacts {
    #[must_use]
    pub fn of(call: &ToolCall) -> Self {
        Self {
            kind: call.kind,
            id: call.id.clone(),
            namespace: call.namespace.clone(),
            name: call.name.clone(),
        }
    }
}

/// One final report entry with what the report site knows about it. `call` is `None` for a
/// `block_all` entry and for an unenforceable entry (P-2).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedCall {
    pub entry: ReportEntry,
    pub call: Option<CallFacts>,
    pub detail: Option<Arc<DecisionDetail>>,
}

impl ReportedCall {
    #[must_use]
    pub fn new(entry: ReportEntry, call: Option<CallFacts>, detail: Option<Arc<DecisionDetail>>) -> Self {
        Self { entry, call, detail }
    }
}

/// The ids, the time and the sequence of a record. The recorder owns the clock and the entropy.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordHeader {
    pub decision_id: Uuid,
    pub decided_at: SystemTime,
    pub instance_id: Uuid,
    pub sequence: u64,
    pub org: Option<Uuid>,
}

impl RecordHeader {
    #[must_use]
    pub fn new(decision_id: Uuid, decided_at: SystemTime, instance_id: Uuid, sequence: u64, org: Option<Uuid>) -> Self {
        Self {
            decision_id,
            decided_at,
            instance_id,
            sequence,
            org,
        }
    }
}

/// One applied tool decision (§ 5.1).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRecord {
    pub decision_id: Uuid,
    pub decided_at: SystemTime,
    pub instance_id: Uuid,
    pub sequence: u64,
    pub org: Option<Uuid>,
    pub principal_prn: String,
    pub principal: Option<CedarUid>,
    pub api_key_id: Option<String>,
    pub scope_prn: String,
    pub request_id: Option<String>,
    pub session: SessionInfo,
    pub side: Side,
    pub tool: ToolRef,
    pub ingress: String,
    pub model: String,
    pub mode: Option<TenantMode>,
    pub applied: AppliedEffect,
    pub block_code: Option<BlockCode>,
    pub basis: RecordBasis,
    pub prevention: Prevention,
    pub request_action: Option<RecordedAction>,
    pub cedar: Option<CedarDetail>,
    pub arguments: Option<ArgumentDigest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_basis_maps_to_the_record_basis_with_the_same_label() {
        for (b, want) in [
            (PolicyBasis::Policy, RecordBasis::Policy),
            (PolicyBasis::ErroredForbid, RecordBasis::ErroredForbid),
            (PolicyBasis::Unavailable, RecordBasis::Unavailable),
        ] {
            assert_eq!(RecordBasis::from(b), want);
            assert_eq!(RecordBasis::from(b).as_str(), b.as_str());
        }
    }
}
