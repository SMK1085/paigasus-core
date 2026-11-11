// SPDX-License-Identifier: Apache-2.0
//! The OpenAI Chat Completions streaming protocol (spec § 4.2–§ 4.4, D10, D12, D20).

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{Map, Value, json};

use crate::json::parse_strict;
use crate::openai::assembly::{self, ChoiceAssembly};
use crate::openai::classify::{self, Chunk};
use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::notice_line;
use crate::sse::{self, Record};
use crate::types::{HoldKey, ToolCall};

const META_KEYS: [&str; 5] = ["id", "object", "created", "model", "system_fingerprint"];

#[derive(Default)]
struct ChoiceState {
    asm: ChoiceAssembly,
    hold_open: bool,
    finished: bool,
    has_text: bool,
    notice_done: bool,
    /// The calls handed to the decider, in hold order.
    calls: Vec<ToolCall>,
}

#[derive(Default)]
pub(crate) struct OpenAiStream {
    choices: BTreeMap<u32, ChoiceState>,
    meta: Map<String, Value>,
}

fn meta_of(v: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    if let Value::Object(top) = v {
        for k in META_KEYS {
            if let Some(x) = top.get(k) {
                m.insert(k.to_owned(), x.clone());
            }
        }
    }
    m
}

fn notice_chunk(meta: &Map<String, Value>, choice: u32, text: &str) -> Bytes {
    let mut m = meta.clone();
    m.insert("choices".to_owned(), json!([{"index": choice, "delta": {"content": text}, "finish_reason": null}]));
    sse::frame(None, &serde_json::to_vec(&Value::Object(m)).expect("a Value always serializes"))
}

impl ChoiceState {
    /// The (position, name) of every call the hold knows.
    fn known(&self) -> Vec<(u32, String)> {
        if self.calls.is_empty() {
            self.asm.names()
        } else {
            self.calls.iter().map(|c| (c.position, c.name.clone())).collect()
        }
    }

    fn notice_text(&self, verdict: &Verdict) -> String {
        let lines: Vec<String> = self
            .known()
            .iter()
            .enumerate()
            .filter_map(|(slot, (_, name))| verdict.block_code(slot).map(|code| notice_line(name, code)))
            .collect();
        let body = if lines.is_empty() {
            notice_line("unknown", verdict.block_code(0).unwrap_or(crate::types::BlockCode::Malformed))
        } else {
            lines.join("\n")
        };
        if self.has_text { format!("\n\n{body}") } else { body }
    }

    fn allowed_left(&self, verdict: &Verdict) -> usize {
        (0..self.known().len()).filter(|slot| verdict.block_code(*slot).is_none()).count()
    }
}

impl Protocol for OpenAiStream {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError> {
        if rec.data == b"[DONE]" {
            return Ok(Step::End(Frame {
                event: None,
                payload: rec.data,
                value: None,
                touches: Vec::new(),
            }));
        }
        let value = parse_strict(&rec.data)?;
        let choices = match classify::chunk(&value)? {
            Chunk::UpstreamError => {
                return Ok(Step::UpstreamError(Frame {
                    event: None,
                    payload: rec.data,
                    value: Some(value),
                    touches: Vec::new(),
                }));
            }
            Chunk::Choices(c) => c,
        };
        for (k, v) in meta_of(&value) {
            self.meta.insert(k, v);
        }
        let (mut touches, mut opened, mut completed) = (Vec::new(), Vec::new(), Vec::new());
        for cv in choices {
            let key = HoldKey::Choice(cv.index);
            if !self.choices.contains_key(&cv.index) && cv.index as usize != self.choices.len() {
                return Err(StepError::Deviation("choice_order"));
            }
            let st = self.choices.entry(cv.index).or_default();
            if cv.has_text {
                st.has_text = true;
            }
            if !cv.fragments.is_empty() || cv.legacy.is_some() {
                assembly::check_tool_choice(cv.index, &cv.fragments, cv.legacy.as_ref())?;
                if st.finished {
                    return Err(StepError::Deviation("late_fragment"));
                }
                if !st.hold_open {
                    st.hold_open = true;
                    opened.push(key);
                }
                touches.push(key);
                match verdicts.get(&key) {
                    None => st.asm.apply(&cv.fragments, cv.legacy.as_ref())?,
                    Some(Verdict::PassThrough) => {
                        let _ = st.asm.apply(&cv.fragments, cv.legacy.as_ref());
                    }
                    Some(_) => {}
                }
            }
            if cv.finish_reason.is_some() {
                if st.finished {
                    return Err(StepError::Deviation("second_finish"));
                }
                st.finished = true;
                if st.hold_open {
                    touches.push(key);
                    let finished = match verdicts.get(&key) {
                        None => Some(st.asm.finish(cv.index)?),
                        Some(Verdict::PassThrough) => st.asm.finish(cv.index).ok(),
                        Some(_) => None,
                    };
                    if let Some(calls) = finished {
                        st.calls = calls.clone();
                        completed.push((key, calls));
                    }
                }
            }
        }
        touches.sort();
        touches.dedup();
        Ok(Step::Frame {
            frame: Frame {
                event: None,
                payload: rec.data,
                value: Some(value),
                touches,
            },
            opened,
            completed,
            reports: Vec::new(),
        })
    }

    fn incomplete(&self) -> Vec<HoldKey> {
        self.choices.iter().filter(|(_, s)| s.hold_open && !s.finished).map(|(c, _)| HoldKey::Choice(*c)).collect()
    }

    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes> {
        let Some(mut value) = frame.value else {
            let mut out = self.closing(verdicts);
            out.push(sse::frame(None, &frame.payload));
            return out;
        };
        let blocking: Vec<u32> = frame
            .touches
            .iter()
            .filter_map(|k| match (k, verdicts.get(k)) {
                (HoldKey::Choice(c), Some(v)) if v.blocks_any() => Some(*c),
                _ => None,
            })
            .collect();
        if blocking.is_empty() {
            return vec![sse::frame(None, &frame.payload)];
        }
        let meta = meta_of(&value);
        let mut notices = Vec::new();
        if let Some(Value::Array(choices)) = value.get_mut("choices") {
            for ch in choices.iter_mut() {
                let idx = ch.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
                if let Some(v) = verdicts.get(&HoldKey::Choice(idx)).filter(|_| blocking.contains(&idx)) {
                    let st = self.choices.entry(idx).or_default();
                    if let Some(text) = rewrite_choice(ch, v, st) {
                        notices.push(notice_chunk(&meta, idx, &text));
                    }
                }
            }
        }
        // P-10: never drop the frame or a choice entry. openai-python finds a choice by list
        // position, so a missing first appearance would shift every later choice.
        notices.push(sse::frame(None, &serde_json::to_vec(&value).expect("a Value always serializes")));
        notices
    }

    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes> {
        let mut out = Vec::new();
        for (idx, st) in &mut self.choices {
            let Some(v) = verdicts.get(&HoldKey::Choice(*idx)) else { continue };
            if st.hold_open && v.blocks_any() && !st.notice_done {
                st.notice_done = true;
                out.push(notice_chunk(&self.meta, *idx, &st.notice_text(v)));
            }
        }
        out
    }

    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)> {
        match hold {
            HoldKey::Choice(c) => self.choices.get(&c).map(ChoiceState::known).unwrap_or_default(),
            HoldKey::Block(_) => Vec::new(),
        }
    }

    fn unframed(&self, rec: Record) -> Bytes {
        sse::frame(None, &rec.data)
    }
}

/// Rewrite one choice entry in place (§ 4.4). Returns the notice text when this entry carries the
/// choice's `finish_reason` and the notice is still owed.
fn rewrite_choice(ch: &mut Value, verdict: &Verdict, st: &mut ChoiceState) -> Option<String> {
    let slots: Vec<u32> = st.known().iter().map(|(p, _)| *p).collect();
    let slot_of = |pos: u32| slots.iter().position(|p| *p == pos).unwrap_or(slots.len());
    let rank = |pos: u32| -> u32 { u32::try_from((0..slot_of(pos)).filter(|s| verdict.block_code(*s).is_none()).count()).expect("at most 128 calls per choice") };
    if let Some(Value::Object(delta)) = ch.get_mut("delta") {
        if let Some(Value::Array(tc)) = delta.get_mut("tool_calls") {
            tc.retain_mut(|e| {
                let pos = e.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
                if verdict.block_code(slot_of(pos)).is_some() {
                    return false;
                }
                e["index"] = rank(pos).into();
                true
            });
            if tc.is_empty() {
                delta.remove("tool_calls");
            }
        }
        if delta.contains_key("function_call") && verdict.block_code(0).is_some() {
            delta.remove("function_call");
        }
    }
    if let Value::Object(o) = ch {
        o.remove("logprobs");
    }
    let finish = ch.get("finish_reason").and_then(Value::as_str).map(str::to_owned)?;
    if st.allowed_left(verdict) == 0 && (finish == "tool_calls" || finish == "function_call") {
        ch["finish_reason"] = "stop".into();
    }
    if st.notice_done {
        return None;
    }
    st.notice_done = true;
    Some(st.notice_text(verdict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Protocol, Step, Verdict, Verdicts};
    use crate::sse::Record;
    use crate::types::{BlockCode, Decision, HoldKey};
    use serde_json::{Value, json};

    fn rec(s: &str) -> Record {
        Record {
            event: None,
            data: s.as_bytes().to_vec(),
        }
    }
    fn chunk(choices: Value) -> String {
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","system_fingerprint":"fp","choices":choices}).to_string()
    }

    /// Steps every record, then releases every frame under `verdict` for each completed hold.
    /// Returns the concatenated output and the completed calls.
    fn run(records: &[String], verdict: impl Fn(&[crate::types::ToolCall]) -> Verdict) -> (String, Vec<crate::types::ToolCall>) {
        let mut p = OpenAiStream::default();
        let mut verdicts = Verdicts::new();
        let mut frames = Vec::new();
        let mut calls = Vec::new();
        for r in records {
            match p.step(rec(r), &verdicts).ok().expect("canonical record") {
                Step::Frame { frame, completed, .. } => {
                    for (k, c) in completed {
                        verdicts.insert(k, verdict(&c));
                        calls.extend(c);
                    }
                    frames.push(frame);
                }
                Step::End(f) | Step::UpstreamError(f) => frames.push(f),
            }
        }
        let mut out = Vec::new();
        for f in frames {
            for b in p.release(f, &verdicts) {
                out.extend_from_slice(&b);
            }
        }
        for b in p.closing(&verdicts) {
            out.extend_from_slice(&b);
        }
        (String::from_utf8(out).unwrap(), calls)
    }

    fn two_calls() -> Vec<String> {
        vec![
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":"Sure."},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"read","arguments":"{}"}}]},"finish_reason":null}])),
            chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":1,"id":"c1","type":"function","function":{"name":"rm","arguments":"{\"p\":\"/\"}"}}]},"logprobs":{"content":[{"token":"rm"}]},"finish_reason":null}]),
            ),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}])),
            "[DONE]".to_owned(),
        ]
    }

    #[test]
    fn allow_all_reemits_every_payload_unchanged() {
        let records = two_calls();
        let (out, calls) = run(&records, |c| Verdict::PerCall(vec![Decision::Allow; c.len()]));
        let want: String = records.iter().map(|r| format!("data: {r}\n\n")).collect();
        assert_eq!(out, want);
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["read", "rm"]);
    }

    #[test]
    fn a_blocked_second_call_is_removed_and_its_logprobs_stripped() {
        let (out, _) = run(&two_calls(), |_| Verdict::PerCall(vec![Decision::Allow, Decision::Block { code: BlockCode::Policy }]));
        assert!(!out.contains("\\\"p\\\""), "the blocked arguments leaked: {out}");
        assert!(!out.contains("\"token\":\"rm\""), "the logprobs leaked: {out}");
        assert!(out.contains("\"name\":\"read\""));
        assert!(out.contains(r#"[paigasus] tool call \"rm\" was blocked (policy)."#));
        assert!(out.contains("\"finish_reason\":\"tool_calls\""), "one call remains, so finish_reason stays");
        let notice_at = out.find("[paigasus]").unwrap();
        let finish_at = out.find("\"finish_reason\":\"tool_calls\"").unwrap();
        assert!(notice_at < finish_at, "the notice precedes the finish frame");
        assert!(out.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn blocking_every_call_rewrites_finish_reason_to_stop_and_keeps_role() {
        let records = vec![
            chunk(
                json!([{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"x"}}]},"finish_reason":null}]),
            ),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}])),
        ];
        let (out, _) = run(&records, |_| Verdict::BlockAll(BlockCode::Policy));
        assert!(out.contains("\"role\":\"assistant\""), "role is kept: {out}");
        assert!(!out.contains("tool_calls\":["), "no tool_calls remain: {out}");
        assert!(out.contains("\"finish_reason\":\"stop\""));
        let notice: Value = serde_json::from_str(out.lines().find(|l| l.contains("[paigasus]")).unwrap().strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(notice["id"], "chatcmpl-1");
        assert_eq!(notice["system_fingerprint"], "fp");
        assert_eq!(notice["choices"][0]["index"], 0);
        assert!(!notice["choices"][0]["delta"]["content"].as_str().unwrap().starts_with('\n'), "no text streamed before, so no prefix");
    }

    #[test]
    fn a_legacy_call_is_blocked_and_finish_reason_function_call_becomes_stop() {
        let records = vec![
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":"x","function_call":{"name":"f","arguments":"SECRET"}},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"function_call"}])),
        ];
        let (out, _) = run(&records, |_| Verdict::BlockAll(BlockCode::Policy));
        assert!(!out.contains("SECRET"));
        assert!(out.contains("\"finish_reason\":\"stop\""));
        assert!(out.contains("\\n\\n[paigasus]"), "text streamed first, so the notice is prefixed");
    }

    #[test]
    fn closing_owes_a_notice_for_a_truncated_blocked_choice() {
        let records = [chunk(
            json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"SEC"}}]}}]),
        )];
        let mut p = OpenAiStream::default();
        let Step::Frame { frame, opened, .. } = p.step(rec(&records[0]), &Verdicts::new()).ok().unwrap() else {
            panic!()
        };
        assert_eq!(opened, vec![HoldKey::Choice(0)]);
        assert_eq!(p.incomplete(), vec![HoldKey::Choice(0)]);
        let mut verdicts = Verdicts::new();
        verdicts.insert(HoldKey::Choice(0), Verdict::BlockAll(BlockCode::Truncated));
        let out = String::from_utf8(p.release(frame, &verdicts).concat()).unwrap();
        assert!(!out.contains("SEC") && !out.contains("tool_calls"), "the blocked call is gone: {out}");
        assert!(out.contains("\"delta\":{}"), "the choice entry is kept, emptied (P-10): {out}");
        let closing = String::from_utf8(p.closing(&verdicts).concat()).unwrap();
        assert!(closing.contains("(truncated)"));
        assert!(p.closing(&verdicts).is_empty(), "the notice is owed once");
    }

    #[test]
    fn a_fragment_after_finish_reason_is_a_deviation() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(
            rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"x"}}]}}]))),
            &v,
        )
        .ok()
        .unwrap();
        p.step(rec(&chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}]))), &v).ok().unwrap();
        let late = p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"x"}}]}}]))), &v);
        assert!(matches!(late, Err(StepError::Deviation("late_fragment"))));
    }

    #[test]
    fn a_frame_touches_only_the_choices_that_carry_tool_data() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        let both = chunk(json!([
            {"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"x"}}]}},
            {"index":1,"delta":{"content":"text"}}
        ]));
        let Step::Frame { frame, .. } = p.step(rec(&both), &v).ok().unwrap() else { panic!() };
        assert_eq!(frame.touches, vec![HoldKey::Choice(0)]);
    }

    #[test]
    fn choices_must_first_appear_in_order() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        let first_is_one = p.step(rec(&chunk(json!([{"index":1,"delta":{"content":"x"}}]))), &v);
        assert!(matches!(first_is_one, Err(StepError::Deviation("choice_order"))));
        let mut q = OpenAiStream::default();
        assert!(q.step(rec(&chunk(json!([{"index":0,"delta":{"content":"a"}},{"index":1,"delta":{"content":"b"}}]))), &v).is_ok());
        assert!(
            q.step(rec(&chunk(json!([{"index":1,"delta":{"content":"c"}}]))), &v).is_ok(),
            "a seen choice may come in any order later"
        );
    }

    #[test]
    fn a_text_frame_of_a_holding_choice_touches_nothing() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(
            rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"x"}}]}}]))),
            &v,
        )
        .ok()
        .unwrap();
        let Step::Frame { frame, .. } = p.step(rec(&chunk(json!([{"index":0,"delta":{"content":"later"}}]))), &v).ok().unwrap() else {
            panic!()
        };
        assert!(frame.touches.is_empty());
    }

    #[test]
    fn a_tool_call_in_choice_one_is_a_deviation() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"content":"a"}},{"index":1,"delta":{"content":"b"}}]))), &v).is_ok());
        let r = p.step(
            rec(&chunk(json!([{"index":1,"delta":{"tool_calls":[{"index":0,"id":"b","type":"function","function":{"name":"y"}}]}}]))),
            &v,
        );
        assert!(matches!(r, Err(StepError::Deviation("multi_choice_tool_calls"))));
    }
}
