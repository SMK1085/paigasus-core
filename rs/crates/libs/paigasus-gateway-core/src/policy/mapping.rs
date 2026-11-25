// SPDX-License-Identifier: Apache-2.0
//! The mapping from gateway-core types into the kernel tool-policy types (SMA-752 Task 7).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use paigasus_kernel::detect::{DETECTOR_KERNEL, Finding, Severity};
use paigasus_kernel::tool_policy::history::{ExtractedFields, extract_fields};
use paigasus_kernel::tool_policy::{Declared, NAME_COLLISION, ToolInput, ToolInvocation, normalise};
use serde_json::Value;

use crate::policy::history::FieldPlan;
use crate::turn::{ToolDefinition, ToolTurn};
use crate::types::{Format, ToolCall, ToolCallKind};

/// The value of `context.request.ingress` for a wire format.
#[must_use]
pub fn ingress(format: Format) -> &'static str {
    match format {
        Format::OpenAiChat => "chat_completions",
        Format::AnthropicMessages => "anthropic_messages",
        Format::OpenAiResponses => "responses",
    }
}

/// The normalised display names that two or more visible definitions share (D23).
#[must_use]
pub fn name_collisions(defs: &[ToolDefinition]) -> BTreeSet<String> {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for d in defs {
        *count.entry(normalise(&d.display_name())).or_default() += 1;
    }
    count.into_iter().filter(|(_, n)| *n >= 2).map(|(name, _)| name).collect()
}

/// The normalised name under which a call of this definition arrives. A built-in definition is
/// named by its type, but its call has a fixed name: `computer_use_preview` gives `computer` calls.
/// The other built-ins (`local_shell`, `shell`, `apply_patch`, `tool_search`) use one name for both.
pub(crate) fn definition_call_name(d: &ToolDefinition) -> String {
    let n = normalise(&d.display_name());
    if d.kind == ToolCallKind::Other && n == "computer_use_preview" { "computer".to_owned() } else { n }
}

/// The kernel input for call arguments: text for a custom-style call, JSON for the others.
pub(crate) fn input_for(kind: ToolCallKind, arguments: &str) -> ToolInput {
    match kind {
        ToolCallKind::Custom | ToolCallKind::ApplyPatch | ToolCallKind::Other => ToolInput::Text(arguments.to_owned()),
        ToolCallKind::Function
        | ToolCallKind::LegacyFunctionCall
        | ToolCallKind::AnthropicToolUse
        | ToolCallKind::Computer
        | ToolCallKind::LocalShell
        | ToolCallKind::Shell
        | ToolCallKind::ToolSearch
        | ToolCallKind::McpApprovalRequest => ToolInput::Json(arguments.to_owned()),
    }
}

/// The stored fields of one call under a field plan (SMA-753 P3). `None` when no plan applies.
/// The values stay in memory until the report site digests them; no record holds them.
#[must_use]
pub(crate) fn call_fields(call: &ToolCall, plan: Option<&FieldPlan>) -> Option<ExtractedFields> {
    let plan = plan?;
    let paths = plan.paths_for(&normalise(&call.display_name()));
    Some(extract_fields(&input_for(call.kind, &call.arguments), &paths))
}

/// Whether a call of this kind carries JSON arguments that a definition's JSON schema describes.
fn has_json_schema(kind: ToolCallKind) -> bool {
    matches!(kind, ToolCallKind::Function | ToolCallKind::LegacyFunctionCall | ToolCallKind::AnthropicToolUse)
}

/// The schema of the one definition that declares this call. `None` for zero or two matching
/// definitions, for a schema that is not an object, for incomplete definitions (the declaring
/// definition can be one that is not visible), and unless the call is a JSON-schema kind equal
/// to the definition's kind: a `custom` call never gets a `function` schema (A7).
#[must_use]
pub fn call_schema(call: &ToolCall, turn: &ToolTurn) -> Option<Arc<Value>> {
    let ToolTurn::Parsed(f) = turn else { return None };
    if !f.definitions_complete || !has_json_schema(call.kind) {
        return None;
    }
    let n = normalise(&call.display_name());
    let mut hits = f.definitions.iter().filter(|d| definition_call_name(d) == n);
    let first = hits.next()?;
    if hits.next().is_some() || first.kind != call.kind {
        return None;
    }
    first.schema.clone().filter(|s| s.is_object())
}

/// The kernel invocation for a call. The name stays raw: the kernel normalises it.
#[must_use]
pub fn invocation(call: &ToolCall, turn: &ToolTurn) -> ToolInvocation {
    let raw = call.display_name();
    let input = input_for(call.kind, &call.arguments);
    let declared = match turn {
        ToolTurn::Parsed(f) => {
            let n = normalise(&raw);
            // Spec 6.1.1: with incomplete definitions the kernel cannot tell. // GUARD-INCOMPLETE
            if !f.definitions_complete {
                Declared::Unknown
            } else if call.kind == ToolCallKind::McpApprovalRequest && f.definitions.iter().any(|d| d.kind == ToolCallKind::Other && d.name == "mcp") {
                // The call is named `server_label.name`; the definition is the bare `mcp` type.
                Declared::Unknown
            } else if f.definitions.iter().any(|d| definition_call_name(d) == n) {
                Declared::Yes
            } else {
                Declared::No
            }
        }
        ToolTurn::Unavailable(_) => Declared::Unknown,
    };
    ToolInvocation::new(raw, input, declared)
}

/// The request-level findings for one call: `name_collision` (detector `kernel`, `high`, no
/// version) when its normalised name is in the set.
#[must_use]
pub fn call_findings(call: &ToolCall, collisions: &BTreeSet<String>) -> Vec<Finding> {
    if collisions.contains(&normalise(&call.display_name())) {
        vec![Finding::new(DETECTOR_KERNEL, NAME_COLLISION, Severity::High, "")]
    } else {
        Vec::new()
    }
}

/// A field plan that stores `path` for the tool `tool`, for the tests of the report sites.
#[cfg(test)]
pub(crate) fn test_plan(tool: &str, path: &str) -> FieldPlan {
    use paigasus_kernel::tool_policy::history::{Catalog, CmpOp, Condition, FieldPath, FieldTest, Literal, Part, Predicate, Scope, Shape, ToolMatch};
    let path = FieldPath::new(path.split('.').map(str::to_owned).collect()).expect("path");
    let predicate = Predicate::AllOf(
        Box::new(Predicate::Tool(ToolMatch::NameKey(tool.to_owned()))),
        Box::new(Predicate::Field(FieldTest::new(path, CmpOp::Gt, Literal::Long(1)))),
    );
    FieldPlan::of(&[Condition::new("s", Part::Tenant, Scope::Session, 1000, Shape::Formerly { predicate })], &Catalog::unknown())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::tool_policy::{Effect, Facts, Findings, RequestInfo, SessionInfo, ToolPolicies, decide};
    use paigasus_kernel::{Prn, to_cedar_uid};
    use uuid::Uuid;

    use super::*;
    use crate::turn::TurnFacts;

    fn def(name: &str, ns: Option<&str>) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: ns.map(str::to_owned),
            kind: ToolCallKind::Function,
            schema: None,
        }
    }

    fn call(name: &str, kind: ToolCallKind) -> ToolCall {
        ToolCall {
            choice: 0,
            position: 0,
            id: None,
            name: name.to_owned(),
            namespace: None,
            kind,
            arguments: "{}".to_owned(),
        }
    }

    fn turn(definitions: Vec<ToolDefinition>, definitions_complete: bool) -> ToolTurn {
        ToolTurn::Parsed(TurnFacts {
            definitions,
            definitions_complete,
            ..TurnFacts::default()
        })
    }

    #[test]
    fn ingress_names() {
        assert_eq!(ingress(Format::OpenAiChat), "chat_completions");
        assert_eq!(ingress(Format::AnthropicMessages), "anthropic_messages");
        assert_eq!(ingress(Format::OpenAiResponses), "responses");
    }

    #[test]
    fn collisions_group_by_normalised_display_name() {
        let c = name_collisions(&[def("Read_File", None), def("read_file ", None), def("read_file", Some("ns")), def("other", None)]);
        assert_eq!(c.into_iter().collect::<Vec<_>>(), ["read_file"]);
        // A namespaced tool and a bare tool of the same short name do not collide.
        assert!(name_collisions(&[def("read_file", Some("ns")), def("read_file", None)]).is_empty());
    }

    #[test]
    fn a_call_name_is_normalised_before_decide() {
        let c = call("READ_FILE ", ToolCallKind::Function);
        let inv = invocation(&c, &turn(vec![def("read_file", None)], true));
        assert_eq!(inv.name, "READ_FILE ");
        let floor = Arc::new(ToolPolicies::compile_floor(r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"read_file", resource);"#).expect("floor"));
        let policies = ToolPolicies::compile(floor, "", &[]).expect("compile");
        let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
        let facts = Facts::new(
            to_cedar_uid(&prn),
            Uuid::nil(),
            RequestInfo::new("prn:pgs:gateway:::scope/x", ingress(Format::OpenAiChat), "m"),
            SessionInfo::placeholder(),
        );
        let v = decide(&inv, &facts, &Findings::new(), &policies);
        assert_eq!(v.decision, Effect::Deny);
        assert!(v.floor_denied);
    }

    #[test]
    fn declared_follows_the_turn() {
        let c = call("Read_File", ToolCallKind::Function);
        let defs = vec![def("read_file", None)];
        assert_eq!(invocation(&c, &turn(defs.clone(), true)).declared, Declared::Yes);
        assert_eq!(invocation(&call("nope", ToolCallKind::Function), &turn(defs.clone(), true)).declared, Declared::No);
        assert_eq!(invocation(&call("nope", ToolCallKind::Function), &turn(defs.clone(), false)).declared, Declared::Unknown);
        assert_eq!(invocation(&c, &ToolTurn::Unavailable("malformed")).declared, Declared::Unknown);
        // Found, but the definitions are incomplete: Unknown (spec 6.1.1).
        assert_eq!(invocation(&c, &turn(defs, false)).declared, Declared::Unknown);
    }

    fn other_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            namespace: None,
            kind: ToolCallKind::Other,
            schema: None,
        }
    }

    #[test]
    fn built_in_definitions_match_their_fixed_call_names() {
        for (def_type, kind, call_name) in [
            ("computer_use_preview", ToolCallKind::Computer, "computer"),
            ("computer", ToolCallKind::Computer, "computer"),
            ("local_shell", ToolCallKind::LocalShell, "local_shell"),
            ("shell", ToolCallKind::Shell, "shell"),
            ("apply_patch", ToolCallKind::ApplyPatch, "apply_patch"),
            ("tool_search", ToolCallKind::ToolSearch, "tool_search"),
        ] {
            let c = call(call_name, kind);
            let complete = invocation(&c, &turn(vec![other_def(def_type)], true)).declared;
            // `tool_search` makes the real parser set incomplete; here the set is complete.
            assert_eq!(complete, Declared::Yes, "{def_type}");
            assert_eq!(invocation(&c, &turn(vec![other_def("web_search")], true)).declared, Declared::No, "{def_type}");
        }
    }

    #[test]
    fn an_mcp_approval_call_is_unknown_when_an_mcp_definition_exists() {
        let mut c = call("search", ToolCallKind::McpApprovalRequest);
        c.namespace = Some("srv".to_owned());
        assert_eq!(invocation(&c, &turn(vec![other_def("mcp")], true)).declared, Declared::Unknown);
        assert_eq!(invocation(&c, &turn(vec![other_def("web_search")], true)).declared, Declared::No);
    }

    #[test]
    fn input_kind_follows_the_call_kind() {
        let t = turn(vec![], true);
        for k in [ToolCallKind::Function, ToolCallKind::LegacyFunctionCall, ToolCallKind::AnthropicToolUse] {
            assert!(matches!(invocation(&call("x", k), &t).input, ToolInput::Json(_)), "{k:?}");
        }
        for k in [ToolCallKind::Custom, ToolCallKind::ApplyPatch, ToolCallKind::Other] {
            assert!(matches!(invocation(&call("x", k), &t).input, ToolInput::Text(_)), "{k:?}");
        }
        for k in [
            ToolCallKind::Computer,
            ToolCallKind::LocalShell,
            ToolCallKind::Shell,
            ToolCallKind::ToolSearch,
            ToolCallKind::McpApprovalRequest,
        ] {
            assert!(matches!(invocation(&call("x", k), &t).input, ToolInput::Json(_)), "{k:?}");
        }
    }

    #[test]
    fn collision_finding_per_call() {
        let set = name_collisions(&[def("a", None), def("A", None), def("b", None)]);
        let found = call_findings(&call(" A", ToolCallKind::Function), &set);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].kind.as_str(), found[0].severity, found[0].detector.as_str(), found[0].version.as_str()),
            (NAME_COLLISION, Severity::High, DETECTOR_KERNEL, "")
        );
        assert!(call_findings(&call("b", ToolCallKind::Function), &set).is_empty());
    }

    #[test]
    fn call_schema_needs_exactly_one_object_schema() {
        let with = |name: &str, schema: Option<Value>| {
            let mut d = def(name, None);
            d.schema = schema.map(Arc::new);
            d
        };
        let turn_of = |definitions: Vec<ToolDefinition>| {
            ToolTurn::Parsed(TurnFacts {
                definitions,
                definitions_complete: true,
                ..TurnFacts::default()
            })
        };
        let c = call("read", ToolCallKind::Function);
        let obj = serde_json::json!({"type": "object"});
        let one = turn_of(vec![with("read", Some(obj.clone())), with("other", None)]);
        assert_eq!(call_schema(&c, &one).as_deref(), Some(&obj));
        let two = turn_of(vec![with("read", Some(obj.clone())), with("Read", Some(obj.clone()))]);
        assert_eq!(call_schema(&c, &two), None);
        let boolean = turn_of(vec![with("read", Some(serde_json::json!(true)))]);
        assert_eq!(call_schema(&c, &boolean), None);
        assert_eq!(call_schema(&c, &turn_of(vec![with("other", Some(obj))])), None);
    }

    #[test]
    fn call_schema_needs_a_json_call_of_the_definition_kind() {
        let obj = serde_json::json!({"type": "object"});
        let with_kind = |kind: ToolCallKind| {
            let mut d = def("read", None);
            d.kind = kind;
            d.schema = Some(Arc::new(obj.clone()));
            d
        };
        let complete = |d: ToolDefinition| turn(vec![d], true);
        // A custom (text) call gets no schema, also from a function definition of the same name.
        assert_eq!(call_schema(&call("read", ToolCallKind::Custom), &complete(with_kind(ToolCallKind::Function))), None);
        assert_eq!(call_schema(&call("read", ToolCallKind::Custom), &complete(with_kind(ToolCallKind::Custom))), None);
        // A JSON call of another kind than the definition gets no schema.
        assert_eq!(call_schema(&call("read", ToolCallKind::Function), &complete(with_kind(ToolCallKind::Custom))), None);
        assert_eq!(call_schema(&call("read", ToolCallKind::Function), &complete(with_kind(ToolCallKind::LegacyFunctionCall))), None);
        // Each JSON-schema kind matches its own definition kind.
        for k in [ToolCallKind::Function, ToolCallKind::LegacyFunctionCall, ToolCallKind::AnthropicToolUse] {
            assert_eq!(call_schema(&call("read", k), &complete(with_kind(k))).as_deref(), Some(&obj), "{k:?}");
        }
    }

    #[test]
    fn call_schema_is_none_for_incomplete_definitions() {
        let mut d = def("read", None);
        d.schema = Some(Arc::new(serde_json::json!({"type": "object"})));
        let c = call("read", ToolCallKind::Function);
        assert!(call_schema(&c, &turn(vec![d.clone()], true)).is_some());
        assert_eq!(call_schema(&c, &turn(vec![d], false)), None);
        assert_eq!(call_schema(&c, &ToolTurn::Unavailable("malformed")), None);
    }

    #[test]
    fn input_for_picks_json_or_text_by_kind() {
        assert!(matches!(input_for(ToolCallKind::Custom, "x"), ToolInput::Text(_)));
        assert!(matches!(input_for(ToolCallKind::Function, "{}"), ToolInput::Json(_)));
    }
}
