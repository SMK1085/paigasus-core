// SPDX-License-Identifier: Apache-2.0
//! The decisions of the calls of one hold, from a policy snapshot (§ 6.4, D33, D34, P-4 to P-6).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use paigasus_kernel::detect::{DETECTOR_BUILT_IN, DETECTOR_ERROR, DETECTOR_KERNEL, Finding, HostSet, Severity};
use paigasus_kernel::tool_policy::{Declared, Effect, Facts, Findings, ToolPolicies, Verdict, decide};

use super::mapping::{call_findings, invocation, name_collisions};
use super::snapshot::{OnUnavailable, PolicySet, PolicySnapshot, TenantMode, TenantPart};
use crate::decision_log::{CedarDetail, FindingRecord, records_of};
use crate::turn::ToolTurn;
use crate::types::{BlockCode, Decision, ToolCall};

/// What a call's decision rests on. It is the `basis` metric label (§ 6.7).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyBasis {
    /// A normal policy evaluation.
    Policy,
    /// A `forbid` that failed to evaluate decided the deny (D26).
    ErroredForbid,
    /// The policies (the floor, or the tenant part) were unavailable.
    Unavailable,
}

impl PolicyBasis {
    /// The metric label spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyBasis::Policy => "policy",
            PolicyBasis::ErroredForbid => "errored_forbid",
            PolicyBasis::Unavailable => "unavailable",
        }
    }
}

/// The decision of one call, with what it rests on and the kernel verdict (when one was made).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome {
    pub decision: Decision,
    pub basis: PolicyBasis,
    pub verdict: Option<Verdict>,
    /// The Cedar detail for the decision log (SMA-755). `Some` whenever a kernel verdict exists.
    pub cedar: Option<CedarDetail>,
}

fn unavailable(on: OnUnavailable) -> Decision {
    match on {
        OnUnavailable::Closed => Decision::Block { code: BlockCode::PolicyUnavailable },
        OnUnavailable::Open => Decision::Observe { would: BlockCode::PolicyUnavailable },
    }
}

/// The kernel finding set of the gateway findings: each kind with its highest severity.
pub(super) fn kernel_findings(given: &[Finding]) -> Findings {
    let mut set = Findings::new();
    for f in given {
        set.add(&f.kind, f.severity);
    }
    set
}

/// The Cedar detail of one kernel verdict: the findings that the gateway gave (`given`) and the
/// kernel-added ones. This is the one place that builds a detail from a verdict, so
/// `detection_disabled` always comes from `facts.detection` (the default `false` is fail-wrong).
pub(super) fn cedar_detail(v: &Verdict, given: &[Finding], declared: Declared, facts: &Facts, duration: Duration) -> CedarDetail {
    let mut cedar = CedarDetail::new(
        v.decision,
        v.determining.clone(),
        v.errored.clone(),
        v.floor_denied,
        records_of(given, v),
        v.max_severity,
        declared,
        v.action_group,
        duration,
    );
    cedar.detection_disabled = !facts.detection;
    cedar
}

/// The Cedar detail of a result decision that the plan did not evaluate, because the request
/// used up its pair budget (§ 6.8.2, A13). No policy decided it: the effect is `Deny` (fail
/// closed), with no determining and no errored policy, and `given` holds the `detector_error`
/// that says why. The action group comes from the policies, with no Cedar decision.
pub(super) fn unevaluated_detail(given: &[Finding], name: &str, policies: &ToolPolicies, facts: &Facts) -> CedarDetail {
    let max_severity = given.iter().map(|f| f.severity).max().filter(|_| facts.detection);
    let mut cedar = CedarDetail::new(
        Effect::Deny,
        Vec::new(),
        Vec::new(),
        false,
        given.iter().map(FindingRecord::from_finding).collect(),
        max_severity,
        Declared::Unknown,
        policies.action_group(name),
        Duration::ZERO,
    );
    cedar.detection_disabled = !facts.detection;
    cedar
}

fn basis_of(v: &Verdict) -> PolicyBasis {
    if v.errored_forbid { PolicyBasis::ErroredForbid } else { PolicyBasis::Policy }
}

/// The gateway decision for one kernel verdict under the snapshot's tenant part and mode.
fn outcome(v: Verdict, cedar: CedarDetail, tenant: TenantPart, mode: TenantMode, on: OnUnavailable) -> CallOutcome {
    // D33: a floor deny is enforced in every tenant mode. The `if` below is the one statement
    // that the red-first proof deletes (preflight F1).
    // FLOOR-RULE begin
    if v.floor_denied {
        return CallOutcome {
            decision: Decision::Block { code: BlockCode::Policy },
            basis: basis_of(&v),
            verdict: Some(v),
            cedar: Some(cedar),
        };
    }
    // FLOOR-RULE end
    let (decision, basis) = match (tenant, mode) {
        // Mode `off`: the floor alone applies, and its default deny does not count (P-4).
        (TenantPart::Off, _) | (_, TenantMode::Off) => (Decision::Allow, basis_of(&v)),
        (TenantPart::Unavailable, TenantMode::LogOnly) => (Decision::Observe { would: BlockCode::PolicyUnavailable }, PolicyBasis::Unavailable),
        (TenantPart::Unavailable, _) => (unavailable(on), PolicyBasis::Unavailable),
        (TenantPart::Evaluated, _) if v.decision == Effect::Allow => (Decision::Allow, basis_of(&v)),
        (TenantPart::Evaluated, TenantMode::LogOnly) => (Decision::Observe { would: BlockCode::Policy }, basis_of(&v)),
        (TenantPart::Evaluated, _) => (Decision::Block { code: BlockCode::Policy }, basis_of(&v)),
    };
    CallOutcome {
        decision,
        basis,
        verdict: Some(v),
        cedar: Some(cedar),
    }
}

/// The detector output of one call (§ 6.9): the findings of `detect_call` and the hosts of its
/// arguments. `hosts` is `None` when no scan read them; the kernel then reads them itself.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct CallScan {
    pub findings: Vec<Finding>,
    pub hosts: Option<HostSet>,
}

impl CallScan {
    /// A scan with these findings and hosts.
    #[must_use]
    pub fn new(findings: Vec<Finding>, hosts: Option<HostSet>) -> Self {
        Self { findings, hosts }
    }
}

/// How the scans of `decide_calls` map to its calls.
#[derive(Debug, Clone, Copy)]
enum ScansFor<'a> {
    /// No detector ran: no scan findings, and the kernel reads the hosts.
    None,
    /// One scan per call, in the order of the calls.
    PerCall(&'a [CallScan]),
    /// The count is wrong, so no scan can be matched to its call. Every call gets one
    /// `detector_error` (detector `built_in`, high) in place of the scans (A3).
    Mismatch,
}

fn scans_for(calls: usize, scans: &[CallScan]) -> ScansFor<'_> {
    if scans.is_empty() {
        ScansFor::None
    } else if scans.len() == calls {
        ScansFor::PerCall(scans)
    } else {
        ScansFor::Mismatch
    }
}

/// One outcome per call, in order. The findings of a call are its request-level findings
/// (`name_collision`) and the findings of its scan; the kernel adds `unparsed_arguments` and the
/// hosts-cap `detector_error` itself. The Cedar detail records all of them, each with its
/// severity and detector, and the `max_severity` of the verdict.
///
/// `scans` is empty (no detectors ran) or has one entry per call, in the order of `calls`. When a
/// scan has hosts, the invocation carries them, so the kernel does not read the arguments again
/// (S8). Any other count is a defect of the caller: a debug build panics, and a release build
/// uses no scan and gives every call a `detector_error` finding (detector `built_in`, high).
///
/// `now` is the only time source (P-7): it is read before and after each kernel `decide`, and the
/// difference is the `decide_duration` of the call. The core reads no clock of its own.
#[must_use]
pub fn decide_calls(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], scans: &[CallScan], now: &dyn Fn() -> Instant) -> Vec<CallOutcome> {
    decide_calls_with(snapshot, turn, calls, scans, &Findings::new(), now)
}

/// `decide_calls` with request-level findings (`extra`) that every call of the hold carries, in
/// addition to its own findings (SMA-753). Each one has the detector `kernel` and the severity
/// that `extra` holds for it.
#[must_use]
pub fn decide_calls_with(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], scans: &[CallScan], extra: &Findings, now: &dyn Fn() -> Instant) -> Vec<CallOutcome> {
    debug_assert!(scans.is_empty() || scans.len() == calls.len(), "one scan per call, or none");
    decide_with(snapshot, turn, calls, scans_for(calls.len(), scans), extra, now)
}

/// `decide_calls_with` after the count check. It is also the release path of a wrong count.
fn decide_with(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], scans: ScansFor<'_>, extra: &Findings, now: &dyn Fn() -> Instant) -> Vec<CallOutcome> {
    let PolicySet::Available { policies, tenant, facts } = &snapshot.set else {
        // P-6: `closed` blocks in every tenant mode, `off` included.
        return calls
            .iter()
            .map(|_| CallOutcome {
                decision: unavailable(snapshot.on_unavailable),
                basis: PolicyBasis::Unavailable,
                verdict: None,
                cedar: None,
            })
            .collect();
    };
    let collisions = match turn {
        ToolTurn::Parsed(f) => name_collisions(&f.definitions),
        ToolTurn::Unavailable(_) => BTreeSet::new(),
    };
    calls
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut inv = invocation(c, turn);
            let mut given = call_findings(c, &collisions);
            match scans {
                ScansFor::None => {}
                ScansFor::PerCall(s) => {
                    if let Some(hosts) = s[i].hosts.clone() {
                        inv = inv.with_hosts(hosts);
                    }
                    given.extend(s[i].findings.iter().cloned());
                }
                ScansFor::Mismatch => given.push(Finding::new(DETECTOR_BUILT_IN, DETECTOR_ERROR, Severity::High, "")),
            }
            for kind in extra.iter() {
                // `iter` yields only kinds that the set holds, so the fallback never applies.
                let severity = extra.severity(kind).unwrap_or(Severity::High);
                given.push(Finding::new(DETECTOR_KERNEL, kind, severity, ""));
            }
            let started = now();
            let v = decide(&inv, facts, &kernel_findings(&given), policies);
            let duration = now().saturating_duration_since(started);
            let cedar = cedar_detail(&v, &given, inv.declared, facts, duration);
            outcome(v, cedar, *tenant, snapshot.mode, snapshot.on_unavailable)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::detect::SECRET_IN_ARGS;
    use paigasus_kernel::tool_policy::{RequestInfo, SessionInfo, ToolPolicies};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use uuid::Uuid;

    use super::*;
    use crate::policy::SHIPPED_FLOOR;
    use crate::turn::{ToolDefinition, TurnFacts};
    use crate::types::{Format, ToolCallKind};

    const FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"danger", resource);"#;
    const TENANT: &str = r#"
        @id("all") permit(principal, action, resource);
        @id("t") forbid(principal, action == Pgs::Gateway::Action::"tdeny", resource);
        @id("e") forbid(principal, action == Pgs::Gateway::Action::"err", resource) when { context.input.missing == 1 };
    "#;

    fn facts() -> Facts {
        let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
        Facts::new(
            to_cedar_uid(&prn),
            Uuid::nil(),
            RequestInfo::new("prn:pgs:gateway:::scope/x", crate::policy::ingress(Format::OpenAiChat), "m"),
            SessionInfo::placeholder(),
        )
    }

    /// The compiled set for a tenant part: floor and tenant when evaluated, else the floor alone (P-4).
    fn available(floor: &str, tenant: TenantPart) -> PolicySet {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let text = if tenant == TenantPart::Evaluated { TENANT } else { "" };
        PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(floor, text, &[]).expect("compile")),
            tenant,
            facts: facts(),
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            choice: 0,
            position: 0,
            id: None,
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Function,
            arguments: "{}".to_owned(),
        }
    }

    fn one(set: PolicySet, mode: TenantMode, on: OnUnavailable, tool: &str) -> CallOutcome {
        let snap = PolicySnapshot::new(set, mode, on);
        let mut out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call(tool)], &[], &Instant::now);
        assert_eq!(out.len(), 1);
        out.remove(0)
    }

    const MODES: [TenantMode; 3] = [TenantMode::Off, TenantMode::LogOnly, TenantMode::Enforce];
    const ONS: [OnUnavailable; 2] = [OnUnavailable::Open, OnUnavailable::Closed];
    const BLOCK_POLICY: Decision = Decision::Block { code: BlockCode::Policy };
    const BLOCK_UNAVAILABLE: Decision = Decision::Block { code: BlockCode::PolicyUnavailable };
    const OBSERVE_POLICY: Decision = Decision::Observe { would: BlockCode::Policy };
    const OBSERVE_UNAVAILABLE: Decision = Decision::Observe { would: BlockCode::PolicyUnavailable };

    #[test]
    fn the_section_6_4_table() {
        // Unavailable, any mode incl. Off: Closed blocks, Open observes (P-6).
        for mode in MODES {
            let c = one(PolicySet::Unavailable, mode, OnUnavailable::Closed, "ok");
            assert_eq!((c.decision, c.basis, c.verdict.is_none()), (BLOCK_UNAVAILABLE, PolicyBasis::Unavailable, true), "{mode:?}");
            let o = one(PolicySet::Unavailable, mode, OnUnavailable::Open, "ok");
            assert_eq!((o.decision, o.basis), (OBSERVE_UNAVAILABLE, PolicyBasis::Unavailable), "{mode:?}");
            assert!(o.decision.is_allow());
            assert!(o.verdict.is_none(), "{mode:?}");
        }
        // Available, floor_denied, any mode and any tenant part: Block{Policy} (D33).
        for tenant in [TenantPart::Evaluated, TenantPart::Off, TenantPart::Unavailable] {
            for mode in MODES {
                for on in ONS {
                    let c = one(available(FLOOR, tenant), mode, on, "danger");
                    assert_eq!(c.decision, BLOCK_POLICY, "{tenant:?} {mode:?} {on:?}");
                    assert_eq!(c.basis, PolicyBasis::Policy);
                    assert!(c.verdict.expect("verdict").floor_denied);
                }
            }
        }
        // Available + Off: the floor-alone default deny does not count (P-4).
        for on in ONS {
            let c = one(available(FLOOR, TenantPart::Off), TenantMode::Off, on, "ok");
            assert_eq!(c.decision, Decision::Allow, "{on:?}");
            assert_eq!(c.verdict.expect("verdict").decision, Effect::Deny);
        }
        // Mode Off ignores the tenant part (controller ruling): a deny that is not a floor deny
        // is an allow, whatever the tenant part.
        for on in ONS {
            for tool in ["tdeny", "err"] {
                let c = one(available(FLOOR, TenantPart::Evaluated), TenantMode::Off, on, tool);
                assert_eq!(c.decision, Decision::Allow, "Evaluated {tool} {on:?}");
                assert_eq!(c.verdict.expect("verdict").decision, Effect::Deny, "{tool}");
            }
            let c = one(available(FLOOR, TenantPart::Unavailable), TenantMode::Off, on, "ok");
            assert_eq!(c.decision, Decision::Allow, "Unavailable ok {on:?}");
        }
        // Available + tenant Unavailable, not floor-denied.
        let rows = [
            (TenantMode::LogOnly, OnUnavailable::Closed, OBSERVE_UNAVAILABLE),
            (TenantMode::LogOnly, OnUnavailable::Open, OBSERVE_UNAVAILABLE),
            (TenantMode::Enforce, OnUnavailable::Closed, BLOCK_UNAVAILABLE),
            (TenantMode::Enforce, OnUnavailable::Open, OBSERVE_UNAVAILABLE),
        ];
        for (mode, on, want) in rows {
            let c = one(available(FLOOR, TenantPart::Unavailable), mode, on, "ok");
            assert_eq!((c.decision, c.basis), (want, PolicyBasis::Unavailable), "{mode:?} {on:?}");
        }
        // Available + Evaluated.
        for on in ONS {
            for mode in [TenantMode::LogOnly, TenantMode::Enforce] {
                assert_eq!(one(available(FLOOR, TenantPart::Evaluated), mode, on, "ok").decision, Decision::Allow);
            }
            assert_eq!(one(available(FLOOR, TenantPart::Evaluated), TenantMode::LogOnly, on, "tdeny").decision, OBSERVE_POLICY);
            assert_eq!(one(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, on, "tdeny").decision, BLOCK_POLICY);
        }
    }

    #[test]
    fn log_only_never_blocks_a_tenant_deny() {
        for (tool, basis) in [("tdeny", PolicyBasis::Policy), ("err", PolicyBasis::ErroredForbid)] {
            let c = one(available(FLOOR, TenantPart::Evaluated), TenantMode::LogOnly, OnUnavailable::Closed, tool);
            assert_eq!((c.decision, c.basis), (OBSERVE_POLICY, basis), "{tool}");
            assert!(c.decision.is_allow(), "{tool}");
        }
    }

    #[test]
    fn a_floor_errored_forbid_blocks_with_its_own_basis() {
        const ERRORING_FLOOR: &str = r#"@id("fe") forbid(principal, action == Pgs::Gateway::Action::"ferr", resource) when { context.input.missing == 1 };"#;
        for tenant in [TenantPart::Evaluated, TenantPart::Off, TenantPart::Unavailable] {
            for mode in MODES {
                for on in ONS {
                    let c = one(available(ERRORING_FLOOR, tenant), mode, on, "ferr");
                    assert_eq!((c.decision, c.basis), (BLOCK_POLICY, PolicyBasis::ErroredForbid), "{tenant:?} {mode:?} {on:?}");
                    let v = c.verdict.expect("verdict");
                    assert!(v.floor_denied && v.errored_forbid);
                }
            }
        }
    }

    #[test]
    fn an_errored_forbid_has_its_own_basis() {
        let c = one(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Open, "err");
        assert_eq!((c.decision, c.basis), (BLOCK_POLICY, PolicyBasis::ErroredForbid));
        let v = c.verdict.expect("verdict");
        assert!(v.errored_forbid);
        assert!(!v.floor_denied);
    }

    #[test]
    fn one_outcome_per_call_in_order() {
        let snap = PolicySnapshot::new(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Closed);
        let out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call("ok"), call("danger"), call("tdeny")], &[], &Instant::now);
        assert_eq!(out.iter().map(|o| o.decision).collect::<Vec<_>>(), [Decision::Allow, BLOCK_POLICY, BLOCK_POLICY]);
        let floor_hit: Vec<bool> = out.iter().map(|o| o.verdict.as_ref().expect("verdict").floor_denied).collect();
        assert_eq!(floor_hit, [false, true, false]);
        let unavailable = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
        assert_eq!(
            decide_calls(&unavailable, &ToolTurn::Unavailable("test"), &[call("a"), call("b"), call("c")], &[], &Instant::now).len(),
            3
        );
    }

    #[test]
    fn extra_findings_reach_every_call_of_the_hold() {
        const TENANT_FORGED: &str = r#"
            @id("all") permit(principal, action, resource);
            @id("forged") forbid(principal, action, resource) when { context.findings.contains("forged_history") };
        "#;
        let floor = Arc::new(ToolPolicies::compile_floor(FLOOR).expect("floor"));
        let set = PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(floor, TENANT_FORGED, &[]).expect("compile")),
            tenant: TenantPart::Evaluated,
            facts: facts(),
        };
        let snap = PolicySnapshot::new(set, TenantMode::Enforce, OnUnavailable::Closed);
        let turn = ToolTurn::Unavailable("test");
        let calls = [call("a"), call("b")];
        let mut extra = Findings::new();
        extra.add(paigasus_kernel::detect::FORGED_HISTORY, Severity::High);
        let blocked = decide_calls_with(&snap, &turn, &calls, &[], &extra, &Instant::now);
        assert_eq!(blocked.iter().map(|o| o.decision).collect::<Vec<_>>(), [BLOCK_POLICY, BLOCK_POLICY]);
        // Each call records the extra finding with the detector `kernel` and its severity.
        for o in &blocked {
            let records = &o.cedar.as_ref().expect("cedar").findings;
            assert_eq!(records, &[crate::decision_log::FindingRecord::new("forged_history", Some(Severity::High), DETECTOR_KERNEL, "")]);
        }
        let allowed = decide_calls(&snap, &turn, &calls, &[], &Instant::now);
        assert_eq!(allowed.iter().map(|o| o.decision).collect::<Vec<_>>(), [Decision::Allow, Decision::Allow]);
    }

    #[test]
    fn the_shipped_collision_floor_blocks_both_colliding_calls() {
        let def = |name: &str| ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Function,
            schema: None,
        };
        let turn = ToolTurn::Parsed(TurnFacts {
            definitions: vec![def("send"), def("Send"), def("other")],
            definitions_complete: true,
            ..TurnFacts::default()
        });
        let snap = PolicySnapshot::new(available(SHIPPED_FLOOR, TenantPart::Off), TenantMode::Off, OnUnavailable::Open);
        let out = decide_calls(&snap, &turn, &[call("send"), call("Send"), call("other")], &[], &Instant::now);
        assert_eq!(out.iter().map(|o| o.decision).collect::<Vec<_>>(), [BLOCK_POLICY, BLOCK_POLICY, Decision::Allow]);
    }

    fn colliding_turn() -> ToolTurn {
        let def = |name: &str| ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Function,
            schema: None,
        };
        ToolTurn::Parsed(TurnFacts {
            definitions: vec![def("send"), def("Send")],
            definitions_complete: true,
            ..TurnFacts::default()
        })
    }

    fn kinds_and_severities(o: &CallOutcome) -> Vec<(&str, Option<Severity>, &str)> {
        let c = o.cedar.as_ref().expect("cedar");
        c.findings.iter().map(|f| (f.kind.as_str(), f.severity, f.detector.as_str())).collect()
    }

    #[test]
    fn the_response_record_keeps_the_name_collision_and_the_kernel_findings() {
        let snap = PolicySnapshot::new(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Closed);
        let mut bad = call("send");
        bad.arguments = "[1]".to_owned();
        let out = decide_calls(&snap, &colliding_turn(), &[bad], &[], &Instant::now);
        assert_eq!(
            kinds_and_severities(&out[0]),
            [("name_collision", Some(Severity::High), "kernel"), ("unparsed_arguments", Some(Severity::Medium), "kernel")]
        );
        assert_eq!(out[0].cedar.as_ref().expect("cedar").max_severity, Some(Severity::High));
        assert!(!out[0].cedar.as_ref().expect("cedar").detection_disabled);
    }

    #[test]
    fn with_detection_off_the_record_keeps_the_severities_and_no_max_severity() {
        let PolicySet::Available { policies, tenant, facts } = available(FLOOR, TenantPart::Evaluated) else {
            unreachable!("available() is always Available")
        };
        let set = PolicySet::Available {
            policies,
            tenant,
            facts: facts.with_detection(false),
        };
        let snap = PolicySnapshot::new(set, TenantMode::Enforce, OnUnavailable::Closed);
        let mut bad = call("send");
        bad.arguments = "[1]".to_owned();
        let out = decide_calls(&snap, &colliding_turn(), &[bad], &[], &Instant::now);
        let c = out[0].cedar.as_ref().expect("cedar");
        assert_eq!(c.max_severity, out[0].verdict.as_ref().expect("verdict").max_severity);
        assert_eq!(c.max_severity, None);
        assert!(c.detection_disabled);
        assert_eq!(
            kinds_and_severities(&out[0]),
            [("name_collision", Some(Severity::High), "kernel"), ("unparsed_arguments", Some(Severity::Medium), "kernel")]
        );
    }

    #[test]
    fn decide_calls_attaches_the_cedar_detail_and_the_duration() {
        use std::cell::Cell;
        use std::time::Duration;

        let t = Instant::now();
        // The clock reads four times for two calls: t, t+3ms, t+5ms, t+6ms.
        let ticks = [0_u64, 3, 5, 6];
        let next = Cell::new(0_usize);
        let clock = || {
            let i = next.get();
            next.set(i + 1);
            t + Duration::from_millis(ticks[i])
        };
        let snap = PolicySnapshot::new(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Closed);
        let turn = ToolTurn::Parsed(TurnFacts {
            definitions: vec![ToolDefinition {
                name: "ok".to_owned(),
                namespace: None,
                kind: ToolCallKind::Function,
                schema: None,
            }],
            definitions_complete: true,
            ..TurnFacts::default()
        });
        let out = decide_calls(&snap, &turn, &[call("ok"), call("danger")], &[], &clock);
        assert_eq!(next.get(), 4, "two reads per call");
        assert_eq!(out.len(), 2);
        let durations: Vec<Duration> = out.iter().map(|o| o.cedar.as_ref().expect("cedar").decide_duration).collect();
        assert_eq!(durations, [Duration::from_millis(3), Duration::from_millis(1)]);
        for o in &out {
            let v = o.verdict.as_ref().expect("verdict");
            let c = o.cedar.as_ref().expect("cedar");
            assert_eq!(c.findings.iter().map(|f| f.kind.clone()).collect::<BTreeSet<_>>(), v.findings);
            assert_eq!(c.max_severity, v.max_severity);
            assert_eq!((c.effect, c.floor_denied, c.action_group), (v.decision, v.floor_denied, v.action_group));
            assert_eq!((&c.determining, &c.errored), (&v.determining, &v.errored));
        }
        // `declared` comes from the turn: `ok` is a definition, `danger` is not.
        assert_eq!(out[0].cedar.as_ref().expect("cedar").declared, Declared::Yes);
        assert_eq!(out[1].cedar.as_ref().expect("cedar").declared, Declared::No);
        assert!(out[1].cedar.as_ref().expect("cedar").floor_denied);
    }

    #[test]
    fn an_unavailable_set_has_no_cedar_detail() {
        let calls = [call("a"), call("b")];
        for mode in MODES {
            for on in ONS {
                let snap = PolicySnapshot::new(PolicySet::Unavailable, mode, on);
                let out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &calls, &[], &Instant::now);
                assert_eq!(out.len(), 2);
                assert!(out.iter().all(|o| o.cedar.is_none() && o.verdict.is_none()), "{mode:?} {on:?}");
            }
        }
    }

    /// A snapshot whose tenant part is `tenant` (enforced) over the test floor.
    fn tenant_snapshot(tenant: &str) -> PolicySnapshot {
        let floor = Arc::new(ToolPolicies::compile_floor(FLOOR).expect("floor"));
        let set = PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(floor, tenant, &[]).expect("compile")),
            tenant: TenantPart::Evaluated,
            facts: facts(),
        };
        PolicySnapshot::new(set, TenantMode::Enforce, OnUnavailable::Closed)
    }

    fn secret() -> Finding {
        Finding::new(SECRET_IN_ARGS, SECRET_IN_ARGS, Severity::High, "v1")
    }

    const FORBID_HIGH: &str = r#"
        @id("all") permit(principal, action, resource);
        @id("high") forbid(principal, action, resource) when { context.max_severity >= 3 };
    "#;

    #[test]
    fn scan_findings_reach_the_decision_and_the_record() {
        let snap = tenant_snapshot(FORBID_HIGH);
        let turn = ToolTurn::Unavailable("test");
        let o = &decide_calls(&snap, &turn, &[call("t")], &[CallScan::new(vec![secret()], None)], &Instant::now)[0];
        assert_eq!(o.decision, BLOCK_POLICY);
        let c = o.cedar.as_ref().expect("cedar");
        assert_eq!(
            (c.max_severity, c.findings[0].kind.as_str(), c.findings[0].detector.as_str()),
            (Some(Severity::High), SECRET_IN_ARGS, SECRET_IN_ARGS)
        );
        assert!(o.verdict.as_ref().expect("verdict").findings.contains(SECRET_IN_ARGS));
        // The same call with no scan is allowed: the scan finding made the deny.
        assert_eq!(decide_calls(&snap, &turn, &[call("t")], &[], &Instant::now)[0].decision, Decision::Allow);
    }

    #[test]
    fn given_hosts_are_used() {
        let snap = tenant_snapshot(r#"@id("h") permit(principal, action, resource) when { context.call.hosts == ["x.example"] };"#);
        let turn = ToolTurn::Unavailable("test");
        let mut hosts = HostSet::new();
        hosts.insert("x.example");
        // The arguments are `{}`, so a kernel scan finds no host: only the given set can permit.
        let o = &decide_calls(&snap, &turn, &[call("t")], &[CallScan::new(Vec::new(), Some(hosts))], &Instant::now)[0];
        assert_eq!(o.decision, Decision::Allow);
        assert_eq!(decide_calls(&snap, &turn, &[call("t")], &[CallScan::default()], &Instant::now)[0].decision, BLOCK_POLICY);
    }

    #[test]
    fn no_scans_keeps_today() {
        let snap = PolicySnapshot::new(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Closed);
        let calls = [call("ok"), call("danger"), call("tdeny")];
        let turn = ToolTurn::Unavailable("test");
        let none = decide_calls(&snap, &turn, &calls, &[], &Instant::now);
        let empty = decide_calls(&snap, &turn, &calls, &[CallScan::default(), CallScan::default(), CallScan::default()], &Instant::now);
        let strip = |v: Vec<CallOutcome>| v.into_iter().map(|o| (o.decision, o.basis, o.verdict)).collect::<Vec<_>>();
        assert_eq!(strip(none), strip(empty));
    }

    #[test]
    fn the_record_keeps_the_collision_the_kernel_and_the_scan_findings() {
        let snap = PolicySnapshot::new(available(FLOOR, TenantPart::Evaluated), TenantMode::Enforce, OnUnavailable::Closed);
        let mut bad = call("send");
        bad.arguments = "[1]".to_owned();
        let scan = CallScan::new(vec![Finding::new(DETECTOR_BUILT_IN, "other", Severity::Low, "")], None);
        let out = decide_calls(&snap, &colliding_turn(), &[bad], &[scan], &Instant::now);
        assert_eq!(
            kinds_and_severities(&out[0]),
            [
                ("name_collision", Some(Severity::High), "kernel"),
                ("other", Some(Severity::Low), "built_in"),
                ("unparsed_arguments", Some(Severity::Medium), "kernel")
            ]
        );
    }

    #[test]
    fn a_wrong_scan_count_gives_every_call_a_detector_error() {
        assert!(matches!(scans_for(2, &[]), ScansFor::None));
        assert!(matches!(scans_for(1, &[CallScan::default()]), ScansFor::PerCall(_)));
        let mut hosts = HostSet::new();
        hosts.insert("x.example");
        let one = [CallScan::new(vec![Finding::new(DETECTOR_BUILT_IN, "other", Severity::Low, "")], Some(hosts))];
        for (n, scans) in [(2, &one[..]), (0, &one[..])] {
            assert!(matches!(scans_for(n, scans), ScansFor::Mismatch), "{n}");
        }
        // The release path: `decide_calls` checks the count with a `debug_assert!` first.
        let snap = tenant_snapshot(FORBID_HIGH);
        let turn = ToolTurn::Unavailable("test");
        let out = decide_with(&snap, &turn, &[call("a"), call("b")], scans_for(2, &one), &Findings::new(), &Instant::now);
        assert_eq!(out.len(), 2);
        for o in &out {
            assert_eq!(o.decision, BLOCK_POLICY);
            // No scan finding and no scan host reaches any call: only the error.
            assert_eq!(kinds_and_severities(o), [(DETECTOR_ERROR, Some(Severity::High), DETECTOR_BUILT_IN)]);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "one scan per call, or none")]
    fn a_wrong_scan_count_panics_in_a_debug_build() {
        let snap = tenant_snapshot(FORBID_HIGH);
        let _ = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call("a"), call("b")], &[CallScan::default()], &Instant::now);
    }

    #[test]
    fn a_call_scan_debug_prints_counts_only() {
        let mut hosts = HostSet::new();
        hosts.insert("canary.example");
        let d = format!("{:?}", CallScan::new(vec![secret()], Some(hosts)));
        assert!(!d.contains("canary"), "{d}");
    }

    #[test]
    fn basis_spellings() {
        assert_eq!(PolicyBasis::Policy.as_str(), "policy");
        assert_eq!(PolicyBasis::ErroredForbid.as_str(), "errored_forbid");
        assert_eq!(PolicyBasis::Unavailable.as_str(), "unavailable");
    }
}
