// SPDX-License-Identifier: Apache-2.0

//! The Cedar decider (SMA-752 § 6.3.4). It decides the calls of one hold from the request's policy
//! snapshot (D30): it never reads the cache. Evaluation is synchronous inside the future, and the
//! decider spawns nothing (SMA-749 D25).
//!
//! The decider writes no log line. It counts the decision metrics and computes the keyed argument digest for
//! the decision log. The recorder writes the per-decision line (SMA-755). Metrics never hold the arguments
//! (ADR-0027 (i)).

use std::sync::Arc;
use std::time::Instant;

use metrics::counter;
use paigasus_gateway_core::decision_log::{DecisionDetail, FindingRecord, RecordBasis};
use paigasus_gateway_core::{DecideFuture, DecidedCall, Decision, DecisionContext, OnUnavailable, PolicySet, PolicySnapshot, TenantMode, ToolCall, ToolCallDecider, ToolTurn, decide_calls};
use paigasus_observability::names;

use crate::application::decision_log::DecisionRecorder;

/// The `ToolCallDecider` that evaluates the tool policies. It holds the one shared recorder, for
/// the argument digest (SMA-755).
pub struct CedarDecider {
    recorder: Arc<DecisionRecorder>,
}

impl CedarDecider {
    #[must_use]
    pub fn new(recorder: Arc<DecisionRecorder>) -> Self {
        Self { recorder }
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

/// The `(kind, detector, severity)` labels of a finding. `webhook` says that the finding came from a
/// configured webhook, so the name of the detector is not a label (A2). A finding outside
/// [`FINDING_TRIPLES`] is `(other, other, <severity>)`. No severity counts as `high`.
#[must_use]
pub fn finding_labels(f: &FindingRecord, webhook: bool) -> (&'static str, &'static str, &'static str) {
    let severity = f.severity.map_or("high", |s| s.as_str());
    let severity = SEVERITY_LABELS.iter().find(|s| **s == severity).copied().unwrap_or("high");
    let detector = if webhook {
        "webhook"
    } else {
        DETECTOR_LABELS.iter().find(|d| **d == f.detector).copied().unwrap_or("other")
    };
    FINDING_TRIPLES
        .iter()
        .find(|(k, d, s)| *k == f.kind && *d == detector && *s == severity)
        .copied()
        .unwrap_or(("other", "other", severity))
}

/// Increment `gateway_tool_policy_findings_total` for a finding.
pub(crate) fn count_finding(side: &'static str, f: &FindingRecord, webhook: bool) {
    let (kind, detector, severity) = finding_labels(f, webhook);
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

/// Decide the calls from the snapshot of `ctx`. `None` is an unavailable set under `closed`: the
/// tool policy is on, but the request side did not run, so every call is blocked (fail closed).
fn decide_now(recorder: &DecisionRecorder, ctx: &DecisionContext, turn: &ToolTurn, calls: &[ToolCall]) -> Vec<DecidedCall> {
    let started = Instant::now();
    let fallback;
    let snapshot = match ctx.policy.as_deref() {
        Some(s) => s,
        None => {
            fallback = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
            &fallback
        }
    };
    // No detector scan runs here yet, so the kernel reads the hosts of each call itself.
    let outcomes = decide_calls(snapshot, turn, calls, &[], &Instant::now);
    let mode = snapshot.mode.as_str();
    // The digest is computed inside the timed region, so it is part of the overhead sample (§ 5.3).
    let digests: Vec<_> = calls.iter().map(|c| recorder.digest(&c.arguments)).collect();
    for o in &outcomes {
        let effect = effect_label(o.decision);
        let basis = o.basis.as_str();
        counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "response", "mode" => mode, "effect" => effect, "basis" => basis).increment(1);
        if let Some(c) = &o.cedar {
            for f in &c.findings {
                // No pipeline reaches the decider yet, so no finding is a webhook finding here.
                count_finding("response", f, false);
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
        Box::pin(std::future::ready(Ok(decide_now(&self.recorder, &ctx, &turn, &calls))))
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

    #[test]
    fn labels_follow_the_table() {
        use paigasus_kernel::detect::Severity;
        let rec = |kind: &str, sev: Option<Severity>, detector: &str| FindingRecord::new(kind, sev, detector, "v1");
        assert_eq!(
            finding_labels(&rec("secret_in_args", Some(Severity::High), "secret_in_args"), false),
            ("secret_in_args", "secret_in_args", "high")
        );
        assert_eq!(finding_labels(&rec("detector_error", Some(Severity::High), "slow"), true), ("detector_error", "webhook", "high"));
        assert_eq!(finding_labels(&rec("detector_error", Some(Severity::High), "judge"), false), ("detector_error", "judge", "high"));
        // A kind outside the table, and a severity that the table does not hold for the kind.
        assert_eq!(finding_labels(&rec("custom_x", Some(Severity::Medium), "kernel"), false), ("other", "other", "medium"));
        assert_eq!(finding_labels(&rec("secret_in_args", Some(Severity::Info), "secret_in_args"), false), ("other", "other", "info"));
        // No severity (an old event) counts as high. A host-like detector name never reaches a label.
        assert_eq!(finding_labels(&rec("custom_x", None, "evil.example.com"), false), ("other", "other", "high"));
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
