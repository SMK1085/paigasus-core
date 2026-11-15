// SPDX-License-Identifier: Apache-2.0

//! The Cedar decider (SMA-752 § 6.3.4). It decides the calls of one hold from the request's policy
//! snapshot (D30): it never reads the cache. Evaluation is synchronous inside the future, and the
//! decider spawns nothing (SMA-749 D25).
//!
//! Logs and metrics hold the sanitized tool name, the verdict and the policy ids. They never hold
//! the arguments (ADR-0027 (i)).

use std::sync::Arc;
use std::time::Instant;

use metrics::{counter, histogram};
use paigasus_gateway_core::{DecideFuture, Decision, DecisionContext, OnUnavailable, PolicySet, PolicySnapshot, TenantMode, ToolCall, ToolCallDecider, ToolTurn, decide_calls, sanitize_name};
use paigasus_kernel::tool_policy::{PolicyOrigin, PolicyRef};
use paigasus_observability::names;

/// The `ToolCallDecider` that evaluates the tool policies.
#[derive(Debug, Clone, Copy, Default)]
pub struct CedarDecider;

/// The `effect` metric label of a decision.
pub(crate) fn effect_label(d: Decision) -> &'static str {
    match d {
        Decision::Allow => "allow",
        Decision::Observe { .. } => "observe",
        // `Block`, and any later variant: count it as the stricter effect.
        _ => "block",
    }
}

/// The policy ids for a log field: `<set>:<id>`, comma-separated, every id sanitized.
fn refs(list: &[PolicyRef]) -> String {
    list.iter()
        .map(|r| {
            let set = match r.set {
                PolicyOrigin::Floor => "floor",
                _ => "tenant",
            };
            format!("{set}:{}", sanitize_name(&r.id))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Decide the calls from the snapshot of `ctx`. `None` is an unavailable set under `closed`: the
/// tool policy is on, but the request side did not run, so every call is blocked (fail closed).
fn decide_now(ctx: &DecisionContext, turn: &ToolTurn, calls: &[ToolCall]) -> Vec<Decision> {
    let started = Instant::now();
    let fallback;
    let snapshot = match ctx.policy.as_deref() {
        Some(s) => s,
        None => {
            fallback = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
            &fallback
        }
    };
    let outcomes = decide_calls(snapshot, turn, calls);
    let mode = snapshot.mode.as_str();
    let request_id = ctx.request_id.as_deref().unwrap_or("");
    for (call, o) in calls.iter().zip(&outcomes) {
        let effect = effect_label(o.decision);
        let basis = o.basis.as_str();
        counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "response", "mode" => mode, "effect" => effect, "basis" => basis).increment(1);
        let (determining, errored) = o.verdict.as_ref().map_or_else(|| (String::new(), String::new()), |v| (refs(&v.determining), refs(&v.errored)));
        tracing::info!(
            request_id,
            tool = %sanitize_name(&call.display_name()),
            verdict = effect,
            basis,
            mode,
            determining = %determining,
            errored = %errored,
            "tool-policy decision"
        );
    }
    // One sample per request: the request side plus this evaluation (preflight F14).
    histogram!(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).record((snapshot.request_elapsed + started.elapsed()).as_secs_f64());
    outcomes.into_iter().map(|o| o.decision).collect()
}

impl ToolCallDecider for CedarDecider {
    fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        Box::pin(std::future::ready(Ok(decide_now(&ctx, &turn, &calls))))
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

    async fn decide(ctx: DecisionContext, turn: ToolTurn, call_names: &[&str]) -> Vec<Decision> {
        let calls = calls(call_names);
        let n = calls.len();
        let out = CedarDecider.decide(Arc::new(ctx), Arc::new(turn), calls).await.expect("decide");
        assert_eq!(out.len(), n, "one decision per call");
        out
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
    async fn the_decider_counts_each_call_and_records_one_overhead_sample() {
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
        let d = decide(ctx.with_policy(snap), turn, &["rm", "ok", "ok"]).await;
        assert_eq!(d, [BLOCK_POLICY, Decision::Allow, Decision::Allow]);
        let m = testing::Snap::take(&snapshotter);
        let dec = |effect: &str| {
            m.counter(
                names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
                &[("side", "response"), ("mode", "log_only"), ("effect", effect), ("basis", "policy")],
            )
        };
        assert_eq!((dec("block"), dec("allow")), (1, 2));
        let samples = m.histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS);
        assert_eq!(samples.len(), 1, "one sample per decide");
        assert!(samples[0] >= 5.0, "the sample holds the request-side time");
    }

    #[test]
    fn policy_ids_are_sanitized_in_logs() {
        let r = [PolicyRef::new(PolicyOrigin::Floor, "f 1"), PolicyRef::new(PolicyOrigin::Tenant, "t\n2")];
        assert_eq!(refs(&r), "floor:f_1,tenant:t_2");
    }
}
