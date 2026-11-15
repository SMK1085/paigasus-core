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
use paigasus_gateway_core::{DecisionContext, Format, OnUnavailable, PolicySet, PolicySnapshot, RequestAction, RequestEntry, TenantMode, TenantPart, ToolTurn, ingress, plan_request, rewrite_request};
use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies};
use paigasus_kernel::{Prn, to_cedar_uid};
use paigasus_observability::names;
use uuid::Uuid;

use super::cache::{Lookup, PolicyCache};
use crate::config::ToolPolicySettings;
use crate::domain::org_of;

/// Why the request side refuses a request. Both are a 400 `BadRequestBody` (P-10).
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The turn is `ToolTurn::Unavailable` and the tenant mode is `enforce` (D17). Also used when
    /// an enforced plan cannot be applied to the body (controller ruling, Task 10).
    UnreadableTurn,
    /// The `tool_choice` has a shape the gateway does not know, and an enforced removal exists
    /// (§ 6.5.1 step 4).
    UnknownToolChoice,
}

impl Refusal {
    /// The `reason` metric label.
    fn label(&self) -> &'static str {
        match self {
            Refusal::UnreadableTurn => "unreadable_turn",
            Refusal::UnknownToolChoice => "unknown_tool_choice",
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

/// Count a refusal, or a refusal that enforce mode would make (P-10).
fn count_refusal(mode: TenantMode, refusal: &Refusal) {
    counter!(names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL, "mode" => mode.as_str(), "reason" => refusal.label()).increment(1);
}

/// Record the overhead of a request whose calls the decider never decided (preflight F14). When the
/// decider runs, it records the sample itself, with the request-side time in it.
pub fn record_request_overhead(snapshot: &PolicySnapshot) {
    histogram!(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS).record(snapshot.request_elapsed.as_secs_f64());
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
        let Ok(Lookup::Ready(floor)) = tokio::time::timeout_at(deadline, self.cache.floor(on)).await else {
            return PolicySet::Unavailable;
        };
        let (policies, tenant): (Arc<ToolPolicies>, TenantPart) = if mode == TenantMode::Off {
            // The tenant set is not consulted in mode off (P-4).
            (Arc::clone(&floor.floor_only), TenantPart::Off)
        } else {
            match tokio::time::timeout_at(deadline, self.cache.tenant(org, &floor, on)).await {
                Ok(Lookup::Ready(t)) => (t, TenantPart::Evaluated),
                Ok(Lookup::Unavailable) | Err(_) => (Arc::clone(&floor.floor_only), TenantPart::Unavailable),
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
    /// A [`Refusal`] when the request must be refused with a 400 (D17, § 6.5.1 step 4).
    pub async fn apply(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes) -> Result<RequestOutcome, Refusal> {
        let started = Instant::now();
        let org = org_of(&ctx.scope);
        // D7, D34: the settings of the scope's org; without an org, the global values.
        let (mode, on) = org.map_or((self.settings.default_mode, self.settings.on_policy_unavailable), |o| self.settings.tenant(o));

        // D17, P-10: an unreadable turn is refused only in enforce mode; log-only counts it.
        if !matches!(turn, ToolTurn::Parsed(_)) && mode != TenantMode::Off {
            count_refusal(mode, &Refusal::UnreadableTurn);
            if mode == TenantMode::Enforce {
                return Err(Refusal::UnreadableTurn);
            }
        }

        let set = self.policy_set(ctx, org, mode, on, model, format).await;
        let snapshot = PolicySnapshot::new(set, mode, on);

        let (body, entries) = if let ToolTurn::Parsed(facts) = turn {
            let plan = plan_request(&snapshot, format, facts);
            if plan.refuse {
                count_refusal(mode, &Refusal::UnknownToolChoice);
                return Err(Refusal::UnknownToolChoice);
            }
            let body = if plan.is_noop() {
                body
            } else if let Some(rewritten) = rewrite_request(format, &body, &plan) {
                rewritten
            } else {
                // Never send the original bytes for an enforced plan (controller ruling, Task 10).
                count_refusal(mode, &Refusal::UnreadableTurn);
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
        let refusals = |m: &Snap, mode: &str| m.counter(names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL, &[("mode", mode), ("reason", "unreadable_turn")]);

        let enforce = testing::policy(&source, settings(TenantMode::Enforce));
        assert_eq!(run(&enforce, body.clone()).await.err(), Some(Refusal::UnreadableTurn));
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "enforce"), 1);

        let log_only = testing::policy(&source, settings(TenantMode::LogOnly));
        let out = run(&log_only, body.clone()).await.expect("log_only forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr(), "the original bytes");
        assert!(out.entries.is_empty());
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "log_only"), 1, "counted as enforce would refuse (P-10)");

        let off = testing::policy(&source, settings(TenantMode::Off));
        let out = run(&off, body.clone()).await.expect("off forwards");
        assert_eq!(out.body.as_ptr(), body.as_ptr());
        let m = Snap::take(&snapshotter);
        assert_eq!(refusals(&m, "off"), 0);
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
        // With the policies unavailable, the request side changes nothing (§ 6.4).
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

    #[tokio::test]
    async fn the_snapshot_holds_the_request_side_time_and_the_helper_records_it() {
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
        record_request_overhead(&out.snapshot);
        let samples = Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS);
        assert_eq!(samples, [out.snapshot.request_elapsed.as_secs_f64()]);
    }
}
