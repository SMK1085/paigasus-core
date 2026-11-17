// SPDX-License-Identifier: Apache-2.0

//! The request side of the tool policy (SMA-752 § 6.5, D19): build the request's policy snapshot,
//! plan the request-side actions, refuse (D17), rewrite in enforce mode, and report.
//!
//! The snapshot goes into `DecisionContext` (D30), so the decider uses the same policies as the
//! request side. Logs and metrics hold no arguments, results or prompts (ADR-0027 (i)).

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use metrics::{counter, histogram};
use paigasus_gateway_core::{
    DecisionContext, Format, OnUnavailable, PolicySet, PolicySnapshot, RefusalCause, RequestAction, RequestEntry, TenantMode, TenantPart, ToolTurn, ingress, plan_request, rewrite_request,
};
use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies};
use paigasus_kernel::{Prn, to_cedar_uid};
use paigasus_observability::names;
use uuid::Uuid;

use super::cache::{Lookup, PolicyCache};
use crate::config::ToolPolicySettings;
use crate::domain::org_of;

/// Why the request side refuses a request. Each refusal is a 400 (P-10, SMA-776).
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The turn is `ToolTurn::Unavailable` and the tenant mode is `enforce` (D17). Also used when
    /// an enforced plan cannot be applied to the body (controller ruling, Task 10).
    UnreadableTurn,
    /// The `tool_choice` has a shape the gateway does not know, and an enforced removal exists
    /// (§ 6.5.1 step 4).
    UnknownToolChoice,
    /// An enforced block of a result that the gateway cannot replace with a notice (SMA-776).
    UnreplaceableResult,
}

impl Refusal {
    /// The refusal for a plan cause. A cause that this service does not know fails closed (§ 6.4).
    fn from_cause(cause: RefusalCause) -> Self {
        match cause {
            RefusalCause::UnreplaceableResult => Refusal::UnreplaceableResult,
            RefusalCause::UnknownToolChoice => Refusal::UnknownToolChoice,
            _ => Refusal::UnreadableTurn,
        }
    }

    /// The `reason` metric label.
    fn label(&self) -> &'static str {
        match self {
            Refusal::UnreadableTurn => "unreadable_turn",
            Refusal::UnknownToolChoice => "unknown_tool_choice",
            Refusal::UnreplaceableResult => "unreplaceable_result",
        }
    }
}

/// What the request side gives back: the body to send, the request-side report entries, and the
/// snapshot for `DecisionContext` (D30).
#[derive(Debug)]
pub struct RequestOutcome {
    pub body: Bytes,
    pub entries: Vec<RequestEntry>,
    pub snapshot: Arc<PolicySnapshot>,
}

/// The request-side tool-policy service (D19).
pub struct RequestToolPolicy {
    cache: Arc<PolicyCache>,
    settings: ToolPolicySettings,
}

/// Count and log a refusal, or a refusal that enforce mode would make (P-10, D17: log-only is
/// "forwarded, logged and counted"). `refused` says whether the request is refused (400) or
/// forwarded. `cause` names the branch (`unreadable_turn`, `unknown_tool_choice`,
/// `rewrite_failed`). The line holds the request id, the mode and the reason only: no body
/// content and no tool name (ADR-0027 (i)).
fn refusal_event(ctx: &DecisionContext, mode: TenantMode, refusal: &Refusal, cause: &'static str, refused: bool) {
    counter!(names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL, "mode" => mode.as_str(), "reason" => refusal.label(), "refused" => if refused { "true" } else { "false" }).increment(1);
    tracing::warn!(
        request_id = ctx.request_id.as_deref().unwrap_or("-"),
        mode = mode.as_str(),
        reason = refusal.label(),
        cause,
        refused,
        "tool-policy request refusal"
    );
}

/// One overhead sample per request (preflight F14, spec G8). The gateway holds this for the life of
/// the request: to the end of the handler for a full answer, and inside the response body for a
/// stream. On drop it records the request-side time plus every decide call of the request, so a
/// stream with many holds and a request with no call both record exactly one sample.
#[derive(Debug)]
pub struct OverheadSample(Arc<PolicySnapshot>);

impl OverheadSample {
    #[must_use]
    pub fn new(snapshot: Arc<PolicySnapshot>) -> Self {
        OverheadSample(snapshot)
    }
}

impl Drop for OverheadSample {
    fn drop(&mut self) {
        histogram!(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).record(self.0.overhead().as_secs_f64());
    }
}

impl RequestToolPolicy {
    /// `settings` must come from a config that passed `GatewayConfig::validate`.
    #[must_use]
    pub fn new(cache: Arc<PolicyCache>, settings: ToolPolicySettings) -> Self {
        RequestToolPolicy { cache, settings }
    }

    #[cfg(test)]
    pub(crate) fn cache(&self) -> &Arc<PolicyCache> {
        &self.cache
    }

    /// The policies of a request (§ 6.4). The whole lookup waits at most `load_timeout` (D31).
    async fn policy_set(&self, ctx: &DecisionContext, org: Option<Uuid>, mode: TenantMode, on: OnUnavailable, model: &str, format: Format) -> PolicySet {
        // P-6: a PRN that does not parse is the same case as an unavailable floor. A scope that
        // parses but names no org has no resource to evaluate against, so it is unavailable too.
        let (Some(org), Ok(principal)) = (org, Prn::parse(&ctx.principal)) else {
            return PolicySet::Unavailable;
        };
        let deadline = tokio::time::Instant::now() + self.settings.load_timeout;
        // The cache ends each wait at `deadline` and then runs its `open` fallback, so no outer
        // timeout is needed: an outer one would drop that fallback (CodeRabbit, PR 475).
        let Lookup::Ready(floor) = self.cache.floor_until(on, deadline).await else {
            return PolicySet::Unavailable;
        };
        let (policies, tenant): (Arc<ToolPolicies>, TenantPart) = if mode == TenantMode::Off {
            // The tenant set is not consulted in mode off (P-4).
            (Arc::clone(&floor.floor_only), TenantPart::Off)
        } else {
            match self.cache.tenant_until(org, &floor, on, deadline).await {
                Lookup::Ready(t) => (t, TenantPart::Evaluated),
                Lookup::Unavailable => (Arc::clone(&floor.floor_only), TenantPart::Unavailable),
            }
        };
        let facts = Facts::new(to_cedar_uid(&principal), org, RequestInfo::new(ctx.scope.clone(), ingress(format), model), SessionInfo::placeholder());
        PolicySet::Available { policies, tenant, facts }
    }

    /// Apply the tool policy to one request (§ 6.5). `turn` must be `parse_request(format, &body)`:
    /// the plan is made from the same bytes that the rewrite edits (controller ruling, Task 10).
    ///
    /// # Errors
    ///
    /// A [`Refusal`] when the request must be refused with a 400 (D17, § 6.5.1 step 4). A refused
    /// request records its overhead sample here: it has no snapshot to hold (preflight F14).
    pub async fn apply(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes) -> Result<RequestOutcome, Refusal> {
        let started = Instant::now();
        let out = self.apply_inner(ctx, model, format, turn, body, started).await;
        if out.is_err() {
            histogram!(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).record(started.elapsed().as_secs_f64());
        }
        out
    }

    async fn apply_inner(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes, started: Instant) -> Result<RequestOutcome, Refusal> {
        let org = org_of(&ctx.scope);
        // D7, D34: the settings of the scope's org; without an org, the global values.
        let (mode, on) = org.map_or((self.settings.default_mode, self.settings.on_policy_unavailable), |o| self.settings.tenant(o));

        // D17, P-10: an unreadable turn is refused only in enforce mode; log-only counts it.
        if !matches!(turn, ToolTurn::Parsed(_)) && mode != TenantMode::Off {
            refusal_event(ctx, mode, &Refusal::UnreadableTurn, "unreadable_turn", mode == TenantMode::Enforce);
            if mode == TenantMode::Enforce {
                return Err(Refusal::UnreadableTurn);
            }
        }

        let set = self.policy_set(ctx, org, mode, on, model, format).await;
        let snapshot = PolicySnapshot::new(set, mode, on);

        let (body, entries) = if let ToolTurn::Parsed(facts) = turn {
            let plan = plan_request(&snapshot, format, facts);
            if let Some(cause) = plan.refusal() {
                let refusal = Refusal::from_cause(cause);
                refusal_event(ctx, mode, &refusal, refusal.label(), true);
                return Err(refusal);
            }
            // No cause is enforced: report each cause that the mode did not enforce (G5).
            for cause in plan.refusals.keys() {
                let refusal = Refusal::from_cause(*cause);
                refusal_event(ctx, mode, &refusal, refusal.label(), false);
            }
            let body = if plan.is_noop() {
                body
            } else if let Some(rewritten) = rewrite_request(format, &body, &plan) {
                rewritten
            } else {
                // Never send the original bytes for an enforced plan (controller ruling, Task 10).
                refusal_event(ctx, mode, &Refusal::UnreadableTurn, "rewrite_failed", true);
                return Err(Refusal::UnreadableTurn);
            };
            (body, plan.entries)
        } else {
            (body, Vec::new())
        };

        for e in entries.iter().filter(|e| e.action != RequestAction::ResetToolChoice) {
            let effect = if e.enforced { "block" } else { "observe" };
            counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "request", "mode" => mode.as_str(), "effect" => effect, "basis" => "policy").increment(1);
        }

        let snapshot = Arc::new(snapshot.with_request_elapsed(started.elapsed()));
        Ok(RequestOutcome { body, entries, snapshot })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_gateway_core::{RequestReason, parse_request};

    use super::*;
    use crate::application::tool_policy::testing::{self, FakeSource, ORG_A_PRN, PRINCIPAL, Snap, chat_request, settings};

    const FORBID_X: &str = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"x", resource);"#;

    fn ctx() -> DecisionContext {
        DecisionContext::new(PRINCIPAL, ORG_A_PRN, Some("req-1".into()))
    }

    const FORBID_COMPUTER: &str = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;
    const FLOOR_COMPUTER: &str = r#"@id("floor-computer") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;

    /// A Responses request with one computer call and its screenshot output.
    fn computer_request(extra: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"model":"m"{extra},"input":[{{"type":"computer_call","id":"cu_1","call_id":"c3","actions":[{{"type":"screenshot"}}],"status":"completed"}},{{"type":"computer_call_output","call_id":"c3","output":{{"type":"computer_screenshot","image_url":"data:image/png;base64,SECRET"}}}}]}}"#
        ))
    }

    fn refusals(m: &Snap, mode: &str, reason: &str, refused: &str) -> u64 {
        m.counter(names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL, &[("mode", mode), ("reason", reason), ("refused", refused)])
    }

    async fn run_responses(policy: &RequestToolPolicy, body: Bytes) -> Result<RequestOutcome, Refusal> {
        let turn = parse_request(Format::OpenAiResponses, &body);
        policy.apply(&ctx(), "m", Format::OpenAiResponses, &turn, body).await
    }

    #[tokio::test]
    async fn a_blocked_computer_result_is_refused_in_enforce_and_by_the_floor_in_log_only() {
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_COMPUTER), settings(TenantMode::Enforce));
        assert_eq!(run_responses(&enforce, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
        let floor = testing::policy(&FakeSource::new(FLOOR_COMPUTER, testing::PERMIT_ALL), settings(TenantMode::LogOnly));
        assert_eq!(run_responses(&floor, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
    }

    async fn run(policy: &RequestToolPolicy, body: Bytes) -> Result<RequestOutcome, Refusal> {
        let turn = parse_request(Format::OpenAiChat, &body);
        policy.apply(&ctx(), "m", Format::OpenAiChat, &turn, body).await
    }

    fn tool_names(body: &[u8]) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_slice(body).expect("json");
        v["tools"]
            .as_array()
            .map_or_else(Vec::new, |a| a.iter().map(|t| t["function"]["name"].as_str().expect("name").to_owned()).collect())
    }

    #[tokio::test]
    async fn an_unavailable_turn_is_refused_only_in_enforce() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        // A duplicate key: the strict parse fails, so the turn is unavailable (C13).
        let body = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);

        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run(&enforce, body.clone()).await.err(), Some(Refusal::UnreadableTurn));
        let m = Snap::take(&snapshotter);
        assert_eq!((refusals(&m, "enforce", "unreadable_turn", "true"), refusals(&m, "enforce", "unreadable_turn", "false")), (1, 0));

        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let out = run(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr(), "the original bytes");
        assert!(out.entries.is_empty());
        let m = Snap::take(&snapshotter);
        assert_eq!(
            (refusals(&m, "log_only", "unreadable_turn", "false"), refusals(&m, "log_only", "unreadable_turn", "true")),
            (1, 0),
            "counted as enforce would refuse (P-10)"
        );

        let off = testing::policy(&source, settings(TenantMode::Off));
        let out = run(&off, body.clone()).await.expect("off forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr());
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "off", "unreadable_turn", "true") + refusals(&m, "off", "unreadable_turn", "false"), 0);
    }

    #[tokio::test]
    async fn an_unknown_tool_choice_with_an_enforced_removal_is_refused() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let body = chat_request(&["x", "y"], r#","tool_choice":"weird""#);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run(&enforce, body.clone()).await.err(), Some(Refusal::UnknownToolChoice));
        // Log-only: the tenant removal is not enforced, so the request goes out unchanged.
        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let out = run(&log_only, body.clone()).await.expect("log_only");
        assert_eq!(out.body, body);
        // A floor removal is enforced in every mode (D33), so log-only refuses it too.
        let floor_body = chat_request(&["rm", "y"], r#","tool_choice":"weird""#);
        assert_eq!(run(&log_only, floor_body).await.err(), Some(Refusal::UnknownToolChoice));
    }

    #[tokio::test]
    async fn a_computer_block_is_counted_as_refused_or_as_not_enforced() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, FORBID_COMPUTER);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run_responses(&enforce, computer_request("")).await.err(), Some(Refusal::UnreplaceableResult));
        let m = Snap::take(&snapshotter);
        assert_eq!(
            (refusals(&m, "enforce", "unreplaceable_result", "true"), refusals(&m, "enforce", "unreplaceable_result", "false")),
            (1, 0)
        );

        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let body = computer_request("");
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let out = run_responses(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr(), "the original bytes");
        let m = Snap::take(&snapshotter);
        assert_eq!(
            (refusals(&m, "log_only", "unreplaceable_result", "true"), refusals(&m, "log_only", "unreplaceable_result", "false")),
            (0, 1)
        );
        let line = one_line(&logs.text(), "tool-policy request refusal");
        for want in ["req-1", "log_only", "unreplaceable_result", "refused=false"] {
            assert!(line.contains(want), "{want:?} in {line}");
        }
        assert!(!line.contains("SECRET") && !line.contains("computer\""), "{line}");
    }

    #[tokio::test]
    async fn an_unknown_tool_choice_with_a_log_only_removal_is_counted_not_refused() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let log_only = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_X), settings(TenantMode::LogOnly));
        let body = chat_request(&["x", "y"], r#","tool_choice":"weird""#);
        let out = run(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body, body);
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "log_only", "unknown_tool_choice", "false"), 1);
        assert_eq!(refusals(&m, "log_only", "unknown_tool_choice", "true"), 0);
    }

    /// Review Focus 4: two enforced causes, one refused sample.
    #[tokio::test]
    async fn a_request_with_two_enforced_causes_counts_once() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let tenant = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action in [Pgs::Gateway::Action::"x", Pgs::Gateway::Action::"computer"], resource);"#;
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, tenant), settings(TenantMode::Enforce));
        let body = computer_request(r#","tools":[{"type":"function","name":"x","parameters":{}}],"tool_choice":{"type":"a_future_kind"}"#);
        assert_eq!(run_responses(&enforce, body).await.err(), Some(Refusal::UnreplaceableResult));
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "enforce", "unreplaceable_result", "true"), 1);
        assert_eq!(refusals(&m, "enforce", "unknown_tool_choice", "true") + refusals(&m, "enforce", "unknown_tool_choice", "false"), 0);
    }

    /// The single line of `logs` that holds `needle`.
    fn one_line(logs: &str, needle: &str) -> String {
        let lines: Vec<&str> = logs.lines().filter(|l| l.contains(needle)).collect();
        assert_eq!(lines.len(), 1, "want one line with {needle:?}: {logs}");
        lines[0].to_owned()
    }

    /// D17: a refusal, and a log-only refusal that is forwarded, is logged once with the request
    /// id, the mode and the reason. The line holds no body content and no tool name.
    #[tokio::test]
    async fn each_refusal_reason_is_logged_in_enforce_and_log_only() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let unreadable = Bytes::from_static(br#"{"model":"m","model":"SECRET_MODEL","messages":[]}"#);
        let weird = chat_request(&["rm", "y"], r#","tool_choice":"weird_choice""#);
        for (mode, want_mode) in [(TenantMode::Enforce, "enforce"), (TenantMode::LogOnly, "log_only")] {
            let policy = testing::policy(&source, settings(mode));
            for (body, reason, refused) in [(unreadable.clone(), "unreadable_turn", mode == TenantMode::Enforce), (weird.clone(), "unknown_tool_choice", true)] {
                let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
                let _ = run(&policy, body).await;
                let line = one_line(&logs.text(), "tool-policy request refusal");
                for want in ["req-1", want_mode, reason, if refused { "refused=true" } else { "refused=false" }] {
                    assert!(line.contains(want), "{want:?} in {line}");
                }
                for secret in ["SECRET_MODEL", "weird_choice", "\"rm\"", "messages"] {
                    assert!(!line.contains(secret), "{secret:?} must not be logged: {line}");
                }
            }
        }
    }

    #[tokio::test]
    async fn log_only_returns_the_original_bytes() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let policy = testing::policy(&source, settings(TenantMode::LogOnly));
        let body = chat_request(&["x", "y"], "");
        let out = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(out.body.as_ptr(), body.as_ptr(), "the same Bytes");
        assert_eq!(out.entries.len(), 1);
        assert_eq!(
            (out.entries[0].tool.as_str(), out.entries[0].action, out.entries[0].enforced),
            ("x", RequestAction::RemoveDefinition, false)
        );
        assert_eq!(out.entries[0].reason, RequestReason::ToolCallDenied);
    }

    #[tokio::test]
    async fn enforce_returns_the_rewritten_body() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let out = run(&policy, chat_request(&["x", "y", "rm"], "")).await.expect("apply");
        assert_eq!(tool_names(&out.body), ["y"]);
        assert_eq!(out.entries.len(), 2);
        assert!(out.entries.iter().all(|e| e.enforced && e.action == RequestAction::RemoveDefinition));
        let m = Snap::take(&snapshotter);
        assert_eq!(
            m.counter(
                names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
                &[("side", "request"), ("mode", "enforce"), ("effect", "block"), ("basis", "policy")]
            ),
            2
        );
    }

    #[tokio::test]
    async fn a_floor_removal_is_enforced_in_log_only() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let policy = testing::policy(&source, settings(TenantMode::LogOnly));
        let out = run(&policy, chat_request(&["x", "rm"], "")).await.expect("apply");
        assert_eq!(tool_names(&out.body), ["x"]);
        let enforced: Vec<(&str, bool)> = out.entries.iter().map(|e| (e.tool.as_str(), e.enforced)).collect();
        assert_eq!(enforced, [("x", false), ("rm", true)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_source_is_unavailable_within_load_timeout() {
        // D31: the tenant load passes `load_timeout`, so the tenant part is unavailable.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_tenant_delay(Duration::from_secs(30));
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let started = tokio::time::Instant::now();
        let body = chat_request(&["x"], "");
        let out = run(&policy, body.clone()).await.expect("apply");
        let waited = started.elapsed();
        assert!(waited <= Duration::from_millis(1100), "waited {waited:?}");
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Unavailable, .. }));
        // With the tenant part unavailable, only a floor deny changes the request (I3 ruling, D33);
        // the floor does not deny "x", so the body goes out unchanged.
        assert_eq!(out.body, body);
        assert!(out.entries.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_floor_and_the_tenant_share_one_load_timeout() {
        // D31: a slow floor and a slow tenant together still wait at most `load_timeout`.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_floor_delay(Duration::from_millis(600));
        source.set_tenant_delay(Duration::from_secs(30));
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let started = tokio::time::Instant::now();
        let out = run(&policy, chat_request(&["x"], "")).await.expect("apply");
        let waited = started.elapsed();
        assert!(waited <= Duration::from_millis(1100), "waited {waited:?}");
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Unavailable, .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn open_keeps_the_last_good_tenant_set_when_a_reload_is_slow() {
        // § 6.3.3: after an invalidation, a slow reload under `open` still serves the last good
        // tenant set up to `max_stale`. The shared request deadline must not drop that fallback.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let mut s = settings(TenantMode::Enforce);
        s.on_policy_unavailable = OnUnavailable::Open;
        let policy = testing::policy(&source, s);
        let first = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert!(matches!(first.snapshot.set, PolicySet::Available { tenant: TenantPart::Evaluated, .. }));
        assert_eq!(tool_names(&first.body), ["y"], "the tenant forbids x");

        policy.cache().invalidate_all();
        source.set_tenant_delay(Duration::from_secs(30));
        let started = tokio::time::Instant::now();
        let out = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert!(started.elapsed() <= Duration::from_millis(1100), "waited {:?}", started.elapsed());
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Evaluated, .. }), "the last good tenant set");
        assert_eq!(tool_names(&out.body), ["y"], "the tenant forbid still applies");
    }

    #[tokio::test]
    async fn the_snapshot_holds_the_request_side_time_and_the_sample_records_it_once() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let out = run(&policy, chat_request(&["y"], "")).await.expect("apply");
        assert!(out.snapshot.request_elapsed > Duration::ZERO);
        assert!(
            Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).is_empty(),
            "apply records no sample itself"
        );
        let sample = OverheadSample::new(Arc::clone(&out.snapshot));
        out.snapshot.add_decide_elapsed(Duration::from_secs(2));
        out.snapshot.add_decide_elapsed(Duration::from_secs(3));
        drop(sample);
        let samples = Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS);
        assert_eq!(
            samples,
            [(out.snapshot.request_elapsed + Duration::from_secs(5)).as_secs_f64()],
            "one sample: the request side plus both decide calls"
        );
    }

    #[tokio::test]
    async fn a_refused_request_records_one_sample() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        let body = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);
        assert_eq!(run(&enforce, body).await.err(), Some(Refusal::UnreadableTurn));
        assert_eq!(Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).len(), 1);
    }
}
