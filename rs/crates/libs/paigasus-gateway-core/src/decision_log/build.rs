// SPDX-License-Identifier: Apache-2.0
//! The builders (SMA-755 § 5.4, P-2 to P-5). Each takes the facts of one report entry and returns
//! one record. No arguments, no clock and no entropy reach a builder.

use paigasus_kernel::tool_policy::{SessionInfo, normalise};
use paigasus_kernel::{CedarUid, Prn, to_cedar_uid};
use uuid::Uuid;

use super::record::{AppliedEffect, DecisionRecord, Prevention, RecordBasis, RecordHeader, RecordedAction, ReportedCall, Side, ToolRef};
use crate::decider::DecisionContext;
use crate::policy::{PolicySet, TenantMode};
use crate::report::{ReportDecision, RequestAction, RequestEntry, sanitize_name};
use crate::types::{Basis, BlockCode, ToolCallKind};

/// The cap of `namespace`, `action_id`, `call_id`, `model` and `ingress`, in bytes.
pub const MAX_FIELD_BYTES: usize = 256;

/// `s` cut to at most `MAX_FIELD_BYTES` on a UTF-8 boundary, and whether a cut happened.
#[must_use]
pub fn cap(s: &str) -> (String, bool) {
    if s.len() <= MAX_FIELD_BYTES {
        return (s.to_owned(), false);
    }
    let mut end = MAX_FIELD_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_owned(), true)
}

/// The org of a response record: `Facts.org` when the set is available, else `None`. The gateway
/// falls back to the org of the scope PRN.
#[must_use]
pub fn response_org(ctx: &DecisionContext) -> Option<Uuid> {
    match ctx.policy.as_deref().map(|p| &p.set) {
        Some(PolicySet::Available { facts, .. }) => Some(facts.org),
        _ => None,
    }
}

fn principal_of(ctx: &DecisionContext) -> Option<CedarUid> {
    match ctx.policy.as_deref().map(|p| &p.set) {
        Some(PolicySet::Available { facts, .. }) => Some(facts.principal.clone()),
        _ => Prn::parse(&ctx.principal).ok().map(|p| to_cedar_uid(&p)),
    }
}

fn session_of(ctx: &DecisionContext) -> SessionInfo {
    if let Some(s) = ctx.session.as_deref() {
        return s.info.clone();
    }
    match ctx.policy.as_deref().map(|p| &p.set) {
        Some(PolicySet::Available { facts, .. }) => facts.session.clone(),
        _ => SessionInfo::placeholder(),
    }
}

fn mode_of(ctx: &DecisionContext) -> Option<TenantMode> {
    let snap = ctx.policy.as_deref()?;
    matches!(snap.set, PolicySet::Available { .. }).then_some(snap.mode)
}

fn cap_into(s: &str, truncated: &mut bool) -> String {
    let (out, cut) = cap(s);
    *truncated |= cut;
    out
}

/// A record with the fields that every side shares. The caller sets the rest.
fn base(h: RecordHeader, ctx: &DecisionContext, side: Side, mode: Option<TenantMode>, tool: ToolRef) -> DecisionRecord {
    let mut truncated = tool.truncated;
    let ingress = cap_into(&ctx.ingress, &mut truncated);
    let model = cap_into(&ctx.model, &mut truncated);
    let mut tool = tool;
    tool.truncated = truncated;
    DecisionRecord {
        decision_id: h.decision_id,
        decided_at: h.decided_at,
        instance_id: h.instance_id,
        sequence: h.sequence,
        org: h.org,
        principal_prn: ctx.principal.clone(),
        principal: principal_of(ctx),
        api_key_id: ctx.api_key_id.clone(),
        scope_prn: ctx.scope.clone(),
        // The cap sets nothing else: `tool.truncated` is about the tool and the fields of the tool call.
        request_id: ctx.request_id.as_deref().map(|r| cap(r).0),
        session: session_of(ctx),
        side,
        tool,
        ingress,
        model,
        mode,
        applied: AppliedEffect::Allow,
        block_code: None,
        basis: RecordBasis::Policy,
        prevention: Prevention::NotApplicable,
        request_action: None,
        cedar: None,
        arguments: None,
    }
}

fn tool_ref(display_name: &str, namespace: Option<&str>, kind: Option<ToolCallKind>, call_id: Option<&str>) -> ToolRef {
    let mut truncated = display_name.len() > 128;
    let name = sanitize_name(display_name);
    let action_id = cap_into(&normalise(display_name), &mut truncated);
    let namespace = cap_into(namespace.unwrap_or(""), &mut truncated);
    let call_id = cap_into(call_id.unwrap_or(""), &mut truncated);
    ToolRef {
        name,
        namespace,
        action_id,
        truncated,
        kind: kind.map_or("", ToolCallKind::as_str).to_owned(),
        call_id,
    }
}

/// The prevention of a response entry (P-3).
fn prevention(decision: ReportDecision, applied: AppliedEffect, kind: Option<ToolCallKind>, ingress: &str) -> Prevention {
    if decision == ReportDecision::Unenforceable {
        return Prevention::ProviderMayRun;
    }
    if applied != AppliedEffect::Block {
        return Prevention::NotApplicable;
    }
    let may_run = match kind {
        Some(k) => k.provider_may_run(),
        None => ingress == "responses",
    };
    if may_run { Prevention::ProviderMayRun } else { Prevention::Prevented }
}

/// The record of one response-side report entry.
#[must_use]
pub fn build_response(h: RecordHeader, ctx: &DecisionContext, r: &ReportedCall) -> DecisionRecord {
    let e = &r.entry;
    let call = r.call.as_ref();
    let tool = tool_ref(&e.name, call.and_then(|c| c.namespace.as_deref()), call.map(|c| c.kind), call.and_then(|c| c.id.as_deref()));
    let mut rec = base(h, ctx, Side::Response, mode_of(ctx), tool);
    rec.applied = match e.decision {
        ReportDecision::Block => AppliedEffect::Block,
        ReportDecision::Unenforceable => AppliedEffect::Allow,
        ReportDecision::Allow if e.code.is_some() => AppliedEffect::Observe,
        ReportDecision::Allow => AppliedEffect::Allow,
    };
    rec.block_code = e.code;
    rec.basis = if e.basis == Some(Basis::OnFailure) {
        RecordBasis::OnFailure
    } else if let Some(d) = &r.detail {
        d.basis
    } else if mode_of(ctx).is_none() && ctx.policy.is_some() {
        RecordBasis::Unavailable
    } else {
        RecordBasis::Policy
    };
    rec.prevention = prevention(e.decision, rec.applied, call.map(|c| c.kind), &rec.ingress);
    if let Some(d) = &r.detail {
        rec.cedar = d.cedar.clone();
        rec.arguments = d.arguments.clone();
    }
    rec
}

fn recorded(a: RequestAction) -> RecordedAction {
    match a {
        RequestAction::RemoveDefinition => RecordedAction::RemoveDefinition,
        RequestAction::ReplaceResult => RecordedAction::ReplaceResult,
        RequestAction::ResetToolChoice => RecordedAction::ResetToolChoice,
    }
}

/// The record of one request-side change (P-5).
#[must_use]
pub fn build_request(h: RecordHeader, ctx: &DecisionContext, mode: Option<TenantMode>, e: &RequestEntry) -> DecisionRecord {
    let mut rec = base(h, ctx, Side::Request, mode, tool_ref(&e.tool, None, None, None));
    rec.applied = if e.enforced { AppliedEffect::Block } else { AppliedEffect::Observe };
    rec.block_code = Some(BlockCode::Policy);
    rec.basis = RecordBasis::Policy;
    // A replaced result is a result that the model already has: nothing was prevented (R10).
    rec.prevention = if e.enforced && e.action != RequestAction::ReplaceResult {
        Prevention::Prevented
    } else {
        Prevention::NotApplicable
    };
    rec.request_action = Some(recorded(e.action));
    rec
}

/// The record of a request that the tool policy refuses (P-5). The tool is empty.
#[must_use]
pub fn build_refusal(h: RecordHeader, ctx: &DecisionContext, mode: Option<TenantMode>) -> DecisionRecord {
    let mut rec = base(h, ctx, Side::Request, mode, ToolRef::default());
    rec.applied = AppliedEffect::Block;
    rec.block_code = None;
    rec.prevention = Prevention::Prevented;
    rec.request_action = Some(RecordedAction::Refuse);
    rec
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect, Facts, Findings, RequestInfo, ToolPolicies};

    use super::*;
    use crate::decision_log::record::{ArgumentDigest, CallFacts, CedarDetail, DecisionDetail};
    use crate::policy::{OnUnavailable, PolicySnapshot, TenantPart};
    use crate::report::{ReportEntry, RequestReason};
    use crate::types::ToolCall;

    const PRN: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003";
    const SCOPE: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000004";

    fn header(org: Option<Uuid>) -> RecordHeader {
        RecordHeader::new(Uuid::from_u128(1), SystemTime::UNIX_EPOCH, Uuid::from_u128(2), 7, org)
    }

    fn ctx() -> DecisionContext {
        DecisionContext::new(PRN, SCOPE, Some("req-1".into())).with_request(Some("key-1".into()), "chat_completions", "gpt-x")
    }

    fn available_ctx(mode: TenantMode) -> (DecisionContext, Uuid) {
        let org = Uuid::from_u128(9);
        let principal = to_cedar_uid(&Prn::parse(PRN).unwrap());
        let facts = Facts::new(principal, org, RequestInfo::new(SCOPE, "chat_completions", "gpt-x"), SessionInfo::new("s-1", 3));
        let policies = Arc::new(ToolPolicies::compile(Arc::new(ToolPolicies::compile_floor("").unwrap()), "", &[]).unwrap());
        let snap = PolicySnapshot::new(
            PolicySet::Available {
                policies,
                tenant: TenantPart::Evaluated,
                facts,
            },
            mode,
            OnUnavailable::Closed,
        );
        (ctx().with_policy(Arc::new(snap)), org)
    }

    fn unavailable_ctx() -> DecisionContext {
        ctx().with_policy(Arc::new(PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed)))
    }

    fn entry(decision: ReportDecision, basis: Option<Basis>, code: Option<BlockCode>) -> ReportEntry {
        ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision,
            basis,
            code,
            enforced: Some(decision == ReportDecision::Block),
            reason: None,
        }
    }

    fn call(kind: ToolCallKind) -> CallFacts {
        CallFacts::of(&ToolCall {
            choice: 0,
            position: 0,
            id: Some("call_1".into()),
            name: "Get_Weather".into(),
            namespace: Some("ns".into()),
            kind,
            arguments: "SECRET".into(),
        })
    }

    fn cedar(effect: Effect) -> CedarDetail {
        CedarDetail::new(
            effect,
            vec![],
            vec![],
            false,
            BTreeSet::from(["f".to_owned()]),
            Declared::Yes,
            ActionGroup::Catalog,
            Duration::from_millis(2),
        )
    }

    fn detail(basis: RecordBasis, effect: Effect) -> Arc<DecisionDetail> {
        Arc::new(DecisionDetail::new(basis, Some(cedar(effect)), Some(ArgumentDigest::new([7; 32], "k1"))))
    }

    #[test]
    fn the_record_takes_the_session_of_the_context() {
        let (c, _) = available_ctx(TenantMode::Enforce);
        let sid = Uuid::from_u128(77);
        let session = crate::session::SessionContext::new(Uuid::from_u128(9), sid, 5, Findings::new(), crate::session::StoreBudget::new(Duration::from_secs(1)));
        let with = c.clone().with_session(Arc::new(session));
        assert_eq!(session_of(&with), SessionInfo::new(sid.to_string(), 5));
        assert_eq!(session_of(&c), SessionInfo::new("s-1", 3));
        assert_eq!(session_of(&ctx()), SessionInfo::placeholder());
    }

    #[test]
    fn a_response_with_an_available_set_takes_facts_from_the_set() {
        let (c, org) = available_ctx(TenantMode::LogOnly);
        let r = ReportedCall::new(
            entry(ReportDecision::Allow, Some(Basis::Decider), None),
            Some(call(ToolCallKind::Function)),
            Some(detail(RecordBasis::Policy, Effect::Allow)),
        );
        let rec = build_response(header(response_org(&c)), &c, &r);
        assert_eq!(rec.org, Some(org));
        assert_eq!(rec.principal, Some(to_cedar_uid(&Prn::parse(PRN).unwrap())));
        assert_eq!(rec.session, SessionInfo::new("s-1", 3));
        assert_eq!(rec.mode, Some(TenantMode::LogOnly));
        assert_eq!((rec.api_key_id.as_deref(), rec.ingress.as_str(), rec.model.as_str()), (Some("key-1"), "chat_completions", "gpt-x"));
        assert_eq!(rec.applied, AppliedEffect::Allow);
        assert_eq!(rec.basis, RecordBasis::Policy);
        assert_eq!(rec.cedar, Some(cedar(Effect::Allow)));
        assert_eq!(rec.arguments, Some(ArgumentDigest::new([7; 32], "k1")));
        assert_eq!((rec.tool.kind.as_str(), rec.tool.call_id.as_str(), rec.tool.namespace.as_str()), ("function", "call_1", "ns"));
        assert_eq!(rec.tool.action_id, "get_weather");
        assert_eq!((rec.sequence, rec.side, rec.request_action), (7, Side::Response, None));
    }

    #[test]
    fn an_unavailable_set_takes_the_principal_from_the_prn() {
        let c = unavailable_ctx();
        assert_eq!(response_org(&c), None);
        let r = ReportedCall::new(
            entry(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::PolicyUnavailable)),
            Some(call(ToolCallKind::Function)),
            None,
        );
        let rec = build_response(header(None), &c, &r);
        assert_eq!(rec.principal, Some(to_cedar_uid(&Prn::parse(PRN).unwrap())));
        assert_eq!(rec.principal_prn, PRN);
        assert_eq!(rec.mode, None);
        assert_eq!(rec.session, SessionInfo::placeholder());
        assert_eq!(rec.org, None);
        assert_eq!(rec.basis, RecordBasis::Unavailable);
        assert_eq!((rec.cedar, rec.arguments), (None, None));
        assert_eq!((rec.applied, rec.block_code), (AppliedEffect::Block, Some(BlockCode::PolicyUnavailable)));
    }

    #[test]
    fn an_observed_hold_is_applied_allow_with_the_cedar_block() {
        let (c, _) = available_ctx(TenantMode::Enforce);
        let r = ReportedCall::new(
            entry(ReportDecision::Allow, Some(Basis::OnFailure), Some(BlockCode::Policy)),
            Some(call(ToolCallKind::Function)),
            Some(detail(RecordBasis::Policy, Effect::Deny)),
        );
        let rec = build_response(header(None), &c, &r);
        assert_eq!(rec.applied, AppliedEffect::Observe);
        assert_eq!(rec.block_code, Some(BlockCode::Policy));
        assert_eq!(rec.basis, RecordBasis::OnFailure);
        assert_eq!(rec.cedar.as_ref().map(|d| d.effect), Some(Effect::Deny));
        assert_eq!(rec.prevention, Prevention::NotApplicable);
    }

    #[test]
    fn a_block_all_entry_has_no_cedar_and_no_call_id() {
        let (c, _) = available_ctx(TenantMode::Enforce);
        let r = ReportedCall::new(entry(ReportDecision::Block, Some(Basis::OnFailure), Some(BlockCode::HoldLimit)), None, None);
        let rec = build_response(header(None), &c, &r);
        assert_eq!(rec.basis, RecordBasis::OnFailure);
        assert_eq!(rec.cedar, None);
        assert_eq!((rec.tool.kind.as_str(), rec.tool.call_id.as_str()), ("", ""));
        assert_eq!(rec.prevention, Prevention::Prevented);
    }

    #[test]
    fn an_unknown_kind_block_on_responses_may_run_at_the_provider() {
        let c = ctx().with_request(None, "responses", "m");
        let r = ReportedCall::new(entry(ReportDecision::Block, Some(Basis::OnFailure), Some(BlockCode::HoldLimit)), None, None);
        assert_eq!(build_response(header(None), &c, &r).prevention, Prevention::ProviderMayRun);
    }

    #[test]
    fn an_unenforceable_entry_may_run_at_the_provider() {
        let r = ReportedCall::new(entry(ReportDecision::Unenforceable, None, None), None, None);
        let rec = build_response(header(None), &ctx(), &r);
        assert_eq!(rec.applied, AppliedEffect::Allow);
        assert_eq!(rec.prevention, Prevention::ProviderMayRun);
    }

    #[test]
    fn a_shell_block_may_run_at_the_provider() {
        let mk = |k| ReportedCall::new(entry(ReportDecision::Block, Some(Basis::Decider), Some(BlockCode::Policy)), Some(call(k)), None);
        assert_eq!(build_response(header(None), &ctx(), &mk(ToolCallKind::Shell)).prevention, Prevention::ProviderMayRun);
        assert_eq!(build_response(header(None), &ctx(), &mk(ToolCallKind::Function)).prevention, Prevention::Prevented);
    }

    #[test]
    fn a_request_entry_record() {
        let e = RequestEntry::new("Get_Weather".into(), RequestAction::RemoveDefinition, false, RequestReason::ToolCallDenied);
        let rec = build_request(header(None), &ctx(), None, &e);
        assert_eq!(rec.side, Side::Request);
        assert_eq!(rec.applied, AppliedEffect::Observe);
        assert_eq!((rec.block_code, rec.basis), (Some(BlockCode::Policy), RecordBasis::Policy));
        assert_eq!((rec.cedar, rec.arguments), (None, None));
        assert_eq!(rec.tool.kind, "");
        assert_eq!(rec.tool.call_id, "");
        assert_eq!(rec.request_action, Some(RecordedAction::RemoveDefinition));
        assert_eq!(rec.principal, Some(to_cedar_uid(&Prn::parse(PRN).unwrap())));
        assert_eq!(rec.session, SessionInfo::placeholder());
        let e = RequestEntry::new("x".into(), RequestAction::ResetToolChoice, true, RequestReason::ToolCallDenied);
        let rec = build_request(header(None), &ctx(), Some(TenantMode::Enforce), &e);
        assert_eq!((rec.applied, rec.mode), (AppliedEffect::Block, Some(TenantMode::Enforce)));
    }

    #[test]
    fn a_request_side_replace_result_prevents_nothing() {
        let mk = |action, enforced| {
            build_request(
                header(None),
                &ctx(),
                Some(TenantMode::Enforce),
                &RequestEntry::new("x".into(), action, enforced, RequestReason::ToolCallDenied),
            )
        };
        assert_eq!(mk(RequestAction::ReplaceResult, true).prevention, Prevention::NotApplicable);
        assert_eq!(mk(RequestAction::RemoveDefinition, true).prevention, Prevention::Prevented);
        assert_eq!(mk(RequestAction::ResetToolChoice, true).prevention, Prevention::Prevented);
        assert_eq!(mk(RequestAction::RemoveDefinition, false).prevention, Prevention::NotApplicable);
        assert_eq!(build_refusal(header(None), &ctx(), None).prevention, Prevention::Prevented);
    }

    #[test]
    fn a_long_request_id_is_capped_on_a_character_boundary() {
        let id = format!("{}é", "r".repeat(255));
        let c = DecisionContext::new(PRN, SCOPE, Some(id));
        let rec = build_refusal(header(None), &c, None);
        assert_eq!(rec.request_id.as_deref(), Some("r".repeat(255).as_str()));
        assert!(!rec.tool.truncated, "the request id cap sets no other flag");
        let c = DecisionContext::new(PRN, SCOPE, Some("short".into()));
        assert_eq!(build_refusal(header(None), &c, None).request_id.as_deref(), Some("short"));
    }

    #[test]
    fn a_refusal_record() {
        let rec = build_refusal(header(None), &ctx(), Some(TenantMode::Enforce));
        assert_eq!(rec.request_action, Some(RecordedAction::Refuse));
        assert_eq!((rec.applied, rec.block_code, rec.basis), (AppliedEffect::Block, None, RecordBasis::Policy));
        assert_eq!(rec.tool, ToolRef::default());
    }

    #[test]
    fn a_one_mib_name_is_capped() {
        let e = RequestEntry::new("a".repeat(1 << 20), RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied);
        let rec = build_request(header(None), &ctx(), None, &e);
        assert_eq!(rec.tool.name.len(), 128);
        assert!(rec.tool.action_id.len() <= MAX_FIELD_BYTES);
        assert!(rec.tool.truncated);
    }

    #[test]
    fn model_and_ingress_are_capped() {
        let c = ctx().with_request(None, "i".repeat(1000), "m".repeat(1000));
        let rec = build_refusal(header(None), &c, None);
        assert_eq!((rec.ingress.len(), rec.model.len()), (MAX_FIELD_BYTES, MAX_FIELD_BYTES));
        assert!(rec.tool.truncated);
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        let ns = format!("{}é", "a".repeat(255));
        let (out, cut) = cap(&ns);
        assert!(cut && out.len() <= MAX_FIELD_BYTES && out == "a".repeat(255));
    }

    #[test]
    fn a_short_field_is_not_cut() {
        assert_eq!(cap("abc"), ("abc".to_owned(), false));
    }

    #[test]
    fn a_record_has_no_arguments_text() {
        let r = ReportedCall::new(entry(ReportDecision::Allow, Some(Basis::Decider), None), Some(call(ToolCallKind::Function)), None);
        let rec = build_response(header(None), &ctx(), &r);
        assert!(!format!("{rec:?}").contains("SECRET"));
    }
}
