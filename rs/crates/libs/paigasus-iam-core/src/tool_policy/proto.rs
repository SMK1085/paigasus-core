// SPDX-License-Identifier: Apache-2.0

//! The mapping between the protocol [`HistoryCondition`] and its `paigasus.iam.v1` proto form
//! (SMA-753 spec P15). IAM stores the proto form in a compiled set, and the gateway reads it. The
//! gateway-core parity replay runs the same mapping, so it lives here and not in the IAM service.

use paigasus_proto::paigasus::iam::v1 as pb;

use super::protocol::{CmpOp, EventKind, HistoryCondition, Literal, Predicate, Scope, Shape};

/// A proto condition that does not map back to a protocol condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    /// An unknown enum value, an empty `oneof` or a contradictory tool match. The text is a fixed
    /// reason.
    #[error("a stored history condition is malformed: {0}")]
    Malformed(&'static str),
}

/// The proto form of one condition.
#[must_use]
pub fn condition_to_pb(c: &HistoryCondition) -> pb::HistoryCondition {
    let shape = match &c.shape {
        Shape::Formerly { predicate } => pb::history_condition::Shape::Formerly(pb::FormerlyShape {
            predicate: Some(predicate_to_pb(predicate)),
        }),
        Shape::CountWithin { predicate, op, threshold } => pb::history_condition::Shape::CountWithin(pb::CountWithinShape {
            predicate: Some(predicate_to_pb(predicate)),
            op: op_to_pb(*op) as i32,
            threshold: *threshold,
        }),
        Shape::SumWithin { predicate, field_path, op, threshold } => pb::history_condition::Shape::SumWithin(pb::SumWithinShape {
            predicate: Some(predicate_to_pb(predicate)),
            field_path: field_path.clone(),
            op: op_to_pb(*op) as i32,
            threshold: *threshold,
        }),
    };
    let scope = match c.scope {
        Scope::Session => pb::HistoryScope::Session,
        Scope::Principal => pb::HistoryScope::Principal,
        Scope::Key => pb::HistoryScope::Key,
    };
    pb::HistoryCondition {
        slot: c.slot.clone(),
        policy_id: c.policy_id.clone(),
        scope: scope as i32,
        window_ms: c.window_ms,
        shape: Some(shape),
    }
}

/// The protocol form of one proto condition.
///
/// # Errors
/// [`ProtoError::Malformed`] for an unknown scope, operator or event kind, a missing shape,
/// predicate node or literal value, or a tool match that sets both or neither of its fields.
pub fn condition_from_pb(c: pb::HistoryCondition) -> Result<HistoryCondition, ProtoError> {
    let scope = match pb::HistoryScope::try_from(c.scope) {
        Ok(pb::HistoryScope::Session) => Scope::Session,
        Ok(pb::HistoryScope::Principal) => Scope::Principal,
        Ok(pb::HistoryScope::Key) => Scope::Key,
        _ => return Err(ProtoError::Malformed("a condition has an unknown scope")),
    };
    let shape = match c.shape.ok_or(ProtoError::Malformed("a condition has no shape"))? {
        pb::history_condition::Shape::Formerly(s) => Shape::Formerly {
            predicate: predicate_from_pb(s.predicate)?,
        },
        pb::history_condition::Shape::CountWithin(s) => Shape::CountWithin {
            predicate: predicate_from_pb(s.predicate)?,
            op: op_from_pb(s.op)?,
            threshold: s.threshold,
        },
        pb::history_condition::Shape::SumWithin(s) => Shape::SumWithin {
            predicate: predicate_from_pb(s.predicate)?,
            field_path: s.field_path,
            op: op_from_pb(s.op)?,
            threshold: s.threshold,
        },
    };
    Ok(HistoryCondition {
        slot: c.slot,
        policy_id: c.policy_id,
        scope,
        window_ms: c.window_ms,
        shape,
    })
}

fn op_to_pb(op: CmpOp) -> pb::ComparisonOp {
    match op {
        CmpOp::Lt => pb::ComparisonOp::Lt,
        CmpOp::Le => pb::ComparisonOp::Le,
        CmpOp::Eq => pb::ComparisonOp::Eq,
        CmpOp::Ge => pb::ComparisonOp::Ge,
        CmpOp::Gt => pb::ComparisonOp::Gt,
        CmpOp::Ne => pb::ComparisonOp::Ne,
    }
}

fn op_from_pb(op: i32) -> Result<CmpOp, ProtoError> {
    match pb::ComparisonOp::try_from(op) {
        Ok(pb::ComparisonOp::Lt) => Ok(CmpOp::Lt),
        Ok(pb::ComparisonOp::Le) => Ok(CmpOp::Le),
        Ok(pb::ComparisonOp::Eq) => Ok(CmpOp::Eq),
        Ok(pb::ComparisonOp::Ge) => Ok(CmpOp::Ge),
        Ok(pb::ComparisonOp::Gt) => Ok(CmpOp::Gt),
        Ok(pb::ComparisonOp::Ne) => Ok(CmpOp::Ne),
        _ => Err(ProtoError::Malformed("a condition has an unknown comparison operator")),
    }
}

fn predicate_to_pb(p: &Predicate) -> pb::HistoryPredicate {
    use pb::history_predicate::Node;
    let pair = |left: &Predicate, right: &Predicate| {
        Box::new(pb::PredicatePair {
            left: Some(Box::new(predicate_to_pb(left))),
            right: Some(Box::new(predicate_to_pb(right))),
        })
    };
    let node = match p {
        Predicate::EventKind { kind } => Node::EventKind(match kind {
            EventKind::Request => pb::HistoryEventKind::Request,
            EventKind::Response => pb::HistoryEventKind::Response,
        } as i32),
        Predicate::Tool { name_key } => Node::Tool(pb::ToolMatch {
            name_key: name_key.clone(),
            any_catalog_tool: false,
        }),
        Predicate::AnyCatalogTool {} => Node::Tool(pb::ToolMatch {
            name_key: String::new(),
            any_catalog_tool: true,
        }),
        Predicate::Field { path, op, value } => Node::Field(pb::FieldComparison {
            path: path.clone(),
            op: op_to_pb(*op) as i32,
            value: Some(pb::LiteralValue {
                value: Some(match value {
                    Literal::Long(v) => pb::literal_value::Value::LongValue(*v),
                    Literal::String(v) => pb::literal_value::Value::StringValue(v.clone()),
                    Literal::Bool(v) => pb::literal_value::Value::BoolValue(*v),
                    Literal::Decimal(v) => pb::literal_value::Value::DecimalValue(v.clone()),
                }),
            }),
        }),
        Predicate::AllOf { left, right } => Node::AllOf(pair(left, right)),
        Predicate::AnyOf { left, right } => Node::AnyOf(pair(left, right)),
        Predicate::Negation { inner } => Node::Negation(Box::new(predicate_to_pb(inner))),
    };
    pb::HistoryPredicate { node: Some(node) }
}

fn predicate_from_pb(p: Option<pb::HistoryPredicate>) -> Result<Predicate, ProtoError> {
    use pb::history_predicate::Node;
    let node = p.and_then(|p| p.node).ok_or(ProtoError::Malformed("a predicate has no node"))?;
    let pair = |pair: Box<pb::PredicatePair>| -> Result<(Box<Predicate>, Box<Predicate>), ProtoError> {
        let pair = *pair;
        Ok((Box::new(predicate_from_pb(pair.left.map(|b| *b))?), Box::new(predicate_from_pb(pair.right.map(|b| *b))?)))
    };
    Ok(match node {
        Node::EventKind(kind) => Predicate::EventKind {
            kind: match pb::HistoryEventKind::try_from(kind) {
                Ok(pb::HistoryEventKind::Request) => EventKind::Request,
                Ok(pb::HistoryEventKind::Response) => EventKind::Response,
                _ => return Err(ProtoError::Malformed("a predicate has an unknown event kind")),
            },
        },
        Node::Tool(t) if t.any_catalog_tool && t.name_key.is_empty() => Predicate::AnyCatalogTool {},
        Node::Tool(t) if !t.any_catalog_tool && !t.name_key.is_empty() => Predicate::Tool { name_key: t.name_key },
        Node::Tool(_) => return Err(ProtoError::Malformed("a tool match sets both or neither of its fields")),
        Node::Field(f) => {
            let value = match f.value.and_then(|v| v.value).ok_or(ProtoError::Malformed("a field comparison has no value"))? {
                pb::literal_value::Value::LongValue(v) => Literal::Long(v),
                pb::literal_value::Value::StringValue(v) => Literal::String(v),
                pb::literal_value::Value::BoolValue(v) => Literal::Bool(v),
                pb::literal_value::Value::DecimalValue(v) => Literal::Decimal(v),
            };
            Predicate::Field {
                path: f.path,
                op: op_from_pb(f.op)?,
                value,
            }
        }
        Node::AllOf(p) => {
            let (left, right) = pair(p)?;
            Predicate::AllOf { left, right }
        }
        Node::AnyOf(p) => {
            let (left, right) = pair(p)?;
            Predicate::AnyOf { left, right }
        }
        Node::Negation(inner) => Predicate::Negation {
            inner: Box::new(predicate_from_pb(Some(*inner))?),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(value: Literal, op: CmpOp) -> Predicate {
        Predicate::Field {
            path: vec!["a".into(), "b".into()],
            op,
            value,
        }
    }

    fn condition(slot: &str, scope: Scope, shape: Shape) -> HistoryCondition {
        HistoryCondition {
            slot: slot.into(),
            policy_id: "p".into(),
            scope,
            window_ms: 60_000,
            shape,
        }
    }

    fn both(left: Predicate, right: Predicate) -> Predicate {
        Predicate::AllOf {
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Every predicate node (`event_kind` of both kinds, `tool`, `any_catalog_tool`, `field`,
    /// `all_of`, `any_of`, `negation`), every literal kind, every operator, every scope and every
    /// shape.
    fn every_kind() -> Vec<HistoryCondition> {
        let tree = both(
            Predicate::EventKind { kind: EventKind::Request },
            Predicate::AnyOf {
                left: Box::new(Predicate::Tool { name_key: "gh.search".into() }),
                right: Box::new(Predicate::Negation {
                    inner: Box::new(Predicate::AnyCatalogTool {}),
                }),
            },
        );
        let literals = both(
            both(field(Literal::Long(-5), CmpOp::Lt), field(Literal::String("x".into()), CmpOp::Eq)),
            both(field(Literal::Bool(true), CmpOp::Ne), field(Literal::Decimal("1.50".into()), CmpOp::Ge)),
        );
        let ops = both(
            both(field(Literal::Long(1), CmpOp::Le), field(Literal::Long(2), CmpOp::Gt)),
            Predicate::EventKind { kind: EventKind::Response },
        );
        vec![
            condition("s0", Scope::Session, Shape::Formerly { predicate: tree }),
            condition(
                "s1",
                Scope::Principal,
                Shape::CountWithin {
                    predicate: literals,
                    op: CmpOp::Le,
                    threshold: 3,
                },
            ),
            condition(
                "s2",
                Scope::Key,
                Shape::SumWithin {
                    predicate: ops,
                    field_path: vec!["amount".into()],
                    op: CmpOp::Gt,
                    threshold: 500,
                },
            ),
        ]
    }

    #[test]
    fn condition_round_trips_through_the_proto() {
        for c in every_kind() {
            let pb = condition_to_pb(&c);
            assert_eq!(condition_from_pb(pb), Ok(c));
        }
    }

    #[test]
    fn the_proto_form_holds_the_wire_values() {
        let pb = condition_to_pb(&every_kind()[0]);
        assert_eq!((pb.scope, pb.window_ms, pb.slot.as_str()), (pb::HistoryScope::Session as i32, 60_000, "s0"));
        let Some(pb::history_condition::Shape::Formerly(shape)) = pb.shape else {
            panic!("a formerly shape");
        };
        let Some(pb::history_predicate::Node::AllOf(pair)) = shape.predicate.and_then(|p| p.node) else {
            panic!("an all_of root");
        };
        assert_eq!(pair.left.and_then(|p| p.node), Some(pb::history_predicate::Node::EventKind(pb::HistoryEventKind::Request as i32)));
    }

    fn formerly(predicate: pb::HistoryPredicate) -> pb::HistoryCondition {
        pb::HistoryCondition {
            scope: pb::HistoryScope::Session as i32,
            shape: Some(pb::history_condition::Shape::Formerly(pb::FormerlyShape { predicate: Some(predicate) })),
            ..Default::default()
        }
    }

    fn node(n: pb::history_predicate::Node) -> pb::HistoryPredicate {
        pb::HistoryPredicate { node: Some(n) }
    }

    #[test]
    fn each_malformed_form_is_refused_with_its_reason() {
        use pb::history_predicate::Node;
        let tool = |name_key: &str, any_catalog_tool: bool| {
            node(Node::Tool(pb::ToolMatch {
                name_key: name_key.into(),
                any_catalog_tool,
            }))
        };
        let count = |op: i32| pb::HistoryCondition {
            scope: pb::HistoryScope::Key as i32,
            shape: Some(pb::history_condition::Shape::CountWithin(pb::CountWithinShape {
                predicate: Some(tool("t", false)),
                op,
                threshold: 1,
            })),
            ..Default::default()
        };
        let cases: Vec<(pb::HistoryCondition, &str)> = vec![
            (
                pb::HistoryCondition {
                    scope: 99,
                    ..formerly(tool("t", false))
                },
                "a condition has an unknown scope",
            ),
            (
                pb::HistoryCondition {
                    scope: pb::HistoryScope::Unspecified as i32,
                    ..formerly(tool("t", false))
                },
                "a condition has an unknown scope",
            ),
            (
                pb::HistoryCondition {
                    shape: None,
                    ..formerly(tool("t", false))
                },
                "a condition has no shape",
            ),
            (formerly(pb::HistoryPredicate { node: None }), "a predicate has no node"),
            (count(99), "a condition has an unknown comparison operator"),
            (formerly(node(Node::EventKind(99))), "a predicate has an unknown event kind"),
            (formerly(tool("t", true)), "a tool match sets both or neither of its fields"),
            (formerly(tool("", false)), "a tool match sets both or neither of its fields"),
            (
                formerly(node(Node::Field(pb::FieldComparison {
                    path: vec!["a".into()],
                    op: pb::ComparisonOp::Eq as i32,
                    value: None,
                }))),
                "a field comparison has no value",
            ),
            (
                formerly(node(Node::AllOf(Box::new(pb::PredicatePair {
                    left: Some(Box::new(tool("t", false))),
                    right: None,
                })))),
                "a predicate has no node",
            ),
        ];
        for (c, reason) in cases {
            assert_eq!(condition_from_pb(c), Err(ProtoError::Malformed(reason)), "{reason}");
        }
        assert_eq!(condition_from_pb(count(pb::ComparisonOp::Ge as i32)).map(|c| c.scope), Ok(Scope::Key), "the control is well formed");
    }
}
