// SPDX-License-Identifier: Apache-2.0
//! The decisions of the calls of one hold, from a policy snapshot (§ 6.4, D33, D34, P-4 to P-6).

use std::collections::BTreeSet;

use paigasus_kernel::tool_policy::{Effect, Verdict, decide};

use super::mapping::{call_findings, invocation, name_collisions};
use super::snapshot::{OnUnavailable, PolicySet, PolicySnapshot, TenantMode, TenantPart};
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
}

fn unavailable(on: OnUnavailable) -> Decision {
    match on {
        OnUnavailable::Closed => Decision::Block { code: BlockCode::PolicyUnavailable },
        OnUnavailable::Open => Decision::Observe { would: BlockCode::PolicyUnavailable },
    }
}

fn basis_of(v: &Verdict) -> PolicyBasis {
    if v.errored_forbid { PolicyBasis::ErroredForbid } else { PolicyBasis::Policy }
}

/// The gateway decision for one kernel verdict under the snapshot's tenant part and mode.
fn outcome(v: Verdict, tenant: TenantPart, mode: TenantMode, on: OnUnavailable) -> CallOutcome {
    // D33: a floor deny is enforced in every tenant mode. The `if` below is the one statement
    // that the red-first proof deletes (preflight F1). // FLOOR-RULE begin
    if v.floor_denied {
        return CallOutcome {
            decision: Decision::Block { code: BlockCode::Policy },
            basis: basis_of(&v),
            verdict: Some(v),
        };
    } // FLOOR-RULE end
    let (decision, basis) = match (tenant, mode) {
        // Mode `off`: the floor alone applies, and its default deny does not count (P-4).
        (TenantPart::Off, _) | (_, TenantMode::Off) => (Decision::Allow, basis_of(&v)),
        (TenantPart::Unavailable, TenantMode::LogOnly) => (Decision::Observe { would: BlockCode::PolicyUnavailable }, PolicyBasis::Unavailable),
        (TenantPart::Unavailable, _) => (unavailable(on), PolicyBasis::Unavailable),
        (TenantPart::Evaluated, _) if v.decision == Effect::Allow => (Decision::Allow, basis_of(&v)),
        (TenantPart::Evaluated, TenantMode::LogOnly) => (Decision::Observe { would: BlockCode::Policy }, basis_of(&v)),
        (TenantPart::Evaluated, _) => (Decision::Block { code: BlockCode::Policy }, basis_of(&v)),
    };
    CallOutcome { decision, basis, verdict: Some(v) }
}

/// One outcome per call, in order. The findings of a call are its request-level findings
/// (`name_collision`); the kernel adds `unparsed_arguments` itself.
#[must_use]
pub fn decide_calls(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall]) -> Vec<CallOutcome> {
    let PolicySet::Available { policies, tenant, facts } = &snapshot.set else {
        // P-6: `closed` blocks in every tenant mode, `off` included.
        return calls
            .iter()
            .map(|_| CallOutcome {
                decision: unavailable(snapshot.on_unavailable),
                basis: PolicyBasis::Unavailable,
                verdict: None,
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
            let v = decide(&invocation(c, turn), facts, &call_findings(c, &collisions), policies);
            outcome(v, *tenant, snapshot.mode, snapshot.on_unavailable)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies};
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
        let prn = Prn::parse("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004").expect("prn");
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
        let mut out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call(tool)]);
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
        for tool in ["tdeny", "err"] {
            let c = one(available(FLOOR, TenantPart::Evaluated), TenantMode::LogOnly, OnUnavailable::Closed, tool);
            assert!(c.decision.is_allow(), "{tool}");
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
        let out = decide_calls(&snap, &ToolTurn::Unavailable("test"), &[call("ok"), call("danger"), call("tdeny")]);
        assert_eq!(out.iter().map(|o| o.decision).collect::<Vec<_>>(), [Decision::Allow, BLOCK_POLICY, BLOCK_POLICY]);
        let floor_hit: Vec<bool> = out.iter().map(|o| o.verdict.as_ref().expect("verdict").floor_denied).collect();
        assert_eq!(floor_hit, [false, true, false]);
        let unavailable = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
        assert_eq!(decide_calls(&unavailable, &ToolTurn::Unavailable("test"), &[call("a"), call("b"), call("c")]).len(), 3);
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
        let out = decide_calls(&snap, &turn, &[call("send"), call("Send"), call("other")]);
        assert_eq!(out.iter().map(|o| o.decision).collect::<Vec<_>>(), [BLOCK_POLICY, BLOCK_POLICY, Decision::Allow]);
    }

    #[test]
    fn basis_spellings() {
        assert_eq!(PolicyBasis::Policy.as_str(), "policy");
        assert_eq!(PolicyBasis::ErroredForbid.as_str(), "errored_forbid");
        assert_eq!(PolicyBasis::Unavailable.as_str(), "unavailable");
    }
}
