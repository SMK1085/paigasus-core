// SPDX-License-Identifier: Apache-2.0
//! Non-stream Responses bodies (spec § 6.5 non-stream, § 15).
//!
//! The body is parsed with `parse_strict_responses(.., Body)` and checked with
//! `check_numbers_responses(.., Body)` by the caller. [`calls`] then reads `output[]` and applies
//! the per-item rules that a body can satisfy without a stream: R8 (item shape, unique `id`s and
//! `call_id`s, no reserved prefix), the classification of § 6.2, and R11 for a completed
//! `function_call`. It shares its item and status logic with the stream (`assembly`).
//!
//! A client-run item whose `status` is not `"completed"` is a truncation (R15, ruling F7). It is
//! not `Malformed`: [`forced`] gives it the block code `Truncated`, which [`rewrite`] applies over
//! the decider's answer.

use std::collections::HashSet;

use serde_json::Value;

use crate::json::Malformed;
use crate::report::{Blocked, ReportDecision, ReportEntry, notice_line};
use crate::responses::assembly::{RESERVED_ID_PREFIX, arguments_complete, is_completed, item_shape, tool_call};
use crate::responses::classify::{ItemClass, item_class};
use crate::responses::rewrite::completed_message;
use crate::types::{BlockCode, ToolCall, ToolCallKind};

/// The client-run calls of a body, in `output_index` order, and the `unenforceable` entries of its
/// provider-run items. A truncated call is in the list: see [`forced`].
///
/// # Errors
/// `Malformed` when `output` is not an array, or an item breaks R8 or R11, or a call cannot be read.
pub(crate) fn calls(v: &Value) -> Result<(Vec<ToolCall>, Vec<ReportEntry>), Malformed> {
    let Some(Value::Array(output)) = v.get("output") else { return Err(Malformed("output")) };
    let mut out = Vec::new();
    let mut unenforceable = Vec::new();
    let mut ids: HashSet<&str> = HashSet::new();
    let mut call_ids: HashSet<&str> = HashSet::new();
    for (i, item) in output.iter().enumerate() {
        let position = u32::try_from(i).map_err(|_| Malformed("too_many_items"))?;
        let (obj, ty, id) = item_shape(Some(item)).map_err(|d| Malformed(d.0))?;
        if id.starts_with(RESERVED_ID_PREFIX) {
            return Err(Malformed("reserved_id"));
        }
        if !ids.insert(id) {
            return Err(Malformed("duplicate_id"));
        }
        if let Some(Value::String(c)) = obj.get("call_id")
            && !call_ids.insert(c)
        {
            return Err(Malformed("duplicate_call_id"));
        }
        match item_class(ty) {
            ItemClass::Content => {}
            ItemClass::ProviderRun => unenforceable.push(ReportEntry {
                choice: 0,
                position,
                name: ty.to_owned(),
                decision: ReportDecision::Unenforceable,
                basis: None,
                code: None,
                enforced: None,
                reason: None,
            }),
            ItemClass::ClientRun(kind) => {
                let call = tool_call(position, obj, kind)?;
                // R11, for the item a single frame can satisfy. Any other status is R15 (A3).
                if kind == ToolCallKind::Function && is_completed(obj, ty) && !arguments_complete(&call.arguments) {
                    return Err(Malformed("incomplete_arguments"));
                }
                out.push(call);
            }
        }
    }
    Ok((out, unenforceable))
}

/// The block code that the body itself forces on each call: `Truncated` for a call whose item is
/// not `"completed"` (R15, ruling F7). It takes precedence over any decision. The report of the
/// service uses it too, so the header and the rewrite agree.
pub(crate) fn forced(v: &Value, calls: &[ToolCall]) -> Vec<Option<BlockCode>> {
    calls
        .iter()
        .map(|c| {
            let item = v.get("output").and_then(|o| o.get(c.position as usize)).and_then(Value::as_object)?;
            let ty = item.get("type").and_then(Value::as_str)?;
            (!is_completed(item, ty)).then_some(BlockCode::Truncated)
        })
        .collect()
}

/// Replace each blocked item with the completed message of § 6.5. Only `output[i]` changes: the
/// `status`, `usage`, `incomplete_details` and `error` of the body stay as they were.
/// `codes[i]` is the decision for `calls[i]`; a forced `Truncated` wins over it.
pub(crate) fn rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<Blocked>]) {
    let forced = forced(v, calls);
    let Some(Value::Array(output)) = v.get_mut("output") else { return };
    for ((c, code), forced) in calls.iter().zip(codes).zip(forced) {
        let Some(code) = forced.map(|code| Blocked { code, reference: None }).or(*code) else { continue };
        if let Some(slot) = output.get_mut(c.position as usize) {
            *slot = completed_message(c.position, &notice_line(&c.display_name(), code.code, code.reference));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{ResponsesDoc, check_numbers_responses, parse_strict_responses};
    use serde_json::json;

    fn fc(id: &str, call_id: &str, args: &str, status: &str) -> Value {
        json!({"type":"function_call","id":id,"call_id":call_id,"name":"read","arguments":args,"status":status})
    }
    fn body(output: Vec<Value>) -> Value {
        json!({"id":"resp_1","object":"response","status":"completed","output":output,"usage":{"total_tokens":3},"incomplete_details":null,"error":null})
    }
    fn err_of(output: Vec<Value>) -> Option<&'static str> {
        calls(&body(output)).err().map(|e| e.0)
    }

    #[test]
    fn a_body_yields_its_client_run_calls() {
        let v = body(vec![
            json!({"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[]}),
            fc("fc_1", "c1", "{\"a\":1}", "completed"),
            json!({"type":"custom_tool_call","id":"ctc_1","call_id":"c2","name":"sql","namespace":"db","input":"SELECT 1"}),
            json!({"type":"shell_call","id":"sh_1","call_id":"c3","status":"completed","action":{"commands":["ls"],"max_output_length":20000}}),
            json!({"type":"mcp_approval_request","id":"mcpr_1","name":"rm","server_label":"srv","arguments":"{}"}),
        ]);
        let (calls, unenforceable) = calls(&v).unwrap();
        assert!(unenforceable.is_empty());
        assert_eq!(
            calls.iter().map(|c| (c.position, c.id.as_deref(), c.display_name(), c.kind)).collect::<Vec<_>>(),
            [
                (1, Some("c1"), "read".to_owned(), ToolCallKind::Function),
                (2, Some("c2"), "db.sql".to_owned(), ToolCallKind::Custom),
                (3, Some("c3"), "shell".to_owned(), ToolCallKind::Shell),
                (4, Some("mcpr_1"), "srv.rm".to_owned(), ToolCallKind::McpApprovalRequest),
            ]
        );
        assert_eq!(calls[0].arguments, "{\"a\":1}");
        assert_eq!(calls[1].arguments, "SELECT 1");
        assert!(!calls[2].arguments.contains("status"));
    }

    #[test]
    fn provider_items_are_unenforceable() {
        let v = body(vec![
            json!({"type":"web_search_call","id":"ws_1","status":"completed"}),
            json!({"type":"reasoning","id":"rs_1","summary":[]}),
            json!({"type":"mcp_call","id":"mcp_1","name":"x","arguments":"{}"}),
        ]);
        let (calls, un) = calls(&v).unwrap();
        assert!(calls.is_empty());
        assert_eq!(
            un.iter().map(|e| (e.position, e.name.as_str(), e.decision)).collect::<Vec<_>>(),
            [(0, "web_search_call", ReportDecision::Unenforceable), (2, "mcp_call", ReportDecision::Unenforceable)]
        );
    }

    const ID: &str = "0193f6c2-8a4e-7b10-9c2d-5e8f1a2b3c4d";

    #[test]
    fn responses_body_notice_has_the_ref() {
        let v = body(vec![fc("fc_0", "c0", "{}", "completed"), fc("fc_1", "c1", "{}", "completed")]);
        let (c, _) = calls(&v).unwrap();
        let id = uuid::Uuid::try_parse(ID).unwrap();
        let mut out = v.clone();
        rewrite(
            &mut out,
            &c,
            &[
                Some(Blocked {
                    code: BlockCode::Policy,
                    reference: Some(id),
                }),
                Some(Blocked {
                    code: BlockCode::Policy,
                    reference: None,
                }),
            ],
        );
        assert_eq!(out["output"][0]["content"][0]["text"], format!("[paigasus] tool call \"read\" was blocked (policy). ref={ID}"));
        assert_eq!(out["output"][1]["content"][0]["text"], "[paigasus] tool call \"read\" was blocked (policy).");
    }

    #[test]
    fn a_blocked_item_becomes_the_completed_message() {
        let v = body(vec![fc("fc_0", "c0", "{}", "completed"), fc("fc_1", "c1", "{\"p\":\"SECRET\"}", "completed")]);
        let (c, _) = calls(&v).unwrap();
        let mut out = v.clone();
        rewrite(
            &mut out,
            &c,
            &[
                None,
                Some(Blocked {
                    code: BlockCode::Policy,
                    reference: None,
                }),
            ],
        );
        assert_eq!(out["output"][0], v["output"][0]);
        assert_eq!(out["output"][1]["id"], "msg_paigasus_1");
        assert_eq!(out["output"][1]["content"][0]["text"], "[paigasus] tool call \"read\" was blocked (policy).");
        for k in ["status", "usage", "incomplete_details", "error", "id", "object"] {
            assert_eq!(out[k], v[k], "{k}");
        }
        assert!(!out.to_string().contains("SECRET"));
        // Nothing blocked: nothing changes.
        let mut same = v.clone();
        rewrite(&mut same, &c, &[None, None]);
        assert_eq!(same, v);
    }

    /// Ruling F7: a client-run item that is not `"completed"` is blocked with `Truncated`, never
    /// `Malformed`, and it wins over an allow.
    #[test]
    fn an_item_not_completed_is_truncated() {
        for status in ["incomplete", "in_progress"] {
            let v = body(vec![fc("fc_0", "c0", "{\"a\":", status)]);
            let (c, _) = calls(&v).unwrap();
            assert_eq!(forced(&v, &c), [Some(BlockCode::Truncated)], "{status}");
            let mut out = v.clone();
            rewrite(&mut out, &c, &[None]);
            assert_eq!(out["output"][0]["id"], "msg_paigasus_0");
            assert!(out["output"][0]["content"][0]["text"].as_str().unwrap().ends_with("(truncated)."));
        }
        // An absent status: a truncation for a type whose model has `status`, a call otherwise (T4b).
        let mut no_status = fc("fc_0", "c0", "{}", "completed");
        no_status.as_object_mut().unwrap().remove("status");
        let v = body(vec![no_status, json!({"type":"custom_tool_call","id":"ctc_1","call_id":"c2","name":"sql","input":"x"})]);
        let (c, _) = calls(&v).unwrap();
        assert_eq!(forced(&v, &c), [Some(BlockCode::Truncated), None]);
    }

    #[test]
    fn body_rules_follow_r8_and_r11() {
        let msg = |id: &str| json!({"type":"message","id":id,"role":"assistant","status":"completed","content":[]});
        assert_eq!(err_of(vec![msg("a"), msg("a")]), Some("duplicate_id"));
        assert_eq!(err_of(vec![fc("a", "c", "{}", "completed"), fc("b", "c", "{}", "completed")]), Some("duplicate_call_id"));
        assert_eq!(err_of(vec![msg("msg_paigasus_0")]), Some("reserved_id"));
        assert_eq!(err_of(vec![json!({"type":"message","id":""})]), Some("item_shape"));
        assert_eq!(err_of(vec![json!({"id":"a"})]), Some("item_shape"));
        for bad in ["{\"a\":", "not json", "{} {}"] {
            assert_eq!(err_of(vec![fc("a", "c", bad, "completed")]), Some("incomplete_arguments"), "{bad}");
        }
        for ok in ["", "{}", "\"x\"", "[1]"] {
            assert_eq!(err_of(vec![fc("a", "c", ok, "completed")]), None, "{ok}");
        }
        assert_eq!(calls(&json!({"id":"r"})).err(), Some(Malformed("output")));
        assert_eq!(calls(&json!({"output":null})).err(), Some(Malformed("output")));
    }

    #[test]
    fn a_strictly_parsed_body_reads_the_same() {
        let bytes = body(vec![fc("fc_1", "c1", "{}", "completed")]).to_string();
        let v = parse_strict_responses(bytes.as_bytes(), ResponsesDoc::Body).unwrap();
        check_numbers_responses(&v, ResponsesDoc::Body).unwrap();
        assert_eq!(calls(&v).unwrap().0.len(), 1);
    }
}
