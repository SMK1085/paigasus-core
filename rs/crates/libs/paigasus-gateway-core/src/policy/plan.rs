// SPDX-License-Identifier: Apache-2.0
//! The request-side plan of one request (§ 6.5.1 steps 1-4, P-7, P-9, P-15): which definitions to
//! remove, which tool results to replace, and how `tool_choice` changes. Each item carries its own
//! `enforced` flag; `rewrite_request` applies only the enforced items. No I/O.

use std::collections::{BTreeMap, BTreeSet};

use paigasus_kernel::tool_policy::{Facts, Findings, NAME_COLLISION, ToolPolicies, ToolVerdict, decide_tool, normalise};

use super::mapping::{definition_call_name, name_collisions};
use super::snapshot::{PolicySet, PolicySnapshot, TenantMode, TenantPart};
use crate::report::{RequestAction, RequestEntry, RequestReason};
use crate::turn::{ToolChoice, ToolDefinition, TurnFacts};
use crate::types::{Format, ToolCallKind};

/// The Responses built-in client-run tools that the plan may remove (P-15). Every other
/// definition of kind `Other` is a hosted tool that the provider runs, and is never removed.
pub(crate) const REMOVABLE_BUILT_INS: [&str; 5] = ["local_shell", "shell", "apply_patch", "computer", "computer_use_preview"];

/// The tool result to replace (P-7).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultTarget {
    /// Every result that refers to this call id.
    CallId(String),
    /// A legacy chat `role:"function"` result with this name (it has no call id).
    Name(String),
    /// Every Responses `local_shell_call_output` whose `id` is this. The id matched the `call_id`
    /// or the item `id` of an earlier `local_shell_call`.
    LocalShellId(String),
}

/// The `tool_choice` after the enforced removals (§ 6.5.1 step 4).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceRewrite {
    /// Leave `tool_choice` as it is.
    Keep,
    /// Set `tool_choice` to `auto`.
    Auto,
    /// Keep only these allowed tools (display names, as the caller spelled them).
    Allowed(Vec<String>),
}

/// One planned change. Only an enforced item changes the body (P-9).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem<T> {
    pub target: T,
    pub enforced: bool,
}

/// The request-side plan.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPlan {
    /// The normalised display names of the definitions to remove. Every definition with that
    /// name is removed (P-7).
    pub removed: Vec<PlanItem<String>>,
    pub replaced: Vec<PlanItem<ResultTarget>>,
    /// Computed from the enforced removals only.
    pub choice: ChoiceRewrite,
    /// The choice is `Unknown` and an enforced removal exists: refuse the request (D17).
    pub refuse: bool,
    /// The report entries, in the order: removals, replacements, the `tool_choice` reset.
    pub entries: Vec<RequestEntry>,
}

impl RequestPlan {
    fn empty() -> Self {
        RequestPlan {
            removed: Vec::new(),
            replaced: Vec::new(),
            choice: ChoiceRewrite::Keep,
            refuse: false,
            entries: Vec::new(),
        }
    }

    /// Whether the plan changes nothing that is enforced: the body goes out as it came in.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        !self.refuse && self.choice == ChoiceRewrite::Keep && !self.removed.iter().any(|i| i.enforced) && !self.replaced.iter().any(|i| i.enforced)
    }
}

/// Whether the plan may remove a definition of this kind and name (P-15).
fn removable(d: &ToolDefinition) -> bool {
    d.kind != ToolCallKind::Other || REMOVABLE_BUILT_INS.contains(&d.name.as_str())
}

/// The kernel answers for one snapshot, one per normalised tool name.
struct Judge<'a> {
    policies: &'a ToolPolicies,
    facts: &'a Facts,
    collisions: BTreeSet<String>,
    /// Only a floor deny counts: the tenant mode is `off`, or the set is the floor alone because
    /// the tenant part is `Off` or `Unavailable` (P-4).
    floor_only: bool,
    mode: TenantMode,
    seen: BTreeMap<String, Option<bool>>,
}

impl Judge<'_> {
    /// `Some(enforced)` when the tool is denied for every input and the deny counts, else `None`.
    fn denied(&mut self, name: &str) -> Option<bool> {
        let n = normalise(name);
        if let Some(v) = self.seen.get(&n) {
            return *v;
        }
        let mut findings = Findings::new();
        if self.collisions.contains(&n) {
            findings.insert(NAME_COLLISION);
        }
        let v = match decide_tool(&n, self.facts, &findings, self.policies) {
            ToolVerdict::DeniedForAllInputs { by_floor } if by_floor || !self.floor_only => Some(by_floor || self.mode == TenantMode::Enforce),
            _ => None,
        };
        self.seen.insert(n, v);
        v
    }
}

/// Step 3: decides each result target once (P-7).
struct ResultJudge<'j, 'p, 'k> {
    judge: &'j mut Judge<'p>,
    plan: &'j mut RequestPlan,
    /// Every target decided so far, and whether it is replaced. A repeated target has the same
    /// outcome, so it is not decided again.
    seen: BTreeMap<(u8, &'k str), bool>,
}

impl<'k> ResultJudge<'_, '_, 'k> {
    /// Whether the target is replaced: any of `names` is denied. A new replaced target adds one
    /// plan item and one report entry.
    fn decide(&mut self, key: (u8, &'k str), target: impl FnOnce() -> ResultTarget, names: &[&str]) -> bool {
        if let Some(replaced) = self.seen.get(&key) {
            return *replaced;
        }
        let mut hit: Option<(&str, bool)> = None;
        for &name in names {
            if let Some(enforced) = self.judge.denied(name) {
                hit = Some(match hit {
                    Some((first, e)) => (first, e || enforced),
                    None => (name, enforced),
                });
            }
        }
        self.seen.insert(key, hit.is_some());
        let Some((name, enforced)) = hit else { return false };
        self.plan
            .entries
            .push(RequestEntry::new(name.to_owned(), RequestAction::ReplaceResult, enforced, RequestReason::ToolResultBlocked));
        self.plan.replaced.push(PlanItem { target: target(), enforced });
        true
    }
}

/// The `tool_choice` after removing `removed` (normalised names): the rewrite, whether to refuse,
/// and the name of the tool that caused a reset.
fn choice_after(choice: &ToolChoice, removed: &BTreeSet<&str>) -> (ChoiceRewrite, bool, Option<String>) {
    match choice {
        ToolChoice::Named(n) if removed.contains(normalise(n).as_str()) => (ChoiceRewrite::Auto, false, Some(n.clone())),
        ToolChoice::AllowedTools(names) => {
            let (gone, left): (Vec<&String>, Vec<&String>) = names.iter().partition(|n| removed.contains(normalise(n).as_str()));
            match (gone.first(), left.is_empty()) {
                (None, _) => (ChoiceRewrite::Keep, false, None),
                (Some(g), true) => (ChoiceRewrite::Auto, false, Some((*g).clone())),
                (Some(g), false) => (ChoiceRewrite::Allowed(left.into_iter().cloned().collect()), false, Some((*g).clone())),
            }
        }
        // With only log-only removals, `removed` is empty here: no refuse and no entry (§ 6.5.3 has
        // no refuse action).
        ToolChoice::Unknown => (ChoiceRewrite::Keep, !removed.is_empty(), None),
        _ => (ChoiceRewrite::Keep, false, None),
    }
}

/// Plan the request-side actions (§ 6.5.1 steps 1-4, and the Anthropic part of step 5). With the
/// policies unavailable, the plan is empty (§ 6.4). With the tenant part unavailable, only floor
/// denies are planned, in every tenant mode, as in mode `off` (controller ruling I3: D33 says the
/// floor is always enforced, which outranks the § 6.4 sentence "the request side changes
/// nothing"). An item is enforced when a floor policy decided its deny (D33) or the tenant mode is
/// `enforce`; in log-only mode a tenant deny is only reported (D25).
///
/// Anthropic (step 5, controller ruling): when the messages hold `tool_use` or `tool_result`
/// blocks and the enforced removals would leave `tools[]` empty (server tools count), the last
/// definition in array order stays. Its removal is reported with `enforced: false`, and it does not
/// count in the `tool_choice` rewrite. Its calls are still blocked at call time. The other parts of
/// step 5 (dropping `tools`, `tool_choice` and their companions) belong to the rewrite.
#[must_use]
pub fn plan_request(snapshot: &PolicySnapshot, format: Format, turn: &TurnFacts) -> RequestPlan {
    let PolicySet::Available { policies, tenant, facts } = &snapshot.set else {
        return RequestPlan::empty();
    };
    // Only the floor counts when the mode is `off` (controller ruling, Task 8), when the set is the
    // floor alone, and when the tenant part is unavailable (controller ruling, I3: D33, the floor
    // is always enforced). In each case `policies` is the floor alone, or the tenant is ignored.
    let floor_only = snapshot.mode == TenantMode::Off || *tenant != TenantPart::Evaluated;
    let mut judge = Judge {
        policies,
        facts,
        // Step 1: the request-level findings.
        collisions: name_collisions(&turn.definitions),
        floor_only,
        mode: snapshot.mode,
        seen: BTreeMap::new(),
    };
    let mut plan = RequestPlan::empty();

    // Step 2: definitions, one item per normalised display name (P-7). A definition is decided by
    // the name its calls arrive under (`computer_use_preview` gives `computer` calls).
    let mut done: BTreeSet<String> = BTreeSet::new();
    for d in turn.definitions.iter().filter(|d| removable(d)) {
        let target = normalise(&d.display_name());
        if !done.insert(target.clone()) {
            continue;
        }
        if let Some(enforced) = judge.denied(&definition_call_name(d)) {
            plan.entries
                .push(RequestEntry::new(d.display_name(), RequestAction::RemoveDefinition, enforced, RequestReason::ToolCallDenied));
            plan.removed.push(PlanItem { target, enforced });
        }
    }

    // Step 5, Anthropic: keep the last definition when blocks refer to tools and nothing else would
    // stay in `tools[]`. Removal entries come first and in item order, so index `i` matches.
    let mut kept: Option<String> = None;
    let blocks = !turn.prior_calls.is_empty() || !turn.tool_results.is_empty();
    if format == Format::AnthropicMessages && blocks && !turn.server_tools {
        let gone: BTreeSet<&str> = plan.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
        let empties = turn.definitions.iter().all(|d| gone.contains(normalise(&d.display_name()).as_str()));
        if let Some(last) = turn.definitions.last().filter(|_| empties) {
            let t = normalise(&last.display_name());
            if let Some(i) = plan.removed.iter().position(|item| item.target == t) {
                plan.removed[i].enforced = false; // ANTHROPIC-KEEP-LAST
                plan.entries[i].enforced = false;
            }
            kept = Some(t);
        }
    }

    // Step 3: results. A result with a call id takes the names of the earlier calls with that id;
    // it is replaced if any of them is denied. A legacy result carries its own name.
    // One index of the prior calls by id, built once. A duplicate id keeps every name in order.
    let mut by_id: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    // A `local_shell_call_output` can name its call by `id` alone. The provider schema does not say
    // if that is the `call_id` or the item `id` of the call, so it is matched against both ids of
    // every earlier `local_shell_call` (fail-safe).
    let mut shell_ids: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for c in &turn.prior_calls {
        if let Some(id) = c.id.as_deref() {
            by_id.entry(id).or_default().push(c.name.as_str());
        }
        if c.local_shell {
            let ids: BTreeSet<&str> = c.id.iter().chain(&c.item_id).map(String::as_str).collect();
            for id in ids {
                shell_ids.entry(id).or_default().push(c.name.as_str());
            }
        }
    }
    let mut results = ResultJudge {
        judge: &mut judge,
        plan: &mut plan,
        seen: BTreeMap::new(),
    };
    for r in &turn.tool_results {
        let replaced = match (&r.call_id, &r.name) {
            (Some(id), _) => results.decide((0, id.as_str()), || ResultTarget::CallId(id.clone()), by_id.get(id.as_str()).map_or(&[][..], Vec::as_slice)),
            (None, Some(name)) => results.decide((1, name.as_str()), || ResultTarget::Name(name.clone()), &[name.as_str()]),
            (None, None) => false,
        };
        // The `id` form of a `local_shell_call_output`, when its `call_id` did not replace it.
        if !replaced && let Some(id) = &r.item_id {
            results.decide((2, id.as_str()), || ResultTarget::LocalShellId(id.clone()), shell_ids.get(id.as_str()).map_or(&[][..], Vec::as_slice));
        }
    }

    // Step 4: `tool_choice`, from the enforced removals. A reset that only all removals together
    // would make is reported once, not enforced (P-9).
    let enforced: BTreeSet<&str> = plan.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
    // The kept Anthropic definition stays in the request, so it does not count here either.
    let all: BTreeSet<&str> = plan.removed.iter().map(|i| i.target.as_str()).filter(|t| kept.as_deref() != Some(*t)).collect();
    let (choice, refuse, reset) = choice_after(&turn.tool_choice, &enforced);
    let reset_entry = match reset {
        Some(tool) => Some((tool, true)),
        None => choice_after(&turn.tool_choice, &all).2.map(|tool| (tool, false)),
    };
    if let Some((tool, enforced)) = reset_entry {
        plan.entries.push(RequestEntry::new(tool, RequestAction::ResetToolChoice, enforced, RequestReason::ToolCallDenied));
    }
    plan.choice = choice;
    plan.refuse = refuse;
    plan
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::tool_policy::{RequestInfo, SessionInfo};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use uuid::Uuid;

    use super::*;
    use crate::policy::SHIPPED_FLOOR;
    use crate::policy::snapshot::OnUnavailable;
    use crate::turn::{PriorCall, ToolResultRef, ToolTurn, parse_request};

    const NO_FLOOR: &str = "";
    const RM_FLOOR: &str = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
    const PERMIT_ALL: &str = r#"@id("all") permit(principal, action, resource);"#;
    const PERMIT_OK: &str = r#"@id("ok") permit(principal, action == Pgs::Gateway::Action::"ok", resource);"#;

    fn facts() -> Facts {
        let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
        Facts::new(
            to_cedar_uid(&prn),
            Uuid::nil(),
            RequestInfo::new("prn:pgs:gateway:::scope/x", crate::policy::ingress(Format::OpenAiChat), "m"),
            SessionInfo::placeholder(),
        )
    }

    /// A snapshot whose set is the floor and, when `tenant` is `Evaluated`, the tenant text (P-4).
    fn snap(floor: &str, tenant_text: &str, tenant: TenantPart, mode: TenantMode) -> PolicySnapshot {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let text = if tenant == TenantPart::Evaluated { tenant_text } else { "" };
        let set = PolicySet::Available {
            policies: Arc::new(ToolPolicies::compile(floor, text, &[]).expect("compile")),
            tenant,
            facts: facts(),
        };
        PolicySnapshot::new(set, mode, OnUnavailable::Closed)
    }

    fn enforce(tenant_text: &str) -> PolicySnapshot {
        snap(NO_FLOOR, tenant_text, TenantPart::Evaluated, TenantMode::Enforce)
    }

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Function,
            schema: None,
        }
    }

    fn other(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Other,
            schema: None,
        }
    }

    fn prior(id: &str, name: &str) -> PriorCall {
        PriorCall {
            id: Some(id.to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            local_shell: false,
            item_id: None,
        }
    }

    fn by_id(id: &str) -> ToolResultRef {
        ToolResultRef {
            call_id: Some(id.to_owned()),
            name: None,
            item_id: None,
        }
    }

    fn turn(definitions: Vec<ToolDefinition>) -> TurnFacts {
        TurnFacts { definitions, ..TurnFacts::default() }
    }

    fn item<T>(target: T, enforced: bool) -> PlanItem<T> {
        PlanItem { target, enforced }
    }

    fn removed(p: &RequestPlan) -> Vec<(&str, bool)> {
        p.removed.iter().map(|i| (i.target.as_str(), i.enforced)).collect()
    }

    fn actions(p: &RequestPlan) -> Vec<(RequestAction, bool, RequestReason)> {
        p.entries.iter().map(|e| (e.action, e.enforced, e.reason)).collect()
    }

    #[test]
    fn an_unavailable_set_plans_nothing() {
        let mut t = turn(vec![def("rm"), def("x")]);
        t.prior_calls = vec![prior("c1", "rm")];
        t.tool_results = vec![by_id("c1")];
        t.tool_choice = ToolChoice::Unknown;
        let unavailable = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed);
        let p = plan_request(&unavailable, Format::OpenAiChat, &t);
        assert_eq!(p, RequestPlan::empty());
        assert!(p.is_noop());
    }

    /// Controller ruling (I3, D33): an unavailable tenant part plans the floor alone in every
    /// mode. A floor-denied definition is removed and a floor-denied result is replaced, both
    /// enforced; the floor-alone default deny of "x" does not count (P-4).
    #[test]
    fn an_unavailable_tenant_part_plans_the_floor_in_every_mode() {
        let mut t = turn(vec![def("rm"), def("x")]);
        t.prior_calls = vec![prior("c1", "rm"), prior("c2", "x")];
        t.tool_results = vec![by_id("c1"), by_id("c2")];
        for mode in [TenantMode::Enforce, TenantMode::LogOnly, TenantMode::Off] {
            let p = plan_request(&snap(RM_FLOOR, "", TenantPart::Unavailable, mode), Format::OpenAiChat, &t);
            assert_eq!(removed(&p), [("rm", true)], "{mode:?}");
            assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)], "{mode:?}");
            assert_eq!(
                actions(&p),
                [
                    (RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied),
                    (RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked),
                ],
                "{mode:?}"
            );
            assert!(!p.is_noop());
        }
        // With no floor deny, an unavailable tenant part plans nothing.
        let p = plan_request(&snap(NO_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce), Format::OpenAiChat, &t);
        assert!(p.is_noop() && p.entries.is_empty());
    }

    #[test]
    fn a_tool_denied_for_every_input_is_removed() {
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiChat, &turn(vec![def("X "), def("ok")]));
        assert_eq!(p.removed, [item("x".to_owned(), true)]);
        assert_eq!(actions(&p), [(RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied)]);
        assert_eq!(p.entries[0].tool, "X ");
        assert!(!p.is_noop());
    }

    #[test]
    fn every_definition_of_a_removed_name_goes_together() {
        // P-7: one item per normalised display name.
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiChat, &turn(vec![def("x"), def("ok"), def("X")]));
        assert_eq!(removed(&p), [("x", true)]);
        assert_eq!(p.entries.len(), 1);
    }

    #[test]
    fn an_input_reading_permit_keeps_the_definition() {
        let permit = r#"@id("p") permit(principal, action == Pgs::Gateway::Action::"read_file", resource) when { context.input.path like "/tmp/*" };"#;
        let p = plan_request(&enforce(permit), Format::OpenAiChat, &turn(vec![def("read_file")]));
        assert!(p.removed.is_empty());
        assert!(p.entries.is_empty());
        assert!(p.is_noop());
    }

    #[test]
    fn log_only_plans_but_does_not_enforce() {
        let mut t = turn(vec![def("x"), def("ok")]);
        t.prior_calls = vec![prior("c1", "x")];
        t.tool_results = vec![by_id("c1")];
        t.tool_choice = ToolChoice::Named("x".to_owned());
        let p = plan_request(&snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t);
        assert_eq!(removed(&p), [("x", false)]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), false)]);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(!p.refuse);
        assert_eq!(p.entries.len(), 3);
        assert!(p.entries.iter().all(|e| !e.enforced));
        assert!(p.is_noop());
    }

    #[test]
    fn a_floor_removal_is_enforced_in_every_mode() {
        let cases = [
            snap(RM_FLOOR, "", TenantPart::Off, TenantMode::Off),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Off),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::LogOnly),
            snap(RM_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Enforce),
            // An unavailable tenant part plans the floor alone in every mode (I3 ruling).
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::Off),
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::LogOnly),
            snap(RM_FLOOR, "", TenantPart::Unavailable, TenantMode::Enforce),
        ];
        for s in cases {
            let p = plan_request(&s, Format::OpenAiChat, &turn(vec![def("rm"), def("ok")]));
            // The floor-alone default deny of "ok" does not count (P-4).
            assert_eq!(removed(&p), [("rm", true)], "{:?}", s.mode);
            assert!(!p.is_noop());
        }
        // Mode `off` with a tenant set: a tenant-only deny is not planned.
        let p = plan_request(&snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::Off), Format::OpenAiChat, &turn(vec![def("x")]));
        assert!(p.removed.is_empty());
    }

    #[test]
    fn an_empty_catalog_in_enforce_removes_every_definition() {
        let invoke = r#"@id("i") permit(principal, action in Pgs::Gateway::Action::"InvokeTool", resource);"#;
        let p = plan_request(&enforce(invoke), Format::OpenAiChat, &turn(vec![def("a"), def("b")]));
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        let unregistered = r#"@id("u") permit(principal, action in Pgs::Gateway::Action::"Unregistered", resource);"#;
        assert!(plan_request(&enforce(unregistered), Format::OpenAiChat, &turn(vec![def("a"), def("b")])).removed.is_empty());
    }

    #[test]
    fn colliding_definitions_are_not_removed() {
        // The collision floor reads `context.findings`, so it cannot deny every input.
        let s = snap(SHIPPED_FLOOR, PERMIT_ALL, TenantPart::Evaluated, TenantMode::Enforce);
        let p = plan_request(&s, Format::OpenAiChat, &turn(vec![def("send"), def("Send")]));
        assert!(p.removed.is_empty());
        assert!(p.is_noop());
    }

    #[test]
    fn hosted_responses_tools_are_never_removed() {
        let defs = vec![
            other("web_search"),
            other("mcp"),
            other("mcp"),
            other("tool_search"),
            other("local_shell"),
            other("shell"),
            other("apply_patch"),
            other("computer"),
            other("computer_use_preview"),
        ];
        let p = plan_request(&enforce(PERMIT_OK), Format::OpenAiResponses, &turn(defs));
        // `mcp` collides with itself, and is still kept (P-15).
        assert_eq!(
            removed(&p),
            [("local_shell", true), ("shell", true), ("apply_patch", true), ("computer", true), ("computer_use_preview", true)]
        );
        // A `computer_use_preview` definition is decided by its call name `computer`.
        let permit_computer = r#"@id("c") permit(principal, action == Pgs::Gateway::Action::"computer", resource);"#;
        let p = plan_request(&enforce(permit_computer), Format::OpenAiResponses, &turn(vec![other("computer_use_preview")]));
        assert!(p.removed.is_empty());
    }

    #[test]
    fn an_id_only_local_shell_result_matches_both_ids_of_the_call() {
        let s = enforce(PERMIT_OK);
        let shell = |name: &str| PriorCall {
            id: Some("c1".to_owned()),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
            local_shell: true,
            item_id: Some("ls_1".to_owned()),
        };
        let out = |id: &str| ToolResultRef {
            call_id: None,
            name: None,
            item_id: Some(id.to_owned()),
        };
        let run = |prior_calls: Vec<PriorCall>, tool_results: Vec<ToolResultRef>| {
            let mut t = turn(vec![]);
            t.prior_calls = prior_calls;
            t.tool_results = tool_results;
            plan_request(&s, Format::OpenAiResponses, &t)
        };
        // The output `id` is the call's `call_id` or its item `id`: replaced either way.
        for id in ["c1", "ls_1"] {
            let p = run(vec![shell("x")], vec![out(id)]);
            assert_eq!(p.replaced, [item(ResultTarget::LocalShellId(id.to_owned()), true)], "{id}");
            assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)], "{id}");
        }
        // An allowed call, or an `id` that matches no call: not replaced.
        assert!(run(vec![shell("ok")], vec![out("ls_1")]).replaced.is_empty());
        assert!(run(vec![shell("x")], vec![out("ls_2")]).replaced.is_empty());
        // Only a `local_shell_call` is matched by `id`.
        let mut function = shell("x");
        function.local_shell = false;
        function.item_id = None;
        assert!(run(vec![function], vec![out("c1")]).replaced.is_empty());
        // A result with both ids: its `call_id` replaces it, with one entry.
        let both = ToolResultRef {
            call_id: Some("c1".to_owned()),
            name: None,
            item_id: Some("ls_1".to_owned()),
        };
        let p = run(vec![shell("x")], vec![both]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert_eq!(p.entries.len(), 1);
    }

    #[test]
    fn result_replacement_by_call_id_and_by_earlier_call() {
        let s = enforce(PERMIT_OK);
        let replaced = |prior_calls: Vec<PriorCall>, tool_results: Vec<ToolResultRef>| {
            let mut t = turn(vec![]);
            t.prior_calls = prior_calls;
            t.tool_results = tool_results;
            plan_request(&s, Format::OpenAiChat, &t)
        };
        // A chat `role:"tool"` result named by its earlier call (Anthropic `tool_use_id`, Responses
        // `call_id` and `approval_request_id` reach the plan in the same shape).
        let p = replaced(vec![prior("c1", "x")], vec![by_id("c1")]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert_eq!(actions(&p), [(RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked)]);
        assert_eq!(p.entries[0].tool, "x");
        // An allowed earlier call: not replaced.
        assert!(replaced(vec![prior("c1", "ok")], vec![by_id("c1")]).replaced.is_empty());
        // Two earlier calls with one id: replaced if any is denied.
        let p = replaced(vec![prior("c1", "ok"), prior("c1", "x")], vec![by_id("c1")]);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        // No earlier call with that id: not replaced.
        assert!(replaced(vec![prior("c2", "x")], vec![by_id("c1")]).replaced.is_empty());
        // A legacy `role:"function"` result carries its name.
        let legacy = ToolResultRef {
            call_id: None,
            name: Some("x".to_owned()),
            item_id: None,
        };
        let p = replaced(vec![], vec![legacy]);
        assert_eq!(p.replaced, [item(ResultTarget::Name("x".to_owned()), true)]);
        // Two results for one call id: one item.
        assert_eq!(replaced(vec![prior("c1", "x")], vec![by_id("c1"), by_id("c1")]).replaced.len(), 1);
        // A result with neither id nor name: not replaced.
        let none = ToolResultRef {
            call_id: None,
            name: None,
            item_id: None,
        };
        assert!(replaced(vec![], vec![none]).replaced.is_empty());
    }

    #[test]
    fn result_replacement_follows_the_floor_and_the_mode() {
        let mut t = turn(vec![]);
        t.prior_calls = vec![prior("c1", "rm"), prior("c2", "x")];
        t.tool_results = vec![by_id("c1"), by_id("c2")];
        let p = plan_request(&snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true), item(ResultTarget::CallId("c2".to_owned()), false)]);
        // Mixed floor and tenant denies on one id: enforced.
        t.prior_calls = vec![prior("c1", "x"), prior("c1", "rm")];
        t.tool_results = vec![by_id("c1")];
        let p = plan_request(&snap(RM_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly), Format::OpenAiChat, &t);
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
    }

    fn with_choice(defs: &[&str], choice: ToolChoice) -> TurnFacts {
        let mut t = turn(defs.iter().map(|n| def(n)).collect());
        t.tool_choice = choice;
        t
    }

    fn resets(p: &RequestPlan) -> Vec<(String, bool)> {
        p.entries.iter().filter(|e| e.action == RequestAction::ResetToolChoice).map(|e| (e.tool.clone(), e.enforced)).collect()
    }

    #[test]
    fn tool_choice_after_removals() {
        let s = enforce(PERMIT_OK);
        let named = |n: &str| ToolChoice::Named(n.to_owned());
        let allowed = |n: &[&str]| ToolChoice::AllowedTools(n.iter().map(|x| (*x).to_owned()).collect());

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], named("A")));
        assert_eq!((p.choice.clone(), p.refuse), (ChoiceRewrite::Auto, false));
        assert_eq!(resets(&p), [("A".to_owned(), true)]);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], named("ok")));
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(resets(&p).is_empty());

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "ok"], allowed(&["a", "ok"])));
        assert_eq!(p.choice, ChoiceRewrite::Allowed(vec!["ok".to_owned()]));
        assert_eq!(resets(&p), [("a".to_owned(), true)]);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "b", "ok"], allowed(&["a", "b"])));
        assert_eq!(p.choice, ChoiceRewrite::Auto);
        assert_eq!(resets(&p).len(), 1);

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["ok"], allowed(&["ok"])));
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(p.is_noop());

        for c in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required] {
            let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a"], c));
            assert_eq!((p.choice.clone(), p.refuse), (ChoiceRewrite::Keep, false));
            assert!(resets(&p).is_empty());
        }

        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown));
        assert!(p.refuse);
        assert!(!p.is_noop());
        assert!(resets(&p).is_empty());
        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["ok"], ToolChoice::Unknown));
        assert!(!p.refuse);

        let log_only = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let p = plan_request(&log_only, Format::OpenAiChat, &with_choice(&["a"], ToolChoice::Unknown));
        assert!(!p.refuse);
        assert!(p.is_noop());
    }

    #[test]
    fn a_log_only_reset_is_reported_once() {
        // P-9: the floor removes "a" (enforced); the log-only tenant would remove "b".
        let s = snap(
            r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"a", resource);"#,
            PERMIT_OK,
            TenantPart::Evaluated,
            TenantMode::LogOnly,
        );
        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "b", "ok"], ToolChoice::Named("b".to_owned())));
        assert_eq!(removed(&p), [("a", true), ("b", false)]);
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert_eq!(resets(&p), [("b".to_owned(), false)]);
        // An enforced reset is reported once, as enforced.
        let p = plan_request(&s, Format::OpenAiChat, &with_choice(&["a", "b", "ok"], ToolChoice::AllowedTools(vec!["a".to_owned(), "b".to_owned()])));
        assert_eq!(p.choice, ChoiceRewrite::Allowed(vec!["b".to_owned()]));
        assert_eq!(resets(&p), [("a".to_owned(), true)]);
    }

    fn parsed(format: Format, body: serde_json::Value) -> TurnFacts {
        match parse_request(format, body.to_string().as_bytes()) {
            ToolTurn::Parsed(f) => f,
            ToolTurn::Unavailable(r) => panic!("unavailable: {r}"),
        }
    }

    /// A tenant that permits everything and forbids one tool for every input.
    fn forbid(tool: &str) -> PolicySnapshot {
        enforce(&format!(r#"{PERMIT_ALL} @id("x") forbid(principal, action == Pgs::Gateway::Action::"{tool}", resource);"#))
    }

    #[test]
    fn a_parsed_result_is_decided_by_the_full_call_name() {
        // A Responses namespace call: the result is decided as `db.query`, not `query`.
        let responses = serde_json::json!({"model":"m",
            "tools":[{"type":"namespace","name":"db","tools":[{"type":"function","name":"query","parameters":{}}]}],
            "input":[{"type":"function_call","call_id":"c1","namespace":"db","name":"query","arguments":"{}"},
                     {"type":"function_call_output","call_id":"c1","output":"x"}]});
        let p = plan_request(&forbid("db.query"), Format::OpenAiResponses, &parsed(Format::OpenAiResponses, responses));
        assert_eq!(p.replaced, [item(ResultTarget::CallId("c1".to_owned()), true)]);
        assert!(p.entries.iter().any(|e| (e.action, e.tool.as_str()) == (RequestAction::ReplaceResult, "db.query")));

        // An MCP approval: the response is decided as `server_label.name`.
        let mcp = serde_json::json!({"model":"m",
            "input":[{"type":"mcp_approval_request","id":"mcpr_1","server_label":"srv","name":"search","arguments":"{}"},
                     {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true}]});
        let p = plan_request(&forbid("srv.search"), Format::OpenAiResponses, &parsed(Format::OpenAiResponses, mcp));
        assert_eq!(p.replaced, [item(ResultTarget::CallId("mcpr_1".to_owned()), true)]);

        // An Anthropic `tool_result` by `tool_use_id`.
        let anthropic = serde_json::json!({"model":"c","tools":[{"name":"rm","input_schema":{}}, {"name":"ok","input_schema":{}}],
            "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}]});
        let p = plan_request(&forbid("rm"), Format::AnthropicMessages, &parsed(Format::AnthropicMessages, anthropic));
        assert_eq!(p.replaced, [item(ResultTarget::CallId("t1".to_owned()), true)]);
    }

    fn anthropic(tools: serde_json::Value, blocks: bool, choice: Option<serde_json::Value>) -> TurnFacts {
        let messages = if blocks {
            serde_json::json!([{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"a","input":{}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}])
        } else {
            serde_json::json!([{"role":"user","content":"hi"}])
        };
        let mut body = serde_json::json!({"model":"c","tools":tools,"messages":messages});
        if let Some(c) = choice {
            body["tool_choice"] = c;
        }
        parsed(Format::AnthropicMessages, body)
    }

    #[test]
    fn anthropic_keeps_the_last_definition_when_blocks_refer_to_tools() {
        let s = enforce(PERMIT_OK);
        let two = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}]);
        // Blocks, every definition denied: `b` (the last) stays, reported not enforced.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), true, None));
        assert_eq!(removed(&p), [("a", true), ("b", false)]);
        assert_eq!(
            actions(&p),
            [
                (RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied),
                (RequestAction::RemoveDefinition, false, RequestReason::ToolCallDenied),
                (RequestAction::ReplaceResult, true, RequestReason::ToolResultBlocked),
            ]
        );
        // A named choice of the kept tool stays named, with no reset entry.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), true, Some(serde_json::json!({"type":"tool","name":"b"}))));
        assert_eq!(p.choice, ChoiceRewrite::Keep);
        assert!(resets(&p).is_empty());
        // A named choice of a removed tool is still reset.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), true, Some(serde_json::json!({"type":"tool","name":"a"}))));
        assert_eq!((p.choice.clone(), resets(&p)), (ChoiceRewrite::Auto, vec![("a".to_owned(), true)]));
        // No blocks: every definition goes.
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two.clone(), false, None));
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        // A server tool stays in `tools[]`, so the rule does not apply.
        let with_server = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}},{"type":"web_search_20250305","name":"web_search"}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(with_server, true, None));
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
        // A definition that stays anyway: the rule does not apply.
        let with_ok = serde_json::json!([{"name":"a","input_schema":{}},{"name":"ok","input_schema":{}}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(with_ok, true, None));
        assert_eq!(removed(&p), [("a", true)]);
        // Another format with the same facts: every definition goes.
        let p = plan_request(&s, Format::OpenAiChat, &anthropic(two, true, None));
        assert_eq!(removed(&p), [("a", true), ("b", true)]);
    }

    #[test]
    fn anthropic_keeps_the_last_definition_only_against_enforced_removals() {
        // Log-only: nothing is enforced, so nothing needs to be kept; the plan reports both.
        let s = snap(NO_FLOOR, PERMIT_OK, TenantPart::Evaluated, TenantMode::LogOnly);
        let two = serde_json::json!([{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}]);
        let p = plan_request(&s, Format::AnthropicMessages, &anthropic(two, true, None));
        assert_eq!(removed(&p), [("a", false), ("b", false)]);
    }
}
