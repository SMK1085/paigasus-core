// SPDX-License-Identifier: Apache-2.0

//! SMA-753 spec P7: the history slots of one hold, with the failure rules. One pure function,
//! [`hold_slots`], gives the slots of every call of the hold once, before any decision (spec P1),
//! and says whether the tenant part is unavailable for the hold.
//!
//! A missing slot is listed once for each call whose decision lacks it, so the metric
//! `gateway_tool_history_slots_missing_total{part, reason}` counts one per call and slot.

use metrics::counter;
use paigasus_gateway_core::HistoryPolicy;
use paigasus_kernel::tool_policy::history::{Missing, MissingReason, Part, Slots, evaluate};
use paigasus_observability::names;

use crate::application::tool_history::{ReserveOutcome, WindowRead};

/// What the history store gave the decider for one hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOutcome {
    /// The gateway has no `[tool_history]` table: the decider has no history.
    Disabled,
    /// The request has no session, because its lookup failed.
    NoSession,
    /// The reserve ran (or failed): its outcome.
    Read(ReserveOutcome),
}

/// The slots of one hold (spec P7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HoldSlots {
    /// One entry per call, in call order. Empty when the policy has no condition.
    pub per_call: Vec<Slots>,
    /// The tenant part is unavailable for this hold: decide it on the floor-only set.
    pub tenant_unavailable: bool,
    /// Each missing slot, once per call whose decision lacks it.
    pub missing: Vec<Missing>,
}

/// P7 as one pure function. `calls` is the hold length.
///
/// - No policy, or no condition: no slot, no effect (design A6).
/// - `Disabled`: every slot is missing (`disabled`). The tenant set is still enforced.
/// - `NoSession`, or a failed reserve (`store`): every slot is missing with that reason, and the
///   tenant part is unavailable when it has a condition. Its slots then count as missing too.
/// - A read window: the kernel evaluator gives the slots of each call. A truncated set, an unknown
///   event or an unknown catalog misses only the slots that need it.
/// - `NotNeeded` with a condition cannot happen (`ReadPlan::of` gives a plan for any condition).
///   It is treated as `Failed` (fail closed).
#[must_use]
pub fn hold_slots(policy: Option<&HistoryPolicy>, outcome: &StoreOutcome, calls: usize) -> HoldSlots {
    let Some(policy) = policy.filter(|p| !p.conditions().is_empty()) else {
        return HoldSlots::default();
    };
    let reserve = match outcome {
        StoreOutcome::Disabled => return unavailable(policy, MissingReason::Disabled, calls),
        StoreOutcome::NoSession => return unavailable(policy, MissingReason::NoSession, calls),
        StoreOutcome::Read(reserve) => reserve,
    };
    let WindowRead::Read(window) = &reserve.window else {
        return unavailable(policy, MissingReason::Store, calls);
    };
    let mut slots = HoldSlots::default();
    for index in 0..calls {
        let evaluation = evaluate(policy.conditions(), window, &reserve.hold, index, reserve.now_ms, policy.catalog());
        slots.per_call.push(evaluation.slots);
        slots.missing.extend(evaluation.missing);
    }
    slots
}

/// Every slot of every call missing with `reason`. The tenant part is unavailable when it has a
/// condition, except under `disabled`: with no history table the tenant set is enforced.
fn unavailable(policy: &HistoryPolicy, reason: MissingReason, calls: usize) -> HoldSlots {
    let missing = (0..calls).flat_map(|_| policy.conditions().iter().map(move |c| Missing::new(c.slot.clone(), c.part, reason))).collect();
    HoldSlots {
        per_call: vec![Slots::new(); calls],
        tenant_unavailable: reason != MissingReason::Disabled && policy.has_part(Part::Tenant),
        missing,
    }
}

/// The two `part` labels of the slots metric.
pub(crate) const PARTS: [Part; 2] = [Part::Floor, Part::Tenant];

/// Increment `gateway_tool_history_slots_missing_total{part, reason}` once for each entry.
pub(crate) fn count_missing(missing: &[Missing]) {
    for m in missing {
        counter!(names::GATEWAY_TOOL_HISTORY_SLOTS_MISSING_TOTAL, "part" => m.part.as_str(), "reason" => m.reason.as_str()).increment(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use paigasus_kernel::tool_policy::Findings;
    use paigasus_kernel::tool_policy::history::{
        Catalog, CmpOp, Condition, Event, EventKind, Field, FieldPath, FieldTest, Fields, Literal, Predicate, Scope, SetTool, Shape, ToolMatch, Window, WindowSet,
    };
    use uuid::Uuid;

    use super::*;
    use crate::adapters::history::HmacHistoryDigester;
    use crate::application::tool_policy::testing::ORG_A;

    const NOW: u64 = 1_790_000_000_000;

    fn tool_is(name: &str) -> Predicate {
        Predicate::Tool(ToolMatch::NameKey(name.into()))
    }

    fn path(name: &str) -> FieldPath {
        FieldPath::new(vec![name.to_owned()]).expect("a path")
    }

    /// `count_within(predicate) >= 1` over one minute in the session.
    fn count(slot: &str, part: Part, predicate: Predicate) -> Condition {
        Condition::new(
            slot,
            part,
            Scope::Session,
            60_000,
            Shape::CountWithin {
                predicate,
                op: CmpOp::Ge,
                threshold: 1,
            },
        )
    }

    fn policy(floor: &[Condition], tenant: &[Condition], catalog: Catalog) -> HistoryPolicy {
        HistoryPolicy::build(floor, tenant, ORG_A, catalog, None, &HmacHistoryDigester::keyless())
    }

    fn event(n: u128, tool: &str, fields: Fields) -> Event {
        Event::new(Uuid::from_u128(n), NOW - 1_000, EventKind::Request, tool, fields, None)
    }

    fn no_fields() -> Fields {
        Fields::Values(BTreeMap::new())
    }

    fn read(sets: Vec<WindowSet>, hold: Vec<Event>) -> StoreOutcome {
        StoreOutcome::Read(ReserveOutcome {
            findings: Findings::new(),
            now_ms: NOW,
            hold,
            window: WindowRead::Read(Window::new(sets)),
        })
    }

    fn reserve_with(window: WindowRead) -> StoreOutcome {
        StoreOutcome::Read(ReserveOutcome {
            findings: Findings::new(),
            now_ms: NOW,
            hold: Vec::new(),
            window,
        })
    }

    fn set(tool: &str, events: Vec<Event>) -> WindowSet {
        WindowSet::new(Scope::Session, SetTool::Tool(tool.into()), events, None, false)
    }

    /// The `(slot, part, reason)` of each missing entry.
    fn missing(slots: &HoldSlots) -> Vec<(&str, Part, MissingReason)> {
        slots.missing.iter().map(|m| (m.slot.as_str(), m.part, m.reason)).collect()
    }

    fn floor_and_tenant() -> HistoryPolicy {
        policy(
            &[count("f", Part::Floor, tool_is("a"))],
            &[count("t", Part::Tenant, tool_is("b"))],
            Catalog::known(["a".to_owned(), "b".to_owned()]),
        )
    }

    #[test]
    fn no_condition_has_no_effect() {
        let empty = policy(&[], &[], Catalog::unknown());
        for outcome in [StoreOutcome::Disabled, StoreOutcome::NoSession, reserve_with(WindowRead::Failed), reserve_with(WindowRead::NotNeeded)] {
            assert_eq!(hold_slots(None, &outcome, 3), HoldSlots::default(), "{outcome:?}");
            assert_eq!(hold_slots(Some(&empty), &outcome, 3), HoldSlots::default(), "{outcome:?}");
        }
    }

    #[test]
    fn no_history_table_misses_every_slot_as_disabled_and_keeps_the_tenant() {
        let p = floor_and_tenant();
        let got = hold_slots(Some(&p), &StoreOutcome::Disabled, 2);
        assert!(!got.tenant_unavailable, "the tenant set is enforced");
        assert_eq!(got.per_call, vec![Slots::new(); 2]);
        let d = MissingReason::Disabled;
        assert_eq!(missing(&got), [("f", Part::Floor, d), ("t", Part::Tenant, d), ("f", Part::Floor, d), ("t", Part::Tenant, d)]);
    }

    #[test]
    fn a_store_failure_misses_the_floor_slots_and_makes_a_conditioned_tenant_unavailable() {
        let p = floor_and_tenant();
        let got = hold_slots(Some(&p), &reserve_with(WindowRead::Failed), 1);
        assert!(got.tenant_unavailable);
        assert_eq!(got.per_call, vec![Slots::new()]);
        assert!(missing(&got).contains(&("f", Part::Floor, MissingReason::Store)), "{:?}", got.missing);
    }

    #[test]
    fn a_store_failure_keeps_a_tenant_without_conditions() {
        let p = policy(&[count("f", Part::Floor, tool_is("a"))], &[], Catalog::unknown());
        let got = hold_slots(Some(&p), &reserve_with(WindowRead::Failed), 2);
        assert!(!got.tenant_unavailable, "a tenant part with no condition stays evaluated");
        assert_eq!(missing(&got), [("f", Part::Floor, MissingReason::Store), ("f", Part::Floor, MissingReason::Store)]);
    }

    #[test]
    fn an_unavailable_tenant_counts_each_of_its_slots_as_missing_with_store_or_no_session() {
        let p = policy(
            &[count("f", Part::Floor, tool_is("a"))],
            &[count("t1", Part::Tenant, tool_is("b")), count("t2", Part::Tenant, tool_is("c"))],
            Catalog::unknown(),
        );
        for (outcome, reason) in [(reserve_with(WindowRead::Failed), MissingReason::Store), (StoreOutcome::NoSession, MissingReason::NoSession)] {
            let got = hold_slots(Some(&p), &outcome, 1);
            assert!(got.tenant_unavailable, "{reason:?}");
            assert_eq!(missing(&got), [("f", Part::Floor, reason), ("t1", Part::Tenant, reason), ("t2", Part::Tenant, reason)]);
        }
        // Every primed pair can count: `disabled` of both parts here, and the rest of the reasons
        // of both parts by the evaluator (the tests below and the kernel tests).
        let pairs: HashSet<(Part, MissingReason)> = [StoreOutcome::Disabled, StoreOutcome::NoSession, reserve_with(WindowRead::Failed)]
            .iter()
            .flat_map(|o| hold_slots(Some(&p), o, 1).missing)
            .map(|m| (m.part, m.reason))
            .collect();
        for part in PARTS {
            for reason in [MissingReason::Disabled, MissingReason::Store, MissingReason::NoSession] {
                assert!(pairs.contains(&(part, reason)), "{part:?} {reason:?}");
            }
        }
    }

    #[test]
    fn the_not_needed_branch_is_treated_as_failed() {
        let p = floor_and_tenant();
        assert_eq!(
            hold_slots(Some(&p), &reserve_with(WindowRead::NotNeeded), 2),
            hold_slots(Some(&p), &reserve_with(WindowRead::Failed), 2)
        );
    }

    #[test]
    fn no_session_misses_the_floor_slots_and_makes_a_conditioned_tenant_unavailable() {
        let p = floor_and_tenant();
        let got = hold_slots(Some(&p), &StoreOutcome::NoSession, 1);
        assert!(got.tenant_unavailable);
        assert!(missing(&got).contains(&("f", Part::Floor, MissingReason::NoSession)), "{:?}", got.missing);
        let floor_only = policy(&[count("f", Part::Floor, tool_is("a"))], &[], Catalog::unknown());
        assert!(!hold_slots(Some(&floor_only), &StoreOutcome::NoSession, 1).tenant_unavailable);
    }

    #[test]
    fn a_truncated_set_misses_only_its_slots() {
        let p = floor_and_tenant();
        let truncated = WindowSet::new(Scope::Session, SetTool::Tool("a".into()), Vec::new(), None, true);
        let outcome = read(vec![truncated, set("b", vec![event(1, "b", no_fields())])], Vec::new());
        let got = hold_slots(Some(&p), &outcome, 1);
        assert!(!got.tenant_unavailable);
        assert_eq!(missing(&got), [("f", Part::Floor, MissingReason::Truncated)]);
        assert_eq!(got.per_call[0].get("t"), Some(true), "the tenant slot reads another set");
        assert_eq!(got.per_call[0].get("f"), None);
    }

    #[test]
    fn an_unknown_event_misses_its_slot() {
        let field = Predicate::Field(FieldTest::new(path("n"), CmpOp::Eq, Literal::Long(1)));
        let p = policy(
            &[],
            &[
                count("u", Part::Tenant, Predicate::AllOf(Box::new(tool_is("a")), Box::new(field))),
                count("k", Part::Tenant, tool_is("a")),
            ],
            Catalog::unknown(),
        );
        // The stored arguments did not parse: the field test is unknown on that event.
        let outcome = read(vec![set("a", vec![event(1, "a", Fields::Unparsed)])], Vec::new());
        let got = hold_slots(Some(&p), &outcome, 1);
        assert_eq!(missing(&got), [("u", Part::Tenant, MissingReason::Unknown)]);
        assert_eq!(got.per_call[0].get("k"), Some(true), "a tool-only condition still counts the event");
        // Control: a stored Long gives the slot.
        let mut values = BTreeMap::new();
        values.insert(path("n"), Field::Long(1));
        let got = hold_slots(Some(&p), &read(vec![set("a", vec![event(1, "a", Fields::Values(values))])], Vec::new()), 1);
        assert!(got.missing.is_empty(), "{:?}", got.missing);
        assert_eq!(got.per_call[0].get("u"), Some(true));
    }

    #[test]
    fn any_catalog_tool_with_no_catalog_misses_the_floor_slot() {
        let any = Predicate::Tool(ToolMatch::AnyCatalogTool);
        let p = policy(&[count("f", Part::Floor, any.clone())], &[], Catalog::unknown());
        let got = hold_slots(Some(&p), &read(Vec::new(), Vec::new()), 2);
        assert_eq!(missing(&got), [("f", Part::Floor, MissingReason::Catalog), ("f", Part::Floor, MissingReason::Catalog)]);
        assert!(!got.tenant_unavailable);
        // Control: with a known catalog the slot has a value.
        let known = policy(&[count("f", Part::Floor, any)], &[], Catalog::known(["a".to_owned()]));
        let got = hold_slots(Some(&known), &read(vec![set("a", Vec::new())], Vec::new()), 1);
        assert!(got.missing.is_empty(), "{:?}", got.missing);
        assert_eq!(got.per_call[0].get("f"), Some(false));
    }

    #[test]
    fn each_call_sees_the_earlier_calls_of_the_hold() {
        let p = policy(&[], &[count("t", Part::Tenant, tool_is("a"))], Catalog::unknown());
        let hold = vec![event(1, "b", no_fields()), event(2, "a", no_fields()), event(3, "b", no_fields())];
        let got = hold_slots(Some(&p), &read(vec![set("a", Vec::new())], hold), 3);
        let values: Vec<_> = got.per_call.iter().map(|s| s.get("t")).collect();
        assert_eq!(values, [Some(false), Some(true), Some(true)]);
    }
}
