// SPDX-License-Identifier: Apache-2.0
//! The Anthropic Messages streaming protocol (spec § 4.2–§ 4.4, D11).
//!
//! The SDK selects records by the `event:` name and decodes them by `data.type`. The gateway
//! dispatches on `data.type`, rejects a disagreeing `event:`, and always writes the event line
//! itself, so every client reads the record the gateway classified.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

use bytes::Bytes;
use serde_json::{Value, json};

use crate::json::{MAX_ANTHROPIC_INDEX, Malformed, index_of, opt_str, parse_strict};
use crate::protocol::{Deviation, Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::{ReportDecision, ReportEntry, notice_line};
use crate::sse::{self, Record};
use crate::types::{HoldKey, ToolCall, ToolCallKind};

const EVENTS: [&str; 8] = [
    "message_start",
    "message_delta",
    "message_stop",
    "ping",
    "error",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Thinking,
    Server,
    ToolUse,
    Other,
}

struct BlockState {
    kind: Kind,
    open: bool,
    id: Option<String>,
    name: String,
    args: String,
    raws: Vec<Value>,
    notice_done: bool,
}

impl BlockState {
    fn is_hold(&self) -> bool {
        matches!(self.kind, Kind::ToolUse | Kind::Other)
    }
}

#[derive(Default)]
pub(crate) struct AnthropicStream {
    blocks: Vec<BlockState>,
}

fn key(i: u32) -> HoldKey {
    HoldKey::Block(i)
}

fn block_index(v: &Value) -> Result<u32, Malformed> {
    index_of(v.get("index"), MAX_ANTHROPIC_INDEX)
}

fn plain(ev: &'static str, rec: Record, value: Value, touches: Vec<HoldKey>) -> Step {
    Step::Frame {
        frame: Frame {
            event: Some(ev),
            payload: rec.data,
            value: Some(value),
            touches,
        },
        opened: Vec::new(),
        completed: Vec::new(),
        reports: Vec::new(),
    }
}

fn text_delta(i: u32, text: &str) -> Bytes {
    let v = json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": text}});
    sse::frame(Some("content_block_delta"), v.to_string().as_bytes())
}

fn stop_frame(i: u32) -> Bytes {
    sse::frame(Some("content_block_stop"), json!({"type": "content_block_stop", "index": i}).to_string().as_bytes())
}

impl AnthropicStream {
    fn holds(&self) -> Vec<HoldKey> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_hold())
            .map(|(i, _)| key(u32::try_from(i).expect("index is bounded")))
            .collect()
    }

    fn open_block(&mut self, i: u32) -> Result<&mut BlockState, StepError> {
        self.blocks.get_mut(i as usize).filter(|b| b.open).ok_or(StepError::Deviation("unknown_block"))
    }

    fn call_of(b: &BlockState, i: u32) -> ToolCall {
        let (kind, arguments) = match b.kind {
            Kind::ToolUse => (ToolCallKind::AnthropicToolUse, b.args.clone()),
            _ => (ToolCallKind::Other, serde_json::to_string(&b.raws).expect("a Value array always serializes")),
        };
        ToolCall {
            choice: 0,
            position: i,
            id: b.id.clone(),
            name: b.name.clone(),
            kind,
            arguments,
        }
    }
}

impl Protocol for AnthropicStream {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError> {
        let value = parse_strict(&rec.data)?;
        let ty = value.get("type").and_then(Value::as_str).ok_or(Malformed("type"))?;
        let ev: &'static str = EVENTS.iter().copied().find(|e| *e == ty).ok_or(Malformed("unknown_event"))?;
        // The splitter yields an empty `event:` value as `Some("")`; that is an absent name, not a mismatch.
        if rec.event.as_deref().filter(|e| !e.is_empty()).is_some_and(|e| e != ev) {
            return Err(StepError::Malformed("event_mismatch"));
        }
        match ev {
            "ping" => Ok(plain(ev, rec, value, Vec::new())),
            "error" => Ok(Step::UpstreamError(Frame {
                event: Some(ev),
                payload: rec.data,
                value: Some(value),
                touches: Vec::new(),
            })),
            "message_stop" => Ok(Step::End(Frame {
                event: Some(ev),
                payload: rec.data,
                value: Some(value),
                touches: Vec::new(),
            })),
            "message_start" => {
                match value.pointer("/message/content") {
                    None => {}
                    Some(Value::Array(a)) if a.is_empty() => {}
                    Some(_) => return Err(Deviation("prefilled_content").into()),
                }
                Ok(plain(ev, rec, value, Vec::new()))
            }
            "message_delta" => {
                let touches = self.holds();
                Ok(plain(ev, rec, value, touches))
            }
            "content_block_start" => {
                let i = block_index(&value)?;
                if i as usize != self.blocks.len() {
                    return Err(Deviation("block_index").into());
                }
                let cb = value.get("content_block").and_then(Value::as_object).ok_or(Malformed("content_block"))?;
                let bty = cb.get("type").and_then(Value::as_str).ok_or(Malformed("block_type"))?;
                let kind = match bty {
                    "text" => Kind::Text,
                    "thinking" | "redacted_thinking" => Kind::Thinking,
                    "server_tool_use" | "mcp_tool_use" => Kind::Server,
                    t if t.ends_with("_tool_result") => Kind::Server,
                    "tool_use" => Kind::ToolUse,
                    _ => Kind::Other,
                };
                let mut b = BlockState {
                    kind,
                    open: true,
                    id: None,
                    name: String::new(),
                    args: String::new(),
                    raws: Vec::new(),
                    notice_done: false,
                };
                let (mut touches, mut opened, mut reports) = (Vec::new(), Vec::new(), Vec::new());
                match kind {
                    Kind::ToolUse => {
                        match cb.get("input") {
                            Some(Value::Object(m)) if m.is_empty() => {}
                            _ => return Err(Deviation("prefilled_input").into()),
                        }
                        b.id = opt_str(cb.get("id"), "id")?;
                        b.name = cb.get("name").and_then(Value::as_str).ok_or(Malformed("name"))?.to_owned();
                    }
                    Kind::Other => {
                        b.id = opt_str(cb.get("id"), "id")?;
                        b.name = cb.get("name").and_then(Value::as_str).unwrap_or(bty).to_owned();
                        b.raws.push(Value::Object(cb.clone()));
                    }
                    Kind::Server => {
                        let name = cb.get("name").and_then(Value::as_str).unwrap_or(bty).to_owned();
                        reports.push(ReportEntry {
                            choice: 0,
                            position: i,
                            name,
                            decision: ReportDecision::Unenforceable,
                            basis: None,
                            code: None,
                        });
                    }
                    Kind::Text | Kind::Thinking => {}
                }
                if b.is_hold() {
                    touches.push(key(i));
                    opened.push(key(i));
                }
                self.blocks.push(b);
                Ok(Step::Frame {
                    frame: Frame {
                        event: Some(ev),
                        payload: rec.data,
                        value: Some(value),
                        touches,
                    },
                    opened,
                    completed: Vec::new(),
                    reports,
                })
            }
            "content_block_delta" => {
                let i = block_index(&value)?;
                let d = value.get("delta").and_then(Value::as_object).ok_or(Malformed("delta"))?.clone();
                let dty = d.get("type").and_then(Value::as_str).ok_or(Malformed("delta_type"))?.to_owned();
                let decided = matches!(verdicts.get(&key(i)), Some(v) if *v != Verdict::PassThrough);
                let b = self.open_block(i)?;
                let mut touches = Vec::new();
                match (b.kind, dty.as_str()) {
                    (Kind::ToolUse, "input_json_delta") => {
                        let pj = d.get("partial_json").and_then(Value::as_str).ok_or(StepError::Deviation("partial_json"))?;
                        if !decided {
                            b.args.push_str(pj);
                        }
                        touches.push(key(i));
                    }
                    (Kind::ToolUse, _) => return Err(StepError::Deviation("tool_delta_type")),
                    (Kind::Other, _) => {
                        if !decided {
                            b.raws.push(Value::Object(d));
                        }
                        touches.push(key(i));
                    }
                    (Kind::Text, "text_delta" | "citations_delta") | (Kind::Thinking, "thinking_delta" | "signature_delta") | (Kind::Server, "input_json_delta") => {}
                    _ => return Err(StepError::Malformed("delta_type")),
                }
                Ok(plain(ev, rec, value, touches))
            }
            // "content_block_stop"
            _ => {
                let i = block_index(&value)?;
                let decided = matches!(verdicts.get(&key(i)), Some(v) if *v != Verdict::PassThrough);
                let b = self.open_block(i)?;
                b.open = false;
                let (mut touches, mut completed) = (Vec::new(), Vec::new());
                if b.is_hold() {
                    touches.push(key(i));
                    if !decided {
                        completed.push((key(i), vec![Self::call_of(b, i)]));
                    }
                }
                Ok(Step::Frame {
                    frame: Frame {
                        event: Some(ev),
                        payload: rec.data,
                        value: Some(value),
                        touches,
                    },
                    opened: Vec::new(),
                    completed,
                    reports: Vec::new(),
                })
            }
        }
    }

    fn incomplete(&self) -> Vec<HoldKey> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_hold() && b.open)
            .map(|(i, _)| key(u32::try_from(i).expect("index is bounded")))
            .collect()
    }

    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes> {
        let ev = frame.event.unwrap_or("ping");
        let original = || sse::frame(Some(ev), &frame.payload);
        let Some(value) = frame.value.as_ref() else { return vec![original()] };
        let code_of = |i: u32| verdicts.get(&key(i)).and_then(|v| v.block_code(0));
        match ev {
            "content_block_start" | "content_block_delta" | "content_block_stop" => {
                let Ok(i) = block_index(value) else { return vec![original()] };
                let Some(code) = code_of(i) else { return vec![original()] };
                match ev {
                    "content_block_start" => vec![sse::frame(
                        Some(ev),
                        json!({"type": ev, "index": i, "content_block": {"type": "text", "text": ""}}).to_string().as_bytes(),
                    )],
                    "content_block_delta" => Vec::new(),
                    _ => {
                        let Some(b) = self.blocks.get_mut(i as usize) else { return vec![original()] };
                        b.notice_done = true;
                        vec![text_delta(i, &notice_line(&b.name, code)), original()]
                    }
                }
            }
            "message_delta" => {
                let any_blocked = self.holds().iter().any(|k| verdicts.get(k).is_some_and(Verdict::blocks_any));
                let any_allowed = self.holds().iter().any(|k| verdicts.get(k).is_none_or(|v| !v.blocks_any()));
                if any_blocked && !any_allowed && value.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("tool_use") {
                    let mut v = value.clone();
                    v["delta"]["stop_reason"] = "end_turn".into();
                    return vec![sse::frame(Some(ev), v.to_string().as_bytes())];
                }
                vec![original()]
            }
            "message_stop" => {
                let mut out = self.closing(verdicts);
                out.push(original());
                out
            }
            _ => vec![original()],
        }
    }

    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes> {
        let mut out = Vec::new();
        for (i, b) in self.blocks.iter_mut().enumerate() {
            let i = u32::try_from(i).expect("index is bounded");
            let code = verdicts.get(&key(i)).and_then(|v| v.block_code(0));
            if let Some(code) = code
                && b.is_hold()
                && b.open
                && !b.notice_done
            {
                b.notice_done = true;
                out.push(text_delta(i, &notice_line(&b.name, code)));
                out.push(stop_frame(i));
            }
        }
        out
    }

    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)> {
        match hold {
            HoldKey::Block(i) => self.blocks.get(i as usize).map(|b| vec![(i, b.name.clone())]).unwrap_or_default(),
            HoldKey::Choice(_) => Vec::new(),
        }
    }

    fn unframed(&self, rec: Record) -> Bytes {
        sse::frame(rec.event.as_deref(), &rec.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Protocol, Step, Verdict, Verdicts};
    use crate::report::ReportDecision;
    use crate::sse::Record;
    use crate::types::{BlockCode, Decision, HoldKey, ToolCallKind};
    use serde_json::{Value, json};

    fn rec(v: Value) -> Record {
        let ty = v["type"].as_str().unwrap().to_owned();
        Record {
            event: Some(ty),
            data: v.to_string().into_bytes(),
        }
    }

    fn tool_stream() -> Vec<Value> {
        vec![
            json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude","stop_reason":null}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me look."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"rm","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"SECRET\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
    }

    fn run(events: Vec<Value>, verdict: Verdict) -> (String, Vec<crate::types::ToolCall>, Vec<ReportEntry>) {
        let mut p = AnthropicStream::default();
        let mut verdicts = Verdicts::new();
        let (mut frames, mut calls, mut reps) = (Vec::new(), Vec::new(), Vec::new());
        for e in events {
            match p.step(rec(e), &verdicts).ok().expect("canonical event") {
                Step::Frame { frame, completed, reports, .. } => {
                    for (k, c) in completed {
                        verdicts.insert(k, verdict.clone());
                        calls.extend(c);
                    }
                    reps.extend(reports);
                    frames.push(frame);
                }
                Step::End(f) | Step::UpstreamError(f) => frames.push(f),
            }
        }
        let mut out = Vec::new();
        for f in frames {
            out.extend(p.release(f, &verdicts).concat());
        }
        (String::from_utf8(out).unwrap(), calls, reps)
    }

    #[test]
    fn an_allowed_tool_use_is_reemitted_with_event_lines() {
        let events = tool_stream();
        let (out, calls, _) = run(events.clone(), Verdict::AllowAll);
        let want: String = events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect();
        assert_eq!(out, want);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].kind, calls[0].position, calls[0].name.as_str()), (ToolCallKind::AnthropicToolUse, 1, "rm"));
        assert_eq!(calls[0].arguments, "{\"path\":\"SECRET\"}");
        assert_eq!(calls[0].id.as_deref(), Some("toolu_1"));
    }

    #[test]
    fn a_blocked_tool_use_becomes_a_text_block_at_the_same_index() {
        let (out, _, _) = run(tool_stream(), Verdict::PerCall(vec![Decision::Block { code: BlockCode::Policy }]));
        assert!(!out.contains("SECRET"), "{out}");
        assert!(!out.contains("\"type\":\"tool_use\""), "{out}");
        assert!(out.contains(r#"{"content_block":{"text":"","type":"text"},"index":1,"type":"content_block_start"}"#), "{out}");
        assert!(out.contains(r#"[paigasus] tool call \"rm\" was blocked (policy)."#));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        // The notice delta comes before the block's stop.
        let notice = out.find("[paigasus]").unwrap();
        let stop1 = out.find(r#"{"index":1,"type":"content_block_stop"}"#).unwrap();
        assert!(notice < stop1);
        for frame in out.split("\n\n").filter(|f| !f.is_empty()) {
            let mut lines = frame.lines();
            let ev = lines.next().unwrap().strip_prefix("event: ").expect("every frame has an event line");
            let data: Value = serde_json::from_str(lines.next().unwrap().strip_prefix("data: ").unwrap()).unwrap();
            assert_eq!(data["type"], ev);
        }
    }

    #[test]
    fn server_tools_pass_and_are_reported_unenforceable() {
        let events = vec![
            json!({"type":"message_start","message":{"content":[]}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"s1","name":"web_search","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"s1","content":[]}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_stop"}),
        ];
        let (out, calls, reps) = run(events, Verdict::BlockAll(BlockCode::Policy));
        assert!(calls.is_empty());
        assert_eq!(reps.len(), 2);
        assert!(reps.iter().all(|r| r.decision == ReportDecision::Unenforceable));
        assert!(out.contains("web_search"));
    }

    #[test]
    fn an_unknown_block_type_is_held_as_other() {
        let events = vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"computer_use_v9","id":"x","name":"click"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"coords_delta","x":1}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        let (_, calls, _) = run(events, Verdict::AllowAll);
        assert_eq!((calls[0].kind, calls[0].name.as_str()), (ToolCallKind::Other, "click"));
        let raw: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(raw[1]["x"], 1);
    }

    #[test]
    fn every_non_canonical_event_is_rejected() {
        let mut p = AnthropicStream::default();
        let v = Verdicts::new();
        let bad = |p: &mut AnthropicStream, r: Record| p.step(r, &v).is_err();
        assert!(
            bad(
                &mut p,
                Record {
                    event: Some("ping".into()),
                    data: br#"{"type":"message_stop"}"#.to_vec()
                }
            ),
            "event mismatch"
        );
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"new_event"}))), "unknown type");
        assert!(
            bad(&mut AnthropicStream::default(), rec(json!({"type":"message_start","message":{"content":[{"type":"tool_use"}]}}))),
            "prefilled content"
        );
        assert!(
            bad(
                &mut AnthropicStream::default(),
                rec(json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}))
            ),
            "index gap"
        );
        assert!(
            bad(
                &mut AnthropicStream::default(),
                rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{"p":"/"}}}))
            ),
            "prefilled input"
        );
        assert!(
            bad(
                &mut AnthropicStream::default(),
                rec(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))
            ),
            "delta for an unknown block"
        );
        let mut q = AnthropicStream::default();
        q.step(
            rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{}}})),
            &v,
        )
        .ok()
        .unwrap();
        assert!(
            bad(&mut q, rec(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))),
            "a text delta on a tool block"
        );
    }

    #[test]
    fn a_block_without_a_stop_is_incomplete_and_closing_finishes_it() {
        let mut p = AnthropicStream::default();
        let v = Verdicts::new();
        let Step::Frame { opened, .. } = p
            .step(
                rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{}}})),
                &v,
            )
            .ok()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(opened, vec![HoldKey::Block(0)]);
        assert_eq!(p.incomplete(), vec![HoldKey::Block(0)]);
        let mut verdicts = Verdicts::new();
        verdicts.insert(HoldKey::Block(0), Verdict::BlockAll(BlockCode::Truncated));
        let closing = String::from_utf8(p.closing(&verdicts).concat()).unwrap();
        assert!(closing.contains("(truncated)") && closing.contains("content_block_stop"), "{closing}");
    }

    #[test]
    fn an_empty_event_name_is_treated_as_absent_and_never_re_emitted() {
        let mut p = AnthropicStream::default();
        let v = Verdicts::new();
        let r = Record {
            event: Some(String::new()),
            data: br#"{"type":"ping"}"#.to_vec(),
        };
        let Step::Frame { frame, .. } = p.step(r, &v).ok().expect("an empty event name is absent") else {
            panic!()
        };
        let out = String::from_utf8(p.release(frame, &v).concat()).unwrap();
        assert_eq!(out, "event: ping\ndata: {\"type\":\"ping\"}\n\n");
    }
}
