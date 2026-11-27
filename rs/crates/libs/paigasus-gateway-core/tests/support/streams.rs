// SPDX-License-Identifier: Apache-2.0
//! Grammar-based generators of hostile upstream streams (spec § 8.1).
//!
//! Hidden calls, the ones a correct interceptor must never let through, are named `blocked_*` and
//! carry `CANARY` in their arguments. Allowed calls never contain `CANARY`.

use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quirk {
    DuplicateDelta,
    CaseFoldedKey,
    ProtoKey,
    FloatIndex,
    BoolIndex,
    NegativeIndex,
    RepeatedName,
    Bom,
    LaxLine,
    UnknownFields,
    AfterDone,
    NoFinalBlank,
    Oversized,
    NaNRecord,
    JsonBody,
    EventLine,
    IndexGap,
    Comments,
    Interleave,
    EscapedKey,
    /// A tool-bearing chunk without `"object":"chat.completion.chunk"` (openai-python skips it).
    DropObject,
    /// Two choices' records merged into one chunk, the later choice first (LangChain reads
    /// `choices[0]` only).
    MultiChoiceChunk,
    /// One call of choice 0 gets `type: "web_search"` with its `function` body kept.
    UnknownType,
    /// One chunk names choice 0 twice.
    DuplicateChoiceIndex,
    /// The choices are rendered in reverse order, so choice 1 appears first (P-10).
    ReverseChoices,
    /// One function or legacy call of choice 0 loses the last character of its arguments, so they
    /// are not one complete JSON value (`incomplete_arguments`).
    TruncatedArgs,
    /// One function or legacy call of choice 0 gets two JSON objects as arguments (`{..}{"b":2}`).
    ConcatenatedObjects,
    /// Every call's arguments gain PascalCase, `constructor` and `prototype` keys. They are
    /// application data, so the stream is conforming (I-1).
    CapitalisedArgKeys,
    /// A chunk without a `choices` key (`chunk_without_choices` inside an open call).
    NoChoicesKey,
    /// A chunk whose choice 0 entry has no `delta` (`choice_without_delta` inside an open call).
    /// At an odd position, a delta-less `{"index":1}` is added behind the first record that opens
    /// a call instead (spec § 5.2, the opening record counts).
    NoDelta,
    /// A fragment with `"type":null` and a function body for call 0: a present `type` key counts
    /// as the type fragment, so it is a repeated type or a type/shape mismatch. When choice 0 has
    /// no call, a bare `{"index":0,"type":null}` goes after its first record instead: a conforming
    /// bodyless call that the core and the oracle both name `unknown`.
    NullType,
    /// A top-level `chunk` key with an empty `choices` (spec § 2.2 H1): LangChain reads
    /// `chunk.choices` then, so the hidden call sits inside `chunk`.
    ChunkKey,
    /// A fragment in a record with top-level `"type":"content.delta"` (spec § 2.2 H2): LangChain
    /// skips that record.
    ContentDeltaType,
    /// Two records with an array outside `tool_calls`, an indexed list and then a scalar list:
    /// openai-python's `accumulate_delta` raises at the second (spec § 5.7). The array sits under
    /// an unknown delta key, a tool-call entry key or a `function` key, chosen by the position.
    ArrayOutsideToolCalls,
    /// Two legacy `function_call` fragments with an extra key whose JSON type changes (`"x":"s"`,
    /// then `"x":1`): LangChain raises at the second (spec § 5.8).
    FunctionCallKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Function,
    Custom,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub name: &'static str,
    pub args: String,
    pub kind: CallKind,
    pub pieces: usize,
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub before: Option<String>,
    pub calls: Vec<Call>,
    pub after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub choices: Vec<Choice>,
    pub quirks: Vec<(Quirk, usize)>,
    pub cuts: Vec<usize>,
}

const ALLOWED: [&str; 2] = ["read_file", "send_mail"];
const BLOCKED: [&str; 2] = ["blocked_rm", "blocked_exfil"];

fn call_strategy() -> impl Strategy<Value = Call> {
    (prop::bool::ANY, 0usize..2, "[a-z]{0,12}", 1usize..4, 0u8..10).prop_map(|(blocked, n, s, pieces, k)| {
        let name = if blocked { BLOCKED[n] } else { ALLOWED[n] };
        let args = if blocked { format!("{{\"v\":\"CANARY-{s}\"}}") } else { format!("{{\"v\":\"{s}\"}}") };
        let kind = match k {
            0 => CallKind::Custom,
            1 | 2 => CallKind::Legacy,
            _ => CallKind::Function,
        };
        Call { name, args, kind, pieces }
    })
}

fn choice_strategy() -> impl Strategy<Value = Choice> {
    (prop::option::of("[a-zA-Z ]{1,10}"), prop::collection::vec(call_strategy(), 0..4), prop::option::of("[a-z ]{1,8}")).prop_map(|(before, mut calls, after)| {
        if calls.len() > 1 {
            for c in &mut calls {
                if c.kind == CallKind::Legacy {
                    c.kind = CallKind::Function; // a legacy call is the only call of its choice
                }
            }
        }
        Choice { before, calls, after }
    })
}

fn quirk_strategy() -> impl Strategy<Value = Quirk> {
    prop_oneof![
        Just(Quirk::DuplicateDelta),
        Just(Quirk::CaseFoldedKey),
        Just(Quirk::ProtoKey),
        Just(Quirk::FloatIndex),
        Just(Quirk::BoolIndex),
        Just(Quirk::NegativeIndex),
        Just(Quirk::RepeatedName),
        Just(Quirk::Bom),
        Just(Quirk::LaxLine),
        Just(Quirk::UnknownFields),
        Just(Quirk::AfterDone),
        Just(Quirk::NoFinalBlank),
        Just(Quirk::Oversized),
        Just(Quirk::NaNRecord),
        Just(Quirk::JsonBody),
        Just(Quirk::EventLine),
        Just(Quirk::IndexGap),
        Just(Quirk::Comments),
        Just(Quirk::Interleave),
        Just(Quirk::EscapedKey),
        Just(Quirk::DropObject),
        Just(Quirk::MultiChoiceChunk),
        Just(Quirk::UnknownType),
        Just(Quirk::DuplicateChoiceIndex),
        Just(Quirk::ReverseChoices),
        Just(Quirk::TruncatedArgs),
        Just(Quirk::ConcatenatedObjects),
        Just(Quirk::CapitalisedArgKeys),
        Just(Quirk::NoChoicesKey),
        Just(Quirk::NoDelta),
        Just(Quirk::NullType),
        Just(Quirk::ChunkKey),
        Just(Quirk::ContentDeltaType),
        Just(Quirk::ArrayOutsideToolCalls),
        Just(Quirk::FunctionCallKey),
    ]
}

pub fn openai_scenario() -> impl Strategy<Value = Scenario> {
    (
        prop::collection::vec(choice_strategy(), 1..3),
        prop::collection::vec((quirk_strategy(), 0usize..64), 0..3),
        prop::collection::vec(0usize..100_000, 0..6),
    )
        .prop_map(|(choices, quirks, cuts)| Scenario { choices, quirks, cuts })
}

fn chunk(choices: Value) -> Value {
    json!({"id":"chatcmpl-g","object":"chat.completion.chunk","created":1,"model":"m","choices":choices})
}

fn hidden_entry(index: Value) -> Value {
    json!({"index":index,"id":"call_h","type":"function","function":{"name":"blocked_hidden","arguments":"{\"v\":\"CANARY-hidden\"}"}})
}

fn split(s: &str, pieces: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let step = chars.len().div_ceil(pieces.max(1)).max(1);
    chars.chunks(step).map(|c| c.iter().collect()).collect()
}

fn has(s: &Scenario, q: Quirk) -> Option<usize> {
    s.quirks.iter().find(|(x, _)| *x == q).map(|(_, at)| *at)
}

/// Arguments with keys that the envelope rules would reject (`Id`, `Name`, `constructor`, …).
pub fn capitalise_args(args: &str) -> String {
    match args.strip_prefix('{') {
        Some(rest) => format!(r#"{{"Id":1,"Name":"n","constructor":{{"Type":"t"}},"prototype":[],{rest}"#),
        None => args.to_owned(),
    }
}

/// The chunks of one choice. The first chunk carries `role` (openai-node's `finalChatCompletion`
/// raises without one), as every real upstream sends it.
fn choice_records(c: usize, ch: &Choice, interleave: bool, unknown_type: Option<usize>, bad_args: Option<(Quirk, usize)>, capitalise: bool) -> Vec<Value> {
    let mut out = vec![chunk(json!([{"index":c,"delta":{"role":"assistant","content":ch.before.clone().unwrap_or_default()}}]))];
    let odd_call = unknown_type.filter(|_| c == 0 && !ch.calls.is_empty()).map(|k| k % ch.calls.len());
    let json_calls: Vec<usize> = (0..ch.calls.len()).filter(|i| ch.calls[*i].kind != CallKind::Custom).collect();
    let bad = bad_args.filter(|_| c == 0 && !json_calls.is_empty()).map(|(q, k)| (q, json_calls[k % json_calls.len()]));
    // Fragments per call: the first carries id/type/name.
    let per_call: Vec<Vec<Value>> = ch
        .calls
        .iter()
        .enumerate()
        .map(|(i, call)| {
            let ty = if odd_call == Some(i) {
                "web_search"
            } else if call.kind == CallKind::Custom {
                "custom"
            } else {
                "function"
            };
            let (body, args_key) = if call.kind == CallKind::Custom { ("custom", "input") } else { ("function", "arguments") };
            let args = if capitalise { capitalise_args(&call.args) } else { call.args.clone() };
            let args = match bad {
                Some((Quirk::TruncatedArgs, b)) if b == i => args[..args.len() - 1].to_owned(),
                Some((Quirk::ConcatenatedObjects, b)) if b == i => format!("{args}{{\"b\":2}}"),
                _ => args,
            };
            split(&args, call.pieces)
                .into_iter()
                .enumerate()
                .map(|(k, piece)| {
                    let delta = if call.kind == CallKind::Legacy {
                        if k == 0 {
                            json!({"function_call":{"name":call.name,"arguments":piece}})
                        } else {
                            json!({"function_call":{"arguments":piece}})
                        }
                    } else if k == 0 {
                        json!({"tool_calls":[{"index":i,"id":format!("call_{c}_{i}"),"type":ty,body:{"name":call.name,args_key:piece}}]})
                    } else {
                        json!({"tool_calls":[{"index":i,body:{args_key:piece}}]})
                    };
                    chunk(json!([{"index":c,"delta":delta}]))
                })
                .collect()
        })
        .collect();
    if interleave {
        let longest = per_call.iter().map(Vec::len).max().unwrap_or(0);
        for k in 0..longest {
            for frags in &per_call {
                if let Some(f) = frags.get(k) {
                    out.push(f.clone());
                }
            }
        }
    } else {
        out.extend(per_call.into_iter().flatten());
    }
    if let Some(t) = &ch.after {
        out.push(chunk(json!([{"index":c,"delta":{"content":t}}])));
    }
    let finish = if ch.calls.iter().any(|c| c.kind == CallKind::Legacy) {
        "function_call"
    } else if ch.calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    out.push(chunk(json!([{"index":c,"delta":{},"finish_reason":finish}])));
    out
}

fn choice_of(rec: &Value) -> Option<u64> {
    rec.pointer("/choices/0/index").and_then(Value::as_u64)
}

fn is_tool_bearing(rec: &Value) -> bool {
    rec.get("choices")
        .and_then(Value::as_array)
        .is_some_and(|cs| cs.iter().any(|c| c.pointer("/delta/tool_calls").is_some() || c.pointer("/delta/function_call").is_some()))
}

/// Whether `line` is a generated record whose only entry carries tool data for choice 0.
fn opens_call(line: &str) -> bool {
    let Some(Ok(v)) = line.strip_prefix("data: ").map(|d| serde_json::from_str::<Value>(d.trim_end())) else {
        return false;
    };
    let Some([c]) = v.get("choices").and_then(Value::as_array).map(Vec::as_slice) else { return false };
    c.get("index").and_then(Value::as_u64) == Some(0) && is_tool_bearing(&v)
}

pub fn render_openai(s: &Scenario) -> Vec<u8> {
    let interleave = has(s, Quirk::Interleave).is_some();
    let bad_args = s.quirks.iter().copied().find(|(q, _)| matches!(q, Quirk::TruncatedArgs | Quirk::ConcatenatedObjects));
    let mut per: Vec<Vec<Value>> = s
        .choices
        .iter()
        .enumerate()
        .map(|(i, c)| choice_records(i, c, interleave, has(s, Quirk::UnknownType), bad_args, has(s, Quirk::CapitalisedArgKeys).is_some()))
        .collect();
    if has(s, Quirk::ReverseChoices).is_some() {
        per.reverse();
    }
    let mut recs: Vec<Value> = Vec::new();
    if interleave {
        let longest = per.iter().map(Vec::len).max().unwrap_or(0);
        for k in 0..longest {
            for p in &per {
                if let Some(r) = p.get(k) {
                    recs.push(r.clone());
                }
            }
        }
    } else {
        recs = per.into_iter().flatten().collect();
    }
    if let Some(at) = has(s, Quirk::MultiChoiceChunk) {
        // Merge record p with the first later record of another choice, that choice first.
        let p = at % recs.len();
        if let Some(q) = (p + 1..recs.len()).find(|q| choice_of(&recs[*q]) != choice_of(&recs[p])) {
            let later = recs.remove(q);
            let mut both = later["choices"].as_array().cloned().unwrap_or_default();
            both.extend(recs[p]["choices"].as_array().cloned().unwrap_or_default());
            recs[p]["choices"] = Value::Array(both);
        }
    }
    if let Some(at) = has(s, Quirk::DropObject) {
        let tool: Vec<usize> = (0..recs.len()).filter(|i| is_tool_bearing(&recs[*i])).collect();
        let i = if tool.is_empty() { at % recs.len() } else { tool[at % tool.len()] };
        if let Value::Object(m) = &mut recs[i] {
            m.remove("object");
        }
    }
    let mut lines: Vec<String> = recs.iter().map(|r| format!("data: {r}\n\n")).collect();
    let mut prefix = String::new();
    let mut suffix = String::new();
    for (q, at) in &s.quirks {
        let pos = at % (lines.len() + 1);
        let insert = |lines: &mut Vec<String>, rec: String| lines.insert(pos, format!("data: {rec}\n\n"));
        match q {
            Quirk::DuplicateDelta => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{}]}},"delta":{{"content":"x"}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::CaseFoldedKey => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"Tool_Calls":[{}]}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::ProtoKey => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"__proto__":{{"tool_calls":[{}]}}}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::FloatIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0.0))]}}])).to_string()),
            Quirk::BoolIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(true))]}}])).to_string()),
            Quirk::NegativeIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(-1))]}}])).to_string()),
            Quirk::RepeatedName => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"blocked_hidden","arguments":"CANARY-rn"}}]}}])).to_string()),
            Quirk::IndexGap => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(9))]}}])).to_string()),
            Quirk::NaNRecord => insert(&mut lines, format!(r#"{{"x":NaN,"choices":[{{"index":0,"delta":{{"tool_calls":[{}]}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::EscapedKey => insert(
                &mut lines,
                r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool\u005fcalls":[{"index":0,"id":"call_e","type":"function","function":{"name":"blocked_escaped","arguments":"CANARY-esc"}}]}}]}"#.to_owned(),
            ),
            Quirk::Oversized => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":format!("CANARY-big{}", "x".repeat(5000))}}]}}])).to_string()),
            Quirk::DuplicateChoiceIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"content":"x"}},{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}])).to_string()),
            Quirk::LaxLine => lines.insert(
                pos,
                format!(
                    "  data: {}\n\nData: {}\n\n",
                    chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}])),
                    chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}]))
                ),
            ),
            Quirk::NoDelta if at % 2 == 1 && lines.iter().any(|l| opens_call(l)) => {
                // R-B (spec § 5.2): a delta-less entry at position 1 of the record that opens a call.
                let i = lines.iter().position(|l| opens_call(l)).expect("checked");
                let mut v: Value = serde_json::from_str(lines[i].trim_start_matches("data: ").trim_end()).expect("a generated record");
                v["choices"].as_array_mut().expect("checked").push(json!({"index":1}));
                lines[i] = format!("data: {v}\n\n");
            }
            Quirk::NoChoicesKey | Quirk::NoDelta => {
                // SMA-749 D2 strips the BOM, so a client that ignores the BOM-prefixed first line
                // would read it after the strip (known limitation, spec § 9). Keep it off line 0.
                let at = if pos == 0 && has(s, Quirk::Bom).is_some() { 1 } else { pos };
                if at <= lines.len() {
                    let rec = if *q == Quirk::NoChoicesKey { r#"{"object":"chat.completion.chunk","id":"q"}"#.to_owned() } else { chunk(json!([{"index":0}])).to_string() };
                    lines.insert(at, format!("data: {rec}\n\n"));
                }
            }
            Quirk::NullType if s.choices[0].calls.is_empty() && has(s, Quirk::ReverseChoices).is_none() => {
                let at = 1.min(lines.len());
                lines.insert(at, format!("data: {}\n\n", chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"type":null}]}}]))));
            }
            Quirk::NullType => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"type":null,"function":{"name":"blocked_hidden","arguments":"CANARY-nt"}}]}}])).to_string()),
            Quirk::ChunkKey => insert(&mut lines, json!({"object":"chat.completion.chunk","id":"q","choices":[],"chunk":{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_ck","type":"function","function":{"name":"blocked_chunk","arguments":"CANARY-chunk"}}]}}]}}).to_string()),
            Quirk::ContentDeltaType => insert(&mut lines, {
                let mut r = chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"zz"}}]}}]));
                r["type"] = json!("content.delta");
                r.to_string()
            }),
            Quirk::ArrayOutsideToolCalls => {
                let delta = |list: Value| match at % 3 {
                    0 => json!({"foo":list}),
                    1 => json!({"tool_calls":[{"index":0,"function":{"arguments":""},"x":list}]}),
                    _ => json!({"tool_calls":[{"index":0,"function":{"arguments":"","x":list}}]}),
                };
                let two = [json!([{"index":0}]), json!([1])].map(|l| format!("data: {}\n\n", chunk(json!([{"index":0,"delta":delta(l)}]))));
                // As for NoChoicesKey: keep the raising records off the BOM-prefixed line 0 (spec § 9).
                let at = if pos == 0 && has(s, Quirk::Bom).is_some() { 1.min(lines.len()) } else { pos };
                lines.splice(at..at, two);
            }
            Quirk::FunctionCallKey => {
                let two = [json!("s"), json!(1)].map(|x| format!("data: {}\n\n", chunk(json!([{"index":0,"delta":{"function_call":{"arguments":"","x":x}}}]))));
                lines.splice(pos..pos, two);
            }
            Quirk::UnknownFields => lines.insert(pos, "id: 7\nretry: 10\nfoo: bar\n".to_owned()),
            Quirk::Comments => lines.insert(pos, ": hello\n: data: {\"x\":\"CANARY-comment\"}\n".to_owned()),
            Quirk::EventLine => lines.insert(pos, "event: message\n".to_owned()),
            Quirk::Bom => prefix = "\u{feff}".to_owned(),
            Quirk::AfterDone => suffix = format!("data: {}\n\n", chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}]))),
            Quirk::NoFinalBlank | Quirk::JsonBody | Quirk::Interleave | Quirk::DropObject | Quirk::MultiChoiceChunk | Quirk::UnknownType | Quirk::ReverseChoices | Quirk::TruncatedArgs | Quirk::ConcatenatedObjects | Quirk::CapitalisedArgKeys => {}
        }
    }
    let mut body = format!("{prefix}{}data: [DONE]\n\n{suffix}", lines.concat());
    if has(s, Quirk::NoFinalBlank).is_some() {
        body.truncate(body.trim_end_matches('\n').len());
    }
    if has(s, Quirk::JsonBody).is_some() {
        body = json!({"choices":[{"index":0,"message":{"tool_calls":[{"id":"c","type":"function","function":{"name":"blocked_json","arguments":"CANARY-json"}}]}}]}).to_string();
    }
    body.into_bytes()
}

#[derive(Debug, Clone)]
pub enum Block {
    Text(String),
    Tool {
        name: &'static str,
        args: String,
        pieces: usize,
    },
    Server,
    /// A `thinking` block: one `thinking_delta`, one `signature_delta`, then the stop (SMA-758).
    Thinking(String),
}

#[derive(Debug, Clone)]
pub struct AnthropicScenario {
    pub blocks: Vec<Block>,
    pub quirks: Vec<(AQuirk, usize)>,
    pub cuts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AQuirk {
    /// A `ping` event whose data is a hidden `tool_use` start (`event_type_mismatch`).
    EventMismatch,
    /// A hidden `tool_use` start with a non-empty `input` (`prefilled_input`).
    PrefilledInput,
    /// `message_start` carries a prefilled `tool_use` in `message.content` (`prefilled_message`).
    PrefilledMessage,
    /// A second `content_block_start` reuses index 0 (`index_reuse`).
    IndexReuse,
    /// No `event:` lines, only `data:` lines (the type is read from the payload).
    NoEventLines,
    /// A BOM starts the stream.
    Bom,
    /// A hidden `content_block_start` follows `message_stop` (`after_done`).
    AfterStop,
    /// A delta record with a duplicate `delta` key (`duplicate_key`).
    DuplicateKey,
    /// A record over `max_record_bytes` (`oversized`).
    Oversized,
    /// The next block starts before the previous one stops (`block_interleave`).
    Interleave,
    /// A tool's last argument piece is lost, so its JSON is incomplete (`incomplete_arguments`).
    TruncatedArgs,
    /// A delta with index -1 (anthropic-python's `content[-1]` is the last block).
    NegativeIndex,
    /// A delta for block 0 right after the last block stop.
    DeltaAfterStop,
    /// Every tool's input gains PascalCase, `constructor` and `prototype` keys (application data,
    /// so conforming; I-1).
    CapitalisedArgKeys,
    /// SMA-758: each `input_json_delta` of every tool is cut at odd byte offsets, inside a JSON
    /// string escape (`\"`) and next to `é`. The joined input is unchanged (and conforming).
    SplitPartialJson,
    /// SMA-758: a `ping` event between two deltas of the first `Tool` block (conforming).
    PingInHeldBlock,
    /// SMA-758: the `message_delta` frame moves to just before the last `content_block_stop`.
    MessageDeltaBeforeLastStop,
}

fn anthropic_block() -> impl Strategy<Value = Block> {
    prop_oneof![
        "[a-z ]{1,10}".prop_map(Block::Text),
        "[a-z ]{0,10}".prop_map(Block::Thinking),
        (prop::bool::ANY, 0usize..2, "[a-z]{0,10}", 1usize..4).prop_map(|(b, n, s, pieces)| if b {
            Block::Tool {
                name: BLOCKED[n],
                args: format!("{{\"v\":\"CANARY-{s}\",\"n\":10.5}}"),
                pieces,
            }
        } else {
            Block::Tool {
                name: ALLOWED[n],
                args: format!("{{\"v\":\"{s}\",\"n\":10.5}}"),
                pieces,
            }
        }),
        Just(Block::Server),
    ]
}

fn anthropic_quirk() -> impl Strategy<Value = AQuirk> {
    prop_oneof![
        Just(AQuirk::EventMismatch),
        Just(AQuirk::PrefilledInput),
        Just(AQuirk::PrefilledMessage),
        Just(AQuirk::IndexReuse),
        Just(AQuirk::NoEventLines),
        Just(AQuirk::Bom),
        Just(AQuirk::AfterStop),
        Just(AQuirk::DuplicateKey),
        Just(AQuirk::Oversized),
        Just(AQuirk::Interleave),
        Just(AQuirk::TruncatedArgs),
        Just(AQuirk::NegativeIndex),
        Just(AQuirk::DeltaAfterStop),
        Just(AQuirk::CapitalisedArgKeys),
        Just(AQuirk::SplitPartialJson),
        Just(AQuirk::PingInHeldBlock),
        Just(AQuirk::MessageDeltaBeforeLastStop),
    ]
}

pub fn anthropic_scenario() -> impl Strategy<Value = AnthropicScenario> {
    anthropic_scenario_with_quirks(0..3)
}

/// The Anthropic generator with a chosen number of quirks per scenario (SMA-758).
pub fn anthropic_scenario_with_quirks(n: std::ops::Range<usize>) -> impl Strategy<Value = AnthropicScenario> {
    (
        prop::collection::vec(anthropic_block(), 0..7),
        prop::collection::vec((anthropic_quirk(), 0usize..64), n),
        prop::collection::vec(0usize..100_000, 0..6),
    )
        .prop_map(|(blocks, quirks, cuts)| AnthropicScenario { blocks, quirks, cuts })
}

/// Cuts a tool's JSON at every odd byte offset that is a character boundary, and after every
/// backslash, so a cut falls inside an escape such as `\"` (SMA-758).
fn split_odd(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        if i > start && (i % 2 == 1 || prev == '\\') {
            out.push(s[start..i].to_owned());
            start = i;
        }
        prev = c;
    }
    out.push(s[start..].to_owned());
    out
}

fn ahas(s: &AnthropicScenario, q: AQuirk) -> Option<usize> {
    s.quirks.iter().find(|(x, _)| *x == q).map(|(_, at)| *at)
}

pub fn render_anthropic(s: &AnthropicScenario) -> Vec<u8> {
    let mut evs: Vec<Value> = vec![json!({"type":"message_start","message":{"id":"msg_g","type":"message","role":"assistant","content":[],"model":"c","usage":{"input_tokens":1,"output_tokens":1}}})];
    let tools: Vec<usize> = s.blocks.iter().enumerate().filter(|(_, b)| matches!(b, Block::Tool { .. })).map(|(i, _)| i).collect();
    let truncated = ahas(s, AQuirk::TruncatedArgs).filter(|_| !tools.is_empty()).map(|k| tools[k % tools.len()]);
    // With Interleave, block k's stop is deferred until after block k+1's start.
    let interleave_at = ahas(s, AQuirk::Interleave).filter(|_| s.blocks.len() > 1).map(|k| k % (s.blocks.len() - 1));
    let mut deferred: Option<Value> = None;
    let ping_at = ahas(s, AQuirk::PingInHeldBlock).and_then(|_| tools.first().copied());
    for (i, b) in s.blocks.iter().enumerate() {
        match b {
            Block::Text(t) => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}));
                if let Some(d) = deferred.take() {
                    evs.push(d);
                }
                evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":t}}));
            }
            Block::Tool { name, args, pieces } => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("toolu_{i}"),"name":name,"input":{}}}));
                if let Some(d) = deferred.take() {
                    evs.push(d);
                }
                let args = if ahas(s, AQuirk::CapitalisedArgKeys).is_some() { capitalise_args(args) } else { args.clone() };
                let mut parts = if ahas(s, AQuirk::SplitPartialJson).is_some() {
                    // The string value gains an escaped quote and `é`, so cuts land inside them.
                    split_odd(&args.replacen("\"v\":\"", "\"v\":\"\\\"é", 1))
                } else {
                    split(&args, *pieces)
                };
                if truncated == Some(i) {
                    let last = parts.pop().unwrap_or_default();
                    parts.push(last[..last.len().saturating_sub(1)].to_owned());
                }
                for (k, p) in parts.into_iter().enumerate() {
                    evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":p}}));
                    if k == 0 && ping_at == Some(i) {
                        evs.push(json!({"type":"ping"}));
                    }
                }
            }
            Block::Thinking(t) => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"thinking","thinking":""}}));
                if let Some(d) = deferred.take() {
                    evs.push(d);
                }
                evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"thinking_delta","thinking":t}}));
                evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"signature_delta","signature":"EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMDdv"}}));
            }
            Block::Server => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"server_tool_use","id":format!("srv_{i}"),"name":"web_search","input":{}}}));
                if let Some(d) = deferred.take() {
                    evs.push(d);
                }
                evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"x\"}"}}));
            }
        }
        let stop = json!({"type":"content_block_stop","index":i});
        if interleave_at == Some(i) {
            deferred = Some(stop);
        } else {
            evs.push(stop);
        }
    }
    let any_tool = !tools.is_empty();
    let message_delta = json!({"type":"message_delta","delta":{"stop_reason": if any_tool { "tool_use" } else { "end_turn" },"stop_sequence":null},"usage":{"output_tokens":1}});
    match evs
        .iter()
        .rposition(|e| e["type"] == "content_block_stop")
        .filter(|_| ahas(s, AQuirk::MessageDeltaBeforeLastStop).is_some())
    {
        Some(at) => evs.insert(at, message_delta),
        None => evs.push(message_delta),
    }
    evs.push(json!({"type":"message_stop"}));
    let no_events = ahas(s, AQuirk::NoEventLines).is_some();
    let mut frames: Vec<String> = evs
        .iter()
        .map(|e| {
            if no_events {
                format!("data: {e}\n\n")
            } else {
                format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())
            }
        })
        .collect();
    let hidden_start = |idx: usize, input: Value| json!({"type":"content_block_start","index":idx,"content_block":{"type":"tool_use","id":"toolu_h","name":"blocked_hidden","input":input}});
    let mut prefix = String::new();
    let mut suffix = String::new();
    for (q, at) in &s.quirks {
        let pos = 1 + at % frames.len().max(1);
        let pos = pos.min(frames.len().saturating_sub(1));
        match q {
            AQuirk::EventMismatch => frames.insert(pos, format!("event: ping\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({"v":"CANARY-em"})))),
            AQuirk::PrefilledInput => frames.insert(pos, format!("event: content_block_start\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({"v":"CANARY-pi"})))),
            AQuirk::PrefilledMessage => {
                frames[0] = format!(
                    "event: message_start\ndata: {}\n\n",
                    json!({"type":"message_start","message":{"content":[{"type":"tool_use","id":"t","name":"blocked_hidden","input":{"v":"CANARY-pm"}}]}})
                )
            }
            AQuirk::IndexReuse => frames.insert(
                pos,
                format!(
                    "event: content_block_start\ndata: {}\n\n",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_r","name":"blocked_reuse","input":{}}})
                ),
            ),
            AQuirk::DuplicateKey => frames.insert(
                pos,
                format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"CANARY-dup"},"delta":{"type":"text_delta","text":"x"}}"#
                ),
            ),
            AQuirk::Oversized => frames.insert(
                pos,
                format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":format!("CANARY-big{}", "x".repeat(5000))}})
                ),
            ),
            AQuirk::NegativeIndex => frames.insert(
                pos,
                format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    json!({"type":"content_block_delta","index":-1,"delta":{"type":"input_json_delta","partial_json":"\"CANARY-neg\""}})
                ),
            ),
            AQuirk::DeltaAfterStop => frames.insert(
                // Right after the last block stop, so the delta follows a stop whatever else moved.
                frames
                    .iter()
                    .rposition(|f| f.contains("\"content_block_stop\""))
                    .map_or(frames.len().saturating_sub(2).max(1), |p| p + 1),
                format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"CANARY-late\""}})
                ),
            ),
            AQuirk::Bom => prefix = "\u{feff}".to_owned(),
            AQuirk::AfterStop => suffix = format!("event: content_block_start\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({}))),
            AQuirk::NoEventLines
            | AQuirk::Interleave
            | AQuirk::TruncatedArgs
            | AQuirk::CapitalisedArgKeys
            | AQuirk::SplitPartialJson
            | AQuirk::PingInHeldBlock
            | AQuirk::MessageDeltaBeforeLastStop => {}
        }
    }
    format!("{prefix}{}{suffix}", frames.concat()).into_bytes()
}

// ---- Responses (SMA-769 § 10.3) ----------------------------------------------------------------

/// The kinds of output item the Responses generator makes: every class of spec § 6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RKind {
    Message,
    Reasoning,
    Function,
    Custom,
    Computer,
    LocalShell,
    Shell,
    ApplyPatch,
    ToolSearch,
    McpApproval,
    WebSearch,
    FileSearch,
    /// A `function_call_output` item in the output: client-run, kind `Other` (fail closed).
    Other,
}

impl RKind {
    /// Client-run: held and decided (§ 6.2).
    pub fn client_run(self) -> bool {
        !matches!(self, RKind::Message | RKind::Reasoning | RKind::WebSearch | RKind::FileSearch)
    }

    fn has_call_id(self) -> bool {
        self.client_run() && self != RKind::McpApproval
    }

    fn id_prefix(self) -> &'static str {
        match self {
            RKind::Message => "msg",
            RKind::Reasoning => "rs",
            RKind::Function => "fc",
            RKind::Custom => "ctc",
            RKind::Computer => "cu",
            RKind::LocalShell => "lsh",
            RKind::Shell => "sh",
            RKind::ApplyPatch => "apc",
            RKind::ToolSearch => "ts",
            RKind::McpApproval => "mcpr",
            RKind::WebSearch => "ws",
            RKind::FileSearch => "fs",
            RKind::Other => "fco",
        }
    }
}

/// One output item. `name` is `read_file`/`send_mail` or a `blocked_*` name for the kinds that
/// carry a name (function, custom, MCP approval request); a `blocked_*` item has `CANARY` in its
/// `text`. `text` is the arguments, the input, the message or summary text, the shell command, the
/// patch body, or the query.
#[derive(Debug, Clone)]
pub struct RItem {
    pub kind: RKind,
    pub name: &'static str,
    pub text: String,
    pub pieces: usize,
    pub namespace: Option<&'static str>,
}

impl RItem {
    fn new(kind: RKind, blocked: bool, n: usize, s: &str, pieces: usize, namespaced: bool) -> Self {
        let named = matches!(kind, RKind::Function | RKind::Custom | RKind::McpApproval);
        let blocked = blocked && named;
        let name = if !named {
            ""
        } else if blocked {
            BLOCKED[n]
        } else {
            ALLOWED[n]
        };
        let text = match kind {
            RKind::Function | RKind::McpApproval if blocked => format!("{{\"v\":\"CANARY-{s}\"}}"),
            RKind::Function | RKind::McpApproval => format!("{{\"v\":\"{s}\"}}"),
            RKind::Custom if blocked => format!("CANARY-{s}"),
            RKind::Message | RKind::Reasoning | RKind::WebSearch | RKind::FileSearch if s.is_empty() => "t".to_owned(),
            _ => s.to_owned(),
        };
        let namespace = (namespaced && matches!(kind, RKind::Function | RKind::Custom)).then_some("fs");
        RItem { kind, name, text, pieces, namespace }
    }

    /// An allowed item of `kind`, which a quirk needs.
    fn plain(kind: RKind) -> Self {
        RItem::new(kind, false, 0, "a", 2, false)
    }
}

/// One quirk per client raise path of spec § 3.2 and per rule of § 6.4 (ruling F3). The doc line
/// names the rule (or the raise path) that the quirk is placed to reach first, when it is the only
/// quirk; the variant used depends on the quirk's position value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RQuirk {
    /// § 3.2 openai-node `event: error` raise; § 6.3: an `event:` name that differs from `data.type`.
    EventErrorName,
    /// § 3.2 truthy top-level `error` (every SDK raises); § 6.3: an upstream error record.
    TopLevelError,
    /// § 3.2 `[DONE]` prefix match (Python stops) vs exact match (Node raises): `[DONE]x`.
    DoneTrailing,
    /// § 3.2 unknown event type (Node raises); § 6.3: any other type is malformed.
    UnknownType,
    /// R4: an `output_item.added` that reuses an earlier `output_index`.
    RepeatedIndex,
    /// R6: a function delta before its item's `added` (no open item has its index).
    DeltaBeforeAdded,
    /// R6: an `item_id` that is not the open item's `id` (openai-node raises).
    ItemIdMismatch,
    /// R6 and R-e: a function delta on a custom or a message item.
    WrongItemTypeEvent,
    /// R9 and R-b: a message whose `done` item is a function call.
    TypeChangesAtDone,
    /// R9 and R-b: a message whose terminal `output` item is a function call.
    TypeChangesInTerminal,
    /// R10: a call in the terminal `output` that never streamed; at an odd position a record after
    /// the terminal makes openai-node raise after it emitted that output (ruling T7e).
    TerminalItemNeverStreamed,
    /// R8 and § 3.2 LangChain `AttributeError`: `item: null` on `added`.
    ItemNull,
    /// R8: an item `id` that an earlier item has.
    DuplicateId,
    /// R8: a `call_id` that an earlier item has.
    DuplicateCallId,
    /// R8: an upstream item `id` with the reserved `msg_paigasus_` prefix.
    ReservedId,
    /// R2: a hidden call in `response.in_progress.output` (or `response.created.output`).
    EarlyOutput,
    /// R9: a terminal function item with a new `name`.
    TerminalNewName,
    /// R9: a terminal function item with a new `namespace`.
    TerminalNewNamespace,
    /// R11: `added.item.arguments` that is not `""` (LangChain reads it).
    AddedArgumentsNotEmpty,
    /// R3 and R-d: a terminal event with no `output` (Python fills it from partial items).
    TerminalOutputAbsent,
    /// R3: a terminal event with `output: null`.
    TerminalOutputNull,
    /// R15 and § 3.2 `response.failed` raises: the last client-run item has no `done`, and the
    /// terminal is `response.failed` with its partial item (even) or with no `output` (odd: R3
    /// first, and the openai-python R-d fill exposes the partial call).
    FailedWithOpenItem,
    /// § 3.2 / 749 D17: a `type: "error"` event inside a client-run item.
    ErrorEventMidItem,
    /// R15 (M5): a client-run `done` with `status: "incomplete"` (a function's arguments cut).
    DoneIncomplete,
    /// R11 (`incomplete_arguments`): completed function arguments that are no complete JSON value.
    InvalidJsonArguments,
    /// R1: a lifecycle or terminal `response.id` that differs from the first.
    ResponseIdChanged,
    /// § 3.2 leading BOM (openai-node strips it, openai-python does not).
    Bom,
    /// § 3.2 a last event with no blank line at EOF (Python drops it, Node flushes it).
    NoFinalBlankLine,
    /// R7 exemption: a `keepalive` without `sequence_number` (conforming).
    KeepaliveWithoutSequence,
    /// R5: the next item starts before a client-run item's `done`. At an even position the item
    /// never gets its `done`, while the terminal `output` holds it in full.
    ItemInterleaved,
    /// R7: a `sequence_number` that does not increase (on the first event: no integer), or one on
    /// a single event only when the stream has none.
    SequenceNotIncreasing,
    /// R16: an event for an item after its `done`.
    EventAfterDone,
    /// R13: a shell command that differs between the command events and the `done` item, or
    /// non-empty `added` commands that differ (ruling F2).
    ShellCommandMismatch,
    /// R1: the first record is not `response.created`.
    FirstNotCreated,
    /// R1 (ruling T4c): a second `response.created` (`repeated_created`).
    RepeatedCreated,
    /// R3 (ruling F8): a record after the terminal event: dropped and counted.
    RecordAfterTerminal,
    /// R15 (ruling T6a): no terminal event at all.
    NoTerminal,
    /// R4: an `output_index` that skips one.
    IndexGap,
    /// R10: a client-run terminal item that differs from its `done` item.
    TerminalArgsDiffer,
    /// R11: function deltas, `arguments.done` and the `done` arguments that disagree.
    ArgumentsMismatch,
    /// R12: custom input deltas, `input.done` and the `done` input that disagree.
    InputMismatch,
    /// R14 (amendment A4): an `apply_patch_call` diff that differs between the diff events and the
    /// `done` item.
    DiffMismatch,
    /// § 6.4 subset rule (amendment A2): an `added` field that is not a partial view of `done`.
    AddedNotSubset,
    /// R15 (ruling T4b): no `done` status. A truncation for a function call (even); conforming for
    /// a custom call, whose model has no `status` field (odd).
    AbsentStatus,
    /// R15: EOF inside a client-run item, with no terminal event.
    EofInItem,
    /// D16 (`max_record_bytes`): a record over the limit with `CANARY`: a function delta (even) or
    /// a terminal `output` with a blocked call (odd).
    Oversized,
    /// D13 (`max_held_bytes`): one open function call whose deltas, each under the record limit,
    /// hold more than the hold limit in all.
    HoldLimit,
}

/// Every [`RQuirk`], for the strategy and for the per-quirk test.
pub const RQUIRKS: [RQuirk; 47] = [
    RQuirk::EventErrorName,
    RQuirk::TopLevelError,
    RQuirk::DoneTrailing,
    RQuirk::UnknownType,
    RQuirk::RepeatedIndex,
    RQuirk::DeltaBeforeAdded,
    RQuirk::ItemIdMismatch,
    RQuirk::WrongItemTypeEvent,
    RQuirk::TypeChangesAtDone,
    RQuirk::TypeChangesInTerminal,
    RQuirk::TerminalItemNeverStreamed,
    RQuirk::ItemNull,
    RQuirk::DuplicateId,
    RQuirk::DuplicateCallId,
    RQuirk::ReservedId,
    RQuirk::EarlyOutput,
    RQuirk::TerminalNewName,
    RQuirk::TerminalNewNamespace,
    RQuirk::AddedArgumentsNotEmpty,
    RQuirk::TerminalOutputAbsent,
    RQuirk::TerminalOutputNull,
    RQuirk::FailedWithOpenItem,
    RQuirk::ErrorEventMidItem,
    RQuirk::DoneIncomplete,
    RQuirk::InvalidJsonArguments,
    RQuirk::ResponseIdChanged,
    RQuirk::Bom,
    RQuirk::NoFinalBlankLine,
    RQuirk::KeepaliveWithoutSequence,
    RQuirk::ItemInterleaved,
    RQuirk::SequenceNotIncreasing,
    RQuirk::EventAfterDone,
    RQuirk::ShellCommandMismatch,
    RQuirk::FirstNotCreated,
    RQuirk::RepeatedCreated,
    RQuirk::RecordAfterTerminal,
    RQuirk::NoTerminal,
    RQuirk::IndexGap,
    RQuirk::TerminalArgsDiffer,
    RQuirk::ArgumentsMismatch,
    RQuirk::InputMismatch,
    RQuirk::DiffMismatch,
    RQuirk::AddedNotSubset,
    RQuirk::AbsentStatus,
    RQuirk::EofInItem,
    RQuirk::Oversized,
    RQuirk::HoldLimit,
];

/// A generated Responses stream. The real-traffic facts of spec § 3.3 hold unless a quirk breaks
/// one: items are contiguous, `output_index` is stable, a function's `added.arguments` is `""`,
/// a reasoning item's `encrypted_content` differs between `done` and the terminal `output` (A1),
/// and built-in `added` items are partial (A2). `sequence`, `event_lines` and `obfuscation` follow
/// M2 when true; `done_marker` adds the `[DONE]` that OpenAI does not send.
#[derive(Debug, Clone)]
pub struct ResponsesScenario {
    pub items: Vec<RItem>,
    pub quirks: Vec<(RQuirk, usize)>,
    pub cuts: Vec<usize>,
    pub sequence: bool,
    pub event_lines: bool,
    pub done_marker: bool,
    pub obfuscation: bool,
}

fn ritem_strategy() -> impl Strategy<Value = RItem> {
    let kind = prop_oneof![
        4 => Just(RKind::Function),
        2 => Just(RKind::Custom),
        2 => Just(RKind::Message),
        2 => Just(RKind::Reasoning),
        1 => Just(RKind::Computer),
        1 => Just(RKind::LocalShell),
        1 => Just(RKind::Shell),
        1 => Just(RKind::ApplyPatch),
        1 => Just(RKind::ToolSearch),
        1 => Just(RKind::McpApproval),
        1 => Just(RKind::WebSearch),
        1 => Just(RKind::FileSearch),
        1 => Just(RKind::Other),
    ];
    (kind, prop::bool::ANY, 0usize..2, "[a-z]{0,10}", 1usize..4, 0u8..4).prop_map(|(kind, blocked, n, s, pieces, ns)| RItem::new(kind, blocked, n, &s, pieces, ns == 0))
}

pub fn responses_scenario() -> impl Strategy<Value = ResponsesScenario> {
    (
        prop::collection::vec(ritem_strategy(), 0..6),
        prop::collection::vec((prop::sample::select(RQUIRKS.to_vec()), 0usize..64), 0..3),
        prop::collection::vec(0usize..100_000, 0..6),
        (prop::bool::weighted(0.8), prop::bool::weighted(0.8), prop::bool::weighted(0.3), prop::bool::ANY),
    )
        .prop_map(|(items, quirks, cuts, (sequence, event_lines, done_marker, obfuscation))| {
            let mut s = ResponsesScenario {
                items,
                quirks,
                cuts,
                sequence,
                event_lines,
                done_marker,
                obfuscation,
            };
            s.ensure_targets();
            s
        })
}

impl ResponsesScenario {
    /// Add the items a quirk needs, so that every quirk reaches its rule (ruling F3).
    pub fn ensure_targets(&mut self) {
        for (q, at) in self.quirks.clone() {
            let any = |s: &Self, f: &dyn Fn(&RItem) -> bool| s.items.iter().any(f);
            let need: Option<RKind> = match q {
                RQuirk::DeltaBeforeAdded
                | RQuirk::ItemIdMismatch
                | RQuirk::TerminalNewName
                | RQuirk::TerminalNewNamespace
                | RQuirk::TerminalArgsDiffer
                | RQuirk::AddedArgumentsNotEmpty
                | RQuirk::ArgumentsMismatch
                | RQuirk::InvalidJsonArguments
                | RQuirk::Oversized
                | RQuirk::HoldLimit
                | RQuirk::EventAfterDone => (!any(self, &|i| i.kind == RKind::Function)).then_some(RKind::Function),
                RQuirk::AbsentStatus if at.is_multiple_of(2) => (!any(self, &|i| i.kind == RKind::Function)).then_some(RKind::Function),
                RQuirk::AbsentStatus | RQuirk::InputMismatch => (!any(self, &|i| i.kind == RKind::Custom)).then_some(RKind::Custom),
                RQuirk::TypeChangesAtDone | RQuirk::TypeChangesInTerminal => (!any(self, &|i| i.kind == RKind::Message)).then_some(RKind::Message),
                RQuirk::WrongItemTypeEvent => (!any(self, &|i| matches!(i.kind, RKind::Custom | RKind::Message))).then_some(RKind::Message),
                RQuirk::ShellCommandMismatch => (!any(self, &|i| i.kind == RKind::Shell)).then_some(RKind::Shell),
                RQuirk::DiffMismatch => (!any(self, &|i| i.kind == RKind::ApplyPatch && !i.text.is_empty())).then_some(RKind::ApplyPatch),
                RQuirk::AddedNotSubset => (!any(self, &|i| matches!(i.kind, RKind::Computer | RKind::LocalShell | RKind::ToolSearch | RKind::McpApproval))).then_some(RKind::Computer),
                RQuirk::DoneIncomplete | RQuirk::ErrorEventMidItem | RQuirk::NoTerminal => (!any(self, &|i| i.kind.client_run())).then_some(RKind::Function),
                RQuirk::RepeatedIndex | RQuirk::IndexGap | RQuirk::ItemNull | RQuirk::ReservedId => self.items.is_empty().then_some(RKind::Message),
                RQuirk::FailedWithOpenItem | RQuirk::EofInItem => (!self.items.last().is_some_and(|i| i.kind.client_run())).then_some(RKind::Function),
                RQuirk::ItemInterleaved => {
                    let n = self.items.len();
                    if !self.items.iter().take(n.saturating_sub(1)).any(|i| i.kind.client_run()) {
                        self.items.push(RItem::plain(RKind::Function));
                        self.items.push(RItem::plain(RKind::Message));
                    }
                    None
                }
                RQuirk::DuplicateId => {
                    while self.items.len() < 2 {
                        self.items.push(RItem::plain(RKind::Message));
                    }
                    None
                }
                RQuirk::DuplicateCallId => {
                    while self.items.iter().filter(|i| i.kind.has_call_id()).count() < 2 {
                        self.items.push(RItem::plain(RKind::Function));
                    }
                    None
                }
                _ => None,
            };
            if let Some(kind) = need {
                let mut it = RItem::plain(kind);
                if kind == RKind::ApplyPatch {
                    it.text = "x".to_owned();
                }
                if matches!(q, RQuirk::FailedWithOpenItem | RQuirk::EofInItem) {
                    self.items.push(it);
                } else {
                    let pos = at % (self.items.len() + 1);
                    self.items.insert(pos, it);
                }
            }
        }
    }
}

/// One record of a generated Responses stream.
#[derive(Debug, Clone)]
enum Rec {
    /// An event; it gets a `sequence_number` when the stream has them.
    Ev(Value),
    /// An event that never gets a `sequence_number` (a keepalive).
    Bare(Value),
    /// An event under another `event:` name.
    Named(Value, &'static str),
    /// A `data:` line that is no JSON.
    Raw(&'static str),
}

/// The records of one item: `added`, the middle events, the records of an interleaved item
/// (`inner`), and `done`; `pre` goes before `added` and `post` after `done`.
#[derive(Debug, Clone, Default)]
struct Seg {
    pre: Vec<Rec>,
    added: Option<Value>,
    middle: Vec<Rec>,
    inner: Vec<Rec>,
    done: Option<Value>,
    post: Vec<Rec>,
}

impl Seg {
    fn flat(self) -> Vec<Rec> {
        let mut out = self.pre;
        out.extend(self.added.map(Rec::Ev));
        out.extend(self.middle);
        out.extend(self.inner);
        out.extend(self.done.map(Rec::Ev));
        out.extend(self.post);
        out
    }

    fn added_item(&mut self) -> Option<&mut Value> {
        self.added.as_mut().map(|a| &mut a["item"])
    }

    fn done_item(&mut self) -> Option<&mut Value> {
        self.done.as_mut().map(|d| &mut d["item"])
    }

    /// The middle events, as JSON, of the type `ty`.
    fn middles(&mut self, ty: &str) -> Vec<&mut Value> {
        self.middle
            .iter_mut()
            .filter_map(|r| match r {
                Rec::Ev(v) if v["type"] == ty => Some(v),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Items,
    Absent,
    Null,
}

const ADDED_T: &str = "response.output_item.added";
const DONE_T: &str = "response.output_item.done";

fn added_ev(i: usize, item: Value) -> Value {
    json!({"type": ADDED_T, "item": item, "output_index": i})
}

fn done_ev(i: usize, item: Value) -> Value {
    json!({"type": DONE_T, "item": item, "output_index": i})
}

fn scoped_ev(ty: &str, i: usize, id: &str, extra: Value) -> Value {
    let mut v = json!({"type": ty});
    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
        m.extend(e);
        m.insert("item_id".into(), json!(id));
        m.insert("output_index".into(), json!(i));
    }
    v
}

fn with_status(item: &Value, status: &str) -> Value {
    let mut v = item.clone();
    v["status"] = json!(status);
    v
}

/// The hidden call that quirks inject: its name is blocked and it carries the canary.
fn hidden_call(id: &str) -> Value {
    json!({"type": "function_call", "id": id, "status": "completed", "arguments": "{\"v\":\"CANARY-hidden\"}", "call_id": "call_hidden", "name": "blocked_hidden"})
}

/// The deltas of `text`, as `pieces` events of type `ty` (each with `extra`).
fn deltas(ty: &str, i: usize, id: &str, text: &str, pieces: usize, obfuscation: bool) -> Vec<Rec> {
    split(text, pieces)
        .into_iter()
        .map(|d| {
            let mut v = scoped_ev(ty, i, id, json!({"delta": d}));
            if obfuscation {
                v["obfuscation"] = json!("ob");
            }
            Rec::Ev(v)
        })
        .collect()
}

/// The records of item `i` and its terminal `output` item, as § 3.3 measured them.
fn build_item(i: usize, it: &RItem, id: &str, call_id: Option<&str>, obf: bool) -> (Seg, Value) {
    let call_id = call_id.unwrap_or("");
    let text = it.text.as_str();
    let mut seg = Seg::default();
    let (open, done, terminal) = match it.kind {
        RKind::Function | RKind::Custom => {
            let (ty, key, family) = if it.kind == RKind::Function {
                ("function_call", "arguments", "response.function_call_arguments")
            } else {
                ("custom_tool_call", "input", "response.custom_tool_call_input")
            };
            let mut done = json!({"type": ty, "id": id, "status": "completed", "call_id": call_id, "name": it.name, key: text});
            if let Some(ns) = it.namespace {
                done["namespace"] = json!(ns);
            }
            let mut open = with_status(&done, "in_progress");
            open[key] = json!("");
            seg.middle = deltas(&format!("{family}.delta"), i, id, text, it.pieces, obf);
            seg.middle.push(Rec::Ev(scoped_ev(&format!("{family}.done"), i, id, json!({key: text}))));
            (open, done.clone(), done)
        }
        RKind::Message => {
            let part = json!({"type": "output_text", "annotations": [], "logprobs": [], "text": text});
            let empty = json!({"type": "output_text", "annotations": [], "logprobs": [], "text": ""});
            let open = json!({"type": "message", "id": id, "status": "in_progress", "content": [], "role": "assistant"});
            let done = json!({"type": "message", "id": id, "status": "completed", "content": [part.clone()], "role": "assistant"});
            seg.middle.push(Rec::Ev(scoped_ev("response.content_part.added", i, id, json!({"content_index": 0, "part": empty}))));
            for d in split(text, it.pieces) {
                let mut v = scoped_ev("response.output_text.delta", i, id, json!({"content_index": 0, "delta": d, "logprobs": []}));
                if obf {
                    v["obfuscation"] = json!("ob");
                }
                seg.middle.push(Rec::Ev(v));
            }
            seg.middle
                .push(Rec::Ev(scoped_ev("response.output_text.done", i, id, json!({"content_index": 0, "logprobs": [], "text": text}))));
            seg.middle.push(Rec::Ev(scoped_ev("response.content_part.done", i, id, json!({"content_index": 0, "part": part}))));
            (open, done.clone(), done)
        }
        RKind::Reasoning => {
            let summary = json!({"type": "summary_text", "text": text});
            let open = json!({"type": "reasoning", "id": id, "content": [], "encrypted_content": format!("enc-added-{i}"), "summary": []});
            let done = json!({"type": "reasoning", "id": id, "content": [], "encrypted_content": format!("enc-done-{i}"), "summary": [summary.clone()]});
            seg.middle.push(Rec::Ev(scoped_ev(
                "response.reasoning_summary_part.added",
                i,
                id,
                json!({"part": {"type": "summary_text", "text": ""}, "summary_index": 0}),
            )));
            for d in split(text, it.pieces) {
                let mut v = scoped_ev("response.reasoning_summary_text.delta", i, id, json!({"delta": d, "summary_index": 0}));
                if obf {
                    v["obfuscation"] = json!("ob");
                }
                seg.middle.push(Rec::Ev(v));
            }
            seg.middle
                .push(Rec::Ev(scoped_ev("response.reasoning_summary_text.done", i, id, json!({"summary_index": 0, "text": text}))));
            seg.middle
                .push(Rec::Ev(scoped_ev("response.reasoning_summary_part.done", i, id, json!({"part": summary, "summary_index": 0}))));
            // Amendment A1: the terminal `encrypted_content` differs from the `done` one.
            let terminal = {
                let mut t = done.clone();
                t["encrypted_content"] = json!(format!("enc-terminal-{i}"));
                t
            };
            (open, done, terminal)
        }
        RKind::Computer => {
            let action = if text.is_empty() { json!({"type": "screenshot"}) } else { json!({"type": "type", "text": text}) };
            let open = json!({"type": "computer_call", "id": id, "status": "in_progress", "call_id": call_id});
            // A2 (M4): `actions` only at `done`.
            let done = json!({"type": "computer_call", "id": id, "status": "completed", "actions": [action], "call_id": call_id});
            (open, done.clone(), done)
        }
        RKind::LocalShell => {
            let done = json!({"type": "local_shell_call", "id": id, "status": "completed", "call_id": call_id, "action": {"type": "exec", "command": ["ls", text], "env": {}}});
            (with_status(&done, "in_progress"), done.clone(), done)
        }
        RKind::Shell => {
            let mut commands = vec![format!("ls {text}")];
            if it.pieces == 3 {
                commands.push("date".to_owned());
            }
            let open =
                json!({"type": "shell_call", "id": id, "status": "in_progress", "action": {"commands": [], "max_output_length": null, "timeout_ms": null}, "call_id": call_id, "environment": null});
            let done = json!({"type": "shell_call", "id": id, "status": "completed", "action": {"commands": commands, "max_output_length": 20000, "timeout_ms": null}, "call_id": call_id, "environment": null});
            for (k, c) in commands.iter().enumerate() {
                let cmd = |ty: &str, extra: Value| {
                    let mut v = json!({"type": ty});
                    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
                        m.extend(e);
                        m.insert("command_index".into(), json!(k));
                        m.insert("output_index".into(), json!(i));
                    }
                    v
                };
                seg.middle.push(Rec::Ev(cmd("response.shell_call_command.added", json!({"command": ""}))));
                for d in split(c, it.pieces) {
                    let mut v = cmd("response.shell_call_command.delta", json!({"delta": d}));
                    if obf {
                        v["obfuscation"] = json!("ob");
                    }
                    seg.middle.push(Rec::Ev(v));
                }
                seg.middle.push(Rec::Ev(cmd("response.shell_call_command.done", json!({"command": c}))));
            }
            (open, done.clone(), done)
        }
        RKind::ApplyPatch if text.is_empty() => {
            let done = json!({"type": "apply_patch_call", "id": id, "status": "completed", "call_id": call_id, "operation": {"type": "delete_file", "path": "old.txt"}});
            (with_status(&done, "in_progress"), done.clone(), done)
        }
        RKind::ApplyPatch => {
            let diff = format!("+{text}\n");
            let done = json!({"type": "apply_patch_call", "id": id, "status": "completed", "call_id": call_id, "operation": {"type": "create_file", "diff": diff, "path": "a.txt"}});
            let mut open = with_status(&done, "in_progress");
            open["operation"]["diff"] = json!("");
            seg.middle = deltas("response.apply_patch_call_operation_diff.delta", i, id, &diff, it.pieces, obf);
            seg.middle.push(Rec::Ev(scoped_ev("response.apply_patch_call_operation_diff.done", i, id, json!({"diff": diff}))));
            (open, done.clone(), done)
        }
        RKind::ToolSearch => {
            let done = json!({"type": "tool_search_call", "id": id, "status": "completed", "arguments": {"query": text}, "call_id": call_id, "execution": "client"});
            let mut open = with_status(&done, "in_progress");
            open["arguments"] = json!({});
            (open, done.clone(), done)
        }
        RKind::McpApproval => {
            let done = json!({"type": "mcp_approval_request", "id": id, "arguments": text, "name": it.name, "server_label": "srv"});
            let mut open = done.clone();
            if it.pieces.is_multiple_of(2) {
                open["arguments"] = json!("");
            }
            (open, done.clone(), done)
        }
        RKind::WebSearch => {
            let open = json!({"type": "web_search_call", "id": id, "status": "in_progress"});
            let done = json!({"type": "web_search_call", "id": id, "status": "completed", "action": {"type": "search", "queries": [text], "query": text}});
            for t in ["in_progress", "searching", "completed"] {
                seg.middle.push(Rec::Ev(scoped_ev(&format!("response.web_search_call.{t}"), i, id, json!({}))));
            }
            (open, done.clone(), done)
        }
        RKind::FileSearch => {
            let open = json!({"type": "file_search_call", "id": id, "status": "in_progress", "queries": []});
            let done = json!({"type": "file_search_call", "id": id, "status": "completed", "queries": [text], "results": null});
            for t in ["in_progress", "searching", "completed"] {
                seg.middle.push(Rec::Ev(scoped_ev(&format!("response.file_search_call.{t}"), i, id, json!({}))));
            }
            (open, done.clone(), done)
        }
        RKind::Other => {
            let done = json!({"type": "function_call_output", "id": id, "status": "completed", "call_id": call_id, "output": text});
            (with_status(&done, "in_progress"), done.clone(), done)
        }
    };
    seg.added = Some(added_ev(i, open));
    seg.done = Some(done_ev(i, done));
    (seg, terminal)
}

fn rhas(s: &ResponsesScenario, q: RQuirk) -> Option<usize> {
    s.quirks.iter().find(|(x, _)| *x == q).map(|(_, at)| *at)
}

/// The index of the `at`-th item (cyclically) that `f` accepts.
fn pick(items: &[RItem], at: usize, f: impl Fn(&RItem) -> bool) -> Option<usize> {
    let c: Vec<usize> = (0..items.len()).filter(|i| f(&items[*i])).collect();
    (!c.is_empty()).then(|| c[at % c.len()])
}

fn lifecycle_ev(ty: &str) -> Value {
    json!({"type": ty, "response": {"id": "resp_1", "object": "response", "status": "in_progress", "output": []}})
}

/// Append `x` to the `key` string of `v`.
fn append(v: &mut Value, key: &str, x: &str) {
    let s = v[key].as_str().unwrap_or("").to_owned();
    v[key] = json!(format!("{s}{x}"));
}

pub fn render_responses(s: &ResponsesScenario) -> Vec<u8> {
    // Quirks that change an item before it is built.
    let mut items = s.items.clone();
    for (q, at) in &s.quirks {
        let target = match q {
            RQuirk::InvalidJsonArguments => pick(&items, *at, |i| i.kind == RKind::Function),
            RQuirk::DoneIncomplete => pick(&items, *at, |i| i.kind.client_run()).filter(|t| items[*t].kind == RKind::Function),
            _ => None,
        };
        if let Some(t) = target {
            items[t].text.pop(); // `{"v":"…"` : no complete JSON value (M5)
        }
        if *q == RQuirk::HoldLimit
            && let Some(t) = pick(&items, *at, |i| i.kind == RKind::Function)
        {
            // 20 deltas of about 1 KiB: each record is under 4 KiB, the hold is over 16 KiB.
            items[t].text = format!("{{\"v\":\"{}\"}}", "y".repeat(20_000));
            items[t].pieces = 20;
        }
    }
    let n = items.len();
    let mut ids: Vec<String> = items.iter().enumerate().map(|(i, it)| format!("{}_{i}", it.kind.id_prefix())).collect();
    let mut call_ids: Vec<Option<String>> = items.iter().enumerate().map(|(i, it)| it.kind.has_call_id().then(|| format!("call_{i}"))).collect();
    if let Some(at) = rhas(s, RQuirk::DuplicateId)
        && n >= 2
    {
        let t = 1 + at % (n - 1);
        ids[t] = ids[t - 1].clone();
    }
    if let Some(at) = rhas(s, RQuirk::DuplicateCallId) {
        let c: Vec<usize> = (0..n).filter(|i| call_ids[*i].is_some()).collect();
        if c.len() >= 2 {
            let k = c[1 + at % (c.len() - 1)];
            call_ids[k] = call_ids[c[0]].clone();
        }
    }
    if let Some(at) = rhas(s, RQuirk::ReservedId)
        && n > 0
    {
        let t = at % n;
        ids[t] = format!("msg_paigasus_{t}");
    }
    let mut segs = Vec::new();
    let mut output = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let (seg, term) = build_item(i, it, &ids[i], call_ids[i].as_deref(), s.obfuscation);
        segs.push(seg);
        output.push(term);
    }
    let mut head = vec![Rec::Ev(lifecycle_ev("response.created")), Rec::Ev(lifecycle_ev("response.in_progress"))];
    let mut terminal_type = "response.completed";
    let mut output_mode = OutputMode::Items;
    let mut terminal_id = "resp_1";
    let mut terminal_present = true;
    let mut tail: Vec<Rec> = Vec::new();
    let mut eof = false;
    let fpick = |at: usize| pick(&items, at, |i| i.kind == RKind::Function);
    for (q, at) in &s.quirks {
        let at = *at;
        match q {
            RQuirk::RepeatedIndex if n > 0 => {
                let t = at % n;
                segs[t].post.push(Rec::Ev(added_ev(t, hidden_call("fc_hidden"))));
            }
            RQuirk::IndexGap if n > 0 => {
                let t = at % n;
                segs[t].post.push(Rec::Ev(added_ev(t + 2, hidden_call("fc_hidden"))));
            }
            RQuirk::ItemNull if n > 0 => {
                let t = at % n;
                segs[t].post.push(Rec::Ev(json!({"type": ADDED_T, "item": null, "output_index": t + 1})));
            }
            RQuirk::DeltaBeforeAdded => {
                if let Some(t) = fpick(at)
                    && let Some(k) = segs[t].middle.iter().position(|r| matches!(r, Rec::Ev(v) if v["type"] == "response.function_call_arguments.delta"))
                {
                    let d = segs[t].middle.remove(k);
                    segs[t].pre.push(d);
                }
            }
            RQuirk::ItemIdMismatch => {
                if let Some(t) = fpick(at)
                    && let Some(v) = segs[t].middles("response.function_call_arguments.done").into_iter().next()
                {
                    v["item_id"] = json!("fc_other");
                }
            }
            RQuirk::WrongItemTypeEvent => {
                if let Some(t) = pick(&items, at, |i| matches!(i.kind, RKind::Custom | RKind::Message)) {
                    let ev = scoped_ev("response.function_call_arguments.delta", t, &ids[t], json!({"delta": "{\"v\":\"CANARY-wt\"}"}));
                    segs[t].middle.insert(0, Rec::Ev(ev));
                }
            }
            RQuirk::TypeChangesAtDone => {
                if let Some(t) = pick(&items, at, |i| i.kind == RKind::Message) {
                    let h = hidden_call(&ids[t]);
                    if let Some(d) = segs[t].done_item() {
                        *d = h.clone();
                    }
                    output[t] = h;
                }
            }
            RQuirk::TypeChangesInTerminal => {
                if let Some(t) = pick(&items, at, |i| i.kind == RKind::Message) {
                    output[t] = hidden_call(&ids[t]);
                }
            }
            RQuirk::TerminalItemNeverStreamed => {
                output.push(hidden_call("fc_hidden"));
                if at % 4 >= 2 {
                    terminal_type = "response.failed";
                }
                if at % 2 == 1 {
                    tail.push(Rec::Ev(json!({"type": "response.unknown_kind"})));
                }
            }
            RQuirk::EarlyOutput => {
                let k = if at.is_multiple_of(2) { 1 } else { 0 };
                if let Some(Rec::Ev(v)) = head.get_mut(k) {
                    v["response"]["output"] = json!([hidden_call("fc_hidden")]);
                }
            }
            RQuirk::TerminalNewName => {
                if let Some(t) = fpick(at) {
                    output[t]["name"] = json!("blocked_hidden");
                }
            }
            RQuirk::TerminalNewNamespace => {
                if let Some(t) = fpick(at) {
                    output[t]["namespace"] = json!("evil");
                }
            }
            RQuirk::TerminalArgsDiffer => {
                if let Some(t) = fpick(at) {
                    output[t]["arguments"] = json!("{\"v\":\"CANARY-td\"}");
                }
            }
            RQuirk::AddedArgumentsNotEmpty => {
                if let Some(t) = fpick(at) {
                    let args = items[t].text.clone();
                    if let Some(a) = segs[t].added_item() {
                        a["arguments"] = json!(args);
                    }
                }
            }
            RQuirk::ArgumentsMismatch | RQuirk::InputMismatch => {
                let (kind, family, key) = if *q == RQuirk::ArgumentsMismatch {
                    (RKind::Function, "response.function_call_arguments", "arguments")
                } else {
                    (RKind::Custom, "response.custom_tool_call_input", "input")
                };
                if let Some(t) = pick(&items, at, |i| i.kind == kind) {
                    match at % 3 {
                        0 => {
                            // A delta that the `*.done` event and the `done` item do not have.
                            let ev = scoped_ev(&format!("{family}.delta"), t, &ids[t], json!({"delta": "x"}));
                            let k = segs[t].middle.len().saturating_sub(1);
                            segs[t].middle.insert(k, Rec::Ev(ev));
                        }
                        1 => {
                            if let Some(v) = segs[t].middles(&format!("{family}.done")).into_iter().next() {
                                append(v, key, "x");
                            }
                        }
                        _ => {
                            if let Some(d) = segs[t].done_item() {
                                append(d, key, "x");
                                output[t] = d.clone();
                            }
                        }
                    }
                }
            }
            RQuirk::DiffMismatch => {
                if let Some(t) = pick(&items, at, |i| i.kind == RKind::ApplyPatch && !i.text.is_empty()) {
                    match at % 3 {
                        0 => {
                            if let Some(d) = segs[t].done_item() {
                                d["operation"]["diff"] = json!("+changed\n");
                                output[t] = d.clone();
                            }
                        }
                        1 => {
                            if let Some(v) = segs[t].middles("response.apply_patch_call_operation_diff.delta").into_iter().next() {
                                append(v, "delta", "x");
                            }
                        }
                        _ => {
                            if let Some(v) = segs[t].middles("response.apply_patch_call_operation_diff.done").into_iter().next() {
                                append(v, "diff", "x");
                            }
                        }
                    }
                }
            }
            RQuirk::ShellCommandMismatch => {
                if let Some(t) = pick(&items, at, |i| i.kind == RKind::Shell) {
                    match at % 4 {
                        0 => {
                            if let Some(d) = segs[t].done_item() {
                                d["action"]["commands"][0] = json!("rm -rf /");
                                output[t] = d.clone();
                            }
                        }
                        1 => {
                            if let Some(v) = segs[t].middles("response.shell_call_command.delta").into_iter().next() {
                                append(v, "delta", "x");
                            }
                        }
                        2 => {
                            if let Some(v) = segs[t].middles("response.shell_call_command.done").into_iter().next() {
                                append(v, "command", "x");
                            }
                        }
                        _ => {
                            if let Some(a) = segs[t].added_item() {
                                a["action"]["commands"] = json!(["rm -rf /"]);
                            }
                        }
                    }
                }
            }
            RQuirk::AddedNotSubset => {
                if let Some(t) = pick(&items, at, |i| matches!(i.kind, RKind::Computer | RKind::LocalShell | RKind::ToolSearch | RKind::McpApproval)) {
                    let kind = items[t].kind;
                    if let Some(a) = segs[t].added_item() {
                        if at.is_multiple_of(2) {
                            a["extra"] = json!("x"); // a field that `done` does not have
                        } else {
                            match kind {
                                RKind::Computer => a["actions"] = json!([{"type": "click", "button": "left"}]),
                                RKind::LocalShell => a["action"]["command"] = json!(["rm", "-rf"]),
                                RKind::ToolSearch => a["arguments"] = json!({"query": "other"}),
                                _ => a["arguments"] = json!("{\"x\":1}"),
                            }
                        }
                    }
                }
            }
            RQuirk::DoneIncomplete => {
                if let Some(t) = pick(&items, at, |i| i.kind.client_run()) {
                    if let Some(d) = segs[t].done_item() {
                        d["status"] = json!("incomplete");
                        output[t] = d.clone();
                    }
                    terminal_type = "response.incomplete";
                }
            }
            RQuirk::AbsentStatus => {
                let kind = if at.is_multiple_of(2) { RKind::Function } else { RKind::Custom };
                if let Some(t) = pick(&items, at / 2, |i| i.kind == kind)
                    && let Some(Value::Object(d)) = segs[t].done_item()
                {
                    d.remove("status");
                    output[t] = Value::Object(d.clone());
                }
            }
            RQuirk::ErrorEventMidItem => {
                if let Some(t) = pick(&items, at, |i| i.kind.client_run()) {
                    let k = at % (segs[t].middle.len() + 1);
                    segs[t].middle.insert(k, Rec::Ev(json!({"type": "error", "code": "server_error", "message": "boom", "param": null})));
                }
            }
            RQuirk::EventAfterDone => {
                if let Some(t) = fpick(at) {
                    let ev = scoped_ev("response.function_call_arguments.delta", t, &ids[t], json!({"delta": "CANARY-late"}));
                    segs[t].post.insert(0, Rec::Ev(ev));
                }
            }
            RQuirk::TerminalOutputAbsent | RQuirk::TerminalOutputNull => {
                output_mode = if *q == RQuirk::TerminalOutputAbsent { OutputMode::Absent } else { OutputMode::Null };
                terminal_type = ["response.completed", "response.incomplete", "response.failed"][at % 3];
            }
            RQuirk::ResponseIdChanged => {
                if at.is_multiple_of(2) {
                    if let Some(Rec::Ev(v)) = head.get_mut(1) {
                        v["response"]["id"] = json!("resp_other");
                    }
                } else {
                    terminal_id = "resp_other";
                }
            }
            RQuirk::FirstNotCreated => {
                if at.is_multiple_of(2) && !head.is_empty() {
                    head.remove(0);
                } else if head.len() >= 2 {
                    head.swap(0, 1);
                }
            }
            RQuirk::RecordAfterTerminal => {
                tail.push(Rec::Ev(if at.is_multiple_of(2) {
                    added_ev(n, hidden_call("fc_hidden"))
                } else {
                    json!({"type": "response.completed", "response": {"id": "resp_1", "object": "response", "status": "completed", "output": [hidden_call("fc_hidden")]}})
                }));
            }
            RQuirk::NoTerminal => terminal_present = false,
            RQuirk::Oversized => {
                let big = format!("CANARY-big{}", "x".repeat(5000));
                if at.is_multiple_of(2) {
                    if let Some(t) = fpick(at) {
                        let ev = scoped_ev("response.function_call_arguments.delta", t, &ids[t], json!({"delta": big}));
                        let k = 1.min(segs[t].middle.len());
                        segs[t].middle.insert(k, Rec::Ev(ev));
                    }
                } else {
                    let mut h = hidden_call("fc_big");
                    h["arguments"] = json!(format!("{{\"v\":\"{big}\"}}"));
                    output.push(h);
                }
            }
            _ => {}
        }
    }
    // Quirks that move or drop whole items, applied last.
    for (q, at) in &s.quirks {
        match q {
            RQuirk::ItemInterleaved => {
                // Pick only from client-run items that have an item after them.
                let c: Vec<usize> = (0..segs.len().saturating_sub(1)).filter(|i| items[*i].kind.client_run()).collect();
                if let Some(t) = (!c.is_empty()).then(|| c[*at % c.len()]) {
                    let next = std::mem::take(&mut segs[t + 1]).flat();
                    segs[t].inner = next;
                    if at.is_multiple_of(2) {
                        segs[t].done = None;
                    }
                }
            }
            RQuirk::FailedWithOpenItem | RQuirk::EofInItem => {
                if let Some(t) = (0..segs.len()).rev().find(|i| items[*i].kind.client_run()) {
                    segs.truncate(t + 1);
                    output.truncate(t + 1);
                    let partial = segs[t].added_item().cloned();
                    segs[t].done = None;
                    if *q == RQuirk::EofInItem {
                        terminal_present = false;
                        eof = true;
                    } else {
                        terminal_type = "response.failed";
                        if at.is_multiple_of(2) {
                            output[t] = partial.unwrap_or(Value::Null);
                        } else {
                            output_mode = OutputMode::Absent;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut recs = head;
    for seg in segs {
        recs.extend(seg.flat());
    }
    let term_at = recs.len();
    if terminal_present {
        let status = terminal_type.trim_start_matches("response.");
        let mut resp = json!({"id": terminal_id, "object": "response", "status": status});
        match output_mode {
            OutputMode::Items => resp["output"] = Value::Array(output),
            OutputMode::Null => resp["output"] = Value::Null,
            OutputMode::Absent => {}
        }
        match terminal_type {
            "response.failed" => resp["error"] = json!({"code": "server_error", "message": "failed"}),
            "response.incomplete" => resp["incomplete_details"] = json!({"reason": "max_output_tokens"}),
            _ => {}
        }
        recs.push(Rec::Ev(json!({"type": terminal_type, "response": resp})));
    }
    recs.extend(tail);
    // Records inserted between the first two records and the terminal event.
    let mid = |at: usize| 2.min(term_at) + at % (term_at.saturating_sub(2) + 1);
    for (q, at) in &s.quirks {
        let at = *at;
        match q {
            RQuirk::EventErrorName => {
                let k = mid(at).min(recs.len().saturating_sub(1));
                if let Some(Rec::Ev(v)) = recs.get(k).cloned() {
                    recs[k] = Rec::Named(v, "error");
                }
            }
            RQuirk::TopLevelError => recs.insert(mid(at), Rec::Ev(json!({"type": "response.error", "error": {"code": "server_error", "message": "boom"}}))),
            RQuirk::DoneTrailing => recs.insert(mid(at), Rec::Raw("[DONE]x")),
            RQuirk::UnknownType => recs.insert(mid(at), Rec::Ev(json!({"type": "response.unknown_kind", "item": hidden_call("fc_hidden")}))),
            RQuirk::KeepaliveWithoutSequence => recs.insert(mid(at), Rec::Bare(json!({"type": "keepalive"}))),
            RQuirk::RepeatedCreated => recs.insert(mid(at), Rec::Ev(lifecycle_ev("response.created"))),
            _ => {}
        }
    }
    // R7: `sequence_number` 0, 1, 2, … on every event but a bare keepalive.
    let mut seq = 0u64;
    for r in &mut recs {
        if let Rec::Ev(v) | Rec::Named(v, _) = r
            && s.sequence
        {
            v["sequence_number"] = json!(seq);
            seq += 1;
        }
    }
    if let Some(at) = rhas(s, RQuirk::SequenceNotIncreasing) {
        let evs: Vec<usize> = (0..recs.len()).filter(|i| matches!(recs[*i], Rec::Ev(_))).collect();
        if !evs.is_empty() {
            let k = evs[at % evs.len()];
            if let Rec::Ev(v) = &mut recs[k] {
                if s.sequence {
                    // The previous number again (or 0): no strict increase.
                    let prev = v["sequence_number"].as_u64().unwrap_or(0).saturating_sub(1);
                    v["sequence_number"] = json!(if at.is_multiple_of(2) { prev } else { 0 });
                    if k == evs[0] {
                        v["sequence_number"] = json!("0"); // the first event: a number that is no integer
                    }
                } else {
                    v["sequence_number"] = json!(7); // on one event only
                }
            }
        }
    }
    let mut body = String::new();
    if rhas(s, RQuirk::Bom).is_some() {
        body.push('\u{feff}');
    }
    for r in &recs {
        let (name, data) = match r {
            Rec::Ev(v) | Rec::Bare(v) => (s.event_lines.then(|| v["type"].as_str().unwrap_or("").to_owned()), v.to_string()),
            Rec::Named(v, n) => (Some((*n).to_owned()), v.to_string()),
            Rec::Raw(d) => (None, (*d).to_owned()),
        };
        if let Some(n) = name {
            body += &format!("event: {n}\n");
        }
        body += &format!("data: {data}\n\n");
    }
    if s.done_marker && !eof {
        body += "data: [DONE]\n\n";
    }
    if rhas(s, RQuirk::NoFinalBlankLine).is_some() {
        body.truncate(body.trim_end_matches('\n').len());
    }
    body.into_bytes()
}

/// A conforming stream with an item of every kind (two streams together hold all 13), for the
/// per-quirk test.
pub fn every_kind_scenario(second: bool) -> ResponsesScenario {
    let kinds: &[RKind] = if second {
        &[
            RKind::McpApproval,
            RKind::ToolSearch,
            RKind::LocalShell,
            RKind::WebSearch,
            RKind::Function,
            RKind::FileSearch,
            RKind::Other,
        ]
    } else {
        &[RKind::Reasoning, RKind::Function, RKind::Message, RKind::Custom, RKind::Shell, RKind::ApplyPatch, RKind::Computer]
    };
    let mut items: Vec<RItem> = kinds.iter().map(|k| RItem::plain(*k)).collect();
    for i in items.iter_mut().filter(|i| i.kind == RKind::ApplyPatch) {
        i.text = "x".to_owned(); // a patch with a diff
    }
    ResponsesScenario {
        items,
        quirks: Vec::new(),
        cuts: Vec::new(),
        sequence: true,
        event_lines: true,
        done_marker: false,
        obfuscation: true,
    }
}
