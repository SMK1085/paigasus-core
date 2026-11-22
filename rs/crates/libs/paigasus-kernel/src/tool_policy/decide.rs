// SPDX-License-Identifier: Apache-2.0
//! The decision of one tool call (§ 6.1.5): one Cedar evaluation on the merged floor and tenant set.

use std::str::FromStr;

use cedar_policy::{AuthorizationError, Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicyId, Request, RestrictedExpression};

use std::collections::BTreeSet;

use super::compile::{ActionGroup, PolicyOrigin, PolicyRef, RESOURCE_TYPE, ToolPolicies, action_entity};
use super::input::map_input;
use super::normalise::normalise;
use super::types::{Facts, Findings, ToolInput, ToolInvocation, UNPARSED_ARGUMENTS};
use crate::detect::{DETECTOR_KERNEL, Finding, HostSet, NO_SEVERITY, Severity, call_hosts, detector_error};

/// The effect of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    /// The call may go on.
    Allow,
    /// The call is denied.
    Deny,
}

/// The result of `decide`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Verdict {
    /// The effect.
    pub decision: Effect,
    /// The policies that decided (the reason set), sorted: floor first, then by id.
    pub determining: Vec<PolicyRef>,
    /// The policies that failed to evaluate, sorted the same way.
    pub errored: Vec<PolicyRef>,
    /// Whether an errored policy is a `forbid`. The decision is then `Deny` (D26).
    pub errored_forbid: bool,
    /// Whether the decision is `Deny` and a floor `forbid` is determining or errored (P-3, D33).
    pub floor_denied: bool,
    /// The final finding kinds that Cedar saw: the caller set, plus the kinds of `added`.
    pub findings: BTreeSet<String>,
    /// The action group of the tool name.
    pub action_group: ActionGroup,
    /// The value of `context.max_severity`: the highest severity of the caller findings and of
    /// `added`. `None` (-1 in the context) with no finding, or with the detectors off.
    pub max_severity: Option<Severity>,
    /// The findings that the kernel added itself, detector `kernel`: `unparsed_arguments` (`info`
    /// for free text, `medium` for bad JSON, D21) and the hosts-cap `detector_error` (D36).
    pub added: Vec<Finding>,
    /// The policies of `errored` whose effect is `forbid`, sorted the same way (D12).
    pub errored_forbids: Vec<PolicyRef>,
}

impl Verdict {
    /// The request cannot be evaluated, so the floor was not evaluated. Treat it as floor-denied,
    /// so that the floor stays enforced in every tenant mode (D33).
    fn unevaluable(findings: &Findings, action_group: ActionGroup) -> Self {
        Self {
            decision: Effect::Deny,
            determining: Vec::new(),
            errored: Vec::new(),
            errored_forbid: true,
            floor_denied: true,
            findings: findings.iter().map(str::to_owned).collect(),
            action_group,
            max_severity: findings.max_severity(),
            added: Vec::new(),
            errored_forbids: Vec::new(),
        }
    }
}

/// What the kernel put into the context besides the Cedar value: the final finding kinds, the
/// findings that it added itself and the severity that it wrote.
pub(crate) struct Seen {
    pub kinds: BTreeSet<String>,
    pub added: Vec<Finding>,
    pub max_severity: Option<Severity>,
}

/// The findings that the kernel adds for one call: `unparsed_arguments` when the arguments are
/// not one strict JSON object, and `detector_error` when the host set overflowed.
fn kernel_findings(input: &ToolInput, unparsed: bool, hosts_overflowed: bool) -> Vec<Finding> {
    let mut added = Vec::new();
    if unparsed {
        let severity = if matches!(input, ToolInput::Text(_)) { Severity::Info } else { Severity::Medium };
        added.push(Finding::new(DETECTOR_KERNEL, UNPARSED_ARGUMENTS, severity, ""));
    }
    if hosts_overflowed {
        added.push(detector_error(DETECTOR_KERNEL));
    }
    added
}

fn string(s: &str) -> RestrictedExpression {
    RestrictedExpression::new_string(s.to_owned())
}

fn record<const N: usize>(pairs: [(&str, RestrictedExpression); N]) -> RestrictedExpression {
    RestrictedExpression::new_record(pairs.into_iter().map(|(k, v)| (k.to_owned(), v))).expect("the record keys are distinct")
}

/// The Cedar context of a call. `None` is the optimistic context of `decide_tool`:
/// `input` is `{}`, `call.declared` is `"yes"`, `call.hosts` is `[]` and `max_severity` is -1.
///
/// The hosts come from `call.hosts` when the caller gave them, else from one `call_hosts` scan.
/// With `facts.detection` false, `max_severity` is -1, `call.hosts` is `[]` and the kernel adds
/// no hosts-cap finding.
pub(crate) fn context(call: Option<&ToolInvocation>, facts: &Facts, findings: &Findings) -> (Context, Seen) {
    let mut kinds: BTreeSet<String> = findings.iter().map(str::to_owned).collect();
    let empty = HostSet::new();
    let scanned: HostSet;
    let (input, input_text, declared, hosts, added) = match call {
        Some(c) => {
            let mapped = map_input(&c.input);
            let hosts = match (&c.hosts, facts.detection) {
                (_, false) => &empty,
                (Some(given), true) => given,
                (None, true) => {
                    scanned = call_hosts(&c.input);
                    &scanned
                }
            };
            let added = kernel_findings(&c.input, mapped.unparsed, hosts.overflowed());
            (mapped.input, mapped.input_text, c.declared.as_str(), hosts, added)
        }
        None => (record([]), None, "yes", &empty, Vec::new()),
    };
    kinds.extend(added.iter().map(|f| f.kind.clone()));
    let max_severity = match call {
        Some(_) if facts.detection => findings.max_severity().max(added.iter().map(|f| f.severity).max()),
        _ => None,
    };
    let hosts = RestrictedExpression::new_set(hosts.iter().map(string));
    let mut pairs = vec![
        ("input".to_owned(), input),
        ("findings".to_owned(), RestrictedExpression::new_set(kinds.iter().map(|k| string(k)))),
        ("max_severity".to_owned(), RestrictedExpression::new_long(max_severity.map_or(NO_SEVERITY, Severity::as_long))),
        ("call".to_owned(), record([("declared", string(declared)), ("hosts", hosts)])),
        (
            "request".to_owned(),
            record([
                ("scope", string(&facts.request.scope)),
                ("ingress", string(&facts.request.ingress)),
                ("model", string(&facts.request.model)),
            ]),
        ),
        (
            "session".to_owned(),
            record([("id", string(&facts.session.id)), ("turn", RestrictedExpression::new_long(facts.session.turn))]),
        ),
    ];
    if let Some(text) = input_text {
        pairs.push(("input_text".to_owned(), RestrictedExpression::new_string(text)));
    }
    (Context::from_pairs(pairs).expect("the context keys are distinct"), Seen { kinds, added, max_severity })
}

fn uid(entity_type: &str, id: &str) -> Option<EntityUid> {
    Some(EntityUid::from_type_name_and_id(EntityTypeName::from_str(entity_type).ok()?, EntityId::new(id)))
}

fn sorted(mut refs: Vec<PolicyRef>) -> Vec<PolicyRef> {
    refs.sort_by(|a, b| (a.set == PolicyOrigin::Tenant, &a.id).cmp(&(b.set == PolicyOrigin::Tenant, &b.id)));
    refs
}

/// The Cedar request and the entities for the tool `name`, with the context of `call` (or the
/// optimistic context for `None`), what the kernel added to it, and the action group. `None` if Cedar cannot build the request.
pub(crate) fn request(call: Option<&ToolInvocation>, name: &str, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> Option<(Request, Entities, Seen, ActionGroup)> {
    let (action, entities, group) = action_entity(policies, &normalise(name));
    let org = facts.org.as_hyphenated().to_string();
    let principal = uid(&facts.principal.entity_type, &facts.principal.entity_id)?;
    let resource = uid(RESOURCE_TYPE, &org)?;
    let (context, seen) = context(call, facts, findings);
    let request = Request::new(principal, action, resource, context, None).ok()?;
    Some((request, entities, seen, group))
}

/// Decide one tool call. Pure: no I/O, no clock, no random numbers, no global state.
///
/// A request that Cedar cannot build (an entity type in the facts that is not a valid name)
/// cannot be evaluated, so it is treated as floor-denied: `Deny` with `floor_denied` and
/// `errored_forbid` true.
#[must_use]
pub fn decide(call: &ToolInvocation, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> Verdict {
    let Some((request, entities, seen, action_group)) = request(Some(call), &call.name, facts, findings, policies) else {
        return Verdict::unevaluable(findings, action_entity(policies, &normalise(&call.name)).2);
    };
    let response = Authorizer::new().is_authorized(&request, policies.set(), &entities);
    let diagnostics = response.diagnostics();
    let lookup = |id: &PolicyId| {
        let origin = policies.origin(id).cloned();
        debug_assert!(origin.is_some(), "every merged policy id has an origin");
        origin
    };
    let determining = sorted(diagnostics.reason().filter_map(lookup).collect());
    let errored_ids: Vec<&PolicyId> = diagnostics
        .errors()
        .map(|e| match e {
            AuthorizationError::PolicyEvaluationError(pe) => pe.policy_id(),
        })
        .collect();
    let is_forbid = |id: &&PolicyId| policies.set().policy(id).is_some_and(|p| p.effect() == cedar_policy::Effect::Forbid);
    let errored_forbid = errored_ids.iter().any(is_forbid);
    let errored_forbids = sorted(errored_ids.iter().copied().filter(is_forbid).filter_map(lookup).collect());
    let errored = sorted(errored_ids.into_iter().filter_map(lookup).collect());
    let decision = if response.decision() == Decision::Allow && !errored_forbid { Effect::Allow } else { Effect::Deny };
    // Floor policies are forbids only: `compile_floor` rejects a floor permit.
    let floor_denied = decision == Effect::Deny && determining.iter().chain(&errored).any(|r| r.set == PolicyOrigin::Floor);
    Verdict {
        decision,
        determining,
        errored,
        errored_forbid,
        floor_denied,
        findings: seen.kinds,
        action_group,
        max_severity: seen.max_severity,
        added: seen.added,
        errored_forbids,
    }
}
