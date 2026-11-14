// SPDX-License-Identifier: Apache-2.0
//! Rust models of how the Responses API clients turn stream bytes into tool calls (SMA-769
//! § 10.1).
//!
//! Each model reads the interceptor's OUTPUT and derives the calls from EVERY surface its SDK
//! exposes, because the SDKs read a call from different events (§ 3.2, R-a). One call that two
//! surfaces show the same way is one model call ([`dedupe`]); two surfaces that show it
//! differently are two calls, and only one of them can match the decided call.
//!
//! A model stops at the first point where its SDK raises (`Run::error`), with the calls the
//! client saw up to that point. A model reproduces a raise only where the source makes it
//! certain. A model that raises later than its SDK sees more calls, which is stricter; a model
//! that raised earlier would see fewer calls, which would be more lenient.
//!
//! An `output_item.added` item is never a call in these models: it is a partial view by design
//! (spec § 15, A2 and A5).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use paigasus_gateway_core::ToolCall;
use serde_json::{Map, Value, json};

use super::oracle::{ModelCall, Run, Runner, js_truthy, lc_parse_partial_json, node_events, py_events, py_get, py_int, py_truthy, responses_kind_of};

/// The models of the Responses clients, in the order [`super::oracle::check_i1`] runs them.
pub const RESPONSES_RUNNERS: &[(&str, Runner)] = &[
    ("openai-python-responsestream", run_python_stream),
    ("openai-python-raw", run_python_raw),
    ("openai-node-responses", run_node),
    ("langchain-responses", run_langchain),
];

const ADDED: &str = "response.output_item.added";
const DONE: &str = "response.output_item.done";
const LIFECYCLE: [&str; 6] = [
    "response.created",
    "response.queued",
    "response.in_progress",
    "response.completed",
    "response.failed",
    "response.incomplete",
];
const INTERIM: [&str; 3] = ["response.created", "response.queued", "response.in_progress"];
const TERMINAL: [&str; 3] = ["response.completed", "response.failed", "response.incomplete"];

/// The item types that no client runs: tool-free content and provider-run items (spec § 6.2).
const NOT_RUN_BY_CLIENT: [&str; 17] = [
    "message",
    "reasoning",
    "compaction",
    "web_search_call",
    "file_search_call",
    "code_interpreter_call",
    "image_generation_call",
    "mcp_call",
    "mcp_list_tools",
    "program",
    "program_output",
    "additional_tools",
    "tool_search_output",
    "shell_call_output",
    "local_shell_call_output",
    "computer_call_output",
    "apply_patch_call_output",
];

/// The model kind of an item type, or `None` for an item that no client runs.
fn kind_of_type(ty: &str) -> Option<&'static str> {
    if NOT_RUN_BY_CLIENT.contains(&ty) {
        return None;
    }
    Some(match ty {
        "function_call" => "function",
        "custom_tool_call" => "custom",
        "computer_call" => "computer",
        "local_shell_call" => "local_shell",
        "shell_call" => "shell",
        "apply_patch_call" => "apply_patch",
        "tool_search_call" => "tool_search",
        "mcp_approval_request" => "mcp_approval_request",
        _ => "item",
    })
}

/// The fixed name of a built-in kind (spec § 5).
fn fixed_name(kind: &str) -> Option<&'static str> {
    match kind {
        "computer" => Some("computer"),
        "local_shell" => Some("local_shell"),
        "shell" => Some("shell"),
        "apply_patch" => Some("apply_patch"),
        "tool_search" => Some("tool_search"),
        _ => None,
    }
}

/// An optional string field: absent or `null` is `None`, any other non-string is `Err`.
fn opt_text(v: Option<&Value>) -> Result<Option<String>, ()> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

/// `model_dump(exclude_none=True)`: every object field whose value is `null` is dropped, at any
/// depth. A `null` element of an array stays.
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// The compact JSON of an item without `status`, as the decided form of an item kind (spec § 5).
fn item_json(o: &Map<String, Value>, normalised: bool) -> String {
    let mut m = o.clone();
    m.remove("status");
    let mut v = Value::Object(m);
    if normalised {
        strip_nulls(&mut v);
    }
    v.to_string()
}

/// The call a client runs for one output item (spec § 5, § 6.2), or `None` for an item that no
/// client runs and for a value that is no item. `normalised`: the SDK dropped the `null` fields.
///
/// A function or custom call whose name, body, call id or namespace has another JSON type than
/// the SDK reads becomes kind `item`, the whole item: no decided call of that kind can match it,
/// so a call the SDK cannot read as decided fails closed.
fn item_call(item: &Value, position: Option<u32>, normalised: bool) -> Option<ModelCall> {
    let Value::Object(o) = item else { return None };
    let ty = o.get("type").and_then(Value::as_str);
    let kind = match ty {
        Some(t) => kind_of_type(t)?,
        None => "item",
    };
    let call = |kind: &'static str, name: String, arguments: String, parsed: bool, id: Option<String>, namespace: Option<String>| ModelCall {
        choice: 0,
        kind,
        name,
        arguments,
        parsed,
        position,
        id,
        namespace,
    };
    let other = || {
        let id = match (opt_text(o.get("call_id")), opt_text(o.get("id"))) {
            (Ok(Some(c)), _) => Some(c),
            (_, Ok(i)) => i,
            _ => None,
        };
        call("item", ty.unwrap_or("").to_owned(), item_json(o, normalised), normalised, id, None)
    };
    let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(match kind {
        "function" | "custom" => {
            let body = if kind == "function" { "arguments" } else { "input" };
            let (Some(name), Some(arguments), Some(id), Ok(namespace)) = (text("name"), text(body), text("call_id"), opt_text(o.get("namespace"))) else {
                return Some(other());
            };
            call(kind, name, arguments, false, Some(id), namespace)
        }
        "item" => other(),
        _ => {
            let name = match fixed_name(kind) {
                Some(n) => n.to_owned(),
                None => match text("name") {
                    Some(n) => n,
                    None => return Some(other()),
                },
            };
            let id = match kind {
                "mcp_approval_request" => text("id"),
                _ => match opt_text(o.get("call_id")) {
                    Ok(c) => c,
                    Err(()) => return Some(other()),
                },
            };
            let namespace = match kind {
                "mcp_approval_request" => match opt_text(o.get("server_label")) {
                    Ok(ns) => ns,
                    Err(()) => return Some(other()),
                },
                _ => None,
            };
            call(kind, name, item_json(o, normalised), normalised, id, namespace)
        }
    })
}

/// The calls of an output list, each at its list position.
fn output_calls(output: &[Value], normalised: bool, calls: &mut Vec<ModelCall>) {
    for (i, item) in output.iter().enumerate() {
        calls.extend(item_call(item, Some(u32::try_from(i).expect("small")), normalised));
    }
}

/// One call on several surfaces, shown the same way, is one call. Two different views stay two
/// calls.
fn dedupe(calls: Vec<ModelCall>) -> Vec<ModelCall> {
    let mut out: Vec<ModelCall> = Vec::new();
    for c in calls {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// An `output_index` as a position. A value that is no `u32` gets `u32::MAX`, which no decided
/// call has.
fn position_of(v: Option<&Value>) -> Option<u32> {
    Some(v.and_then(py_int).and_then(|i| u32::try_from(i).ok()).unwrap_or(u32::MAX))
}

// ---- matching ----------------------------------------------------------------------------------

/// A decided Responses call in the models' terms (spec § 5).
pub fn as_model_call(c: &ToolCall) -> ModelCall {
    ModelCall {
        choice: c.choice,
        kind: responses_kind_of(c.kind),
        name: c.name.clone(),
        arguments: c.arguments.clone(),
        parsed: false,
        position: Some(c.position),
        id: c.id.clone(),
        namespace: c.namespace.clone(),
    }
}

/// Whether a model call matches a decided call (spec § 10.1, ruling F9): the same choice, kind,
/// name and call id; the same `output_index` where the SDK shows one; the same namespace; and the
/// same arguments. A function's raw `""`
/// matches the decided `{}` (749 § 11). A LangChain tool call (`parsed`) holds the parsed
/// arguments, and a custom call's `{"__arg1": input}`. An item kind matches as the JSON of the
/// item without `status`, after the SDK's `null` drop when `parsed`. The match is exact: a prefix
/// of allowed arguments is a failure (757 D2).
pub fn matches(mc: &ModelCall, d: &ModelCall) -> bool {
    if mc.choice != d.choice || mc.kind != d.kind || mc.name != d.name || mc.id != d.id {
        return false;
    }
    if mc.position.is_some() && mc.position != d.position {
        return false;
    }
    if mc.namespace != d.namespace {
        return false;
    }
    let parse = |s: &str| serde_json::from_str::<Value>(s).ok();
    match mc.kind {
        "function" if mc.parsed => {
            let full = if d.arguments.is_empty() { Some(json!({})) } else { parse(&d.arguments) };
            full.is_some() && parse(&mc.arguments) == full
        }
        "function" => mc.arguments == d.arguments || (mc.arguments.is_empty() && d.arguments == "{}"),
        "custom" if mc.parsed => parse(&mc.arguments) == Some(json!({"__arg1": d.arguments})),
        "custom" => mc.arguments == d.arguments,
        _ => {
            let (Some(mut got), Some(mut want)) = (parse(&mc.arguments), parse(&d.arguments)) else {
                return false;
            };
            if mc.parsed {
                strip_nulls(&mut got);
                strip_nulls(&mut want);
            }
            got == want
        }
    }
}

// ---- Python value semantics --------------------------------------------------------------------

/// A JSON value as a Python dict key: equal keys are equal Python values (`1 == 1.0 == True`).
/// The derived order sorts numbers numerically, as `sorted()` does for the int indexes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PyKey {
    None,
    Num(i128),
    Float(u64),
    Str(String),
    Tuple(Vec<PyKey>),
}

/// A Python dict key; a list or a dict is unhashable and raises.
fn py_key(v: Option<&Value>) -> Result<PyKey, String> {
    Ok(match v {
        None | Some(Value::Null) => PyKey::None,
        Some(Value::Bool(b)) => PyKey::Num(i128::from(*b)),
        Some(Value::Number(n)) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => PyKey::Num(i128::from(i)),
            (_, Some(u), _) => PyKey::Num(i128::from(u)),
            (_, _, Some(f)) if f.fract() == 0.0 && f.abs() < 1e18 => PyKey::Num(f as i128),
            (_, _, f) => PyKey::Float(f.unwrap_or(f64::NAN).to_bits()),
        },
        Some(Value::String(s)) => PyKey::Str(s.clone()),
        Some(_) => return Err("TypeError: unhashable type".into()),
    })
}

/// Python `==` of two attribute values (an absent attribute is `None`).
fn py_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (py_key(a), py_key(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.unwrap_or(&Value::Null) == b.unwrap_or(&Value::Null),
    }
}

// ---- openai-python ResponseStream --------------------------------------------------------------

const PY_ROUTED: [&str; 4] = ["function_call", "custom_tool_call", "shell_call", "shell_call_output"];

/// `_output_identity`: `(type, id, call_id)`, with no `id` for the four routed types.
fn py_identity(item: &Value) -> Result<PyKey, String> {
    let Value::Object(o) = item else { return Err("AttributeError: item.type".into()) };
    let ty = o.get("type");
    let routed = ty.and_then(Value::as_str).is_some_and(|t| PY_ROUTED.contains(&t));
    let id = if routed { None } else { o.get("id") };
    Ok(PyKey::Tuple(vec![py_key(ty)?, py_key(id)?, py_key(o.get("call_id"))?]))
}

/// `_compatible_output_id`.
fn py_compatible(prev: &Value, repl: &Value) -> bool {
    let id = prev.get("id");
    let optional = matches!(prev.get("type").and_then(Value::as_str), Some("function_call" | "custom_tool_call"));
    py_eq(id, repl.get("id")) || (py_key(id) == Ok(PyKey::None) && optional)
}

/// `for item in output`: a non-list that iterates to anything raises at `item.type`.
fn py_output_list(v: Option<&Value>) -> Result<Option<Vec<Value>>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a.clone())),
        Some(Value::String(s)) if s.is_empty() => Ok(Some(Vec::new())),
        Some(Value::Object(m)) if m.is_empty() => Ok(Some(Vec::new())),
        Some(_) => Err("AttributeError: output item has no attribute 'type'".into()),
    }
}

#[derive(Default)]
struct PyStream {
    /// `None` until `response.created`; then the snapshot's `id`.
    id: Option<Option<Value>>,
    /// `_output_items`: the `added` items (and the initial snapshot's), by `output_index`.
    added: BTreeMap<PyKey, Value>,
    /// `_completed_output`: the `done` items, by `output_index`.
    done: BTreeMap<PyKey, Value>,
    /// `_completed_positions`: identity to `output_index`.
    done_at: HashMap<PyKey, PyKey>,
    /// Whether `_completed_response` is set, so `get_final_response()` returns.
    completed: bool,
}

impl PyStream {
    /// `_get_output_item`.
    fn item(&mut self, ev: &Map<String, Value>) -> Result<&mut Value, String> {
        let key = py_key(Some(ev.get("output_index").ok_or("AttributeError: event.output_index")?))?;
        self.added
            .get_mut(&key)
            .ok_or_else(|| "RuntimeError: Received a content event for an output index before receiving its output item".to_owned())
    }

    /// `handle_event` for one event: `accumulate_event`, then the events it yields. A yielded
    /// `output_item.done` item, the `output` of a yielded `response.failed` or
    /// `response.incomplete`, and the completed response are the call surfaces.
    fn step(&mut self, ev: &Map<String, Value>, calls: &mut Vec<ModelCall>) -> Result<(), String> {
        let ty = ev.get("type").and_then(Value::as_str).unwrap_or("");
        let attr = |k: &str| ev.get(k).ok_or_else(|| format!("AttributeError: event.{k}"));
        // `handle_event` rebuilds these two events with `event.output_index` (and `event.item`).
        if ty == ADDED || ty == DONE {
            attr("output_index")?;
        }
        if ty == DONE {
            attr("item")?;
        }
        let Some(id) = &self.id else {
            if ty != "response.created" {
                return Err(format!("RuntimeError: Expected to have received `response.created` before `{ty}`"));
            }
            let Value::Object(r) = attr("response")? else {
                return Err("AttributeError: response.to_dict".into());
            };
            self.id = Some(r.get("id").cloned());
            if let Some(Value::Array(out)) = r.get("output") {
                for (i, item) in out.iter().enumerate() {
                    self.added.insert(PyKey::Num(i128::try_from(i).expect("small")), item.clone());
                }
            }
            return Ok(());
        };
        if LIFECYCLE.contains(&ty) && !py_eq(attr("response")?.get("id"), id.as_ref()) {
            return Err("RuntimeError: Lifecycle response identity changed.".into());
        }
        match ty {
            "response.created" | "response.queued" | "response.in_progress" => {
                let output = ev["response"].get("output").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut replacements: Vec<(PyKey, Value)> = Vec::new();
                for item in output {
                    let k = py_identity(&item)?;
                    if replacements.iter().any(|(x, _)| *x == k) {
                        return Err("RuntimeError: Response snapshot has conflicting output identities.".into());
                    }
                    replacements.push((k, item));
                }
                for (k, item) in replacements {
                    if let Some(idx) = self.done_at.get(&k).cloned() {
                        if !py_compatible(&self.done[&idx], &item) {
                            return Err("RuntimeError: Response snapshot has conflicting output identities.".into());
                        }
                        self.done.insert(idx, item);
                    }
                }
            }
            ADDED => {
                let Some(item) = ev.get("item").filter(|i| !i.is_null()) else { return Ok(()) };
                let key = py_key(ev.get("output_index"))?;
                if self.added.contains_key(&key) {
                    return Err("RuntimeError: Duplicate response output index.".into());
                }
                if !item.is_object() {
                    return Err("AttributeError: item.type".into());
                }
                self.added.insert(key, item.clone());
            }
            "response.content_part.added" => {
                let part = ev.get("part").cloned();
                let item = self.item(ev)?;
                if item.get("type").and_then(Value::as_str) == Some("message")
                    && let (Some(Value::Array(content)), Some(part)) = (item.get_mut("content"), part)
                {
                    content.push(part);
                }
            }
            "response.output_text.delta" | "response.output_text.done" => {
                let ci = attr("content_index")?.clone();
                let delta = if ty == "response.output_text.delta" { Some(attr("delta")?.clone()) } else { None };
                let item = self.item(ev)?;
                if item.get("type").and_then(Value::as_str) != Some("message") {
                    return Err("AssertionError: output.type == \"message\"".into());
                }
                let Some(Value::Array(content)) = item.get_mut("content") else {
                    return Err("TypeError: output.content".into());
                };
                let idx = py_int(&ci).ok_or("TypeError: list indices must be integers")?;
                let p = py_get(content.len(), idx).ok_or("IndexError: list index out of range")?;
                if content[p].get("type").and_then(Value::as_str) != Some("output_text") {
                    return Err("AssertionError: content.type == \"output_text\"".into());
                }
                if let Some(delta) = delta {
                    match (content[p].get_mut("text"), delta) {
                        (Some(Value::String(t)), Value::String(d)) => t.push_str(&d),
                        _ => return Err("TypeError: content.text += event.delta".into()),
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = attr("delta")?.clone();
                let item = self.item(ev)?;
                if item.get("type").and_then(Value::as_str) != Some("function_call") {
                    return Err("AssertionError: output.type == \"function_call\"".into());
                }
                match (item.get_mut("arguments"), delta) {
                    (Some(Value::String(a)), Value::String(d)) => a.push_str(&d),
                    _ => return Err("TypeError: output.arguments += event.delta".into()),
                }
            }
            DONE => {
                let key = py_key(ev.get("output_index"))?;
                let item = &ev["item"];
                if let Some(a) = self.added.get(&key)
                    && (py_identity(a)? != py_identity(item)? || !py_compatible(a, item))
                {
                    return Err("RuntimeError: Completed output item identity changed.".into());
                }
                let identity = py_identity(item)?;
                if self.done_at.get(&identity).is_some_and(|p| *p != key) {
                    return Err("RuntimeError: Response snapshot has conflicting output identities.".into());
                }
                if let Some(prev) = self.done.get(&key)
                    && self.done_at.remove(&py_identity(prev)?).is_none()
                {
                    return Err("KeyError: completed position".into());
                }
                self.done.insert(key.clone(), item.clone());
                self.done_at.insert(identity, key);
                calls.extend(item_call(item, position_of(ev.get("output_index")), false));
            }
            "response.completed" => {
                let output = py_output_list(ev["response"].get("output"))?;
                let mut finals: HashMap<PyKey, Value> = HashMap::new();
                for item in output.iter().flatten() {
                    finals.insert(py_identity(item)?, item.clone());
                }
                for c in self.done.values() {
                    if let Some(f) = finals.get(&py_identity(c)?)
                        && !py_compatible(c, f)
                    {
                        return Err("RuntimeError: Completed response output identity changed.".into());
                    }
                }
                for (k, item) in &self.added {
                    let f = finals.get(&py_identity(item)?);
                    if !self.done.contains_key(k) && f.is_none_or(|f| !py_compatible(item, f)) {
                        return Err("RuntimeError: Cannot complete response with unfinished output items.".into());
                    }
                }
                let output = output.unwrap_or_else(|| self.done.values().cloned().collect());
                self.completed = true;
                output_calls(&output, false, calls);
            }
            "response.failed" | "response.incomplete" => {
                let output = match ev["response"].get("output") {
                    None | Some(Value::Null) => {
                        let keys: BTreeSet<&PyKey> = self.added.keys().chain(self.done.keys()).collect();
                        keys.into_iter().map(|k| self.done.get(k).or_else(|| self.added.get(k)).cloned().unwrap_or(Value::Null)).collect()
                    }
                    Some(Value::Array(a)) => a.clone(),
                    Some(_) => Vec::new(),
                };
                output_calls(&output, false, calls);
            }
            _ => {}
        }
        Ok(())
    }
}

fn python_stream_steps(bytes: &[u8], st: &mut PyStream, calls: &mut Vec<ModelCall>) -> Result<(), String> {
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
        let Value::Object(ev) = &v else { return Err("AttributeError: event.type".into()) };
        st.step(ev, calls)?;
    }
    Ok(())
}

/// openai-python `client.responses.stream()` (`ResponseStream`). The SSE layer is
/// [`py_events`]: `[DONE]` stops by prefix, and a top-level truthy `error` raises `APIError`;
/// the `event:` name is not read, so an `error` event passes. `ResponseStreamState` keys every
/// item by `output_index`, checks the item identities at `done` and at the terminal event, and
/// asserts the item type on a text delta and on a function-call arguments delta. The first event
/// must be `response.created`.
///
/// Calls: the item of every yielded `output_item.done`, the `output` of a yielded
/// `response.failed` or `response.incomplete` (filled from the `done` and `added` items when it is
/// absent, R-d), and the completed response (`get_final_response()`, filled from the `done`
/// items when its `output` is absent). An `output_item.added` item is not a call (A5).
/// `get_final_response()` raises when the stream ends without `response.completed`.
/// SOURCE: openai-python 3.48.0, `openai/_streaming.py` `Stream.__stream__`;
/// `openai/lib/streaming/responses/_responses.py` `ResponseStream`, `ResponseStreamState`
/// `handle_event` and `accumulate_event`, `_output_identity`, `_compatible_output_id`;
/// `openai/lib/_parsing/_responses.py` `parse_response`.
pub fn run_python_stream(bytes: &[u8]) -> Run {
    let mut st = PyStream::default();
    let mut calls = Vec::new();
    let error = python_stream_steps(bytes, &mut st, &mut calls)
        .err()
        .or_else(|| (!st.completed).then(|| "RuntimeError: Didn't receive a `response.completed` event.".to_owned()));
    Run { calls: dedupe(calls), error }
}

fn python_raw_steps(bytes: &[u8], calls: &mut Vec<ModelCall>) -> Result<(), String> {
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
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty == DONE
            && let Some(item) = v.get("item")
        {
            calls.extend(item_call(item, position_of(v.get("output_index")), false));
        }
        if TERMINAL.contains(&ty)
            && let Some(Value::Array(output)) = v.pointer("/response/output")
        {
            output_calls(output, false, calls);
        }
    }
    Ok(())
}

/// openai-python `client.responses.create(stream=True)`: the same SSE layer as
/// [`run_python_stream`] and no accumulation. It raises only at the SSE layer: a strict UTF-8
/// error, a JSON error, or a top-level truthy `error`.
///
/// Calls: the item of every `output_item.done` and the `output` of every terminal event.
/// SOURCE: openai-python 3.48.0, `openai/_streaming.py` `SSEDecoder` and `Stream.__stream__`.
pub fn run_python_raw(bytes: &[u8]) -> Run {
    let mut calls = Vec::new();
    let error = python_raw_steps(bytes, &mut calls).err();
    Run { calls: dedupe(calls), error }
}

// ---- openai-node ResponseStream ----------------------------------------------------------------

/// `supportedResponseEventTypes`: 58 types plus `keepalive` and `error`. Any other type raises
/// (`Unhandled response stream event`), the two `response.apply_patch_call_operation_diff.*`
/// types included.
const NODE_TYPES: [&str; 60] = [
    "response.output_item.added",
    "response.output_item.done",
    "response.content_part.added",
    "response.content_part.done",
    "response.output_text.delta",
    "response.output_text.done",
    "response.output_text.annotation.added",
    "response.refusal.delta",
    "response.refusal.done",
    "response.function_call_arguments.delta",
    "response.function_call_arguments.done",
    "response.custom_tool_call_input.delta",
    "response.custom_tool_call_input.done",
    "response.mcp_call_arguments.delta",
    "response.mcp_call_arguments.done",
    "response.shell_call_command.added",
    "response.shell_call_command.done",
    "response.shell_call_command.delta",
    "response.shell_call_output_content.delta",
    "response.shell_call_output_content.done",
    "response.reasoning_text.delta",
    "response.reasoning_text.done",
    "response.reasoning_summary_part.added",
    "response.reasoning_summary_part.done",
    "response.reasoning_summary_text.delta",
    "response.reasoning_summary_text.done",
    "response.code_interpreter_call_code.delta",
    "response.code_interpreter_call_code.done",
    "response.code_interpreter_call.in_progress",
    "response.code_interpreter_call.interpreting",
    "response.code_interpreter_call.completed",
    "response.file_search_call.in_progress",
    "response.file_search_call.searching",
    "response.file_search_call.completed",
    "response.web_search_call.in_progress",
    "response.web_search_call.searching",
    "response.web_search_call.completed",
    "response.image_generation_call.in_progress",
    "response.image_generation_call.generating",
    "response.image_generation_call.completed",
    "response.mcp_call.in_progress",
    "response.mcp_call.completed",
    "response.mcp_call.failed",
    "response.created",
    "response.queued",
    "response.in_progress",
    "response.completed",
    "response.failed",
    "response.incomplete",
    "response.audio.delta",
    "response.audio.done",
    "response.audio.transcript.delta",
    "response.audio.transcript.done",
    "response.compaction.compacting",
    "response.image_generation_call.partial_image",
    "response.mcp_list_tools.in_progress",
    "response.mcp_list_tools.completed",
    "response.mcp_list_tools.failed",
    "keepalive",
    "error",
];

/// `expectedOutputItemTypes`: an item-scoped event, the type of its item, and so a required
/// `item_id`.
const NODE_EXPECTED: &[(&str, &str)] = &[
    ("response.output_text.delta", "message"),
    ("response.output_text.done", "message"),
    ("response.output_text.annotation.added", "message"),
    ("response.refusal.delta", "message"),
    ("response.refusal.done", "message"),
    ("response.function_call_arguments.delta", "function_call"),
    ("response.function_call_arguments.done", "function_call"),
    ("response.custom_tool_call_input.delta", "custom_tool_call"),
    ("response.custom_tool_call_input.done", "custom_tool_call"),
    ("response.mcp_call_arguments.delta", "mcp_call"),
    ("response.mcp_call_arguments.done", "mcp_call"),
    ("response.mcp_call.in_progress", "mcp_call"),
    ("response.mcp_call.completed", "mcp_call"),
    ("response.mcp_call.failed", "mcp_call"),
    ("response.shell_call_output_content.delta", "shell_call_output"),
    ("response.shell_call_output_content.done", "shell_call_output"),
    ("response.reasoning_text.delta", "reasoning"),
    ("response.reasoning_text.done", "reasoning"),
    ("response.reasoning_summary_part.added", "reasoning"),
    ("response.reasoning_summary_part.done", "reasoning"),
    ("response.reasoning_summary_text.delta", "reasoning"),
    ("response.reasoning_summary_text.done", "reasoning"),
    ("response.code_interpreter_call_code.delta", "code_interpreter_call"),
    ("response.code_interpreter_call_code.done", "code_interpreter_call"),
    ("response.code_interpreter_call.in_progress", "code_interpreter_call"),
    ("response.code_interpreter_call.interpreting", "code_interpreter_call"),
    ("response.code_interpreter_call.completed", "code_interpreter_call"),
    ("response.file_search_call.in_progress", "file_search_call"),
    ("response.file_search_call.searching", "file_search_call"),
    ("response.file_search_call.completed", "file_search_call"),
    ("response.web_search_call.in_progress", "web_search_call"),
    ("response.web_search_call.searching", "web_search_call"),
    ("response.web_search_call.completed", "web_search_call"),
    ("response.image_generation_call.in_progress", "image_generation_call"),
    ("response.image_generation_call.generating", "image_generation_call"),
    ("response.image_generation_call.completed", "image_generation_call"),
    ("response.image_generation_call.partial_image", "image_generation_call"),
    ("response.mcp_list_tools.in_progress", "mcp_list_tools"),
    ("response.mcp_list_tools.completed", "mcp_list_tools"),
    ("response.mcp_list_tools.failed", "mcp_list_tools"),
    ("response.compaction.compacting", "compaction"),
];

const NODE_ROUTED: [&str; 4] = PY_ROUTED;

fn node_type(item: &Value) -> Option<&str> {
    item.get("type").and_then(Value::as_str)
}

fn node_routed(item: &Value) -> bool {
    node_type(item).is_some_and(|t| NODE_ROUTED.contains(&t))
}

/// `outputIdentity`: `JSON.stringify([type, id unless routed, call_id])`.
fn node_identity(item: &Value) -> String {
    let field = |k: &str| item.get(k).cloned().unwrap_or(Value::Null);
    let id = if node_routed(item) { Value::Null } else { field("id") };
    json!([field("type"), id, field("call_id")]).to_string()
}

/// `getOutputItemIdentityKeys`: an own string `type`; a non-empty string `id` (empty allowed for
/// `computer_call`; absent allowed for a function or custom call); for a routed type, a string
/// `call_id` (empty allowed for `function_call`).
fn node_keys(item: &Value, what: &str) -> Result<Vec<String>, String> {
    let bad = |f: &str| Err(format!("OpenAIError: expected a non-empty output item {f} for {what}"));
    let Value::Object(o) = item else { return bad("type") };
    let Some(ty) = o.get("type").and_then(Value::as_str) else { return bad("type") };
    let mut keys = Vec::new();
    match o.get("id") {
        Some(Value::String(id)) if !id.is_empty() || ty == "computer_call" => keys.push(format!("id:{id}")),
        None if matches!(ty, "function_call" | "custom_tool_call") => {}
        _ => return bad("id"),
    }
    if node_routed(item) {
        match o.get("call_id") {
            Some(Value::String(c)) if !c.is_empty() || ty == "function_call" => keys.push(format!("call:{ty}:{c}")),
            _ => return bad("call_id"),
        }
    }
    Ok(keys)
}

/// `validateResponseOutputItemIdentity`.
fn node_validate(old: &Value, new: &Value, what: &str) -> Result<(), String> {
    node_keys(old, what)?;
    node_keys(new, what)?;
    if node_type(old) != node_type(new) {
        return Err("OpenAIError: expected output item type".into());
    }
    let adds_optional_id = old.get("id").is_none() && matches!(node_type(old), Some("function_call" | "custom_tool_call"));
    if old.get("id") != new.get("id") && !adds_optional_id {
        return Err("OpenAIError: expected output item id".into());
    }
    if node_routed(old) && node_routed(new) && old.get("call_id") != new.get("call_id") {
        return Err("OpenAIError: expected output item call_id".into());
    }
    Ok(())
}

/// Every identity key of the snapshot's items.
fn node_identity_set(output: &[Value]) -> Result<HashSet<String>, String> {
    let mut set = HashSet::new();
    for item in output {
        for k in node_keys(item, "response snapshot")? {
            if !set.insert(k.clone()) {
                return Err(format!("OpenAIError: duplicate output item identity '{k}'"));
            }
        }
    }
    Ok(set)
}

/// `getOutput`: an own, truthy entry of the output array.
fn node_output(output: &mut [Value], pos: Option<usize>) -> Result<&mut Value, String> {
    match pos.and_then(|p| output.get_mut(p)) {
        Some(v) if js_truthy(Some(v)) => Ok(v),
        _ => Err(format!("OpenAIError: missing output at index {pos:?}")),
    }
}

/// `Number.isSafeInteger(i) && i >= 0`.
fn node_index(v: Option<&Value>) -> Option<u64> {
    let n = v?.as_number()?;
    let max = (1u64 << 53) - 1;
    n.as_u64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64)).filter(|i| *i <= max)
}

/// JavaScript `String(v)`; `None` is `undefined`.
fn js_str(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Some(Value::Array(a)) => a.iter().map(|e| if e.is_null() { String::new() } else { js_str(Some(e)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// JavaScript `a += b` on a string field: a string on either side concatenates.
fn js_add(a: Option<&Value>, b: Option<&Value>) -> Value {
    match (a, b) {
        (Some(Value::Number(x)), Some(Value::Number(y))) => json!(x.as_f64().unwrap_or(f64::NAN) + y.as_f64().unwrap_or(f64::NAN)),
        (Some(Value::String(_)), _) | (_, Some(Value::String(_))) => Value::String(js_str(a) + &js_str(b)),
        _ => Value::Null, // NaN
    }
}

#[derive(Debug, Clone)]
struct NodeSnap {
    id: Option<Value>,
    output: Vec<Value>,
}

#[derive(Default)]
struct NodeStream {
    snap: Option<NodeSnap>,
    /// `#outputPositions`: wire `output_index` to snapshot position.
    positions: BTreeMap<u64, usize>,
    by_identity: HashMap<String, u64>,
    /// `#completedOutput` and `#unfinishedOutput`, by wire index.
    completed: BTreeMap<u64, Value>,
    unfinished: BTreeMap<u64, String>,
    dirty: bool,
}

/// `cloneValidatedResponse`: the response's `output` with its identity keys checked.
fn node_clone_validated(r: &Map<String, Value>) -> Result<NodeSnap, String> {
    let Some(Value::Array(output)) = r.get("output") else {
        return Err("OpenAIError: missing output at index 0".into());
    };
    node_identity_set(output)?;
    Ok(NodeSnap {
        id: r.get("id").cloned(),
        output: output.clone(),
    })
}

/// `replaceResponseOutputItem`.
fn node_replace(snap: &mut NodeSnap, pos: usize, item: &Value) -> Result<(), String> {
    let set = node_identity_set(&snap.output)?;
    let old = node_output(&mut snap.output, Some(pos))?;
    node_validate(old, item, DONE)?;
    if old.get("id").is_none()
        && let Some(Value::String(id)) = item.get("id")
        && set.contains(&format!("id:{id}"))
    {
        return Err(format!("OpenAIError: duplicate output item identity 'id:{id}'"));
    }
    *old = item.clone();
    Ok(())
}

impl NodeStream {
    /// `#outputAtWireIndex`.
    fn at_wire(&self, w: u64) -> Option<Value> {
        self.completed.get(&w).cloned().or_else(|| {
            let p = self.positions.get(&w)?;
            self.snap.as_ref()?.output.get(*p).cloned()
        })
    }

    /// `#reorderInterimOutput`.
    fn reorder(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(snap) = self.snap.as_mut() else { return };
        let ordered: Vec<(u64, usize)> = self.positions.iter().map(|(w, p)| (*w, *p)).collect();
        if ordered.iter().enumerate().any(|(np, (_, op))| *op != np) {
            snap.output = ordered.iter().map(|(_, op)| snap.output.get(*op).cloned().unwrap_or(Value::Null)).collect();
            self.positions = ordered.iter().enumerate().map(|(np, (w, _))| (*w, np)).collect();
        }
    }

    /// `#assertComplete`.
    fn assert_complete(&self, r: &Map<String, Value>) -> Result<(), String> {
        if self.unfinished.is_empty() && self.completed.is_empty() {
            return Ok(());
        }
        let output = match r.get("output") {
            Some(Value::Array(a)) => a.as_slice(),
            None | Some(Value::Null) => &[],
            Some(_) => return Err("TypeError: response.output.map is not a function".into()),
        };
        let finals: HashMap<String, &Value> = output.iter().map(|i| (node_identity(i), i)).collect();
        for item in self.completed.values() {
            if let Some(f) = finals.get(&node_identity(item)) {
                node_validate(item, f, "response.completed")?;
            }
        }
        for (w, identity) in &self.unfinished {
            let f = finals.get(identity).ok_or("OpenAIError: Cannot complete response with unfinished output items.")?;
            let prev = self.at_wire(*w).ok_or("OpenAIError: Cannot complete response with a missing output identity.")?;
            node_validate(&prev, f, "response.completed")?;
        }
        Ok(())
    }

    /// `#recordOutput` for a lifecycle event that replaced the snapshot: the wire positions are
    /// rebuilt from the new output by identity.
    fn rebuild(&mut self, new: &NodeSnap, ty: &str) -> Result<(), String> {
        let mut prev: HashMap<String, u64> = HashMap::new();
        for (w, item) in &self.completed {
            prev.insert(node_identity(item), *w);
        }
        for (w, identity) in &self.unfinished {
            prev.insert(identity.clone(), *w);
        }
        let mut positions = BTreeMap::new();
        self.by_identity.clear();
        for (np, item) in new.output.iter().enumerate() {
            let identity = node_identity(item);
            let w = prev.get(&identity).copied().unwrap_or(np as u64);
            let done_differs = self.completed.get(&w).is_some_and(|d| node_identity(d) != identity);
            let unfinished_differs = self.unfinished.get(&w).is_some_and(|u| *u != identity);
            if positions.contains_key(&w) || done_differs || unfinished_differs {
                return Err("OpenAIError: Response snapshot has conflicting output positions.".into());
            }
            if let Some(p) = self.at_wire(w) {
                node_validate(&p, item, ty)?;
            }
            positions.insert(w, np);
            self.by_identity.insert(identity.clone(), w);
            match self.completed.get_mut(&w) {
                Some(done) => *done = item.clone(),
                None => {
                    self.unfinished.insert(w, identity);
                }
            }
        }
        self.positions = positions;
        self.dirty = !matches!(ty, "response.failed" | "response.incomplete");
        Ok(())
    }

    /// `#updateInterimResponse`: `Ok(true)` when the snapshot took the interim output in place.
    fn update_interim(&mut self, incoming: &Map<String, Value>) -> Result<bool, String> {
        let Some(Value::Array(output)) = incoming.get("output") else { return Ok(false) };
        if output.is_empty() {
            return Ok(true);
        }
        let validated = node_clone_validated(incoming)?;
        let mut replacements = Vec::new();
        for item in validated.output {
            let Some(&w) = self.by_identity.get(&node_identity(&item)) else { return Ok(false) };
            let Some(&p) = self.positions.get(&w) else { return Ok(false) };
            replacements.push((w, p, item));
        }
        let snap = self.snap.as_mut().expect("an interim update has a snapshot");
        for (w, p, item) in replacements {
            node_replace(snap, p, &item)?;
            if let Some(done) = self.completed.get_mut(&w) {
                *done = item;
            }
        }
        self.reorder();
        Ok(true)
    }

    /// A lifecycle event. `#addEvent` emits it with the prepared response (its `output` filled
    /// when absent), so the `output` of an emitted terminal event is a call surface, counted at
    /// the emit point (ruling T7e).
    fn lifecycle(&mut self, ty: &str, ev: &Map<String, Value>, emitted: &mut Vec<ModelCall>) -> Result<(), String> {
        let Some(r) = ev.get("response") else {
            return Err("OpenAIError: Lifecycle events require their own response field.".into());
        };
        let Value::Object(r) = r else {
            return Err("OpenAIError: Lifecycle events require a response object.".into());
        };
        let mut r = r.clone();
        if let Some(s) = &self.snap
            && r.get("id") != s.id.as_ref()
        {
            return Err("OpenAIError: Lifecycle response identity changed.".into());
        }
        if r.get("output").is_none_or(Value::is_null) {
            let output: Vec<Value> = match ty {
                "response.completed" => self.completed.values().cloned().collect(),
                "response.failed" | "response.incomplete" => self
                    .positions
                    .keys()
                    .map(|w| self.at_wire(*w).ok_or_else(|| format!("OpenAIError: missing output at index {w}")))
                    .collect::<Result<_, _>>()?,
                _ => Vec::new(),
            };
            r.insert("output".into(), Value::Array(output));
        }
        if ty == "response.completed" {
            self.assert_complete(&r)?;
        }
        if INTERIM.contains(&ty)
            && let Some(snap) = &self.snap
        {
            let snap_output = snap.output.clone();
            if self.update_interim(&r)? {
                return Ok(());
            }
            let Some(Value::Array(new)) = r.get_mut("output") else {
                return Err("TypeError: response.output.map is not a function".into());
            };
            let ids: HashSet<String> = new.iter().map(node_identity).collect();
            new.extend(snap_output.into_iter().filter(|i| !ids.contains(&node_identity(i))));
        }
        if self.snap.is_none() && ty != "response.created" {
            return Err(format!("OpenAIError: When snapshot hasn't been set yet, expected 'response.created' event, got {ty}"));
        }
        let new = node_clone_validated(&r)?;
        if ty == "response.completed" {
            self.dirty = false;
        } else {
            self.rebuild(&new, ty)?;
        }
        self.snap = Some(new);
        if INTERIM.contains(&ty) {
            self.reorder();
        }
        if TERMINAL.contains(&ty)
            && let Some(Value::Array(output)) = r.get("output")
        {
            output_calls(output, false, emitted);
        }
        Ok(())
    }

    /// `validateOutputItemIdentity` with `rejectInvalidShellTargets`.
    fn validate_scoped(&mut self, ty: &str, ev: &Map<String, Value>, pos: Option<usize>) -> Result<(), String> {
        if ty == DONE {
            return Ok(());
        }
        let snap = self.snap.as_mut().expect("checked");
        if ty.starts_with("response.shell_call_command.") {
            let out = node_output(&mut snap.output, pos)?;
            if node_type(out) != Some("shell_call") {
                return Err("OpenAIError: expected output item type 'shell_call'".into());
            }
            return Ok(());
        }
        let content_part = matches!(ty, "response.content_part.added" | "response.content_part.done");
        let Some(expected) = (if content_part { Some("") } else { NODE_EXPECTED.iter().find(|(t, _)| *t == ty).map(|(_, e)| *e) }) else {
            return Ok(());
        };
        let item_id = match ev.get("item_id") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => return Err(format!("OpenAIError: expected a non-empty item_id for {ty}")),
        };
        let out = node_output(&mut snap.output, pos)?;
        if out.get("id").and_then(Value::as_str) != Some(item_id.as_str()) {
            return Err(format!("OpenAIError: expected item_id {:?}, got '{item_id}'", out.get("id")));
        }
        let expected = if content_part {
            match ev.get("part") {
                Some(Value::Object(p)) if p.get("type").and_then(Value::as_str) == Some("reasoning_text") => "reasoning",
                Some(Value::Object(_)) => "message",
                _ => return Err("TypeError: event.part.type".into()),
            }
        } else {
            expected
        };
        if node_type(out) != Some(expected) {
            return Err(format!("OpenAIError: expected output item type '{expected}'"));
        }
        Ok(())
    }

    /// One output event: `#prepareOutputEvent`, the accumulator, `#recordOutput`, then the
    /// emitted event. An emitted `output_item.done` item is a call surface.
    fn output_event(&mut self, ty: &str, ev: &Map<String, Value>, emitted: &mut Vec<ModelCall>) -> Result<(), String> {
        let Some(snap) = self.snap.as_ref() else {
            return Err(format!("OpenAIError: When snapshot hasn't been set yet, expected 'response.created' event, got {ty}"));
        };
        let wire = if ev.contains_key("output_index") {
            Some(node_index(ev.get("output_index")).ok_or("OpenAIError: Invalid response output index.")?)
        } else {
            None
        };
        let mut pos = None;
        if let Some(w) = wire {
            if ty == ADDED {
                if ev.get("item").is_none_or(Value::is_null) {
                    return Ok(()); // the event is ignored and not emitted
                }
                if self.completed.contains_key(&w) || self.unfinished.contains_key(&w) {
                    return Err(format!("OpenAIError: Duplicate response output index {w}."));
                }
                pos = Some(snap.output.len());
            } else {
                pos = Some(*self.positions.get(&w).ok_or_else(|| format!("OpenAIError: missing output at index {w}"))?);
            }
        }
        self.validate_scoped(ty, ev, pos)?;
        let snap = self.snap.as_mut().expect("checked");
        let item = ev.get("item");
        match ty {
            ADDED => {
                let item = item.expect("checked");
                let keys = node_keys(item, ty)?;
                let set = node_identity_set(&snap.output)?;
                if let Some(k) = keys.iter().find(|k| set.contains(*k)) {
                    return Err(format!("OpenAIError: duplicate output item identity '{k}'"));
                }
                if pos != Some(snap.output.len()) {
                    return Err("OpenAIError: missing output at index undefined".into());
                }
                snap.output.push(item.clone());
            }
            DONE => {
                let p = pos.ok_or("OpenAIError: missing output at index undefined")?;
                node_replace(snap, p, item.unwrap_or(&Value::Null))?;
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" | "response.custom_tool_call_input.delta" | "response.custom_tool_call_input.done" => {
                let (want, key) = if ty.contains("function_call") {
                    ("function_call", "arguments")
                } else {
                    ("custom_tool_call", "input")
                };
                let out = node_output(&mut snap.output, pos)?;
                if node_type(out) == Some(want)
                    && let Value::Object(o) = out
                {
                    if ty.ends_with(".delta") {
                        let v = js_add(o.get(key), ev.get("delta"));
                        o.insert(key.into(), v);
                    } else {
                        match ev.get(key) {
                            Some(v) => o.insert(key.into(), v.clone()),
                            None => o.remove(key),
                        };
                    }
                }
            }
            "response.shell_call_command.added" | "response.shell_call_command.done" | "response.shell_call_command.delta" => {
                let out = node_output(&mut snap.output, pos)?;
                if node_type(out) == Some("shell_call") {
                    let Some(Value::Object(action)) = out.get_mut("action") else {
                        return Err("TypeError: output.action.commands".into());
                    };
                    let Some(Value::Array(commands)) = action.get_mut("commands") else {
                        return Err("OpenAIError: missing command".into());
                    };
                    let ci = node_index(ev.get("command_index")).ok_or("OpenAIError: missing command")?;
                    let ci = usize::try_from(ci).map_err(|_| "OpenAIError: missing command")?;
                    let append = ty == "response.shell_call_command.added";
                    if ci > commands.len() || (ci == commands.len() && !append) {
                        return Err(format!("OpenAIError: missing command at index {ci}"));
                    }
                    let v = if ty.ends_with(".delta") {
                        js_add(commands.get(ci), ev.get("delta"))
                    } else {
                        ev.get("command").cloned().unwrap_or(Value::Null)
                    };
                    if ci == commands.len() {
                        commands.push(v);
                    } else {
                        commands[ci] = v;
                    }
                }
            }
            _ => {}
        }
        // `#recordOutput` for an output event.
        if let (Some(w), Some(p)) = (wire, pos) {
            if ty == ADDED {
                let identity = node_identity(item.expect("checked"));
                self.positions.insert(w, p);
                self.unfinished.insert(w, identity.clone());
                self.by_identity.insert(identity, w);
                self.dirty = true;
            } else if ty == DONE {
                let done = snap.output[p].clone();
                self.completed.insert(w, done);
                self.unfinished.remove(&w);
            }
        }
        if ty == DONE {
            emitted.extend(item_call(item.unwrap_or(&Value::Null), wire.and_then(|w| u32::try_from(w).ok()).or(Some(u32::MAX)), false));
        }
        Ok(())
    }
}

fn node_steps(bytes: &[u8], st: &mut NodeStream, emitted: &mut Vec<ModelCall>) -> Result<(), String> {
    const UNHANDLED: &str = "OpenAIError: Unhandled response stream event: unknown";
    for sse in node_events(bytes) {
        if sse.data == "[DONE]" {
            break;
        }
        let v: Value = serde_json::from_str(&sse.data).map_err(|_| "SyntaxError: Error reading response: malformed server-sent event JSON.".to_owned())?;
        // A `thread.*` event is yielded as `{event, data}`, which has no `type`.
        if sse.event.as_deref().is_some_and(|e| e.starts_with("thread.")) {
            return Err(UNHANDLED.into());
        }
        if sse.event.as_deref() == Some("error") {
            return Err("APIError: error event".into());
        }
        if let Value::Object(m) = &v
            && js_truthy(m.get("error"))
        {
            return Err("APIError: error record".into());
        }
        let Value::Object(ev) = &v else { return Err(UNHANDLED.into()) };
        let ty = match ev.get("type") {
            Some(Value::String(t)) => t.as_str(),
            _ => return Err(UNHANDLED.into()),
        };
        if ty == "error" {
            return Err("APIError: error event".into());
        }
        if !NODE_TYPES.contains(&ty) {
            return Err(format!("OpenAIError: Unhandled response stream event: {ty}"));
        }
        if ty == "keepalive" {
            if st.snap.is_none() {
                return Err("OpenAIError: When snapshot hasn't been set yet, expected 'response.created' event, got keepalive".into());
            }
            continue;
        }
        if LIFECYCLE.contains(&ty) {
            st.lifecycle(ty, ev, emitted)?;
        } else {
            st.output_event(ty, ev, emitted)?;
        }
    }
    Ok(())
}

/// openai-node `ResponseStream` (`client.responses.stream()`, `finalResponse()`). The SSE layer
/// is [`node_events`]: `[DONE]` stops by exact match; an `event: error`, a top-level truthy
/// `error` and a `type: "error"` record raise `APIError`. An event type outside the 60 supported
/// ones raises, so the two `response.apply_patch_call_operation_diff.*` types raise. The
/// accumulator keys by `output_index` (a wire index mapped to a snapshot position); an
/// item-scoped event needs an `item_id` equal to its item's `id` and an item of the expected
/// type; a shell command event needs a `shell_call`. `arguments.done`, `input.done`,
/// `shell_call_command.done` and `output_item.done` replace the value or the whole item; a
/// lifecycle event replaces the whole snapshot.
///
/// Calls: every emitted `output_item.done` item, the `response.output` of every emitted terminal
/// event (`response.completed`, `.failed`, `.incomplete`, with the output that `#prepareEvent`
/// fills), counted at the emit point (ruling T7e), and the final snapshot (`finalResponse()`;
/// partial at a clean EOF without a terminal event). At a raise `finalResponse()` rejects, so
/// only the emitted events count.
/// SOURCE: openai-node 7.57.0, `src/core/streaming.ts` `Stream.fromSSEResponse` and `SSEDecoder`;
/// `src/lib/responses/ResponseStream.ts` `#addEvent`, `#prepareEvent`, `#prepareOutputEvent`,
/// `#recordOutput`, `#updateInterimResponse`, `#assertComplete`, `#endRequest`, `outputIdentity`;
/// `src/internal/responses/response-accumulator.ts` `accumulateResponseWithContext`,
/// `sanitizeResponseEvent`, `validateOutputItemIdentity`, `getOutputItemIdentityKeys`,
/// `replaceResponseOutputItem`, `accumulateRefusalAndArgumentsEvent`, `accumulateShellEvent`.
pub fn run_node(bytes: &[u8]) -> Run {
    let mut st = NodeStream::default();
    let mut calls = Vec::new();
    if let Err(e) = node_steps(bytes, &mut st, &mut calls) {
        return Run { calls: dedupe(calls), error: Some(e) };
    }
    if st.snap.is_none() {
        return Run {
            calls: dedupe(calls),
            error: Some("OpenAIError: request ended without sending any events".into()),
        };
    }
    st.reorder();
    output_calls(&st.snap.as_ref().expect("checked").output, false, &mut calls);
    Run { calls: dedupe(calls), error: None }
}

// ---- LangChain ---------------------------------------------------------------------------------

/// The `output_item.done` item types that `_convert_responses_chunk_to_generation_chunk` turns
/// into a content block by `model_dump(exclude_none=True)`.
const LC_DUMPED: [&str; 13] = [
    "compaction",
    "web_search_call",
    "file_search_call",
    "computer_call",
    "code_interpreter_call",
    "mcp_call",
    "mcp_list_tools",
    "mcp_approval_request",
    "image_generation_call",
    "tool_search_call",
    "tool_search_output",
    "apply_patch_call",
    "apply_patch_call_output",
];

/// The content blocks that are calls (spec § 10.1, `base.py:5596-5614`).
const LC_CALL_BLOCKS: [&str; 4] = ["computer_call", "apply_patch_call", "mcp_approval_request", "tool_search_call"];

/// Python's type of a JSON value, as `type(a) is not type(b)` compares it.
fn py_type(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(n) if n.is_i64() || n.is_u64() => 2,
        Value::Number(_) => 3,
        Value::String(_) => 4,
        Value::Array(_) => 5,
        Value::Object(_) => 6,
    }
}

/// langchain-core 1.8.0 `merge_dicts`.
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
        if rv.is_null() {
            continue;
        }
        if py_type(lv) != py_type(rv) {
            return Err(format!("TypeError: additional_kwargs[{k:?}] already exists in this message, but with a different type."));
        }
        match (lv, rv) {
            (Value::String(l), Value::String(r)) => {
                let keep = (k == "index" && l.starts_with("lc_")) || (matches!(k.as_str(), "id" | "output_version" | "model_provider") && l == r);
                if !keep {
                    l.push_str(r);
                }
            }
            (Value::Object(l), Value::Object(r)) => lc_merge_dicts(l, r)?,
            (Value::Array(l), Value::Array(r)) => lc_merge_lists(l, r)?,
            (l, r) if *l == *r => {}
            (Value::Bool(_), _) => return Err(format!("TypeError: unsupported type for {k:?}")),
            (Value::Number(l), Value::Number(r)) if l.is_i64() || l.is_u64() => {
                if matches!(k.as_str(), "index" | "created" | "timestamp") {
                    *l = r.clone();
                } else {
                    let sum = i128::from(l.as_i64().unwrap_or(i64::MAX)) + i128::from(r.as_i64().unwrap_or(i64::MAX));
                    *l = serde_json::Number::from(i64::try_from(sum).unwrap_or(i64::MAX));
                }
            }
            _ => return Err(format!("TypeError: unsupported type for {k:?}")),
        }
    }
    Ok(())
}

fn lc_id_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        _ => false,
    }
}

/// langchain-core 1.8.0 `merge_lists` for one right element: an element with an int (or
/// `lc_*`) `index` merges, without its `type`, into the first left element with an equal index
/// and no conflicting id; any other element is appended. `Ok(true)`: appended.
fn lc_merge_one(left: &mut Vec<Value>, e: &Value) -> Result<bool, String> {
    let index = e.as_object().and_then(|o| o.get("index"));
    let indexed = match index {
        Some(Value::Bool(_)) => true,
        Some(Value::Number(n)) => n.is_i64() || n.is_u64(),
        Some(Value::String(s)) => s.starts_with("lc_"),
        _ => false,
    };
    let Value::Object(eo) = e else {
        left.push(e.clone());
        return Ok(true);
    };
    if !indexed {
        left.push(e.clone());
        return Ok(true);
    }
    let hit = left.iter().position(|l| {
        l.as_object()
            .is_some_and(|lo| lo.contains_key("index") && py_eq(lo.get("index"), index) && (lc_id_empty(lo.get("id")) || lc_id_empty(eo.get("id")) || py_eq(lo.get("id"), eo.get("id"))))
    });
    let Some(p) = hit else {
        left.push(e.clone());
        return Ok(true);
    };
    let mut new = eo.clone();
    new.remove("type");
    let Value::Object(target) = &mut left[p] else { unreachable!("matched an object") };
    lc_merge_dicts(target, &new)?;
    Ok(false)
}

fn lc_merge_lists(left: &mut Vec<Value>, right: &[Value]) -> Result<(), String> {
    for e in right {
        lc_merge_one(left, e)?;
    }
    Ok(())
}

/// `model_dump(exclude_none=True, mode="json")` of an item, plus the LangChain `index`.
fn lc_dump(item: &Value, index: i64) -> Value {
    let mut d = item.clone();
    strip_nulls(&mut d);
    if let Value::Object(m) = &mut d {
        m.insert("index".into(), json!(index));
    }
    d
}

struct Lc {
    /// The summed `content` and `tool_call_chunks`; `None` before the first generation chunk.
    content: Option<Vec<Value>>,
    chunks: Option<Vec<Value>>,
    /// The kind of each summed tool call chunk: the kind of the chunk that opened it.
    origin: Vec<&'static str>,
    /// `item_output_indices`: item id to the first `output_index` seen.
    item_index: HashMap<PyKey, Value>,
    current_index: i64,
    current_output: Value,
    current_sub: Value,
}

impl Lc {
    fn new() -> Self {
        Lc {
            content: None,
            chunks: None,
            origin: Vec::new(),
            item_index: HashMap::new(),
            current_index: -1,
            current_output: json!(-1),
            current_sub: json!(-1),
        }
    }

    /// `_advance`: a new content index whenever the (pinned) output index or the sub-index
    /// changes.
    fn advance(&mut self, item_id: Option<&Value>, output_idx: &Value, sub: Option<&Value>) -> Result<i64, String> {
        let pinned = match item_id {
            Some(k) => self.item_index.get(&py_key(Some(k))?).cloned(),
            None => None,
        };
        let oi = pinned.unwrap_or_else(|| output_idx.clone());
        match sub {
            None => {
                if !py_eq(Some(&self.current_output), Some(&oi)) {
                    self.current_index += 1;
                }
            }
            Some(s) => {
                if !py_eq(Some(&self.current_output), Some(&oi)) || !py_eq(Some(&self.current_sub), Some(s)) {
                    self.current_index += 1;
                }
                self.current_sub = s.clone();
            }
        }
        self.current_output = oi;
        Ok(self.current_index)
    }

    /// `_convert_responses_chunk_to_generation_chunk` for one event, then the chunk sum
    /// (`add_ai_message_chunks`: `merge_lists` on `content` and on `tool_call_chunks`).
    fn step(&mut self, ev: &Map<String, Value>) -> Result<(), String> {
        let ty = ev.get("type").and_then(Value::as_str).unwrap_or("");
        let attr = |k: &str| ev.get(k).ok_or_else(|| format!("AttributeError: chunk.{k}"));
        let dumped = |k: &str| ev.get(k).cloned().unwrap_or(Value::Null);
        let item = ev.get("item");
        // `chunk.item.type`: an item that is not an object raises.
        let item_type = || -> Result<&str, String> {
            match item {
                Some(Value::Object(o)) => Ok(o.get("type").and_then(Value::as_str).unwrap_or("")),
                _ => Err("AttributeError: 'NoneType' object has no attribute 'type'".into()),
            }
        };
        let field = |k: &str| item.and_then(|i| i.get(k)).cloned().unwrap_or(Value::Null);
        let item_id = item.and_then(|i| i.get("id")).filter(|v| py_truthy(Some(v))).or_else(|| ev.get("item_id")).filter(|v| !v.is_null());
        if let (Some(k), Some(oi)) = (item_id, ev.get("output_index").filter(|v| !v.is_null())) {
            self.item_index.entry(py_key(Some(k))?).or_insert_with(|| oi.clone());
        }
        let mut content: Vec<Value> = Vec::new();
        let mut chunks: Vec<(Value, &'static str)> = Vec::new();
        if ty == "response.output_text.delta" {
            let i = self.advance(item_id, attr("output_index")?, Some(attr("content_index")?))?;
            content.push(json!({"type": "text", "text": dumped("delta"), "index": i}));
        } else if ty == "response.output_text.annotation.added" {
            let i = self.advance(item_id, attr("output_index")?, Some(attr("content_index")?))?;
            let Some(Value::Object(a)) = ev.get("annotation") else {
                return Err("AttributeError: annotation.model_dump".into());
            };
            let mut a = Value::Object(a.clone());
            strip_nulls(&mut a);
            content.push(json!({"type": "text", "annotations": [a], "index": i}));
        } else if ty == "response.output_text.done" {
            let i = self.advance(item_id, attr("output_index")?, Some(attr("content_index")?))?;
            content.push(json!({"type": "text", "text": "", "id": dumped("item_id"), "index": i}));
        } else if ty == "response.created" || ty == "response.completed" || ty == "response.incomplete" {
            // No content and no tool call chunks; `response.completed` and
            // `response.incomplete` carry the usage and the metadata only.
        } else if ty == "response.failed" {
            return Err("ValueError: response failed".into());
        } else if ty == "error" {
            return Err("ValueError: error event".into());
        } else if ty == ADDED && item_type()? == "message" {
            let phase = field("phase");
            if py_truthy(Some(&phase)) {
                let i = self.advance(item_id, attr("output_index")?, Some(&json!(0)))?;
                content.push(json!({"type": "text", "text": "", "phase": phase, "index": i}));
            }
        } else if ty == ADDED && item_type()? == "function_call" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            chunks.push((
                json!({"type": "tool_call_chunk", "name": field("name"), "args": field("arguments"), "id": field("call_id"), "index": i}),
                "function",
            ));
            let mut block = json!({"type": "function_call", "name": field("name"), "arguments": field("arguments"), "call_id": field("call_id"), "id": field("id"), "index": i});
            for k in ["namespace", "async"] {
                if !field(k).is_null() {
                    block[k] = field(k);
                }
            }
            content.push(block);
        } else if ty == DONE && LC_DUMPED.contains(&item_type()?) {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            content.push(lc_dump(item.expect("checked"), i));
        } else if ty == DONE && item_type()? == "custom_tool_call" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            content.push(lc_dump(item.expect("checked"), i));
            let args = format!("{{\"__arg1\": {}}}", field("input"));
            chunks.push((json!({"type": "tool_call_chunk", "name": field("name"), "args": args, "id": field("call_id"), "index": i}), "custom"));
        } else if ty == "response.function_call_arguments.delta" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            chunks.push((json!({"type": "tool_call_chunk", "args": dumped("delta"), "index": i}), "function"));
            content.push(json!({"type": "function_call", "arguments": dumped("delta"), "index": i}));
        } else if ty == "response.refusal.done" {
            content.push(json!({"type": "refusal", "refusal": dumped("refusal")}));
        } else if ty == ADDED && item_type()? == "reasoning" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            self.current_sub = json!(0);
            let mut d = lc_dump(item.expect("checked"), i);
            if let Value::Object(m) = &mut d {
                m.remove("encrypted_content");
            }
            content.push(d);
        } else if ty == DONE && item_type()? == "reasoning" && py_truthy(Some(&field("encrypted_content"))) {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            content.push(json!({"type": "reasoning", "id": field("id"), "summary": [], "encrypted_content": field("encrypted_content"), "index": i}));
        } else if ty == "response.reasoning_summary_part.added" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            content.push(json!({"summary": [{"index": dumped("summary_index"), "type": "summary_text", "text": ""}], "index": i, "type": "reasoning", "id": dumped("item_id")}));
        } else if ty == "response.image_generation_call.partial_image" {
            // Not supported yet: a chunk with nothing.
        } else if ty == "response.reasoning_summary_text.delta" {
            let i = self.advance(item_id, attr("output_index")?, None)?;
            content.push(json!({"summary": [{"index": dumped("summary_index"), "type": "summary_text", "text": dumped("delta")}], "index": i, "type": "reasoning"}));
        } else {
            return Ok(()); // no generation chunk
        }
        match self.content.as_mut() {
            None => self.content = Some(content),
            Some(c) => lc_merge_lists(c, &content)?,
        }
        match self.chunks.as_mut() {
            None => {
                self.origin = chunks.iter().map(|(_, k)| *k).collect();
                self.chunks = Some(chunks.into_iter().map(|(c, _)| c).collect());
            }
            Some(list) => {
                for (c, kind) in chunks {
                    if lc_merge_one(list, &c)? {
                        self.origin.push(kind);
                    }
                }
            }
        }
        Ok(())
    }

    /// The namespace that the application reads for the tool call at content index `index`: the
    /// `namespace` of the `function_call` (or `custom_tool_call`) content block at that index
    /// (`base.py:5578-5588` for a function, the item dump at `base.py:5620-5622` for a custom
    /// call). The tool call chunk itself has no namespace (`base.py:5569-5577`, `5623-5631`). A non-string
    /// namespace is its JSON text, so it never equals a decided one.
    fn namespace_at(&self, index: Option<&Value>) -> Option<String> {
        let block = self
            .content
            .iter()
            .flatten()
            .find(|b| matches!(b.get("type").and_then(Value::as_str), Some("function_call" | "custom_tool_call")) && b.get("index").is_some() && py_eq(b.get("index"), index))?;
        match block.get("namespace") {
            None | Some(Value::Null) => None,
            Some(Value::String(ns)) => Some(ns.clone()),
            Some(v) => Some(v.to_string()),
        }
    }

    /// `init_tool_calls` of the summed chunk, plus the content blocks that are calls.
    fn calls(&self) -> Vec<ModelCall> {
        let mut out = Vec::new();
        for (c, kind) in self.chunks.iter().flatten().zip(&self.origin) {
            let args = c.get("args");
            let parsed = match args {
                a if !py_truthy(a) => Some(json!({})),
                Some(Value::String(s)) => lc_parse_partial_json(s),
                _ => None, // `json.loads` raises on a non-string: an invalid tool call
            };
            let Some(Value::Object(a)) = parsed else { continue };
            let name = match c.get("name") {
                n if !py_truthy(n) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            let id = match c.get("id") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(v) => Some(v.to_string()),
            };
            let namespace = self.namespace_at(c.get("index"));
            out.push(ModelCall {
                choice: 0,
                kind,
                name,
                arguments: Value::Object(a).to_string(),
                parsed: true,
                position: None,
                id,
                namespace,
            });
        }
        for b in self.content.iter().flatten() {
            let Value::Object(m) = b else { continue };
            if !m.get("type").and_then(Value::as_str).is_some_and(|t| LC_CALL_BLOCKS.contains(&t)) {
                continue;
            }
            let mut m = m.clone();
            m.remove("index");
            out.extend(item_call(&Value::Object(m), None, true));
        }
        out
    }
}

fn langchain_steps(bytes: &[u8], lc: &mut Lc) -> Result<(), String> {
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
        let Value::Object(ev) = &v else { return Err("AttributeError: chunk.type".into()) };
        lc.step(ev)?;
    }
    Ok(())
}

/// LangChain `ChatOpenAI` with the Responses API (`_stream_responses`) over the openai-python
/// raw stream (the SSE layer of [`run_python_raw`]). Each event goes through
/// `_convert_responses_chunk_to_generation_chunk`, and the chunks are summed. Keying: an item's
/// events land on one content index, pinned by the item id (`item.id`, else `item_id`) to the
/// first `output_index` seen (R-f); a new content index starts whenever that index changes. A
/// `function_call` is a tool call chunk from `added` (name, arguments, call id) plus every
/// `function_call_arguments.delta` at that content index, whatever the item type there (R-e);
/// `function_call_arguments.done` and the function's `output_item.done` are ignored. A
/// `custom_tool_call` is a tool call chunk from its `done` (`{"__arg1": input}`). Ten built-in
/// item types plus the three below become content blocks from their `done` item
/// (`model_dump(exclude_none=True)`). It raises on `response.failed`, on `type: "error"`, on an
/// `output_item.added` or `.done` whose `item` is `null`, and in `merge_dicts` on a type change.
///
/// Calls: `tool_calls` of the summed chunk (`init_tool_calls` with `parse_partial_json`; an
/// `invalid_tool_calls` entry is not a call), plus the content blocks of `computer_call`,
/// `apply_patch_call`, `mcp_approval_request` and `tool_search_call`. `local_shell_call` and
/// `shell_call` are dropped. A tool call carries no output index. Its namespace is not on the tool
/// call chunk (`base.py:5569-5577`) but on the content block at the same content index, which the
/// application reads: `function_call_content["namespace"]` (`base.py:5587-5588`), or the custom
/// item's dump. The model takes it from there ([`Lc::namespace_at`]). Not
/// modelled: the `non_standard` branch of `merge_lists` (no OpenAI block has that type) and the
/// raises inside `_construct_lc_result_from_responses_api` at the terminal event (a missing raise
/// only makes the model stricter).
/// SOURCE: langchain-openai 1.8.0, `langchain_openai/chat_models/base.py` `_stream_responses`
/// and `_convert_responses_chunk_to_generation_chunk` (`base.py:5408-5700`); langchain-core
/// 1.8.0, `langchain_core/utils/_merge.py` `merge_dicts` and `merge_lists`,
/// `langchain_core/messages/ai.py` `add_ai_message_chunks` and `init_tool_calls`,
/// `langchain_core/messages/base.py` `merge_content`, `langchain_core/utils/json.py`
/// `parse_partial_json`; openai-python 3.48.0, `openai/_streaming.py`.
pub fn run_langchain(bytes: &[u8]) -> Run {
    let mut lc = Lc::new();
    let error = langchain_steps(bytes, &mut lc).err();
    Run { calls: lc.calls(), error }
}
