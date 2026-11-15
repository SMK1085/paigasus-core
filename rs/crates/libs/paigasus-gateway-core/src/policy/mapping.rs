// SPDX-License-Identifier: Apache-2.0
//! The mapping from gateway-core types into the kernel tool-policy types (SMA-752 Task 7).

use std::collections::{BTreeMap, BTreeSet};

use paigasus_kernel::tool_policy::{Declared, Findings, NAME_COLLISION, ToolInput, ToolInvocation, normalise};

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
fn definition_call_name(d: &ToolDefinition) -> String {
    let n = normalise(&d.display_name());
    if d.kind == ToolCallKind::Other && n == "computer_use_preview" { "computer".to_owned() } else { n }
}

/// The kernel invocation for a call. The name stays raw: the kernel normalises it.
#[must_use]
pub fn invocation(call: &ToolCall, turn: &ToolTurn) -> ToolInvocation {
    let raw = call.display_name();
    let input = match call.kind {
        ToolCallKind::Custom | ToolCallKind::ApplyPatch | ToolCallKind::Other => ToolInput::Text(call.arguments.clone()),
        ToolCallKind::Function
        | ToolCallKind::LegacyFunctionCall
        | ToolCallKind::AnthropicToolUse
        | ToolCallKind::Computer
        | ToolCallKind::LocalShell
        | ToolCallKind::Shell
        | ToolCallKind::ToolSearch
        | ToolCallKind::McpApprovalRequest => ToolInput::Json(call.arguments.clone()),
    };
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

/// The request-level findings for one call: `name_collision` when its normalised name is in the set.
#[must_use]
pub fn call_findings(call: &ToolCall, collisions: &BTreeSet<String>) -> Findings {
    let mut f = Findings::new();
    if collisions.contains(&normalise(&call.display_name())) {
        f.insert(NAME_COLLISION);
    }
    f
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use paigasus_kernel::tool_policy::{Effect, Facts, RequestInfo, SessionInfo, ToolPolicies, decide};
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
        let prn = Prn::parse("prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004").expect("prn");
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
        assert!(call_findings(&call(" A", ToolCallKind::Function), &set).contains(NAME_COLLISION));
        assert!(!call_findings(&call("b", ToolCallKind::Function), &set).contains(NAME_COLLISION));
    }
}
