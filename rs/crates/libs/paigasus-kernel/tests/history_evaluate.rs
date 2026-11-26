// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "tool-policy")]
//! The history condition types and the three-valued evaluator (SMA-753 PR 2, spec P1).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use paigasus_kernel::tool_policy::history::{
    Catalog, CmpOp, Condition, Digest, Evaluation, Event, EventKind, Field, FieldPath, FieldTest, Fields, HistoryDigester, KeyId, Literal, Missing, MissingReason, Part, Predicate, RETENTION_MS,
    Scalar, Scope, SetTool, Shape, Slots, ToolMatch, ToolSet, Window, WindowSet, evaluate,
};
use uuid::Uuid;

const NOW: u64 = 1_000_000_000;
const K0: [u8; 8] = [0; 8];
const K1: [u8; 8] = [1; 8];
const K9: [u8; 8] = [9; 8];

fn k(bytes: [u8; 8]) -> KeyId {
    KeyId::new(bytes)
}

fn d(n: u8) -> Digest {
    Digest::new([n; 16])
}

fn p(name: &str) -> FieldPath {
    FieldPath::new(name.split('.').map(str::to_owned).collect()).expect("path")
}

fn uid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// One event `ts_s` seconds before `NOW` (a negative value is in the future), under key `K1`.
fn ev(id: u128, ts_s: i64, kind: EventKind, tool: &str, fields: Fields) -> Event {
    let ts_ms = (NOW as i64 - ts_s * 1000) as u64;
    Event::new(uid(id), ts_ms, kind, tool, fields, Some(k(K1)))
}

fn req(id: u128, ts_s: i64, tool: &str, fields: Fields) -> Event {
    ev(id, ts_s, EventKind::Request, tool, fields)
}

fn vals(pairs: &[(&str, Field)]) -> Fields {
    Fields::Values(pairs.iter().map(|(k, v)| (p(k), *v)).collect::<BTreeMap<_, _>>())
}

fn amount(n: i64) -> Fields {
    vals(&[("amount", Field::Long(n))])
}

fn set(scope: Scope, tool: SetTool, events: Vec<Event>) -> WindowSet {
    WindowSet::new(scope, tool, events, None, false)
}

/// A window with the `Every` set of `scope`, the `send` and `search` sets, and one set per tool
/// named by the events.
fn win(scope: Scope, events: Vec<Event>) -> Window {
    let mut sets = vec![set(scope, SetTool::Every, events.clone())];
    let mut names: BTreeSet<String> = events.iter().map(|e| e.tool.clone()).collect();
    names.extend(["send".to_owned(), "search".to_owned()]);
    for name in names {
        let of_tool = events.iter().filter(|e| e.tool == name).cloned().collect();
        sets.push(set(scope, SetTool::Tool(name), of_tool));
    }
    Window::new(sets)
}

fn tool(name: &str) -> Predicate {
    Predicate::Tool(ToolMatch::NameKey(name.into()))
}

fn field(path: &str, op: CmpOp, literal: Literal) -> Predicate {
    Predicate::Field(FieldTest::new(p(path), op, literal))
}

fn all(a: Predicate, b: Predicate) -> Predicate {
    Predicate::AllOf(Box::new(a), Box::new(b))
}

fn any(a: Predicate, b: Predicate) -> Predicate {
    Predicate::AnyOf(Box::new(a), Box::new(b))
}

fn not(a: Predicate) -> Predicate {
    Predicate::Not(Box::new(a))
}

fn cond(scope: Scope, window_ms: u64, shape: Shape) -> Condition {
    Condition::new("s", Part::Tenant, scope, window_ms, shape)
}

fn formerly(predicate: Predicate) -> Condition {
    cond(Scope::Session, 600_000, Shape::Formerly { predicate })
}

fn sum(predicate: Predicate, op: CmpOp, threshold: i64) -> Condition {
    cond(
        Scope::Session,
        600_000,
        Shape::SumWithin {
            predicate,
            path: p("amount"),
            op,
            threshold,
        },
    )
}

fn count(predicate: Predicate, op: CmpOp, threshold: i64) -> Condition {
    cond(Scope::Session, 600_000, Shape::CountWithin { predicate, op, threshold })
}

fn known(names: &[&str]) -> Catalog {
    Catalog::known(names.iter().map(|s| (*s).to_owned()))
}

fn run(c: &Condition, window: &Window) -> Evaluation {
    evaluate(std::slice::from_ref(c), window, &[], 0, NOW, &known(&["send", "search"]))
}

/// The slot value, or the missing reason.
fn outcome(e: &Evaluation) -> Result<bool, MissingReason> {
    match e.slots.get("s") {
        Some(v) => Ok(v),
        None => Err(e.missing.iter().find(|m| m.slot == "s").map(|m| m.reason).expect("a slot without a value has a reason")),
    }
}

fn one(c: &Condition, window: &Window) -> Result<bool, MissingReason> {
    outcome(&run(c, window))
}

fn big_send() -> Predicate {
    all(tool("send"), field("amount", CmpOp::Gt, Literal::Long(1000)))
}

#[test]
fn formerly_is_true_on_one_true_event_and_missing_on_one_unknown_event() {
    let c = formerly(big_send());
    assert_eq!(one(&c, &win(Scope::Session, vec![req(1, 10, "send", amount(5)), req(2, 20, "send", amount(2000))])), Ok(true));
    assert_eq!(one(&c, &win(Scope::Session, vec![req(1, 10, "send", amount(5))])), Ok(false));
    assert_eq!(
        one(&c, &win(Scope::Session, vec![req(1, 10, "send", amount(5)), req(2, 20, "send", Fields::Unparsed)])),
        Err(MissingReason::Unknown)
    );
    // A true event wins over an unknown one.
    assert_eq!(one(&c, &win(Scope::Session, vec![req(1, 10, "send", amount(2000)), req(2, 20, "send", Fields::Unparsed)])), Ok(true));
}

#[test]
fn the_window_is_closed_and_a_future_event_is_inside_every_window() {
    let c = cond(Scope::Session, 600_000, Shape::Formerly { predicate: tool("send") });
    let at = |ts_ms: u64| Event::new(uid(1), ts_ms, EventKind::Request, "send", amount(1), Some(k(K1)));
    let now = 1_000_000;
    let check = |ts_ms: u64| outcome(&evaluate(std::slice::from_ref(&c), &win(Scope::Session, vec![at(ts_ms)]), &[], 0, now, &known(&["send"])));
    assert_eq!(check(400_000), Ok(true));
    assert_eq!(check(399_999), Ok(false));
    assert_eq!(check(5_000_000), Ok(true));
}

#[test]
fn count_within_takes_the_value_only_when_every_possible_count_agrees() {
    let pred = big_send();
    let events = |t: u128, u: u128| {
        let mut out: Vec<Event> = (0..t).map(|i| req(i, 1, "send", amount(2000))).collect();
        out.extend((0..u).map(|i| req(100 + i, 1, "send", Fields::Unparsed)));
        out
    };
    let le3 = count(pred.clone(), CmpOp::Le, 3);
    assert_eq!(one(&le3, &win(Scope::Session, events(2, 1))), Ok(true));
    assert_eq!(one(&le3, &win(Scope::Session, events(3, 1))), Err(MissingReason::Unknown));
    let ge1 = count(pred, CmpOp::Ge, 1);
    assert_eq!(one(&ge1, &win(Scope::Session, events(1, 5))), Ok(true));
    assert_eq!(one(&ge1, &win(Scope::Session, events(0, 0))), Ok(false));
}

#[test]
fn sum_within_adds_longs_adds_zero_for_absent_and_negative_and_saturates() {
    let c = sum(tool("send"), CmpOp::Le, 110);
    let events = vec![
        req(1, 1, "send", amount(60)),
        req(2, 2, "send", vals(&[("amount", Field::Absent)])),
        req(3, 3, "send", amount(-30)),
        req(4, 4, "send", amount(50)),
    ];
    assert_eq!(one(&c, &win(Scope::Session, events.clone())), Ok(true));
    assert_eq!(
        one(&sum(tool("send"), CmpOp::Lt, 110), &win(Scope::Session, events)),
        Ok(false),
        "the negative value adds 0, so the sum is 110"
    );
    let big = vec![req(1, 1, "send", amount(i64::MAX)), req(2, 2, "send", amount(1))];
    assert_eq!(one(&sum(tool("send"), CmpOp::Eq, i64::MAX), &win(Scope::Session, big)), Ok(true));
}

#[test]
fn a_non_long_value_at_a_sum_path_makes_the_slot_missing() {
    let c = sum(tool("send"), CmpOp::Le, 1_000_000);
    for fields in [vals(&[("amount", Field::Digest(d(3)))]), vals(&[("amount", Field::Mistyped)]), Fields::Unparsed] {
        let w = win(Scope::Session, vec![req(1, 1, "send", amount(5)), req(2, 2, "send", fields)]);
        assert_eq!(one(&c, &w), Err(MissingReason::Unknown));
    }
}

#[test]
fn a_free_text_call_of_another_tool_does_not_remove_a_sum_slot() {
    let c = sum(tool("send"), CmpOp::Le, 100);
    let w = win(Scope::Session, vec![req(1, 1, "send", amount(40)), req(2, 2, "search", Fields::Unparsed)]);
    assert_eq!(one(&c, &w), Ok(true));
}

#[test]
fn a_path_that_is_not_in_the_event_is_false_and_adds_zero() {
    let old = vals(&[("other", Field::Long(1))]);
    let w = win(Scope::Session, vec![req(1, 1, "send", old.clone())]);
    assert_eq!(one(&formerly(field("amount", CmpOp::Ge, Literal::Long(0))), &w), Ok(false));
    let w = win(Scope::Session, vec![req(1, 1, "send", old), req(2, 2, "send", amount(7))]);
    assert_eq!(one(&sum(tool("send"), CmpOp::Eq, 7), &w), Ok(true));
}

#[test]
fn field_tests_follow_the_three_valued_table() {
    let digests = Literal::Digests(vec![(k(K1), d(1))]);
    let row = |stored: Fields, key: Option<KeyId>, op: CmpOp, literal: Literal| {
        let e = Event::new(uid(1), NOW, EventKind::Request, "send", stored, key);
        one(&formerly(field("amount", op, literal)), &win(Scope::Session, vec![e]))
    };
    let at = |f: Field| vals(&[("amount", f)]);
    // Long and a Long literal: the comparison.
    assert_eq!(row(at(Field::Long(5)), Some(k(K1)), CmpOp::Gt, Literal::Long(4)), Ok(true));
    assert_eq!(row(at(Field::Long(5)), Some(k(K1)), CmpOp::Gt, Literal::Long(5)), Ok(false));
    // Digest and literal digests: equal under the event's key id.
    assert_eq!(row(at(Field::Digest(d(1))), Some(k(K1)), CmpOp::Eq, digests.clone()), Ok(true));
    assert_eq!(row(at(Field::Digest(d(2))), Some(k(K1)), CmpOp::Eq, digests.clone()), Ok(false));
    assert_eq!(row(at(Field::Digest(d(2))), Some(k(K1)), CmpOp::Ne, digests.clone()), Ok(true));
    // Different types: unknown.
    assert_eq!(row(at(Field::Digest(d(1))), Some(k(K1)), CmpOp::Gt, Literal::Long(4)), Err(MissingReason::Unknown));
    assert_eq!(row(at(Field::Long(5)), Some(k(K1)), CmpOp::Eq, digests.clone()), Err(MissingReason::Unknown));
    // Absent, and a path that is not in the event: false.
    assert_eq!(row(at(Field::Absent), Some(k(K1)), CmpOp::Eq, Literal::Long(0)), Ok(false));
    assert_eq!(row(vals(&[]), Some(k(K1)), CmpOp::Eq, Literal::Long(0)), Ok(false));
    // Mistyped, and unparsed arguments: unknown.
    assert_eq!(row(at(Field::Mistyped), Some(k(K1)), CmpOp::Eq, Literal::Long(0)), Err(MissingReason::Unknown));
    assert_eq!(row(Fields::Unparsed, Some(k(K1)), CmpOp::Eq, Literal::Long(0)), Err(MissingReason::Unknown));
    // A digest under a key id that the gateway does not know, or with no key id: unknown.
    assert_eq!(row(at(Field::Digest(d(1))), Some(k(K9)), CmpOp::Eq, digests.clone()), Err(MissingReason::Unknown));
    assert_eq!(row(at(Field::Digest(d(1))), None, CmpOp::Eq, digests), Err(MissingReason::Unknown));
}

#[test]
fn a_string_value_against_a_long_literal_is_unknown() {
    let transfer = all(tool("transfer"), field("amount", CmpOp::Gt, Literal::Long(1000)));
    let w = win(Scope::Session, vec![req(1, 1, "transfer", vals(&[("amount", Field::Digest(d(4)))]))]);
    assert_eq!(one(&formerly(transfer), &w), Err(MissingReason::Unknown));
    let by_digest = all(tool("transfer"), field("amount", CmpOp::Eq, Literal::Digests(vec![(k(K1), d(4))])));
    let w = win(Scope::Session, vec![req(1, 1, "transfer", amount(5))]);
    assert_eq!(one(&formerly(by_digest), &w), Err(MissingReason::Unknown));
}

#[test]
fn kleene_logic_for_all_of_any_of_and_not() {
    // On an event with `amount = 5`: `amount == 5` is true, `amount == 6` is false, `x == 0` is
    // unknown (`x` is Mistyped).
    let t = || field("amount", CmpOp::Eq, Literal::Long(5));
    let f = || field("amount", CmpOp::Eq, Literal::Long(6));
    let u = || field("x", CmpOp::Eq, Literal::Long(0));
    let w = win(Scope::Session, vec![req(1, 1, "send", vals(&[("amount", Field::Long(5)), ("x", Field::Mistyped)]))]);
    let value = |pr: Predicate| one(&formerly(pr), &w).ok();
    type Leaf = fn() -> Predicate;
    let leaves: [(Leaf, Option<bool>); 3] = [
        (|| field("amount", CmpOp::Eq, Literal::Long(5)), Some(true)),
        (|| field("amount", CmpOp::Eq, Literal::Long(6)), Some(false)),
        (|| field("x", CmpOp::Eq, Literal::Long(0)), None),
    ];
    for (a, va) in leaves {
        for (b, vb) in leaves {
            let and = match (va, vb) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
            let or = match (va, vb) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
            assert_eq!(value(all(a(), b())), and, "AllOf({va:?}, {vb:?})");
            assert_eq!(value(any(a(), b())), or, "AnyOf({va:?}, {vb:?})");
        }
    }
    assert_eq!(value(not(t())), Some(false));
    assert_eq!(value(not(f())), Some(true));
    assert_eq!(value(not(u())), None);
    assert_eq!(one(&formerly(not(u())), &w), Err(MissingReason::Unknown));
}

#[test]
fn the_call_at_index_i_sees_the_hold_up_to_i_only() {
    let c = sum(tool("send"), CmpOp::Le, 5000);
    let hold: Vec<Event> = (0..3).map(|i| req(10 + i, 0, "send", amount(2000))).collect();
    let w = win(Scope::Session, vec![]);
    let at = |index| outcome(&evaluate(std::slice::from_ref(&c), &w, &hold, index, NOW, &known(&["send"])));
    assert_eq!(at(0), Ok(true));
    assert_eq!(at(1), Ok(true));
    assert_eq!(at(2), Ok(false));
}

#[test]
fn the_events_of_a_window_and_the_hold_are_counted_once_by_id() {
    let c = count(tool("send"), CmpOp::Eq, 2);
    let e1 = req(1, 1, "send", amount(1));
    let e2 = req(2, 2, "send", amount(1));
    // `e1` is in the Every set, the send set and the hold.
    let w = win(Scope::Session, vec![e1.clone(), e2]);
    let e = evaluate(std::slice::from_ref(&c), &w, &[e1], 0, NOW, &known(&["send"]));
    assert_eq!(outcome(&e), Ok(true));
}

#[test]
fn a_key_scope_condition_reads_the_key_sets() {
    let c = cond(
        Scope::Key,
        600_000,
        Shape::CountWithin {
            predicate: tool("send"),
            op: CmpOp::Eq,
            threshold: 1,
        },
    );
    let mut sets = vec![set(Scope::Key, SetTool::Tool("send".into()), vec![req(1, 1, "send", amount(1))])];
    sets.push(set(Scope::Session, SetTool::Tool("send".into()), vec![req(2, 1, "send", amount(1)), req(3, 1, "send", amount(1))]));
    assert_eq!(one(&c, &Window::new(sets)), Ok(true));
    // With only session sets, the key set is not in the window.
    let only_session = Window::new(vec![set(Scope::Session, SetTool::Tool("send".into()), vec![])]);
    assert_eq!(one(&c, &only_session), Err(MissingReason::Store));
}

#[test]
fn a_mark_at_or_after_the_window_start_makes_the_slot_missing() {
    let c = formerly(tool("send"));
    let start = NOW - 600_000;
    let marked = |mark| Window::new(vec![WindowSet::new(Scope::Session, SetTool::Tool("send".into()), vec![], Some(mark), false)]);
    assert_eq!(one(&c, &marked(start)), Err(MissingReason::Truncated));
    assert_eq!(one(&c, &marked(start + 1)), Err(MissingReason::Truncated));
    assert_eq!(one(&c, &marked(start - 1)), Ok(false));
}

#[test]
fn a_truncated_set_makes_only_the_conditions_that_read_it_missing() {
    let sets = vec![
        WindowSet::new(Scope::Session, SetTool::Tool("send".into()), vec![], None, true),
        set(Scope::Session, SetTool::Tool("search".into()), vec![req(1, 1, "search", vals(&[]))]),
        set(Scope::Session, SetTool::Every, vec![req(1, 1, "search", vals(&[]))]),
    ];
    let w = Window::new(sets);
    let conditions = [
        Condition::new("search", Part::Tenant, Scope::Session, 600_000, Shape::Formerly { predicate: tool("search") }),
        Condition::new(
            "every",
            Part::Floor,
            Scope::Session,
            600_000,
            Shape::Formerly {
                predicate: Predicate::EventKind(EventKind::Request),
            },
        ),
    ];
    let e = evaluate(&conditions, &w, &[], 0, NOW, &known(&["send", "search"]));
    assert_eq!(e.slots.get("search"), Some(true));
    assert_eq!(e.slots.get("every"), None);
    assert_eq!(e.missing, vec![Missing::new("every", Part::Floor, MissingReason::Truncated)]);
}

#[test]
fn a_window_longer_than_the_retention_is_missing() {
    let w = win(Scope::Session, vec![req(1, 1, "send", amount(1))]);
    let with = |window_ms| cond(Scope::Session, window_ms, Shape::Formerly { predicate: tool("send") });
    assert_eq!(one(&with(RETENTION_MS + 1), &w), Err(MissingReason::Retention));
    assert_eq!(one(&with(RETENTION_MS), &w), Ok(true));
    assert_eq!(RETENTION_MS, 90_000_000);
}

#[test]
fn an_unbound_or_empty_digest_literal_is_missing_with_no_key() {
    let w = win(Scope::Session, vec![req(1, 1, "send", vals(&[("to", Field::Digest(d(1)))]))]);
    for literal in [Literal::Unbound(Scalar::Str("bob".into())), Literal::Digests(vec![])] {
        // Under `AnyOf` with a true side, the slot is still missing: the check runs before any event.
        let c = formerly(any(tool("send"), field("to", CmpOp::Eq, literal)));
        assert_eq!(one(&c, &w), Err(MissingReason::NoKey));
    }
}

#[test]
fn any_catalog_tool_with_an_unknown_catalog_is_missing() {
    let w = win(Scope::Session, vec![req(1, 1, "send", amount(1))]);
    let c = formerly(Predicate::Tool(ToolMatch::AnyCatalogTool));
    assert_eq!(outcome(&evaluate(std::slice::from_ref(&c), &w, &[], 0, NOW, &Catalog::unknown())), Err(MissingReason::Catalog));
    assert_eq!(outcome(&evaluate(std::slice::from_ref(&c), &w, &[], 0, NOW, &known(&["send"]))), Ok(true));
    assert_eq!(
        outcome(&evaluate(std::slice::from_ref(&c), &w, &[], 0, NOW, &known(&["other"]))),
        Err(MissingReason::Store),
        "the set of a catalog tool is required"
    );
    let w_other = Window::new(vec![
        set(Scope::Session, SetTool::Tool("other".into()), vec![]),
        set(Scope::Session, SetTool::Tool("send".into()), vec![req(1, 1, "send", amount(1))]),
    ]);
    assert_eq!(
        outcome(&evaluate(std::slice::from_ref(&c), &w_other, &[], 0, NOW, &known(&["other"]))),
        Ok(false),
        "a send event is not a catalog tool"
    );
}

#[test]
fn an_event_under_the_previous_key_matches_and_an_unknown_key_is_unknown() {
    let literal = Literal::Digests(vec![(k(K1), d(1)), (k(K0), d(0))]);
    let c = formerly(field("to", CmpOp::Eq, literal));
    let under = |key| {
        win(
            Scope::Session,
            vec![Event::new(uid(1), NOW, EventKind::Request, "send", vals(&[("to", Field::Digest(d(0)))]), Some(key))],
        )
    };
    assert_eq!(one(&c, &under(k(K0))), Ok(true));
    assert_eq!(one(&c, &under(k(K9))), Err(MissingReason::Unknown));
}

#[test]
fn a_sum_event_with_an_unknown_predicate_makes_the_slot_missing_even_with_a_long() {
    let pred = all(tool("send"), field("to", CmpOp::Eq, Literal::Digests(vec![(k(K1), d(1))])));
    let c = sum(pred, CmpOp::Le, 1_000_000);
    let e = Event::new(uid(1), NOW, EventKind::Request, "send", vals(&[("to", Field::Digest(d(1))), ("amount", Field::Long(5))]), Some(k(K9)));
    assert_eq!(one(&c, &win(Scope::Session, vec![e])), Err(MissingReason::Unknown));
}

#[test]
fn a_named_tool_condition_does_not_require_the_every_set() {
    let only_send = Window::new(vec![set(Scope::Session, SetTool::Tool("send".into()), vec![req(1, 1, "send", amount(1))])]);
    assert_eq!(one(&formerly(tool("send")), &only_send), Ok(true));
    assert_eq!(one(&formerly(not(tool("search"))), &only_send), Err(MissingReason::Store));
    let catalog_tools = ToolSet::new(BTreeSet::new(), true, false);
    assert!(catalog_tools.requires_every(&Catalog::unknown()), "AnyCatalogTool with an unknown catalog requires Every");
    assert!(!catalog_tools.requires_every(&known(&["send"])));
    assert!(!ToolSet::new(BTreeSet::from(["send".to_owned()]), false, false).requires_every(&Catalog::unknown()));
    assert!(ToolSet::new(BTreeSet::new(), false, true).requires_every(&known(&[])));
}

/// A catalog of `n` tools `t0` .. `t{n-1}`.
fn catalog_of(n: usize) -> Catalog {
    Catalog::known((0..n).map(|i| format!("t{i}")))
}

#[test]
fn a_catalog_tool_set_past_the_read_set_limit_requires_the_every_set() {
    use paigasus_kernel::tool_policy::history::MAX_READ_SETS;
    assert_eq!(MAX_READ_SETS, 64);
    let catalog_tools = ToolSet::new(BTreeSet::new(), true, false);
    assert!(!catalog_tools.requires_every(&catalog_of(MAX_READ_SETS)), "64 catalog tools: one set each");
    assert!(catalog_tools.requires_every(&catalog_of(MAX_READ_SETS + 1)), "65 catalog tools: the Every set");
    // The named tools of the set count too, once each, with the catalog tools.
    let with_name = ToolSet::new(BTreeSet::from(["x".to_owned()]), true, false);
    assert!(with_name.requires_every(&catalog_of(MAX_READ_SETS)), "64 catalog tools and one more name");
    let with_catalog_name = ToolSet::new(BTreeSet::from(["t0".to_owned()]), true, false);
    assert!(!with_catalog_name.requires_every(&catalog_of(MAX_READ_SETS)), "a name in the catalog counts once");
    // A tool set with no catalog never expands, so the limit does not apply to it.
    let many: BTreeSet<String> = (0..=MAX_READ_SETS).map(|i| format!("n{i}")).collect();
    assert!(!ToolSet::new(many, false, false).requires_every(&catalog_of(MAX_READ_SETS + 1)));
}

#[test]
fn a_large_catalog_condition_reads_the_every_set_and_counts_only_catalog_tools() {
    let catalog = catalog_of(65);
    let c = count(Predicate::Tool(ToolMatch::AnyCatalogTool), CmpOp::Ge, 2);
    // The Every set alone: the reserve reads no per-tool set for this condition.
    let events = vec![
        req(1, 1, "t64", Fields::Values(BTreeMap::new())),
        req(2, 2, "t0", Fields::Values(BTreeMap::new())),
        req(3, 3, "other", Fields::Values(BTreeMap::new())),
    ];
    let only_every = Window::new(vec![set(Scope::Session, SetTool::Every, events.clone())]);
    let e = evaluate(std::slice::from_ref(&c), &only_every, &[], 0, NOW, &catalog);
    assert_eq!(e.missing, vec![], "no set is missing and none is truncated");
    assert_eq!(e.slots.get("s"), Some(true), "t64 and t0 count; `other` is not in the catalog");
    let one_catalog_call = Window::new(vec![set(Scope::Session, SetTool::Every, vec![events[0].clone(), events[2].clone()])]);
    assert_eq!(evaluate(std::slice::from_ref(&c), &one_catalog_call, &[], 0, NOW, &catalog).slots.get("s"), Some(false));
    // A truncated Every set still makes the slot missing: the fallback does not fail open.
    let truncated = Window::new(vec![WindowSet::new(Scope::Session, SetTool::Every, events, None, true)]);
    assert_eq!(
        evaluate(std::slice::from_ref(&c), &truncated, &[], 0, NOW, &catalog).missing,
        vec![Missing::new("s", Part::Tenant, MissingReason::Truncated)]
    );
}

#[test]
fn field_tests_gives_each_field_test_its_own_tool_set() {
    let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    let a = FieldTest::new(p("a"), CmpOp::Eq, Literal::Long(1));
    let b = FieldTest::new(p("b"), CmpOp::Eq, Literal::Long(2));
    let first = all(tool("send"), Predicate::Field(a.clone()));
    assert_eq!(first.field_tests(), vec![(ToolSet::new(names(&["send"]), false, false), &a)]);
    let second = any(all(tool("x"), Predicate::Field(a.clone())), Predicate::Field(b.clone()));
    let tests = second.field_tests();
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0], (ToolSet::new(names(&["x"]), false, false), &a));
    assert_eq!(tests[1].1, &b);
    assert!(tests[1].0.every);
    assert!(tests[1].0.names.is_empty());
}

#[test]
fn missing_reason_as_str_gives_the_metric_labels_and_all_lists_each_once() {
    let labels: Vec<&str> = MissingReason::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(labels, ["disabled", "store", "no_session", "truncated", "unknown", "retention", "no_key", "catalog"]);
    assert_eq!(MissingReason::ALL.len(), 8);
    assert_eq!(labels.iter().collect::<HashSet<_>>().len(), 8);
}

#[test]
fn part_as_str_gives_floor_and_tenant() {
    assert_eq!((Part::Floor.as_str(), Part::Tenant.as_str()), ("floor", "tenant"));
}

#[test]
fn catalog_contains_is_none_when_unknown() {
    assert_eq!(known(&["a"]).contains("a"), Some(true));
    assert_eq!(known(&["a"]).contains("b"), Some(false));
    assert_eq!(Catalog::unknown().contains("a"), None);
}

#[test]
fn tools_walk_intersects_all_of_and_unites_any_of() {
    let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    let f = || field("a", CmpOp::Eq, Literal::Long(1));
    assert_eq!(all(tool("send"), f()).tools(), ToolSet::new(names(&["send"]), false, false));
    assert_eq!(any(tool("a"), tool("b")).tools(), ToolSet::new(names(&["a", "b"]), false, false));
    assert_eq!(not(tool("a")).tools(), ToolSet::new(BTreeSet::new(), false, true));
    assert_eq!(all(Predicate::Tool(ToolMatch::AnyCatalogTool), tool("a")).tools(), ToolSet::new(names(&["a"]), true, false));
    assert_eq!(all(tool("a"), tool("b")).tools(), ToolSet::new(BTreeSet::new(), false, false));
    assert_eq!(Predicate::EventKind(EventKind::Response).tools(), ToolSet::new(BTreeSet::new(), false, true));
    assert_eq!(
        cond(
            Scope::Session,
            1,
            Shape::SumWithin {
                predicate: tool("send"),
                path: p("amount"),
                op: CmpOp::Le,
                threshold: 1
            }
        )
        .tools(),
        ToolSet::new(names(&["send"]), false, false)
    );
}

/// Digest = `[n; 16]` where `n` is the length of the canonical bytes; key ids `K1` then `K0`.
struct TestDigester;

impl HistoryDigester for TestDigester {
    fn current(&self, _org: Uuid, _path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)> {
        Some((k(K1), d(value.canonical_bytes().len() as u8)))
    }
    fn all(&self, _org: Uuid, _path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)> {
        let n = value.canonical_bytes().len() as u8;
        vec![(k(K1), d(n)), (k(K0), d(n + 100))]
    }
}

#[test]
fn bind_digests_replaces_every_unbound_literal() {
    let unbound = |path| field(path, CmpOp::Eq, Literal::Unbound(Scalar::Str("bob".into())));
    let pred = all(not(unbound("to")), any(unbound("cc"), field("amount", CmpOp::Gt, Literal::Long(3))));
    let c = cond(
        Scope::Principal,
        1000,
        Shape::CountWithin {
            predicate: pred,
            op: CmpOp::Ge,
            threshold: 2,
        },
    );
    let bound = c.bind_digests(uid(7), &TestDigester);
    let expected = Literal::Digests(vec![(k(K1), d(4)), (k(K0), d(104))]);
    let Shape::CountWithin { predicate, op, threshold } = &bound.shape else {
        panic!("the shape is kept")
    };
    assert_eq!((*op, *threshold), (CmpOp::Ge, 2));
    let literals: Vec<&Literal> = predicate.field_tests().into_iter().map(|(_, t)| &t.literal).collect();
    assert_eq!(literals, vec![&expected, &expected, &Literal::Long(3)]);
    assert_eq!((bound.slot.as_str(), bound.part, bound.scope, bound.window_ms), ("s", Part::Tenant, Scope::Principal, 1000));
}

#[test]
fn cmp_op_apply_compares_left_with_right() {
    let cases = [
        (CmpOp::Lt, [true, false, false]),
        (CmpOp::Le, [true, true, false]),
        (CmpOp::Eq, [false, true, false]),
        (CmpOp::Ge, [false, true, true]),
        (CmpOp::Gt, [false, false, true]),
        (CmpOp::Ne, [true, false, true]),
    ];
    for (op, expected) in cases {
        assert_eq!([op.apply(1, 2), op.apply(2, 2), op.apply(3, 2)], expected, "{op:?}");
    }
}

#[test]
fn slots_insert_get_iter_and_is_empty() {
    let mut s = Slots::new();
    assert!(s.is_empty());
    s.insert("b", false);
    s.insert("a", true);
    assert_eq!(s.get("a"), Some(true));
    assert_eq!(s.get("z"), None);
    assert_eq!(s.iter().collect::<Vec<_>>(), vec![("a", true), ("b", false)]);
    assert!(!s.is_empty());
    assert_eq!(Evaluation::new(Slots::new(), vec![]).slots, Slots::new());
}

#[test]
fn a_known_catalog_lists_its_names_and_an_unknown_catalog_lists_none() {
    let known = Catalog::known(["send".to_owned(), "search".to_owned()]);
    let expected: HashSet<String> = ["send".to_owned(), "search".to_owned()].into();
    assert_eq!(known.names(), Some(&expected));
    assert_eq!(Catalog::unknown().names(), None);
}

#[test]
fn the_shape_predicate_is_the_predicate_of_each_shape() {
    let shapes = [
        Shape::Formerly { predicate: tool("a") },
        Shape::CountWithin {
            predicate: tool("a"),
            op: CmpOp::Ge,
            threshold: 1,
        },
        Shape::SumWithin {
            predicate: tool("a"),
            path: p("amount"),
            op: CmpOp::Le,
            threshold: 1,
        },
    ];
    for shape in &shapes {
        assert_eq!(shape.predicate(), &tool("a"), "{shape:?}");
    }
}

#[test]
fn scopes_order_session_then_principal_then_key() {
    let mut scopes = vec![Scope::Key, Scope::Session, Scope::Principal];
    scopes.sort();
    assert_eq!(scopes, [Scope::Session, Scope::Principal, Scope::Key]);
}

#[test]
fn an_unavailable_window_gives_its_reason_after_the_pre_checks() {
    let w = Window::unavailable(MissingReason::NoSession);
    assert_eq!(one(&formerly(tool("send")), &w), Err(MissingReason::NoSession));
    let long = cond(Scope::Session, RETENTION_MS + 1, Shape::Formerly { predicate: tool("send") });
    assert_eq!(one(&long, &w), Err(MissingReason::Retention));
    // The hold alone does not give a value when the window is unavailable.
    let e = evaluate(&[formerly(tool("send"))], &w, &[req(1, 0, "send", amount(1))], 0, NOW, &known(&["send"]));
    assert_eq!(outcome(&e), Err(MissingReason::NoSession));
}
