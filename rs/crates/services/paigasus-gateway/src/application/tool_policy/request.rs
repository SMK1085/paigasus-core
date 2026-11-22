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
    DecisionContext, Format, OnUnavailable, PolicySet, PolicySnapshot, RefusalCause, RequestAction, RequestEntry, TenantMode, TenantPart, ToolTurn, ingress, name_collisions, plan_request,
    rewrite_request,
};
use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies};
use paigasus_kernel::{Prn, to_cedar_uid};
use paigasus_observability::names;
use uuid::Uuid;

use super::cache::{Lookup, PolicyCache};
use super::decider::finding_label;
use crate::application::decision_log::DecisionRecorder;
use crate::config::ToolPolicySettings;
use crate::domain::org_of;
use crate::domain::tool_policy::TenantSettings;

/// Why the request side refuses a request. Each refusal is a 400 (P-10, SMA-776).
/// `UnreadableTurn` and `UnknownToolChoice` are a 400 `invalid-request-body`.
/// `UnreplaceableResult` is a 400 `tool-result-unreplaceable` (SMA-776).
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

/// A refused request: why, and the plan entries for the decision header (SMA-776 § 6.5).
/// `entries` is empty when no plan exists (an unreadable turn).
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub refusal: Refusal,
    pub entries: Vec<RequestEntry>,
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
    recorder: Arc<DecisionRecorder>,
}

/// Count and log a refusal, or a refusal that enforce mode would make (P-10, D17: log-only is
/// "forwarded, logged and counted"). `refused` says whether the request is refused (400) or
/// forwarded. `cause` names the branch (`unreadable_turn`, `unknown_tool_choice`,
/// `unreplaceable_result`, `rewrite_failed`). The line holds the request id, the mode and the reason only: no body
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
    pub fn new(cache: Arc<PolicyCache>, settings: ToolPolicySettings, recorder: Arc<DecisionRecorder>) -> Self {
        RequestToolPolicy { cache, settings, recorder }
    }

    /// The shared decision recorder (P-8).
    #[must_use]
    pub fn recorder(&self) -> &Arc<DecisionRecorder> {
        &self.recorder
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

    /// The policies of a request when the source returns the tenant settings (spec §9.3, R2).
    ///
    /// `pre` is the last known settings of the org, else `enforce` with the global fail mode. The
    /// floor lookup always uses the GLOBAL fail mode: a floor outage never follows a tenant value
    /// (spec §3.4, preflight F6). The tenant lookup always runs, also in mode `off`, because a
    /// fresh load can change the mode. The settings after the lookup (`fin`) decide the mode.
    async fn policy_set_from_source(&self, ctx: &DecisionContext, org: Uuid, model: &str, format: Format) -> (PolicySet, TenantSettings) {
        let global = self.settings.on_policy_unavailable;
        let pre = self.cache.tenant_settings(org).unwrap_or(TenantSettings {
            mode: TenantMode::Enforce,
            on_unavailable: global,
        });
        let fin_of = || self.cache.tenant_settings(org).unwrap_or(pre);
        let Ok(principal) = Prn::parse(&ctx.principal) else {
            return (PolicySet::Unavailable, fin_of());
        };
        let deadline = tokio::time::Instant::now() + self.settings.load_timeout;
        let Lookup::Ready(floor) = self.cache.floor_until(global, deadline).await else {
            return (PolicySet::Unavailable, fin_of());
        };
        // The set and the settings come from one cache entry, so a load that lands between two
        // reads cannot pair a set with the mode of another load. `pre.on_unavailable` is only the
        // default: a stored entry with settings decides the fallback by its own fail mode.
        let (tenant, settings) = self.cache.tenant_with_settings_until(org, &floor, pre.on_unavailable, deadline).await;
        let fin = settings.unwrap_or(pre);
        let (policies, part): (Arc<ToolPolicies>, TenantPart) = if fin.mode == TenantMode::Off {
            // The tenant set is not consulted in mode off (P-4), even when the lookup returned one.
            (Arc::clone(&floor.floor_only), TenantPart::Off)
        } else {
            match tenant {
                Lookup::Ready(t) => (t, TenantPart::Evaluated),
                Lookup::Unavailable => (Arc::clone(&floor.floor_only), TenantPart::Unavailable),
            }
        };
        let facts = Facts::new(to_cedar_uid(&principal), org, RequestInfo::new(ctx.scope.clone(), ingress(format), model), SessionInfo::placeholder());
        (PolicySet::Available { policies, tenant: part, facts }, fin)
    }

    /// Apply the tool policy to one request (§ 6.5). `turn` must be `parse_request(format, &body)`:
    /// the plan is made from the same bytes that the rewrite edits (controller ruling, Task 10).
    ///
    /// # Errors
    ///
    /// A [`Refused`] when the request must be refused with a 400 (D17, § 6.5.1 step 4, SMA-776).
    /// It carries the plan entries for the header. A refused request records its overhead sample
    /// here: it has no snapshot to hold (preflight F14).
    pub async fn apply(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes) -> Result<RequestOutcome, Refused> {
        let started = Instant::now();
        let out = self.apply_inner(ctx, model, format, turn, body, started).await;
        if out.is_err() {
            histogram!(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).record(started.elapsed().as_secs_f64());
        }
        out
    }

    /// D17, P-10: an unreadable turn is refused only in enforce mode; log-only counts it.
    fn check_turn(&self, ctx: &DecisionContext, turn: &ToolTurn, mode: TenantMode) -> Result<(), Refused> {
        if !matches!(turn, ToolTurn::Parsed(_)) && mode != TenantMode::Off {
            refusal_event(ctx, mode, &Refusal::UnreadableTurn, "unreadable_turn", mode == TenantMode::Enforce);
            if mode == TenantMode::Enforce {
                self.recorder.record_refusal(ctx, Some(mode));
                return Err(Refused {
                    refusal: Refusal::UnreadableTurn,
                    entries: Vec::new(),
                });
            }
        }
        Ok(())
    }

    /// One decision record per request entry (SMA-755 P-5).
    fn record_entries(&self, ctx: &DecisionContext, mode: TenantMode, entries: &[RequestEntry]) {
        for e in entries {
            self.recorder.record_request(ctx, Some(mode), e);
        }
    }

    async fn apply_inner(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes, started: Instant) -> Result<RequestOutcome, Refused> {
        let org = org_of(&ctx.scope);
        let (set, mode, on) = match org {
            // The source returns the tenant settings: they decide the mode and the fail mode. The
            // turn check follows the lookup, because the lookup can change the mode.
            Some(o) if self.settings.settings_from_source => {
                let (set, fin) = self.policy_set_from_source(ctx, o, model, format).await;
                // A floor outage follows the global value, never the tenant's (spec §3.4, F6).
                let on = if matches!(set, PolicySet::Unavailable) {
                    self.settings.on_policy_unavailable
                } else {
                    fin.on_unavailable
                };
                self.check_turn(ctx, turn, fin.mode)?;
                (set, fin.mode, on)
            }
            _ => {
                // D7, D34: the settings of the scope's org; without an org, the global values.
                let (mode, on) = org.map_or((self.settings.default_mode, self.settings.on_policy_unavailable), |o| self.settings.tenant(o));
                self.check_turn(ctx, turn, mode)?;
                (self.policy_set(ctx, org, mode, on, model, format).await, mode, on)
            }
        };
        let snapshot = PolicySnapshot::new(set, mode, on);

        let (body, entries) = if let ToolTurn::Parsed(facts) = turn {
            // One finding per colliding tool name (SMA-755 § 5.9).
            for _ in name_collisions(&facts.definitions) {
                counter!(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, "side" => "request", "kind" => finding_label("name_collision")).increment(1);
            }
            let plan = plan_request(&snapshot, format, facts, &[], &Instant::now);
            if let Some(cause) = plan.refusal() {
                let refusal = Refusal::from_cause(cause);
                refusal_event(ctx, mode, &refusal, refusal.label(), true);
                self.record_entries(ctx, mode, &plan.entries);
                self.recorder.record_refusal(ctx, Some(mode));
                return Err(Refused { refusal, entries: plan.entries });
            }
            let body = if plan.is_noop() {
                body
            } else if let Some(rewritten) = rewrite_request(format, &body, &plan) {
                rewritten
            } else {
                // Never send the original bytes for an enforced plan (controller ruling, Task 10).
                // This request is refused, so it counts once: the unenforced causes stay silent.
                refusal_event(ctx, mode, &Refusal::UnreadableTurn, "rewrite_failed", true);
                self.record_entries(ctx, mode, &plan.entries);
                self.recorder.record_refusal(ctx, Some(mode));
                return Err(Refused {
                    refusal: Refusal::UnreadableTurn,
                    entries: plan.entries,
                });
            };
            // The request goes on: report each cause that the mode did not enforce (G5).
            for cause in plan.refusals.keys() {
                let refusal = Refusal::from_cause(*cause);
                refusal_event(ctx, mode, &refusal, refusal.label(), false);
            }
            (body, plan.entries)
        } else {
            (body, Vec::new())
        };

        for e in entries.iter().filter(|e| e.action != RequestAction::ResetToolChoice) {
            let effect = if e.enforced { "block" } else { "observe" };
            counter!(names::GATEWAY_TOOL_POLICY_DECISIONS_TOTAL, "side" => "request", "mode" => mode.as_str(), "effect" => effect, "basis" => "policy").increment(1);
        }

        self.record_entries(ctx, mode, &entries);
        let snapshot = Arc::new(snapshot.with_request_elapsed(started.elapsed()));
        Ok(RequestOutcome { body, entries, snapshot })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_gateway_core::{BlockCode, Decision, Format as Fmt, RequestReason, decide_calls, parse_body, parse_request};

    use super::*;
    use crate::adapters::policy_source::iam::tenant_from_response;
    use crate::application::tool_policy::testing::{self, FakeSource, ORG_A_PRN, PRINCIPAL, Snap, chat_request, settings, settings_from_source};
    use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies, TenantSettings};
    use paigasus_proto::paigasus::iam::v1::{
        CompiledToolPolicySet, GetCompiledToolPoliciesResponse, HistoryCondition, ToolPolicyFailMode, ToolPolicyMode, ToolPolicySetKind, ToolPolicySettings as ProtoSettings,
    };

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

    async fn run_responses(policy: &RequestToolPolicy, body: Bytes) -> Result<RequestOutcome, Refused> {
        let turn = parse_request(Format::OpenAiResponses, &body);
        policy.apply(&ctx(), "m", Format::OpenAiResponses, &turn, body).await
    }

    #[tokio::test]
    async fn a_blocked_computer_result_is_refused_in_enforce_and_by_the_floor_in_log_only() {
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_COMPUTER), settings(TenantMode::Enforce));
        let refused = run_responses(&enforce, computer_request("")).await.expect_err("refused");
        assert_eq!(refused.refusal, Refusal::UnreplaceableResult);
        assert_eq!(
            refused.entries.iter().map(|e| (e.tool.as_str(), e.action, e.enforced)).collect::<Vec<_>>(),
            [("computer", RequestAction::ReplaceResult, true)]
        );
        let floor = testing::policy(&FakeSource::new(FLOOR_COMPUTER, testing::PERMIT_ALL), settings(TenantMode::LogOnly));
        assert_eq!(run_responses(&floor, computer_request("")).await.err().map(|r| r.refusal), Some(Refusal::UnreplaceableResult));
    }

    async fn run(policy: &RequestToolPolicy, body: Bytes) -> Result<RequestOutcome, Refused> {
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
    async fn a_removal_and_a_refusal_are_recorded() {
        use paigasus_gateway_core::decision_log::{AppliedEffect, RecordedAction, Side};
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        // A removal that the request goes on with: one record.
        let (log, rec) = testing::memory_recorder();
        let enforce = testing::policy_with(&source, settings(TenantMode::Enforce), rec);
        let out = run(&enforce, chat_request(&["x", "y"], "")).await.expect("forwarded");
        assert_eq!(tool_names(&out.body), ["y"]);
        let records = log.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            (records[0].request_action, records[0].applied, records[0].side),
            (Some(RecordedAction::RemoveDefinition), AppliedEffect::Block, Side::Request)
        );
        assert_eq!(
            (records[0].tool.name.as_str(), records[0].block_code, records[0].mode),
            ("x", Some(BlockCode::Policy), Some(TenantMode::Enforce))
        );
        // A refused request: its entry, then one refusal record.
        let (log, rec) = testing::memory_recorder();
        let enforce = testing::policy_with(&source, settings(TenantMode::Enforce), rec);
        let refused = run(&enforce, chat_request(&["x", "y"], r#","tool_choice":"weird""#)).await.expect_err("refused");
        assert_eq!(refused.refusal, Refusal::UnknownToolChoice);
        let records = log.records();
        let actions: Vec<_> = records.iter().map(|r| r.request_action).collect();
        assert_eq!(actions, [Some(RecordedAction::RemoveDefinition), Some(RecordedAction::Refuse)], "{records:?}");
        let refusal = &records[1];
        assert_eq!((refusal.applied, refusal.block_code, refusal.tool.name.as_str()), (AppliedEffect::Block, None, ""));
        assert_eq!(records.iter().map(|r| r.sequence).collect::<Vec<_>>(), [1, 2]);
        // An unreadable turn in enforce is refused with no entry: only the refusal record.
        let (log, rec) = testing::memory_recorder();
        let enforce = testing::policy_with(&source, settings(TenantMode::Enforce), rec);
        let dup = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);
        assert_eq!(run(&enforce, dup).await.err().map(|r| r.refusal), Some(Refusal::UnreadableTurn));
        let actions: Vec<_> = log.records().iter().map(|r| r.request_action).collect();
        assert_eq!(actions, [Some(RecordedAction::Refuse)]);
    }

    #[tokio::test]
    async fn a_log_only_would_refuse_writes_no_refusal_record() {
        use paigasus_gateway_core::decision_log::{AppliedEffect, RecordedAction};
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        let (log, rec) = testing::memory_recorder();
        let log_only = testing::policy_with(&source, settings(TenantMode::LogOnly), rec);
        // An unreadable turn: forwarded, counted as a would-refuse, no record at all.
        let dup = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);
        run(&log_only, dup).await.expect("log_only forwards");
        assert!(log.records().is_empty(), "{:?}", log.records());
        // An unknown tool choice with a tenant removal: forwarded. The removal is recorded as observed, there is no refusal.
        run(&log_only, chat_request(&["x", "y"], r#","tool_choice":"weird""#)).await.expect("log_only forwards");
        let records = log.records();
        assert_eq!(records.iter().map(|r| r.request_action).collect::<Vec<_>>(), [Some(RecordedAction::RemoveDefinition)], "{records:?}");
        assert_eq!(records[0].applied, AppliedEffect::Observe);
    }

    #[tokio::test]
    async fn request_side_findings_are_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        // Two definitions with one normal form, and one that is alone.
        run(&enforce, chat_request(&["a", "A", "b"], "")).await.expect("forwarded");
        let m = Snap::take(&snapshotter);
        let finding = |side: &str, kind: &str| m.counter(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &[("side", side), ("kind", kind)]);
        assert_eq!((finding("request", "name_collision"), finding("response", "name_collision")), (1, 0));
        // No collision, no finding.
        run(&enforce, chat_request(&["a", "b"], "")).await.expect("forwarded");
        assert_eq!(
            Snap::take(&snapshotter).counter(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL, &[("side", "request"), ("kind", "name_collision")]),
            0
        );
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
        assert_eq!(run(&enforce, body.clone()).await.err().map(|r| r.refusal), Some(Refusal::UnreadableTurn));
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
        assert_eq!(run(&enforce, body.clone()).await.err().map(|r| r.refusal), Some(Refusal::UnknownToolChoice));
        // Log-only: the tenant removal is not enforced, so the request goes out unchanged.
        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let out = run(&log_only, body.clone()).await.expect("log_only");
        assert_eq!(out.body, body);
        // A floor removal is enforced in every mode (D33), so log-only refuses it too.
        let floor_body = chat_request(&["rm", "y"], r#","tool_choice":"weird""#);
        assert_eq!(run(&log_only, floor_body).await.err().map(|r| r.refusal), Some(Refusal::UnknownToolChoice));
    }

    #[tokio::test]
    async fn a_computer_block_is_counted_as_refused_or_as_not_enforced() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, FORBID_COMPUTER);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run_responses(&enforce, computer_request("")).await.err().map(|r| r.refusal), Some(Refusal::UnreplaceableResult));
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

    /// A request with two enforced causes counts one refused sample, not one per cause.
    #[tokio::test]
    async fn a_request_with_two_enforced_causes_counts_once() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let tenant = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action in [Pgs::Gateway::Action::"x", Pgs::Gateway::Action::"computer"], resource);"#;
        let enforce = testing::policy(&FakeSource::new(testing::FLOOR, tenant), settings(TenantMode::Enforce));
        let body = computer_request(r#","tools":[{"type":"function","name":"x","parameters":{}}],"tool_choice":{"type":"a_future_kind"}"#);
        assert_eq!(run_responses(&enforce, body).await.err().map(|r| r.refusal), Some(Refusal::UnreplaceableResult));
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

    /// A refusal because the rewrite failed counts once. The unenforced computer cause of the same
    /// request stays silent: the request is refused, not forwarded. The body passed to `apply`
    /// is not the body the turn came from, so the rewrite cannot edit it.
    #[tokio::test]
    async fn a_failed_rewrite_counts_one_refusal_and_no_unenforced_cause() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let log_only = testing::policy(&FakeSource::new(testing::FLOOR, FORBID_COMPUTER), settings(TenantMode::LogOnly));
        // The floor removes `rm` in every mode: an enforced plan. The tenant block of the computer
        // result is not enforced in log-only: an unenforced cause.
        let good = computer_request(r#","tools":[{"type":"function","name":"rm","parameters":{}}]"#);
        let turn = parse_request(Format::OpenAiResponses, &good);
        let refused = log_only
            .apply(&ctx(), "m", Format::OpenAiResponses, &turn, Bytes::from_static(b"{"))
            .await
            .expect_err("the rewrite fails");
        assert_eq!(refused.refusal, Refusal::UnreadableTurn);
        assert_eq!(refused.entries.len(), 2, "the plan entries are kept for the header: {:?}", refused.entries);
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "log_only", "unreadable_turn", "true"), 1);
        assert_eq!(refusals(&m, "log_only", "unreplaceable_result", "false") + refusals(&m, "log_only", "unreplaceable_result", "true"), 0);
    }

    #[tokio::test]
    async fn a_refused_request_records_one_sample() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        let body = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);
        assert_eq!(run(&enforce, body).await.err().map(|r| r.refusal), Some(Refusal::UnreadableTurn));
        assert_eq!(Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).len(), 1);
    }
    fn from_source(mode: TenantMode, on_unavailable: OnUnavailable) -> Option<TenantSettings> {
        Some(TenantSettings { mode, on_unavailable })
    }

    #[tokio::test]
    async fn the_mode_from_the_source_overrides_the_default_mode() {
        // Default mode `off`, source `enforce`: a forbidden call is removed.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Closed));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Off));
        let out = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert_eq!(tool_names(&out.body), ["y"]);
        assert_eq!(out.snapshot.mode, TenantMode::Enforce);

        // Default mode `enforce`, source `log_only`: the call is observed, not removed.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::LogOnly, OnUnavailable::Closed));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Enforce));
        let body = chat_request(&["x", "y"], "");
        let out = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(out.body, body);
        assert_eq!(out.snapshot.mode, TenantMode::LogOnly);
        assert!(out.entries.iter().all(|e| !e.enforced));
    }

    #[tokio::test]
    async fn a_changed_mode_from_the_source_applies_on_the_next_load() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::LogOnly, OnUnavailable::Closed));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Off));
        let body = chat_request(&["x", "y"], "");
        let first = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(first.body, body, "log_only removes nothing");
        // The mode changes at the source. A mode change alone changes the version.
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Closed));
        policy.cache().invalidate_all();
        let second = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(tool_names(&second.body), ["y"], "the new mode applies in the same request that reloads");
        assert_eq!(second.snapshot.mode, TenantMode::Enforce);
    }

    #[tokio::test]
    async fn mode_off_from_the_source_still_applies_the_floor() {
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::Off, OnUnavailable::Closed));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Enforce));
        let out = run(&policy, chat_request(&["x", "rm", "y"], "")).await.expect("apply");
        assert_eq!(tool_names(&out.body), ["x", "y"], "the floor removes rm, the tenant forbid on x does not apply");
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Off, .. }));
        assert_eq!(out.snapshot.mode, TenantMode::Off);
    }

    #[tokio::test]
    async fn with_no_known_settings_the_global_fail_mode_applies() {
        // Only the tenant load fails: the floor loads. The default mode is `off`, so a mutation that
        // used the default mode for "no settings known" would show here (preflight F10).
        for global in [OnUnavailable::Closed, OnUnavailable::Open] {
            let source = FakeSource::new(testing::FLOOR, FORBID_X);
            source.set_fail_tenant(true);
            let mut s = settings_from_source(TenantMode::Off);
            s.on_policy_unavailable = global;
            let policy = testing::policy(&source, s);
            let out = run(&policy, chat_request(&["x", "rm", "y"], "")).await.expect("apply");
            assert_eq!(out.snapshot.mode, TenantMode::Enforce, "no settings known: enforce ({global:?})");
            assert_eq!(out.snapshot.on_unavailable, global, "no settings known: the global fail mode");
            assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Unavailable, .. }));
            assert_eq!(tool_names(&out.body), ["x", "y"], "the floor still applies ({global:?})");
            // End to end: one call to a tool of the tenant. Closed blocks it. Open allows it and reports it.
            let body = r#"{"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c0","type":"function","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
            let calls = parse_body(Fmt::OpenAiChat, body.as_bytes(), 1 << 20).expect("body").calls;
            let turn = parse_request(Format::OpenAiChat, &chat_request(&["y"], ""));
            let outcomes = decide_calls(&out.snapshot, &turn, &calls, &[], &Instant::now);
            let want = if global == OnUnavailable::Closed {
                Decision::Block { code: BlockCode::PolicyUnavailable }
            } else {
                Decision::Observe { would: BlockCode::PolicyUnavailable }
            };
            assert_eq!(outcomes.len(), 1);
            assert_eq!(outcomes[0].decision, want, "{global:?}");
        }
    }

    #[tokio::test]
    async fn the_tenant_is_looked_up_in_mode_off_so_a_new_mode_applies() {
        // The source says `off`: the first request loads it and applies the floor alone.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::Off, OnUnavailable::Closed));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Enforce));
        let body = chat_request(&["x", "y"], "");
        let first = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(first.body, body, "mode off: the tenant forbid does not apply");
        assert_eq!(source.tenant_calls(), 1);
        // The event path: the mode changes at the source and the cache is invalidated.
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Closed));
        policy.cache().invalidate_all();
        let second = run(&policy, body.clone()).await.expect("apply");
        assert_eq!(source.tenant_calls(), 2, "the cached mode off did not stop the tenant lookup");
        assert_eq!(tool_names(&second.body), ["y"], "the new mode applies in the request that reloads");
        assert_eq!(second.snapshot.mode, TenantMode::Enforce);
    }

    #[tokio::test]
    async fn the_global_fail_mode_decides_when_the_floor_is_unavailable() {
        // Spec §3.4, §9.3, preflight F6: a floor outage never follows the tenant fail mode.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Open));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Off));
        run(&policy, chat_request(&["y"], "")).await.expect("apply: the tenant settings are known now");
        policy.cache().invalidate_all();
        source.set_fail_floor(true);
        let out = run(&policy, chat_request(&["y"], "")).await.expect("apply");
        assert!(matches!(out.snapshot.set, PolicySet::Unavailable), "the floor is down and `open` is the tenant value");
        assert_eq!(out.snapshot.on_unavailable, OnUnavailable::Closed, "the global value, not the tenant's open");
        assert_eq!(out.snapshot.mode, TenantMode::Enforce, "the last known mode");
    }

    #[tokio::test]
    async fn the_last_known_fail_mode_applies_when_the_tenant_set_becomes_unavailable() {
        // Global `closed`, tenant `open`: after a good load, a source error serves the last good set.
        let source = FakeSource::new(testing::FLOOR, FORBID_X);
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Open));
        let policy = testing::policy(&source, settings_from_source(TenantMode::Off));
        let first = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert_eq!(tool_names(&first.body), ["y"]);
        source.set_fail_tenant(true);
        policy.cache().invalidate_all();
        let out = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Evaluated, .. }), "the last good set");
        assert_eq!(out.snapshot.on_unavailable, OnUnavailable::Open);
        assert_eq!(tool_names(&out.body), ["y"]);
    }

    /// A source that answers as IAM does: a proto answer, mapped by the IAM adapter. Its tenant set
    /// holds a history condition, so R5 refuses it (final review I-1).
    struct IamHistoryAnswer(GetCompiledToolPoliciesResponse);

    #[async_trait::async_trait]
    impl PolicySource for IamHistoryAnswer {
        async fn floor(&self) -> Result<PolicyText, SourceError> {
            Ok(PolicyText {
                text: testing::FLOOR.to_owned(),
                version: "f1".to_owned(),
            })
        }
        async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError> {
            tenant_from_response(org, self.0.clone())
        }
    }

    /// The request side over an IAM answer with a history set, `mode`, `fail` and the global value.
    fn history_policy(mode: ToolPolicyMode, fail: ToolPolicyFailMode, global: OnUnavailable) -> RequestToolPolicy {
        let answer = GetCompiledToolPoliciesResponse {
            set: Some(CompiledToolPolicySet {
                kind: ToolPolicySetKind::Tenant as i32,
                org_id: testing::ORG_A.hyphenated().to_string(),
                version: 2,
                cedar_policies: testing::PERMIT_ALL.to_owned(),
                conditions: vec![HistoryCondition {
                    slot: "h0".to_owned(),
                    policy_id: "p".to_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            settings: Some(ProtoSettings {
                mode: mode as i32,
                fail_mode: fail as i32,
                ..Default::default()
            }),
            floor_version: 1,
        };
        let mut s = settings_from_source(TenantMode::Enforce);
        s.on_policy_unavailable = global;
        let cache = PolicyCache::new(Arc::new(IamHistoryAnswer(answer)), crate::application::tool_policy::CacheSettings::from_settings(&s));
        RequestToolPolicy::new(cache, s, Arc::new(DecisionRecorder::tracing()))
    }

    /// The decision on one call to the tool `y` under `snapshot`.
    fn decide_y(snapshot: &PolicySnapshot) -> Decision {
        let body = r#"{"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c0","type":"function","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let calls = parse_body(Fmt::OpenAiChat, body.as_bytes(), 1 << 20).expect("body").calls;
        let turn = parse_request(Format::OpenAiChat, &chat_request(&["y"], ""));
        let outcomes = decide_calls(snapshot, &turn, &calls, &[], &Instant::now);
        assert_eq!(outcomes.len(), 1);
        outcomes[0].decision
    }

    #[tokio::test]
    async fn an_org_in_mode_off_with_a_history_set_is_not_blocked_under_a_global_closed() {
        let policy = history_policy(ToolPolicyMode::Off, ToolPolicyFailMode::Closed, OnUnavailable::Closed);
        let out = run(&policy, chat_request(&["x", "rm", "y"], "")).await.expect("apply");
        assert_eq!(out.snapshot.mode, TenantMode::Off, "the mode from IAM, not the R2 enforce");
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Off, .. }));
        assert_eq!(tool_names(&out.body), ["x", "y"], "the floor still applies");
        assert_eq!(decide_y(&out.snapshot), Decision::Allow);
    }

    #[tokio::test]
    async fn an_org_in_log_only_with_a_history_set_is_not_blocked_under_a_global_closed() {
        let policy = history_policy(ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed, OnUnavailable::Closed);
        let out = run(&policy, chat_request(&["x", "y"], "")).await.expect("apply");
        assert_eq!(out.snapshot.mode, TenantMode::LogOnly);
        assert!(matches!(out.snapshot.set, PolicySet::Available { tenant: TenantPart::Unavailable, .. }));
        assert_eq!(decide_y(&out.snapshot), Decision::Observe { would: BlockCode::PolicyUnavailable });
    }

    #[tokio::test]
    async fn the_tenant_fail_mode_decides_for_an_enforced_history_set() {
        // The global value is the opposite of the tenant value in each case, so it cannot decide.
        let open = history_policy(ToolPolicyMode::Enforce, ToolPolicyFailMode::Open, OnUnavailable::Closed);
        let out = run(&open, chat_request(&["y"], "")).await.expect("apply");
        assert_eq!((out.snapshot.mode, out.snapshot.on_unavailable), (TenantMode::Enforce, OnUnavailable::Open));
        assert_eq!(decide_y(&out.snapshot), Decision::Observe { would: BlockCode::PolicyUnavailable });
        let closed = history_policy(ToolPolicyMode::Enforce, ToolPolicyFailMode::Closed, OnUnavailable::Open);
        let out = run(&closed, chat_request(&["y"], "")).await.expect("apply");
        assert_eq!((out.snapshot.mode, out.snapshot.on_unavailable), (TenantMode::Enforce, OnUnavailable::Closed));
        assert_eq!(decide_y(&out.snapshot), Decision::Block { code: BlockCode::PolicyUnavailable });
    }

    #[tokio::test]
    async fn an_unreadable_turn_uses_the_mode_from_the_source() {
        let body = Bytes::from_static(br#"{"model":"m","model":"n","messages":[]}"#);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        source.set_settings(from_source(TenantMode::Enforce, OnUnavailable::Closed));
        let enforce = testing::policy(&source, settings_from_source(TenantMode::Off));
        assert_eq!(run(&enforce, body.clone()).await.err().map(|r| r.refusal), Some(Refusal::UnreadableTurn));

        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        source.set_settings(from_source(TenantMode::LogOnly, OnUnavailable::Closed));
        let log_only = testing::policy(&source, settings_from_source(TenantMode::Enforce));
        let out = run(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr());
    }
}
