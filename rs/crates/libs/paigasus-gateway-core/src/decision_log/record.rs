// SPDX-License-Identifier: Apache-2.0
//! The record types (SMA-755 § 5.1). A record never holds raw arguments (A1).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use paigasus_kernel::CedarUid;
use paigasus_kernel::detect::{Finding, Severity};
use paigasus_kernel::tool_policy::history::ExtractedFields;
use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect, FORGED_HISTORY, HISTORY_TRUNCATED, MIXED_SESSIONS, NAME_COLLISION, PolicyRef, SessionInfo, UNPARSED_ARGUMENTS, Verdict};
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
    /// A tool result with findings that the gateway did not replace (SMA-855 § 6.11.2).
    ScreenResult,
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

/// One finding of a decision, for the record: the kind, the severity and the detector. It holds
/// no evidence, now or later (SMA-855 D3), so the derived `Debug` prints no content.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRecord {
    pub kind: String,
    /// `None` is an unknown severity: an event that a gateway older than SMA-855 wrote.
    pub severity: Option<Severity>,
    /// The detector name. Empty for an event older than SMA-855.
    pub detector: String,
    /// The version of the detector rules. Empty when the detector has no version.
    pub version: String,
}

impl FindingRecord {
    #[must_use]
    pub fn new(kind: impl Into<String>, severity: Option<Severity>, detector: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            severity,
            detector: detector.into(),
            version: version.into(),
        }
    }

    /// The record of a finding. The evidence of the finding stays behind (D3).
    #[must_use]
    pub fn from_finding(f: &Finding) -> Self {
        Self::new(f.kind.clone(), Some(f.severity), f.detector.clone(), f.version.clone())
    }
}

fn sort_records(records: &mut [FindingRecord]) {
    records.sort_by(|a, b| (a.kind.as_str(), a.detector.as_str()).cmp(&(b.kind.as_str(), b.detector.as_str())));
}

/// The finding records of one decision: the findings that the gateway gave (`given`) and the
/// findings that the kernel added (`v.added`), sorted by kind and then by detector.
#[must_use]
pub fn records_of(given: &[Finding], v: &Verdict) -> Vec<FindingRecord> {
    let mut records: Vec<FindingRecord> = given.iter().chain(&v.added).map(FindingRecord::from_finding).collect();
    sort_records(&mut records);
    records
}

/// The fixed severity of a kind, for a finding of unknown severity (SMA-855 § 6.11.2, P-8). Only
/// `name_collision`, `unparsed_arguments` and the three history kinds of SMA-753 existed before
/// SMA-855. Each has the severity that the gateway gives it now. Any other kind is `High`.
pub(crate) fn inferred_severity(kind: &str) -> Severity {
    match kind {
        NAME_COLLISION => Severity::High,
        UNPARSED_ARGUMENTS => Severity::Info,
        FORGED_HISTORY => Severity::High,
        MIXED_SESSIONS => Severity::Medium,
        HISTORY_TRUNCATED => Severity::Low,
        _ => Severity::High,
    }
}

/// The `max_severity` of a decoded record, from its findings (§ 6.11.2): `None` with no
/// findings; the highest severity when every severity is known; else the highest over the known
/// severities and the [`inferred_severity`] of each unknown one. The flag is `true` exactly when
/// a severity was inferred. The proto decode and the trace encoder both use this one rule.
#[must_use]
pub fn derive_max_severity(findings: &[FindingRecord]) -> (Option<Severity>, bool) {
    let inferred = findings.iter().any(|f| f.severity.is_none());
    let max = findings.iter().map(|f| f.severity.unwrap_or_else(|| inferred_severity(&f.kind))).max();
    (max, inferred)
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
    /// The findings of the decision, sorted by kind and then by detector: the gateway findings
    /// and the kernel-added ones (`Verdict.added`).
    pub findings: Vec<FindingRecord>,
    /// The value that the kernel wrote into `context.max_severity` (`None` is -1).
    ///
    /// With the detectors off (`Facts.detection` false), the kernel writes -1 although the
    /// findings can hold severities (for example `unparsed_arguments`). The record keeps both as
    /// the verdict gives them, so this field is then `None` while `findings` is not empty, and
    /// `detection_disabled` is `true`. The proto has no field for this value: a decode derives it
    /// from the findings ([`derive_max_severity`]), except when `detection_disabled` is set.
    pub max_severity: Option<Severity>,
    /// `max_severity` was derived on decode from a finding of unknown severity (an event older
    /// than SMA-855). Always `false` on the write path.
    pub severity_inferred: bool,
    /// The detectors were off for the decision (`Facts.detection` false), so the kernel wrote
    /// `max_severity` -1 whatever the findings hold. `decide_calls` sets it from the facts, so the
    /// record states the fact and does not guess it from the findings. A decode of a flagged
    /// event keeps `max_severity` `None`. `new` sets it to `false`.
    pub detection_disabled: bool,
    pub declared: Declared,
    pub action_group: ActionGroup,
    pub decide_duration: Duration,
}

impl CedarDetail {
    /// The detail of a decision. The findings are sorted by kind and then by detector.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        effect: Effect,
        determining: Vec<PolicyRef>,
        errored: Vec<PolicyRef>,
        floor_denied: bool,
        mut findings: Vec<FindingRecord>,
        max_severity: Option<Severity>,
        declared: Declared,
        action_group: ActionGroup,
        decide_duration: Duration,
    ) -> Self {
        sort_records(&mut findings);
        Self {
            effect,
            determining,
            errored,
            floor_denied,
            findings,
            max_severity,
            severity_inferred: false,
            detection_disabled: false,
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
    /// The item `id` of a Responses `local_shell_call`. `None` for every other call.
    pub item_id: Option<String>,
}

impl CallFacts {
    #[must_use]
    pub fn of(call: &ToolCall) -> Self {
        Self {
            kind: call.kind,
            id: call.id.clone(),
            namespace: call.namespace.clone(),
            name: call.name.clone(),
            item_id: call.item_id(),
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
    /// The id that the decider gave the decision of this call. `None` when the decision vector
    /// is not aligned with the calls, or when the decider gave no id.
    pub decision_id: Option<Uuid>,
    /// The values at the planned paths of this call (SMA-753 P3). `None` when no field plan
    /// applies. The report site digests them for the history append. A decision record never
    /// holds them (ADR-0027 (i)), and `Debug` prints no value.
    pub fields: Option<ExtractedFields>,
}

impl ReportedCall {
    #[must_use]
    pub fn new(entry: ReportEntry, call: Option<CallFacts>, detail: Option<Arc<DecisionDetail>>) -> Self {
        Self {
            entry,
            call,
            detail,
            decision_id: None,
            fields: None,
        }
    }

    /// Set the stored fields of the call.
    #[must_use]
    pub fn with_fields(mut self, fields: Option<ExtractedFields>) -> Self {
        self.fields = fields;
        self
    }

    /// Set the decision id of the call.
    #[must_use]
    pub fn with_decision_id(mut self, id: Option<Uuid>) -> Self {
        self.decision_id = id;
        self
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
    use paigasus_kernel::detect::{Finding, Severity};
    use paigasus_kernel::tool_policy::Verdict;

    use super::*;

    fn verdict_with_added(added: Vec<Finding>) -> Verdict {
        let mut v = paigasus_kernel::tool_policy::decide(
            &paigasus_kernel::tool_policy::ToolInvocation::new("t", paigasus_kernel::tool_policy::ToolInput::Text("x".into()), Declared::Yes),
            &paigasus_kernel::tool_policy::Facts::new(
                CedarUid {
                    entity_type: "Pgs::Iam::Principal".into(),
                    entity_id: "p".into(),
                },
                Uuid::nil(),
                paigasus_kernel::tool_policy::RequestInfo::new("s", "chat_completions", "m"),
                SessionInfo::placeholder(),
            ),
            &paigasus_kernel::tool_policy::Findings::new(),
            &paigasus_kernel::tool_policy::ToolPolicies::compile(Arc::new(paigasus_kernel::tool_policy::ToolPolicies::compile_floor("").unwrap()), "", &[]).unwrap(),
        );
        v.added = added;
        v
    }

    #[test]
    fn records_are_sorted_and_hold_the_kernel_findings() {
        let given = [
            Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1"),
            Finding::new("secret_in_args", "secret_in_args", Severity::High, "v1"),
        ];
        let v = verdict_with_added(vec![Finding::new("kernel", "unparsed_arguments", Severity::Info, "")]);
        let r = records_of(&given, &v);
        let got: Vec<_> = r.iter().map(|f| (f.kind.as_str(), f.severity, f.detector.as_str(), f.version.as_str())).collect();
        assert_eq!(
            got,
            [
                ("secret_in_args", Some(Severity::High), "secret_in_args", "v1"),
                ("unparsed_arguments", Some(Severity::Info), "kernel", ""),
                ("unseen_host", Some(Severity::Medium), "unseen_host", "v1"),
            ]
        );
    }

    #[test]
    fn records_of_one_kind_are_sorted_by_detector() {
        let given = [Finding::new("judge", "other", Severity::Low, ""), Finding::new("built_in", "other", Severity::High, "")];
        let r = records_of(&given, &verdict_with_added(Vec::new()));
        assert_eq!(r.iter().map(|f| f.detector.as_str()).collect::<Vec<_>>(), ["built_in", "judge"]);
    }

    #[test]
    fn a_new_detail_sorts_its_findings() {
        let findings = vec![FindingRecord::new("b", Some(Severity::Low), "kernel", ""), FindingRecord::new("a", None, "kernel", "")];
        let c = CedarDetail::new(Effect::Allow, vec![], vec![], false, findings, Some(Severity::Low), Declared::Yes, ActionGroup::Catalog, Duration::ZERO);
        assert_eq!(c.findings.iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!((c.max_severity, c.severity_inferred), (Some(Severity::Low), false));
    }

    #[test]
    fn inferred_severities() {
        assert_eq!(
            [inferred_severity("name_collision"), inferred_severity("unparsed_arguments"), inferred_severity("zzz")],
            [Severity::High, Severity::Info, Severity::High]
        );
        // The three history kinds of SMA-753 (a gateway before SMA-855 wrote them with no severity).
        assert_eq!(
            [inferred_severity("forged_history"), inferred_severity("mixed_sessions"), inferred_severity("history_truncated")],
            [Severity::High, Severity::Medium, Severity::Low]
        );
    }

    #[test]
    fn the_derived_max_severity_follows_the_read_path_rule() {
        let known = |k: &str, s| FindingRecord::new(k, Some(s), "kernel", "");
        let unknown = |k: &str| FindingRecord::new(k, None, "", "");
        assert_eq!(derive_max_severity(&[]), (None, false));
        assert_eq!(derive_max_severity(&[known("a", Severity::Low), known("b", Severity::Medium)]), (Some(Severity::Medium), false));
        assert_eq!(derive_max_severity(&[unknown("unparsed_arguments")]), (Some(Severity::Info), true));
        assert_eq!(derive_max_severity(&[unknown("unparsed_arguments"), known("b", Severity::Low)]), (Some(Severity::Low), true));
        assert_eq!(derive_max_severity(&[unknown("name_collision"), unknown("unparsed_arguments")]), (Some(Severity::High), true));
        assert_eq!(derive_max_severity(&[unknown("zzz")]), (Some(Severity::High), true));
    }

    #[test]
    fn a_finding_record_debug_has_no_evidence() {
        let f = Finding::new("secret_in_args", "secret_in_args", Severity::High, "v1").with_evidence(paigasus_kernel::detect::Evidence::host("r", Some("/CANARY"), "CANARY-host.example".into()));
        let d = format!("{:?}", FindingRecord::from_finding(&f));
        assert!(!d.contains("CANARY"), "{d}");
    }

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
