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
    NoDelta,
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
            Quirk::NoChoicesKey | Quirk::NoDelta => {
                // SMA-749 D2 strips the BOM, so a client that ignores the BOM-prefixed first line
                // would read it after the strip (known limitation, spec § 9). Keep it off line 0.
                let at = if pos == 0 && has(s, Quirk::Bom).is_some() { 1 } else { pos };
                if at <= lines.len() {
                    let rec = if *q == Quirk::NoChoicesKey { r#"{"object":"chat.completion.chunk","id":"q"}"#.to_owned() } else { chunk(json!([{"index":0}])).to_string() };
                    lines.insert(at, format!("data: {rec}\n\n"));
                }
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
    Tool { name: &'static str, args: String, pieces: usize },
    Server,
}

#[derive(Debug, Clone)]
pub struct AnthropicScenario {
    pub blocks: Vec<Block>,
    pub quirks: Vec<(AQuirk, usize)>,
    pub cuts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AQuirk {
    EventMismatch,
    PrefilledInput,
    PrefilledMessage,
    IndexReuse,
    NoEventLines,
    Bom,
    AfterStop,
    DuplicateKey,
    Oversized,
    /// The next block starts before the previous one stops (`block_interleave`).
    Interleave,
    /// A tool's last argument piece is lost, so its JSON is incomplete (`incomplete_arguments`).
    TruncatedArgs,
    /// A delta with index -1 (anthropic-python's `content[-1]` is the last block).
    NegativeIndex,
    /// A delta for a block after its stop.
    DeltaAfterStop,
    /// Every tool's input gains PascalCase, `constructor` and `prototype` keys (application data,
    /// so conforming; I-1).
    CapitalisedArgKeys,
}

pub fn anthropic_scenario() -> impl Strategy<Value = AnthropicScenario> {
    let block = prop_oneof![
        "[a-z ]{1,10}".prop_map(Block::Text),
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
    ];
    let quirk = prop_oneof![
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
    ];
    (
        prop::collection::vec(block, 0..5),
        prop::collection::vec((quirk, 0usize..64), 0..3),
        prop::collection::vec(0usize..100_000, 0..6),
    )
        .prop_map(|(blocks, quirks, cuts)| AnthropicScenario { blocks, quirks, cuts })
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
                let mut parts = split(&args, *pieces);
                if truncated == Some(i) {
                    let last = parts.pop().unwrap_or_default();
                    parts.push(last[..last.len().saturating_sub(1)].to_owned());
                }
                for p in parts {
                    evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":p}}));
                }
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
    evs.push(json!({"type":"message_delta","delta":{"stop_reason": if any_tool { "tool_use" } else { "end_turn" },"stop_sequence":null},"usage":{"output_tokens":1}}));
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
                frames.len().saturating_sub(2).max(1),
                format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"CANARY-late\""}})
                ),
            ),
            AQuirk::Bom => prefix = "\u{feff}".to_owned(),
            AQuirk::AfterStop => suffix = format!("event: content_block_start\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({}))),
            AQuirk::NoEventLines | AQuirk::Interleave | AQuirk::TruncatedArgs | AQuirk::CapitalisedArgKeys => {}
        }
    }
    format!("{prefix}{}{suffix}", frames.concat()).into_bytes()
}
