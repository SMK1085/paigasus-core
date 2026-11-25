// SPDX-License-Identifier: Apache-2.0
//! The tool sets of a predicate and the digest binding (SMA-753 spec P1, P3).
//!
//! `Predicate::tools` and `Predicate::field_tests` share the one walk in this file.

use uuid::Uuid;

use super::fields::HistoryDigester;
use super::types::{Condition, FieldTest, Literal, Predicate, Shape, ToolMatch, ToolSet};

impl ToolSet {
    fn every() -> Self {
        Self { every: true, ..Self::default() }
    }

    fn union(self, other: Self) -> Self {
        let mut names = self.names;
        names.extend(other.names);
        Self {
            names,
            catalog: self.catalog || other.catalog,
            every: self.every || other.every,
        }
    }

    /// The tools that both sides match. A side with `every` is neutral. Without the catalog
    /// contents, a catalog side keeps the names of the other side (an over-approximation).
    fn intersect(self, other: Self) -> Self {
        let catalog = self.catalog || other.catalog;
        if self.every && other.every {
            return self.union(other);
        }
        if self.every {
            return Self { catalog, ..other };
        }
        if other.every {
            return Self { catalog, ..self };
        }
        let mut names: std::collections::BTreeSet<String> = self.names.intersection(&other.names).cloned().collect();
        if self.catalog {
            names.extend(other.names.iter().cloned());
        }
        if other.catalog {
            names.extend(self.names.iter().cloned());
        }
        Self { names, catalog, every: false }
    }
}

impl Predicate {
    /// The tool set of the whole predicate.
    pub fn tools(&self) -> ToolSet {
        self.walk().0
    }

    /// One `(ToolSet, &FieldTest)` for each `FieldTest`, in predicate order, where the tool set is
    /// the one that holds at that test (spec P3).
    pub fn field_tests(&self) -> Vec<(ToolSet, &FieldTest)> {
        self.walk().1
    }

    /// The one walk: the tool set of `self`, and each field test with its tool set.
    fn walk(&self) -> (ToolSet, Vec<(ToolSet, &FieldTest)>) {
        match self {
            Self::Tool(ToolMatch::NameKey(k)) => (
                ToolSet {
                    names: [k.clone()].into(),
                    ..ToolSet::default()
                },
                vec![],
            ),
            Self::Tool(ToolMatch::AnyCatalogTool) => (ToolSet { catalog: true, ..ToolSet::default() }, vec![]),
            Self::EventKind(_) => (ToolSet::every(), vec![]),
            Self::Field(test) => (ToolSet::every(), vec![(ToolSet::every(), test)]),
            Self::Not(inner) => {
                let (set, tests) = inner.walk();
                (
                    ToolSet {
                        catalog: set.catalog,
                        ..ToolSet::every()
                    },
                    tests,
                )
            }
            Self::AnyOf(a, b) => {
                let (sa, mut ta) = a.walk();
                let (sb, tb) = b.walk();
                ta.extend(tb);
                (sa.union(sb), ta)
            }
            Self::AllOf(a, b) => {
                let (sa, ta) = a.walk();
                let (sb, tb) = b.walk();
                let mut tests: Vec<_> = ta.into_iter().map(|(s, t)| (s.intersect(sb.clone()), t)).collect();
                tests.extend(tb.into_iter().map(|(s, t)| (s.intersect(sa.clone()), t)));
                (sa.intersect(sb), tests)
            }
        }
    }

    fn bind(&self, org: Uuid, d: &dyn HistoryDigester) -> Self {
        match self {
            Self::Field(test) => {
                let literal = match &test.literal {
                    Literal::Unbound(value) => Literal::Digests(d.all(org, &test.path, value)),
                    other => other.clone(),
                };
                Self::Field(FieldTest::new(test.path.clone(), test.op, literal))
            }
            Self::AllOf(a, b) => Self::AllOf(Box::new(a.bind(org, d)), Box::new(b.bind(org, d))),
            Self::AnyOf(a, b) => Self::AnyOf(Box::new(a.bind(org, d)), Box::new(b.bind(org, d))),
            Self::Not(a) => Self::Not(Box::new(a.bind(org, d))),
            other => other.clone(),
        }
    }
}

impl Condition {
    /// The tool set of the predicate of the shape.
    pub fn tools(&self) -> ToolSet {
        self.shape.predicate().tools()
    }

    /// The condition with each `Literal::Unbound` replaced by its digests for `org` (plan decision
    /// D-A). With no history key, the digests are empty, and the slot is missing (`no_key`).
    pub fn bind_digests(&self, org: Uuid, d: &dyn HistoryDigester) -> Condition {
        let shape = match &self.shape {
            Shape::Formerly { predicate } => Shape::Formerly { predicate: predicate.bind(org, d) },
            Shape::CountWithin { predicate, op, threshold } => Shape::CountWithin {
                predicate: predicate.bind(org, d),
                op: *op,
                threshold: *threshold,
            },
            Shape::SumWithin { predicate, path, op, threshold } => Shape::SumWithin {
                predicate: predicate.bind(org, d),
                path: path.clone(),
                op: *op,
                threshold: *threshold,
            },
        };
        Condition { shape, ..self.clone() }
    }
}
