// SPDX-License-Identifier: Apache-2.0
//! OpenAI non-stream bodies (spec § 4.2, § 4.4).
//!
//! Each `choices[].message` is turned into the same fragments the stream classifier produces and
//! goes through the same assembly (`check_tool_choice`, `ChoiceAssembly`), so the decider sees
//! the calls a client sees and the canonical rules cannot drift between the stream and the body.
//! A `Deviation` there is `Malformed` here: a body has no earlier frame to release.
//!
//! A body with tool calls must say `"object":"chat.completion"`, by the rule the stream applies to
//! chunks (`classify::check_object`). A text-only body is not checked: it carries no call.

use serde_json::{Map, Value, json};

use crate::json::{MAX_OPENAI_INDEX, Malformed, index_of};
use crate::openai::assembly::{ChoiceAssembly, check_tool_choice};
use crate::openai::classify::{BODY_OBJECT, Fragment, NameArgs, check_object, fragment_at, legacy_call};
use crate::report::notice_line;
use crate::types::{BlockCode, ToolCall};

/// Tool calls per choice are bounded as in the stream, where a fragment `index` is at most 127.
const MAX_CALLS: usize = MAX_OPENAI_INDEX as usize + 1;

fn message_parts(m: &Map<String, Value>) -> Result<(Vec<Fragment>, Option<NameArgs>), Malformed> {
    match m.get("content") {
        None | Some(Value::Null | Value::String(_) | Value::Array(_)) => {}
        Some(_) => return Err(Malformed("content")),
    }
    let entries: &[Value] = match m.get("tool_calls") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(a)) => a,
        Some(_) => return Err(Malformed("tool_calls")),
    };
    if entries.len() > MAX_CALLS {
        return Err(Malformed("too_many_calls"));
    }
    let frags = entries
        .iter()
        .enumerate()
        .map(|(i, e)| fragment_at(e, u32::try_from(i).expect("bounded by MAX_CALLS")))
        .collect::<Result<Vec<_>, _>>()?;
    let legacy = legacy_call(m.get("function_call"))?;
    Ok((frags, legacy))
}

pub(crate) fn calls(v: &Value) -> Result<Vec<ToolCall>, Malformed> {
    let Value::Object(top) = v else { return Err(Malformed("not_object")) };
    let Some(Value::Array(choices)) = top.get("choices") else { return Err(Malformed("choices")) };
    let mut out = Vec::new();
    let mut seen: Vec<u32> = Vec::new();
    let mut carries = false;
    for (position, ch) in choices.iter().enumerate() {
        let Value::Object(c) = ch else { return Err(Malformed("choice")) };
        if c.contains_key("delta") {
            return Err(Malformed("delta_in_body"));
        }
        let index = index_of(c.get("index"), MAX_OPENAI_INDEX)?;
        // The stream rules: a repeated index is malformed (LangChain merges it), and a first-seen
        // index must equal the number of choices seen so far (openai-python finds a choice by
        // list position, P-10).
        if seen.contains(&index) {
            return Err(Malformed("duplicate_choice_index"));
        }
        if index as usize != seen.len() {
            return Err(Malformed("choice_order"));
        }
        seen.push(index);
        let Some(Value::Object(m)) = c.get("message") else { return Err(Malformed("message")) };
        let (frags, legacy) = message_parts(m)?;
        if frags.is_empty() && legacy.is_none() {
            continue;
        }
        carries = true;
        let dev = |d: crate::protocol::Deviation| Malformed(d.0);
        check_tool_choice(index, position, &frags, legacy.as_ref()).map_err(dev)?;
        let mut asm = ChoiceAssembly::default();
        asm.apply(&frags, legacy.as_ref()).map_err(dev)?;
        out.extend(asm.finish(index).map_err(dev)?);
    }
    check_object(top, BODY_OBJECT, carries)?;
    Ok(out)
}

pub(crate) fn rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<BlockCode>]) {
    let Some(Value::Array(choices)) = v.get_mut("choices") else { return };
    for ch in choices {
        let idx = ch.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
        let mine: Vec<(&ToolCall, Option<BlockCode>)> = calls.iter().zip(codes).filter(|(c, _)| c.choice == idx).map(|(c, k)| (c, *k)).collect();
        if mine.iter().all(|(_, k)| k.is_none()) {
            continue;
        }
        let notice: String = mine.iter().filter_map(|(c, k)| k.map(|k| notice_line(&c.name, k))).collect::<Vec<_>>().join("\n");
        let remaining = mine.iter().filter(|(_, k)| k.is_none()).count();
        let blocked: Vec<u32> = mine.iter().filter(|(_, k)| k.is_some()).map(|(c, _)| c.position).collect();
        if let Value::Object(o) = ch {
            o.remove("logprobs");
        }
        if let Some(Value::Object(m)) = ch.get_mut("message") {
            if let Some(Value::Array(tc)) = m.get_mut("tool_calls") {
                let mut pos = 0u32;
                tc.retain(|_| {
                    let keep = !blocked.contains(&pos);
                    pos += 1;
                    keep
                });
                if tc.is_empty() {
                    m.remove("tool_calls");
                }
            }
            if m.contains_key("function_call") && blocked.contains(&0) {
                m.remove("function_call");
            }
            let content = m.remove("content").unwrap_or(Value::Null);
            let new = match content {
                Value::String(s) if !s.is_empty() => Value::String(format!("{s}\n\n{notice}")),
                Value::Array(mut parts) => {
                    parts.push(json!({"type": "text", "text": notice}));
                    Value::Array(parts)
                }
                _ => Value::String(notice),
            };
            m.insert("content".to_owned(), new);
        }
        let finish = ch.get("finish_reason").and_then(Value::as_str).map(str::to_owned);
        if remaining == 0 && matches!(finish.as_deref(), Some("tool_calls" | "function_call")) {
            ch["finish_reason"] = "stop".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bodyless_call_type_over_1024_bytes_is_malformed() {
        let body = |ty: String| json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"tool_calls": [{"id": "c", "type": ty}]}}]});
        assert!(calls(&body("a".repeat(1024))).is_ok());
        assert!(matches!(calls(&body("a".repeat(1025))), Err(Malformed("long_identifier"))));
    }

    #[test]
    fn an_extra_key_in_a_legacy_body_function_call_is_malformed() {
        let body = |fc: serde_json::Value| json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"function_call": fc}}]});
        assert!(matches!(calls(&body(json!({"name": "f", "arguments": "{}", "x": 1}))), Err(Malformed("function_call_key"))));
        assert_eq!(calls(&body(json!({"name": "f", "arguments": "{}"}))).unwrap().len(), 1);
    }

    #[test]
    fn a_function_call_and_a_custom_call_in_one_body_parse_to_two_calls() {
        let tool_calls = json!([
            {"id": "c0", "type": "function", "function": {"name": "read", "arguments": "{}"}},
            {"id": "c1", "type": "custom", "custom": {"name": "note", "input": "x"}}
        ]);
        let body = json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"tool_calls": tool_calls}}]});
        assert_eq!(calls(&body).unwrap().len(), 2);
    }
}
