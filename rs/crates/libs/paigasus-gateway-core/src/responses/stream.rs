// SPDX-License-Identifier: Apache-2.0
//! The OpenAI Responses streaming protocol (spec D10–D13, § 6.3–§ 6.6).
//!
//! One hold per response (D10): it opens at the first client-run `output_item.added` and
//! completes at the terminal event, with every client-run call of the response. Every frame of a
//! client-run item, and the terminal frame of a response whose hold opened, touch
//! [`HoldKey::Response`]; no other frame touches anything. Once the hold opens, every later frame waits behind it in the interceptor's FIFO
//! (749 D12), the frames of provider-run items included.
//!
//! Output is re-framed with an `event: <data.type>` line (D11). A frame that holds no blocked item
//! keeps its payload bytes. A blocked item is rewritten frame by frame (§ 6.5): its `added` frame
//! becomes three events, its middle frames are dropped, and its `done` frame becomes the completed
//! message. When the item has no `done` frame, the `output_item.done` event is synthesized before
//! the next frame that ends the item: the terminal event, `[DONE]`, an upstream error record, EOF
//! ([`Protocol::closing`]) or the D13 frame ([`Protocol::closing_on_abort`]). In the terminal
//! event, the item at each blocked index is replaced with the completed message.

use std::collections::HashSet;

use bytes::Bytes;
use serde_json::Value;

use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::notice_line;
use crate::responses::assembly::Assembly;
use crate::responses::classify::{Event, Scope, classify, scope_of};
use crate::responses::rewrite::{added_events, completed_message, done_event};
use crate::sse::{self, Record};
use crate::types::{BlockCode, HoldKey};

/// The longest `event:` name written, in bytes (Review Focus 1).
const MAX_EVENT_BYTES: usize = 64;

/// The `output_item.done` that a blocked item still owes the client: its rewritten `added` frame
/// went out, its `done` frame did not.
struct Owed {
    index: u32,
    notice: String,
    /// The `sequence_number` of the item's `added` frame, repeated as the rewrite repeats it.
    seq: Option<Value>,
}

pub(crate) struct ResponsesStream {
    asm: Assembly,
    /// The hold was handed to the decider at the terminal event.
    completed: bool,
    owed: Option<Owed>,
    /// The blocked items whose `output_item.done` the client already has.
    closed: HashSet<u32>,
}

impl ResponsesStream {
    /// `cap` is the stream's `max_held_bytes` (749 D13).
    pub(crate) fn with_cap(cap: usize) -> Self {
        ResponsesStream {
            asm: Assembly::with_cap(cap),
            completed: false,
            owed: None,
            closed: HashSet::new(),
        }
    }

    /// The notice of the call in `slot`, by its qualified name (§ 6.6).
    fn notice(&self, slot: usize, code: BlockCode) -> String {
        let known = self.asm.known();
        let name = known.get(slot).map_or("unknown", |(_, n)| n.as_str());
        notice_line(name, code)
    }

    /// The synthesized `output_item.done` of the owed item, if any (§ 6.5).
    fn owed_done(&mut self) -> Vec<Bytes> {
        let Some(o) = self.owed.take() else { return Vec::new() };
        self.closed.insert(o.index);
        vec![event_frame(&done_event(o.index, &o.notice, o.seq.as_ref()))]
    }

    /// One frame of the client-run item at `index`, which the verdict blocks with `code`.
    fn rewrite_item(&mut self, value: &Value, index: u32, slot: usize, code: BlockCode) -> Vec<Bytes> {
        let seq = value.get("sequence_number");
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.added") => {
                let notice = self.notice(slot, code);
                let out = added_events(index, &notice, seq).iter().map(event_frame).collect();
                self.owed = Some(Owed { index, notice, seq: seq.cloned() });
                out
            }
            Some("response.output_item.done") if !self.closed.contains(&index) => {
                if self.owed.as_ref().is_some_and(|o| o.index == index) {
                    self.owed = None;
                }
                self.closed.insert(index);
                vec![event_frame(&done_event(index, &self.notice(slot, code), seq))]
            }
            _ => Vec::new(),
        }
    }

    /// The terminal event with every blocked index replaced by its completed message. `None` when
    /// nothing in it is blocked.
    fn rewrite_terminal(&self, value: &Value, verdict: &Verdict) -> Option<Value> {
        let mut v = value.clone();
        let output = v.get_mut("response").and_then(|r| r.get_mut("output")).and_then(Value::as_array_mut)?;
        let mut changed = false;
        for (slot, (index, name)) in self.asm.known().iter().enumerate() {
            let Some(code) = verdict.block_code(slot) else { continue };
            if let Some(item) = output.get_mut(*index as usize) {
                *item = completed_message(*index, &notice_line(name, code));
                changed = true;
            }
        }
        changed.then_some(v)
    }
}

/// D11 and Review Focus 1: an `event:` name only when it is `^[a-z0-9_.]{1,64}$`.
fn safe_event(t: &str) -> bool {
    (1..=MAX_EVENT_BYTES).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// The `event:` name of a payload: its `data.type`, when that is a safe name.
fn event_of(v: &Value) -> Option<&str> {
    v.get("type").and_then(Value::as_str).filter(|t| safe_event(t))
}

/// One gateway-built event, framed with its type.
fn event_frame(v: &Value) -> Bytes {
    sse::frame(event_of(v), &serde_json::to_vec(v).expect("a Value always serializes"))
}

impl Protocol for ResponsesStream {
    fn step(&mut self, rec: Record, _verdicts: &Verdicts) -> Result<Step, StepError> {
        // R3: after the terminal event only `[DONE]` may follow. Checked first, so a malformed
        // late record cannot become a failure either.
        if self.asm.ended() && rec.data != b"[DONE]" {
            return Err(StepError::AfterEnd);
        }
        let (ty, value) = match classify(&rec)? {
            Event::Done => {
                return Ok(Step::End(Frame {
                    event: None,
                    payload: rec.data,
                    value: None,
                    touches: Vec::new(),
                }));
            }
            Event::UpstreamError(value) => {
                return Ok(Step::UpstreamError(Frame {
                    event: None,
                    payload: rec.data,
                    value: Some(value),
                    touches: Vec::new(),
                }));
            }
            Event::Known { ty, value } => (ty, value),
        };
        let effect = self.asm.accept(ty, &value)?;
        // The terminal frame carries the hold's calls in `output`, once the hold has opened.
        let holds = effect.item.is_some() || (effect.terminal && self.asm.hold_open());
        let touches = if holds { vec![HoldKey::Response] } else { Vec::new() };
        let frame = Frame {
            event: Some(ty),
            payload: rec.data,
            value: Some(value),
            touches,
        };
        if effect.terminal {
            let mut completed = Vec::new();
            if self.asm.hold_open() && !self.asm.hold_cut() {
                self.completed = true;
                completed.push((HoldKey::Response, self.asm.hold_calls()));
            }
            return Ok(Step::Terminal { frame, completed });
        }
        Ok(Step::Frame {
            frame,
            opened: if effect.opened { vec![HoldKey::Response] } else { Vec::new() },
            completed: Vec::new(),
            reports: effect.report.into_iter().collect(),
        })
    }

    fn incomplete(&self) -> Vec<HoldKey> {
        if self.asm.hold_cut() && !self.completed { vec![HoldKey::Response] } else { Vec::new() }
    }

    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes> {
        let Some(value) = frame.value else {
            let mut out = self.owed_done();
            out.push(sse::frame(None, &frame.payload));
            return out;
        };
        let original = || sse::frame(event_of(&value), &frame.payload);
        let verdict = verdicts.get(&HoldKey::Response).filter(|v| v.blocks_any());
        if !frame.touches.contains(&HoldKey::Response) {
            // A keepalive or a response-level event may sit inside an item (R5 is about items);
            // any other frame here ends the open item: an upstream error record.
            let inside = frame.event.map(scope_of).is_some_and(|s| matches!(s, Scope::Keepalive | Scope::ResponseLevel));
            let mut out = if inside { Vec::new() } else { self.owed_done() };
            out.push(original());
            return out;
        }
        let Some(verdict) = verdict else { return vec![original()] };
        if frame.event.map(scope_of) == Some(Scope::Terminal) {
            let mut out = self.owed_done();
            out.push(match self.rewrite_terminal(&value, verdict) {
                Some(v) => event_frame(&v),
                None => original(),
            });
            return out;
        }
        let index = value.get("output_index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok());
        let blocked = index.and_then(|i| self.asm.slot_of(i).map(|s| (i, s))).and_then(|(i, s)| verdict.block_code(s).map(|c| (i, s, c)));
        match blocked {
            Some((i, slot, code)) => self.rewrite_item(&value, i, slot, code),
            None => vec![original()],
        }
    }

    fn closing(&mut self, _verdicts: &Verdicts) -> Vec<Bytes> {
        self.owed_done()
    }

    fn closing_on_abort(&mut self, _verdicts: &Verdicts) -> Vec<Bytes> {
        self.owed_done()
    }

    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)> {
        match hold {
            HoldKey::Response => self.asm.known(),
            HoldKey::Choice(_) | HoldKey::Block(_) => Vec::new(),
        }
    }

    /// Pass-through framing (P-4): the `event:` line names `data.type` when it is a safe name.
    fn unframed(&self, rec: Record) -> Bytes {
        let value: Option<Value> = serde_json::from_slice(&rec.data).ok();
        sse::frame(value.as_ref().and_then(event_of), &rec.data)
    }

    #[cfg(test)]
    fn retained(&self) -> usize {
        self.asm.retained()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::*;
    use crate::interceptor::{Action, Interceptor, Metric};
    use crate::limits::Limits;
    use crate::types::{Decision, Format, ToolCall};

    fn rec(v: &Value) -> Record {
        Record {
            event: v["type"].as_str().map(str::to_owned),
            data: v.to_string().into_bytes(),
        }
    }

    fn done_rec() -> Record {
        Record {
            event: None,
            data: b"[DONE]".to_vec(),
        }
    }

    fn lifecycle(t: &str, output: Value) -> Value {
        json!({"type": t, "response": {"id": "resp_1", "status": "in_progress", "output": output}})
    }

    fn terminal(t: &str, output: Vec<Value>) -> Value {
        json!({"type": t, "response": {"id": "resp_1", "status": "completed", "output": output, "usage": {"total_tokens": 9}}})
    }

    fn added(i: u32, item: &Value) -> Value {
        let mut a = item.clone();
        if a["type"] == "function_call" {
            a["arguments"] = json!("");
            a["status"] = json!("in_progress");
        }
        json!({"type": "response.output_item.added", "output_index": i, "item": a})
    }

    fn item_done(i: u32, item: &Value) -> Value {
        json!({"type": "response.output_item.done", "output_index": i, "item": item})
    }

    fn fc(id: &str, name: &str, args: &str, status: &str) -> Value {
        json!({"id": id, "type": "function_call", "status": status, "arguments": args, "call_id": format!("call_{id}"), "name": name})
    }

    /// One item: its events and its `done` item.
    type Item = (Vec<Value>, Value);

    fn function_item(i: u32, item: Value, deltas: &[&str]) -> Item {
        let id = item["id"].as_str().unwrap().to_owned();
        let mut evs = vec![added(i, &item)];
        for d in deltas {
            evs.push(json!({"type": "response.function_call_arguments.delta", "output_index": i, "item_id": id, "delta": d}));
        }
        evs.push(json!({"type": "response.function_call_arguments.done", "output_index": i, "item_id": id, "arguments": item["arguments"]}));
        evs.push(item_done(i, &item));
        (evs, item)
    }

    fn message_item(i: u32, id: &str, text: &str) -> Item {
        let part = json!({"type": "output_text", "text": text, "annotations": []});
        let done = json!({"type": "message", "id": id, "status": "completed", "role": "assistant", "content": [part]});
        let open = json!({"type": "message", "id": id, "status": "in_progress", "role": "assistant", "content": []});
        let evs = vec![
            json!({"type": "response.output_item.added", "output_index": i, "item": open}),
            json!({"type": "response.content_part.added", "output_index": i, "item_id": id, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}),
            json!({"type": "response.output_text.delta", "output_index": i, "item_id": id, "content_index": 0, "delta": text}),
            json!({"type": "response.output_text.done", "output_index": i, "item_id": id, "content_index": 0, "text": text}),
            json!({"type": "response.content_part.done", "output_index": i, "item_id": id, "content_index": 0, "part": part}),
            item_done(i, &done),
        ];
        (evs, done)
    }

    fn web_search_item(i: u32, id: &str) -> Item {
        let done = json!({"type": "web_search_call", "id": id, "status": "completed", "action": {"type": "search", "query": "q"}});
        let evs = vec![
            json!({"type": "response.output_item.added", "output_index": i, "item": {"type": "web_search_call", "id": id, "status": "in_progress"}}),
            json!({"type": "response.web_search_call.in_progress", "output_index": i, "item_id": id}),
            json!({"type": "response.web_search_call.completed", "output_index": i, "item_id": id}),
            item_done(i, &done),
        ];
        (evs, done)
    }

    /// `created`, `in_progress`, the items, the terminal event with their `done` items.
    fn stream_of(items: Vec<Item>, terminal_type: &str) -> Vec<Value> {
        let mut evs = vec![lifecycle("response.created", json!([])), lifecycle("response.in_progress", json!([]))];
        let mut output = Vec::new();
        for (e, d) in items {
            evs.extend(e);
            output.push(d);
        }
        evs.push(terminal(terminal_type, output));
        evs
    }

    /// Give every event the `sequence_number` 0, 1, 2, … (R7).
    fn numbered(evs: Vec<Value>) -> Vec<Value> {
        evs.into_iter()
            .enumerate()
            .map(|(i, mut e)| {
                e["sequence_number"] = json!(i);
                e
            })
            .collect()
    }

    fn framed(v: &Value) -> String {
        format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
    }

    /// Step every record, give each completed hold `verdict` and each cut hold `BlockAll(cut)`,
    /// release every frame, then close. Returns the output, the decided calls and the stream.
    fn run(records: &[Record], verdict: impl Fn(&[ToolCall]) -> Verdict, cut: BlockCode) -> (String, Vec<ToolCall>, ResponsesStream) {
        let mut p = ResponsesStream::with_cap(1 << 20);
        let mut verdicts = Verdicts::new();
        let mut frames = Vec::new();
        let mut calls = Vec::new();
        for r in records {
            match p.step(r.clone(), &verdicts).ok().expect("canonical record") {
                Step::Terminal { frame, completed } => {
                    for (k, c) in completed {
                        verdicts.insert(k, verdict(&c));
                        calls.extend(c);
                    }
                    for k in p.incomplete() {
                        verdicts.entry(k).or_insert(Verdict::BlockAll(cut));
                    }
                    frames.push(frame);
                }
                Step::Frame { frame, .. } | Step::End(frame) | Step::UpstreamError(frame) => frames.push(frame),
            }
        }
        for k in p.incomplete() {
            verdicts.entry(k).or_insert(Verdict::BlockAll(cut));
        }
        let mut out = Vec::new();
        for f in frames {
            out.extend(p.release(f, &verdicts));
        }
        out.extend(p.closing(&verdicts));
        (String::from_utf8(out.concat()).unwrap(), calls, p)
    }

    fn records(evs: &[Value]) -> Vec<Record> {
        evs.iter().map(rec).collect()
    }

    /// The `data:` payloads of an output, parsed. `[DONE]` is `Value::Null`.
    fn payloads(out: &str) -> Vec<Value> {
        out.split("\n\n")
            .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")))
            .map(|d| serde_json::from_str(d).unwrap_or(Value::Null))
            .collect()
    }

    fn types(out: &str) -> Vec<String> {
        payloads(out).iter().map(|v| v["type"].as_str().unwrap_or("[DONE]").to_owned()).collect()
    }

    fn block_all(_: &[ToolCall]) -> Verdict {
        Verdict::BlockAll(BlockCode::Policy)
    }

    fn two_items() -> Vec<Value> {
        numbered(stream_of(
            vec![
                message_item(0, "msg_1", "Sure."),
                function_item(1, fc("fc_1", "rm", "{\"p\":\"CANARY\"}", "completed"), &["{\"p\":", "\"CANARY\"}"]),
            ],
            "response.completed",
        ))
    }

    #[test]
    fn allow_all_reemits_every_payload_with_its_event_line() {
        let evs = two_items();
        let mut recs = records(&evs);
        recs.push(done_rec());
        let (out, calls, _) = run(&recs, |c| Verdict::PerCall(vec![Decision::Allow; c.len()]), BlockCode::Truncated);
        let want: String = evs.iter().map(framed).collect::<String>() + "data: [DONE]\n\n";
        assert_eq!(out, want);
        assert_eq!(calls.iter().map(|c| (c.position, c.name.as_str())).collect::<Vec<_>>(), [(1, "rm")]);
        let (out, _, _) = run(&recs, |_| Verdict::AllowAll, BlockCode::Truncated);
        assert_eq!(out, want);
    }

    #[test]
    fn a_blocked_function_call_becomes_a_message_at_its_index() {
        let evs = two_items();
        let (out, _, _) = run(&records(&evs), |_| Verdict::PerCall(vec![Decision::Block { code: BlockCode::Policy }]), BlockCode::Truncated);
        assert!(!out.contains("CANARY") && !out.contains("\\\"p\\\""), "the blocked arguments leaked: {out}");
        let notice = r#"[paigasus] tool call "rm" was blocked (policy)."#;
        let got = payloads(&out);
        // The message item passes unchanged; the call becomes the four events of § 6.5.
        let message_frames = 6;
        assert_eq!(got[..2 + message_frames], evs[..2 + message_frames]);
        let added_seq = &evs[2 + message_frames]["sequence_number"];
        let done_seq = &evs[evs.len() - 2]["sequence_number"];
        let [a, b, c] = added_events(1, notice, Some(added_seq));
        assert_eq!(got[2 + message_frames..2 + message_frames + 4], [a, b, c, done_event(1, notice, Some(done_seq))]);
        let term = got.last().unwrap();
        assert_eq!(term["type"], "response.completed");
        assert_eq!(term["response"]["output"][1], completed_message(1, notice));
        assert_eq!(term["response"]["output"][0], evs.last().unwrap()["response"]["output"][0], "only the blocked index changes");
        assert_eq!(term["response"]["usage"], json!({"total_tokens": 9}), "no other field of the terminal changes");
        assert_eq!(got.len(), 2 + message_frames + 4 + 1, "every middle frame of the call is dropped");
        // Each rewritten frame has its own event line.
        assert!(
            out.contains("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_paigasus_1\""),
            "{out}"
        );
    }

    /// A call with its `added` frame and one delta, and no `done`.
    fn cut_call() -> Vec<Value> {
        let item = fc("fc_1", "rm", "{\"p\":\"CANARY", "in_progress");
        let (mut evs, _) = function_item(0, item, &["{\"p\":\"CANARY"]);
        evs.truncate(2);
        let mut all = vec![lifecycle("response.created", json!([])), lifecycle("response.in_progress", json!([]))];
        all.extend(evs);
        numbered(all)
    }

    #[test]
    fn a_blocked_item_without_done_gets_a_synthesized_done() {
        let notice = r#"[paigasus] tool call "rm" was blocked (truncated)."#;
        let evs = cut_call();
        let seq = evs[2]["sequence_number"].clone();
        let want_done = done_event(0, notice, Some(&seq));
        // Before the terminal frame (release).
        let mut with_terminal = evs.clone();
        let mut open = with_terminal[2]["item"].clone();
        open["status"] = json!("in_progress");
        with_terminal.push(json!({"type": "response.incomplete", "sequence_number": 4, "response": {"id": "resp_1", "status": "incomplete", "output": [open]}}));
        let (out, calls, _) = run(&records(&with_terminal), block_all, BlockCode::Truncated);
        assert!(calls.is_empty(), "a cut hold is not decided");
        assert_eq!(
            types(&out)[2..],
            [
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_item.done",
                "response.incomplete"
            ]
        );
        let got = payloads(&out);
        assert_eq!(got[5], want_done);
        assert_eq!(got[6]["response"]["output"][0], completed_message(0, notice));
        assert!(!out.contains("CANARY"), "{out}");
        // Before EOF (`closing`).
        let (out, _, mut p) = run(&records(&evs), block_all, BlockCode::Truncated);
        assert_eq!(payloads(&out).last().unwrap(), &want_done);
        assert!(p.closing(&Verdicts::new()).is_empty(), "the done is owed once");
        // Before `[DONE]`.
        let mut with_marker = records(&evs);
        with_marker.push(done_rec());
        let (out, _, _) = run(&with_marker, block_all, BlockCode::Truncated);
        assert!(out.ends_with(&format!("{}data: [DONE]\n\n", framed(&want_done))), "{out}");
        // Before the D13 frame (`closing_on_abort`).
        let mut p = ResponsesStream::with_cap(1 << 20);
        let mut verdicts = Verdicts::new();
        let mut frames = Vec::new();
        for r in records(&evs) {
            if let Ok(Step::Frame { frame, .. }) = p.step(r, &verdicts) {
                frames.push(frame);
            }
        }
        verdicts.insert(HoldKey::Response, Verdict::BlockAll(BlockCode::Malformed));
        for f in frames {
            p.release(f, &verdicts);
        }
        let owed = String::from_utf8(p.closing_on_abort(&verdicts).concat()).unwrap();
        assert_eq!(owed, framed(&done_event(0, &notice.replace("truncated", "malformed"), Some(&seq))));
        assert!(p.closing_on_abort(&verdicts).is_empty());
    }

    /// D17 through the interceptor: a failed stream blocks the open call, and the abort closes the
    /// rewritten message before the D13 frame.
    #[test]
    fn an_abort_closes_a_rewritten_item_before_the_terminal_frame() {
        let evs = cut_call();
        let mut i = interceptor();
        let t0 = Instant::now();
        let mut acts = i.push(&sse(&evs), t0);
        acts.extend(i.push(b"data: {\"a\":1,\"a\":2}\n\n", t0));
        assert!(acts.contains(&Action::Count(Metric::OnFailure("malformed"))), "{acts:?}");
        let out = emitted(&acts);
        assert!(out.contains("msg_paigasus_0") && !out.contains("CANARY"), "{out}");
        let notice = r#"[paigasus] tool call "rm" was blocked (malformed)."#;
        let done = framed(&done_event(0, notice, Some(&evs[2]["sequence_number"])));
        assert_eq!(i.abort(), vec![Action::Emit(Bytes::from(done)), Action::Emit(Bytes::from_static(b"TERMINAL"))]);
    }

    #[test]
    fn the_rewrite_repeats_the_sequence_number() {
        let evs = two_items();
        let (out, _, _) = run(&records(&evs), block_all, BlockCode::Truncated);
        let got = payloads(&out);
        let added_seq = &evs[8]["sequence_number"];
        for v in &got[8..11] {
            assert_eq!(&v["sequence_number"], added_seq, "{v}");
        }
        let plain: Vec<Value> = evs
            .iter()
            .map(|e| {
                let mut e = e.clone();
                e.as_object_mut().unwrap().remove("sequence_number");
                e
            })
            .collect();
        let (out, _, _) = run(&records(&plain), block_all, BlockCode::Truncated);
        assert!(out.contains("msg_paigasus_1"), "{out}");
        assert!(!out.contains("sequence_number"), "{out}");
    }

    fn interceptor() -> Interceptor {
        Interceptor::new(Format::OpenAiResponses, Limits::defaults(), Bytes::from_static(b"TERMINAL"), Instant::now())
    }

    fn sse(evs: &[Value]) -> Vec<u8> {
        evs.iter().map(framed).collect::<String>().into_bytes()
    }

    fn emitted(acts: &[Action]) -> String {
        acts.iter()
            .filter_map(|a| if let Action::Emit(b) = a { Some(String::from_utf8_lossy(b).into_owned()) } else { None })
            .collect()
    }

    #[test]
    fn a_truncated_item_blocks_the_whole_hold() {
        let first = function_item(0, fc("fc_1", "read", "{}", "completed"), &["{}"]);
        let second = function_item(1, fc("fc_2", "rm", "{\"p\":\"CANA", "incomplete"), &["{\"p\":\"CANA"]);
        let evs = numbered(stream_of(vec![first, second], "response.incomplete"));
        let mut i = interceptor();
        let t0 = Instant::now();
        let mut acts = i.push(&sse(&evs), t0);
        acts.extend(i.finish(t0));
        assert!(!acts.iter().any(|a| matches!(a, Action::Decide { .. })), "the decider is not asked: {acts:?}");
        assert!(acts.contains(&Action::Count(Metric::OnFailure("truncated"))));
        assert!(i.is_finished());
        let out = emitted(&acts);
        assert!(!out.contains("CANA"), "{out}");
        for (pos, name) in [(0, "read"), (1, "rm")] {
            assert!(out.contains(&format!(r#""position":{pos},"name":"{name}""#)), "{out}");
            assert!(out.contains(&format!(r#"tool call \"{name}\" was blocked (truncated)."#)), "{out}");
            assert!(out.contains(&format!("msg_paigasus_{pos}")), "{out}");
        }
        assert_eq!(out.matches(r#""code":"truncated""#).count(), 2, "{out}");
    }

    #[test]
    fn frames_after_the_terminal_are_dropped_and_done_is_emitted() {
        let evs = numbered(stream_of(vec![message_item(0, "msg_1", "hi")], "response.completed"));
        let mut p = ResponsesStream::with_cap(1 << 20);
        let v = Verdicts::new();
        for e in &evs {
            p.step(rec(e), &v).ok().expect("canonical");
        }
        let late = json!({"type": "response.output_text.delta", "output_index": 0, "item_id": "msg_1", "delta": "LATE", "sequence_number": 99});
        assert!(matches!(p.step(rec(&late), &v), Err(StepError::AfterEnd)));
        assert!(matches!(p.step(rec(&evs[0]), &v), Err(StepError::AfterEnd)), "a second created or terminal too (F8)");
        assert!(matches!(p.step(rec(evs.last().unwrap()), &v), Err(StepError::AfterEnd)));
        let broken = Record {
            event: None,
            data: b"{\"a\":1,\"a\":2}".to_vec(),
        };
        assert!(matches!(p.step(broken, &v), Err(StepError::AfterEnd)), "even a malformed one");
        assert!(matches!(p.step(done_rec(), &v), Ok(Step::End(_))));
        // Through the interceptor: dropped and counted, with no failure, and `[DONE]` emitted.
        let mut bytes = sse(&evs);
        bytes.extend(framed(&late).as_bytes());
        bytes.extend(b"data: [DONE]\n\n");
        let mut i = interceptor();
        let t0 = Instant::now();
        let mut acts = i.push(&bytes, t0);
        acts.extend(i.finish(t0));
        let out = emitted(&acts);
        assert!(!out.contains("LATE") && out.ends_with("data: [DONE]\n\n"), "{out}");
        assert!(acts.contains(&Action::Count(Metric::RecordDropped("after_done"))));
        assert!(!acts.iter().any(|a| matches!(a, Action::Count(Metric::OnFailure(_)))), "{acts:?}");
    }

    #[test]
    fn an_oversized_record_after_the_terminal_is_dropped() {
        let evs = numbered(stream_of(vec![function_item(0, fc("fc_1", "rm", "\"X1\"", "completed"), &["\"X1\""])], "response.completed"));
        let limits = Limits {
            max_record_bytes: 4096,
            ..Limits::defaults()
        };
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiResponses, limits, Bytes::from_static(b"TERMINAL"), t0);
        let mut acts = i.push(&sse(&evs), t0);
        acts.extend(i.push(format!("data: {}\n\n", "x".repeat(5000)).as_bytes(), t0));
        assert!(i.is_deciding(HoldKey::Response), "the hold is still the decider's");
        assert!(acts.contains(&Action::Count(Metric::RecordDropped("after_done"))), "{acts:?}");
        assert!(!acts.contains(&Action::Count(Metric::OnFailure("malformed"))), "{acts:?}");
        let out = emitted(&i.decided(HoldKey::Response, Ok(vec![Decision::Block { code: BlockCode::Policy }]), t0));
        assert!(out.contains("(policy)") && !out.contains("X1") && !out.contains("(malformed)"), "{out}");
    }

    #[test]
    fn provider_frames_wait_behind_an_open_hold() {
        let evs = numbered(stream_of(
            vec![function_item(0, fc("fc_1", "read", "{}", "completed"), &["{}"]), web_search_item(1, "ws_1")],
            "response.completed",
        ));
        let mut i = interceptor();
        let t0 = Instant::now();
        let before = emitted(&i.push(&sse(&evs), t0));
        assert!(!before.contains("\"type\":\"web_search_call\""), "the provider item waits behind the hold: {before}");
        assert!(
            before.contains(r#""position":1,"name":"web_search_call""#) && before.contains(r#""decision":"unenforceable""#),
            "{before}"
        );
        let after = emitted(&i.decided(HoldKey::Response, Ok(vec![Decision::Allow]), t0));
        let call = after
            .find("event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0")
            .expect("call released");
        let search = after.find("response.web_search_call.in_progress").expect("search released");
        let end = after.find("event: response.completed").expect("terminal released");
        assert!(call < search && search < end, "{after}");
    }

    #[test]
    fn unframed_writes_a_safe_event_line() {
        let p = ResponsesStream::with_cap(1 << 20);
        let raw = |s: &str| Record {
            event: None,
            data: s.as_bytes().to_vec(),
        };
        assert_eq!(
            &p.unframed(raw(r#"{"type":"response.created"}"#))[..],
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n"
        );
        let long = format!(r#"{{"type":"{}"}}"#, "a".repeat(65));
        let exact = format!(r#"{{"type":"{}"}}"#, "a".repeat(64));
        for bad in [r#"{"type":"a\nb"}"#, r#"{"type":"A"}"#, r#"{"type":""}"#, r#"{"type":1}"#, long.as_str(), "not json", "[DONE]"] {
            let out = p.unframed(raw(bad));
            assert!(out.starts_with(b"data: "), "{bad}: {}", String::from_utf8_lossy(&out));
        }
        assert!(p.unframed(raw(&exact)).starts_with(b"event: aaaa"), "64 bytes is a safe name");
    }

    #[test]
    fn known_calls_name_the_qualified_names() {
        let ns = json!({"id": "fc_1", "type": "function_call", "status": "completed", "arguments": "{}", "call_id": "c1", "name": "f", "namespace": "ns"});
        let mcp = json!({"id": "mcpr_1", "type": "mcp_approval_request", "name": "rm", "server_label": "srv", "arguments": "{}"});
        let evs = numbered(stream_of(
            vec![function_item(0, ns, &["{}"]), (vec![added(1, &mcp), item_done(1, &mcp)], mcp.clone())],
            "response.completed",
        ));
        let mut p = ResponsesStream::with_cap(1 << 20);
        let v = Verdicts::new();
        p.step(rec(&evs[0]), &v).ok().unwrap();
        p.step(rec(&evs[1]), &v).ok().unwrap();
        p.step(rec(&evs[2]), &v).ok().unwrap();
        assert_eq!(p.known_calls(HoldKey::Response), [(0, "ns.f".to_owned())], "named at added");
        for e in &evs[3..evs.len() - 1] {
            p.step(rec(e), &v).ok().unwrap();
        }
        assert_eq!(p.known_calls(HoldKey::Response), [(0, "ns.f".to_owned()), (1, "srv.rm".to_owned())]);
        assert!(p.known_calls(HoldKey::Choice(0)).is_empty());
        let (out, _, _) = run(&records(&evs), block_all, BlockCode::Truncated);
        assert!(
            out.contains(r#"tool call \"ns.f\" was blocked (policy)."#) && out.contains(r#"tool call \"srv.rm\" was blocked (policy)."#),
            "{out}"
        );
    }

    /// Spec § 7.1 on the stream: application keys in the data subtrees of a lifecycle event pass
    /// unchanged.
    #[test]
    fn structured_output_keys_pass_on_the_stream() {
        let schema = r#"{"type":"object","properties":{"Name":{"type":"string"},"ID":{"type":"integer","maximum":1e300},"Type":{"type":"string"},"constructor":{"type":"string"}}}"#;
        let created = format!(
            r#"{{"type":"response.created","sequence_number":0,"response":{{"id":"resp_1","status":"in_progress","output":[],"metadata":{{"Id":"x"}},"text":{{"format":{{"type":"json_schema","name":"s","schema":{schema}}}}}}}}}"#
        );
        let mut i = interceptor();
        let t0 = Instant::now();
        let out = emitted(&i.push(format!("event: response.created\ndata: {created}\n\n").as_bytes(), t0));
        assert_eq!(out, format!("event: response.created\ndata: {created}\n\n"));
    }

    /// D23 on real traffic: every recorded stream passes through the interceptor under allow-all
    /// with each record byte-identical. The three `max_output_tokens_in_call` streams end with a cut
    /// call, which is blocked `Truncated` and rewritten (spec § 15 A3, ruling F6).
    #[test]
    fn recorded_streams_pass_through_the_interceptor() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/responses/recorded");
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "sse"))
            .collect();
        names.sort();
        assert_eq!(names.len(), 28);
        let limits = Limits {
            max_record_bytes: 16_777_216,
            max_held_bytes: 33_554_432,
            ..Limits::defaults()
        };
        for path in names {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path).unwrap();
            let t0 = Instant::now();
            let mut i = Interceptor::new(Format::OpenAiResponses, limits, Bytes::from_static(b"TERMINAL"), t0);
            let mut pending = i.push(&bytes, t0);
            pending.extend(i.finish(t0));
            let mut out = String::new();
            let mut decides = 0;
            while !pending.is_empty() {
                for a in std::mem::take(&mut pending) {
                    match a {
                        Action::Emit(b) => out.push_str(std::str::from_utf8(&b).unwrap()),
                        Action::Decide { hold, calls } => {
                            decides += 1;
                            assert!(calls.iter().all(|c| c.kind != crate::types::ToolCallKind::Other), "{name}");
                            pending.extend(i.decided(hold, Ok(vec![Decision::Allow; calls.len()]), t0));
                        }
                        Action::Count(m) => assert!(!matches!(m, Metric::RecordDropped(_)), "{name}: {m:?}"),
                        Action::Wake(_) => {}
                    }
                }
            }
            assert!(i.is_finished(), "{name}");
            let records: String = out.split_inclusive("\n\n").filter(|f| !f.starts_with(':')).collect();
            let input = String::from_utf8(bytes).unwrap();
            if name.contains("max_output_tokens_in_call") {
                assert_eq!(decides, 0, "{name}: a cut hold is not decided");
                assert!(out.contains(r#"tool call \"write_file\" was blocked (truncated)."#), "{name}: {out}");
                let got = payloads(&records);
                let want = payloads(&input);
                // The cut call is the last item; a `reasoning` item can come before it.
                let cut = want.iter().rposition(|v| v["type"] == "response.output_item.added").unwrap();
                let at = cut;
                let index = want[cut]["output_index"].as_u64().unwrap() as usize;
                assert_eq!(got[..at], want[..at], "{name}: the frames before the call are unchanged");
                assert_eq!(
                    types(&records)[at..],
                    [
                        "response.output_item.added",
                        "response.content_part.added",
                        "response.output_text.delta",
                        "response.output_item.done",
                        "response.incomplete"
                    ],
                    "{name}"
                );
                let term = got.last().unwrap();
                assert_eq!(term["response"]["output"][index]["id"], format!("msg_paigasus_{index}"), "{name}");
                assert_eq!(term["response"]["usage"], want.last().unwrap()["response"]["usage"], "{name}");
            } else {
                assert_eq!(records, input, "{name}: allow-all changes no record");
                assert_eq!(
                    decides,
                    usize::from(
                        ["function_call", "custom_tool_call", "computer_call", "apply_patch_call", "shell_call"]
                            .iter()
                            .any(|t| input.contains(&format!("\"type\":\"{t}\"")))
                    ),
                    "{name}"
                );
            }
        }
    }
}
