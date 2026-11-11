// SPDX-License-Identifier: Apache-2.0
//! Rust models of how client SDKs turn stream bytes into tool calls (spec § 8.1).
//!
//! The models read the interceptor's OUTPUT. Each one reproduces its SDK's SSE decoder and its
//! accumulation rules, including the quirks that made the canonical form necessary (spec D9).
//!
//! A model runs to the end of the stream or to the first point where its SDK raises
//! (`Run::error`). `Run::calls` is the client's snapshot at that point. The SDKs surface calls
//! while they stream, before any final result: openai-python and openai-node fire
//! `tool_calls.function.arguments.done` events, and LangChain yields `tool_call_chunks`. So a
//! client can act on a call before its SDK raises, and the snapshot at a raise is checked too
//! ([`check_i1`]).
//!
//! JSON is parsed with serde_json in every model. serde_json and all four SDKs keep the last of
//! two duplicate keys. serde_json rejects `NaN`, which Python's `json` accepts, so a Python model
//! raises on a record its SDK would read; that only makes a model stricter, never more lenient.

use std::collections::BTreeMap;

use paigasus_gateway_core::{Decision, Format, ToolCall, ToolCallKind};
use serde_json::{Map, Value, json};

/// One tool call a client would run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCall {
    /// The choice the client attributes the call to. Always 0 for Anthropic and LangChain.
    pub choice: u32,
    /// `function`, `custom`, `legacy`, `tool_use`, or `other`.
    pub kind: &'static str,
    pub name: String,
    /// The raw argument string, or, when `parsed`, the compact JSON of the value the client parsed
    /// from it (anthropic-python's `input`, LangChain's `args`).
    pub arguments: String,
    pub parsed: bool,
}

/// What one client derives from a byte stream.
#[derive(Debug, Clone, Default)]
pub struct Run {
    pub calls: Vec<ModelCall>,
    /// `Some` when the SDK raises. `calls` then holds the snapshot at that point.
    pub error: Option<String>,
}

pub type Model = fn(&[u8]) -> Result<Vec<ModelCall>, String>;
pub type Runner = fn(&[u8]) -> Run;

fn result(r: Run) -> Result<Vec<ModelCall>, String> {
    match r.error {
        Some(e) => Err(e),
        None => Ok(r.calls),
    }
}

fn openai_python(b: &[u8]) -> Result<Vec<ModelCall>, String> {
    result(run_openai_python(b))
}
fn openai_node(b: &[u8]) -> Result<Vec<ModelCall>, String> {
    result(run_openai_node(b))
}
fn langchain(b: &[u8]) -> Result<Vec<ModelCall>, String> {
    result(run_langchain(b))
}
fn anthropic_python(b: &[u8]) -> Result<Vec<ModelCall>, String> {
    result(run_anthropic_python(b))
}

/// Every model. openai-node raises on an Anthropic stream (it iterates `chunk.choices`), so pair
/// models with the stream format through [`runners`] where that matters.
pub const MODELS: &[(&str, Model)] = &[
    ("openai-python", openai_python),
    ("openai-node", openai_node),
    ("langchain", langchain),
    ("anthropic-python", anthropic_python),
];

const OPENAI_RUNNERS: &[(&str, Runner)] = &[("openai-python", run_openai_python), ("openai-node", run_openai_node), ("langchain", run_langchain)];
const ANTHROPIC_RUNNERS: &[(&str, Runner)] = &[("anthropic-python", run_anthropic_python)];

/// The models of the clients that read `format`.
pub fn runners(format: Format) -> &'static [(&'static str, Runner)] {
    match format {
        Format::AnthropicMessages => ANTHROPIC_RUNNERS,
        _ => OPENAI_RUNNERS,
    }
}

/// The model kind of a decided call.
pub fn kind_of(k: ToolCallKind) -> &'static str {
    match k {
        ToolCallKind::Function => "function",
        ToolCallKind::Custom => "custom",
        ToolCallKind::LegacyFunctionCall => "legacy",
        ToolCallKind::AnthropicToolUse => "tool_use",
        _ => "other",
    }
}

/// A decided call in the models' terms. A `tool_use` call's arguments are compared as parsed JSON,
/// because anthropic-python hands the client the parsed `input`, not the string.
pub fn as_model_call(c: &ToolCall) -> ModelCall {
    ModelCall {
        choice: c.choice,
        kind: kind_of(c.kind),
        name: c.name.clone(),
        arguments: c.arguments.clone(),
        parsed: false,
    }
}

/// Whether `p`, a value parsed from a prefix of some JSON text, can be a prefix of `f`, the value
/// of the whole text: strings and numbers may be cut short, objects and arrays may lack members.
pub fn json_prefix(p: &Value, f: &Value) -> bool {
    match (p, f) {
        (Value::Object(a), Value::Object(b)) => a.iter().all(|(k, v)| b.get(k).is_some_and(|w| json_prefix(v, w))),
        (Value::Array(a), Value::Array(b)) => a.len() <= b.len() && a.iter().zip(b).all(|(x, y)| json_prefix(x, y)),
        (Value::String(a), Value::String(b)) => b.starts_with(a.as_str()),
        (Value::Number(a), Value::Number(b)) => b.to_string().starts_with(&a.to_string()),
        _ => p == f,
    }
}

/// Whether a model call's arguments match a decided call's. The decider sees `{}` for empty
/// function arguments (the core's rule), so a client's raw `""` matches it. A `parsed` model value
/// is compared with the parsed decided string. With `partial` (the snapshot at a raise) the model
/// may hold a prefix of the decided arguments: the client stopped reading inside the call.
fn arguments_match(mc: &ModelCall, decided: &ModelCall, partial: bool) -> bool {
    if mc.kind == "other" {
        return true; // the decided form of kind Other is the raw entry
    }
    if mc.parsed {
        let full = if decided.arguments.is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_str::<Value>(&decided.arguments)
        };
        let (Ok(full), Ok(got)) = (full, serde_json::from_str::<Value>(&mc.arguments)) else {
            return false;
        };
        return if partial { json_prefix(&got, &full) } else { got == full };
    }
    if partial {
        return decided.arguments.starts_with(&mc.arguments);
    }
    mc.arguments == decided.arguments || (mc.arguments.is_empty() && decided.arguments == "{}")
}

/// Invariant I1 as property P1 states it (spec § 2, § 8.1). Every call a client model derives from
/// `out` matches a distinct call the decider allowed, by choice, kind, name and arguments
/// (arguments are not compared for kind `other`, whose decided form is the raw entry).
///
/// A model that raises on `out` is an error unless the interceptor dropped a record or took an
/// `on_failure` path (`core_failed`), or the client already raises on the upstream bytes `input`:
/// a correct interceptor must not make a working client fail. An excused raise is still checked:
/// the client may have acted on calls before it raised, so every call in its snapshot must be a
/// decided, allowed call, of which it may hold only a prefix if it raised inside the call.
pub fn check_i1(format: Format, input: &[u8], out: &[u8], decided: &[(ToolCall, Decision)], core_failed: bool) -> Result<(), String> {
    let allowed: Vec<ModelCall> = decided.iter().filter(|(_, d)| d.is_allow()).map(|(c, _)| as_model_call(c)).collect();
    let show = || String::from_utf8_lossy(out).into_owned();
    for (model, run) in runners(format) {
        let r = run(out);
        let partial = match &r.error {
            None => false,
            Some(_) if core_failed || run(input).error.is_some() => true,
            Some(e) => return Err(format!("{model} raised on the output: {e}\n{}", show())),
        };
        let mut left = allowed.clone();
        for mc in r.calls {
            let hit = left
                .iter()
                .position(|a| a.choice == mc.choice && a.kind == mc.kind && a.name == mc.name && arguments_match(&mc, a, partial));
            match hit {
                Some(i) => {
                    left.remove(i);
                }
                None => return Err(format!("{model} runs an undecided or blocked call {mc:?}\nallowed: {allowed:?}\n{}", show())),
            }
        }
    }
    Ok(())
}

// ---- SSE decoders ------------------------------------------------------------------------------

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sse {
    pub event: Option<String>,
    pub data: String,
}

/// Lines split at CRLF, CR or LF; the last element is the unterminated rest (maybe empty).
fn lines(bytes: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&bytes[start..i]);
                start = i + 1;
            }
            b'\r' => {
                out.push(&bytes[start..i]);
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    (out, &bytes[start..])
}

/// The `SSEDecoder` of openai-python and anthropic-python: lines split at CR, LF and CRLF, each
/// line decoded as strict UTF-8 (a bad byte raises), no BOM handling, a dispatch on a blank line
/// whenever an event name, data, an id or a retry was seen (so `event:` without `data:` dispatches
/// an empty data string), and no dispatch at the end of the stream without a blank line.
/// SOURCE: openai-python <pinned in Task 18>, src/openai/_streaming.py SSEDecoder.decode and
/// _SSELineDecoder; anthropic-python <pinned in Task 18>, src/anthropic/_streaming.py SSEDecoder.
pub fn py_events(bytes: &[u8]) -> Result<Vec<Sse>, String> {
    let (ls, rest) = lines(bytes);
    let mut out = Vec::new();
    let (mut event, mut data, mut last_id, mut retry): (Option<String>, Vec<String>, Option<String>, Option<i64>) = (None, Vec::new(), None, None);
    for raw in ls {
        let line = std::str::from_utf8(raw).map_err(|e| format!("UnicodeDecodeError: {e}"))?;
        if line.is_empty() {
            if event.as_deref().is_none_or(str::is_empty) && data.is_empty() && last_id.as_deref().is_none_or(str::is_empty) && retry.is_none() {
                continue;
            }
            out.push(Sse {
                event: event.take(),
                data: data.join("\n"),
            });
            data.clear();
            retry = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (name, value) = match line.find(':') {
            Some(i) => (&line[..i], line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..])),
            None => (line, ""),
        };
        match name {
            "event" => event = Some(value.to_owned()),
            "data" => data.push(value.to_owned()),
            "id" if !value.contains('\0') => last_id = Some(value.to_owned()),
            "retry" => retry = value.parse().ok().or(retry),
            _ => {}
        }
    }
    std::str::from_utf8(rest).map_err(|e| format!("UnicodeDecodeError: {e}"))?;
    Ok(out)
}

/// openai-node's `SSEDecoder` behind `LineDecoder` and a `TextDecoder`: lossy UTF-8 with the BOM
/// stripped, lines split at CR, LF and CRLF, `event` and `data` fields only, a dispatch on a blank
/// line when an event name or a data line was seen, and a flush of the pending event at the end.
/// SOURCE: openai-node <not pinned: modelled only>, src/core/streaming.ts SSEDecoder.
pub fn node_events(bytes: &[u8]) -> Vec<Sse> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let (ls, rest) = lines(text.as_bytes());
    let mut all: Vec<&[u8]> = ls;
    if !rest.is_empty() {
        all.push(rest);
    }
    let mut out = Vec::new();
    let (mut event, mut data): (Option<String>, Vec<String>) = (None, Vec::new());
    let dispatch = |event: &mut Option<String>, data: &mut Vec<String>, out: &mut Vec<Sse>| {
        if event.as_deref().is_none_or(str::is_empty) && data.is_empty() {
            return;
        }
        out.push(Sse {
            event: event.take(),
            data: data.join("\n"),
        });
        data.clear();
    };
    for raw in all {
        let line = std::str::from_utf8(raw).expect("lossy text is UTF-8");
        if line.is_empty() {
            dispatch(&mut event, &mut data, &mut out);
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (name, value) = match line.find(':') {
            Some(i) => (&line[..i], line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..])),
            None => (line, ""),
        };
        match name {
            "event" => event = Some(value.to_owned()),
            "data" => data.push(value.to_owned()),
            _ => {}
        }
    }
    dispatch(&mut event, &mut data, &mut out);
    out
}

/// A reference WHATWG SSE splitter, independent of the crate's: (event, data) per record.
pub fn records(bytes: &[u8]) -> Vec<(Option<String>, Vec<u8>)> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    let (mut data, mut has, mut event): (Vec<u8>, bool, Option<String>) = (Vec::new(), false, None);
    for line in text.split('\n') {
        if line.is_empty() {
            if has {
                data.pop();
                if !data.is_empty() {
                    out.push((event.take(), std::mem::take(&mut data)));
                }
            }
            data.clear();
            has = false;
            event = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (name, value) = match line.find(':') {
            Some(i) => (&line[..i], line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..])),
            None => (line, ""),
        };
        match name {
            "data" => {
                data.extend_from_slice(value.as_bytes());
                data.push(b'\n');
                has = true;
            }
            "event" => event = Some(value.to_owned()),
            _ => {}
        }
    }
    out
}

pub fn payloads(bytes: &[u8]) -> Vec<Vec<u8>> {
    records(bytes).into_iter().map(|(_, d)| d).collect()
}

// ---- Python and JavaScript value semantics -----------------------------------------------------

fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn js_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// A Python `int` (a `bool` is one), or `None` for anything else.
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|_| i64::MAX)),
        _ => None,
    }
}

/// `list[idx]` with Python's negative indexing; `None` is an `IndexError`.
fn py_get(len: usize, idx: i64) -> Option<usize> {
    let len = i64::try_from(len).expect("small");
    let pos = if idx < 0 { idx + len } else { idx };
    (0..len).contains(&pos).then(|| usize::try_from(pos).expect("in range"))
}

/// Where `list.insert(idx, x)` puts `x`.
fn py_insert_at(len: usize, idx: i64) -> usize {
    let l = i64::try_from(len).expect("small");
    let pos = if idx < 0 { (idx + l).max(0) } else { idx.min(l) };
    usize::try_from(pos).expect("clamped")
}

fn py_scalar(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

fn has_indexed_entries(v: &Value) -> bool {
    matches!(v, Value::Array(a) if a.iter().any(|e| matches!(e, Value::Object(o) if o.contains_key("index"))))
}

fn py_add_numbers(a: &Value, b: &Value) -> Option<Value> {
    let int = |v: &Value| match v {
        Value::Bool(x) => Some(i64::from(*x)),
        Value::Number(n) => n.as_i64(),
        _ => None,
    };
    let float = |v: &Value| match v {
        Value::Bool(x) => Some(f64::from(u8::from(*x))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    if !matches!(a, Value::Number(_) | Value::Bool(_)) || !matches!(b, Value::Number(_) | Value::Bool(_)) {
        return None;
    }
    match (int(a), int(b)) {
        (Some(x), Some(y)) => Some(json!(x.wrapping_add(y))),
        _ => Some(json!(float(a)? + float(b)?)),
    }
}

// ---- openai-python -----------------------------------------------------------------------------

/// openai-python `accumulate_delta`. `index` and `type` are replaced, strings and numbers added,
/// dicts merged, indexed lists merged by `list[index]` (Python indexing, `insert` on IndexError).
/// A value of a different type is kept, not replaced.
/// SOURCE: openai-python <pinned in Task 18>, src/openai/lib/streaming/_deltas.py accumulate_delta.
fn accumulate_delta(acc: &mut Map<String, Value>, delta: &Map<String, Value>) -> Result<(), String> {
    for (k, dv) in delta {
        if acc.get(k).is_none_or(Value::is_null) {
            if has_indexed_entries(dv) {
                acc.insert(k.clone(), Value::Array(Vec::new()));
            } else {
                acc.insert(k.clone(), dv.clone());
                continue;
            }
        }
        if k == "index" || k == "type" {
            acc.insert(k.clone(), dv.clone());
            continue;
        }
        let av = acc.get_mut(k).expect("present");
        if let Some(sum) = py_add_numbers(av, dv) {
            *av = sum;
            continue;
        }
        match (av, dv) {
            (Value::String(a), Value::String(b)) => a.push_str(b),
            (Value::Object(a), Value::Object(b)) => accumulate_delta(a, b)?,
            (Value::Array(a), Value::Array(b)) => {
                if a.iter().all(py_scalar) && (!a.is_empty() || !has_indexed_entries(dv)) {
                    a.extend(b.iter().cloned());
                    continue;
                }
                for e in b {
                    let Value::Object(eo) = e else {
                        return Err("TypeError: list delta entry is not a dictionary".into());
                    };
                    let idx = eo.get("index").ok_or("RuntimeError: list delta entry without `index`")?;
                    let idx = py_int(idx).ok_or("TypeError: list delta entry `index` is not an integer")?;
                    match py_get(a.len(), idx) {
                        Some(p) => {
                            let Value::Object(target) = &mut a[p] else { return Err("TypeError: not handled yet".into()) };
                            accumulate_delta(target, eo)?;
                        }
                        None => a.insert(py_insert_at(a.len(), idx), e.clone()),
                    }
                }
            }
            _ => {} // different types: the accumulated value is kept
        }
    }
    Ok(())
}

fn obj<'a>(v: Option<&'a Value>, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.and_then(Value::as_object).ok_or_else(|| format!("AttributeError: {what}"))
}

fn str_or_empty(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_owned()
}

/// The tool calls of one accumulated OpenAI message. A `function` body is run as a function call
/// whatever `type` says (openai-python appends an unknown type as is, openai-node runs every
/// non-`custom` type's `function`); a `custom` body is a custom call; anything else is `other`.
fn message_calls(choice: u32, m: &Map<String, Value>) -> Vec<ModelCall> {
    let mut out = Vec::new();
    if let Some(Value::Array(tc)) = m.get("tool_calls") {
        for e in tc {
            let Value::Object(e) = e else { continue };
            let (kind, name, arguments) = if let Some(Value::Object(f)) = e.get("function") {
                ("function", str_or_empty(f.get("name")), str_or_empty(f.get("arguments")))
            } else if let Some(Value::Object(c)) = e.get("custom") {
                ("custom", str_or_empty(c.get("name")), str_or_empty(c.get("input")))
            } else {
                ("other", str_or_empty(e.get("type")), Value::Object(e.clone()).to_string())
            };
            out.push(ModelCall {
                choice,
                kind,
                name,
                arguments,
                parsed: false,
            });
        }
    }
    if let Some(Value::Object(fc)) = m.get("function_call") {
        out.push(ModelCall {
            choice,
            kind: "legacy",
            name: str_or_empty(fc.get("name")),
            arguments: str_or_empty(fc.get("arguments")),
            parsed: false,
        });
    }
    out
}

/// A choice snapshot as `_convert_initial_chunk_into_snapshot` and the IndexError branch build it.
fn py_new_choice(c: &Map<String, Value>) -> Result<Value, String> {
    let mut snap = c.clone();
    snap.remove("delta");
    let mut message = Map::new();
    accumulate_delta(&mut message, obj(c.get("delta"), "choice.delta")?)?;
    snap.insert("message".into(), Value::Object(message));
    Ok(Value::Object(snap))
}

fn tool_types(choice: &Value) -> Vec<Option<String>> {
    match choice.pointer("/message/tool_calls") {
        Some(Value::Array(a)) => a.iter().map(|e| e.get("type").and_then(Value::as_str).map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

/// openai-python `ChatCompletionStream`: a chunk whose `object` is not `chat.completion.chunk`
/// is skipped (`_is_valid_chat_completion_chunk_weak`); the first chunk becomes the snapshot by
/// ASSIGNING `choices[choice.index]`; later choices are looked up by LIST POSITION
/// (`choices[choice.index]`, appending on IndexError); deltas merge by `accumulate_delta`; then
/// `message.tool_calls[tool_call_chunk.index]` is read for every fragment, outside the try.
/// SOURCE: openai-python <pinned in Task 18>, src/openai/lib/streaming/chat/_completions.py
/// ChatCompletionStreamState._accumulate_chunk; src/openai/_streaming.py Stream.__stream__.
pub fn run_openai_python(bytes: &[u8]) -> Run {
    let mut snap: Option<Vec<Value>> = None;
    let calls = |snap: &Option<Vec<Value>>| -> Vec<ModelCall> {
        let mut out = Vec::new();
        for (i, c) in snap.iter().flatten().enumerate() {
            if let Some(Value::Object(m)) = c.get("message") {
                out.extend(message_calls(u32::try_from(i).expect("small"), m));
            }
        }
        out
    };
    let mut step = || -> Result<(), String> {
        for sse in py_events(bytes)? {
            if sse.data.starts_with("[DONE]") {
                break;
            }
            let v: Value = serde_json::from_str(&sse.data).map_err(|e| format!("JSONDecodeError: {e}"))?;
            if let Value::Object(m) = &v
                && py_truthy(m.get("error"))
            {
                return Err("APIError: error record".into());
            }
            if v.get("object").and_then(Value::as_str) != Some("chat.completion.chunk") {
                continue;
            }
            let Some(Value::Array(choices)) = v.get("choices") else {
                return Err("AttributeError: chunk.choices".into());
            };
            let choices: Vec<&Map<String, Value>> = choices.iter().map(|c| obj(Some(c), "choice")).collect::<Result<_, _>>()?;
            let Some(list) = snap.as_mut() else {
                let mut list: Vec<Value> = choices.iter().map(|c| Value::Object((*c).clone())).collect();
                for c in &choices {
                    let idx = py_int(c.get("index").ok_or("AttributeError: choice.index")?).ok_or("TypeError: choice.index")?;
                    let pos = py_get(list.len(), idx).ok_or("IndexError: list assignment index out of range")?;
                    list[pos] = py_new_choice(c)?;
                }
                snap = Some(list);
                continue;
            };
            for c in &choices {
                let idx = py_int(c.get("index").ok_or("AttributeError: choice.index")?).ok_or("TypeError: list indices must be integers")?;
                let pos = match py_get(list.len(), idx) {
                    Some(p) => {
                        let before = tool_types(&list[p]);
                        let Some(Value::Object(m)) = list[p].get_mut("message") else {
                            return Err("AttributeError: message".into());
                        };
                        accumulate_delta(m, obj(c.get("delta"), "choice.delta")?)?;
                        let after = tool_types(&list[p]);
                        for (i, t) in before.iter().enumerate() {
                            if t.as_deref() == Some("function") && after.get(i).and_then(Option::as_deref) != Some("function") {
                                return Err("AssertionError: new_tool.type == \"function\"".into());
                            }
                        }
                        p
                    }
                    None => {
                        list.push(py_new_choice(c)?);
                        list.len() - 1
                    }
                };
                if let Some(Value::Array(frags)) = c.get("delta").and_then(|d| d.get("tool_calls")) {
                    let len = match list[pos].pointer("/message/tool_calls") {
                        Some(Value::Array(a)) => a.len(),
                        _ => 0,
                    };
                    for f in frags {
                        let i = f.get("index").and_then(py_int).ok_or("TypeError: tool_call_chunk.index")?;
                        py_get(len, i).ok_or("IndexError: tool_calls[tool_call_chunk.index]")?;
                    }
                }
            }
        }
        Ok(())
    };
    let error = step().err();
    Run { calls: calls(&snap), error }
}

// ---- openai-node -------------------------------------------------------------------------------

#[derive(Default)]
struct NodeTool {
    id: Option<Value>,
    ty: Option<Value>,
    function: Option<(Value, String)>,
    custom: Option<(Value, String)>,
}

#[derive(Default)]
struct NodeChoice {
    finish: Option<Value>,
    role: Option<Value>,
    function_call: Option<Map<String, Value>>,
    tools: BTreeMap<u64, NodeTool>,
}

/// JavaScript `String(v)` for a JSON value, enough for `+=` on an argument string.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}

/// A safe-integer index in `0..max`, as `Number.isSafeInteger(i) && i >= 0 && i < max`.
fn js_index(v: Option<&Value>, max: u64) -> Option<u64> {
    let n = v?.as_number()?;
    let i = n.as_u64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64))?;
    (i < max).then_some(i)
}

/// openai-node `ChatCompletionStream`: choices and tool calls are SPARSE arrays by index;
/// `id`, `type`, `function.name` and `custom.name` are REPLACED when truthy; `arguments` is
/// appended when not null, `custom.input` when truthy. A legacy `function_call` object is taken as
/// is, then its name replaced and its arguments appended. `finalChatCompletion` raises when a
/// choice has no `finish_reason` or no `role`, a tool call has no `type`, or a name or arguments
/// are missing. The SSE layer stops at `[DONE]` and raises on an `error` event or a truthy
/// `error` field.
/// SOURCE: openai-node <not pinned: modelled only>, src/lib/ChatCompletionStream.ts
/// #accumulateChatCompletion and finalizeChatCompletion; src/core/streaming.ts Stream.fromSSEResponse.
pub fn run_openai_node(bytes: &[u8]) -> Run {
    let mut choices: BTreeMap<u64, NodeChoice> = BTreeMap::new();
    let mut step = || -> Result<(), String> {
        for sse in node_events(bytes) {
            if sse.data == "[DONE]" {
                break;
            }
            let v: Value = serde_json::from_str(&sse.data).map_err(|e| format!("SyntaxError: {e}"))?;
            if sse.event.as_deref() == Some("error") || js_truthy(v.get("error")) {
                return Err("APIError: error record".into());
            }
            let Some(Value::Array(cs)) = v.get("choices") else {
                return Err("TypeError: chunk.choices is not iterable".into());
            };
            for c in cs {
                let idx = js_index(c.get("index"), 128).ok_or("OpenAIError: invalid choice index")?;
                let ch = choices.entry(idx).or_default();
                if js_truthy(c.get("finish_reason")) {
                    ch.finish = c.get("finish_reason").cloned();
                }
                let Some(d) = c.get("delta").filter(|d| js_truthy(Some(d))) else { continue };
                if js_truthy(d.get("role")) {
                    ch.role = d.get("role").cloned();
                }
                if let Some(f) = d.get("function_call").filter(|f| js_truthy(Some(f))) {
                    match &mut ch.function_call {
                        Some(s) => {
                            if js_truthy(f.get("name")) {
                                s.insert("name".into(), f["name"].clone());
                            }
                            if js_truthy(f.get("arguments")) {
                                let prev = s.get("arguments").filter(|a| !a.is_null()).map(js_string).unwrap_or_default();
                                s.insert("arguments".into(), Value::String(prev + &js_string(&f["arguments"])));
                            }
                        }
                        None => ch.function_call = Some(f.as_object().cloned().unwrap_or_default()),
                    }
                }
                let Some(tcs) = d.get("tool_calls").filter(|t| js_truthy(Some(t))) else { continue };
                let Value::Array(tcs) = tcs else { return Err("TypeError: tool_calls is not iterable".into()) };
                for e in tcs {
                    let i = js_index(e.get("index"), 128).ok_or("OpenAIError: invalid tool call index")?;
                    let t = ch.tools.entry(i).or_default();
                    if js_truthy(e.get("id")) {
                        t.id = e.get("id").cloned();
                    }
                    if js_truthy(e.get("type")) {
                        t.ty = e.get("type").cloned();
                    }
                    if let Some(cu) = e.get("custom").filter(|x| js_truthy(Some(x))) {
                        let s = t.custom.get_or_insert_with(|| (cu.get("name").filter(|n| !n.is_null()).cloned().unwrap_or(json!("")), String::new()));
                        if js_truthy(cu.get("name")) {
                            s.0 = cu["name"].clone();
                        }
                        if js_truthy(cu.get("input")) {
                            s.1.push_str(&js_string(&cu["input"]));
                        }
                    }
                    if let Some(f) = e.get("function").filter(|x| js_truthy(Some(x))) {
                        let s = t.function.get_or_insert_with(|| (f.get("name").filter(|n| !n.is_null()).cloned().unwrap_or(json!("")), String::new()));
                        if js_truthy(f.get("name")) {
                            s.0 = f["name"].clone();
                        }
                        if let Some(a) = f.get("arguments").filter(|a| !a.is_null()) {
                            s.1.push_str(&js_string(a));
                        }
                    }
                }
            }
        }
        Ok(())
    };
    let mut error = step().err();
    let mut calls = Vec::new();
    for (idx, ch) in &choices {
        let choice = u32::try_from(*idx).expect("below 128");
        if error.is_none() && ch.finish.is_none() {
            error = Some(format!("OpenAIError: missing finish_reason for choice {idx}"));
        }
        if error.is_none() && ch.role.is_none() {
            error = Some(format!("OpenAIError: missing role for choice {idx}"));
        }
        for t in ch.tools.values() {
            let custom = t.ty.as_ref().and_then(Value::as_str) == Some("custom");
            if error.is_none() && t.ty.is_none() {
                error = Some("OpenAIError: missing tool_calls[].type".into());
            }
            let (kind, body) = if custom { ("custom", &t.custom) } else { ("function", &t.function) };
            match body {
                Some((name, arguments)) => calls.push(ModelCall {
                    choice,
                    kind,
                    name: js_string(name),
                    arguments: arguments.clone(),
                    parsed: false,
                }),
                None if error.is_none() => error = Some(format!("OpenAIError: missing tool_calls[].{kind}.name")),
                None => {}
            }
        }
        if let Some(fc) = &ch.function_call {
            if error.is_none() && (fc.get("arguments").is_none_or(Value::is_null) || !js_truthy(fc.get("name"))) {
                error = Some("OpenAIError: missing function_call.name or .arguments".into());
            }
            calls.push(ModelCall {
                choice,
                kind: "legacy",
                name: fc.get("name").map(js_string).unwrap_or_default(),
                arguments: fc.get("arguments").filter(|a| !a.is_null()).map(js_string).unwrap_or_default(),
                parsed: false,
            });
        }
    }
    Run { calls, error }
}

// ---- LangChain ---------------------------------------------------------------------------------

/// openai-python's model types dump every declared field, so an absent field is `None`.
fn dumped(v: Option<&Value>) -> Value {
    v.cloned().unwrap_or(Value::Null)
}

/// LangChain `merge_dicts` on two flat dicts: `None` on the left is replaced, `None` on the right
/// ignored, strings concatenated except an equal `id`, equal values kept, a type change raises.
/// SOURCE: langchain-core <not pinned: modelled only>, langchain_core/utils/_merge.py merge_dicts.
fn lc_merge_dicts(left: &mut Map<String, Value>, right: &Map<String, Value>) -> Result<(), String> {
    for (k, rv) in right {
        let Some(lv) = left.get_mut(k) else {
            left.insert(k.clone(), rv.clone());
            continue;
        };
        if lv.is_null() && !rv.is_null() {
            *lv = rv.clone();
            continue;
        }
        if rv.is_null() || *lv == *rv && !lv.is_string() {
            continue;
        }
        let same_type = std::mem::discriminant(lv) == std::mem::discriminant(rv) && (lv.is_i64() || lv.is_u64()) == (rv.is_i64() || rv.is_u64());
        if !same_type {
            return Err(format!("TypeError: additional_kwargs[{k:?}] already exists with a different type"));
        }
        match (lv, rv) {
            (Value::String(l), Value::String(r)) => {
                if !(k == "id" && l == r) {
                    l.push_str(r);
                }
            }
            (Value::Object(l), Value::Object(r)) => lc_merge_dicts(l, r)?,
            (Value::Number(l), Value::Number(r)) if k != "index" => {
                *l = py_add_numbers(&Value::Number(l.clone()), &Value::Number(r.clone()))
                    .and_then(|v| v.as_number().cloned())
                    .ok_or("TypeError")?;
            }
            (Value::Number(l), Value::Number(r)) => *l = r.clone(), // `index` is replaced
            (Value::Array(l), Value::Array(r)) => l.extend(r.iter().cloned()),
            _ => return Err(format!("TypeError: unsupported type for {k:?}")),
        }
    }
    Ok(())
}

/// LangChain over the OpenAI stream: `ChatOpenAI._stream` takes `choices[0]` of EVERY chunk and
/// ignores `choice.index`. `_convert_delta_to_message_chunk` reads `rtc["function"].get(...)` of
/// every model-dumped tool call (a missing `function` is `None`, so a custom call raises
/// AttributeError) and catches only KeyError. The chunks are summed: `tool_call_chunks` merge by
/// `index` VALUE when the ids do not conflict (`merge_lists`), with `name` and `args`
/// concatenated; the very first chunk's list is copied without merging. A legacy
/// `function_call` goes to `additional_kwargs` and merges by `merge_dicts`.
/// SOURCE: langchain-openai <not pinned: modelled only>, langchain_openai/chat_models/base.py
/// _convert_chunk_to_generation_chunk and _convert_delta_to_message_chunk; langchain-core,
/// langchain_core/utils/_merge.py merge_lists, langchain_core/messages/ai.py add_ai_message_chunks.
pub fn run_langchain(bytes: &[u8]) -> Run {
    let mut tcc: Option<Vec<Map<String, Value>>> = None; // None until the first generation chunk
    let mut kwargs: Map<String, Value> = Map::new();
    let mut step = || -> Result<(), String> {
        for sse in py_events(bytes)? {
            if sse.data.starts_with("[DONE]") {
                break;
            }
            let v: Value = serde_json::from_str(&sse.data).map_err(|e| format!("JSONDecodeError: {e}"))?;
            if let Value::Object(m) = &v
                && py_truthy(m.get("error"))
            {
                return Err("APIError: error record".into());
            }
            if v.get("type").and_then(Value::as_str) == Some("content.delta") {
                continue;
            }
            let mut choices = v.get("choices").and_then(Value::as_array).cloned().unwrap_or_default();
            if choices.is_empty() {
                choices = v.pointer("/chunk/choices").and_then(Value::as_array).cloned().unwrap_or_default();
            }
            let mut new_chunks: Vec<Map<String, Value>> = Vec::new();
            let mut new_kwargs = Map::new();
            if let Some(choice) = choices.first() {
                let choice = obj(Some(choice), "choice")?;
                let delta = choice.get("delta").ok_or("KeyError: 'delta'")?;
                if delta.is_null() {
                    continue;
                }
                let delta = obj(Some(delta), "delta")?;
                let fc = delta.get("function_call").filter(|f| !f.is_null());
                if let Some(fc) = fc {
                    let fc = obj(Some(fc), "function_call")?;
                    let mut d = Map::new();
                    d.insert("arguments".into(), dumped(fc.get("arguments")));
                    let name = dumped(fc.get("name"));
                    d.insert("name".into(), if name.is_null() { json!("") } else { name });
                    for (k, x) in fc {
                        d.entry(k.clone()).or_insert_with(|| x.clone());
                    }
                    new_kwargs.insert("function_call".into(), Value::Object(d));
                }
                if let Some(Value::Array(raw)) = delta.get("tool_calls").filter(|t| py_truthy(Some(t))) {
                    let mut built = Vec::new();
                    let mut key_error = false;
                    for rtc in raw {
                        let rtc = obj(Some(rtc), "tool call")?;
                        let f = dumped(rtc.get("function"));
                        let Value::Object(f) = f else {
                            return Err("AttributeError: 'NoneType' object has no attribute 'get'".into());
                        };
                        let Some(index) = rtc.get("index") else {
                            key_error = true;
                            break;
                        };
                        let mut e = Map::new();
                        e.insert("name".into(), dumped(f.get("name")));
                        e.insert("args".into(), dumped(f.get("arguments")));
                        e.insert("id".into(), dumped(rtc.get("id")));
                        e.insert("index".into(), index.clone());
                        built.push(e);
                    }
                    if !key_error {
                        new_chunks = built;
                    }
                }
            }
            match tcc.as_mut() {
                None => tcc = Some(new_chunks),
                Some(merged) => {
                    for e in new_chunks {
                        let idx = e.get("index").filter(|i| py_int(i).is_some());
                        let hit = idx.and_then(|i| {
                            merged.iter().position(|l| {
                                let id_ok = |x: Option<&Value>| x.is_none_or(|x| x.is_null() || x.as_str() == Some(""));
                                l.get("index").is_some_and(|li| py_int(li) == py_int(i)) && (id_ok(l.get("id")) || id_ok(e.get("id")) || l.get("id") == e.get("id"))
                            })
                        });
                        match hit {
                            Some(p) => lc_merge_dicts(&mut merged[p], &e)?,
                            None => merged.push(e),
                        }
                    }
                }
            }
            lc_merge_dicts(&mut kwargs, &new_kwargs)?;
        }
        Ok(())
    };
    let error = step().err();
    // `AIMessageChunk.init_tool_calls`: empty args are `{}`, others go through
    // `parse_partial_json`, and only a dict becomes a tool call; the rest are invalid_tool_calls,
    // which nothing runs.
    let mut calls: Vec<ModelCall> = tcc
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let raw = str_or_empty(e.get("args"));
            let args = if raw.is_empty() { json!({}) } else { lc_parse_partial_json(&raw)? };
            args.is_object().then(|| ModelCall {
                choice: 0,
                kind: "function",
                name: str_or_empty(e.get("name")),
                arguments: args.to_string(),
                parsed: true,
            })
        })
        .collect();
    if let Some(Value::Object(fc)) = kwargs.get("function_call") {
        calls.push(ModelCall {
            choice: 0,
            kind: "legacy",
            name: str_or_empty(fc.get("name")),
            arguments: str_or_empty(fc.get("arguments")),
            parsed: false,
        });
    }
    Run { calls, error }
}

/// LangChain `parse_partial_json`: a complete value as is; otherwise unterminated strings are
/// closed and open brackets closed in order, then trailing characters are dropped until the text
/// parses. A mismatched closing bracket gives `None`. Every candidate the SDK skips with its
/// decoder-offset shortcut fails the same way, so dropping one character at a time is equivalent.
/// SOURCE: langchain-core <not pinned: modelled only>, langchain_core/utils/json.py parse_partial_json.
pub fn lc_parse_partial_json(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(s) {
        return Some(v);
    }
    let (mut new, mut stack): (Vec<String>, Vec<char>) = (Vec::new(), Vec::new());
    let (mut inside, mut escaped) = (false, false);
    for ch in s.chars() {
        let mut out = ch.to_string();
        if inside {
            if ch == '"' && !escaped {
                inside = false;
            } else if ch == '\n' && !escaped {
                out = "\\n".to_owned();
            } else if ch == '\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
        } else if ch == '"' {
            inside = true;
            escaped = false;
        } else if ch == '{' {
            stack.push('}');
        } else if ch == '[' {
            stack.push(']');
        } else if ch == '}' || ch == ']' {
            if stack.last() == Some(&ch) {
                stack.pop();
            } else {
                return None;
            }
        }
        new.push(out);
    }
    if inside {
        if escaped {
            new.pop();
        }
        new.push("\"".to_owned());
    }
    let closing: String = stack.iter().rev().collect();
    while !new.is_empty() {
        if let Ok(v) = serde_json::from_str(&format!("{}{closing}", new.concat())) {
            return Some(v);
        }
        new.pop();
    }
    None
}

// ---- anthropic-python --------------------------------------------------------------------------

/// jiter `from_json(buf, partial_mode=True)`: a truncated value is closed at the end of the input,
/// an unfinished string or literal is dropped (with its key), anything invalid raises.
struct Partial<'a> {
    b: &'a [u8],
    i: usize,
}

impl Partial<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }
    /// `Ok(None)`: the input ended inside this value.
    fn value(&mut self) -> Result<Option<Value>, String> {
        self.ws();
        if self.eof() {
            return Ok(None);
        }
        match self.b[self.i] {
            b'{' => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    self.ws();
                    if self.eof() {
                        return Ok(Some(Value::Object(m)));
                    }
                    if self.b[self.i] == b'}' {
                        self.i += 1;
                        return Ok(Some(Value::Object(m)));
                    }
                    let Some(Value::String(k)) = self.value()? else { return Ok(Some(Value::Object(m))) };
                    self.ws();
                    if self.eof() {
                        return Ok(Some(Value::Object(m)));
                    }
                    if self.b[self.i] != b':' {
                        return Err("ValueError: expected ':'".into());
                    }
                    self.i += 1;
                    let Some(v) = self.value()? else { return Ok(Some(Value::Object(m))) };
                    m.insert(k, v);
                    self.ws();
                    if self.eof() {
                        return Ok(Some(Value::Object(m)));
                    }
                    match self.b[self.i] {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Ok(Some(Value::Object(m)));
                        }
                        _ => return Err("ValueError: expected ',' or '}'".into()),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.ws();
                    if self.eof() {
                        return Ok(Some(Value::Array(a)));
                    }
                    if self.b[self.i] == b']' {
                        self.i += 1;
                        return Ok(Some(Value::Array(a)));
                    }
                    let Some(v) = self.value()? else { return Ok(Some(Value::Array(a))) };
                    a.push(v);
                    self.ws();
                    if self.eof() {
                        return Ok(Some(Value::Array(a)));
                    }
                    match self.b[self.i] {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Ok(Some(Value::Array(a)));
                        }
                        _ => return Err("ValueError: expected ',' or ']'".into()),
                    }
                }
            }
            b'"' => {
                let start = self.i;
                self.i += 1;
                while self.i < self.b.len() {
                    match self.b[self.i] {
                        b'\\' => self.i += 2,
                        b'"' => {
                            self.i += 1;
                            return serde_json::from_slice(&self.b[start..self.i]).map(Some).map_err(|e| format!("ValueError: {e}"));
                        }
                        _ => self.i += 1,
                    }
                }
                Ok(None)
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    self.i += 1;
                }
                let tok = &self.b[start..self.i];
                match serde_json::from_slice::<Value>(tok) {
                    Ok(v) if !v.is_object() && !v.is_array() && !v.is_string() => Ok(Some(v)),
                    _ if self.eof() && [&b"true"[..], b"false", b"null"].iter().any(|l| l.starts_with(tok)) => Ok(None),
                    _ => Err(format!("ValueError: invalid token {:?}", String::from_utf8_lossy(tok))),
                }
            }
        }
    }
}

pub fn partial_json(buf: &str) -> Result<Value, String> {
    let mut p = Partial { b: buf.as_bytes(), i: 0 };
    let v = p.value()?.ok_or("ValueError: EOF while parsing a value")?;
    p.ws();
    if !p.eof() {
        return Err("ValueError: trailing characters".into());
    }
    Ok(v)
}

const NOT_RUN_BY_CLIENT: [&str; 5] = ["text", "thinking", "redacted_thinking", "server_tool_use", "mcp_tool_use"];

/// anthropic-python `MessageStream`. The raw stream SKIPS records whose `event:` is absent or
/// `ping`, raises on `error`, skips empty data, and decodes the rest by `data.type` (the event
/// name is not compared with it). The first event must be `message_start`, whose
/// `message.content` seeds the snapshot. `content_block_start` APPENDS a block whatever its index;
/// deltas and stops look up `content[event.index]` by Python list indexing. An `input_json_delta`
/// for a `tool_use` or `server_tool_use` block is appended to a buffer keyed by `event.index` and
/// partial-parsed into `input`. Nothing stops at `message_stop`.
/// SOURCE: anthropic-python <pinned in Task 18>, src/anthropic/lib/streaming/_messages.py
/// accumulate_event; src/anthropic/_streaming.py Stream.__stream__.
pub fn run_anthropic_python(bytes: &[u8]) -> Run {
    let mut snap: Option<Vec<Map<String, Value>>> = None;
    let mut bufs: BTreeMap<i64, String> = BTreeMap::new();
    let mut step = || -> Result<(), String> {
        for sse in py_events(bytes)? {
            match sse.event.as_deref() {
                None | Some("ping") => continue,
                Some("error") => return Err("APIStatusError: error event".into()),
                _ => {}
            }
            if sse.data.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&sse.data).map_err(|e| format!("JSONDecodeError: {e}"))?;
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            let Some(content) = snap.as_mut() else {
                if ty != "message_start" {
                    return Err(format!("RuntimeError: Unexpected event order, got {ty} before \"message_start\""));
                }
                let blocks = match v.pointer("/message/content") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(a)) => a.iter().map(|b| obj(Some(b), "content block").cloned()).collect::<Result<_, _>>()?,
                    Some(_) => return Err("TypeError: message.content".into()),
                };
                obj(v.get("message"), "event.message")?;
                snap = Some(blocks);
                continue;
            };
            match ty {
                "content_block_start" => content.push(obj(v.get("content_block"), "event.content_block")?.clone()),
                "content_block_delta" => {
                    let idx = v.get("index").and_then(py_int).ok_or("TypeError: event.index")?;
                    let pos = py_get(content.len(), idx).ok_or("IndexError: list index out of range")?;
                    let delta = obj(v.get("delta"), "event.delta")?;
                    let block_ty = content[pos].get("type").and_then(Value::as_str).unwrap_or("");
                    if delta.get("type").and_then(Value::as_str) == Some("input_json_delta") && (block_ty == "tool_use" || block_ty == "server_tool_use") {
                        let pj = delta.get("partial_json").and_then(Value::as_str).ok_or("TypeError: partial_json is not a string")?;
                        let buf = bufs.entry(idx).or_default();
                        buf.push_str(pj);
                        if !buf.is_empty() {
                            let input = partial_json(buf)?;
                            content[pos].insert("input".into(), input);
                        }
                    }
                }
                "content_block_stop" => {
                    let idx = v.get("index").and_then(py_int).ok_or("TypeError: event.index")?;
                    py_get(content.len(), idx).ok_or("IndexError: list index out of range")?;
                }
                "message_delta" => {
                    obj(v.get("delta"), "event.delta")?;
                    obj(v.get("usage"), "event.usage")?;
                }
                _ => {}
            }
        }
        Ok(())
    };
    let error = step().err();
    let calls = snap
        .unwrap_or_default()
        .iter()
        .filter_map(|b| {
            let t = b.get("type").and_then(Value::as_str).unwrap_or("");
            if NOT_RUN_BY_CLIENT.contains(&t) || t.ends_with("_tool_result") {
                return None;
            }
            let (kind, arguments) = if t == "tool_use" {
                ("tool_use", b.get("input").cloned().unwrap_or(Value::Null).to_string())
            } else {
                ("other", Value::Object(b.clone()).to_string())
            };
            Some(ModelCall {
                choice: 0,
                kind,
                name: b.get("name").and_then(Value::as_str).unwrap_or(t).to_owned(),
                arguments,
                parsed: kind == "tool_use",
            })
        })
        .collect();
    Run { calls, error }
}
