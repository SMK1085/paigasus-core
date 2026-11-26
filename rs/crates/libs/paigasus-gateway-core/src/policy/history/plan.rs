// SPDX-License-Identifier: Apache-2.0
//! The field plan, the read plan and the bound history policy of one set (SMA-753 spec P3, P4,
//! P6). They are made once at load. No I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use paigasus_kernel::tool_policy::history::{Catalog, Condition, FieldPath, HistoryDigester, Part, PathSet, Scope, SetTool, Shape, ToolSet};
use uuid::Uuid;

/// The tools of a tool set as the store keys them: `None` for every tool (the `Every` set), else
/// the named tools and, when `catalog`, each catalog tool. A `catalog` set that would list more
/// than `MAX_READ_SETS` tools is `None` too. This is the rule of the evaluator
/// (`ToolSet::requires_every`), so the plans and the evaluator read the same sets.
fn set_tools(tools: &ToolSet, catalog: &Catalog) -> Option<BTreeSet<String>> {
    if tools.requires_every(catalog) {
        return None;
    }
    let mut names = tools.names.clone();
    if tools.catalog {
        names.extend(catalog.names().into_iter().flatten().cloned());
    }
    Some(names)
}

/// The argument paths that the history stores for each tool (spec P3): the paths of one tool, and
/// the paths of every tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldPlan {
    tools: BTreeMap<String, PathSet>,
    every: PathSet,
}

impl FieldPlan {
    /// The paths of the field tests of each condition, and the `sum_within` path, under the tool
    /// set that holds at each one. A `catalog` tool set gives each catalog tool, or every tool when
    /// the catalog is not known or has more than `MAX_READ_SETS` tools.
    #[must_use]
    pub fn of(conditions: &[Condition], catalog: &Catalog) -> Self {
        let mut plan = Self::default();
        for condition in conditions {
            let predicate = condition.shape.predicate();
            for (tools, test) in predicate.field_tests() {
                plan.add(&tools, &test.path, catalog);
            }
            if let Shape::SumWithin { path, .. } = &condition.shape {
                plan.add(&predicate.tools(), path, catalog);
            }
        }
        plan
    }

    fn add(&mut self, tools: &ToolSet, path: &FieldPath, catalog: &Catalog) {
        match set_tools(tools, catalog) {
            None => {
                self.every.insert(path.clone());
            }
            Some(names) => {
                for name in names {
                    self.tools.entry(name).or_default().insert(path.clone());
                }
            }
        }
    }

    /// The paths of both plans.
    #[must_use]
    pub fn union(&self, other: &FieldPlan) -> FieldPlan {
        let mut out = self.clone();
        for (tool, paths) in &other.tools {
            out.tools.entry(tool.clone()).or_default().extend(paths.iter().cloned());
        }
        out.every.extend(other.every.iter().cloned());
        out
    }

    /// The paths to store for a call of `tool` (a name key): its own paths and the every-tool paths.
    #[must_use]
    pub fn paths_for(&self, tool: &str) -> PathSet {
        let mut paths = self.every.clone();
        if let Some(own) = self.tools.get(tool) {
            paths.extend(own.iter().cloned());
        }
        paths
    }

    /// No path for any tool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.every.is_empty() && self.tools.values().all(BTreeSet::is_empty)
    }
}

/// One history set that the reserve reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSet {
    pub scope: Scope,
    pub tool: SetTool,
}

/// The sets that the reserve reads for a policy set, and the longest window (spec P4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPlan {
    pub w_max_ms: u64,
    /// Sorted by scope, then tool, with no duplicates.
    pub sets: Vec<ReadSet>,
}

impl ReadPlan {
    /// One set for each scope and tool that a condition reads: a named tool gives its set, a
    /// `catalog` tool set gives the set of each catalog tool, and every tool (or a `catalog` tool
    /// set with no known catalog, or with more than `MAX_READ_SETS` tools) gives the `Every` set. A condition with only named tools reads no
    /// `Every` set. `None` with no condition.
    ///
    /// The `AllOf` catalog over-approximation of `Predicate::tools` can list more sets than the
    /// predicate can match. Each one is in `sets`, so it counts against the read-set limit.
    #[must_use]
    pub fn of(conditions: &[Condition], catalog: &Catalog) -> Option<ReadPlan> {
        let w_max_ms = conditions.iter().map(|c| c.window_ms).max()?;
        let mut sets: BTreeSet<(Scope, SetTool)> = BTreeSet::new();
        for condition in conditions {
            match set_tools(&condition.tools(), catalog) {
                None => {
                    sets.insert((condition.scope, SetTool::Every));
                }
                Some(names) => sets.extend(names.into_iter().map(|n| (condition.scope, SetTool::Tool(n)))),
            }
        }
        let sets = sets.into_iter().map(|(scope, tool)| ReadSet { scope, tool }).collect();
        Some(ReadPlan { w_max_ms, sets })
    }
}

/// The history conditions of one org, bound to its digests, with their plans (spec P6). The cache
/// makes it once at load; each request snapshot refers to it with an `Arc`.
#[derive(Debug)]
pub struct HistoryPolicy {
    /// The floor conditions first, then the tenant conditions.
    conditions: Vec<Condition>,
    fields: Arc<FieldPlan>,
    read: Option<ReadPlan>,
    catalog: Catalog,
}

impl HistoryPolicy {
    /// Bind every condition with `bind_digests(org, d)`. The field plan is the plan of all
    /// conditions, united with `stored` (the last known plan of the org, design D7) when given.
    #[must_use]
    pub fn build(floor: &[Condition], tenant: &[Condition], org: Uuid, catalog: Catalog, stored: Option<&FieldPlan>, d: &dyn HistoryDigester) -> Self {
        let conditions: Vec<Condition> = floor.iter().chain(tenant).map(|c| c.bind_digests(org, d)).collect();
        let mut fields = FieldPlan::of(&conditions, &catalog);
        if let Some(stored) = stored {
            fields = fields.union(stored);
        }
        let read = ReadPlan::of(&conditions, &catalog);
        Self {
            conditions,
            fields: Arc::new(fields),
            read,
            catalog,
        }
    }

    /// The bound conditions, floor first.
    #[must_use]
    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }

    /// The part has at least one condition.
    #[must_use]
    pub fn has_part(&self, part: Part) -> bool {
        self.conditions.iter().any(|c| c.part == part)
    }

    /// The field plan. A caller that keeps it clones the `Arc`.
    #[must_use]
    pub fn fields(&self) -> &Arc<FieldPlan> {
        &self.fields
    }

    /// The read plan, `None` with no condition.
    #[must_use]
    pub fn read(&self) -> Option<&ReadPlan> {
        self.read.as_ref()
    }

    /// The catalog that the plans were made with.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use paigasus_kernel::tool_policy::history::{
        Catalog, CmpOp, Condition, Digest, FieldPath, FieldTest, HistoryDigester, KeyId, Literal, Part, PathSet, Predicate, Scalar, Scope, SetTool, Shape, ToolMatch,
    };
    use uuid::Uuid;

    use super::*;

    fn p(name: &str) -> FieldPath {
        FieldPath::new(name.split('.').map(str::to_owned).collect()).expect("path")
    }

    fn paths(names: &[&str]) -> PathSet {
        names.iter().map(|n| p(n)).collect()
    }

    fn tool(name: &str) -> Predicate {
        Predicate::Tool(ToolMatch::NameKey(name.to_owned()))
    }

    fn any_catalog() -> Predicate {
        Predicate::Tool(ToolMatch::AnyCatalogTool)
    }

    fn field(path: &str) -> Predicate {
        Predicate::Field(FieldTest::new(p(path), CmpOp::Gt, Literal::Long(1)))
    }

    fn unbound(path: &str) -> Predicate {
        Predicate::Field(FieldTest::new(p(path), CmpOp::Eq, Literal::Unbound(Scalar::Str("bob".to_owned()))))
    }

    fn all(a: Predicate, b: Predicate) -> Predicate {
        Predicate::AllOf(Box::new(a), Box::new(b))
    }

    fn not(a: Predicate) -> Predicate {
        Predicate::Not(Box::new(a))
    }

    fn formerly(slot: &str, part: Part, scope: Scope, window_ms: u64, predicate: Predicate) -> Condition {
        Condition::new(slot, part, scope, window_ms, Shape::Formerly { predicate })
    }

    fn session(slot: &str, predicate: Predicate) -> Condition {
        formerly(slot, Part::Tenant, Scope::Session, 1000, predicate)
    }

    fn known(names: &[&str]) -> Catalog {
        Catalog::known(names.iter().map(|n| (*n).to_owned()))
    }

    fn read(scope: Scope, tool: SetTool) -> ReadSet {
        ReadSet { scope, tool }
    }

    fn named(name: &str) -> SetTool {
        SetTool::Tool(name.to_owned())
    }

    /// Digest = `[n; 16]` where `n` is the length of the canonical bytes; one key id.
    struct TestDigester;

    impl HistoryDigester for TestDigester {
        fn current(&self, _org: Uuid, _path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)> {
            self.all(_org, _path, value).into_iter().next()
        }
        fn all(&self, _org: Uuid, _path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)> {
            let n = u8::try_from(value.canonical_bytes().len()).expect("short");
            vec![(KeyId::new([1; 8]), Digest::new([n; 16]))]
        }
    }

    #[test]
    fn the_field_plan_holds_paths_per_tool_and_for_every_tool() {
        let conditions = [
            session("a", all(tool("send"), field("amount"))),
            session("b", not(field("x"))),
            Condition::new(
                "c",
                Part::Tenant,
                Scope::Session,
                1000,
                Shape::SumWithin {
                    predicate: tool("send"),
                    path: p("total"),
                    op: CmpOp::Le,
                    threshold: 5,
                },
            ),
        ];
        let plan = FieldPlan::of(&conditions, &Catalog::unknown());
        assert_eq!(plan.paths_for("send"), paths(&["amount", "total", "x"]));
        assert_eq!(plan.paths_for("search"), paths(&["x"]), "only the every-tool path");
        assert!(!plan.is_empty());
    }

    #[test]
    fn any_catalog_tool_paths_go_to_each_catalog_tool_or_every_tool() {
        let conditions = [session("a", all(any_catalog(), field("amount")))];
        let plan = FieldPlan::of(&conditions, &known(&["send", "search"]));
        assert_eq!(plan.paths_for("send"), paths(&["amount"]));
        assert_eq!(plan.paths_for("search"), paths(&["amount"]));
        assert_eq!(plan.paths_for("other"), PathSet::new(), "a tool outside the catalog");
        let plan = FieldPlan::of(&conditions, &Catalog::unknown());
        assert_eq!(plan.paths_for("other"), paths(&["amount"]), "an unknown catalog gives every tool");
    }

    #[test]
    fn the_field_plan_of_a_named_tool_condition_adds_no_every_tool_path() {
        let conditions = [session("a", all(tool("send"), all(field("amount"), unbound("to"))))];
        let plan = FieldPlan::of(&conditions, &Catalog::unknown());
        assert_eq!(plan.paths_for("send"), paths(&["amount", "to"]));
        assert_eq!(plan.paths_for("other"), PathSet::new());
        // The plan has the same tool sets as the evaluator reads: `Predicate::field_tests`.
        let tests = conditions[0].shape.predicate().field_tests();
        assert!(
            tests
                .iter()
                .all(|(set, _)| !set.requires_every(&Catalog::unknown()) && set.names == BTreeSet::from(["send".to_owned()]))
        );
    }

    #[test]
    fn field_plan_union_merges_per_tool_and_every_tool_paths() {
        let a = FieldPlan::of(&[session("a", all(tool("send"), field("amount")))], &Catalog::unknown());
        let b = FieldPlan::of(&[session("b", all(tool("send"), field("to"))), session("c", field("x"))], &Catalog::unknown());
        let u = a.union(&b);
        assert_eq!(u.paths_for("send"), paths(&["amount", "to", "x"]));
        assert_eq!(u.paths_for("other"), paths(&["x"]));
        assert_eq!(u, b.union(&a), "the union does not depend on the order");
        assert_eq!(a.union(&FieldPlan::default()), a);
    }

    #[test]
    fn field_plan_is_empty_only_with_no_path() {
        assert!(FieldPlan::default().is_empty());
        assert!(FieldPlan::of(&[session("a", tool("send"))], &Catalog::unknown()).is_empty(), "a condition with no field test");
        assert!(!FieldPlan::of(&[session("a", all(tool("send"), field("amount")))], &Catalog::unknown()).is_empty());
        assert!(!FieldPlan::of(&[session("a", field("amount"))], &Catalog::unknown()).is_empty());
    }

    #[test]
    fn the_read_plan_lists_each_scope_and_tool_once_and_the_largest_window() {
        let conditions = [
            session("a", tool("send")),
            formerly("b", Part::Tenant, Scope::Principal, 5000, any_catalog()),
            formerly("c", Part::Floor, Scope::Session, 2000, not(tool("x"))),
            formerly("d", Part::Tenant, Scope::Session, 10, tool("send")),
            formerly("e", Part::Tenant, Scope::Key, 10, all(any_catalog(), tool("z"))),
        ];
        let plan = ReadPlan::of(&conditions, &known(&["a", "b"])).expect("conditions");
        assert_eq!(plan.w_max_ms, 5000);
        assert_eq!(
            plan.sets,
            [
                read(Scope::Session, named("send")),
                read(Scope::Session, SetTool::Every),
                read(Scope::Principal, named("a")),
                read(Scope::Principal, named("b")),
                // The `AllOf` catalog over-approximation: the catalog tools and the name. Each one
                // counts against the read-set limit of the store.
                read(Scope::Key, named("a")),
                read(Scope::Key, named("b")),
                read(Scope::Key, named("z")),
            ]
        );
        // A named-tool condition adds no `Every` set; an unknown catalog gives `Every`.
        assert_eq!(ReadPlan::of(&conditions[..1], &Catalog::unknown()).expect("one").sets, [read(Scope::Session, named("send"))]);
        assert_eq!(ReadPlan::of(&conditions[1..2], &Catalog::unknown()).expect("one").sets, [read(Scope::Principal, SetTool::Every)]);
    }

    #[test]
    fn a_catalog_of_more_than_the_read_set_limit_reads_the_every_set_once_per_scope() {
        use paigasus_kernel::tool_policy::history::MAX_READ_SETS;
        let names: Vec<String> = (0..=MAX_READ_SETS).map(|i| format!("t{i}")).collect();
        let catalog = Catalog::known(names.clone());
        let conditions = [
            session("a", all(any_catalog(), field("amount"))),
            formerly("b", Part::Tenant, Scope::Principal, 5000, any_catalog()),
            session("c", tool("send")),
        ];
        let plan = ReadPlan::of(&conditions, &catalog).expect("conditions");
        // No per-tool set of the 65 catalog tools: the read stays far under the limit.
        assert_eq!(
            plan.sets,
            [read(Scope::Session, named("send")), read(Scope::Session, SetTool::Every), read(Scope::Principal, SetTool::Every)]
        );
        assert!(plan.sets.len() <= MAX_READ_SETS);
        // The field plan stores the path for every tool, so each catalog tool keeps it.
        let fields = FieldPlan::of(&conditions, &catalog);
        assert_eq!(fields.paths_for("t64"), paths(&["amount"]));
        assert_eq!(fields.paths_for("t0"), paths(&["amount"]));
        // At the limit, each catalog tool still has its own set.
        let at_limit = Catalog::known(names.into_iter().take(MAX_READ_SETS));
        let plan = ReadPlan::of(&conditions[..1], &at_limit).expect("one");
        assert_eq!(plan.sets.len(), MAX_READ_SETS);
        assert!(!plan.sets.contains(&read(Scope::Session, SetTool::Every)));
    }

    #[test]
    fn no_condition_gives_no_read_plan() {
        assert_eq!(ReadPlan::of(&[], &Catalog::unknown()), None);
    }

    #[test]
    fn history_policy_binds_the_literals_of_both_parts() {
        let floor = [formerly("f", Part::Floor, Scope::Session, 1000, all(tool("send"), unbound("to")))];
        let tenant = [formerly("t", Part::Tenant, Scope::Principal, 3000, unbound("cc"))];
        let stored = FieldPlan::of(&[session("s", all(tool("q"), field("old")))], &Catalog::unknown());
        let policy = HistoryPolicy::build(&floor, &tenant, Uuid::from_u128(7), known(&["send"]), Some(&stored), &TestDigester);
        let slots: Vec<(&str, Part)> = policy.conditions().iter().map(|c| (c.slot.as_str(), c.part)).collect();
        assert_eq!(slots, [("f", Part::Floor), ("t", Part::Tenant)], "the floor comes first");
        let digests = Literal::Digests(vec![(KeyId::new([1; 8]), Digest::new([4; 16]))]);
        for c in policy.conditions() {
            let literals: Vec<&Literal> = c.shape.predicate().field_tests().into_iter().map(|(_, t)| &t.literal).collect();
            assert_eq!(literals, [&digests], "{}", c.slot);
        }
        assert_eq!(policy.fields().paths_for("send"), paths(&["cc", "to"]));
        assert_eq!(policy.fields().paths_for("q"), paths(&["cc", "old"]), "the stored plan is united");
        let read = policy.read().expect("conditions");
        assert_eq!((read.w_max_ms, read.sets.len()), (3000, 2));
        assert_eq!(policy.catalog(), &known(&["send"]));
        // With no stored plan, the plan is the plan of the conditions alone.
        let fresh = HistoryPolicy::build(&floor, &tenant, Uuid::from_u128(7), known(&["send"]), None, &TestDigester);
        assert_eq!(fresh.fields().paths_for("q"), paths(&["cc"]));
    }

    #[test]
    fn history_policy_has_part_is_true_only_for_a_part_with_conditions() {
        let floor = [formerly("f", Part::Floor, Scope::Session, 1000, tool("send"))];
        let only_floor = HistoryPolicy::build(&floor, &[], Uuid::nil(), Catalog::unknown(), None, &TestDigester);
        assert!(only_floor.has_part(Part::Floor));
        assert!(!only_floor.has_part(Part::Tenant));
        let none = HistoryPolicy::build(&[], &[], Uuid::nil(), Catalog::unknown(), None, &TestDigester);
        assert!(!none.has_part(Part::Floor) && !none.has_part(Part::Tenant));
        assert!(none.read().is_none() && none.fields().is_empty());
    }
}
