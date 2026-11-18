// SPDX-License-Identifier: Apache-2.0
//! Non-stream bodies (spec § 4.2, § 4.4, § 5.2). The same strict parse and the same canonical
//! rules as the streams, applied to `choices[].message` (OpenAI), `content[]` (Anthropic) or
//! `output[]` (Responses, SMA-769 § 6.5).
//!
//! A body that the streams would call a deviation is `Malformed` here: there is no earlier
//! frame to release, so the caller applies `on_failure` to the whole body.

use serde_json::Value;

use crate::decider::DecidedCall;
use crate::decision_log::{CallFacts, ReportedCall};
use crate::json::{Malformed, ResponsesDoc, check_numbers, check_numbers_responses, parse_strict, parse_strict_responses};
use crate::report::{ReportDecision, ReportEntry, RequestEntry, report_entry, report_header};
use crate::types::{Basis, BlockCode, Decision, Format, ToolCall};
use crate::{anthropic, openai, responses};

/// The tool calls of one complete body, and the means to rewrite it.
pub struct BodyCalls {
    format: Format,
    value: Value,
    /// The block code the body itself forces on each call (`Truncated` for a Responses item that
    /// is not `"completed"`, ruling F7). It wins over every decision. Empty for chat and Anthropic.
    forced: Vec<Option<BlockCode>>,
    pub calls: Vec<ToolCall>,
    pub unenforceable: Vec<ReportEntry>,
}

/// Parse a complete `chat.completion`, Messages or Responses body (strictly) and extract its tool
/// calls.
///
/// # Errors
/// `Malformed` when the body is over `max_bytes`, is not strict JSON, or is not in canonical form.
pub fn parse_body(format: Format, body: &[u8], max_bytes: usize) -> Result<BodyCalls, Malformed> {
    if body.len() > max_bytes {
        return Err(Malformed("too_large"));
    }
    let value = match format {
        // Spec § 7.1: the Responses key list and data subtrees. The numbers of the data subtrees
        // are the application's; `assembly::tool_call` checks those of each client-run item.
        Format::OpenAiResponses => {
            let value = parse_strict_responses(body, ResponsesDoc::Body)?;
            check_numbers_responses(&value, ResponsesDoc::Body)?;
            value
        }
        Format::OpenAiChat | Format::AnthropicMessages => {
            let value = parse_strict(body)?;
            // serde_json has no arbitrary_precision: an integer beyond u64/i64 becomes an f64 and
            // is rounded, so the decider would see a different number than a client that keeps
            // the exact int (I1). Rejecting every such number up front also makes the single
            // re-serialization in `apply` exact for every number that remains, so the rewritten
            // body cannot alter a value.
            check_numbers(&value)?;
            value
        }
    };
    let (calls, unenforceable) = match format {
        Format::OpenAiChat => (openai::body::calls(&value)?, Vec::new()),
        Format::AnthropicMessages => anthropic::body::calls(&value)?,
        Format::OpenAiResponses => responses::body::calls(&value)?,
    };
    let forced = match format {
        Format::OpenAiResponses => responses::body::forced(&value, &calls),
        Format::OpenAiChat | Format::AnthropicMessages => Vec::new(),
    };
    Ok(BodyCalls {
        format,
        value,
        forced,
        calls,
        unenforceable,
    })
}

impl BodyCalls {
    /// The block code of call `i`. A forced code wins (ruling F7). A decision vector of the wrong
    /// length blocks every call (D18).
    fn code(&self, decisions: &[Decision], i: usize) -> Option<BlockCode> {
        if let Some(code) = self.forced.get(i).copied().flatten() {
            return Some(code);
        }
        if decisions.len() != self.calls.len() {
            return Some(BlockCode::DeciderError);
        }
        match decisions[i] {
            Decision::Block { code } => Some(code),
            _ => None,
        }
    }

    /// The code an `Observe` decision for call `i` would have blocked with.
    fn observed(&self, decisions: &[Decision], i: usize) -> Option<BlockCode> {
        if decisions.len() != self.calls.len() {
            return None;
        }
        match decisions[i] {
            Decision::Observe { would } => Some(would),
            _ => None,
        }
    }

    /// Whether the body itself forces `Truncated` on a call (a Responses item that is not
    /// `"completed"`, ruling F7). The service reads it to apply `on_failure = allow` to such a
    /// body as the stream does to a truncation: the body goes through unchanged.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.forced.iter().any(Option::is_some)
    }

    /// The rewritten body, or `None` when nothing is blocked and the original bytes go through.
    #[must_use]
    pub fn apply(self, decisions: &[Decision]) -> Option<Vec<u8>> {
        let codes: Vec<Option<BlockCode>> = (0..self.calls.len()).map(|i| self.code(decisions, i)).collect();
        if codes.iter().all(Option::is_none) {
            return None;
        }
        let mut value = self.value;
        match self.format {
            Format::OpenAiChat => openai::body::rewrite(&mut value, &self.calls, &codes),
            Format::AnthropicMessages => anthropic::body::rewrite(&mut value, &self.calls, &codes),
            Format::OpenAiResponses => responses::body::rewrite(&mut value, &self.calls, &codes),
        }
        Some(serde_json::to_vec(&value).expect("a Value always serializes"))
    }

    /// The final report entries with what the report site knows about each (SMA-755): one per
    /// call in order, then the unenforceable entries with `call: None` (P-2). A decision vector of
    /// the wrong length has no detail, because the details do not belong to these calls.
    #[must_use]
    pub fn reported(&self, decided: &[DecidedCall], basis: Basis) -> Vec<ReportedCall> {
        let decisions: Vec<Decision> = decided.iter().map(|d| d.decision).collect();
        let aligned = decided.len() == self.calls.len();
        let mut out: Vec<ReportedCall> = self
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let code = self.code(&decisions, i);
                let decision = if code.is_some() { ReportDecision::Block } else { ReportDecision::Allow };
                let entry = report_entry(c, decision, basis, code.or_else(|| self.observed(&decisions, i)));
                let detail = if aligned { decided[i].detail.clone() } else { None };
                ReportedCall::new(entry, Some(CallFacts::of(c)), detail)
            })
            .collect();
        out.extend(self.unenforceable.iter().cloned().map(|e| ReportedCall::new(e, None, None)));
        out
    }

    /// The `paigasus-tool-decisions` header value (D22), including the unenforceable entries.
    /// It is built from the same entries as [`BodyCalls::reported`].
    #[must_use]
    pub fn report(&self, decided: &[DecidedCall], basis: Basis, requests: &[RequestEntry]) -> String {
        let entries: Vec<ReportEntry> = self.reported(decided, basis).into_iter().map(|r| r.entry).collect();
        report_header(&entries, requests)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::decision_log::{DecisionDetail, RecordBasis};
    use crate::report::{RequestAction, RequestReason};
    use crate::types::ToolCallKind;
    use serde_json::json;

    const MAX: usize = 1 << 20;
    fn plain(d: Vec<Decision>) -> Vec<DecidedCall> {
        d.into_iter().map(DecidedCall::from).collect()
    }
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
                {"id":"c1","type":"function","function":{"name":"rm","arguments":"\"SECRET\""}}
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
        assert_eq!(b.calls[1].arguments, "\"SECRET\"");
        assert_eq!(b.calls[1].id.as_deref(), Some("c1"));
    }

    #[test]
    fn observe_keeps_the_body_bytes() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        assert!(b.apply(&[Decision::Observe { would: BlockCode::Policy }, Decision::Allow]).is_none());
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let h: Vec<Value> = serde_json::from_str(&b.report(&plain(vec![Decision::Observe { would: BlockCode::Policy }, Decision::Allow]), Basis::Decider, &[])).unwrap();
        assert_eq!(
            (h[0]["decision"].as_str(), h[0]["code"].as_str(), h[0]["enforced"].as_bool()),
            (Some("allow"), Some("policy"), Some(false))
        );
        assert!(h[1]["enforced"].is_null());
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
            json!({"role":"assistant","content":"I will.","tool_calls":[{"id":"c0","type":"function","function":{"name":"rm","arguments":"\"SECRET\""}}]}),
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
            json!({"role":"assistant","content":[{"type":"text","text":"x"}],"function_call":{"name":"f","arguments":"\"SECRET\""}}),
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
        let v = json!({"object":"chat.completion","choices":[{"index":0,"message":{"tool_calls":[{"id":"x","type":"mystery","payload":1}]},"finish_reason":"tool_calls"}]});
        let b = parse_body(Format::OpenAiChat, v.to_string().as_bytes(), MAX).unwrap();
        assert_eq!((b.calls[0].kind, b.calls[0].name.as_str()), (ToolCallKind::Other, "mystery"));
        assert!(b.calls[0].arguments.contains("\"payload\":1"));
        let out: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn a_blocked_call_in_a_later_choice_position_leaves_other_choices_alone() {
        let v = json!({"object":"chat.completion","choices":[
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
        assert!(b.report(&[], Basis::Decider, &[]).contains("unenforceable"));
    }

    #[test]
    fn identifiers_over_1024_bytes_are_malformed_in_both_formats() {
        let (ok, long) = ("a".repeat(1024), "a".repeat(1025));
        let oa = |id: &str, name: &str| {
            openai(
                json!({"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":"{}"}}]}),
                "tool_calls",
            )
        };
        let an = |id: &str, name: &str| anthropic(json!([{"type":"tool_use","id":id,"name":name,"input":{}}]));
        let err = |f, b: Vec<u8>| parse_body(f, &b, 1 << 20).err().map(|e| e.0);
        assert_eq!(err(Format::OpenAiChat, oa(&ok, &ok)), None);
        assert_eq!(err(Format::AnthropicMessages, an(&ok, &ok)), None);
        assert_eq!(err(Format::OpenAiChat, oa(&long, "x")), Some("long_identifier"));
        assert_eq!(err(Format::OpenAiChat, oa("c", &long)), Some("long_identifier"));
        assert_eq!(err(Format::AnthropicMessages, an(&long, "x")), Some("long_identifier"));
        assert_eq!(err(Format::AnthropicMessages, an("t", &long)), Some("long_identifier"));
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
        let h: Vec<Value> = serde_json::from_str(&b.report(&plain(vec![Decision::Allow, block()]), Basis::Decider, &[])).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0]["decision"].as_str(), h[1]["decision"].as_str(), h[1]["code"].as_str()),
            (Some("allow"), Some("block"), Some("policy"))
        );
    }

    #[test]
    fn a_number_that_serde_would_round_is_malformed() {
        let huge = r#"{"content":[{"type":"tool_use","id":"t","name":"a","input":{"id":123456789012345678901234567890}}]}"#;
        assert_eq!(parse_body(Format::AnthropicMessages, huge.as_bytes(), MAX).err(), Some(Malformed("lossy_number")));
        let other = r#"{"content":[{"type":"computer_call","payload":[1e300]}]}"#;
        assert_eq!(parse_body(Format::AnthropicMessages, other.as_bytes(), MAX).err(), Some(Malformed("lossy_number")));
        let ok = r#"{"content":[{"type":"tool_use","id":"t","name":"a","input":{"x":1.5,"n":42,"big":9007199254740993,"u":12345678901234567891}}]}"#;
        let b = parse_body(Format::AnthropicMessages, ok.as_bytes(), MAX).unwrap();
        assert!(b.calls[0].arguments.contains("1.5") && b.calls[0].arguments.contains("42"));
    }

    #[test]
    fn a_wrong_length_decision_vector_blocks_every_call() {
        for decisions in [vec![Decision::Allow], vec![Decision::Allow, Decision::Allow, Decision::Allow]] {
            let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
            let h: Vec<Value> = serde_json::from_str(&b.report(&plain(decisions.clone()), Basis::OnFailure, &[])).unwrap();
            assert!(h.iter().all(|e| e["decision"] == "block" && e["code"] == "decider-error"), "{h:?}");
            let out = String::from_utf8(b.apply(&decisions).unwrap()).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            let text = v["choices"][0]["message"]["content"].as_str().unwrap();
            assert!(text.contains("\"read\" was blocked (decider-error)") && text.contains("\"rm\" was blocked (decider-error)"), "{text}");
            assert!(v["choices"][0]["message"].get("tool_calls").is_none());
        }
    }

    /// The `object` rule (spec § 11): a body that carries tool calls must say it is a completion. A text-only body is not
    /// checked, whatever its `finish_reason`.
    #[test]
    fn a_body_with_calls_or_a_finish_needs_the_completion_object() {
        let mut v: Value = serde_json::from_slice(&two_calls()).unwrap();
        v.as_object_mut().unwrap().remove("object");
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("object"));
        v["object"] = "chat.completion.chunk".into();
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("object"));
        let text = json!({"choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]});
        assert_eq!(err_of(Format::OpenAiChat, &text), None);
        let bare = json!({"choices":[{"index":0,"message":{"role":"assistant","content":"hi"}}]});
        assert_eq!(err_of(Format::OpenAiChat, &bare), None);
    }

    /// Argument data is the application's (I-1): PascalCase and `constructor`/`prototype` keys in
    /// OpenAI arguments or an Anthropic input are calls, not malformed bodies. A duplicate key or
    /// `__proto__` inside them is still rejected.
    #[test]
    fn argument_keys_are_application_data_in_bodies() {
        let call = |args: &str| openai(json!({"tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":args}}]}), "tool_calls");
        for args in [r#"{"id":1}"#, r#"{"Id":1}"#, r#"{"Name":"Alice"}"#, r#"{"prototype":"x"}"#, r#"{"Type":"pdf","constructor":1}"#] {
            let b = parse_body(Format::OpenAiChat, &call(args), MAX).unwrap_or_else(|e| panic!("{args}: {e}"));
            assert_eq!(b.calls[0].arguments, args);
        }
        for args in [r#"{"a":1,"a":2}"#, r#"{"__proto__":1}"#] {
            assert_eq!(parse_body(Format::OpenAiChat, &call(args), MAX).err().map(|e| e.0), Some("incomplete_arguments"), "{args}");
        }
        let anthropic =
            |input: &str| format!(r#"{{"id":"msg","type":"message","role":"assistant","content":[{{"type":"tool_use","id":"t","name":"f","input":{input}}}],"stop_reason":"tool_use"}}"#).into_bytes();
        for input in [r#"{"Name":"Alice","Id":1}"#, r#"{"constructor":{"Content":"x"},"prototype":2}"#] {
            let b = parse_body(Format::AnthropicMessages, &anthropic(input), MAX).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(b.calls[0].arguments, input);
        }
        for input in [r#"{"a":1,"a":2}"#, r#"{"__proto__":1}"#] {
            assert!(parse_body(Format::AnthropicMessages, &anthropic(input), MAX).is_err(), "{input}");
        }
    }

    /// Non-stream bodies follow the stream's argument rule through the same function.
    #[test]
    fn body_function_arguments_must_be_complete() {
        let call = |args: &str| openai(json!({"tool_calls":[{"id":"c","type":"function","function":{"name":"rm","arguments":args}}]}), "tool_calls");
        let v: Value = serde_json::from_slice(&call("{\"a\":")).unwrap();
        assert_eq!(err_of(Format::OpenAiChat, &v), Some("incomplete_arguments"));
        let b = parse_body(Format::OpenAiChat, &call(""), MAX).unwrap();
        assert_eq!(b.calls[0].arguments, "{}");
    }

    fn responses_body(output: Value) -> Vec<u8> {
        json!({"id":"resp_1","object":"response","status":"completed","output":output,"usage":{"total_tokens":3}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn a_responses_body_parses_and_rewrites_through_parse_body() {
        let body = responses_body(json!([
            {"type":"message","id":"msg_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"Sure.","annotations":[]}]},
            {"type":"function_call","id":"fc_1","call_id":"c1","name":"rm","namespace":"fs","arguments":"{\"p\":\"SECRET\"}","status":"completed"},
            {"type":"web_search_call","id":"ws_1","status":"completed"}
        ]));
        let b = parse_body(Format::OpenAiResponses, &body, MAX).unwrap();
        assert_eq!(b.calls.iter().map(|c| (c.position, c.display_name())).collect::<Vec<_>>(), [(1, "fs.rm".to_owned())]);
        assert_eq!(b.unenforceable.len(), 1);
        assert!(b.report(&plain(vec![block()]), Basis::Decider, &[]).contains(r#""name":"fs.rm""#));
        assert!(parse_body(Format::OpenAiResponses, &body, MAX).unwrap().apply(&[Decision::Allow]).is_none(), "allow keeps the bytes");
        let out = b.apply(&[block()]).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(!String::from_utf8(out).unwrap().contains("SECRET"));
        assert_eq!(v["output"][1]["id"], "msg_paigasus_1");
        assert_eq!(v["output"][1]["content"][0]["text"], "[paigasus] tool call \"fs.rm\" was blocked (policy).");
        assert_eq!(v["output"][0]["id"], "msg_1");
        assert_eq!(v["output"][2]["type"], "web_search_call");
        // Ruling F7: a call that is not completed is blocked `Truncated` over an allow, in the
        // rewrite and in the report.
        let cut = responses_body(json!([{"type":"function_call","id":"fc_1","call_id":"c1","name":"rm","arguments":"{\"p\":\"SEC","status":"incomplete"}]));
        let b = parse_body(Format::OpenAiResponses, &cut, MAX).unwrap();
        let report = b.report(&plain(vec![Decision::Allow]), Basis::Decider, &[]);
        assert!(report.contains(r#""decision":"block""#) && report.contains(r#""code":"truncated""#), "{report}");
        let v: Value = serde_json::from_slice(&b.apply(&[Decision::Allow]).unwrap()).unwrap();
        assert_eq!(v["output"][0]["content"][0]["text"], "[paigasus] tool call \"rm\" was blocked (truncated).");
        // The Responses profile, not the chat one: a `choices` body is malformed here.
        assert!(parse_body(Format::OpenAiResponses, &two_calls(), MAX).is_err());
    }

    #[test]
    fn reported_and_report_agree() {
        let cut = responses_body(json!([
            {"type":"function_call","id":"fc_1","call_id":"c1","name":"rm","arguments":"{\"p\":\"SEC","status":"incomplete"},
            {"type":"function_call","id":"fc_2","call_id":"c2","name":"ls","arguments":"{}","status":"completed"},
            {"type":"web_search_call","id":"ws_1","status":"completed"}
        ]));
        let b = parse_body(Format::OpenAiResponses, &cut, MAX).unwrap();
        assert!(b.is_truncated(), "the first call has a forced Truncated");
        let detail = Arc::new(DecisionDetail::new(RecordBasis::Policy, None, None));
        let decided = vec![DecidedCall::new(Decision::Allow, Some(Arc::clone(&detail))), DecidedCall::new(Decision::Allow, None)];
        let reported = b.reported(&decided, Basis::Decider);
        // The call entries in order, then the unenforceable entry without call facts.
        assert_eq!(reported.len(), 3);
        assert_eq!(reported[0].entry.code, Some(BlockCode::Truncated));
        assert_eq!(reported[0].entry.decision, ReportDecision::Block);
        assert_eq!(reported[0].call.as_ref().map(|c| (c.name.as_str(), c.id.as_deref())), Some(("rm", Some("c1"))));
        assert!(reported[0].detail.as_ref().is_some_and(|d| Arc::ptr_eq(d, &detail)), "the detail travels to the entry");
        assert_eq!(reported[1].entry.decision, ReportDecision::Allow);
        assert!(reported[1].call.is_some() && reported[1].detail.is_none());
        assert!(reported[2].call.is_none() && reported[2].detail.is_none(), "an unenforceable entry has no call facts");
        assert_eq!(reported[2].entry, b.unenforceable[0]);
        let entries: Vec<ReportEntry> = reported.into_iter().map(|r| r.entry).collect();
        for requests in [
            Vec::new(),
            vec![RequestEntry::new("t".to_owned(), RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied)],
        ] {
            assert_eq!(b.report(&decided, Basis::Decider, &requests), report_header(&entries, &requests));
        }
    }

    #[test]
    fn a_wrong_length_decision_vector_gives_no_detail() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let detail = Arc::new(DecisionDetail::new(RecordBasis::Policy, None, None));
        let reported = b.reported(&[DecidedCall::new(Decision::Allow, Some(detail))], Basis::OnFailure);
        assert_eq!(reported.len(), 2);
        assert!(reported.iter().all(|r| r.detail.is_none() && r.entry.code == Some(BlockCode::DeciderError)));
    }

    /// SMA-769 C5: `is_truncated` is true only when the body forces `Truncated` on a call.
    #[test]
    fn is_truncated_follows_the_forced_codes() {
        let done = responses_body(json!([{"type":"function_call","id":"fc_1","call_id":"c1","name":"rm","arguments":"{}","status":"completed"}]));
        assert!(!parse_body(Format::OpenAiResponses, &done, MAX).unwrap().is_truncated());
        let cut = responses_body(json!([{"type":"function_call","id":"fc_1","call_id":"c1","name":"rm","arguments":"{\"p\":\"SEC","status":"incomplete"}]));
        assert!(parse_body(Format::OpenAiResponses, &cut, MAX).unwrap().is_truncated());
        assert!(!parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap().is_truncated(), "chat forces nothing");
    }

    /// Ruling F11, spec § 7.1: the body number check covers the envelope and the client-run items
    /// only. A provider-run or tool-free item may carry a number that a float cannot hold.
    #[test]
    fn lossy_numbers_count_only_in_the_envelope_and_client_run_items() {
        let body = |item: &str| format!(r#"{{"id":"r","object":"response","status":"completed","output":[{item}]}}"#);
        let search = r#"{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","n":1e300}}"#;
        let text = r#"{"type":"message","id":"msg_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"x","annotations":[],"n":1e300}]}"#;
        for ok in [search, text] {
            assert!(parse_body(Format::OpenAiResponses, body(ok).as_bytes(), MAX).is_ok(), "{ok}");
        }
        let call = r#"{"id":"r","object":"response","status":"completed","output":[{"type":"function_call","id":"fc_1","call_id":"c1","name":"f","arguments":"{}","status":"completed","n":1e300}]}"#;
        assert_eq!(parse_body(Format::OpenAiResponses, call.as_bytes(), MAX).err().map(|e| e.0), Some("lossy_number"));
        let envelope = r#"{"id":"r","object":"response","status":"completed","n":1e300,"output":[]}"#;
        assert_eq!(parse_body(Format::OpenAiResponses, envelope.as_bytes(), MAX).err().map(|e| e.0), Some("lossy_number"));
    }

    /// Spec § 7.1: application keys and numbers in a data subtree pass on the body.
    #[test]
    fn structured_output_keys_pass_on_the_body() {
        let body = br#"{"id":"resp_1","object":"response","status":"completed","metadata":{"Id":"x"},"text":{"format":{"type":"json_schema","name":"s","schema":{"type":"object","properties":{"Name":{"type":"string"},"ID":{"type":"integer","maximum":1e300},"Type":{"type":"string"},"constructor":{"type":"string"}}}}},"output":[{"type":"message","id":"msg_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"{}","annotations":[]}]}]}"#;
        let b = parse_body(Format::OpenAiResponses, body, MAX).unwrap_or_else(|e| panic!("{e}"));
        assert!(b.calls.is_empty());
        assert!(b.apply(&[]).is_none());
    }
}
