// SPDX-License-Identifier: Apache-2.0
//! The three-valued history evaluator (SMA-753 spec P1). One pure function: no clock, no I/O.

use std::collections::{BTreeSet, HashSet};

use super::fields::{Field, Fields};
use super::types::{Catalog, CmpOp, Condition, Evaluation, Event, FieldTest, Literal, Missing, MissingReason, Predicate, RETENTION_MS, SetTool, Shape, Slots, ToolMatch, Window, WindowSet};

/// Kleene truth values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tri {
    True,
    False,
    Unknown,
}

impl Tri {
    fn of(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// Evaluate each condition on its own. Call `index` sees `hold[0..=index]` only.
pub fn evaluate(conditions: &[Condition], window: &Window, hold: &[Event], index: usize, now_ms: u64, catalog: &Catalog) -> Evaluation {
    let hold = &hold[..hold.len().min(index.saturating_add(1))];
    let mut slots = Slots::new();
    let mut missing = Vec::new();
    for condition in conditions {
        match evaluate_one(condition, window, hold, now_ms, catalog) {
            Ok(value) => slots.insert(condition.slot.clone(), value),
            Err(reason) => missing.push(Missing::new(condition.slot.clone(), condition.part, reason)),
        }
    }
    Evaluation::new(slots, missing)
}

fn evaluate_one(condition: &Condition, window: &Window, hold: &[Event], now_ms: u64, catalog: &Catalog) -> Result<bool, MissingReason> {
    if condition.window_ms > RETENTION_MS {
        return Err(MissingReason::Retention);
    }
    let predicate = condition.shape.predicate();
    let unbound = predicate.field_tests().iter().any(|(_, t)| match &t.literal {
        Literal::Unbound(_) => true,
        Literal::Digests(list) => list.is_empty(),
        Literal::Long(_) => false,
    });
    if unbound {
        return Err(MissingReason::NoKey);
    }
    let tools = condition.tools();
    if tools.catalog && catalog.0.is_none() {
        return Err(MissingReason::Catalog);
    }
    let sets = window.0.as_ref().map_err(|reason| *reason)?;
    let start = now_ms.saturating_sub(condition.window_ms);
    let of_scope: Vec<&WindowSet> = sets.iter().filter(|s| s.scope == condition.scope).collect();

    let (required, read): (BTreeSet<SetTool>, Vec<&WindowSet>) = if tools.requires_every(catalog) {
        ([SetTool::Every].into(), of_scope.clone())
    } else {
        let mut names = tools.names.clone();
        if tools.catalog {
            names.extend(catalog.0.iter().flatten().cloned());
        }
        let required: BTreeSet<SetTool> = names.into_iter().map(SetTool::Tool).collect();
        let read = of_scope.iter().copied().filter(|s| required.contains(&s.tool)).collect();
        (required, read)
    };
    if required.iter().any(|tool| !of_scope.iter().any(|s| &s.tool == tool)) {
        return Err(MissingReason::Store);
    }
    if read.iter().any(|s| s.truncated || s.mark_ms.is_some_and(|mark| mark >= start)) {
        return Err(MissingReason::Truncated);
    }

    let mut seen = HashSet::new();
    let events = read.iter().flat_map(|s| s.events.iter()).chain(hold).filter(|e| e.ts_ms >= start && seen.insert(e.id));
    let results = events.map(|e| (matches(predicate, e, catalog), e));
    match &condition.shape {
        Shape::Formerly { .. } => formerly(results.map(|(t, _)| t)),
        Shape::CountWithin { op, threshold, .. } => count_within(results.map(|(t, _)| t), *op, *threshold),
        Shape::SumWithin { path, op, threshold, .. } => {
            let mut sum: i64 = 0;
            for (t, e) in results {
                match t {
                    Tri::False => continue,
                    Tri::Unknown => return Err(MissingReason::Unknown),
                    Tri::True => {}
                }
                let add = match &e.fields {
                    Fields::Unparsed => return Err(MissingReason::Unknown),
                    Fields::Values(map) => match map.get(path) {
                        None | Some(Field::Absent) => 0,
                        Some(Field::Long(n)) => (*n).max(0),
                        Some(_) => return Err(MissingReason::Unknown),
                    },
                };
                sum = sum.saturating_add(add);
            }
            Ok(op.apply(sum, *threshold))
        }
    }
}

fn formerly(results: impl Iterator<Item = Tri>) -> Result<bool, MissingReason> {
    let mut unknown = false;
    for t in results {
        match t {
            Tri::True => return Ok(true),
            Tri::Unknown => unknown = true,
            Tri::False => {}
        }
    }
    if unknown { Err(MissingReason::Unknown) } else { Ok(false) }
}

/// The value only when the comparison agrees for every count from `true` to `true + unknown`.
fn count_within(results: impl Iterator<Item = Tri>, op: CmpOp, threshold: i64) -> Result<bool, MissingReason> {
    let (mut t, mut u) = (0i64, 0i64);
    for r in results {
        match r {
            Tri::True => t = t.saturating_add(1),
            Tri::Unknown => u = u.saturating_add(1),
            Tri::False => {}
        }
    }
    let low = op.apply(t, threshold);
    // A comparison changes its value at most twice over a range (at `threshold` and next to it),
    // so checking the ends and the points next to the threshold covers every count.
    let upper = t.saturating_add(u);
    let points = [upper, threshold.saturating_sub(1), threshold, threshold.saturating_add(1)];
    if points.iter().filter(|c| (t..=upper).contains(c)).all(|c| op.apply(*c, threshold) == low) {
        Ok(low)
    } else {
        Err(MissingReason::Unknown)
    }
}

fn matches(predicate: &Predicate, event: &Event, catalog: &Catalog) -> Tri {
    match predicate {
        Predicate::EventKind(kind) => Tri::of(event.kind == *kind),
        Predicate::Tool(ToolMatch::NameKey(name)) => Tri::of(event.tool == *name),
        Predicate::Tool(ToolMatch::AnyCatalogTool) => catalog.contains(&event.tool).map_or(Tri::Unknown, Tri::of),
        Predicate::Field(test) => field_test(test, event),
        Predicate::AllOf(a, b) => matches(a, event, catalog).and(matches(b, event, catalog)),
        Predicate::AnyOf(a, b) => matches(a, event, catalog).or(matches(b, event, catalog)),
        Predicate::Not(a) => matches(a, event, catalog).not(),
    }
}

/// The table of spec P1.
fn field_test(test: &FieldTest, event: &Event) -> Tri {
    let Fields::Values(map) = &event.fields else {
        return Tri::Unknown;
    };
    match (map.get(&test.path), &test.literal) {
        (None | Some(Field::Absent), _) => Tri::False,
        (Some(Field::Mistyped), _) => Tri::Unknown,
        (Some(Field::Long(v)), Literal::Long(lit)) => Tri::of(test.op.apply(*v, *lit)),
        (Some(Field::Digest(stored)), Literal::Digests(list)) => {
            let Some(found) = event.key.and_then(|kid| list.iter().find(|(id, _)| *id == kid)) else {
                return Tri::Unknown;
            };
            match test.op {
                CmpOp::Eq => Tri::of(found.1 == *stored),
                CmpOp::Ne => Tri::of(found.1 != *stored),
                _ => Tri::Unknown,
            }
        }
        _ => Tri::Unknown,
    }
}
