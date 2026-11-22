// SPDX-License-Identifier: Apache-2.0

//! The Cedar decider (SMA-752 § 6.3.4). It decides the calls of one hold from the request's policy
//! snapshot (D30): it never reads the cache. Evaluation is synchronous inside the future, and the
//! decider spawns nothing (SMA-749 D25).
//!
//! SMA-753: the decider mints the decision ids first. With a history and a session, the future then
//! reserves one pending event per call under those ids (spec D8), and the decision reads the session
//! findings and the reserve findings. A dropped future drops the reserve.
//!
//! The decider writes no log line. It counts the decision metrics and computes the keyed argument digest for
//! the decision log. The recorder writes the per-decision line (SMA-755). Metrics never hold the arguments
//! (ADR-0027 (i)).

use std::sync::Arc;
use std::time::Instant;

use metrics::counter;
use paigasus_gateway_core::decision_log::{DecisionDetail, RecordBasis};
use paigasus_gateway_core::{DecideFuture, DecidedCall, Decision, DecisionContext, OnUnavailable, PolicySet, PolicySnapshot, TenantMode, ToolCall, ToolCallDecider, ToolTurn, decide_calls_with};
use paigasus_kernel::tool_policy::{FORGED_HISTORY, Findings, HISTORY_TRUNCATED, MIXED_SESSIONS, NAME_COLLISION, UNPARSED_ARGUMENTS};
use paigasus_observability::names;
use uuid::Uuid;

use crate::application::decision_log::DecisionRecorder;
use crate::application::tool_history::ToolHistory;

/// The `ToolCallDecider` that evaluates the tool policies. It holds the one shared recorder, for
/// the argument digest (SMA-755).
pub struct CedarDecider {
    recorder: Arc<DecisionRecorder>,
    /// The tool history (SMA-753). `None`: the decider reserves nothing.
    history: Option<Arc<ToolHistory>>,
}

impl CedarDecider {
    #[must_use]
    pub fn new(recorder: Arc<DecisionRecorder>) -> Self {
        Self { recorder, history: None }
    }

    /// Reserve the calls of each hold in `history` before the decision (SMA-753, spec D8).
    #[must_use]
    pub fn with_history(mut self, history: Arc<ToolHistory>) -> Self {
        self.history = Some(history);
        self
    }

    /// The recorder of this decider. A runtime test reads it to check that the decider, the request side and
    /// the report site share one recorder (P-8).
    #[must_use]
    pub fn recorder(&self) -> &Arc<DecisionRecorder> {
        &self.recorder
    }
}

/// The finding kinds that `gateway_tool_policy_findings_total` labels (SMA-755 § 5.9).
pub const FINDING_KINDS: [&str; 6] = ["name_collision", "unparsed_arguments", "mixed_sessions", "forged_history", "history_truncated", "other"];

/// The `kind` metric label of a finding. An unknown kind is `other`.
#[must_use]
pub fn finding_label(kind: &str) -> &'static str {
    match kind {
        NAME_COLLISION => FINDING_KINDS[0],
        UNPARSED_ARGUMENTS => FINDING_KINDS[1],
        MIXED_SESSIONS => FINDING_KINDS[2],
        FORGED_HISTORY => FINDING_KINDS[3],
        HISTORY_TRUNCATED => FINDING_KINDS[4],
        _ => FINDING_KINDS[5],
    }
}

/// Prime `gateway_tool_policy_findings_total` at zero for every label pair.
pub fn prime_tool_policy_metrics() {
    for side in ["request", "response"] {
        for kind in FINDING_KINDS {
            counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => side, "kind" => kind).absolute(0);
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
///
/// `extra` holds the request-level findings that every call carries (SMA-753). `ids` holds one
/// decision id per call, minted before the reserve.
fn decide_now(ctx: &DecisionContext, turn: &ToolTurn, calls: &[ToolCall], extra: &Findings, ids: Vec<Uuid>, recorder: &DecisionRecorder) -> Vec<DecidedCall> {
    let started = Instant::now();
    let fallback;
    let snapshot = match ctx.policy.as_deref() {
        Some(s) => s,
        None => {
            fallback = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
            &fallback
        }
    };
    let outcomes = decide_calls_with(snapshot, turn, calls, extra, &Instant::now);
    let mode = snapshot.mode.as_str();
    // The digest is computed inside the timed region, so it is part of the overhead sample (§ 5.3).
    let digests: Vec<_> = calls.iter().map(|c| recorder.digest(&c.arguments)).collect();
    for o in &outcomes {
        let effect = effect_label(o.decision);
        let basis = o.basis.as_str();
        counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "response", "mode" => mode, "effect" => effect, "basis" => basis).increment(1);
        if let Some(c) = &o.cedar {
            for f in &c.findings {
                counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => "response", "kind" => finding_label(f)).increment(1);
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
        .zip(ids)
        .map(|((o, digest), id)| DecidedCall::new(o.decision, Some(Arc::new(DecisionDetail::new(RecordBasis::from(o.basis), o.cedar, digest)))).with_decision_id(id))
        .collect()
}

impl ToolCallDecider for CedarDecider {
    fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        // The ids come first (spec D1): the reserve writes its events under them.
        let ids: Vec<Uuid> = calls.iter().map(|_| self.recorder.mint_decision_id()).collect();
        let recorder = Arc::clone(&self.recorder);
        let history = self.history.clone();
        // The future spawns nothing (SMA-749 D25). A store error is counted inside `reserve` and
        // changes no decision (PR 1).
        Box::pin(async move {
            let mut extra = Findings::new();
            if let Some(session) = ctx.session.as_deref() {
                for kind in session.findings.iter() {
                    extra.insert(kind);
                }
                if let Some(history) = &history {
                    // The reserve round trip is part of the overhead sample (spec § 9). Its region
                    // ends before the timer of `decide_now` starts, so nothing counts twice.
                    let started = Instant::now();
                    let found = history.reserve(&ctx, session, &calls, &ids).await;
                    if let Some(s) = ctx.policy.as_deref() {
                        s.add_decide_elapsed(started.elapsed());
                    }
                    for kind in found.iter() {
                        extra.insert(kind);
                    }
                }
            }
            Ok(decide_now(&ctx, &turn, &calls, &extra, ids, &recorder))
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
    async fn the_decider_gives_every_call_its_own_decision_id() {
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let decided = decide_full(ctx.with_policy(out.snapshot), turn, &["x", "y", "rm", "z"]).await;
        let ids: Vec<_> = decided.iter().map(|d| d.decision_id.expect("an id for every call")).collect();
        assert!(ids.iter().all(|id| id.get_version_num() == 7));
        let distinct: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(distinct.len(), 4, "four distinct ids: {ids:?}");
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
        let finding = |side: &str, kind: &str| m.counter(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &[("side", side), ("kind", kind)]);
        assert_eq!(
            (
                finding("response", "unparsed_arguments"),
                finding("response", "name_collision"),
                finding("request", "unparsed_arguments")
            ),
            (1, 0, 0)
        );
    }

    #[test]
    fn an_unknown_finding_kind_is_other() {
        assert_eq!(finding_label("zz"), "other");
        for k in ["name_collision", "unparsed_arguments", "mixed_sessions", "forged_history", "history_truncated"] {
            assert_eq!(finding_label(k), k);
        }
        assert_eq!(
            FINDING_KINDS,
            ["name_collision", "unparsed_arguments", "mixed_sessions", "forged_history", "history_truncated", "other"]
        );
    }

    #[test]
    fn each_history_finding_has_its_own_label() {
        assert_eq!(finding_label(MIXED_SESSIONS), "mixed_sessions");
        assert_eq!(finding_label(FORGED_HISTORY), "forged_history");
        assert_eq!(finding_label(HISTORY_TRUNCATED), "history_truncated");
        assert_eq!(finding_label("something_new"), "other");
        assert_eq!(FINDING_KINDS.len(), 6);
        assert_eq!(FINDING_KINDS[5], "other");
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

    // ---- the reserve (SMA-753) -------------------------------------------------------------------

    use paigasus_gateway_core::{SessionContext, StoreBudget};

    use crate::adapters::history::memory::MemorySessionStore;
    use crate::application::tool_history::ToolHistory;
    use crate::domain::decision_log::OsEntropy;
    use crate::domain::limits::{SystemClock, UnavailableKind};
    use crate::domain::tool_history::{
        AppendReply, AppendRequest, EventKind, EventVerdict, HistoryStoreError, LookupReply, LookupRequest, ReserveRequest, Scope, SessionStore, SettleRequest, WriteRequest,
    };

    /// A store whose every call fails.
    struct Down;

    fn down() -> HistoryStoreError {
        HistoryStoreError::Unavailable {
            kind: UnavailableKind::Io,
            detail: "down".into(),
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for Down {
        async fn lookup(&self, _: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
            Err(down())
        }
        async fn write(&self, _: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
            Err(down())
        }
        async fn reserve(&self, _: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
            Err(down())
        }
        async fn settle(&self, _: &SettleRequest) -> Result<(), HistoryStoreError> {
            Err(down())
        }
        async fn append(&self, _: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
            Err(down())
        }
    }

    fn history_over(store: Arc<dyn SessionStore>) -> Arc<ToolHistory> {
        Arc::new(ToolHistory::new(store, Arc::new(SystemClock), Arc::new(OsEntropy), Duration::from_secs(1)))
    }

    fn session(findings: Findings) -> Arc<SessionContext> {
        Arc::new(SessionContext::new(
            ORG_A,
            uuid::Uuid::from_u128(0x0190a1f0_0000_7000_8000_000000000005),
            1,
            findings,
            StoreBudget::new(Duration::from_secs(1)),
        ))
    }

    /// The context of a request under `tenant`, in enforce mode.
    async fn ctx_under(tenant: &str) -> (DecisionContext, ToolTurn) {
        let source = FakeSource::new(testing::FLOOR, tenant);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        (ctx.with_policy(out.snapshot), turn)
    }

    async fn decide_by(decider: &CedarDecider, ctx: DecisionContext, turn: ToolTurn, call_names: &[&str]) -> Vec<DecidedCall> {
        decider.decide(Arc::new(ctx), Arc::new(turn), calls(call_names)).await.expect("decide")
    }

    #[tokio::test]
    async fn the_reserve_runs_before_the_decision_and_uses_the_decision_ids() {
        let store = Arc::new(MemorySessionStore::new());
        let decider = CedarDecider::new(Arc::new(DecisionRecorder::tracing())).with_history(history_over(store.clone()));
        let s = session(Findings::new());
        let (ctx, turn) = ctx_under(testing::PERMIT_ALL).await;
        let decided = decide_by(&decider, ctx.with_session(Arc::clone(&s)), turn, &["ok", "rm"]).await;
        assert_eq!(decided.iter().map(|d| d.decision).collect::<Vec<_>>(), [Decision::Allow, BLOCK_POLICY]);
        for (tool, d) in ["ok", "rm"].into_iter().zip(&decided) {
            let events = store.events(ORG_A, Scope::Session, &s.session_id.to_string(), tool);
            assert_eq!(events.len(), 1, "one pending event for {tool}: {events:?}");
            assert_eq!((events[0].kind, events[0].verdict), (EventKind::Call, EventVerdict::Pending), "{tool}");
            assert_eq!(Some(events[0].id), d.decision_id, "the event id is the decision id of {tool}");
        }
    }

    #[tokio::test]
    async fn a_session_finding_reaches_the_cedar_context() {
        const TENANT: &str = r#"
            @id("p") permit(principal, action, resource);
            @id("mixed") forbid(principal, action, resource) when { context.findings.contains("mixed_sessions") };
        "#;
        let decider = CedarDecider::new(Arc::new(DecisionRecorder::tracing()));
        let mut mixed = Findings::new();
        mixed.insert(MIXED_SESSIONS);
        let (ctx, turn) = ctx_under(TENANT).await;
        let blocked = decide_by(&decider, ctx.clone().with_session(session(mixed)), turn.clone(), &["ok"]).await;
        assert_eq!(blocked[0].decision, BLOCK_POLICY);
        let allowed = decide_by(&decider, ctx.with_session(session(Findings::new())), turn, &["ok"]).await;
        assert_eq!(allowed[0].decision, Decision::Allow, "the control: no finding, no block");
    }

    #[tokio::test]
    async fn a_reserve_error_still_decides() {
        let failing = CedarDecider::new(Arc::new(DecisionRecorder::tracing())).with_history(history_over(Arc::new(Down)));
        let plain = CedarDecider::new(Arc::new(DecisionRecorder::tracing()));
        let names = ["ok", "rm", "x"];
        let (ctx, turn) = ctx_under(testing::PERMIT_ALL).await;
        let s = session(Findings::new());
        let with_error = decide_by(&failing, ctx.clone().with_session(Arc::clone(&s)), turn.clone(), &names).await;
        let without = decide_by(&plain, ctx, turn, &names).await;
        let effects = |d: &[DecidedCall]| d.iter().map(|c| c.decision).collect::<Vec<_>>();
        assert_eq!(effects(&with_error), effects(&without));
        assert!(with_error.iter().all(|d| d.decision_id.is_some()), "every call keeps its id");
        assert!(s.budget.is_failed(), "the reserve did run and fail");
    }

    /// The memory store, with a reserve that first waits `.1`.
    struct SlowReserve(MemorySessionStore, Duration);

    #[async_trait::async_trait]
    impl SessionStore for SlowReserve {
        async fn lookup(&self, req: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
            self.0.lookup(req).await
        }
        async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
            self.0.write(req).await
        }
        async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
            tokio::time::sleep(self.1).await;
            self.0.reserve(req).await
        }
        async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError> {
            self.0.settle(req).await
        }
        async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
            self.0.append(req).await
        }
    }

    /// Spec § 9: the overhead sample holds the reserve round trip.
    #[tokio::test]
    async fn the_reserve_time_is_in_the_overhead_sample() {
        const WAIT: Duration = Duration::from_millis(80);
        let decider = CedarDecider::new(Arc::new(DecisionRecorder::tracing())).with_history(history_over(Arc::new(SlowReserve(MemorySessionStore::new(), WAIT))));
        let (ctx, turn) = ctx_under(testing::PERMIT_ALL).await;
        let snap = Arc::clone(ctx.policy.as_ref().expect("a snapshot"));
        let decided = decide_by(&decider, ctx.with_session(session(Findings::new())), turn, &["ok"]).await;
        assert_eq!(decided[0].decision, Decision::Allow);
        assert!(snap.decide_elapsed() >= WAIT, "the reserve wait is in the decide time: {:?}", snap.decide_elapsed());
    }
}
