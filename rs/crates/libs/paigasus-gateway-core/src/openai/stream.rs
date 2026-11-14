// SPDX-License-Identifier: Apache-2.0
//! The OpenAI Chat Completions streaming protocol (spec § 4.2–§ 4.4, D10, D12, D20).
//!
//! Three shapes are a deviation only inside a call of choice 0: while a call is open, and in the
//! record that opens one ([`in_call`]). A raising client drops the stream there and holds a strict
//! prefix of the call, while the decider would still hold all of it.
//!
//! - A chunk without a `choices` key (`chunk_without_choices`, spec § 2.2 P-a): openai-python
//!   and openai-node read `chunk.choices`.
//! - A choice entry without a `delta` key, in any position (`choice_without_delta`, P-b):
//!   openai-python reads `choice.delta`, and LangChain raises `KeyError: 'delta'`. The entry of
//!   choice 0 with a `finish_reason` may omit `delta`.
//! - An array in a delta outside the `tool_calls` array itself (`array_outside_tool_calls`, spec
//!   § 5.7): under any other delta key, or under any key of a tool-call entry, its `function` and
//!   `custom` objects included, at any depth. openai-python's `accumulate_delta` raises when a
//!   later list does not merge into an earlier one (`{"foo":[{"index":0}]}`, then `{"foo":[1]}`).
//!
//! `"choices":[]` is keyed, so the final usage chunk is conforming. The record that opens a call
//! counts: a raise later in that record cuts the call after its first fragment (spec § 5.2).

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{Map, Value, json};

use crate::json::parse_strict;
use crate::openai::assembly::{self, Check, ChoiceAssembly};
use crate::openai::classify::{self, ChoiceView, Chunk};
use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::notice_line;
use crate::sse::{self, Record};
use crate::types::HoldKey;

/// The upstream fields a notice chunk copies, in upstream order. `object` is not copied: a notice
/// chunk always says `chat.completion.chunk`, or openai-python would skip it.
const META_KEYS: [&str; 4] = ["id", "created", "model", "system_fingerprint"];

#[derive(Default)]
struct ChoiceState {
    asm: ChoiceAssembly,
    hold_open: bool,
    finished: bool,
    has_text: bool,
    notice_done: bool,
    /// The (position, name) of the calls handed to the decider, in hold order. Never their
    /// arguments: a finished choice keeps none.
    calls: Vec<(u32, String)>,
}

pub(crate) struct OpenAiStream {
    choices: BTreeMap<u32, ChoiceState>,
    meta: Map<String, Value>,
    /// The most an observed choice may retain (D13, § 5.3).
    cap: usize,
}

impl Default for OpenAiStream {
    fn default() -> Self {
        Self::with_cap(crate::limits::Limits::defaults().max_held_bytes)
    }
}

impl OpenAiStream {
    pub(crate) fn with_cap(cap: usize) -> Self {
        OpenAiStream {
            choices: BTreeMap::new(),
            meta: Map::new(),
            cap,
        }
    }
}

/// The scalar (string or number) `META_KEYS` fields of a record. A non-scalar value is skipped.
fn meta_of(v: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    if let Value::Object(top) = v {
        for k in META_KEYS {
            if let Some(x @ (Value::String(_) | Value::Number(_))) = top.get(k) {
                m.insert(k.to_owned(), x.clone());
            }
        }
    }
    m
}

fn notice_chunk(meta: &Map<String, Value>, choice: u32, text: &str) -> Bytes {
    let mut m = Map::new();
    for (k, v) in meta {
        m.insert(k.clone(), v.clone());
        if k == "id" {
            m.insert("object".to_owned(), classify::CHUNK_OBJECT.into());
        }
    }
    m.entry("object").or_insert_with(|| classify::CHUNK_OBJECT.into());
    m.insert("choices".to_owned(), json!([{"index": choice, "delta": {"content": text}, "finish_reason": null}]));
    sse::frame(None, &serde_json::to_vec(&Value::Object(m)).expect("a Value always serializes"))
}

impl ChoiceState {
    /// The (position, name) of every call the hold knows.
    fn known(&self) -> Vec<(u32, String)> {
        if self.calls.is_empty() { self.asm.names() } else { self.calls.clone() }
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

/// Whether a client raise at this record can cut a call of choice 0: a call is open, or this record
/// opens one (it carries tool data for choice 0). Every raise-shape rule uses this one predicate.
fn in_call(open_call: bool, choices: &[ChoiceView]) -> bool {
    open_call || choices.iter().any(|c| c.index == 0 && (!c.fragments.is_empty() || c.legacy.is_some()))
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
        let (choices, keyed) = match classify::chunk(&value)? {
            Chunk::UpstreamError => {
                return Ok(Step::UpstreamError(Frame {
                    event: None,
                    payload: rec.data,
                    value: Some(value),
                    touches: Vec::new(),
                }));
            }
            Chunk::Choices { choices, keyed } => (choices, keyed),
        };
        // Phase 1 checks the whole record and changes nothing. A deviation in any choice drops the
        // record, so no choice may have been changed by then: a hold marked open that the
        // interceptor never registered would wait for a verdict for ever.
        let empty = ChoiceAssembly::default();
        // The shapes at which a client raises (see the module doc).
        let in_call = in_call(self.choices.get(&0).is_some_and(|s| s.hold_open && !s.finished), &choices);
        if in_call && !keyed {
            return Err(StepError::Deviation("chunk_without_choices"));
        }
        let mut planned: Vec<Option<Check>> = Vec::with_capacity(choices.len());
        let mut new_choices = 0;
        for (position, cv) in choices.iter().enumerate() {
            if in_call && !cv.has_delta && !(cv.index == 0 && cv.finish_reason.is_some()) {
                return Err(StepError::Deviation("choice_without_delta"));
            }
            if in_call && cv.stray_array {
                return Err(StepError::Deviation("array_outside_tool_calls"));
            }
            let key = HoldKey::Choice(cv.index);
            let st = self.choices.get(&cv.index);
            if st.is_none() {
                if cv.index as usize != self.choices.len() + new_choices {
                    return Err(StepError::Deviation("choice_order"));
                }
                new_choices += 1;
            }
            let (finished, hold_open, asm) = st.map_or((false, false, &empty), |s| (s.finished, s.hold_open, &s.asm));
            let tools = !cv.fragments.is_empty() || cv.legacy.is_some();
            let undecided = !verdicts.contains_key(&key);
            let mut after = None;
            if tools {
                assembly::check_tool_choice(cv.index, position, &cv.fragments, cv.legacy.as_ref())?;
                if finished {
                    return Err(StepError::Deviation("late_fragment"));
                }
                if undecided {
                    after = Some(asm.check(&cv.fragments, cv.legacy.as_ref())?);
                }
            }
            if cv.finish_reason.is_some() {
                if finished {
                    return Err(StepError::Deviation("second_finish"));
                }
                if (hold_open || tools) && undecided {
                    asm.check_finish(&cv.fragments, cv.legacy.as_ref(), after.as_ref().unwrap_or(asm.state()))?;
                }
            }
            planned.push(after);
        }
        // Phase 2 applies the checked record. Only an accepted record updates the notice meta.
        for (k, v) in meta_of(&value) {
            self.meta.insert(k, v);
        }
        let (mut touches, mut opened, mut completed) = (Vec::new(), Vec::new(), Vec::new());
        for (cv, after) in choices.into_iter().zip(planned) {
            let key = HoldKey::Choice(cv.index);
            let st = self.choices.entry(cv.index).or_default();
            if cv.has_text {
                st.has_text = true;
            }
            if !cv.fragments.is_empty() || cv.legacy.is_some() {
                match (verdicts.get(&key), after) {
                    (None, Some(after)) => st.asm.commit(&cv.fragments, cv.legacy.as_ref(), after),
                    (Some(Verdict::PassThrough), _) => st.asm.observe(&cv.fragments, cv.legacy.as_ref(), self.cap),
                    _ => {}
                }
                if !st.hold_open {
                    st.hold_open = true;
                    opened.push(key);
                }
                touches.push(key);
            }
            if cv.finish_reason.is_some() {
                st.finished = true;
                if st.hold_open {
                    touches.push(key);
                    let finished = match verdicts.get(&key) {
                        // Phase 1 ran the same check on the same state, so this cannot fail.
                        None => Some(st.asm.finish(cv.index)?),
                        Some(Verdict::PassThrough) if !st.asm.overflowed() => st.asm.finish(cv.index).ok(),
                        Some(_) => None,
                    };
                    st.asm.drop_buffers();
                    if let Some(calls) = finished {
                        st.calls = calls.iter().map(|c| (c.position, c.name.clone())).collect();
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

    #[cfg(test)]
    fn retained(&self) -> usize {
        self.choices.values().map(|s| s.asm.retained()).sum()
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
                json!([{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"\"x\""}}]},"finish_reason":null}]),
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
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":"x","function_call":{"name":"f","arguments":"\"SECRET\""}},"finish_reason":null}])),
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

    /// The closing notice takes its metadata only from accepted records, and only scalars: a
    /// dropped deviating record or a non-scalar field cannot reach the client through it.
    #[test]
    fn the_closing_notice_meta_comes_only_from_accepted_scalars() {
        let first = chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"{}"}}]}}]));
        let mut deviating: Value = serde_json::from_str(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"again"}}]}}]))).unwrap();
        deviating["id"] = "chatcmpl-EVIL".into();
        let mut odd: Value = serde_json::from_str(&chunk(json!([{"index":0,"delta":{"content":"x"}}]))).unwrap();
        odd["model"] = json!({"leak":"SECRET"});
        odd.as_object_mut().unwrap().remove("id"); // so only the dropped record could change the id
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(rec(&first), &v).ok().unwrap();
        assert!(p.step(rec(&deviating.to_string()), &v).is_err(), "repeated_name");
        p.step(rec(&odd.to_string()), &v).ok().unwrap();
        let mut verdicts = Verdicts::new();
        verdicts.insert(HoldKey::Choice(0), Verdict::BlockAll(BlockCode::Truncated));
        let closing = String::from_utf8(p.closing(&verdicts).concat()).unwrap();
        assert!(closing.contains("\"id\":\"chatcmpl-1\""), "{closing}");
        assert!(!closing.contains("EVIL") && !closing.contains("SECRET"), "{closing}");
        assert!(closing.contains("\"object\":\"chat.completion.chunk\""), "{closing}");
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

    fn args_frag(a: &str) -> String {
        chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":a}}]}}]))
    }
    fn finish_rec() -> String {
        chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}]))
    }

    #[test]
    fn a_finished_choice_keeps_no_argument_buffers() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        let big = format!("{{\"a\":\"{}\"}}", "x".repeat(1000));
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"rm","arguments":big}}]}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        assert!(p.retained() >= 1000);
        let Step::Frame { completed, .. } = p.step(rec(&finish_rec()), &v).ok().unwrap() else { panic!() };
        assert_eq!(completed[0].1[0].arguments, big);
        assert_eq!(p.retained(), 0, "a finished choice keeps no arguments");

        let mut q = OpenAiStream::default();
        q.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"shell","shell":{"cmd":"y".repeat(1000)}}]}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        assert!(q.retained() >= 1000);
        q.step(rec(&finish_rec()), &v).ok().unwrap();
        assert_eq!(q.retained(), 0, "a finished Other call keeps no raws");
    }

    #[test]
    fn an_observed_choice_stops_retaining_past_the_cap_and_is_not_decided() {
        let mut p = OpenAiStream::with_cap(256);
        let mut v = Verdicts::new();
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"rm","arguments":"{\"a\":\""}}]}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        v.insert(HoldKey::Choice(0), Verdict::PassThrough);
        for _ in 0..100 {
            p.step(rec(&args_frag(&"x".repeat(100))), &v).ok().unwrap();
            assert!(p.retained() <= 256 + 200, "retained {}", p.retained());
        }
        p.step(rec(&args_frag("\"}")), &v).ok().unwrap();
        let Step::Frame { completed, .. } = p.step(rec(&finish_rec()), &v).ok().unwrap() else { panic!() };
        assert!(completed.is_empty(), "an overflowed observed call is not decided");
        assert_eq!(p.retained(), 0);
    }

    #[test]
    fn a_short_observed_choice_is_still_decided() {
        let mut p = OpenAiStream::with_cap(256);
        let mut v = Verdicts::new();
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"rm","arguments":"{"}}]}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        v.insert(HoldKey::Choice(0), Verdict::PassThrough);
        p.step(rec(&args_frag("}")), &v).ok().unwrap();
        let Step::Frame { completed, .. } = p.step(rec(&finish_rec()), &v).ok().unwrap() else { panic!() };
        assert_eq!(completed[0].1[0].arguments, "{}");
    }

    #[test]
    fn an_other_call_with_a_lossy_number_is_a_deviation() {
        let v = Verdicts::new();
        let huge = r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"shell","shell":{"n":123456789012345678901234567890}}]}}]}"#;
        let mut p = OpenAiStream::default();
        p.step(rec(huge), &v).ok().unwrap();
        assert!(matches!(p.step(rec(&finish_rec()), &v), Err(StepError::Deviation("lossy_number"))));
        let same_record =
            r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"shell","shell":{"n":{"m":1e300}}}]},"finish_reason":"tool_calls"}]}"#;
        assert!(matches!(OpenAiStream::default().step(rec(same_record), &v), Err(StepError::Deviation("lossy_number"))));
        let ok = r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"shell","shell":{"n":12345678901234567891,"f":1.5}}]},"finish_reason":"tool_calls"}]}"#;
        assert!(OpenAiStream::default().step(rec(ok), &v).is_ok());
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

    /// LangChain reads `choices[0]` only (spec § 11, `choice_position`), so tool data for choice 0 behind another choice's
    /// entry would reach the decider but not that client.
    #[test]
    fn tool_data_behind_another_choice_entry_is_a_deviation() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"role":"assistant","content":""}},{"index":1,"delta":{"role":"assistant","content":""}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        let frag = json!({"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"rm","arguments":"{}"}}]});
        let behind = chunk(json!([{"index":1,"delta":{"content":"x"}},{"index":0,"delta":frag}]));
        assert!(matches!(p.step(rec(&behind), &v), Err(StepError::Deviation("choice_position"))));
        // Nothing of the rejected record stuck: the same fragment at position 0 still opens the hold.
        match p.step(rec(&chunk(json!([{"index":0,"delta":frag},{"index":1,"delta":{"content":"x"}}]))), &v).ok().unwrap() {
            Step::Frame { opened, .. } => assert_eq!(opened, vec![HoldKey::Choice(0)]),
            _ => panic!("a frame"),
        }
    }

    fn start_call(p: &mut OpenAiStream, v: &Verdicts) {
        let frag = json!({"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read","arguments":"{"}}]});
        p.step(rec(&chunk(json!([{"index":0,"delta":frag}]))), v).ok().unwrap();
    }

    #[test]
    fn a_chunk_without_choices_inside_an_open_call_deviates() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        start_call(&mut p, &v);
        let r = p.step(rec(r#"{"object":"chat.completion.chunk","id":"c"}"#), &v);
        assert!(matches!(r, Err(StepError::Deviation("chunk_without_choices"))));
    }

    #[test]
    fn a_choice_without_delta_inside_an_open_call_deviates() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        start_call(&mut p, &v);
        let r = p.step(rec(&chunk(json!([{"index":0}]))), &v);
        assert!(matches!(r, Err(StepError::Deviation("choice_without_delta"))));
    }

    #[test]
    fn a_choice_without_delta_at_position_1_also_deviates() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        start_call(&mut p, &v);
        let r = p.step(rec(&chunk(json!([{"index":0,"delta":{}},{"index":1}]))), &v);
        assert!(matches!(r, Err(StepError::Deviation("choice_without_delta"))));
    }

    /// Review Focus 1: a bare finish record is what a real stream ends with.
    #[test]
    fn a_finish_record_without_delta_is_conforming() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        start_call(&mut p, &v);
        p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}}]))), &v)
            .ok()
            .unwrap();
        match p.step(rec(&chunk(json!([{"index":0,"finish_reason":"tool_calls"}]))), &v).ok().unwrap() {
            Step::Frame { completed, .. } => assert_eq!(completed.len(), 1),
            _ => panic!("a frame"),
        }
    }

    /// Review Focus 3: with no open call the two shapes change no call.
    #[test]
    fn no_choices_or_no_delta_with_no_open_call_is_conforming() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        assert!(p.step(rec(r#"{"object":"chat.completion.chunk","id":"c"}"#), &v).is_ok());
        assert!(p.step(rec(&chunk(json!([{"index":0}]))), &v).is_ok());
        start_call(&mut p, &v);
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]},"finish_reason":"tool_calls"}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        let usage = json!({"object":"chat.completion.chunk","id":"c","choices":[],"usage":{"total_tokens":1}}).to_string();
        assert!(p.step(rec(&usage), &v).is_ok());
    }

    fn dev(r: Result<Step, StepError>) -> Option<&'static str> {
        match r {
            Err(StepError::Deviation(d)) => Some(d),
            _ => None,
        }
    }

    /// Spec § 5.7: an array outside `tool_calls` inside an open call, at each place openai-python
    /// merges it, at any depth.
    #[test]
    fn an_array_outside_tool_calls_inside_an_open_call_deviates() {
        let v = Verdicts::new();
        for delta in [
            json!({"foo":[{"index":0}]}),
            json!({"foo":[1]}),
            json!({"foo":{"bar":[1]}}),
            json!({"refusal":[]}),
            json!({"tool_calls":[{"index":0,"function":{"arguments":""},"x":[{"index":0}]}]}),
            json!({"tool_calls":[{"index":0,"function":{"arguments":"","x":[1]}}]}),
            json!({"tool_calls":[{"index":0,"x":{"y":[]}}]}),
        ] {
            let mut p = OpenAiStream::default();
            start_call(&mut p, &v);
            assert_eq!(dev(p.step(rec(&chunk(json!([{"index":0,"delta":delta}]))), &v)), Some("array_outside_tool_calls"), "{delta}");
        }
        // In another choice's entry too: openai-python raises for the whole stream.
        let mut p = OpenAiStream::default();
        p.step(rec(&chunk(json!([{"index":0,"delta":{"content":""}},{"index":1,"delta":{"content":""}}]))), &v).ok().unwrap();
        start_call(&mut p, &v);
        assert_eq!(dev(p.step(rec(&chunk(json!([{"index":1,"delta":{"foo":[1]}}]))), &v)), Some("array_outside_tool_calls"));
    }

    #[test]
    fn an_array_in_a_custom_or_legacy_body_deviates() {
        let v = Verdicts::new();
        let mut p = OpenAiStream::default();
        p.step(
            rec(&chunk(
                json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"custom","custom":{"name":"n","input":"a"}}]}}]),
            )),
            &v,
        )
        .ok()
        .unwrap();
        let r = p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"custom":{"input":"b","x":[1]}}]}}]))), &v);
        assert_eq!(dev(r), Some("array_outside_tool_calls"));
        let mut p = OpenAiStream::default();
        p.step(rec(&chunk(json!([{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]))), &v).ok().unwrap();
        let r = p.step(rec(&chunk(json!([{"index":0,"delta":{"function_call":{"arguments":"}"},"x":[1]}}]))), &v);
        assert_eq!(dev(r), Some("array_outside_tool_calls"));
    }

    /// R-B: the record that opens a call counts (probe G1e): the raise in it cuts the call.
    #[test]
    fn an_array_in_the_record_that_opens_a_call_deviates() {
        let v = Verdicts::new();
        let mut p = OpenAiStream::default();
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"role":"assistant","content":"","foo":[{"index":0}]}}]))), &v).is_ok());
        let opening = json!({"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read","arguments":"{"}}],"foo":[1]});
        assert_eq!(dev(p.step(rec(&chunk(json!([{"index":0,"delta":opening}]))), &v)), Some("array_outside_tool_calls"));
        // Nothing of the rejected record stuck: no hold was opened.
        assert!(p.incomplete().is_empty());
    }

    #[test]
    fn an_array_with_no_call_or_in_choice_level_keys_is_conforming() {
        let v = Verdicts::new();
        let mut p = OpenAiStream::default();
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"role":"assistant","content":"","foo":[{"index":0}]}}]))), &v).is_ok());
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"foo":[1]}}]))), &v).is_ok());
        start_call(&mut p, &v);
        // `tool_calls` itself, an empty one, and choice-level `logprobs` arrays are not merged as a delta.
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[]},"logprobs":{"content":[{"token":"x"}]}}]))), &v).is_ok());
        assert!(p.step(rec(&args_frag("}")), &v).is_ok());
        assert!(p.step(rec(&finish_rec()), &v).is_ok());
        assert!(p.step(rec(&chunk(json!([{"index":0,"delta":{"foo":[1]}}]))), &v).is_ok(), "after the finish");
    }

    /// R-B for `choice_without_delta` (probe G3b): a delta-less entry behind the opening fragment.
    #[test]
    fn a_choice_without_delta_in_the_record_that_opens_a_call_deviates() {
        let v = Verdicts::new();
        let mut p = OpenAiStream::default();
        p.step(rec(&chunk(json!([{"index":0,"delta":{"role":"assistant","content":""}}]))), &v).ok().unwrap();
        let frag = json!({"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read","arguments":"{"}}]});
        assert_eq!(dev(p.step(rec(&chunk(json!([{"index":0,"delta":frag},{"index":1}]))), &v)), Some("choice_without_delta"));
        assert!(p.incomplete().is_empty(), "no hold was opened");
    }

    /// The `!finished` guard: after the finish no call is open, so both shapes are conforming.
    #[test]
    fn no_choices_or_no_delta_after_the_finish_is_conforming() {
        let v = Verdicts::new();
        let mut p = OpenAiStream::default();
        start_call(&mut p, &v);
        p.step(rec(&args_frag("}")), &v).ok().unwrap();
        p.step(rec(&finish_rec()), &v).ok().unwrap();
        assert!(p.step(rec(r#"{"object":"chat.completion.chunk","id":"c"}"#), &v).is_ok());
        assert!(p.step(rec(&chunk(json!([{"index":0}]))), &v).is_ok());
    }

    /// Spec § 5.2: `"delta": null` stays `Malformed("delta")` in every position (SMA-749 classifier).
    #[test]
    fn a_null_delta_inside_an_open_call_is_a_stream_failure() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        start_call(&mut p, &v);
        let r = p.step(rec(&chunk(json!([{"index":0,"delta":null}]))), &v);
        assert!(matches!(r, Err(StepError::Malformed)));
        // Through the interceptor, the non-empty queue makes it a stream failure under block.
        let chunks = vec![
            format!(
                "data: {}\n\n",
                chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read","arguments":"\"X1\""}}]}}]))
            ),
            format!("data: {}\n\n", chunk(json!([{"index":0,"delta":null}]))),
        ];
        let mut i = crate::interceptor::Interceptor::new(crate::Format::OpenAiChat, crate::Limits::defaults(), Bytes::from_static(b"data: [DONE]\n\n"), std::time::Instant::now());
        let t0 = std::time::Instant::now();
        let mut acts = Vec::new();
        for c in &chunks {
            acts.extend(i.push(c.as_bytes(), t0));
        }
        assert!(acts.contains(&crate::interceptor::Action::Count(crate::interceptor::Metric::OnFailure("malformed"))), "{acts:?}");
    }
}
