// SPDX-License-Identifier: Apache-2.0
//! The decisions of the calls of one hold, from a policy snapshot (§ 6.4, D33, D34, P-4 to P-6).

use std::collections::BTreeSet;
use std::time::Instant;

use paigasus_kernel::detect::Finding;
use paigasus_kernel::tool_policy::{Effect, Findings, Verdict, decide};

use super::mapping::{call_findings, invocation, name_collisions};
use super::snapshot::{OnUnavailable, PolicySet, PolicySnapshot, TenantMode, TenantPart};
use crate::decision_log::{CedarDetail, records_of};
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
fn kernel_findings(given: &[Finding]) -> Findings {
    let mut set = Findings::new();
    for f in given {
        set.add(&f.kind, f.severity);
    }
    set
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

/// One outcome per call, in order. The findings of a call are its request-level findings
/// (`name_collision`); the kernel adds `unparsed_arguments` itself. The Cedar detail records
/// both, with their severities, and the `max_severity` of the verdict.
///
/// `now` is the only time source (P-7): it is read before and after each kernel `decide`, and the
/// difference is the `decide_duration` of the call. The core reads no clock of its own.
#[must_use]
pub fn decide_calls(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], now: &dyn Fn() -> Instant) -> Vec<CallOutcome> {
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
        .map(|c| {
            let inv = invocation(c, turn);
            let started = now();
            let given = call_findings(c, &collisions);
            let v = decide(&inv, facts, &kernel_findings(&given), policies);
            let duration = now().saturating_duration_since(started);
            let cedar = CedarDetail::new(
                v.decision,
                v.determining.clone(),
                v.errored.clone(),
                v.floor_denied,
                records_of(&given, &v),
                v.max_severity,
                inv.declared,
                v.action_group,
                duration,
            );
            outcome(v, cedar, *tenant, snapshot.mode, snapshot.on_unavailable)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::detect::Severity;
    use paigasus_kernel::tool_policy::{Declared, Facts, RequestInfo, SessionInfo, ToolPolicies};
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
        let mut out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call(tool)], &Instant::now);
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
        let out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call("ok"), call("danger"), call("tdeny")], &Instant::now);
        assert_eq!(out.iter().map(|o| o.decision).collect::<Vec<_>>(), [Decision::Allow, BLOCK_POLICY, BLOCK_POLICY]);
        let floor_hit: Vec<bool> = out.iter().map(|o| o.verdict.as_ref().expect("verdict").floor_denied).collect();
        assert_eq!(floor_hit, [false, true, false]);
        let unavailable = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
        assert_eq!(decide_calls(&unavailable, &ToolTurn::Unavailable("test"), &[call("a"), call("b"), call("c")], &Instant::now).len(), 3);
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
        let out = decide_calls(&snap, &turn, &[call("send"), call("Send"), call("other")], &Instant::now);
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
        let out = decide_calls(&snap, &colliding_turn(), &[bad], &Instant::now);
        assert_eq!(
            kinds_and_severities(&out[0]),
            [("name_collision", Some(Severity::High), "kernel"), ("unparsed_arguments", Some(Severity::Medium), "kernel")]
        );
        assert_eq!(out[0].cedar.as_ref().expect("cedar").max_severity, Some(Severity::High));
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
        let out = decide_calls(&snap, &colliding_turn(), &[bad], &Instant::now);
        let c = out[0].cedar.as_ref().expect("cedar");
        assert_eq!(c.max_severity, out[0].verdict.as_ref().expect("verdict").max_severity);
        assert_eq!(c.max_severity, None);
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
        let out = decide_calls(&snap, &turn, &[call("ok"), call("danger")], &clock);
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
                let out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &calls, &Instant::now);
                assert_eq!(out.len(), 2);
                assert!(out.iter().all(|o| o.cedar.is_none() && o.verdict.is_none()), "{mode:?} {on:?}");
            }
        }
    }

    #[test]
    fn basis_spellings() {
        assert_eq!(PolicyBasis::Policy.as_str(), "policy");
        assert_eq!(PolicyBasis::ErroredForbid.as_str(), "errored_forbid");
        assert_eq!(PolicyBasis::Unavailable.as_str(), "unavailable");
    }
}
