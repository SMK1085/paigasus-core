// SPDX-License-Identifier: Apache-2.0
//! Responses record classification (spec § 6.1–§ 6.3).
//!
//! Sources of the two closed lists:
//! - [`STREAM_EVENT_TYPES`]: the 59 members of `ResponseStreamEvent` in openai-python 3.48.0
//!   (`openai/types/responses/response_stream_event.py`), plus `keepalive`, plus the two
//!   `response.apply_patch_call_operation_diff.*` types that the union lacks (spec § 15, A4).
//! - [`scope_of`]: the item-scoped event map of spec § 6.1, checked against openai-node 7.57.0
//!   `expectedOutputItemTypes` (`internal/responses/response-accumulator.mjs:111-153`). Node maps
//!   `response.shell_call_output_content.*` to `shell_call_output`, which spec § 6.1 omits. This
//!   file follows Node, so the item stays provider-run. Node maps `response.content_part.*` by
//!   the part type; [`content_part_item`] does that and [`classify`] rejects an unknown part. The
//!   scope of the event stays `Item("message")` in [`scope_of`]: use [`content_part_item`] for it.

use serde_json::Value;

use crate::json::{Malformed, ResponsesDoc, parse_strict_responses};
use crate::sse::Record;
use crate::types::ToolCallKind;

/// The known `data.type` values (spec § 6.3): 59 union members, `keepalive` and the two
/// apply-patch diff events.
pub(crate) const STREAM_EVENT_TYPES: [&str; 62] = [
    "response.audio.delta",
    "response.audio.done",
    "response.audio.transcript.delta",
    "response.audio.transcript.done",
    "response.code_interpreter_call_code.delta",
    "response.code_interpreter_call_code.done",
    "response.code_interpreter_call.completed",
    "response.code_interpreter_call.in_progress",
    "response.code_interpreter_call.interpreting",
    "response.compaction.compacting",
    "response.completed",
    "response.content_part.added",
    "response.content_part.done",
    "response.created",
    "error",
    "response.file_search_call.completed",
    "response.file_search_call.in_progress",
    "response.file_search_call.searching",
    "response.function_call_arguments.delta",
    "response.function_call_arguments.done",
    "response.shell_call_command.added",
    "response.shell_call_command.delta",
    "response.shell_call_command.done",
    "response.shell_call_output_content.delta",
    "response.shell_call_output_content.done",
    "response.in_progress",
    "response.failed",
    "response.incomplete",
    "response.output_item.added",
    "response.output_item.done",
    "response.reasoning_summary_part.added",
    "response.reasoning_summary_part.done",
    "response.reasoning_summary_text.delta",
    "response.reasoning_summary_text.done",
    "response.reasoning_text.delta",
    "response.reasoning_text.done",
    "response.refusal.delta",
    "response.refusal.done",
    "response.output_text.delta",
    "response.output_text.done",
    "response.web_search_call.completed",
    "response.web_search_call.in_progress",
    "response.web_search_call.searching",
    "response.image_generation_call.completed",
    "response.image_generation_call.generating",
    "response.image_generation_call.in_progress",
    "response.image_generation_call.partial_image",
    "response.mcp_call_arguments.delta",
    "response.mcp_call_arguments.done",
    "response.mcp_call.completed",
    "response.mcp_call.failed",
    "response.mcp_call.in_progress",
    "response.mcp_list_tools.completed",
    "response.mcp_list_tools.failed",
    "response.mcp_list_tools.in_progress",
    "response.output_text.annotation.added",
    "response.queued",
    "response.custom_tool_call_input.delta",
    "response.custom_tool_call_input.done",
    "keepalive",
    "response.apply_patch_call_operation_diff.delta",
    "response.apply_patch_call_operation_diff.done",
];

/// What a stream item is (spec § 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemClass {
    /// `message`, `reasoning`, `compaction`.
    Content,
    /// Run on the provider: reported `unenforceable`, never held.
    ProviderRun,
    /// Run on the client: held and decided.
    ClientRun(ToolCallKind),
}

/// The class of an output item type. An unknown type is client-run of kind `Other` (fail closed).
pub(crate) fn item_class(item_type: &str) -> ItemClass {
    match item_type {
        "message" | "reasoning" | "compaction" => ItemClass::Content,
        "web_search_call"
        | "file_search_call"
        | "code_interpreter_call"
        | "image_generation_call"
        | "mcp_call"
        | "mcp_list_tools"
        | "program"
        | "program_output"
        | "additional_tools"
        | "tool_search_output"
        | "shell_call_output"
        | "local_shell_call_output"
        | "computer_call_output"
        | "apply_patch_call_output" => ItemClass::ProviderRun,
        "function_call" => ItemClass::ClientRun(ToolCallKind::Function),
        "custom_tool_call" => ItemClass::ClientRun(ToolCallKind::Custom),
        "computer_call" => ItemClass::ClientRun(ToolCallKind::Computer),
        "local_shell_call" => ItemClass::ClientRun(ToolCallKind::LocalShell),
        "shell_call" => ItemClass::ClientRun(ToolCallKind::Shell),
        "apply_patch_call" => ItemClass::ClientRun(ToolCallKind::ApplyPatch),
        "tool_search_call" => ItemClass::ClientRun(ToolCallKind::ToolSearch),
        "mcp_approval_request" => ItemClass::ClientRun(ToolCallKind::McpApprovalRequest),
        _ => ItemClass::ClientRun(ToolCallKind::Other),
    }
}

/// Where an event type sits in the stream (spec § 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// `response.created`, `response.in_progress`, `response.queued`.
    Lifecycle,
    /// `response.completed`, `response.failed`, `response.incomplete`.
    Terminal,
    ItemAdded,
    ItemDone,
    /// Scoped to the one item type named. `Item("")` is no valid item type: it never matches.
    Item(&'static str),
    /// Response-level, with no item (`response.audio.*`).
    ResponseLevel,
    Keepalive,
    Error,
}

pub(crate) fn scope_of(ty: &'static str) -> Scope {
    match ty {
        "response.created" | "response.in_progress" | "response.queued" => return Scope::Lifecycle,
        "response.completed" | "response.failed" | "response.incomplete" => return Scope::Terminal,
        "response.output_item.added" => return Scope::ItemAdded,
        "response.output_item.done" => return Scope::ItemDone,
        "keepalive" => return Scope::Keepalive,
        "error" => return Scope::Error,
        "response.compaction.compacting" => return Scope::Item("compaction"),
        _ => {}
    }
    const FAMILIES: &[(&str, &str)] = &[
        ("response.content_part.", "message"),
        ("response.output_text.", "message"),
        ("response.refusal.", "message"),
        ("response.reasoning_text.", "reasoning"),
        ("response.reasoning_summary_part.", "reasoning"),
        ("response.reasoning_summary_text.", "reasoning"),
        ("response.function_call_arguments.", "function_call"),
        ("response.custom_tool_call_input.", "custom_tool_call"),
        ("response.shell_call_command.", "shell_call"),
        ("response.shell_call_output_content.", "shell_call_output"),
        ("response.mcp_call_arguments.", "mcp_call"),
        ("response.mcp_call.", "mcp_call"),
        ("response.mcp_list_tools.", "mcp_list_tools"),
        ("response.web_search_call.", "web_search_call"),
        ("response.file_search_call.", "file_search_call"),
        ("response.code_interpreter_call_code.", "code_interpreter_call"),
        ("response.code_interpreter_call.", "code_interpreter_call"),
        ("response.image_generation_call.", "image_generation_call"),
        ("response.apply_patch_call_operation_diff.", "apply_patch_call"),
    ];
    if ty.starts_with("response.audio.") {
        return Scope::ResponseLevel;
    }
    FAMILIES.iter().find(|(p, _)| ty.starts_with(p)).map_or(Scope::Item(""), |(_, item)| Scope::Item(item))
}

/// The item type of a `response.content_part.added` or `.done` event, by `part.type` (ruling T3a;
/// Node's `getExpectedOutputItemType`). `output_text` and `refusal` belong to `message`,
/// `reasoning_text` to `reasoning`. Any other, missing or non-string part type is malformed.
pub(crate) fn content_part_item(event: &Value) -> Result<&'static str, Malformed> {
    match event.get("part").and_then(|p| p.get("type")) {
        Some(Value::String(t)) => match t.as_str() {
            "output_text" | "refusal" => Ok("message"),
            "reasoning_text" => Ok("reasoning"),
            _ => Err(Malformed("part_type")),
        },
        _ => Err(Malformed("part_type")),
    }
}

/// One classified record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Done,
    UpstreamError(Value),
    Known { ty: &'static str, value: Value },
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Classify one SSE record by `data.type` (spec § 6.3).
pub(crate) fn classify(rec: &Record) -> Result<Event, Malformed> {
    if rec.data == b"[DONE]" {
        return Ok(Event::Done);
    }
    let value = parse_strict_responses(&rec.data, ResponsesDoc::Event)?;
    let Value::Object(obj) = &value else {
        return Err(Malformed("not_object"));
    };
    let ty = match obj.get("type") {
        Some(Value::String(s)) => s.as_str(),
        _ => return Err(Malformed("type")),
    };
    if let Some(name) = &rec.event
        && name != ty
    {
        return Err(Malformed("event_name"));
    }
    if obj.get("error").is_some_and(truthy) || ty == "error" {
        return Ok(Event::UpstreamError(value));
    }
    match STREAM_EVENT_TYPES.iter().find(|t| **t == ty) {
        Some(known) => {
            if matches!(*known, "response.content_part.added" | "response.content_part.done") {
                content_part_item(&value)?;
            }
            Ok(Event::Known { ty: known, value })
        }
        None => Err(Malformed("unknown_type")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn rec(event: Option<&str>, data: &str) -> Record {
        Record {
            event: event.map(str::to_owned),
            data: data.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_event_list_has_sixty_two_types() {
        // 59 union members, `keepalive`, and the two apply-patch diff types (A4).
        assert_eq!(STREAM_EVENT_TYPES.len(), 62);
        assert_eq!(STREAM_EVENT_TYPES.iter().collect::<HashSet<_>>().len(), 62);
        assert!(STREAM_EVENT_TYPES.contains(&"keepalive"));
        assert!(STREAM_EVENT_TYPES.contains(&"response.output_text.delta"));
        assert!(STREAM_EVENT_TYPES.contains(&"response.apply_patch_call_operation_diff.done"));
    }

    #[test]
    fn every_item_scoped_event_has_one_item_type() {
        use Scope::*;
        let rows: &[(&str, Scope)] = &[
            ("response.content_part.added", Item("message")),
            ("response.output_text.delta", Item("message")),
            ("response.refusal.done", Item("message")),
            ("response.output_text.annotation.added", Item("message")),
            ("response.reasoning_text.delta", Item("reasoning")),
            ("response.reasoning_summary_part.added", Item("reasoning")),
            ("response.reasoning_summary_text.done", Item("reasoning")),
            ("response.function_call_arguments.delta", Item("function_call")),
            ("response.custom_tool_call_input.done", Item("custom_tool_call")),
            ("response.shell_call_command.added", Item("shell_call")),
            ("response.mcp_call_arguments.delta", Item("mcp_call")),
            ("response.mcp_call.in_progress", Item("mcp_call")),
            ("response.mcp_list_tools.failed", Item("mcp_list_tools")),
            ("response.web_search_call.searching", Item("web_search_call")),
            ("response.file_search_call.completed", Item("file_search_call")),
            ("response.code_interpreter_call.interpreting", Item("code_interpreter_call")),
            ("response.code_interpreter_call_code.delta", Item("code_interpreter_call")),
            ("response.image_generation_call.partial_image", Item("image_generation_call")),
            ("response.audio.delta", ResponseLevel),
            ("response.audio.transcript.done", ResponseLevel),
            ("response.compaction.compacting", Item("compaction")),
            ("response.apply_patch_call_operation_diff.delta", Item("apply_patch_call")),
            ("response.apply_patch_call_operation_diff.done", Item("apply_patch_call")),
            ("response.shell_call_output_content.delta", Item("shell_call_output")),
            ("response.created", Lifecycle),
            ("response.queued", Lifecycle),
            ("response.in_progress", Lifecycle),
            ("response.completed", Terminal),
            ("response.incomplete", Terminal),
            ("response.failed", Terminal),
            ("response.output_item.added", ItemAdded),
            ("response.output_item.done", ItemDone),
            ("keepalive", Keepalive),
            ("error", Error),
        ];
        for (ty, want) in rows {
            assert_eq!(scope_of(ty), *want, "{ty}");
        }
        // Every known type has a deliberate scope: none falls through to the never-matching item.
        for ty in STREAM_EVENT_TYPES {
            assert_ne!(scope_of(ty), Item(""), "{ty}");
        }
    }

    #[test]
    fn item_classes_follow_section_6_2() {
        for t in ["message", "reasoning", "compaction"] {
            assert_eq!(item_class(t), ItemClass::Content, "{t}");
        }
        for t in [
            "web_search_call",
            "file_search_call",
            "code_interpreter_call",
            "image_generation_call",
            "mcp_call",
            "mcp_list_tools",
            "program",
            "program_output",
            "additional_tools",
            "tool_search_output",
            "shell_call_output",
            "local_shell_call_output",
            "computer_call_output",
            "apply_patch_call_output",
        ] {
            assert_eq!(item_class(t), ItemClass::ProviderRun, "{t}");
        }
        for (t, k) in [
            ("function_call", ToolCallKind::Function),
            ("custom_tool_call", ToolCallKind::Custom),
            ("computer_call", ToolCallKind::Computer),
            ("local_shell_call", ToolCallKind::LocalShell),
            ("shell_call", ToolCallKind::Shell),
            ("apply_patch_call", ToolCallKind::ApplyPatch),
            ("tool_search_call", ToolCallKind::ToolSearch),
            ("mcp_approval_request", ToolCallKind::McpApprovalRequest),
        ] {
            assert_eq!(item_class(t), ItemClass::ClientRun(k), "{t}");
        }
        for t in ["function_call_output", "custom_tool_call_output", "mcp_approval_response", "never_seen", ""] {
            assert_eq!(item_class(t), ItemClass::ClientRun(ToolCallKind::Other), "{t}");
        }
    }

    #[test]
    fn classify_dispatches_on_data_type() {
        let created = r#"{"type":"response.created","error":null,"response":{"id":"r"}}"#;
        assert!(matches!(classify(&rec(Some("response.created"), created)), Ok(Event::Known { ty: "response.created", .. })));
        assert!(matches!(classify(&rec(None, created)), Ok(Event::Known { .. })));
        assert_eq!(classify(&rec(Some("response.completed"), created)), Err(Malformed("event_name")));
        assert_eq!(classify(&rec(None, "[DONE]")), Ok(Event::Done));
        assert!(matches!(classify(&rec(None, r#"{"type":"error","code":"x"}"#)), Ok(Event::UpstreamError(_))));
        assert!(matches!(classify(&rec(None, r#"{"type":"response.created","error":{"message":"m"}}"#)), Ok(Event::UpstreamError(_))));
        assert!(matches!(classify(&rec(Some("keepalive"), r#"{"type":"keepalive"}"#)), Ok(Event::Known { ty: "keepalive", .. })));
        assert_eq!(classify(&rec(None, r#"{"type":"response.never"}"#)), Err(Malformed("unknown_type")));
        assert_eq!(classify(&rec(None, r#"{"x":1}"#)), Err(Malformed("type")));
        assert_eq!(classify(&rec(None, r#"{"type":3}"#)), Err(Malformed("type")));
        assert_eq!(classify(&rec(None, "[1]")), Err(Malformed("not_object")));
        assert_eq!(classify(&rec(None, r#"{"type":"response.created","Item_Id":"x"}"#)), Err(Malformed("json")));
    }

    #[test]
    fn a_content_part_maps_by_its_part_type() {
        for (part, item) in [("output_text", "message"), ("refusal", "message"), ("reasoning_text", "reasoning")] {
            for ty in ["response.content_part.added", "response.content_part.done"] {
                let data = format!(r#"{{"type":"{ty}","part":{{"type":"{part}"}}}}"#);
                let Ok(Event::Known { value, .. }) = classify(&rec(None, &data)) else { panic!("{data}") };
                assert_eq!(content_part_item(&value), Ok(item), "{data}");
            }
        }
        for data in [
            r#"{"type":"response.content_part.added","part":{"type":"audio"}}"#,
            r#"{"type":"response.content_part.done","part":{"type":3}}"#,
            r#"{"type":"response.content_part.done","part":{}}"#,
            r#"{"type":"response.content_part.added"}"#,
            r#"{"type":"response.content_part.added","part":null}"#,
        ] {
            assert_eq!(classify(&rec(None, data)), Err(Malformed("part_type")), "{data}");
        }
    }

    #[test]
    fn recorded_streams_classify_without_a_malformed_record() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/responses/recorded");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "sse") {
                continue;
            }
            let mut splitter = crate::sse::SseSplitter::new(16 * 1024 * 1024);
            let bytes = std::fs::read(&path).unwrap();
            let mut items = splitter.push(&bytes);
            items.extend(splitter.finish());
            for item in items {
                let crate::sse::SseItem::Record(r) = item else { panic!("oversized in {path:?}") };
                if let Err(e) = classify(&r) {
                    panic!("{path:?}: {e}: {}", String::from_utf8_lossy(&r.data));
                }
                seen += 1;
            }
        }
        assert!(seen > 100, "{seen}");
    }
}
