// SPDX-License-Identifier: Apache-2.0
//! Denied for every input (§ 6.1.6, D5). `decide_tool` says `DeniedForAllInputs` only if the
//! optimistic set denies the call. The optimistic set allows more than the merged set for every
//! input, every set of findings, every `declared` value and every session:
//!
//! - every `permit` keeps its scope and loses its conditions;
//! - every `forbid` whose conditions read `context.input`, `context.input_text`,
//!   `context.findings`, `context.call` (`call.hosts` included), `context.session` or
//!   `context.max_severity` is dropped.
//!
//! So if the optimistic set denies, the merged set denies for every real call.

use cedar_policy::{Authorizer, Decision, Effect, Policy, PolicySet};
use serde_json::Value;

use super::compile::{PolicyOrigin, ToolPolicies};
use super::decide::request;
use super::types::{Facts, Findings};
use crate::detect::SCREEN_INDEPENDENT_KINDS;

/// The result of `decide_tool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolVerdict {
    /// Every call of the tool is denied, whatever its input, findings, `declared` value or session.
    DeniedForAllInputs {
        /// Whether a floor policy decided the deny. Such a deny is always enforced (D33).
        by_floor: bool,
    },
    /// Some call of the tool may be allowed. The definition stays; each call is decided in full.
    MayAllow,
}

/// A part of the context that differs between the calls of one request (or between requests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextAttr {
    Input,
    InputText,
    Findings,
    Call,
    Session,
    MaxSeverity,
}

/// Every part, for a read of the context as a whole or a shape the walk does not know.
const ALL_READS: [ContextAttr; 6] = [
    ContextAttr::Input,
    ContextAttr::InputText,
    ContextAttr::Findings,
    ContextAttr::Call,
    ContextAttr::Session,
    ContextAttr::MaxSeverity,
];

/// A `forbid` that reads one of these parts is dropped from the optimistic set. One element per
/// line, so that a mutation test can delete exactly one rule (preflight F1).
pub(crate) const DROP_READS: &[ContextAttr] = &[
    ContextAttr::Input,       // context.input
    ContextAttr::InputText,   // context.input_text
    ContextAttr::Findings,    // context.findings
    ContextAttr::Call,        // context.call
    ContextAttr::Session,     // context.session
    ContextAttr::MaxSeverity, // context.max_severity
];

/// What a walk has found: some parts, or all of them.
#[derive(Default)]
struct Reads {
    all: bool,
    parts: Vec<ContextAttr>,
}

impl Reads {
    fn attr(&mut self, name: &str) {
        match name {
            "input" => self.parts.push(ContextAttr::Input),
            "input_text" => self.parts.push(ContextAttr::InputText),
            "findings" => self.parts.push(ContextAttr::Findings),
            "call" => self.parts.push(ContextAttr::Call),
            "session" => self.parts.push(ContextAttr::Session),
            "max_severity" => self.parts.push(ContextAttr::MaxSeverity),
            // Fixed for one request.
            "request" => {}
            // An attribute that the context does not hold: count it as a read of all.
            _ => self.all = true,
        }
    }
}

/// Binary operators of the Cedar JSON policy format: `{op: {left, right}}`.
const BINARY: [&str; 17] = [
    "==",
    "!=",
    "in",
    "<",
    "<=",
    ">",
    ">=",
    "&&",
    "||",
    "+",
    "-",
    "*",
    "contains",
    "containsAll",
    "containsAny",
    "hasTag",
    "getTag",
];
/// Unary operators: `{op: {arg}}`.
const UNARY: [&str; 3] = ["!", "neg", "isEmpty"];

fn is_context(v: Option<&Value>) -> bool {
    v.and_then(|v| v.get("Var")).and_then(Value::as_str) == Some("context")
}

/// Walk one expression. Only a `Var` node names the context, so a read is found at the `Var`
/// node or at the `.`/`has` node right above it. Any other shape counts as a read of all.
fn walk(e: &Value, r: &mut Reads) {
    let Some((op, body)) = e.as_object().filter(|m| m.len() == 1).and_then(|m| m.iter().next()) else {
        r.all = true;
        return;
    };
    let field = |k: &str| body.get(k);
    let sub = |k: &str, r: &mut Reads| match field(k) {
        Some(x) => walk(x, r),
        None => r.all = true,
    };
    match op.as_str() {
        "Value" => {}
        "Var" => match body.as_str() {
            Some("principal" | "action" | "resource") => {}
            // `context` as a whole value, or a variable the walk does not know.
            _ => r.all = true,
        },
        "." | "has" if is_context(field("left")) => {
            // `has` takes a path (`context has input.x`) as an array, `.` takes one name.
            match field("attr") {
                Some(Value::String(a)) => r.attr(a),
                Some(Value::Array(path)) => match path.first().and_then(Value::as_str) {
                    Some(a) => r.attr(a),
                    None => r.all = true,
                },
                _ => r.all = true,
            }
        }
        "." | "has" | "like" => sub("left", r),
        "is" => {
            sub("left", r);
            if let Some(x) = field("in") {
                walk(x, r);
            }
        }
        "if-then-else" => ["if", "then", "else"].iter().for_each(|k| sub(k, r)),
        op if BINARY.contains(&op) => ["left", "right"].iter().for_each(|k| sub(k, r)),
        op if UNARY.contains(&op) => sub("arg", r),
        "Set" => match body.as_array() {
            Some(xs) => xs.iter().for_each(|x| walk(x, r)),
            None => r.all = true,
        },
        "Record" => match body.as_object() {
            Some(m) => m.values().for_each(|x| walk(x, r)),
            None => r.all = true,
        },
        // An extension function or method call (`decimal`, `lessThan`, `ip`, ...): every argument
        // is an expression, so a read of the context inside one is found by the walk.
        _ if body.is_array() => body.as_array().into_iter().flatten().for_each(|x| walk(x, r)),
        _ => r.all = true,
    }
}

/// The parts of the context that the conditions of a policy (its `Policy::to_json` form) read,
/// in the order of `ALL_READS`. A shape the walk does not know reads all of them (§ 6.1.6).
pub(crate) fn policy_reads(policy: &Value) -> Vec<ContextAttr> {
    let mut r = Reads::default();
    match policy.get("conditions").and_then(Value::as_array) {
        Some(conditions) => {
            for c in conditions {
                match c.get("body") {
                    Some(body) => walk(body, &mut r),
                    None => r.all = true,
                }
            }
        }
        None => r.all = true,
    }
    ALL_READS.into_iter().filter(|a| r.all || r.parts.contains(a)).collect()
}

/// Whether `e` is `context.findings`.
fn is_findings(e: Option<&Value>) -> bool {
    e.and_then(|e| e.get("."))
        .is_some_and(|d| is_context(d.get("left")) && d.get("attr").and_then(Value::as_str) == Some("findings"))
}

/// Whether an expression reads `context.findings` in a form other than
/// `context.findings.contains(<literal>)` with a decoded value in `SCREEN_INDEPENDENT_KINDS`.
/// Any other node is searched in full, which can only find more reads (the safe side).
fn reads_dependent_findings(e: &Value) -> bool {
    match e {
        Value::Object(m) => {
            if let Some(c) = m.get("contains").filter(|_| m.len() == 1)
                && is_findings(c.get("left"))
                && let Some(Value::String(kind)) = c.get("right").and_then(|r| r.get("Value"))
            {
                return !SCREEN_INDEPENDENT_KINDS.contains(&kind.as_str());
            }
            if m.len() == 1 && is_findings(Some(e)) {
                return true;
            }
            if let Some(h) = m.get("has").filter(|_| m.len() == 1)
                && is_context(h.get("left"))
            {
                let first = match h.get("attr") {
                    Some(Value::String(a)) => Some(a.as_str()),
                    Some(Value::Array(path)) => path.first().and_then(Value::as_str),
                    _ => None,
                };
                // A `has` that names no attribute the walk can read counts as a findings read.
                if first.is_none_or(|a| a == "findings") {
                    return true;
                }
            }
            m.values().any(reads_dependent_findings)
        }
        Value::Array(xs) => xs.iter().any(reads_dependent_findings),
        _ => false,
    }
}

/// Whether a policy (its `Policy::to_json` form) reads a screen-dependent part of the context
/// (§ 6.8.0, D27): `max_severity`, `call`, the context as a whole, a shape the walk does not know,
/// or `findings` in any form other than `contains` of a literal whose decoded value is in
/// `SCREEN_INDEPENDENT_KINDS`. The literal comes from the JSON form, so an escape in the source
/// text cannot hide a dependent kind.
pub(crate) fn screen_dependent(policy: &Value) -> bool {
    // A read of all (`context` as a whole, an unknown shape) holds both parts.
    if policy_reads(policy).iter().any(|a| matches!(a, ContextAttr::MaxSeverity | ContextAttr::Call)) {
        return true;
    }
    match policy.get("conditions").and_then(Value::as_array) {
        Some(conditions) => conditions.iter().any(|c| c.get("body").is_none_or(reads_dependent_findings)),
        None => true,
    }
}

/// The permit `p` with its scope and without its conditions. If the JSON round trip fails, an
/// unconditional permit with the same id, which allows even more.
fn strip_conditions(p: &Policy) -> Policy {
    let stripped = p.to_json().ok().and_then(|mut json| {
        *json.get_mut("conditions")? = Value::Array(Vec::new());
        Policy::from_json(Some(p.id().clone()), json).ok()
    });
    stripped.unwrap_or_else(|| Policy::parse(Some(p.id().clone()), "permit(principal, action, resource);").expect("the unconditional permit parses"))
}

/// Build the optimistic set of `merged`. Production passes `DROP_READS` and `true`; the other
/// values exist for the mutation proof only. Policy ids stay the same, so the origins of the
/// merged set apply.
pub(crate) fn optimistic(merged: &PolicySet, drop_reads: &[ContextAttr], strip_permit_conditions: bool) -> PolicySet {
    let mut out = PolicySet::new();
    for p in merged.policies() {
        let kept = match p.effect() {
            Effect::Permit if strip_permit_conditions => Some(strip_conditions(p)),
            Effect::Permit => Some(p.clone()),
            Effect::Forbid => {
                let reads = p.to_json().map_or_else(|_| ALL_READS.to_vec(), |json| policy_reads(&json));
                (!reads.iter().any(|a| drop_reads.contains(a))).then(|| p.clone())
            }
        };
        if let Some(k) = kept {
            out.add(k).expect("the ids of the merged set are unique");
        }
    }
    out
}

/// Evaluate a given optimistic set for the tool `name`, with Cedar's normal handling: an errored
/// policy is skipped, so an errored forbid does not deny.
fn evaluate(set: &PolicySet, name: &str, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> ToolVerdict {
    let Some((request, entities, ..)) = request(None, name, facts, findings, policies) else {
        return ToolVerdict::MayAllow;
    };
    let response = Authorizer::new().is_authorized(&request, set, &entities);
    if response.decision() == Decision::Allow {
        return ToolVerdict::MayAllow;
    }
    // Floor policies are forbids only, so a floor policy in the reason set is a floor deny.
    let by_floor = response.diagnostics().reason().any(|id| policies.origin(id).is_some_and(|r| r.set == PolicyOrigin::Floor));
    ToolVerdict::DeniedForAllInputs { by_floor }
}

/// Whether the tool `name` is denied for every input (§ 6.1.6). `findings` are the request-level
/// findings. Pure: no I/O, no clock, no random numbers, no global state.
///
/// A request that Cedar cannot build gives `MayAllow`: the definition stays, and `decide` denies
/// each call of it.
#[must_use]
pub fn decide_tool(name: &str, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> ToolVerdict {
    evaluate(policies.optimistic_set(), name, facts, findings, policies)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::CedarUid;
    use crate::detect::Severity;
    use crate::tool_policy::{Declared, Effect as Outcome, NAME_COLLISION, RequestInfo, SessionInfo, ToolInput, ToolInvocation, decide};

    const ALLOW_ALL: &str = r#"@id("all") permit(principal, action, resource);"#;
    const D15: &str = r#"@id("floor-name-collision") forbid(principal, action, resource) when { context.findings.contains("name_collision") };"#;

    fn policies(floor: &str, tenant: &str, catalog: &[&str]) -> ToolPolicies {
        let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
        let catalog: Vec<String> = catalog.iter().map(|s| (*s).to_owned()).collect();
        ToolPolicies::compile(floor, tenant, &catalog).expect("tenant")
    }

    fn facts(turn: i64) -> Facts {
        let uid = CedarUid {
            entity_type: "Pgs::Iam::Principal".into(),
            entity_id: "u".into(),
        };
        Facts::new(uid, Uuid::nil(), RequestInfo::new("prn:pgs:gateway:::scope/x", "responses", "m"), SessionInfo::new("", turn))
    }

    fn findings(kinds: &[&str]) -> Findings {
        let mut f = Findings::new();
        kinds.iter().for_each(|k| f.add(k, if *k == "name_collision" { Severity::High } else { Severity::Medium }));
        f
    }

    fn findings_with(kinds: &[(&str, Severity)]) -> Findings {
        let mut f = Findings::new();
        kinds.iter().for_each(|(k, s)| f.add(k, *s));
        f
    }

    fn dropped(cond: &str) -> bool {
        let p = policies("", &format!(r#"@id("f") forbid(principal, action, resource) when {{ {cond} }};"#), &[]);
        assert_eq!(p.set().policies().count(), 1, "{cond}: the merged set holds the forbid");
        p.optimistic_set().policies().count() == 0
    }

    #[test]
    fn a_forbid_that_reads_only_request_facts_stays() {
        for cond in [
            r#"context.request.model == "m""#,
            r#"context has request"#,
            r#"principal is Pgs::Iam::Principal && action in Pgs::Gateway::Action::"InvokeTool""#,
            r#"decimal("1.5").lessThan(decimal("2.0")) && [1, 2].contains(1)"#,
            r#"if context.request.ingress like "resp*" then true else false"#,
        ] {
            assert!(!dropped(cond), "{cond}");
        }
        // The kept forbid is the one of the merged set.
        let p = policies("", r#"@id("f") forbid(principal, action, resource) when { context.request.model == "m" };"#, &[]);
        assert_eq!(p.optimistic_set().policies().next().map(|q| q.id().to_string()), Some("tenant/f".to_owned()));
    }

    #[test]
    fn each_reading_form_drops_a_forbid() {
        for cond in [
            "context.input.x == 1",
            "context has input",
            "context.input has x",
            "context.input has x.y",
            r#"context.input_text like "*""#,
            r#"context.findings.contains("name_collision")"#,
            r#"context.call.declared == "no""#,
            "context.session.turn > 0",
            "context == {}",
            r#"context["input"] == {}"#,
            "context.unknown_slot == 1",
            r#"context.request.model == "m" && context.input.x == 1"#,
            "!(context.input.x < 5)",
            "if true then context.session.turn == 0 else false",
            r#"decimal("1.5").lessThan(context.input.d)"#,
        ] {
            assert!(dropped(cond), "{cond}");
        }
    }

    #[test]
    fn a_forbid_on_max_severity_or_hosts_is_dropped() {
        for c in ["context.max_severity >= 3", r#"context.call.hosts.contains("a.example")"#] {
            assert!(dropped(c), "{c}");
        }
        // The reads are the named part, not a read of all (ruling F20).
        let reads = |c: &str| {
            let p = Policy::parse(None, format!("forbid(principal, action, resource) when {{ {c} }};")).expect("parses");
            policy_reads(&p.to_json().expect("json"))
        };
        assert_eq!(reads("context.max_severity >= 3"), [ContextAttr::MaxSeverity]);
        assert_eq!(reads("context.max_severity == -1 && context.session.turn > 0"), [ContextAttr::Session, ContextAttr::MaxSeverity]);
        assert_eq!(reads(r#"context.call.hosts.contains("a.example")"#), [ContextAttr::Call]);
    }

    #[test]
    fn an_unknown_json_shape_drops_the_forbid() {
        let body = |b: Value| json!({ "effect": "forbid", "conditions": [{ "kind": "when", "body": b }] });
        for est in [
            body(json!({ "frobnicate": { "left": { "Value": 1 } } })),
            body(json!({ "Unknown": { "name": "u" } })),
            body(json!({ "==": { "left": { "Value": 1 } } })),
            body(json!({ "Var": "something" })),
            body(json!([1, 2])),
            json!({ "effect": "forbid", "conditions": "when" }),
            json!({ "effect": "forbid", "conditions": [{ "kind": "when" }] }),
            json!({ "effect": "forbid" }),
        ] {
            assert_eq!(policy_reads(&est), ALL_READS, "{est}");
        }
        // A known shape that reads nothing reads nothing.
        assert!(policy_reads(&body(json!({ "Value": true }))).is_empty());
        assert!(policy_reads(&json!({ "effect": "forbid", "conditions": [] })).is_empty());
    }

    #[test]
    fn decide_tool_attribution() {
        let f = facts(0);
        let none = Findings::new();
        let floor_rm = r#"@id("rm") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
        let p = policies(floor_rm, ALLOW_ALL, &["rm"]);
        assert_eq!(decide_tool("rm", &f, &none, &p), ToolVerdict::DeniedForAllInputs { by_floor: true });
        assert_eq!(decide_tool(" RM ", &f, &none, &p), ToolVerdict::DeniedForAllInputs { by_floor: true });
        assert_eq!(decide_tool("ls", &f, &none, &p), ToolVerdict::MayAllow);

        let read = r#"@id("r") permit(principal, action == Pgs::Gateway::Action::"read_file", resource) when { context.input.path like "/workspace/*" };"#;
        let p = policies("", read, &["read_file"]);
        assert_eq!(decide_tool("x", &f, &none, &p), ToolVerdict::DeniedForAllInputs { by_floor: false });
        // The permit loses its condition, not its scope.
        assert_eq!(decide_tool("read_file", &f, &none, &p), ToolVerdict::MayAllow);

        // The D15 forbid reads the findings, so it is dropped: the decision is left to each call.
        let p = policies(D15, ALLOW_ALL, &["x"]);
        assert_eq!(decide_tool("x", &f, &findings(&[NAME_COLLISION]), &p), ToolVerdict::MayAllow);

        // A tenant forbid on fixed facts is a deny, but not a floor deny.
        let p = policies(
            "",
            &format!(r#"{ALLOW_ALL} @id("m") forbid(principal, action, resource) when {{ context.request.model == "m" }};"#),
            &[],
        );
        assert_eq!(decide_tool("x", &f, &none, &p), ToolVerdict::DeniedForAllInputs { by_floor: false });

        // An errored forbid is skipped in the optimistic set.
        let p = policies(r#"@id("e") forbid(principal, action, resource) when { context.request.missing == 1 };"#, ALLOW_ALL, &[]);
        assert_eq!(decide_tool("x", &f, &none, &p), ToolVerdict::MayAllow);

        // A principal type that is not a Cedar name cannot be evaluated: the definition stays.
        let mut bad = facts(0);
        bad.principal.entity_type = "not a type".into();
        assert_eq!(decide_tool("rm", &bad, &none, &policies(floor_rm, ALLOW_ALL, &[])), ToolVerdict::MayAllow);
    }

    /// One counterexample: the merged set allows the call, but a mutated optimistic set denies it.
    struct Counter {
        tenant: &'static str,
        call: ToolInvocation,
        tool_findings: Findings,
        call_findings: Findings,
        tool_turn: i64,
        call_turn: i64,
    }

    fn counter(tenant: &'static str, input: ToolInput) -> Counter {
        Counter {
            tenant,
            call: ToolInvocation::new("t", input, Declared::Yes),
            tool_findings: Findings::new(),
            call_findings: Findings::new(),
            tool_turn: 0,
            call_turn: 0,
        }
    }

    fn json(text: &str) -> ToolInput {
        ToolInput::Json(text.into())
    }

    fn counterexample(attr: ContextAttr) -> Counter {
        match attr {
            ContextAttr::Input => counter(
                r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) unless { context.input has a };"#,
                json(r#"{"a": 1}"#),
            ),
            ContextAttr::InputText => counter(
                r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) unless { context has input_text };"#,
                ToolInput::Text("t".into()),
            ),
            ContextAttr::Findings => Counter {
                tool_findings: findings(&[NAME_COLLISION]),
                ..counter(
                    r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) when { context.findings.contains("name_collision") };"#,
                    json("{}"),
                )
            },
            ContextAttr::Call => Counter {
                call: ToolInvocation::new("t", json("{}"), Declared::No),
                ..counter(
                    r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) when { context.call.declared == "yes" };"#,
                    json("{}"),
                )
            },
            ContextAttr::Session => Counter {
                call_turn: 1,
                ..counter(
                    r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) when { context.session.turn == 0 };"#,
                    json("{}"),
                )
            },
            ContextAttr::MaxSeverity => Counter {
                call_findings: findings_with(&[("x", Severity::Low)]),
                ..counter(
                    r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource) when { context.max_severity == -1 };"#,
                    json("{}"),
                )
            },
        }
    }

    /// Assert that `set` (built from `p`) says `DeniedForAllInputs` while `decide` allows `c.call`.
    fn assert_unsound(c: &Counter, p: &ToolPolicies, set: &PolicySet, what: &str) {
        let tool = evaluate(set, &c.call.name, &facts(c.tool_turn), &c.tool_findings, p);
        assert!(matches!(tool, ToolVerdict::DeniedForAllInputs { .. }), "{what}: the mutated set must deny, got {tool:?}");
        let v = decide(&c.call, &facts(c.call_turn), &c.call_findings, p);
        assert_eq!(v.decision, Outcome::Allow, "{what}: the real call must be allowed");
        // The production set is sound on the same case.
        assert_eq!(decide_tool(&c.call.name, &facts(c.tool_turn), &c.tool_findings, p), ToolVerdict::MayAllow, "{what}: production");
    }

    #[test]
    fn removing_any_drop_rule_breaks_soundness() {
        assert_eq!(DROP_READS, ALL_READS, "production drops every part");
        for attr in ALL_READS {
            let c = counterexample(attr);
            let p = policies("", c.tenant, &[]);
            let mutated: Vec<ContextAttr> = DROP_READS.iter().copied().filter(|a| *a != attr).collect();
            assert_unsound(&c, &p, &optimistic(p.set(), &mutated, true), &format!("{attr:?}"));
        }
        // A permit that keeps its condition is unsound too.
        let c = counter(r#"@id("p") permit(principal, action, resource) when { context.input has a };"#, json(r#"{"a": 1}"#));
        let p = policies("", c.tenant, &[]);
        assert_unsound(&c, &p, &optimistic(p.set(), DROP_READS, false), "strip_permit_conditions = false");
    }
}
