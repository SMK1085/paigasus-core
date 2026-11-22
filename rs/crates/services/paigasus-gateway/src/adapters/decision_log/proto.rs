// SPDX-License-Identifier: Apache-2.0

//! The mapping between a `DecisionRecord` and the generated `ToolDecisionEvent` (SMA-755 § 5.2). `to_event`
//! never fails. `from_event` is for the relay and for the tests: it ignores a field that it does not know
//! (prost drops unknown fields on decode), and it rejects a value that it cannot map.

use std::fmt;
use std::time::SystemTime;

use paigasus_gateway_core::decision_log::{
    AppliedEffect, ArgumentDigest, CedarDetail, DecisionRecord, FindingRecord, Prevention, RecordBasis, RecordHeader, RecordedAction, Side, ToolRef, build_refusal, derive_max_severity,
};
use paigasus_gateway_core::{BlockCode, DecisionContext, TenantMode};
use paigasus_kernel::CedarUid;
use paigasus_kernel::detect::Severity;
use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect, PolicyOrigin, PolicyRef, SessionInfo};
use paigasus_proto::paigasus::common::v1::{Actor, AuditMetadata};
use paigasus_proto::paigasus::gateway::v1 as pb;
use uuid::Uuid;

/// Why an event cannot become a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// A field that must hold a UUID does not.
    BadUuid(&'static str),
    /// A field that the record needs is not set.
    MissingField(&'static str),
    /// An enum field is unspecified or unknown.
    BadEnum(&'static str),
    /// Another value is out of range (a digest of the wrong length, a time that does not fit).
    BadValue(&'static str),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::BadUuid(field) => write!(f, "the field `{field}` is not a UUID"),
            MapError::MissingField(field) => write!(f, "the field `{field}` is not set"),
            MapError::BadEnum(field) => write!(f, "the field `{field}` holds an unspecified or unknown enum value"),
            MapError::BadValue(field) => write!(f, "the field `{field}` holds a value out of range"),
        }
    }
}

impl std::error::Error for MapError {}

fn i(v: impl Into<i32>) -> i32 {
    v.into()
}

fn policy_ref(r: &PolicyRef) -> pb::PolicyRef {
    let origin = match r.set {
        PolicyOrigin::Floor => pb::PolicyOrigin::Floor,
        PolicyOrigin::Tenant => pb::PolicyOrigin::Tenant,
        _ => pb::PolicyOrigin::Unspecified,
    };
    pb::PolicyRef { origin: i(origin), id: r.id.clone() }
}

/// The proto value of a severity: the Cedar level plus 1, and `UNSPECIFIED` for an unknown one
/// (SMA-855 § 6.11.1). A later level that this adapter does not know is written as unknown, so
/// that a decode infers the fixed severity of its kind.
fn severity_to_pb(s: Option<Severity>) -> pb::FindingSeverity {
    match s {
        Some(Severity::Info) => pb::FindingSeverity::Info,
        Some(Severity::Low) => pb::FindingSeverity::Low,
        Some(Severity::Medium) => pb::FindingSeverity::Medium,
        Some(Severity::High) => pb::FindingSeverity::High,
        None | Some(_) => pb::FindingSeverity::Unspecified,
    }
}

/// The severity of a proto value. `UNSPECIFIED` is an unknown severity (an event older than SMA-855).
fn severity_from_pb(s: pb::FindingSeverity) -> Option<Severity> {
    match s {
        pb::FindingSeverity::Unspecified => None,
        pb::FindingSeverity::Info => Some(Severity::Info),
        pb::FindingSeverity::Low => Some(Severity::Low),
        pb::FindingSeverity::Medium => Some(Severity::Medium),
        pb::FindingSeverity::High => Some(Severity::High),
    }
}

/// The proto finding of a record: the kind, the severity and the detector. A record has no
/// evidence, so none can reach the event (SMA-855 D3).
fn finding_to_pb(f: &FindingRecord) -> pb::Finding {
    pb::Finding {
        kind: f.kind.clone(),
        severity: i(severity_to_pb(f.severity)),
        detector: f.detector.clone(),
        detector_version: f.version.clone(),
    }
}

fn finding_from(f: pb::Finding) -> Result<FindingRecord, MapError> {
    let severity = pb::FindingSeverity::try_from(f.severity).map_err(|_| MapError::BadEnum("cedar.findings.severity"))?;
    Ok(FindingRecord::new(f.kind, severity_from_pb(severity), f.detector, f.detector_version))
}

fn cedar_to_pb(c: &CedarDetail) -> pb::CedarDetail {
    let effect = match c.effect {
        Effect::Allow => pb::DecisionEffect::Allow,
        Effect::Deny => pb::DecisionEffect::Block,
        _ => pb::DecisionEffect::Unspecified,
    };
    let group = match c.action_group {
        ActionGroup::Catalog => pb::ActionGroup::Catalog,
        ActionGroup::Unregistered => pb::ActionGroup::Unregistered,
        _ => pb::ActionGroup::Unspecified,
    };
    pb::CedarDetail {
        effect: i(effect),
        determining: c.determining.iter().map(policy_ref).collect(),
        errored: c.errored.iter().map(policy_ref).collect(),
        floor_denied: c.floor_denied,
        findings: c.findings.iter().map(finding_to_pb).collect(),
        declared: c.declared.as_str().to_owned(),
        action_group: i(group),
        decide_duration: Some(prost_types::Duration {
            seconds: i64::try_from(c.decide_duration.as_secs()).unwrap_or(i64::MAX),
            nanos: i32::try_from(c.decide_duration.subsec_nanos()).unwrap_or(0),
        }),
    }
}

/// Map a record to the event. The audit block holds `created_at` (= `decided_at`) and the creator (= the
/// principal PRN) only.
#[must_use]
pub fn to_event(r: &DecisionRecord) -> pb::ToolDecisionEvent {
    let side = match r.side {
        Side::Request => pb::DecisionSide::Request,
        Side::Response => pb::DecisionSide::Response,
        _ => pb::DecisionSide::Unspecified,
    };
    let mode = match r.mode {
        None => pb::DecisionMode::Unspecified,
        Some(TenantMode::Off) => pb::DecisionMode::Off,
        Some(TenantMode::LogOnly) => pb::DecisionMode::LogOnly,
        Some(TenantMode::Enforce) => pb::DecisionMode::Enforce,
        Some(_) => pb::DecisionMode::Unspecified,
    };
    let applied = match r.applied {
        AppliedEffect::Allow => pb::DecisionEffect::Allow,
        AppliedEffect::Observe => pb::DecisionEffect::Observe,
        AppliedEffect::Block => pb::DecisionEffect::Block,
        _ => pb::DecisionEffect::Unspecified,
    };
    let basis = match r.basis {
        RecordBasis::Policy => pb::DecisionBasis::Policy,
        RecordBasis::ErroredForbid => pb::DecisionBasis::ErroredForbid,
        RecordBasis::Unavailable => pb::DecisionBasis::Unavailable,
        RecordBasis::OnFailure => pb::DecisionBasis::OnFailure,
        _ => pb::DecisionBasis::Unspecified,
    };
    let prevention = match r.prevention {
        Prevention::NotApplicable => pb::Prevention::NotApplicable,
        Prevention::Prevented => pb::Prevention::Prevented,
        Prevention::ProviderMayRun => pb::Prevention::ProviderMayRun,
        _ => pb::Prevention::Unspecified,
    };
    let request_action = match r.request_action {
        None => pb::RequestAction::Unspecified,
        Some(RecordedAction::RemoveDefinition) => pb::RequestAction::RemoveDefinition,
        Some(RecordedAction::ReplaceResult) => pb::RequestAction::ReplaceResult,
        Some(RecordedAction::ResetToolChoice) => pb::RequestAction::ResetToolChoice,
        Some(RecordedAction::Refuse) => pb::RequestAction::Refuse,
        Some(RecordedAction::ScreenResult) => pb::RequestAction::ScreenResult,
        Some(_) => pb::RequestAction::Unspecified,
    };
    pb::ToolDecisionEvent {
        decision_id: r.decision_id.to_string(),
        audit: Some(AuditMetadata {
            created_at: Some(prost_types::Timestamp::from(r.decided_at)),
            modified_at: None,
            creator: Some(Actor { prn: r.principal_prn.clone() }),
            modifier: None,
        }),
        org_id: r.org.map(|o| o.to_string()).unwrap_or_default(),
        principal_prn: r.principal_prn.clone(),
        principal: r.principal.as_ref().map(|p| pb::CedarUid {
            entity_type: p.entity_type.clone(),
            entity_id: p.entity_id.clone(),
        }),
        api_key_id: r.api_key_id.clone().unwrap_or_default(),
        scope_prn: r.scope_prn.clone(),
        request_id: r.request_id.clone().unwrap_or_default(),
        session_id: r.session.id.clone(),
        turn: r.session.turn,
        side: i(side),
        tool: Some(pb::ToolRef {
            name: r.tool.name.clone(),
            namespace: r.tool.namespace.clone(),
            action_id: r.tool.action_id.clone(),
            truncated: r.tool.truncated,
            kind: r.tool.kind.clone(),
            call_id: r.tool.call_id.clone(),
        }),
        ingress: r.ingress.clone(),
        model: r.model.clone(),
        mode: i(mode),
        applied: i(applied),
        block_code: r.block_code.map(|c| c.as_kebab().to_owned()).unwrap_or_default(),
        basis: i(basis),
        prevention: i(prevention),
        request_action: i(request_action),
        cedar: r.cedar.as_ref().map(cedar_to_pb),
        arguments: r.arguments.as_ref().map(|a| pb::ArgumentDigest {
            hmac_sha256: bytes::Bytes::copy_from_slice(&a.hmac_sha256),
            key_id: a.key_id.clone(),
        }),
        instance_id: r.instance_id.to_string(),
        sequence: r.sequence,
    }
}

fn uuid(s: &str, field: &'static str) -> Result<Uuid, MapError> {
    Uuid::try_parse(s).map_err(|_| MapError::BadUuid(field))
}

fn opt(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn policy_ref_from(p: &pb::PolicyRef) -> Result<PolicyRef, MapError> {
    let set = match pb::PolicyOrigin::try_from(p.origin) {
        Ok(pb::PolicyOrigin::Floor) => PolicyOrigin::Floor,
        Ok(pb::PolicyOrigin::Tenant) => PolicyOrigin::Tenant,
        _ => return Err(MapError::BadEnum("cedar.policy_ref.origin")),
    };
    Ok(PolicyRef::new(set, p.id.clone()))
}

fn cedar_from(c: pb::CedarDetail) -> Result<CedarDetail, MapError> {
    let effect = match pb::DecisionEffect::try_from(c.effect) {
        Ok(pb::DecisionEffect::Allow) => Effect::Allow,
        Ok(pb::DecisionEffect::Block) => Effect::Deny,
        _ => return Err(MapError::BadEnum("cedar.effect")),
    };
    let declared = match c.declared.as_str() {
        "yes" => Declared::Yes,
        "no" => Declared::No,
        "unknown" => Declared::Unknown,
        _ => return Err(MapError::BadEnum("cedar.declared")),
    };
    let action_group = match pb::ActionGroup::try_from(c.action_group) {
        Ok(pb::ActionGroup::Catalog) => ActionGroup::Catalog,
        Ok(pb::ActionGroup::Unregistered) => ActionGroup::Unregistered,
        _ => return Err(MapError::BadEnum("cedar.action_group")),
    };
    let decide_duration = match c.decide_duration {
        None => std::time::Duration::ZERO,
        Some(d) => {
            let secs = u64::try_from(d.seconds).map_err(|_| MapError::BadValue("cedar.decide_duration"))?;
            let nanos = u32::try_from(d.nanos).map_err(|_| MapError::BadValue("cedar.decide_duration"))?;
            std::time::Duration::new(secs, nanos)
        }
    };
    // The proto has no `max_severity`: it is derived from the findings (§ 6.11.2). A finding of
    // an event older than SMA-855 has no severity, so its kind's fixed severity is inferred.
    let findings = c.findings.into_iter().map(finding_from).collect::<Result<Vec<_>, _>>()?;
    let (max_severity, severity_inferred) = derive_max_severity(&findings);
    let mut detail = CedarDetail::new(
        effect,
        c.determining.iter().map(policy_ref_from).collect::<Result<_, _>>()?,
        c.errored.iter().map(policy_ref_from).collect::<Result<_, _>>()?,
        c.floor_denied,
        findings,
        max_severity,
        declared,
        action_group,
        decide_duration,
    );
    detail.severity_inferred = severity_inferred;
    Ok(detail)
}

/// Map an event back to a record.
///
/// # Errors
/// `MapError` when a UUID, a required field or an enum value is bad.
pub fn from_event(e: pb::ToolDecisionEvent) -> Result<DecisionRecord, MapError> {
    let decision_id = uuid(&e.decision_id, "decision_id")?;
    let instance_id = uuid(&e.instance_id, "instance_id")?;
    let org = if e.org_id.is_empty() { None } else { Some(uuid(&e.org_id, "org_id")?) };
    let created = e.audit.and_then(|a| a.created_at).ok_or(MapError::MissingField("audit.created_at"))?;
    let decided_at = SystemTime::try_from(created).map_err(|_| MapError::BadValue("audit.created_at"))?;
    let tool = e.tool.ok_or(MapError::MissingField("tool"))?;
    let side = match pb::DecisionSide::try_from(e.side) {
        Ok(pb::DecisionSide::Request) => Side::Request,
        Ok(pb::DecisionSide::Response) => Side::Response,
        _ => return Err(MapError::BadEnum("side")),
    };
    let mode = match pb::DecisionMode::try_from(e.mode) {
        Ok(pb::DecisionMode::Unspecified) => None,
        Ok(pb::DecisionMode::Off) => Some(TenantMode::Off),
        Ok(pb::DecisionMode::LogOnly) => Some(TenantMode::LogOnly),
        Ok(pb::DecisionMode::Enforce) => Some(TenantMode::Enforce),
        Err(_) => return Err(MapError::BadEnum("mode")),
    };
    let applied = match pb::DecisionEffect::try_from(e.applied) {
        Ok(pb::DecisionEffect::Allow) => AppliedEffect::Allow,
        Ok(pb::DecisionEffect::Observe) => AppliedEffect::Observe,
        Ok(pb::DecisionEffect::Block) => AppliedEffect::Block,
        _ => return Err(MapError::BadEnum("applied")),
    };
    let basis = match pb::DecisionBasis::try_from(e.basis) {
        Ok(pb::DecisionBasis::Policy) => RecordBasis::Policy,
        Ok(pb::DecisionBasis::ErroredForbid) => RecordBasis::ErroredForbid,
        Ok(pb::DecisionBasis::Unavailable) => RecordBasis::Unavailable,
        Ok(pb::DecisionBasis::OnFailure) => RecordBasis::OnFailure,
        _ => return Err(MapError::BadEnum("basis")),
    };
    let prevention = match pb::Prevention::try_from(e.prevention) {
        Ok(pb::Prevention::NotApplicable) => Prevention::NotApplicable,
        Ok(pb::Prevention::Prevented) => Prevention::Prevented,
        Ok(pb::Prevention::ProviderMayRun) => Prevention::ProviderMayRun,
        _ => return Err(MapError::BadEnum("prevention")),
    };
    let request_action = match pb::RequestAction::try_from(e.request_action) {
        Ok(pb::RequestAction::Unspecified) => None,
        Ok(pb::RequestAction::RemoveDefinition) => Some(RecordedAction::RemoveDefinition),
        Ok(pb::RequestAction::ReplaceResult) => Some(RecordedAction::ReplaceResult),
        Ok(pb::RequestAction::ResetToolChoice) => Some(RecordedAction::ResetToolChoice),
        Ok(pb::RequestAction::Refuse) => Some(RecordedAction::Refuse),
        Ok(pb::RequestAction::ScreenResult) => Some(RecordedAction::ScreenResult),
        Err(_) => return Err(MapError::BadEnum("request_action")),
    };
    let block_code = if e.block_code.is_empty() {
        None
    } else {
        Some(BlockCode::ALL.iter().copied().find(|c| c.as_kebab() == e.block_code).ok_or(MapError::BadEnum("block_code"))?)
    };
    let arguments = match e.arguments {
        None => None,
        Some(a) => {
            let digest = <[u8; 32]>::try_from(a.hmac_sha256.as_ref()).map_err(|_| MapError::BadValue("arguments.hmac_sha256"))?;
            Some(ArgumentDigest::new(digest, a.key_id))
        }
    };
    let cedar = e.cedar.map(cedar_from).transpose()?;

    // `DecisionRecord` is non-exhaustive, so a downstream crate cannot name every field in a literal. Build a
    // base record through the public builder and then set every field. The round-trip test fails when a
    // field of the base survives.
    let request_id = opt(e.request_id);
    let ctx = DecisionContext::new(e.principal_prn.clone(), e.scope_prn.clone(), request_id.clone());
    let mut r = build_refusal(RecordHeader::new(decision_id, decided_at, instance_id, e.sequence, org), &ctx, mode);
    r.decision_id = decision_id;
    r.decided_at = decided_at;
    r.instance_id = instance_id;
    r.sequence = e.sequence;
    r.org = org;
    r.principal_prn = e.principal_prn;
    r.principal = e.principal.map(|p| CedarUid {
        entity_type: p.entity_type,
        entity_id: p.entity_id,
    });
    r.api_key_id = opt(e.api_key_id);
    r.scope_prn = e.scope_prn;
    r.request_id = request_id;
    r.session = SessionInfo::new(e.session_id, e.turn);
    r.side = side;
    let mut t = ToolRef::default();
    t.name = tool.name;
    t.namespace = tool.namespace;
    t.action_id = tool.action_id;
    t.truncated = tool.truncated;
    t.kind = tool.kind;
    t.call_id = tool.call_id;
    r.tool = t;
    r.ingress = e.ingress;
    r.model = e.model;
    r.mode = mode;
    r.applied = applied;
    r.block_code = block_code;
    r.basis = basis;
    r.prevention = prevention;
    r.request_action = request_action;
    r.cedar = cedar;
    r.arguments = arguments;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_gateway_core::decision_log::{DecisionDetail, ReportedCall, build_request, build_response};
    use paigasus_gateway_core::{Basis, ReportDecision, ReportEntry, RequestAction, RequestEntry, RequestReason};
    use prost::Message;
    use std::sync::Arc;
    use std::time::Duration;

    const ORG: &str = "0190a100-0000-7000-8000-0000000000a1";

    fn header() -> RecordHeader {
        RecordHeader::new(
            Uuid::from_u128(0x0190_a100_0000_7000_8000_0000_0000_0001),
            SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 123_000_000),
            Uuid::from_u128(9),
            42,
            Some(Uuid::try_parse(ORG).unwrap()),
        )
    }

    fn ctx() -> DecisionContext {
        let mut c = DecisionContext::new("prn:pgs:iam:::user/u", format!("prn:pgs:iam:::organization/{ORG}"), Some("req-1".into()));
        c.api_key_id = Some("key-1".into());
        c.ingress = "chat_completions".into();
        c.model = "gpt-x".into();
        c
    }

    fn response_record() -> DecisionRecord {
        let entry = ReportEntry {
            choice: 0,
            position: 0,
            name: "Get_Weather".into(),
            decision: ReportDecision::Block,
            basis: Some(Basis::Decider),
            code: Some(BlockCode::Policy),
            enforced: Some(true),
            reason: None,
        };
        let cedar = CedarDetail::new(
            Effect::Deny,
            vec![PolicyRef::new(PolicyOrigin::Floor, "f1")],
            vec![PolicyRef::new(PolicyOrigin::Tenant, "t1")],
            true,
            vec![
                FindingRecord::new("name_collision", Some(Severity::High), "kernel", ""),
                FindingRecord::new("secret_in_args", Some(Severity::Medium), "secret_in_args", "v1"),
                FindingRecord::new("unparsed_arguments", Some(Severity::Info), "kernel", ""),
            ],
            Some(Severity::High),
            Declared::Unknown,
            ActionGroup::Unregistered,
            Duration::new(0, 1_500_000),
        );
        let detail = DecisionDetail::new(RecordBasis::ErroredForbid, Some(cedar), Some(ArgumentDigest::new([3; 32], "k1")));
        let mut r = build_response(header(), &ctx(), &ReportedCall::new(entry, None, Some(Arc::new(detail))));
        r.principal = Some(CedarUid {
            entity_type: "Pgs::Iam::User".into(),
            entity_id: "u".into(),
        });
        r.session = SessionInfo::new("sess", 3);
        r.mode = Some(TenantMode::LogOnly);
        r.tool.kind = "function".into();
        r.tool.call_id = "call_1".into();
        r.tool.namespace = "ns".into();
        r.tool.truncated = true;
        r
    }

    fn request_record() -> DecisionRecord {
        let e = RequestEntry::new("Get_Weather".into(), RequestAction::ReplaceResult, false, RequestReason::ToolCallDenied);
        build_request(header(), &ctx(), Some(TenantMode::Enforce), &e)
    }

    fn round_trip(r: &DecisionRecord) -> DecisionRecord {
        let bytes = to_event(r).encode_to_vec();
        from_event(pb::ToolDecisionEvent::decode(bytes.as_slice()).unwrap()).unwrap()
    }

    #[test]
    fn a_record_survives_the_proto_round_trip() {
        let resp = response_record();
        assert!(resp.cedar.is_some() && resp.arguments.is_some() && resp.principal.is_some() && resp.api_key_id.is_some());
        assert_eq!(round_trip(&resp), resp);
        let req = request_record();
        assert_eq!(req.side, Side::Request);
        assert!(req.request_action.is_some());
        assert_eq!(round_trip(&req), req);
    }

    #[test]
    fn audit_carries_created_at_and_creator_only() {
        let r = response_record();
        let e = to_event(&r);
        let audit = e.audit.expect("audit is set");
        assert_eq!(SystemTime::try_from(audit.created_at.expect("created_at")).unwrap(), r.decided_at);
        assert_eq!(audit.creator.expect("creator").prn, r.principal_prn);
        assert!(audit.modified_at.is_none());
        assert!(audit.modifier.is_none());
    }

    #[test]
    fn an_event_with_unknown_fields_decodes() {
        let mut bytes = to_event(&response_record()).encode_to_vec();
        // Field 99, varint 1: an unknown field that a newer gateway may add.
        bytes.extend_from_slice(&[0x98, 0x06, 0x01]);
        let e = pb::ToolDecisionEvent::decode(bytes.as_slice()).unwrap();
        assert_eq!(from_event(e).unwrap(), response_record());
    }

    fn round_trip_cedar(e: pb::ToolDecisionEvent) -> CedarDetail {
        let bytes = e.encode_to_vec();
        from_event(pb::ToolDecisionEvent::decode(bytes.as_slice()).unwrap()).unwrap().cedar.expect("cedar")
    }

    #[test]
    fn each_severity_maps_both_ways() {
        for s in [None, Some(Severity::Info), Some(Severity::Low), Some(Severity::Medium), Some(Severity::High)] {
            assert_eq!(severity_from_pb(severity_to_pb(s)), s);
        }
        assert_eq!(severity_to_pb(None) as i32, 0);
        assert_eq!(severity_to_pb(Some(Severity::Info)) as i32, 1);
        assert_eq!(severity_to_pb(Some(Severity::Low)) as i32, 2);
        assert_eq!(severity_to_pb(Some(Severity::Medium)) as i32, 3);
        assert_eq!(severity_to_pb(Some(Severity::High)) as i32, 4);
    }

    #[test]
    fn a_record_with_findings_survives_the_round_trip() {
        let r = response_record();
        let e = to_event(&r);
        let f = &e.cedar.as_ref().expect("cedar").findings;
        let got: Vec<_> = f.iter().map(|f| (f.kind.as_str(), f.severity, f.detector.as_str(), f.detector_version.as_str())).collect();
        assert_eq!(
            got,
            [
                ("name_collision", 4, "kernel", ""),
                ("secret_in_args", 3, "secret_in_args", "v1"),
                ("unparsed_arguments", 1, "kernel", "")
            ]
        );
        let c = round_trip_cedar(e);
        assert_eq!(c.findings, r.cedar.as_ref().unwrap().findings);
        assert_eq!((c.max_severity, c.severity_inferred), (Some(Severity::High), false));
    }

    fn old_event() -> pb::ToolDecisionEvent {
        let mut e = to_event(&response_record());
        e.cedar.as_mut().unwrap().findings = vec![
            pb::Finding {
                kind: "name_collision".into(),
                ..Default::default()
            },
            pb::Finding {
                kind: "unparsed_arguments".into(),
                ..Default::default()
            },
        ];
        e
    }

    #[test]
    fn a_pre_sma_855_event_decodes_with_inferred_severities() {
        let c = round_trip_cedar(old_event());
        assert_eq!(c.findings.len(), 2);
        assert!(c.findings.iter().all(|f| f.severity.is_none() && f.detector.is_empty() && f.version.is_empty()));
        assert_eq!((c.max_severity, c.severity_inferred), (Some(Severity::High), true));
        let mut only_unparsed = old_event();
        only_unparsed.cedar.as_mut().unwrap().findings.remove(0);
        assert_eq!(round_trip_cedar(only_unparsed).max_severity, Some(Severity::Info));
        let mut none = old_event();
        none.cedar.as_mut().unwrap().findings.clear();
        let c = round_trip_cedar(none);
        assert_eq!((c.max_severity, c.severity_inferred), (None, false));
    }

    #[test]
    fn an_unknown_severity_value_is_rejected() {
        let mut e = to_event(&response_record());
        e.cedar.as_mut().unwrap().findings[0].severity = 9;
        assert_eq!(from_event(e), Err(MapError::BadEnum("cedar.findings.severity")));
    }

    #[test]
    fn screen_result_maps_both_ways() {
        let mut r = request_record();
        r.request_action = Some(RecordedAction::ScreenResult);
        let e = to_event(&r);
        assert_eq!(e.request_action, i(pb::RequestAction::ScreenResult));
        assert_eq!(round_trip(&r).request_action, Some(RecordedAction::ScreenResult));
    }

    #[test]
    fn the_event_holds_no_evidence() {
        const CANARY: &str = "canary-7f3a.example";
        let finding =
            paigasus_kernel::detect::Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1").with_evidence(paigasus_kernel::detect::Evidence::host("r", Some("/url"), CANARY.to_owned()));
        let mut r = response_record();
        r.cedar.as_mut().unwrap().findings = vec![FindingRecord::from_finding(&finding)];
        let bytes = to_event(&r).encode_to_vec();
        assert!(!bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()), "the event holds the evidence host");
        assert!(!bytes.windows(4).any(|w| w == b"/url"), "the event holds the evidence pointer");
    }

    #[test]
    fn a_bad_event_is_rejected_with_the_field_name() {
        let good = to_event(&response_record());
        let mut e = good.clone();
        e.decision_id = "nope".into();
        assert_eq!(from_event(e), Err(MapError::BadUuid("decision_id")));
        let mut e = good.clone();
        e.tool = None;
        assert_eq!(from_event(e), Err(MapError::MissingField("tool")));
        let mut e = good;
        e.side = 0;
        assert_eq!(from_event(e), Err(MapError::BadEnum("side")));
    }
}
