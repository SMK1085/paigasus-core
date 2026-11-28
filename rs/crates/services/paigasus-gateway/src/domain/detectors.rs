// SPDX-License-Identifier: Apache-2.0

//! The detector port (SMA-855 § 6.6.1). A detector screens a batch of subjects (tool calls or tool
//! results) and returns one vector of findings per subject. The built-in stage is a synchronous
//! [`BuiltInStage`], because it also returns the hosts of each call (plan P-1). Webhooks and the
//! judge are async [`Detector`]s.
//!
//! Every type here that holds a subject, a host or a finding has a hand-written `Debug` that prints
//! counts and fixed names only (§ 6.2.4, A2).

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use paigasus_kernel::detect::{Finding, HostSet, Severity};
use paigasus_kernel::tool_policy::ToolInvocation;
use serde_json::Value;
use uuid::Uuid;

/// The rule of the `detector_error` finding for a subject that the scan budget does not cover
/// (§ 6.10): a call on the response side, a result on the request side.
pub const SCAN_LIMIT: &str = "scan_limit";

/// The rule of the `detector_error` finding for a tool result that holds content the request screen
/// cannot read as text (an object `content`, a `document` block).
pub const UNSCANNED_SHAPE: &str = "unscanned_shape";

/// The orgs that a webhook provider serves (SMA-856 D7): every org (a shared provider) or a fixed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgScope {
    /// Every org.
    All,
    /// Only these orgs.
    Only(BTreeSet<Uuid>),
}

impl OrgScope {
    /// Whether the scope holds the org.
    #[must_use]
    pub fn permits(&self, org: Uuid) -> bool {
        match self {
            Self::All => true,
            Self::Only(set) => set.contains(&org),
        }
    }
}

/// Who sent the request: the principal, the session and the webhook opt-in list of the org
/// (SMA-856). `Debug` prints presence and a count only (A4).
#[derive(Clone)]
pub struct Caller {
    /// The principal (a PRN). Empty when it is unknown.
    pub principal: Arc<str>,
    /// The session of the request, when there is one.
    pub session: Option<Uuid>,
    /// The names of the webhook providers that the org opted in to.
    pub opt_in: Arc<[String]>,
}

impl Caller {
    /// A caller with no principal, no session and no opt-in.
    #[must_use]
    pub fn none() -> Self {
        Self {
            principal: Arc::from(""),
            session: None,
            opt_in: Arc::from(Vec::<String>::new()),
        }
    }
}

impl fmt::Debug for Caller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Caller {{ principal: {}, session: {}, opt_in: {} }}",
            !self.principal.is_empty(),
            self.session.is_some(),
            self.opt_in.len()
        )
    }
}

/// The side of the relay that screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Side {
    /// The request: tool results that the caller sends.
    Request,
    /// The response: tool calls of the model.
    Response,
}

/// One thing to screen. It borrows nothing: the pipeline runs inside a `'static` decider future.
#[derive(Clone)]
#[non_exhaustive]
pub enum Subject {
    /// A tool call of the model (response side).
    Call {
        /// The position of the call.
        index: usize,
        /// The call.
        invocation: ToolInvocation,
        /// The JSON Schema of the matching tool definition.
        schema: Option<Arc<Value>>,
    },
    /// A tool result in the request (request side). `text` lives only for the request side.
    Result {
        /// The position of the result.
        index: usize,
        /// The name of the tool that gave the result, when it is known.
        tool: Option<String>,
        /// The result text.
        text: Arc<str>,
    },
}

impl Subject {
    /// Whether the subject is a tool call.
    #[must_use]
    pub fn is_call(&self) -> bool {
        matches!(self, Self::Call { .. })
    }
}

impl fmt::Debug for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call { index, .. } => write!(f, "Subject::Call {{ index: {index}, .. }}"),
            Self::Result { index, .. } => write!(f, "Subject::Result {{ index: {index}, .. }}"),
        }
    }
}

/// The input of one detector run.
#[non_exhaustive]
pub struct DetectBatch {
    /// The side that screens.
    pub side: Side,
    /// The organization of the request.
    pub org: Option<Uuid>,
    /// The request id.
    pub request_id: Option<String>,
    /// The subjects, in order.
    pub subjects: Vec<Subject>,
    /// The hosts of the seen message sources. `None`: no request screen ran.
    pub seen_hosts: Option<Arc<HostSet>>,
    /// The findings of the earlier stages, per subject, with evidence. Empty for the built-ins.
    pub earlier: Vec<Vec<Finding>>,
    /// Who sent the request (SMA-856).
    pub caller: Caller,
    /// The time at which the pipeline stops the late stages. A detector must return before it.
    /// `new` sets the time of its call; `run` sets the cut.
    pub deadline: tokio::time::Instant,
    /// The request body of each call subject for the webhooks, built once per run. `None` until
    /// the pipeline builds it.
    pub webhook_bodies: Option<Arc<[Bytes]>>,
}

impl DetectBatch {
    /// A batch with no earlier findings.
    #[must_use]
    pub fn new(side: Side, org: Option<Uuid>, request_id: Option<String>, subjects: Vec<Subject>, seen_hosts: Option<Arc<HostSet>>) -> Self {
        Self {
            side,
            org,
            request_id,
            subjects,
            seen_hosts,
            earlier: Vec::new(),
            caller: Caller::none(),
            deadline: tokio::time::Instant::now(),
            webhook_bodies: None,
        }
    }

    /// A response-side batch for the calls of one hold, with the caller (SMA-856).
    #[must_use]
    pub fn for_calls(org: Option<Uuid>, request_id: Option<String>, subjects: Vec<Subject>, seen_hosts: Option<Arc<HostSet>>, caller: Caller) -> Self {
        Self {
            caller,
            ..Self::new(Side::Response, org, request_id, subjects, seen_hosts)
        }
    }
}

impl fmt::Debug for DetectBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DetectBatch {{ side: {:?}, org: {}, request_id: {}, subjects: {}, seen_hosts: {}, earlier: {} }}",
            self.side,
            self.org.is_some(),
            self.request_id.is_some(),
            self.subjects.len(),
            self.seen_hosts.as_ref().map_or_else(|| "none".to_owned(), |h| h.len().to_string()),
            self.earlier.len()
        )
    }
}

/// Why a detector gave no findings. Fixed variants, no free text (§ 6.2.4).
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetectorError {
    /// The detector failed.
    Failed,
    /// The detector returned a number of finding vectors that is not the number of subjects.
    WrongShape,
    /// The detector did not finish before the deadline.
    Timeout,
    /// The detector panicked.
    Panicked,
}

impl fmt::Debug for DetectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Failed => "Failed",
            Self::WrongShape => "WrongShape",
            Self::Timeout => "Timeout",
            Self::Panicked => "Panicked",
        })
    }
}

/// Why a webhook gave no findings, as a fixed reason (SMA-856 § 6.4). The first nine are the
/// reasons of the HTTP adapter ([`ADAPTER`]). The others come from the pipeline. The reason is the
/// `version` of the failure finding and the `reason` label of the failure counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookFailure {
    /// The request body is larger than `max_request_bytes`.
    RequestTooLarge,
    /// No in-flight permit was free in time.
    Overloaded,
    /// The connection or the transfer failed.
    Transport,
    /// The provider did not answer in the `timeout_ms` of the provider.
    Timeout,
    /// The adapter stopped at `deadline - 10 ms`.
    Deadline,
    /// The provider answered with a status that is not 200.
    Status,
    /// The answer is larger than the limit.
    ResponseTooLarge,
    /// The answer has no valid signature.
    Signature,
    /// The answer is not a valid answer.
    Malformed,
    /// The detector panicked.
    Panic,
    /// The detector gave an error.
    Error,
    /// The detector gave the wrong number of results.
    Shape,
    /// A finding failed the strict check.
    Check,
    /// The opt-in names a provider that the gateway does not know.
    UnknownProvider,
    /// The org is not in the `orgs` of the provider.
    NotPermitted,
    /// The provider is retired.
    Retired,
}

impl WebhookFailure {
    /// The reason as a snake_case word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequestTooLarge => "request_too_large",
            Self::Overloaded => "overloaded",
            Self::Transport => "transport",
            Self::Timeout => "timeout",
            Self::Deadline => "deadline",
            Self::Status => "status",
            Self::ResponseTooLarge => "response_too_large",
            Self::Signature => "signature",
            Self::Malformed => "malformed",
            Self::Panic => "panic",
            Self::Error => "error",
            Self::Shape => "shape",
            Self::Check => "check",
            Self::UnknownProvider => "unknown_provider",
            Self::NotPermitted => "not_permitted",
            Self::Retired => "retired",
        }
    }
}

/// The nine reasons that the HTTP adapter gives.
pub const ADAPTER: [WebhookFailure; 9] = [
    WebhookFailure::RequestTooLarge,
    WebhookFailure::Overloaded,
    WebhookFailure::Transport,
    WebhookFailure::Timeout,
    WebhookFailure::Deadline,
    WebhookFailure::Status,
    WebhookFailure::ResponseTooLarge,
    WebhookFailure::Signature,
    WebhookFailure::Malformed,
];

/// The `detector_error` finding of a failed detector: the reason is the version, and there is no
/// evidence (SMA-856 D16).
#[must_use]
pub fn failure_finding(detector: &str, severity: Severity, reason: WebhookFailure) -> Finding {
    Finding::new(detector, "detector_error", severity, reason.as_str())
}

/// Whether a `Finding.version` of a provider matches `[a-z0-9._-]{0,32}` (D14).
#[must_use]
pub fn valid_version(v: &str) -> bool {
    v.len() <= 32 && v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether a provider name matches `[a-z0-9_]{1,64}`.
#[must_use]
pub fn valid_name(n: &str) -> bool {
    (1..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// An async detector: a webhook (SMA-856) or the judge (SMA-857).
#[async_trait]
pub trait Detector: Send + Sync {
    /// The name in `Finding.detector`. It must be a known name (D37).
    fn name(&self) -> &str;
    /// The severity of a failure finding that the pipeline makes for this detector (D16).
    fn failure_severity(&self) -> Severity {
        Severity::High
    }
    /// The finding kinds that the detector declares. `None`: every valid kind is allowed.
    fn kinds(&self) -> Option<&BTreeSet<String>> {
        None
    }
    /// One vector of findings per subject, in subject order.
    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError>;
}

/// One registered webhook provider (SMA-856 § 6.6): its detector, the orgs it serves and whether it
/// is retired.
#[derive(Clone)]
pub struct WebhookEntry {
    /// The detector that calls the provider.
    pub detector: Arc<dyn Detector>,
    /// The orgs that the provider serves.
    pub orgs: OrgScope,
    /// A retired provider is never called. An opt-in to it gives a `retired` failure finding.
    pub retired: bool,
}

impl WebhookEntry {
    /// A shared provider: every org, not retired.
    #[must_use]
    pub fn shared(detector: Arc<dyn Detector>) -> Self {
        Self {
            detector,
            orgs: OrgScope::All,
            retired: false,
        }
    }
}

impl fmt::Debug for WebhookEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WebhookEntry {{ retired: {} }}", self.retired)
    }
}

/// The result of the opt-in resolution of one name (SMA-856 D7, D25).
#[derive(Debug)]
#[non_exhaustive]
pub enum OptIn<'a> {
    /// The provider runs.
    Run(&'a WebhookEntry),
    /// The provider exists, but the org is not in its `orgs`.
    NotPermitted,
    /// The provider is retired.
    Retired(&'a WebhookEntry),
    /// No provider has this name.
    Unknown,
}

/// The webhook providers of one gateway, in config order.
#[derive(Clone, Default)]
pub struct WebhookRegistry {
    entries: Vec<WebhookEntry>,
}

impl WebhookRegistry {
    /// A registry of the entries.
    #[must_use]
    pub fn new(entries: Vec<WebhookEntry>) -> Self {
        Self { entries }
    }

    /// The number of providers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there is no provider.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The providers, in config order.
    pub fn iter(&self) -> impl Iterator<Item = &WebhookEntry> {
        self.entries.iter()
    }

    /// Resolve one opt-in name for a batch of `org`. A retired provider gives `Retired` whatever the
    /// org. An `Only` scope permits `Some(org)` in the set and never `None`; `All` permits any batch.
    #[must_use]
    pub fn resolve(&self, name: &str, org: Option<Uuid>) -> OptIn<'_> {
        let Some(entry) = self.entries.iter().find(|e| e.detector.name() == name) else {
            return OptIn::Unknown;
        };
        if entry.retired {
            return OptIn::Retired(entry);
        }
        let permitted = match &entry.orgs {
            OrgScope::All => true,
            OrgScope::Only(set) => org.is_some_and(|o| set.contains(&o)),
        };
        if permitted { OptIn::Run(entry) } else { OptIn::NotPermitted }
    }
}

impl From<Vec<Arc<dyn Detector>>> for WebhookRegistry {
    fn from(detectors: Vec<Arc<dyn Detector>>) -> Self {
        Self::new(detectors.into_iter().map(WebhookEntry::shared).collect())
    }
}

impl fmt::Debug for WebhookRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WebhookRegistry {{ providers: {} }}", self.entries.len())
    }
}

/// The built-in stage (plan P-1). It is synchronous, and it gives the hosts of each call too.
pub trait BuiltInStage: Send + Sync {
    /// One vector of findings and one host set per subject, in subject order.
    fn run(&self, batch: &DetectBatch) -> PipelineOutput;
}

/// The pipeline stage that gave a finding (§ 6.6.2). The metric label of a finding follows its
/// stage, not the detector name in the finding: a webhook that names `kernel` still counts as
/// `webhook` (A2, D37).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stage {
    /// The built-in stage. Its detector names are fixed by this crate.
    BuiltIn,
    /// A configured webhook (SMA-856).
    Webhook,
    /// The judge (SMA-857).
    Judge,
}

/// The output of a detector run: per subject, the findings and the hosts. The hosts are `Some`
/// only for a call that the built-in stage scanned in full.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct PipelineOutput {
    /// The findings, per subject.
    pub findings: Vec<Vec<Finding>>,
    /// The hosts, per subject.
    pub hosts: Vec<Option<HostSet>>,
    /// The stage of each finding, in the shape of `findings`. The pipeline sets it. A
    /// [`BuiltInStage`] leaves it empty, because all of its findings are of [`Stage::BuiltIn`].
    pub stages: Vec<Vec<Stage>>,
}

impl PipelineOutput {
    /// The stage of finding `j` of subject `i`. A finding with no stage entry is of the built-in
    /// stage, the only stage that does not go through the pipeline's own bookkeeping.
    #[must_use]
    pub fn stage_of(&self, i: usize, j: usize) -> Stage {
        self.stages.get(i).and_then(|s| s.get(j)).copied().unwrap_or(Stage::BuiltIn)
    }
}

impl fmt::Debug for PipelineOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let findings: usize = self.findings.iter().map(Vec::len).sum();
        let hosts = self.hosts.iter().filter(|h| h.is_some()).count();
        write!(f, "PipelineOutput {{ subjects: {}, findings: {findings}, hosts: {hosts} }}", self.findings.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_kernel::detect::{Declared, Evidence, Severity, ToolInput};

    const CANARY: &str = "canary-7f3e.example";

    fn subjects() -> Vec<Subject> {
        let input = ToolInput::Json(format!(r#"{{"u":"https://{CANARY}/"}}"#));
        vec![
            Subject::Call {
                index: 0,
                invocation: ToolInvocation::new(format!("tool-{CANARY}"), input, Declared::Yes),
                schema: Some(Arc::new(serde_json::json!({ "description": CANARY }))),
            },
            Subject::Result {
                index: 1,
                tool: Some(CANARY.to_owned()),
                text: Arc::from(format!("result {CANARY}")),
            },
        ]
    }

    #[test]
    fn debug_prints_no_content() {
        let subjects = subjects();
        assert_eq!(format!("{:?}", subjects[0]), "Subject::Call { index: 0, .. }");
        assert_eq!(format!("{:?}", subjects[1]), "Subject::Result { index: 1, .. }");
        let mut seen = HostSet::new();
        seen.insert(CANARY);
        let mut batch = DetectBatch::new(Side::Response, Some(Uuid::nil()), Some(CANARY.to_owned()), subjects, Some(Arc::new(seen.clone())));
        batch.earlier = vec![vec![Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1").with_evidence(Evidence::host(
            "r",
            None,
            CANARY.to_owned(),
        ))]];
        let text = format!("{batch:?}");
        assert_eq!(text, "DetectBatch { side: Response, org: true, request_id: true, subjects: 2, seen_hosts: 1, earlier: 1 }");
        let out = PipelineOutput {
            findings: batch.earlier.clone(),
            hosts: vec![Some(seen), None],
            stages: vec![vec![Stage::Webhook]],
        };
        let text = format!("{out:?}");
        assert_eq!(text, "PipelineOutput { subjects: 1, findings: 1, hosts: 1 }");
        assert!(!text.contains(CANARY));
    }

    #[test]
    fn detector_errors_print_only_their_variant() {
        let all = [DetectorError::Failed, DetectorError::WrongShape, DetectorError::Timeout, DetectorError::Panicked];
        assert_eq!(all.map(|e| format!("{e:?}")), ["Failed", "WrongShape", "Timeout", "Panicked"]);
    }

    #[test]
    fn a_finding_without_a_stage_entry_is_built_in() {
        let out = PipelineOutput {
            findings: vec![vec![detector_error_finding(), detector_error_finding()]],
            hosts: vec![None],
            stages: vec![vec![Stage::Webhook]],
        };
        assert_eq!((out.stage_of(0, 0), out.stage_of(0, 1), out.stage_of(1, 0)), (Stage::Webhook, Stage::BuiltIn, Stage::BuiltIn));
    }

    fn detector_error_finding() -> Finding {
        Finding::new("kernel", "detector_error", Severity::High, "")
    }

    #[test]
    fn the_caller_debug_prints_presence_and_a_count() {
        assert_eq!(format!("{:?}", Caller::none()), "Caller { principal: false, session: false, opt_in: 0 }");
        let caller = Caller {
            principal: Arc::from(CANARY),
            session: Some(Uuid::nil()),
            opt_in: Arc::from(vec![CANARY.to_owned(), "b".to_owned()]),
        };
        let text = format!("{caller:?}");
        assert_eq!(text, "Caller { principal: true, session: true, opt_in: 2 }");
        assert!(!text.contains(CANARY));
    }

    #[test]
    fn for_calls_is_a_response_batch_with_the_caller() {
        let caller = Caller {
            principal: Arc::from("prn:x"),
            session: None,
            opt_in: Arc::from(vec!["a".to_owned()]),
        };
        let b = DetectBatch::for_calls(None, None, Vec::new(), None, caller);
        assert_eq!((b.side, b.caller.opt_in.len(), b.webhook_bodies.is_none()), (Side::Response, 1, true));
    }

    #[test]
    fn the_failure_reasons_are_the_closed_set() {
        assert_eq!(
            ADAPTER.map(WebhookFailure::as_str),
            [
                "request_too_large",
                "overloaded",
                "transport",
                "timeout",
                "deadline",
                "status",
                "response_too_large",
                "signature",
                "malformed"
            ]
        );
        let f = failure_finding("hook_a", Severity::Low, WebhookFailure::UnknownProvider);
        assert_eq!(
            (f.kind.as_str(), f.version.as_str(), f.severity, f.evidence.len()),
            ("detector_error", "unknown_provider", Severity::Low, 0)
        );
        for r in [
            WebhookFailure::Panic,
            WebhookFailure::Error,
            WebhookFailure::Shape,
            WebhookFailure::Check,
            WebhookFailure::NotPermitted,
            WebhookFailure::Retired,
        ] {
            assert!(valid_version(r.as_str()), "{r:?}");
        }
    }

    #[test]
    fn the_version_and_name_rules() {
        let long = "a".repeat(33);
        for v in ["", "v1", "1.2.3-rc_1", &"a".repeat(32)] {
            assert!(valid_version(v), "{v}");
        }
        for v in ["V1", "v 1", "v!", "\u{e9}", long.as_str()] {
            assert!(!valid_version(v), "{v}");
        }
        for n in ["a", "hook_1", &"a".repeat(64)] {
            assert!(valid_name(n), "{n}");
        }
        for n in ["", "Hook", "a-b", "a.b", &"a".repeat(65)] {
            assert!(!valid_name(n), "{n}");
        }
    }

    #[test]
    fn only_a_call_is_a_call() {
        let s = subjects();
        assert_eq!((s[0].is_call(), s[1].is_call()), (true, false));
    }
}
