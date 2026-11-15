// SPDX-License-Identifier: Apache-2.0
//! The decision of one tool call (§ 6.1.5): one Cedar evaluation on the merged floor and tenant set.

use std::str::FromStr;

use cedar_policy::{AuthorizationError, Authorizer, Context, Decision, EntityId, EntityTypeName, EntityUid, PolicyId, Request, RestrictedExpression};

use super::compile::{PolicyOrigin, PolicyRef, RESOURCE_TYPE, ToolPolicies, action_entity};
use super::input::map_input;
use super::normalise::normalise;
use super::types::{Facts, Findings, ToolInvocation, UNPARSED_ARGUMENTS};

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
}

impl Verdict {
    fn deny() -> Self {
        Self {
            decision: Effect::Deny,
            determining: Vec::new(),
            errored: Vec::new(),
            errored_forbid: false,
            floor_denied: false,
        }
    }
}

fn string(s: &str) -> RestrictedExpression {
    RestrictedExpression::new_string(s.to_owned())
}

fn record<const N: usize>(pairs: [(&str, RestrictedExpression); N]) -> RestrictedExpression {
    RestrictedExpression::new_record(pairs.into_iter().map(|(k, v)| (k.to_owned(), v))).expect("the record keys are distinct")
}

/// The Cedar context of a call. `None` is the optimistic context of `decide_tool`:
/// `input` is `{}` and `call.declared` is `"yes"`.
pub(crate) fn context(call: Option<&ToolInvocation>, facts: &Facts, findings: &Findings) -> Context {
    let mut kinds: Vec<String> = findings.iter().map(str::to_owned).collect();
    let (input, input_text, declared) = match call {
        Some(c) => {
            let mapped = map_input(&c.input);
            if mapped.unparsed && !kinds.iter().any(|k| k == UNPARSED_ARGUMENTS) {
                kinds.push(UNPARSED_ARGUMENTS.to_owned());
            }
            (mapped.input, mapped.input_text, c.declared.as_str())
        }
        None => (record([]), None, "yes"),
    };
    let mut pairs = vec![
        ("input".to_owned(), input),
        ("findings".to_owned(), RestrictedExpression::new_set(kinds.iter().map(|k| string(k)))),
        ("call".to_owned(), record([("declared", string(declared))])),
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
    Context::from_pairs(pairs).expect("the context keys are distinct")
}

fn uid(entity_type: &str, id: &str) -> Option<EntityUid> {
    Some(EntityUid::from_type_name_and_id(EntityTypeName::from_str(entity_type).ok()?, EntityId::new(id)))
}

fn sorted(mut refs: Vec<PolicyRef>) -> Vec<PolicyRef> {
    refs.sort_by(|a, b| (a.set == PolicyOrigin::Tenant, &a.id).cmp(&(b.set == PolicyOrigin::Tenant, &b.id)));
    refs
}

/// Decide one tool call. Pure: no I/O, no clock, no random numbers, no global state.
///
/// A request that Cedar cannot build (an entity type in the facts that is not a valid name) is
/// a plain `Deny` that no floor policy decided.
#[must_use]
pub fn decide(call: &ToolInvocation, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> Verdict {
    let normalised = normalise(&call.name);
    let (action, entities) = action_entity(policies, &normalised);
    let org = facts.org.as_hyphenated().to_string();
    let (Some(principal), Some(resource)) = (uid(&facts.principal.entity_type, &facts.principal.entity_id), uid(RESOURCE_TYPE, &org)) else {
        return Verdict::deny();
    };
    let Ok(request) = Request::new(principal, action, resource, context(Some(call), facts, findings), None) else {
        return Verdict::deny();
    };
    let response = Authorizer::new().is_authorized(&request, policies.set(), &entities);
    let diagnostics = response.diagnostics();
    let lookup = |id: &PolicyId| policies.origin(id).cloned();
    let determining = sorted(diagnostics.reason().filter_map(lookup).collect());
    let errored_ids: Vec<&PolicyId> = diagnostics
        .errors()
        .map(|e| match e {
            AuthorizationError::PolicyEvaluationError(pe) => pe.policy_id(),
        })
        .collect();
    let errored_forbid = errored_ids.iter().any(|id| policies.set().policy(id).is_some_and(|p| p.effect() == cedar_policy::Effect::Forbid));
    let errored = sorted(errored_ids.into_iter().filter_map(lookup).collect());
    let decision = if response.decision() == Decision::Allow && !errored_forbid { Effect::Allow } else { Effect::Deny };
    let floor_denied = decision == Effect::Deny && determining.iter().chain(&errored).any(|r| r.set == PolicyOrigin::Floor);
    Verdict {
        decision,
        determining,
        errored,
        errored_forbid,
        floor_denied,
    }
}
