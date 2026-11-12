// SPDX-License-Identifier: Apache-2.0
//! Anthropic non-stream bodies (spec § 4.2, § 4.4).
//!
//! The classification matches the stream's: `text`, `thinking` and `redacted_thinking` are
//! content, server types are reported as unenforceable, `tool_use` is decided with the compact
//! JSON of its `input` (the value a client reads, so the decider sees what the client runs), and
//! any other type is Other, with the block as a one-element JSON array like the stream's raws.
//! `block_interleave`, `incomplete_arguments` and the `prefilled_*` rules describe fragments and
//! have no whole-body counterpart.

use serde_json::{Map, Value, json};

use crate::json::{MAX_ANTHROPIC_INDEX, Malformed, opt_str};
use crate::protocol::MAX_IDENTIFIER_BYTES;
use crate::report::{ReportDecision, ReportEntry, notice_line};
use crate::types::{BlockCode, ToolCall, ToolCallKind};

pub(crate) fn calls(v: &Value) -> Result<(Vec<ToolCall>, Vec<ReportEntry>), Malformed> {
    let Some(Value::Array(content)) = v.get("content") else { return Err(Malformed("content")) };
    // The stream bounds a block index at MAX_ANTHROPIC_INDEX.
    if content.len() as u64 > MAX_ANTHROPIC_INDEX + 1 {
        return Err(Malformed("too_many_blocks"));
    }
    let (mut calls, mut reports) = (Vec::new(), Vec::new());
    for (i, b) in content.iter().enumerate() {
        let position = u32::try_from(i).expect("bounded by MAX_ANTHROPIC_INDEX");
        let Value::Object(o) = b else { return Err(Malformed("block")) };
        let ty = o.get("type").and_then(Value::as_str).ok_or(Malformed("block_type"))?;
        match ty {
            "text" | "thinking" | "redacted_thinking" => {}
            "server_tool_use" | "mcp_tool_use" => reports.push(server(o, ty, position)),
            t if t.ends_with("_tool_result") => reports.push(server(o, ty, position)),
            "tool_use" => {
                let id = opt_str(o.get("id"), "id")?;
                let name = o.get("name").and_then(Value::as_str).ok_or(Malformed("name"))?.to_owned();
                long_check(id.as_deref(), &name)?;
                let Some(input @ Value::Object(_)) = o.get("input") else { return Err(Malformed("input")) };
                calls.push(ToolCall {
                    choice: 0,
                    position,
                    id,
                    name,
                    kind: ToolCallKind::AnthropicToolUse,
                    arguments: input.to_string(),
                });
            }
            _ => {
                let id = opt_str(o.get("id"), "id")?;
                let name = o.get("name").and_then(Value::as_str).unwrap_or(ty).to_owned();
                long_check(id.as_deref(), &name)?;
                calls.push(ToolCall {
                    choice: 0,
                    position,
                    id,
                    name,
                    kind: ToolCallKind::Other,
                    arguments: serde_json::to_string(&[b]).expect("a Value array always serializes"),
                });
            }
        }
    }
    Ok((calls, reports))
}

fn long_check(id: Option<&str>, name: &str) -> Result<(), Malformed> {
    if id.is_some_and(|s| s.len() > MAX_IDENTIFIER_BYTES) || name.len() > MAX_IDENTIFIER_BYTES {
        return Err(Malformed("long_identifier"));
    }
    Ok(())
}

fn server(o: &Map<String, Value>, ty: &str, position: u32) -> ReportEntry {
    let name = o.get("name").and_then(Value::as_str).unwrap_or(ty).to_owned();
    ReportEntry {
        choice: 0,
        position,
        name,
        decision: ReportDecision::Unenforceable,
        basis: None,
        code: None,
    }
}

pub(crate) fn rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<BlockCode>]) {
    let remaining = codes.iter().filter(|k| k.is_none()).count();
    if let Some(Value::Array(content)) = v.get_mut("content") {
        for (c, code) in calls.iter().zip(codes) {
            if let (Some(code), Some(slot)) = (code, content.get_mut(c.position as usize)) {
                *slot = json!({"type": "text", "text": notice_line(&c.name, *code)});
            }
        }
    }
    if remaining == 0 && v.get("stop_reason").and_then(Value::as_str) == Some("tool_use") {
        v["stop_reason"] = "end_turn".into();
    }
}
