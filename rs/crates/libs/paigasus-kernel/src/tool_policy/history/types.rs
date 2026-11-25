// SPDX-License-Identifier: Apache-2.0
//! The history condition types (SMA-753 spec P1).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use uuid::Uuid;

use super::fields::{Digest, FieldPath, Fields, KeyId, Scalar};

/// The longest window that the history keeps, in milliseconds (25 hours, design D10).
pub const RETENTION_MS: u64 = 90_000_000;

/// The policy set that a condition comes from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Floor,
    Tenant,
}

impl Part {
    /// The metric label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Tenant => "tenant",
        }
    }
}

/// The events that a condition reads.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Session,
    Principal,
    Key,
}

/// The kind of a history event.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    Request,
    Response,
}

/// A comparison operator.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
    Ne,
}

impl CmpOp {
    /// `a <op> b`.
    pub fn apply(&self, a: i64, b: i64) -> bool {
        match self {
            Self::Lt => a < b,
            Self::Le => a <= b,
            Self::Eq => a == b,
            Self::Ge => a >= b,
            Self::Gt => a > b,
            Self::Ne => a != b,
        }
    }
}

/// The tool that a predicate matches.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolMatch {
    /// The name key of one tool.
    NameKey(String),
    /// Any tool of the tenant catalog.
    AnyCatalogTool,
}

/// The literal of a field test.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Long(i64),
    /// A string, bool or decimal before `Condition::bind_digests` (plan decision D-A).
    Unbound(Scalar),
    /// The digests of the literal, one for each known key id, current key first.
    Digests(Vec<(KeyId, Digest)>),
}

/// A comparison of the value at one path with a literal.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTest {
    pub path: FieldPath,
    pub op: CmpOp,
    pub literal: Literal,
}

impl FieldTest {
    pub fn new(path: FieldPath, op: CmpOp, literal: Literal) -> Self {
        Self { path, op, literal }
    }
}

/// A predicate on one event.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    EventKind(EventKind),
    Tool(ToolMatch),
    Field(FieldTest),
    AllOf(Box<Predicate>, Box<Predicate>),
    AnyOf(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
}

/// The aggregate of a condition over its window.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Formerly { predicate: Predicate },
    CountWithin { predicate: Predicate, op: CmpOp, threshold: i64 },
    SumWithin { predicate: Predicate, path: FieldPath, op: CmpOp, threshold: i64 },
}

impl Shape {
    /// The predicate of the shape.
    pub(super) fn predicate(&self) -> &Predicate {
        match self {
            Self::Formerly { predicate } | Self::CountWithin { predicate, .. } | Self::SumWithin { predicate, .. } => predicate,
        }
    }
}

/// One history condition. Its value is the Bool of the slot `slot`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    pub slot: String,
    pub part: Part,
    pub scope: Scope,
    pub window_ms: u64,
    pub shape: Shape,
}

impl Condition {
    pub fn new(slot: impl Into<String>, part: Part, scope: Scope, window_ms: u64, shape: Shape) -> Self {
        Self {
            slot: slot.into(),
            part,
            scope,
            window_ms,
            shape,
        }
    }
}

/// The tool of a window set.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SetTool {
    Tool(String),
    /// The set of every event of the scope.
    Every,
}

/// One stored history event.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: Uuid,
    pub ts_ms: u64,
    pub kind: EventKind,
    pub tool: String,
    pub fields: Fields,
    /// The id of the key that made the digests of `fields`.
    pub key: Option<KeyId>,
}

impl Event {
    pub fn new(id: Uuid, ts_ms: u64, kind: EventKind, tool: impl Into<String>, fields: Fields, key: Option<KeyId>) -> Self {
        Self {
            id,
            ts_ms,
            kind,
            tool: tool.into(),
            fields,
            key,
        }
    }
}

/// The events of one set from the reserve reply, and its truncation state (spec P4).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSet {
    pub scope: Scope,
    pub tool: SetTool,
    pub events: Vec<Event>,
    /// The time of the newest event that the store removed from the set, if any.
    pub mark_ms: Option<u64>,
    /// The reply holds only a part of the set.
    pub truncated: bool,
}

impl WindowSet {
    pub fn new(scope: Scope, tool: SetTool, events: Vec<Event>, mark_ms: Option<u64>, truncated: bool) -> Self {
        Self {
            scope,
            tool,
            events,
            mark_ms,
            truncated,
        }
    }
}

/// The sets of the reserve reply, or the reason that no set is available.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window(pub(super) Result<Vec<WindowSet>, MissingReason>);

impl Window {
    pub fn new(sets: Vec<WindowSet>) -> Self {
        Self(Ok(sets))
    }

    /// No set is available: every condition that passes the pre-checks is missing with `reason`.
    pub fn unavailable(reason: MissingReason) -> Self {
        Self(Err(reason))
    }
}

/// The tool catalog of the tenant set, when it is known.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog(pub(super) Option<HashSet<String>>);

impl Catalog {
    pub fn known(names: impl IntoIterator<Item = String>) -> Self {
        Self(Some(names.into_iter().collect()))
    }

    pub fn unknown() -> Self {
        Self(None)
    }

    /// `None` when the catalog is not known.
    pub fn contains(&self, name: &str) -> Option<bool> {
        self.0.as_ref().map(|names| names.contains(name))
    }
}

/// The Bool value of each slot that has a value. A missing slot is absent.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots(BTreeMap<String, bool>);

impl Slots {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, slot: impl Into<String>, value: bool) {
        self.0.insert(slot.into(), value);
    }

    pub fn get(&self, slot: &str) -> Option<bool> {
        self.0.get(slot).copied()
    }

    /// The slots in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, bool)> {
        self.0.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Why a slot has no value.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissingReason {
    Disabled,
    Store,
    NoSession,
    Truncated,
    Unknown,
    Retention,
    NoKey,
    Catalog,
}

impl MissingReason {
    /// Each reason once, in the order of the metric labels.
    pub const ALL: [MissingReason; 8] = [
        Self::Disabled,
        Self::Store,
        Self::NoSession,
        Self::Truncated,
        Self::Unknown,
        Self::Retention,
        Self::NoKey,
        Self::Catalog,
    ];

    /// The metric label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Store => "store",
            Self::NoSession => "no_session",
            Self::Truncated => "truncated",
            Self::Unknown => "unknown",
            Self::Retention => "retention",
            Self::NoKey => "no_key",
            Self::Catalog => "catalog",
        }
    }
}

/// A slot with no value, and the reason.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub slot: String,
    pub part: Part,
    pub reason: MissingReason,
}

impl Missing {
    pub fn new(slot: impl Into<String>, part: Part, reason: MissingReason) -> Self {
        Self { slot: slot.into(), part, reason }
    }
}

/// The result of `evaluate`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub slots: Slots,
    pub missing: Vec<Missing>,
}

impl Evaluation {
    pub fn new(slots: Slots, missing: Vec<Missing>) -> Self {
        Self { slots, missing }
    }
}

/// The tools that a predicate can match: the named tools, the catalog tools when `catalog`, and
/// every tool when `every`.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSet {
    pub names: BTreeSet<String>,
    /// The predicate holds `AnyCatalogTool`.
    pub catalog: bool,
    pub every: bool,
}

impl ToolSet {
    pub fn new(names: BTreeSet<String>, catalog: bool, every: bool) -> Self {
        Self { names, catalog, every }
    }

    /// The condition reads the `Every` set (and all sets of its scope): when `every`, or when
    /// `catalog` and the catalog is not known. `ReadPlan::of` uses the same rule.
    pub fn requires_every(&self, catalog: &Catalog) -> bool {
        self.every || (self.catalog && catalog.0.is_none())
    }
}
