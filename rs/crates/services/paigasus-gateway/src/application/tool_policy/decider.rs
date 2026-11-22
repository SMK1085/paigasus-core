// SPDX-License-Identifier: Apache-2.0

//! The Cedar decider (SMA-752 § 6.3.4). It decides the calls of one hold from the request's policy
//! snapshot (D30): it never reads the cache. Evaluation is synchronous inside the future, and the
//! decider spawns nothing (SMA-749 D25).
//!
//! With the detectors on (SMA-855 § 6.9), the future runs the detector pipeline first, under an
//! absolute deadline from the hold start (D35), and then decides. The pipeline runs inside this one
//! future, which the relay polls: still nothing is spawned (D30).
//!
//! The decider writes no log line. It counts the decision metrics and computes the keyed argument digest for
//! the decision log. The recorder writes the per-decision line (SMA-755). Metrics never hold the arguments
//! (ADR-0027 (i)).

use std::sync::Arc;
use std::time::{Duration, Instant};

use metrics::counter;
use paigasus_gateway_core::decision_log::{DecisionDetail, FindingRecord, RecordBasis};
use paigasus_gateway_core::{
    CallScan, DecideFuture, DecidedCall, Decision, DecisionContext, OnUnavailable, PolicySet, PolicySnapshot, TenantMode, ToolCall, ToolCallDecider, ToolTurn, call_schema, decide_calls, invocation,
};
use paigasus_kernel::detect::Severity;
use paigasus_observability::names;

use crate::application::decision_log::DecisionRecorder;
use crate::application::detectors::DetectorPipeline;
use crate::config::DetectorSettings;
use crate::domain::detectors::{DetectBatch, PipelineOutput, Side, Stage, Subject};

/// The `ToolCallDecider` that evaluates the tool policies. It holds the one shared recorder, for
/// the argument digest (SMA-755), and the detector pipeline when the detectors are on (SMA-855).
pub struct CedarDecider {
    recorder: Arc<DecisionRecorder>,
    detectors: Option<Detectors>,
}

/// The detector pipeline of the decider and what its deadline needs (D35).
#[derive(Clone)]
struct Detectors {
    pipeline: Arc<DetectorPipeline>,
    /// `tool_interception.decide_timeout_secs`: the time of a hold from its completion.
    decide_timeout: Duration,
    settings: DetectorSettings,
}

impl Detectors {
    /// `min(hold_start + decide_timeout - margin, now + timeout)` (D35). The hold part is
    /// absolute, so a hold that waited in the queue behind another one gets a shorter run.
    fn deadline(&self, hold_start: Instant) -> tokio::time::Instant {
        let by_hold = tokio::time::Instant::from_std(hold_start + self.decide_timeout.saturating_sub(self.settings.margin));
        by_hold.min(tokio::time::Instant::now() + self.settings.timeout)
    }
}

impl CedarDecider {
    #[must_use]
    pub fn new(recorder: Arc<DecisionRecorder>) -> Self {
        Self { recorder, detectors: None }
    }

    /// Run `pipeline` on the calls of each hold before the decision (SMA-855 § 6.9).
    /// `decide_timeout` is `tool_interception.decide_timeout_secs`, and `settings` holds the
    /// pipeline timeout and the margin of the deadline (D35).
    #[must_use]
    pub fn with_detectors(mut self, pipeline: Arc<DetectorPipeline>, decide_timeout: Duration, settings: DetectorSettings) -> Self {
        self.detectors = Some(Detectors { pipeline, decide_timeout, settings });
        self
    }

    /// The detector pipeline of this decider, `None` with the detectors off. A runtime test reads it
    /// to check that the runtime gives the decider the one pipeline.
    #[must_use]
    pub fn pipeline(&self) -> Option<&Arc<DetectorPipeline>> {
        self.detectors.as_ref().map(|d| &d.pipeline)
    }

    /// The recorder of this decider. A runtime test reads it to check that the decider, the request side and
    /// the report site share one recorder (P-8).
    #[must_use]
    pub fn recorder(&self) -> &Arc<DecisionRecorder> {
        &self.recorder
    }
}

/// The finding kinds that `gateway_tool_policy_findings_total` labels (SMA-755 § 5.9, SMA-855 § 6.11.3).
pub const FINDING_KINDS: [&str; 9] = [
    "name_collision",
    "unparsed_arguments",
    "undeclared_tool",
    "invalid_args",
    "unseen_host",
    "secret_in_args",
    "injection_markers",
    "detector_error",
    "other",
];

/// The detector labels of the same metric. A webhook has one label, `webhook`, whatever its name (A2).
pub const DETECTOR_LABELS: [&str; 10] = [
    "kernel",
    "built_in",
    "undeclared_tool",
    "invalid_args",
    "unseen_host",
    "secret_in_args",
    "injection_markers",
    "webhook",
    "judge",
    "other",
];

/// The valid `(kind, detector, severity)` triples (SMA-855 P-9). It is the one source of the label
/// mapping and of the prime. A finding outside the table counts as `other`.
pub const FINDING_TRIPLES: [(&str, &str, &str); 13] = [
    ("name_collision", "kernel", "high"),
    ("unparsed_arguments", "kernel", "info"),
    ("unparsed_arguments", "kernel", "medium"),
    ("undeclared_tool", "undeclared_tool", "medium"),
    ("invalid_args", "invalid_args", "low"),
    ("unseen_host", "unseen_host", "medium"),
    ("secret_in_args", "secret_in_args", "high"),
    ("injection_markers", "injection_markers", "medium"),
    ("injection_markers", "injection_markers", "high"),
    ("detector_error", "kernel", "high"),
    ("detector_error", "built_in", "high"),
    ("detector_error", "webhook", "high"),
    ("detector_error", "judge", "high"),
];

/// The severity labels, for the `other` triples.
const SEVERITY_LABELS: [&str; 4] = ["info", "low", "medium", "high"];

/// The `kind` metric label of a finding. An unknown kind is `other`.
#[must_use]
pub fn finding_label(kind: &str) -> &'static str {
    FINDING_KINDS.iter().find(|k| **k == kind).copied().unwrap_or("other")
}

/// The `(kind, detector, severity)` labels of a finding. `stage` is the pipeline stage that gave
/// the finding, `None` for a finding of the kernel or the gateway core. A webhook finding is
/// labelled `webhook` and a judge finding `judge`, whatever detector name it gives (A2, D37): the
/// name of a webhook is not a label, and a webhook that names `kernel` is not counted as the
/// kernel. A finding outside [`FINDING_TRIPLES`] is `(other, other, <severity>)`. No severity
/// counts as `high`.
#[must_use]
pub fn finding_labels(f: &FindingRecord, stage: Option<Stage>) -> (&'static str, &'static str, &'static str) {
    let severity = f.severity.map_or("high", |s| s.as_str());
    let severity = SEVERITY_LABELS.iter().find(|s| **s == severity).copied().unwrap_or("high");
    let detector = match stage {
        Some(Stage::Webhook) => "webhook",
        Some(Stage::Judge) => "judge",
        _ => DETECTOR_LABELS.iter().find(|d| **d == f.detector).copied().unwrap_or("other"),
    };
    FINDING_TRIPLES
        .iter()
        .find(|(k, d, s)| *k == f.kind && *d == detector && *s == severity)
        .copied()
        .unwrap_or(("other", "other", severity))
}

/// Increment `gateway_tool_policy_findings_total` for a finding.
pub(crate) fn count_finding(side: &'static str, f: &FindingRecord, stage: Option<Stage>) {
    let (kind, detector, severity) = finding_labels(f, stage);
    counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => side, "kind" => kind, "detector" => detector, "severity" => severity).increment(1);
}

/// Prime `gateway_tool_policy_findings_total` at zero for every valid triple and the four `other`
/// triples, on each side: 2 x 17 = 34 series.
pub fn prime_tool_policy_metrics() {
    for side in ["request", "response"] {
        let others = SEVERITY_LABELS.map(|s| ("other", "other", s));
        for (kind, detector, severity) in FINDING_TRIPLES.into_iter().chain(others) {
            counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => side, "kind" => kind, "detector" => detector, "severity" => severity).absolute(0);
        }
    }
}

/// The `effect` metric label of a decision.
pub(crate) fn effect_label(d: Decision) -> &'static str {
    match d {
        Decision::Allow => "allow",
        Decision::Observe { .. } => "observe",
        // `Block`, and any later variant: count it as the stricter effect.
        _ => "block",
    }
}

/// A pipeline finding of a webhook or the judge, as its record spells it, with its stage.
type Staged = (String, String, Severity, Stage);

/// The scans of `decide_calls` from the pipeline output: exactly one per subject, in subject order,
/// and the subjects are the calls in call order. With them, per call, the findings of a webhook
/// or the judge with their stage, for the metric label.
fn scans_of(out: PipelineOutput) -> (Vec<CallScan>, Vec<Vec<Staged>>) {
    let late: Vec<Vec<Staged>> = out
        .findings
        .iter()
        .enumerate()
        .map(|(i, found)| {
            found
                .iter()
                .enumerate()
                .filter_map(|(j, f)| match out.stage_of(i, j) {
                    Stage::BuiltIn => None,
                    stage => Some((f.kind.clone(), f.detector.clone(), f.severity, stage)),
                })
                .collect()
        })
        .collect();
    let scans = out.findings.into_iter().zip(out.hosts).map(|(f, h)| CallScan::new(f, h)).collect();
    (scans, late)
}

/// The stage of a record: the first unused pipeline finding of a webhook or the judge with the same
/// kind, detector and severity. Two such findings label alike, so the count of each label is exact.
/// A record with no match came from the kernel, the gateway core or the built-in stage (`None`).
fn take_stage(pending: &mut Vec<Staged>, f: &FindingRecord) -> Option<Stage> {
    let at = pending.iter().position(|(k, d, s, _)| *k == f.kind && *d == f.detector && Some(*s) == f.severity)?;
    Some(pending.swap_remove(at).3)
}

/// The detector batch of the calls of one hold: one call subject per call, in call order (§ 6.9).
/// The seen hosts are those of the request screen, `None` when no screen ran, so `unseen_host` does
/// not run then (S1).
fn call_batch(snapshot: &PolicySnapshot, ctx: &DecisionContext, turn: &ToolTurn, calls: &[ToolCall]) -> DetectBatch {
    let org = match &snapshot.set {
        PolicySet::Available { facts, .. } => Some(facts.org),
        _ => None,
    };
    let subjects = calls
        .iter()
        .enumerate()
        .map(|(index, c)| Subject::Call {
            index,
            invocation: invocation(c, turn),
            schema: call_schema(c, turn),
        })
        .collect();
    let seen = snapshot.seen_hosts.as_ref().map(|s| Arc::clone(&s.0));
    DetectBatch::new(Side::Response, org, ctx.request_id.clone(), subjects, seen)
}

/// The snapshot of `ctx` when its policy set is available: the only case in which the pipeline
/// runs (D27).
fn available(ctx: &DecisionContext) -> Option<&PolicySnapshot> {
    ctx.policy.as_deref().filter(|s| matches!(s.set, PolicySet::Available { .. }))
}

/// Decide the calls from the snapshot of `ctx`. `None` is an unavailable set under `closed`: the
/// tool policy is on, but the request side did not run, so every call is blocked (fail closed).
///
/// `found` is the pipeline output, `None` when no pipeline ran. `started` is the start of the
/// overhead timer: the caller starts it before the pipeline run, so the overhead holds the
/// pipeline time (§ 6.9). It is a tokio instant, so a test with paused time measures it.
fn decide_now(recorder: &DecisionRecorder, ctx: &DecisionContext, turn: &ToolTurn, calls: &[ToolCall], found: Option<PipelineOutput>, started: tokio::time::Instant) -> Vec<DecidedCall> {
    let fallback;
    let snapshot = match ctx.policy.as_deref() {
        Some(s) => s,
        None => {
            fallback = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
            &fallback
        }
    };
    // No scan when no pipeline ran: the kernel then reads the hosts of each call itself.
    let (scans, mut late) = found.map(scans_of).unwrap_or_default();
    let outcomes = decide_calls(snapshot, turn, calls, &scans, &Instant::now);
    let mode = snapshot.mode.as_str();
    // The digest is computed inside the timed region, so it is part of the overhead sample (§ 5.3).
    let digests: Vec<_> = calls.iter().map(|c| recorder.digest(&c.arguments)).collect();
    for (i, o) in outcomes.iter().enumerate() {
        let effect = effect_label(o.decision);
        let basis = o.basis.as_str();
        counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "response", "mode" => mode, "effect" => effect, "basis" => basis).increment(1);
        if let Some(c) = &o.cedar {
            let mut pending = late.get_mut(i).map(std::mem::take).unwrap_or_default();
            for f in &c.findings {
                count_finding("response", f, take_stage(&mut pending, f));
            }
        }
    }
    // Preflight F14: the decider records no sample. It adds its time to the request's snapshot,
    // and the gateway records one sample per request (`OverheadSample`), however many holds.
    if let Some(s) = ctx.policy.as_deref() {
        s.add_decide_elapsed(started.elapsed());
    }
    // Every call gets a detail for the decision log (SMA-755).
    outcomes
        .into_iter()
        .zip(digests)
        .map(|(o, digest)| DecidedCall::new(o.decision, Some(Arc::new(DecisionDetail::new(RecordBasis::from(o.basis), o.cedar, digest)))))
        .collect()
}

impl ToolCallDecider for CedarDecider {
    fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        self.decide_by(ctx, turn, calls, Instant::now())
    }

    fn decide_by(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>, hold_start: Instant) -> DecideFuture {
        // D27: the pipeline runs only with the detectors on and an available policy set.
        let Some(detectors) = self.detectors.clone().filter(|_| available(&ctx).is_some()) else {
            let started = tokio::time::Instant::now();
            return Box::pin(std::future::ready(Ok(decide_now(&self.recorder, &ctx, &turn, &calls, None, started))));
        };
        let recorder = Arc::clone(&self.recorder);
        // One future that the relay polls: the pipeline runs inside it, and nothing is spawned (D30).
        Box::pin(async move {
            // The overhead timer starts before the pipeline run (§ 6.9).
            let started = tokio::time::Instant::now();
            let deadline = detectors.deadline(hold_start);
            let found = match available(&ctx) {
                Some(snapshot) => Some(detectors.pipeline.run(call_batch(snapshot, &ctx, &turn, &calls), deadline).await),
                None => None,
            };
            Ok(decide_now(&recorder, &ctx, &turn, &calls, found, started))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_gateway_core::{BlockCode, DecisionContext, Format, PolicySet, PolicySnapshot, TenantMode, TenantPart, ToolTurn, parse_body, parse_request};

    use super::*;
    use crate::application::tool_policy::RequestToolPolicy;
    use crate::application::tool_policy::testing::{self, FakeSource, ORG_A, ORG_A_PRN, PRINCIPAL, TEAM_IN_A, chat_request, settings};

    const BLOCK_POLICY: Decision = Decision::Block { code: BlockCode::Policy };
    const BLOCK_UNAVAILABLE: Decision = Decision::Block { code: BlockCode::PolicyUnavailable };

    /// The calls of a chat completion that calls each name once.
    fn calls(names: &[&str]) -> Vec<ToolCall> {
        let tool_calls: Vec<serde_json::Value> = names
            .iter()
            .enumerate()
            .map(|(i, n)| serde_json::json!({"id": format!("c{i}"), "type": "function", "function": {"name": n, "arguments": "{}"}}))
            .collect();
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": tool_calls}, "finish_reason": "tool_calls"}]}).to_string();
        parse_body(Format::OpenAiChat, body.as_bytes(), 1 << 20).expect("body").calls
    }

    /// Run the request side, then the decider with the snapshot it returned.
    async fn decide_with(policy: &RequestToolPolicy, scope: &str, tools: &[&str], call_names: &[&str]) -> (Arc<PolicySnapshot>, Vec<Decision>) {
        let body = chat_request(tools, "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, scope, Some("req-1".into()));
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let snapshot = Arc::clone(&out.snapshot);
        let decisions = decide(ctx.with_policy(out.snapshot), turn, call_names).await;
        (snapshot, decisions)
    }

    async fn decide_full(ctx: DecisionContext, turn: ToolTurn, call_names: &[&str]) -> Vec<DecidedCall> {
        let calls = calls(call_names);
        let n = calls.len();
        let out = CedarDecider::new(Arc::new(DecisionRecorder::tracing()))
            .decide(Arc::new(ctx), Arc::new(turn), calls)
            .await
            .expect("decide");
        assert_eq!(out.len(), n, "one decision per call");
        out
    }

    async fn decide(ctx: DecisionContext, turn: ToolTurn, call_names: &[&str]) -> Vec<Decision> {
        decide_full(ctx, turn, call_names).await.into_iter().map(|d| d.decision).collect()
    }

    #[tokio::test]
    async fn the_decider_attaches_a_detail_to_every_call() {
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let decided = decide_full(ctx.clone().with_policy(out.snapshot), turn, &["x", "y", "rm", "z"]).await;
        assert_eq!(decided.len(), 4);
        for d in &decided {
            let detail = d.detail.as_ref().expect("a detail for every call");
            assert_eq!(detail.basis, RecordBasis::Policy);
            assert!(detail.cedar.is_some(), "a kernel verdict has a Cedar detail");
            assert!(detail.arguments.is_none(), "no digest: the recorder has no key");
        }
        // `rm` is a floor deny: its detail says so, and its decision is the block.
        assert_eq!(decided[2].decision, BLOCK_POLICY);
        assert!(decided[2].detail.as_ref().and_then(|d| d.cedar.as_ref()).is_some_and(|c| c.floor_denied));
        // An unavailable set has a basis and no Cedar detail.
        let unavailable = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let decided = decide_full(unavailable, ToolTurn::Unavailable("test"), &["a", "b"]).await;
        for d in &decided {
            let detail = d.detail.as_ref().expect("a detail");
            assert_eq!((detail.basis, detail.cedar.is_none()), (RecordBasis::Unavailable, true));
        }
    }

    #[tokio::test]
    async fn response_side_findings_are_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let ctx = ctx.with_policy(out.snapshot);
        // The arguments of the call are valid JSON but not an object.
        let reply = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [
            {"id": "c0", "type": "function", "function": {"name": "ok", "arguments": "[1]"}},
            {"id": "c1", "type": "function", "function": {"name": "ok", "arguments": "{}"}}
        ]}, "finish_reason": "tool_calls"}]})
        .to_string();
        let calls = parse_body(Format::OpenAiChat, reply.as_bytes(), 1 << 20).expect("body").calls;
        let decided = CedarDecider::new(Arc::new(DecisionRecorder::tracing()))
            .decide(Arc::new(ctx), Arc::new(turn), calls)
            .await
            .expect("decide");
        assert_eq!(decided.len(), 2);
        let m = testing::Snap::take(&snapshotter);
        let finding = |side: &str, kind: &str, detector: &str, severity: &str| {
            m.counter(
                names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
                &[("side", side), ("kind", kind), ("detector", detector), ("severity", severity)],
            )
        };
        // `a_finding_increments_its_triple`: the bad-JSON call counts under its own triple only.
        assert_eq!(
            (
                finding("response", "unparsed_arguments", "kernel", "medium"),
                finding("response", "name_collision", "kernel", "high"),
                finding("request", "unparsed_arguments", "kernel", "medium")
            ),
            (1, 0, 0)
        );
    }

    // ---- SMA-855 Task 21: the pipeline on the response side ----------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    use paigasus_gateway_core::SeenHosts;
    use paigasus_kernel::detect::{DETECTOR_KERNEL, HostSet, SECRET_IN_ARGS, UNSEEN_HOST, detector_error};

    use crate::adapters::detectors::BuiltInDetector;
    use crate::application::tool_policy::testing::{FixedDetector, SlowDetector};
    use crate::config::DETECTOR_MARGIN_MS;
    use crate::domain::detectors::{BuiltInStage, Detector};

    /// The starter policy of § 6.12: permit all, forbid a high finding.
    const STARTER: &str = r#"@id("all") permit(principal, action, resource); @id("detectors-block-high") forbid (principal, action, resource) when { context.max_severity >= 3 };"#;
    const DECIDE_TIMEOUT: Duration = Duration::from_secs(5);
    const PIPELINE_TIMEOUT: Duration = Duration::from_millis(500);

    fn aws_args() -> String {
        format!(r#"{{"k":"{}"}}"#, ["AKIA", "ABCDEFGHIJKLMNOP"].concat())
    }

    fn detector_settings() -> DetectorSettings {
        DetectorSettings {
            enabled: true,
            timeout: PIPELINE_TIMEOUT,
            max_scan_bytes: 1 << 24,
            margin: Duration::from_millis(DETECTOR_MARGIN_MS),
        }
    }

    fn pipeline_with(built_in: Arc<dyn BuiltInStage>, webhooks: Vec<Arc<dyn Detector>>) -> Arc<DetectorPipeline> {
        Arc::new(DetectorPipeline::new(built_in, webhooks, None, PIPELINE_TIMEOUT))
    }

    fn detecting(webhooks: Vec<Arc<dyn Detector>>) -> CedarDecider {
        CedarDecider::new(Arc::new(DecisionRecorder::tracing())).with_detectors(pipeline_with(Arc::new(BuiltInDetector::new(1 << 24)), webhooks), DECIDE_TIMEOUT, detector_settings())
    }

    /// The calls of a chat completion, one per `(name, arguments)`.
    fn calls_with(items: &[(&str, &str)]) -> Vec<ToolCall> {
        let tool_calls: Vec<serde_json::Value> = items
            .iter()
            .enumerate()
            .map(|(i, (n, a))| serde_json::json!({"id": format!("c{i}"), "type": "function", "function": {"name": n, "arguments": a}}))
            .collect();
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": tool_calls}, "finish_reason": "tool_calls"}]}).to_string();
        parse_body(Format::OpenAiChat, body.as_bytes(), 1 << 20).expect("body").calls
    }

    /// The request side of a chat request with no tools, for a tenant text, in `mode`.
    async fn request_side(tenant: &str, mode: TenantMode) -> (DecisionContext, ToolTurn) {
        let source = FakeSource::new(testing::FLOOR, tenant);
        let policy = testing::policy(&source, settings(mode));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, Some("req-1".into()));
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        (ctx.with_policy(out.snapshot), turn)
    }

    fn kinds(d: &DecidedCall) -> Vec<(String, String)> {
        d.detail
            .as_ref()
            .and_then(|x| x.cedar.as_ref())
            .map(|c| c.findings.iter().map(|f| (f.kind.clone(), f.detector.clone())).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn the_pipeline_findings_reach_the_decision() {
        let (ctx, turn) = request_side(STARTER, TenantMode::Enforce).await;
        let (ctx, turn) = (Arc::new(ctx), Arc::new(turn));
        let args = aws_args();
        let out = detecting(Vec::new()).decide(Arc::clone(&ctx), Arc::clone(&turn), calls_with(&[("ok", &args)])).await.expect("decide");
        assert_eq!(out[0].decision, BLOCK_POLICY, "the starter policy forbids a high finding");
        assert!(kinds(&out[0]).contains(&(SECRET_IN_ARGS.to_owned(), SECRET_IN_ARGS.to_owned())), "{:?}", kinds(&out[0]));
        // Control: without the detectors the same call is allowed.
        let plain = CedarDecider::new(Arc::new(DecisionRecorder::tracing()))
            .decide(ctx, turn, calls_with(&[("ok", &args)]))
            .await
            .expect("decide");
        assert_eq!(plain[0].decision, Decision::Allow);
    }

    #[tokio::test]
    async fn a_two_call_hold_gives_two_scans_in_order() {
        // Carried from Task 13: one scan per call, in call order. A wrong count would give every
        // call a `detector_error`; a wrong order would put the secret on the first call.
        let (ctx, turn) = request_side(STARTER, TenantMode::Enforce).await;
        let args = aws_args();
        let out = detecting(Vec::new())
            .decide(Arc::new(ctx), Arc::new(turn), calls_with(&[("ok", "{}"), ("ok", &args)]))
            .await
            .expect("decide");
        assert_eq!(out.iter().map(|d| d.decision).collect::<Vec<_>>(), [Decision::Allow, BLOCK_POLICY]);
        // No tool is declared, so both calls also get `undeclared_tool`.
        let has = |d: &DecidedCall, kind: &str| kinds(d).iter().any(|(k, _)| k == kind);
        assert_eq!((has(&out[0], SECRET_IN_ARGS), has(&out[1], SECRET_IN_ARGS)), (false, true));
        assert!(out.iter().all(|d| !has(d, "detector_error")), "the count check did not trip");
        assert!(out.iter().all(|d| has(d, "undeclared_tool")), "both calls are scanned");
    }

    #[tokio::test]
    async fn no_seen_set_means_no_unseen_host_but_hosts_are_filled() {
        let tenant = r#"@id("h") permit(principal, action, resource) when { context.call.hosts.contains("x.example") };"#;
        let (ctx, turn) = request_side(tenant, TenantMode::Enforce).await;
        let snapshot = Arc::clone(ctx.policy.as_ref().expect("a snapshot"));
        assert!(snapshot.seen_hosts.is_none(), "no request screen ran");
        let turn = Arc::new(turn);
        let calls = || calls_with(&[("ok", r#"{"u":"https://x.example/a"}"#)]);
        let out = detecting(Vec::new()).decide(Arc::new(ctx.clone()), Arc::clone(&turn), calls()).await.expect("decide");
        assert_eq!(out[0].decision, Decision::Allow, "context.call.hosts holds the host");
        assert!(!kinds(&out[0]).iter().any(|(k, _)| k == UNSEEN_HOST), "{:?}", kinds(&out[0]));
        // With a screen that saw another host, the seen set reaches the call subject.
        let mut seen = HostSet::new();
        seen.insert("other.example");
        let screened = Arc::new(PolicySnapshot::clone(&snapshot).with_seen_hosts(SeenHosts(Arc::new(seen))));
        let out = detecting(Vec::new()).decide(Arc::new(ctx.with_policy(screened)), turn, calls()).await.expect("decide");
        assert!(kinds(&out[0]).contains(&(UNSEEN_HOST.to_owned(), UNSEEN_HOST.to_owned())), "{:?}", kinds(&out[0]));
    }

    #[tokio::test(start_paused = true)]
    async fn the_overhead_includes_the_pipeline_time() {
        let (ctx, turn) = request_side(testing::PERMIT_ALL, TenantMode::Enforce).await;
        let snapshot = Arc::clone(ctx.policy.as_ref().expect("a snapshot"));
        let slow: Arc<dyn Detector> = Arc::new(SlowDetector("hook_slow", Duration::from_millis(50)));
        let out = detecting(vec![slow]).decide(Arc::new(ctx), Arc::new(turn), calls_with(&[("ok", "{}")])).await.expect("decide");
        assert_eq!(out[0].decision, Decision::Allow);
        assert!(snapshot.decide_elapsed() >= Duration::from_millis(50), "{:?}", snapshot.decide_elapsed());
    }

    /// A built-in stage that counts its runs.
    struct CountingStage(AtomicUsize, BuiltInDetector);

    impl BuiltInStage for CountingStage {
        fn run(&self, batch: &DetectBatch) -> PipelineOutput {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.1.run(batch)
        }
    }

    #[tokio::test]
    async fn an_unavailable_set_runs_no_pipeline() {
        let stage = Arc::new(CountingStage(AtomicUsize::new(0), BuiltInDetector::new(1 << 24)));
        let decider =
            CedarDecider::new(Arc::new(DecisionRecorder::tracing())).with_detectors(pipeline_with(Arc::clone(&stage) as Arc<dyn BuiltInStage>, Vec::new()), DECIDE_TIMEOUT, detector_settings());
        // No snapshot, and a snapshot with an unavailable set (an unparsed scope).
        let no_snapshot = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = decider
            .decide(Arc::new(no_snapshot), Arc::new(ToolTurn::Unavailable("test")), calls_with(&[("ok", "{}")]))
            .await
            .expect("decide");
        assert_eq!(out[0].decision, BLOCK_UNAVAILABLE);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, "prn:paigasus:iam:default:scope/team-a", None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        assert!(matches!(out.snapshot.set, PolicySet::Unavailable));
        let d = decider
            .decide(Arc::new(ctx.with_policy(out.snapshot)), Arc::new(turn), calls_with(&[("ok", "{}")]))
            .await
            .expect("decide");
        assert_eq!(d[0].decision, BLOCK_UNAVAILABLE);
        assert_eq!(stage.0.load(Ordering::SeqCst), 0, "D27: no pipeline run without an available set");
        // Control: an available set runs the stage once per hold.
        let (ctx, turn) = request_side(testing::PERMIT_ALL, TenantMode::Enforce).await;
        decider.decide(Arc::new(ctx), Arc::new(turn), calls_with(&[("ok", "{}"), ("ok", "{}")])).await.expect("decide");
        assert_eq!(stage.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_webhook_finding_that_names_the_kernel_counts_as_webhook() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (ctx, turn) = request_side(testing::PERMIT_ALL, TenantMode::Enforce).await;
        // A test-only webhook that gives a finding with the detector name `kernel` (a spoof).
        let spoof: Arc<dyn Detector> = Arc::new(FixedDetector("hook_a", vec![detector_error(DETECTOR_KERNEL)]));
        let out = detecting(vec![spoof]).decide(Arc::new(ctx), Arc::new(turn), calls_with(&[("ok", "{}")])).await.expect("decide");
        assert!(kinds(&out[0]).contains(&("detector_error".to_owned(), DETECTOR_KERNEL.to_owned())), "the record keeps the name it gave");
        let m = testing::Snap::take(&snapshotter);
        let finding = |detector: &str| {
            m.counter(
                names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
                &[("side", "response"), ("kind", "detector_error"), ("detector", detector), ("severity", "high")],
            )
        };
        assert_eq!((finding("webhook"), finding("kernel")), (1, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_counts_from_the_hold_start() {
        // D35: the hold completed long ago, so only 100 ms of its decide_timeout (minus the
        // margin) is left. A webhook of 400 ms fits the pipeline timeout (500 ms) but not that.
        let margin = Duration::from_millis(DETECTOR_MARGIN_MS);
        let left = Duration::from_millis(100);
        let slow = || -> Arc<dyn Detector> { Arc::new(SlowDetector("hook_slow", Duration::from_millis(400))) };
        let has_timeout = |d: &DecidedCall| kinds(d).contains(&("detector_error".to_owned(), "hook_slow".to_owned()));

        let (ctx, turn) = request_side(testing::PERMIT_ALL, TenantMode::Enforce).await;
        let snapshot = Arc::clone(ctx.policy.as_ref().expect("a snapshot"));
        let (ctx, turn) = (Arc::new(ctx), Arc::new(turn));
        let hold_start = tokio::time::Instant::now().into_std() - (DECIDE_TIMEOUT - margin - left);
        let out = detecting(vec![slow()])
            .decide_by(Arc::clone(&ctx), Arc::clone(&turn), calls_with(&[("ok", "{}")]), hold_start)
            .await
            .expect("decide");
        assert!(has_timeout(&out[0]), "the webhook is stopped at the hold deadline: {:?}", kinds(&out[0]));
        assert!(snapshot.decide_elapsed() < Duration::from_millis(400), "stopped at about {left:?}: {:?}", snapshot.decide_elapsed());

        // Control: a hold that completed now gets the whole pipeline timeout.
        let hold_start = tokio::time::Instant::now().into_std();
        let out = detecting(vec![slow()]).decide_by(ctx, turn, calls_with(&[("ok", "{}")]), hold_start).await.expect("decide");
        assert!(!has_timeout(&out[0]), "{:?}", kinds(&out[0]));
    }

    #[test]
    fn the_decider_future_is_send() {
        fn send<T: Send>(_: &T) {}
        let fut = detecting(Vec::new()).decide_by(
            Arc::new(DecisionContext::new(PRINCIPAL, ORG_A_PRN, None)),
            Arc::new(ToolTurn::Unavailable("test")),
            Vec::new(),
            Instant::now(),
        );
        send(&fut);
    }

    #[test]
    fn labels_follow_the_table() {
        use paigasus_kernel::detect::Severity;
        let rec = |kind: &str, sev: Option<Severity>, detector: &str| FindingRecord::new(kind, sev, detector, "v1");
        assert_eq!(
            finding_labels(&rec("secret_in_args", Some(Severity::High), "secret_in_args"), None),
            ("secret_in_args", "secret_in_args", "high")
        );
        assert_eq!(
            finding_labels(&rec("detector_error", Some(Severity::High), "slow"), Some(Stage::Webhook)),
            ("detector_error", "webhook", "high")
        );
        assert_eq!(finding_labels(&rec("detector_error", Some(Severity::High), "judge"), None), ("detector_error", "judge", "high"));
        // A kind outside the table, and a severity that the table does not hold for the kind.
        assert_eq!(finding_labels(&rec("custom_x", Some(Severity::Medium), "kernel"), None), ("other", "other", "medium"));
        assert_eq!(finding_labels(&rec("secret_in_args", Some(Severity::Info), "secret_in_args"), None), ("other", "other", "info"));
        // No severity (an old event) counts as high. A host-like detector name never reaches a label.
        assert_eq!(finding_labels(&rec("custom_x", None, "evil.example.com"), None), ("other", "other", "high"));
        for t in FINDING_TRIPLES {
            assert!(FINDING_KINDS.contains(&t.0) && DETECTOR_LABELS.contains(&t.1), "{t:?}");
        }
        assert_eq!(FINDING_TRIPLES.len(), 13);
    }

    #[test]
    fn an_unknown_finding_kind_is_other() {
        assert_eq!(finding_label("zz"), "other");
        for k in FINDING_KINDS {
            assert_eq!(finding_label(k), k);
        }
        assert_eq!(FINDING_KINDS.len(), 9);
        assert_eq!(DETECTOR_LABELS.len(), 10);
    }

    #[tokio::test]
    async fn the_decider_returns_one_decision_per_call() {
        // The permit names the principal type Pgs::Iam::Principal (controller ruling): a wrong
        // principal type would deny "y" and "z" by default.
        let source = FakeSource::new(
            testing::FLOOR,
            r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"x", resource);"#,
        );
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let (_, d) = decide_with(&policy, ORG_A_PRN, &[], &["x", "y", "rm", "z"]).await;
        assert_eq!(d, [BLOCK_POLICY, Decision::Allow, BLOCK_POLICY, Decision::Allow]);
    }

    #[tokio::test]
    async fn the_decider_uses_the_request_snapshot_after_invalidate_all() {
        // Review Focus 3 (D30): a request keeps the snapshot it started with.
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let ctx_v1 = ctx.clone().with_policy(out.snapshot);

        source.set_tenant(r#"@id("p") permit(principal, action, resource); @id("d") forbid(principal, action == Pgs::Gateway::Action::"x", resource);"#);
        policy.cache().invalidate_all();

        // A new request sees v2, so the cache did change.
        let (_, now) = decide_with(&policy, ORG_A_PRN, &[], &["x"]).await;
        assert_eq!(now, [BLOCK_POLICY]);
        // The first request still decides with v1.
        assert_eq!(decide(ctx_v1, turn, &["x"]).await, [Decision::Allow]);
    }

    #[tokio::test]
    async fn tenant_off_still_evaluates_the_floor() {
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Off));
        let (snap, d) = decide_with(&policy, ORG_A_PRN, &[], &["rm", "ok"]).await;
        assert_eq!(d, [BLOCK_POLICY, Decision::Allow]);
        assert!(matches!(snap.set, PolicySet::Available { tenant: TenantPart::Off, .. }));
        assert_eq!(source.tenant_calls(), 0, "mode off does not load the tenant set");
    }

    #[tokio::test]
    async fn a_team_or_project_scope_uses_its_org_settings() {
        let source = FakeSource::new(
            testing::FLOOR,
            r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"x", resource);"#,
        );
        // The default mode is off; only ORG_A is in enforce.
        let mut s = settings(TenantMode::Off);
        s.orgs.insert(ORG_A, (TenantMode::Enforce, None));
        let policy = testing::policy(&source, s);
        for scope in [TEAM_IN_A, testing::PROJECT_IN_A] {
            let (snap, d) = decide_with(&policy, scope, &[], &["x", "y"]).await;
            assert_eq!(snap.mode, TenantMode::Enforce, "{scope}");
            assert_eq!(d, [BLOCK_POLICY, Decision::Allow], "{scope}");
        }
    }

    #[tokio::test]
    async fn an_unparsed_scope_is_unavailable() {
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let (snap, d) = decide_with(&policy, "prn:paigasus:iam:default:scope/team-a", &[], &["ok"]).await;
        assert!(matches!(snap.set, PolicySet::Unavailable));
        assert_eq!(d, [BLOCK_UNAVAILABLE]);
        // An unparsed principal is the same case (P-6).
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new("not-a-prn", ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        assert!(matches!(out.snapshot.set, PolicySet::Unavailable));
        assert_eq!(decide(ctx.with_policy(out.snapshot), turn, &["ok"]).await, [BLOCK_UNAVAILABLE]);
    }

    #[tokio::test]
    async fn an_empty_tenant_set_denies_every_non_floor_call() {
        // Spec D22 and § 6.1.5: one evaluation on the merged set, and Cedar denies by default. A
        // tenant with no directory has an empty set (§ 6.3.2), so it permits nothing.
        let source = FakeSource::new(testing::FLOOR, "");
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let (snap, d) = decide_with(&policy, ORG_A_PRN, &[], &["ok", "rm"]).await;
        assert!(matches!(snap.set, PolicySet::Available { tenant: TenantPart::Evaluated, .. }));
        assert_eq!(d, [BLOCK_POLICY, BLOCK_POLICY]);
        let policy = testing::policy(&source, settings(TenantMode::LogOnly));
        let (_, d) = decide_with(&policy, ORG_A_PRN, &[], &["ok", "rm"]).await;
        assert_eq!(d, [Decision::Observe { would: BlockCode::Policy }, BLOCK_POLICY]);
    }

    #[tokio::test]
    async fn no_snapshot_blocks_every_call() {
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        assert_eq!(decide(ctx, ToolTurn::Unavailable("test"), &["a", "b"]).await, [BLOCK_UNAVAILABLE, BLOCK_UNAVAILABLE]);
    }

    #[tokio::test]
    async fn the_decider_counts_each_call_and_adds_its_time_to_the_snapshot() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::LogOnly));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        // A fixed request-side time, so the sample must be at least that.
        let snap = Arc::new(PolicySnapshot::clone(&out.snapshot).with_request_elapsed(Duration::from_secs(5)));
        let d = decide(ctx.with_policy(Arc::clone(&snap)), turn, &["rm", "ok", "ok"]).await;
        assert_eq!(d, [BLOCK_POLICY, Decision::Allow, Decision::Allow]);
        let m = testing::Snap::take(&snapshotter);
        let dec = |effect: &str| {
            m.counter(
                names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
                &[("side", "response"), ("mode", "log_only"), ("effect", effect), ("basis", "policy")],
            )
        };
        assert_eq!((dec("block"), dec("allow")), (1, 2));
        assert!(
            m.histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).is_empty(),
            "the decider records no sample itself (preflight F14)"
        );
        assert!(snap.decide_elapsed() > Duration::ZERO, "the decide time is in the snapshot");
        assert!(snap.overhead() >= Duration::from_secs(5), "the overhead holds the request-side time");
    }
}
