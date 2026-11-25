// SPDX-License-Identifier: Apache-2.0
//! The mapping of the proto history conditions into the kernel types, with the load validation
//! (SMA-753 spec P5, design D6). A malformed condition makes the whole set malformed: the caller
//! treats the set like an unavailable set.

use std::collections::BTreeSet;

use paigasus_kernel::tool_policy::history::{CmpOp, Condition, EventKind, FieldPath, FieldTest, Literal, Part, Predicate, Scalar, Scope, Shape, ToolMatch};
use paigasus_kernel::tool_policy::{RESERVED_CONTEXT_NAMES, normalise};
use paigasus_proto::paigasus::iam::v1 as pb;
use pb::{history_condition, history_predicate, literal_value};

/// The deepest predicate tree that loads, in predicate nodes along the longest path. A leaf is 1.
/// `paigasus_iam_core::tool_policy::check` counts the same way and refuses deeper trees at compile
/// time. This crate keeps its own copy, so a set from an older compiler still fails closed.
pub const MAX_PREDICATE_DEPTH: usize = 32;

/// A history condition that the gateway refuses to load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("history condition {index} is malformed: {reason}")]
pub struct MalformedCondition {
    /// The position of the condition in the set.
    pub index: usize,
    /// A fixed reason string.
    pub reason: &'static str,
}

/// Map and validate the conditions of one set. `other_slots` holds the slot names of the floor
/// when `part` is `Tenant` (a clash makes the tenant set malformed), and is empty for the floor.
pub fn map_conditions(part: Part, pb: &[pb::HistoryCondition], max_window_ms: u64, other_slots: &BTreeSet<String>) -> Result<Vec<Condition>, MalformedCondition> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(pb.len());
    for (index, condition) in pb.iter().enumerate() {
        let fail = |reason: &'static str| MalformedCondition { index, reason };
        check_slot(&condition.slot).map_err(fail)?;
        if !seen.insert(condition.slot.as_str()) {
            return Err(fail("slot_duplicate"));
        }
        if part == Part::Tenant && other_slots.contains(&condition.slot) {
            return Err(fail("slot_clash"));
        }
        let scope = match pb::HistoryScope::try_from(condition.scope) {
            Ok(pb::HistoryScope::Session) => Scope::Session,
            Ok(pb::HistoryScope::Principal) => Scope::Principal,
            Ok(pb::HistoryScope::Key) => Scope::Key,
            Ok(pb::HistoryScope::Unspecified) | Err(_) => return Err(fail("enum_unknown")),
        };
        if condition.window_ms == 0 || condition.window_ms > max_window_ms {
            return Err(fail("window"));
        }
        let shape = map_shape(condition.shape.as_ref()).map_err(fail)?;
        out.push(Condition::new(condition.slot.clone(), part, scope, condition.window_ms, shape));
    }
    Ok(out)
}

/// A Cedar identifier that is not a reserved context name.
fn check_slot(slot: &str) -> Result<(), &'static str> {
    let mut chars = slot.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') || RESERVED_CONTEXT_NAMES.contains(&slot) {
        return Err("slot_name");
    }
    Ok(())
}

fn map_shape(shape: Option<&history_condition::Shape>) -> Result<Shape, &'static str> {
    match shape.ok_or("shape_unset")? {
        history_condition::Shape::Formerly(s) => Ok(Shape::Formerly {
            predicate: map_predicate(s.predicate.as_ref(), 1)?,
        }),
        history_condition::Shape::CountWithin(s) => Ok(Shape::CountWithin {
            predicate: map_predicate(s.predicate.as_ref(), 1)?,
            op: map_op(s.op)?,
            threshold: s.threshold,
        }),
        history_condition::Shape::SumWithin(s) => Ok(Shape::SumWithin {
            predicate: map_predicate(s.predicate.as_ref(), 1)?,
            path: map_path(&s.field_path)?,
            op: map_op(s.op)?,
            threshold: s.threshold,
        }),
    }
}

/// `depth` is the level of `p`: 1 for the root.
fn map_predicate(p: Option<&pb::HistoryPredicate>, depth: usize) -> Result<Predicate, &'static str> {
    if depth > MAX_PREDICATE_DEPTH {
        return Err("depth");
    }
    // The generated code makes every message optional: a missing predicate is a missing node.
    let node = p.and_then(|p| p.node.as_ref()).ok_or("node_unset")?;
    match node {
        history_predicate::Node::EventKind(kind) => match pb::HistoryEventKind::try_from(*kind) {
            Ok(pb::HistoryEventKind::Request) => Ok(Predicate::EventKind(EventKind::Request)),
            Ok(pb::HistoryEventKind::Response) => Ok(Predicate::EventKind(EventKind::Response)),
            Ok(pb::HistoryEventKind::Unspecified) | Err(_) => Err("enum_unknown"),
        },
        history_predicate::Node::Tool(tool) => map_tool(tool).map(Predicate::Tool),
        history_predicate::Node::Field(field) => map_field(field).map(Predicate::Field),
        history_predicate::Node::AllOf(pair) => {
            let (left, right) = map_pair(pair, depth)?;
            Ok(Predicate::AllOf(Box::new(left), Box::new(right)))
        }
        history_predicate::Node::AnyOf(pair) => {
            let (left, right) = map_pair(pair, depth)?;
            Ok(Predicate::AnyOf(Box::new(left), Box::new(right)))
        }
        history_predicate::Node::Negation(inner) => Ok(Predicate::Not(Box::new(map_predicate(Some(inner), depth + 1)?))),
    }
}

fn map_pair(pair: &pb::PredicatePair, depth: usize) -> Result<(Predicate, Predicate), &'static str> {
    let (Some(left), Some(right)) = (pair.left.as_deref(), pair.right.as_deref()) else {
        return Err("pair_side_missing");
    };
    Ok((map_predicate(Some(left), depth + 1)?, map_predicate(Some(right), depth + 1)?))
}

fn map_tool(tool: &pb::ToolMatch) -> Result<ToolMatch, &'static str> {
    match (tool.name_key.is_empty(), tool.any_catalog_tool) {
        (false, false) => {
            if tool.name_key != normalise(&tool.name_key) {
                return Err("name_key_not_normal");
            }
            Ok(ToolMatch::NameKey(tool.name_key.clone()))
        }
        (true, true) => Ok(ToolMatch::AnyCatalogTool),
        _ => Err("tool_match"),
    }
}

fn map_field(field: &pb::FieldComparison) -> Result<FieldTest, &'static str> {
    let path = map_path(&field.path)?;
    let op = map_op(field.op)?;
    let value = field.value.as_ref().and_then(|v| v.value.as_ref()).ok_or("value_unset")?;
    let literal = match value {
        literal_value::Value::LongValue(n) => Literal::Long(*n),
        literal_value::Value::StringValue(s) => Literal::Unbound(Scalar::Str(s.clone())),
        literal_value::Value::BoolValue(b) => Literal::Unbound(Scalar::Bool(*b)),
        literal_value::Value::DecimalValue(d) => Literal::Unbound(Scalar::Decimal(parse_decimal(d).ok_or("decimal")?)),
    };
    if !matches!(literal, Literal::Long(_)) && op != CmpOp::Eq {
        return Err("op_not_eq");
    }
    Ok(FieldTest::new(path, op, literal))
}

fn map_path(segments: &[String]) -> Result<FieldPath, &'static str> {
    FieldPath::new(segments.to_vec()).map_err(|_| "path")
}

fn map_op(op: i32) -> Result<CmpOp, &'static str> {
    match pb::ComparisonOp::try_from(op) {
        Ok(pb::ComparisonOp::Lt) => Ok(CmpOp::Lt),
        Ok(pb::ComparisonOp::Le) => Ok(CmpOp::Le),
        Ok(pb::ComparisonOp::Eq) => Ok(CmpOp::Eq),
        Ok(pb::ComparisonOp::Ge) => Ok(CmpOp::Ge),
        Ok(pb::ComparisonOp::Gt) => Ok(CmpOp::Gt),
        Ok(pb::ComparisonOp::Ne) => Ok(CmpOp::Ne),
        Ok(pb::ComparisonOp::Unspecified) | Err(_) => Err("enum_unknown"),
    }
}

/// A Cedar decimal literal times 10,000: an optional `-`, digits, `.`, then 1 to 4 digits. The
/// scaled value must fit `i64`.
fn parse_decimal(text: &str) -> Option<i64> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (int_part, frac_part) = rest.split_once('.')?;
    let digits_only = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits_only(int_part) || !digits_only(frac_part) || frac_part.len() > 4 {
        return None;
    }
    let magnitude: i128 = format!("{int_part}{frac_part:0<4}").parse().ok()?;
    i64::try_from(if negative { -magnitude } else { magnitude }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_WINDOW: u64 = 86_400_000;

    fn floor_slots() -> BTreeSet<String> {
        BTreeSet::from(["floor_slot".to_owned()])
    }

    fn event_kind(kind: pb::HistoryEventKind) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::EventKind(kind as i32)),
        }
    }

    fn tool(name_key: &str, any: bool) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::Tool(pb::ToolMatch {
                name_key: name_key.to_owned(),
                any_catalog_tool: any,
            })),
        }
    }

    /// A field comparison. `value: None` is a `LiteralValue` with no member set.
    fn field(path: &[&str], op: pb::ComparisonOp, value: Option<literal_value::Value>) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::Field(pb::FieldComparison {
                path: path.iter().map(|s| (*s).to_owned()).collect(),
                op: op as i32,
                value: Some(pb::LiteralValue { value }),
            })),
        }
    }

    fn pair(left: Option<pb::HistoryPredicate>, right: Option<pb::HistoryPredicate>) -> Box<pb::PredicatePair> {
        Box::new(pb::PredicatePair {
            left: left.map(Box::new),
            right: right.map(Box::new),
        })
    }

    fn all_of(left: pb::HistoryPredicate, right: pb::HistoryPredicate) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::AllOf(pair(Some(left), Some(right)))),
        }
    }

    fn any_of(left: pb::HistoryPredicate, right: pb::HistoryPredicate) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::AnyOf(pair(Some(left), Some(right)))),
        }
    }

    fn negation(inner: pb::HistoryPredicate) -> pb::HistoryPredicate {
        pb::HistoryPredicate {
            node: Some(history_predicate::Node::Negation(Box::new(inner))),
        }
    }

    fn formerly(slot: &str, predicate: pb::HistoryPredicate) -> pb::HistoryCondition {
        pb::HistoryCondition {
            slot: slot.to_owned(),
            policy_id: "p".to_owned(),
            scope: pb::HistoryScope::Session as i32,
            window_ms: 60_000,
            shape: Some(history_condition::Shape::Formerly(pb::FormerlyShape { predicate: Some(predicate) })),
        }
    }

    fn map_one(condition: pb::HistoryCondition) -> Result<Vec<Condition>, MalformedCondition> {
        map_conditions(Part::Tenant, &[condition], MAX_WINDOW, &floor_slots())
    }

    fn predicate_of(c: &Condition) -> &Predicate {
        match &c.shape {
            Shape::Formerly { predicate } => predicate,
            other => panic!("not a formerly shape: {other:?}"),
        }
    }

    fn path(segments: &[&str]) -> FieldPath {
        FieldPath::new(segments.iter().map(|s| (*s).to_owned()).collect()).unwrap()
    }

    #[test]
    fn every_node_and_every_literal_kind_maps() {
        let predicate = all_of(
            event_kind(pb::HistoryEventKind::Request),
            any_of(
                tool("send_mail", false),
                negation(all_of(
                    tool("", true),
                    all_of(
                        field(&["amount"], pb::ComparisonOp::Ge, Some(literal_value::Value::LongValue(7))),
                        any_of(
                            field(&["to", "domain"], pb::ComparisonOp::Eq, Some(literal_value::Value::StringValue("x.test".into()))),
                            any_of(
                                field(&["ok"], pb::ComparisonOp::Eq, Some(literal_value::Value::BoolValue(true))),
                                any_of(
                                    field(&["rate"], pb::ComparisonOp::Eq, Some(literal_value::Value::DecimalValue("1.5".into()))),
                                    field(&["delta"], pb::ComparisonOp::Eq, Some(literal_value::Value::DecimalValue("-0.0001".into()))),
                                ),
                            ),
                        ),
                    ),
                )),
            ),
        );
        let got = map_one(formerly("s1", predicate)).unwrap();
        let test = |p: &[&str], op, literal| Predicate::Field(FieldTest::new(path(p), op, literal));
        let expected = Predicate::AllOf(
            Box::new(Predicate::EventKind(EventKind::Request)),
            Box::new(Predicate::AnyOf(
                Box::new(Predicate::Tool(ToolMatch::NameKey("send_mail".into()))),
                Box::new(Predicate::Not(Box::new(Predicate::AllOf(
                    Box::new(Predicate::Tool(ToolMatch::AnyCatalogTool)),
                    Box::new(Predicate::AllOf(
                        Box::new(test(&["amount"], CmpOp::Ge, Literal::Long(7))),
                        Box::new(Predicate::AnyOf(
                            Box::new(test(&["to", "domain"], CmpOp::Eq, Literal::Unbound(Scalar::Str("x.test".into())))),
                            Box::new(Predicate::AnyOf(
                                Box::new(test(&["ok"], CmpOp::Eq, Literal::Unbound(Scalar::Bool(true)))),
                                Box::new(Predicate::AnyOf(
                                    Box::new(test(&["rate"], CmpOp::Eq, Literal::Unbound(Scalar::Decimal(15_000)))),
                                    Box::new(test(&["delta"], CmpOp::Eq, Literal::Unbound(Scalar::Decimal(-1)))),
                                )),
                            )),
                        )),
                    )),
                )))),
            )),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(predicate_of(&got[0]), &expected);
        assert_eq!((got[0].slot.as_str(), got[0].part, got[0].scope, got[0].window_ms), ("s1", Part::Tenant, Scope::Session, 60_000));
    }

    #[test]
    fn the_count_within_and_sum_within_shapes_map() {
        let count = pb::HistoryCondition {
            shape: Some(history_condition::Shape::CountWithin(pb::CountWithinShape {
                predicate: Some(tool("t", false)),
                op: pb::ComparisonOp::Ge as i32,
                threshold: 3,
            })),
            scope: pb::HistoryScope::Principal as i32,
            ..formerly("c1", tool("t", false))
        };
        let sum = pb::HistoryCondition {
            shape: Some(history_condition::Shape::SumWithin(pb::SumWithinShape {
                predicate: Some(tool("t", false)),
                field_path: vec!["a".into(), "b".into()],
                op: pb::ComparisonOp::Gt as i32,
                threshold: 9,
            })),
            scope: pb::HistoryScope::Key as i32,
            ..formerly("c2", tool("t", false))
        };
        let got = map_conditions(Part::Floor, &[count, sum], MAX_WINDOW, &BTreeSet::new()).unwrap();
        assert_eq!(
            got[0].shape,
            Shape::CountWithin {
                predicate: Predicate::Tool(ToolMatch::NameKey("t".into())),
                op: CmpOp::Ge,
                threshold: 3
            }
        );
        assert_eq!(got[0].scope, Scope::Principal);
        assert_eq!(
            got[1].shape,
            Shape::SumWithin {
                predicate: Predicate::Tool(ToolMatch::NameKey("t".into())),
                path: path(&["a", "b"]),
                op: CmpOp::Gt,
                threshold: 9
            }
        );
        assert_eq!((got[1].scope, got[1].part), (Scope::Key, Part::Floor));
    }

    fn nested(levels: usize) -> pb::HistoryPredicate {
        let mut p = tool("t", false);
        for _ in 1..levels {
            p = negation(p);
        }
        p
    }

    /// One malformed condition for each row of the reason table.
    fn malformed_cases() -> Vec<(&'static str, pb::HistoryCondition)> {
        let ok = || formerly("s1", tool("t", false));
        let with_predicate = |p: pb::HistoryPredicate| formerly("s1", p);
        let sum = |field_path: Vec<String>| pb::HistoryCondition {
            shape: Some(history_condition::Shape::SumWithin(pb::SumWithinShape {
                predicate: Some(tool("t", false)),
                field_path,
                op: pb::ComparisonOp::Gt as i32,
                threshold: 1,
            })),
            ..ok()
        };
        let long = || Some(literal_value::Value::LongValue(1));
        vec![
            ("shape_unset", pb::HistoryCondition { shape: None, ..ok() }),
            ("node_unset", with_predicate(pb::HistoryPredicate { node: None })),
            (
                "node_unset",
                pb::HistoryCondition {
                    shape: Some(history_condition::Shape::Formerly(pb::FormerlyShape { predicate: None })),
                    ..ok()
                },
            ),
            ("value_unset", with_predicate(field(&["a"], pb::ComparisonOp::Eq, None))),
            (
                "value_unset",
                with_predicate(pb::HistoryPredicate {
                    node: Some(history_predicate::Node::Field(pb::FieldComparison {
                        path: vec!["a".into()],
                        op: pb::ComparisonOp::Eq as i32,
                        value: None,
                    })),
                }),
            ),
            (
                "pair_side_missing",
                with_predicate(pb::HistoryPredicate {
                    node: Some(history_predicate::Node::AllOf(pair(None, Some(tool("t", false))))),
                }),
            ),
            (
                "pair_side_missing",
                with_predicate(pb::HistoryPredicate {
                    node: Some(history_predicate::Node::AnyOf(pair(Some(tool("t", false)), None))),
                }),
            ),
            (
                "enum_unknown",
                pb::HistoryCondition {
                    scope: pb::HistoryScope::Unspecified as i32,
                    ..ok()
                },
            ),
            ("enum_unknown", pb::HistoryCondition { scope: 99, ..ok() }),
            ("enum_unknown", with_predicate(event_kind(pb::HistoryEventKind::Unspecified))),
            (
                "enum_unknown",
                with_predicate(pb::HistoryPredicate {
                    node: Some(history_predicate::Node::EventKind(42)),
                }),
            ),
            ("enum_unknown", with_predicate(field(&["a"], pb::ComparisonOp::Unspecified, long()))),
            (
                "enum_unknown",
                pb::HistoryCondition {
                    shape: Some(history_condition::Shape::CountWithin(pb::CountWithinShape {
                        predicate: Some(tool("t", false)),
                        op: 77,
                        threshold: 1,
                    })),
                    ..ok()
                },
            ),
            ("tool_match", with_predicate(tool("t", true))),
            ("tool_match", with_predicate(tool("", false))),
            ("name_key_not_normal", with_predicate(tool("Send_Mail", false))),
            ("path", with_predicate(field(&[], pb::ComparisonOp::Eq, long()))),
            ("path", with_predicate(field(&["a", ""], pb::ComparisonOp::Eq, long()))),
            ("path", with_predicate(field(&["a.b"], pb::ComparisonOp::Eq, long()))),
            ("path", sum(vec![])),
            ("path", sum(vec!["a".into(), String::new()])),
            ("path", sum(vec!["a.b".into()])),
            ("window", pb::HistoryCondition { window_ms: 0, ..ok() }),
            ("window", pb::HistoryCondition { window_ms: MAX_WINDOW + 1, ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: String::new(), ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: "1abc".into(), ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: "a-b".into(), ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: "a b".into(), ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: "input".into(), ..ok() }),
            ("slot_name", pb::HistoryCondition { slot: "session".into(), ..ok() }),
            ("slot_clash", pb::HistoryCondition { slot: "floor_slot".into(), ..ok() }),
            ("depth", with_predicate(nested(MAX_PREDICATE_DEPTH + 1))),
            (
                "decimal",
                with_predicate(field(&["a"], pb::ComparisonOp::Eq, Some(literal_value::Value::DecimalValue("1.23456".into())))),
            ),
            ("op_not_eq", with_predicate(field(&["a"], pb::ComparisonOp::Lt, Some(literal_value::Value::StringValue("x".into()))))),
            ("op_not_eq", with_predicate(field(&["a"], pb::ComparisonOp::Ne, Some(literal_value::Value::BoolValue(true))))),
            ("op_not_eq", with_predicate(field(&["a"], pb::ComparisonOp::Gt, Some(literal_value::Value::DecimalValue("1.5".into()))))),
        ]
    }

    #[test]
    fn each_malformed_form_is_refused_with_its_reason() {
        for (reason, condition) in malformed_cases() {
            let err = map_conditions(Part::Tenant, std::slice::from_ref(&condition), MAX_WINDOW, &floor_slots()).unwrap_err();
            assert_eq!((err.index, err.reason), (0, reason), "case {condition:?}");
        }
    }

    #[test]
    fn a_duplicate_slot_is_refused_at_the_index_of_the_second_condition() {
        let two = [formerly("s1", tool("t", false)), formerly("s1", tool("u", false))];
        let err = map_conditions(Part::Floor, &two, MAX_WINDOW, &BTreeSet::new()).unwrap_err();
        assert_eq!((err.index, err.reason), (1, "slot_duplicate"));
    }

    #[test]
    fn a_floor_slot_name_clashes_only_for_the_tenant_part() {
        let c = formerly("floor_slot", tool("t", false));
        assert_eq!(map_conditions(Part::Floor, std::slice::from_ref(&c), MAX_WINDOW, &floor_slots()).unwrap().len(), 1);
        assert_eq!(map_conditions(Part::Tenant, &[c], MAX_WINDOW, &floor_slots()).unwrap_err().reason, "slot_clash");
    }

    #[test]
    fn a_window_equal_to_the_maximum_loads() {
        let c = pb::HistoryCondition {
            window_ms: MAX_WINDOW,
            ..formerly("s1", tool("t", false))
        };
        assert_eq!(map_one(c).unwrap()[0].window_ms, MAX_WINDOW);
    }

    #[test]
    fn a_predicate_of_depth_32_loads_and_33_is_refused() {
        assert!(map_one(formerly("s1", nested(32))).is_ok());
        assert_eq!(map_one(formerly("s1", nested(33))).unwrap_err().reason, "depth");
        // A pair adds one level to each side, so a deep right side counts too.
        let deep_pair = |levels| formerly("s1", all_of(tool("t", false), nested(levels)));
        assert!(map_one(deep_pair(31)).is_ok());
        assert_eq!(map_one(deep_pair(32)).unwrap_err().reason, "depth");
    }

    #[test]
    fn a_decimal_with_five_fraction_digits_or_out_of_range_is_refused() {
        for bad in [
            "1.23456",
            "1",
            "1.",
            ".5",
            "922337203685477.5808",
            "1e2",
            "",
            "-",
            "-.5",
            "+1.5",
            "1.5 ",
            "-922337203685477.5809",
            "\u{661}.\u{665}",
        ] {
            let c = formerly("s1", field(&["a"], pb::ComparisonOp::Eq, Some(literal_value::Value::DecimalValue(bad.into()))));
            assert_eq!(map_one(c).unwrap_err().reason, "decimal", "literal {bad:?}");
        }
    }

    #[test]
    fn a_decimal_at_the_limits_maps_to_the_scaled_value() {
        for (text, scaled) in [
            ("0.0", 0),
            ("1.0", 10_000),
            ("-1.25", -12_500),
            ("922337203685477.5807", i64::MAX),
            ("-922337203685477.5808", i64::MIN),
            ("007.5", 75_000),
        ] {
            assert_eq!(parse_decimal(text), Some(scaled), "literal {text:?}");
        }
    }

    #[test]
    fn a_long_literal_accepts_every_comparison_operator() {
        for op in [
            pb::ComparisonOp::Lt,
            pb::ComparisonOp::Le,
            pb::ComparisonOp::Eq,
            pb::ComparisonOp::Ge,
            pb::ComparisonOp::Gt,
            pb::ComparisonOp::Ne,
        ] {
            assert!(map_one(formerly("s1", field(&["a"], op, Some(literal_value::Value::LongValue(1))))).is_ok(), "op {op:?}");
        }
    }

    #[test]
    fn a_name_key_that_is_already_normal_loads() {
        assert_eq!(map_one(formerly("_s9", tool(&normalise("Send Mail"), false))).unwrap().len(), 1);
    }

    #[test]
    fn the_error_text_names_the_index_and_the_reason() {
        let err = MalformedCondition { index: 2, reason: "window" };
        assert_eq!(err.to_string(), "history condition 2 is malformed: window");
    }
}
