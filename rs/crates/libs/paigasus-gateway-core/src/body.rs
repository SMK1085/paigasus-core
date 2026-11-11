// SPDX-License-Identifier: Apache-2.0
//! Non-stream bodies (spec § 4.2, § 4.4, § 5.2). The same strict parse and the same canonical
//! rules as the streams, applied to `choices[].message` (OpenAI) or `content[]` (Anthropic).
//!
//! A body that the streams would call a deviation is `Malformed` here: there is no earlier
//! frame to release, so the caller applies `on_failure` to the whole body.

use serde_json::Value;

use crate::json::{Malformed, parse_strict};
use crate::report::{ReportDecision, ReportEntry, header_value};
use crate::types::{Basis, BlockCode, Decision, Format, ToolCall};
use crate::{anthropic, openai};

/// The tool calls of one complete body, and the means to rewrite it.
pub struct BodyCalls {
    format: Format,
    value: Value,
    pub calls: Vec<ToolCall>,
    pub unenforceable: Vec<ReportEntry>,
}

/// Parse a complete `chat.completion` or Messages body (strictly) and extract its tool calls.
///
/// # Errors
/// `Malformed` when the body is over `max_bytes`, is not strict JSON, or is not in canonical form.
pub fn parse_body(format: Format, body: &[u8], max_bytes: usize) -> Result<BodyCalls, Malformed> {
    if body.len() > max_bytes {
        return Err(Malformed("too_large"));
    }
    let value = parse_strict(body)?;
    let (calls, unenforceable) = match format {
        Format::OpenAiChat => (openai::body::calls(&value)?, Vec::new()),
        Format::AnthropicMessages => anthropic::body::calls(&value)?,
    };
    Ok(BodyCalls { format, value, calls, unenforceable })
}

impl BodyCalls {
    /// The block code of call `i`. A missing decision is a block (D18).
    fn code(decisions: &[Decision], i: usize) -> Option<BlockCode> {
        match decisions.get(i) {
            Some(Decision::Block { code }) => Some(*code),
            Some(_) => None,
            None => Some(BlockCode::DeciderError),
        }
    }

    /// The rewritten body, or `None` when nothing is blocked and the original bytes go through.
    #[must_use]
    pub fn apply(self, decisions: &[Decision]) -> Option<Vec<u8>> {
        let codes: Vec<Option<BlockCode>> = (0..self.calls.len()).map(|i| Self::code(decisions, i)).collect();
        if codes.iter().all(Option::is_none) {
            return None;
        }
        let mut value = self.value;
        match self.format {
            Format::OpenAiChat => openai::body::rewrite(&mut value, &self.calls, &codes),
            Format::AnthropicMessages => anthropic::body::rewrite(&mut value, &self.calls, &codes),
        }
        Some(serde_json::to_vec(&value).expect("a Value always serializes"))
    }

    /// The `paigasus-tool-decisions` header value (D22), including the unenforceable entries.
    #[must_use]
    pub fn report(&self, decisions: &[Decision], basis: Basis) -> String {
        let mut entries: Vec<ReportEntry> = self
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let code = Self::code(decisions, i);
                let decision = if code.is_some() { ReportDecision::Block } else { ReportDecision::Allow };
                ReportEntry {
                    choice: c.choice,
                    position: c.position,
                    name: c.name.clone(),
                    decision,
                    basis: Some(basis),
                    code,
                }
            })
            .collect();
        entries.extend(self.unenforceable.iter().cloned());
        header_value(&entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCallKind;
    use serde_json::json;

    const MAX: usize = 1 << 20;
    fn block() -> Decision {
        Decision::Block { code: BlockCode::Policy }
    }
    fn openai(message: Value, finish: &str) -> Vec<u8> {
        json!({"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":message,"logprobs":{"content":[{"token":"SECRET"}]},"finish_reason":finish}],"usage":{"total_tokens":3}}).to_string().into_bytes()
    }
    fn two_calls() -> Vec<u8> {
        openai(
            json!({"role":"assistant","content":null,"tool_calls":[
                {"id":"c0","type":"function","function":{"name":"read","arguments":"{}"}},
                {"id":"c1","type":"function","function":{"name":"rm","arguments":"SECRET"}}
            ]}),
            "tool_calls",
        )
    }
    fn err_of(format: Format, v: &Value) -> Option<&'static str> {
        parse_body(format, v.to_string().as_bytes(), MAX).err().map(|e| e.0)
    }

    #[test]
    fn openai_calls_are_extracted_in_position_order() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        assert_eq!(
            b.calls.iter().map(|c| (c.position, c.name.as_str(), c.kind)).collect::<Vec<_>>(),
            [(0, "read", ToolCallKind::Function), (1, "rm", ToolCallKind::Function)]
        );
        assert_eq!(b.calls[1].arguments, "SECRET");
        assert_eq!(b.calls[1].id.as_deref(), Some("c1"));
    }

    #[test]
    fn allow_all_forwards_the_original_bytes() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        assert!(b.apply(&[Decision::Allow, Decision::Allow]).is_none());
    }

    #[test]
    fn a_blocked_openai_call_is_removed_and_noticed() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let out = String::from_utf8(b.apply(&[Decision::Allow, block()]).unwrap()).unwrap();
        assert!(!out.contains("SECRET"), "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        let m = &v["choices"][0]["message"];
        assert_eq!(m["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(m["content"], "[paigasus] tool call \"rm\" was blocked (policy).");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert!(v["choices"][0].get("logprobs").is_none());
    }

    #[test]
    fn blocking_every_openai_call_sets_stop_and_appends_to_text() {
        let body = openai(
            json!({"role":"assistant","content":"I will.","tool_calls":[{"id":"c0","type":"function","function":{"name":"rm","arguments":"SECRET"}}]}),
            "tool_calls",
        );
        let out = parse_body(Format::OpenAiChat, &body, MAX).unwrap().apply(&[block()]).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v["choices"][0]["message"].get("tool_calls").is_none());
        assert_eq!(v["choices"][0]["message"]["content"], "I will.\n\n[paigasus] tool call \"rm\" was blocked (policy).");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn two_blocked_calls_make_two_notice_lines() {
        let out = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap().apply(&[block(), block()]).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["choices"][0]["message"]["content"],
            "[paigasus] tool call \"read\" was blocked (policy).\n[paigasus] tool call \"rm\" was blocked (policy)."
        );
    }

    #[test]
    fn a_legacy_call_and_array_content_are_handled() {
        let body = openai(
            json!({"role":"assistant","content":[{"type":"text","text":"x"}],"function_call":{"name":"f","arguments":"SECRET"}}),
            "function_call",
        );
        let b = parse_body(Format::OpenAiChat, &body, MAX).unwrap();
        assert_eq!(b.calls[0].kind, ToolCallKind::LegacyFunctionCall);
        let v: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert!(v["choices"][0]["message"].get("function_call").is_none());
        assert_eq!(v["choices"][0]["message"]["content"][1]["type"], "text");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn a_short_decision_vector_blocks_the_rest() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let out = String::from_utf8(b.apply(&[Decision::Allow]).unwrap()).unwrap();
        assert!(!out.contains("SECRET") && out.contains("(decider-error)"));
    }

    #[test]
    fn malformed_openai_bodies_are_rejected() {
        for bad in [
            json!({"choices":[{"index":0,"delta":{}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":{"0":{}}}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"function","function":{"arguments":"{}"}}]}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"function","function":{"name":"a"}}],"function_call":{"name":"b"}}}]}),
            json!({"choices":[{"index":0,"message":{"content":1}}]}),
            json!({"choices":null}),
            json!({"choices":[{"message":{}}]}),
            json!({"choices":[{"index":0,"message":1}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":[{"function":{"name":"a"},"custom":{"name":"b"}}]}}]}),
        ] {
            assert!(parse_body(Format::OpenAiChat, bad.to_string().as_bytes(), MAX).is_err(), "{bad}");
        }
        assert!(parse_body(Format::OpenAiChat, br#"{"choices":[],"a":1,"a":2}"#, MAX).is_err());
        assert_eq!(parse_body(Format::OpenAiChat, &two_calls(), 10).err(), Some(Malformed("too_large")));
    }

    #[test]
    fn a_duplicate_choice_index_is_malformed() {
        let v = json!({"choices":[{"index":0,"message":{"content":"a"}},{"index":0,"message":{"content":"b"}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("duplicate_choice_index"));
    }

    #[test]
    fn choices_must_appear_in_index_order() {
        let v = json!({"choices":[{"index":1,"message":{"content":"a"}},{"index":0,"message":{"content":"b"}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("choice_order"));
    }

    #[test]
    fn a_tool_call_in_choice_one_is_a_deviation() {
        let call = json!([{"id":"c","type":"function","function":{"name":"rm","arguments":"{}"}}]);
        let v = json!({"choices":[{"index":0,"message":{"content":"a"}},{"index":1,"message":{"tool_calls":call}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("multi_choice_tool_calls"));
        let legacy = json!({"choices":[{"index":0,"message":{"content":"a"}},{"index":1,"message":{"function_call":{"name":"f","arguments":"{}"}}}]});
        assert_eq!(err_of(Format::OpenAiChat, &legacy), Some("multi_choice_tool_calls"));
        let text_only = json!({"choices":[{"index":0,"message":{"content":"a"}},{"index":1,"message":{"content":"b","tool_calls":[]}}]});
        assert!(parse_body(Format::OpenAiChat, text_only.to_string().as_bytes(), MAX).is_ok());
    }

    #[test]
    fn a_type_that_disagrees_with_the_body_is_a_mismatch() {
        let v = json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"custom","function":{"name":"a","arguments":"{}"}}]}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("type_shape_mismatch"));
        let v = json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"mystery","function":{"name":"a","arguments":"{}"}}]}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("type_shape_mismatch"));
        let v = json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"function"}]}}]});
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("missing_body"));
    }

    #[test]
    fn an_unknown_type_without_a_body_is_other() {
        let v = json!({"choices":[{"index":0,"message":{"tool_calls":[{"id":"x","type":"mystery","payload":1}]},"finish_reason":"tool_calls"}]});
        let b = parse_body(Format::OpenAiChat, v.to_string().as_bytes(), MAX).unwrap();
        assert_eq!((b.calls[0].kind, b.calls[0].name.as_str()), (ToolCallKind::Other, "mystery"));
        assert!(b.calls[0].arguments.contains("\"payload\":1"));
        let out: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn a_blocked_call_in_a_later_choice_position_leaves_other_choices_alone() {
        let v = json!({"choices":[
            {"index":0,"message":{"content":"a","tool_calls":[{"id":"c","type":"function","function":{"name":"rm","arguments":"{}"}}]},"finish_reason":"tool_calls"},
            {"index":1,"message":{"content":"b"},"finish_reason":"stop","logprobs":null}]});
        let b = parse_body(Format::OpenAiChat, v.to_string().as_bytes(), MAX).unwrap();
        let out: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert_eq!(out["choices"].as_array().unwrap().len(), 2);
        assert_eq!(out["choices"][1], v["choices"][1]);
    }

    fn anthropic(content: Value) -> Vec<u8> {
        json!({"id":"msg_1","type":"message","role":"assistant","model":"claude","content":content,"stop_reason":"tool_use"})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn an_anthropic_tool_use_is_extracted_and_blocked() {
        let body = anthropic(json!([{"type":"text","text":"Ok."},{"type":"tool_use","id":"t1","name":"rm","input":{"path":"SECRET"}}]));
        let b = parse_body(Format::AnthropicMessages, &body, MAX).unwrap();
        assert_eq!(
            (b.calls[0].position, b.calls[0].kind, b.calls[0].arguments.as_str()),
            (1, ToolCallKind::AnthropicToolUse, r#"{"path":"SECRET"}"#)
        );
        let v: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert_eq!(v["content"][1]["type"], "text");
        assert!(!v.to_string().contains("SECRET"));
        assert_eq!(v["stop_reason"], "end_turn");
    }

    #[test]
    fn an_allowed_tool_use_keeps_the_stop_reason() {
        let body = anthropic(json!([{"type":"tool_use","id":"a","name":"read","input":{}},{"type":"tool_use","id":"b","name":"rm","input":{}}]));
        let v: Value = serde_json::from_slice(&parse_body(Format::AnthropicMessages, &body, MAX).unwrap().apply(&[Decision::Allow, block()]).unwrap()).unwrap();
        assert_eq!(v["stop_reason"], "tool_use");
        assert_eq!(v["content"][0]["type"], "tool_use");
    }

    #[test]
    fn an_anthropic_other_block_is_held_like_the_stream_holds_it() {
        let body = anthropic(json!([{"type":"computer_call","id":"x","payload":1}]));
        let b = parse_body(Format::AnthropicMessages, &body, MAX).unwrap();
        assert_eq!((b.calls[0].kind, b.calls[0].name.as_str()), (ToolCallKind::Other, "computer_call"));
        assert_eq!(b.calls[0].arguments, r#"[{"type":"computer_call","id":"x","payload":1}]"#);
    }

    #[test]
    fn anthropic_server_tools_are_reported_not_decided() {
        let body = anthropic(json!([{"type":"server_tool_use","id":"s","name":"web_search","input":{}},{"type":"web_search_tool_result","tool_use_id":"s","content":[]}]));
        let b = parse_body(Format::AnthropicMessages, &body, MAX).unwrap();
        assert!(b.calls.is_empty());
        assert_eq!(b.unenforceable.len(), 2);
        assert!(b.report(&[], Basis::Decider).contains("unenforceable"));
    }

    #[test]
    fn malformed_anthropic_bodies_are_rejected() {
        for bad in [
            json!({"content":null}),
            json!({"content":[1]}),
            json!({"content":[{"text":"x"}]}),
            json!({"content":[{"type":"tool_use","id":"t","input":{}}]}),
            json!({"content":[{"type":"tool_use","id":"t","name":"a","input":"{}"}]}),
            json!({"content":[{"type":"tool_use","id":"t","name":"a"}]}),
            json!({"content":[{"type":"tool_use","id":1,"name":"a","input":{}}]}),
        ] {
            assert!(parse_body(Format::AnthropicMessages, bad.to_string().as_bytes(), MAX).is_err(), "{bad}");
        }
        assert_eq!(parse_body(Format::AnthropicMessages, &anthropic(json!([])), 5).err(), Some(Malformed("too_large")));
    }

    #[test]
    fn the_report_lists_each_decision() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let h: Vec<Value> = serde_json::from_str(&b.report(&[Decision::Allow, block()], Basis::Decider)).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0]["decision"].as_str(), h[1]["decision"].as_str(), h[1]["code"].as_str()),
            (Some("allow"), Some("block"), Some("policy"))
        );
    }
}
