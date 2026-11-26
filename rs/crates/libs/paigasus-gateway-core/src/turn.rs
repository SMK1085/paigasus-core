// SPDX-License-Identifier: Apache-2.0
//! The request side of a turn (spec D19): what tools the caller declared, what it called before,
//! and which results it sent back. Never the content of a tool result. No request rewrite.

use std::sync::Arc;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::json::{Malformed, ResponsesDoc, opt_obj, opt_str, parse_strict, parse_strict_anthropic_request, parse_strict_responses};
use crate::report::notice_ref;
use crate::responses::assembly::tool_call;
use crate::responses::classify::{ItemClass, item_class};
use crate::types::{Format, ToolCallKind};

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolTurn {
    Parsed(TurnFacts),
    /// The request could not be read. Sequence rules (SMA-753) must treat this as unknown history,
    /// never as an empty one.
    Unavailable(&'static str),
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TurnFacts {
    pub definitions: Vec<ToolDefinition>,
    pub prior_calls: Vec<PriorCall>,
    pub tool_results: Vec<ToolResultRef>,
    /// `false` when the request does not list every tool the model may call (spec § 7.2).
    /// The chat and Anthropic formats always list them, so they set `true`.
    pub definitions_complete: bool,
    /// What the caller asked the model to do about tools (spec § 8).
    pub tool_choice: ToolChoice,
    /// Whether an Anthropic `tools[]` entry is a server tool, which `definitions` does not list. The
    /// request plan needs it for § 6.5.1 step 5. `false` for the other formats.
    pub server_tools: bool,
    /// The newest model turn and the results after it. Session linking reads it (SMA-753).
    pub latest: LatestTurn,
}

/// The most ids that `LatestTurn` keeps in one list.
pub const MAX_LINK_IDS: usize = 128;

/// The newest part of the conversation: the last assistant turn and the tool results after it.
/// It holds ids and notice references only, never the text or a result content.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatestTurn {
    /// The ids of the tool calls in the last assistant message, at most `MAX_LINK_IDS`.
    pub call_ids: Vec<String>,
    /// The call ids of the tool results after that message, at most `MAX_LINK_IDS`.
    pub result_call_ids: Vec<String>,
    /// The `ref=` ids in the text of that message, at most `MAX_LINK_IDS`.
    pub refs: Vec<Uuid>,
    pub dropped_calls: usize,
    pub dropped_results: usize,
    pub dropped_refs: usize,
}

fn push_capped<T>(list: &mut Vec<T>, dropped: &mut usize, v: T) {
    if list.len() < MAX_LINK_IDS {
        list.push(v);
    } else {
        *dropped += 1;
    }
}

impl LatestTurn {
    fn call(&mut self, id: &str) {
        push_capped(&mut self.call_ids, &mut self.dropped_calls, id.to_owned());
    }

    fn result(&mut self, id: &str) {
        push_capped(&mut self.result_call_ids, &mut self.dropped_results, id.to_owned());
    }

    /// Keep the `ref=` of every line of `text` that is an exact notice line.
    fn refs_in(&mut self, text: &str) {
        for r in text.split('\n').filter_map(notice_ref) {
            push_capped(&mut self.refs, &mut self.dropped_refs, r);
        }
    }

    /// Read the text of a message `content`: a string, or the parts of the given `kind`.
    fn content(&mut self, content: Option<&Value>, kind: &str) {
        match content {
            Some(Value::String(t)) => self.refs_in(t),
            Some(Value::Array(parts)) => {
                for p in parts {
                    if p.get("type").and_then(Value::as_str) == Some(kind)
                        && let Some(t) = p.get("text").and_then(Value::as_str)
                    {
                        self.refs_in(t);
                    }
                }
            }
            _ => {}
        }
    }
}

fn role(m: &Value) -> Option<&str> {
    m.get("role").and_then(Value::as_str)
}

/// The caller's `tool_choice`. Names are display names (`ns.name` for a Responses namespace tool).
/// A shape the parser does not know is `Unknown`, never a malformed request.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
    AllowedTools(Vec<String>),
    Unknown,
}

impl Default for TurnFacts {
    fn default() -> Self {
        Self {
            definitions: Vec::new(),
            prior_calls: Vec::new(),
            tool_results: Vec::new(),
            definitions_complete: true,
            tool_choice: ToolChoice::Auto,
            server_tools: false,
            latest: LatestTurn::default(),
        }
    }
}

/// A tool the caller declared. `Debug` prints `schema`; never log a definition with `{:?}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    /// The tool namespace of a Responses API definition. `None` for every other format.
    pub namespace: Option<String>,
    pub kind: ToolCallKind,
    /// Shared, so a detector can hold the schema without a copy.
    pub schema: Option<Arc<Value>>,
}

impl ToolDefinition {
    /// `"<namespace>.<name>"`, or `name` when the definition has no namespace.
    #[must_use]
    pub fn display_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A call the caller made on an earlier turn. `Debug` prints `arguments`, which may carry
/// secrets; never log a prior call with `{:?}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorCall {
    pub id: Option<String>,
    /// The display name, as `ToolCall::display_name` gives it: `ns.name` for a Responses
    /// namespace call, `server_label.name` for an `mcp_approval_request`, else the bare name.
    /// The request plan decides a result by this name, so it must match the call's name.
    pub name: String,
    pub arguments: String,
    /// How the call arrived. It decides whether `arguments` is JSON or text.
    pub kind: ToolCallKind,
    /// `true` for a Responses `local_shell_call`. Its result may name it by `id` alone.
    pub local_shell: bool,
    /// The item `id` of a Responses `local_shell_call`. `None` for every other call.
    pub item_id: Option<String>,
}

/// A reference to a tool result. It never holds the result content.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultRef {
    pub call_id: Option<String>,
    pub name: Option<String>,
    /// The item `id` of a Responses `local_shell_call_output`. The provider schema gives this item
    /// an `id` that refers to its call, and no `call_id`. `None` for every other result.
    pub item_id: Option<String>,
    /// How a blocked result is replaced.
    pub shape: ResultShape,
}

/// How a blocked tool result is replaced (SMA-776 § 6.1).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultShape {
    Text,
    ShellChunks,
    Screenshot,
    Approval,
}

/// The shape of a Responses input item of type `ty`, or `None` when the item is not a tool result.
/// This is the one list of the Responses result types.
pub(crate) fn responses_result_shape(ty: &str) -> Option<ResultShape> {
    match ty {
        "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" | "apply_patch_call_output" => Some(ResultShape::Text),
        "shell_call_output" => Some(ResultShape::ShellChunks),
        "computer_call_output" => Some(ResultShape::Screenshot),
        "mcp_approval_response" => Some(ResultShape::Approval),
        _ => None,
    }
}

#[must_use]
pub fn parse_request(format: Format, body: &[u8]) -> ToolTurn {
    match parse_value(format, body).and_then(|v| walk(format, &v, None)) {
        Ok(f) => ToolTurn::Parsed(f),
        Err(_) => ToolTurn::Unavailable("malformed"),
    }
}

/// The strict parse of a request body. `parse_request` and the request screen use the same one.
pub(crate) fn parse_value(format: Format, body: &[u8]) -> Result<Value, Malformed> {
    match format {
        Format::OpenAiResponses => parse_strict_responses(body, ResponsesDoc::Request),
        Format::AnthropicMessages => parse_strict_anthropic_request(body),
        Format::OpenAiChat => parse_strict(body),
    }
}

/// The one format walker (§ 6.8.1, S13). With a sink, it also gives the text of each tool result
/// and of each seen message source (§ 6.3.3). The sink never changes the facts or the errors.
pub(crate) fn walk<'a>(format: Format, v: &'a Value, sink: Option<&mut (dyn TextSink<'a> + '_)>) -> Result<TurnFacts, Malformed> {
    match format {
        Format::OpenAiResponses => responses(v, sink),
        Format::OpenAiChat => openai(v, sink),
        Format::AnthropicMessages => anthropic(v, sink),
    }
}

/// Receives the texts that the request screen reads. The texts borrow from the parsed request.
pub(crate) trait TextSink<'a> {
    /// The text parts of the tool result with this index in `TurnFacts.tool_results`. A result
    /// with no text (a screenshot, an approval) gives no parts. `complete` is `false` when the
    /// result holds content that the walker cannot read as text (an object `content`, a
    /// `document` block): the parts then miss some of the result, and the screen reports it.
    fn result(&mut self, index: usize, parts: &[&'a str], complete: bool);
    /// One text of a seen message source (§ 6.3.3).
    fn message(&mut self, text: &'a str);
}

/// The `text` of each part of `parts` whose `type` is `ty`. Other parts are skipped.
fn typed_texts<'a>(parts: &'a [Value], ty: &str, out: &mut Vec<&'a str>) {
    for p in parts {
        if p.get("type").and_then(Value::as_str) == Some(ty)
            && let Some(t) = p.get("text").and_then(Value::as_str)
        {
            out.push(t);
        }
    }
}

/// The texts of a content value: the string itself, or the `text` of each part of type `ty`.
fn content_texts<'a>(content: Option<&'a Value>, ty: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    match content {
        Some(Value::String(s)) => out.push(s.as_str()),
        Some(Value::Array(parts)) => typed_texts(parts, ty, &mut out),
        _ => {}
    }
    out
}

/// The part types of a tool result that hold no text: an image. Any other part type that is not
/// the text type of the format holds content the screen cannot read, except the Anthropic blocks
/// of [`anthropic_part_texts`].
const NO_TEXT_PARTS: [&str; 3] = ["image", "image_url", "input_image"];

/// The texts of the content of a tool result, and whether they are all of it: `false` for a
/// content that is not a string, an array or null, and for an array part that the screen cannot
/// read (see [`part_texts`]). `anthropic` turns on the Anthropic result blocks.
fn result_texts<'a>(content: Option<&'a Value>, ty: &str, anthropic: bool) -> (Vec<&'a str>, bool) {
    let mut out = Vec::new();
    let complete = content_parts(content, ty, anthropic, &mut out);
    (out, complete)
}

/// Push the texts of a content value: a string, or an array of parts. Return whether all of it was
/// read. An absent or null content holds nothing, so it is read in full.
fn content_parts<'a>(content: Option<&'a Value>, ty: &str, anthropic: bool, out: &mut Vec<&'a str>) -> bool {
    match content {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => {
            out.push(s);
            true
        }
        Some(Value::Array(parts)) => {
            // No early stop: a part that cannot be read does not hide the texts of later parts.
            let mut complete = true;
            for p in parts {
                complete &= part_texts(p, ty, anthropic, out);
            }
            complete
        }
        Some(_) => false,
    }
}

/// Push the texts of one part of a tool result. Return whether all of it was read: `true` for a
/// text part of type `ty` with a string `text` and for an image part, `false` for any other part,
/// except the Anthropic blocks when `anthropic` is set.
fn part_texts<'a>(p: &'a Value, ty: &str, anthropic: bool, out: &mut Vec<&'a str>) -> bool {
    let Some(part_ty) = p.get("type").and_then(Value::as_str) else {
        return false;
    };
    if part_ty == ty {
        return match p.get("text") {
            Some(Value::String(t)) => {
                out.push(t);
                true
            }
            _ => false,
        };
    }
    if NO_TEXT_PARTS.contains(&part_ty) {
        return true;
    }
    anthropic && anthropic_part_texts(p, part_ty, out)
}

/// The Anthropic result blocks that hold readable text (controller ruling, Task 22 review):
/// - `search_result`: its `title`, its `source` and the text blocks of its `content`;
/// - `document` with a `source` of type `text` (`data`) or `content` (text blocks): that text, and
///   its `title` and `context`. A document with a base64, URL or file source is not read (fail
///   closed);
/// - `tool_reference`: a tool name and no free text, so there is nothing to read.
fn anthropic_part_texts<'a>(p: &'a Value, part_ty: &str, out: &mut Vec<&'a str>) -> bool {
    match part_ty {
        "tool_reference" => true,
        "search_result" => {
            let strings = strings_of(p, &["title", "source"], out);
            content_parts(p.get("content"), "text", false, out) && strings
        }
        "document" => {
            let strings = strings_of(p, &["title", "context"], out);
            let source = p.get("source");
            let read = match source.and_then(|s| s.get("type")).and_then(Value::as_str) {
                Some("text") => match source.and_then(|s| s.get("data")) {
                    Some(Value::String(d)) => {
                        out.push(d);
                        true
                    }
                    _ => false,
                },
                Some("content") => content_parts(source.and_then(|s| s.get("content")), "text", false, out),
                _ => false,
            };
            read && strings
        }
        _ => false,
    }
}

/// Push the string values of these keys of `p`. Return `false` when one of them holds another
/// value (it cannot be read); an absent or null key holds nothing.
fn strings_of<'a>(p: &'a Value, keys: &[&str], out: &mut Vec<&'a str>) -> bool {
    keys.iter().fold(true, |complete, k| match p.get(*k) {
        None | Some(Value::Null) => complete,
        Some(Value::String(s)) => {
            out.push(s);
            complete
        }
        Some(_) => false,
    })
}

/// Give each text of a seen message source to the sink.
fn sink_messages<'a>(sink: &mut Option<&mut (dyn TextSink<'a> + '_)>, content: Option<&'a Value>, ty: &str) {
    if let Some(s) = sink.as_deref_mut() {
        for t in content_texts(content, ty) {
            s.message(t);
        }
    }
}

/// Whether a message role is a seen source (§ 6.3.3).
fn seen_role(role: Option<&str>) -> bool {
    matches!(role, Some("system" | "developer" | "user"))
}

/// The text parts of a Responses result item of this shape, and whether they are all of it (see
/// [`result_texts`]).
fn responses_result_texts(item: &Map<String, Value>, shape: ResultShape) -> (Vec<&str>, bool) {
    match shape {
        ResultShape::Text => result_texts(item.get("output"), "input_text", false),
        ResultShape::ShellChunks => {
            let mut out = Vec::new();
            let mut complete = true;
            match item.get("output") {
                None | Some(Value::Null) => {}
                Some(Value::Array(chunks)) => {
                    for c in chunks {
                        if !c.is_object() {
                            complete = false;
                        }
                        for k in ["stdout", "stderr"] {
                            match c.get(k) {
                                None | Some(Value::Null) => {}
                                Some(Value::String(t)) => out.push(t.as_str()),
                                Some(_) => complete = false,
                            }
                        }
                    }
                }
                Some(_) => complete = false,
            }
            (out, complete)
        }
        ResultShape::Screenshot | ResultShape::Approval => (Vec::new(), true),
    }
}

fn arr<'a>(o: &'a Map<String, Value>, k: &'static str) -> Result<&'a [Value], Malformed> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(Malformed(k)),
    }
}

fn obj(v: &Value) -> Result<&Map<String, Value>, Malformed> {
    v.as_object().ok_or(Malformed("object"))
}

fn req_str(o: &Map<String, Value>, k: &'static str) -> Result<String, Malformed> {
    opt_str(o.get(k), k)?.ok_or(Malformed(k))
}

fn openai<'a>(v: &'a Value, mut sink: Option<&mut (dyn TextSink<'a> + '_)>) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts {
        tool_choice: chat_choice(top),
        ..TurnFacts::default()
    };
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            Some("function") => {
                let func = opt_obj(t.get("function"), "function")?.ok_or(Malformed("function"))?;
                f.definitions.push(ToolDefinition {
                    name: req_str(func, "name")?,
                    namespace: None,
                    kind: ToolCallKind::Function,
                    schema: func.get("parameters").cloned().map(Arc::new),
                });
            }
            Some("custom") => {
                let c = opt_obj(t.get("custom"), "custom")?.ok_or(Malformed("custom"))?;
                f.definitions.push(ToolDefinition {
                    name: req_str(c, "name")?,
                    namespace: None,
                    kind: ToolCallKind::Custom,
                    schema: None,
                });
            }
            _ => {}
        }
    }
    for d in arr(top, "functions")? {
        let d = obj(d)?;
        f.definitions.push(ToolDefinition {
            name: req_str(d, "name")?,
            namespace: None,
            kind: ToolCallKind::LegacyFunctionCall,
            schema: d.get("parameters").cloned().map(Arc::new),
        });
    }
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        match opt_str(m.get("role"), "role")?.as_deref() {
            Some("assistant") => {
                for c in arr(m, "tool_calls")? {
                    let c = obj(c)?;
                    let (name, arguments, kind) = prior_body(c)?;
                    f.prior_calls.push(PriorCall {
                        id: opt_str(c.get("id"), "id")?,
                        name,
                        arguments,
                        kind,
                        local_shell: false,
                        item_id: None,
                    });
                }
                if let Some(fc) = opt_obj(m.get("function_call"), "function_call")? {
                    f.prior_calls.push(PriorCall {
                        id: None,
                        name: req_str(fc, "name")?,
                        arguments: opt_str(fc.get("arguments"), "arguments")?.unwrap_or_default(),
                        kind: ToolCallKind::LegacyFunctionCall,
                        local_shell: false,
                        item_id: None,
                    });
                }
            }
            Some(role @ ("tool" | "function")) => {
                let (call_id, name) = if role == "tool" {
                    (opt_str(m.get("tool_call_id"), "tool_call_id")?, None)
                } else {
                    (None, opt_str(m.get("name"), "name")?)
                };
                if let Some(s) = sink.as_deref_mut() {
                    let (parts, complete) = result_texts(m.get("content"), "text", false);
                    s.result(f.tool_results.len(), &parts, complete);
                }
                f.tool_results.push(ToolResultRef {
                    call_id,
                    name,
                    item_id: None,
                    shape: ResultShape::Text,
                });
            }
            role => {
                if seen_role(role) {
                    sink_messages(&mut sink, m.get("content"), "text");
                }
            }
        }
    }
    f.latest = chat_latest(arr(top, "messages")?);
    Ok(f)
}

/// The last assistant message of a chat request, and the `tool` messages after the last assistant
/// message that has tool calls (the forged-history anchor, P11). With no such message, every `tool`
/// message counts. A legacy
/// `function_call` has no id, and a `function` result has no call id, so both are ignored.
fn chat_latest(messages: &[Value]) -> LatestTurn {
    let mut l = LatestTurn::default();
    let last = messages.iter().rposition(|m| role(m) == Some("assistant"));
    if let Some(m) = last.map(|i| &messages[i]) {
        for c in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = c.get("id").and_then(Value::as_str) {
                l.call(id);
            }
        }
        l.content(m.get("content"), "text");
    }
    let anchor = messages
        .iter()
        .rposition(|m| role(m) == Some("assistant") && m.get("tool_calls").and_then(Value::as_array).is_some_and(|c| !c.is_empty()));
    // Without a call-bearing assistant message, every result counts (a forged result has no call
    // to match), also one before a text-only assistant message.
    for m in &messages[anchor.map_or(0, |a| a + 1)..] {
        if role(m) == Some("tool")
            && let Some(id) = m.get("tool_call_id").and_then(Value::as_str)
        {
            l.result(id);
        }
    }
    l
}

/// The (name, arguments) of a prior OpenAI tool call: a `function` body's name and arguments, or a
/// `custom` body's name and input. An entry with neither body, both, or a body that disagrees with
/// its `type` is malformed: an empty name would read as a call to no tool at all.
fn prior_body(c: &Map<String, Value>) -> Result<(String, String, ToolCallKind), Malformed> {
    let ty = opt_str(c.get("type"), "type")?;
    let (body, input, kind) = match (opt_obj(c.get("function"), "function")?, opt_obj(c.get("custom"), "custom")?, ty.as_deref()) {
        (Some(x), None, None | Some("function")) => (x, "arguments", ToolCallKind::Function),
        (None, Some(x), None | Some("custom")) => (x, "input", ToolCallKind::Custom),
        _ => return Err(Malformed("tool_call")),
    };
    Ok((req_str(body, "name")?, opt_str(body.get(input), input)?.unwrap_or_default(), kind))
}

/// The request side of a Responses turn (spec § 7.2), read by `parse_request`.
///
/// `tools[]` gives the definitions: a `function` or `custom` tool by name, the tools of a
/// `namespace` tool with that namespace, and any other tool by its `type` (kind `Other`). A
/// `tool_search` tool makes the set incomplete, as do an `additional_tools` and a
/// `tool_search_output` input item. The `input` items of the client-run types of § 6.2 are the
/// prior calls (read as a response reads them, `assembly::tool_call`), and each output item is a
/// tool result (`responses_result_shape` is the one list of the result types). An `mcp_approval_response` refers to its request by `approval_request_id`, and the
/// id of an `mcp_approval_request` call is its item `id`. A string `input` has no items.
fn responses<'a>(v: &'a Value, mut sink: Option<&mut (dyn TextSink<'a> + '_)>) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts {
        tool_choice: responses_choice(top.get("tool_choice")),
        ..TurnFacts::default()
    };
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        let ty = req_str(t, "type")?;
        match ty.as_str() {
            "function" | "custom" => f.definitions.push(responses_definition(t, &ty, None)?),
            "namespace" => {
                let ns = req_str(t, "name")?;
                for inner in arr(t, "tools")? {
                    let inner = obj(inner)?;
                    let ity = req_str(inner, "type")?;
                    if !matches!(ity.as_str(), "function" | "custom") {
                        return Err(Malformed("type"));
                    }
                    f.definitions.push(responses_definition(inner, &ity, Some(ns.clone()))?);
                }
            }
            other => {
                if other == "tool_search" {
                    f.definitions_complete = false;
                }
                f.definitions.push(ToolDefinition {
                    name: ty,
                    namespace: None,
                    kind: ToolCallKind::Other,
                    schema: None,
                });
            }
        }
    }
    if let Some(s) = sink.as_deref_mut() {
        if let Some(Value::String(t)) = top.get("instructions") {
            s.message(t);
        }
        if let Some(Value::String(t)) = top.get("input") {
            s.message(t);
        }
    }
    // A string `input` is one user message. Only an array has items.
    let items: &[Value] = match top.get("input") {
        Some(Value::Array(a)) => a,
        None | Some(Value::Null | Value::String(_)) => &[],
        Some(_) => return Err(Malformed("input")),
    };
    for (i, item) in items.iter().enumerate() {
        let item = obj(item)?;
        // A message in the short form has no `type`. It is still a seen source (§ 6.3.3).
        let ty = opt_str(item.get("type"), "type")?;
        if matches!(ty.as_deref(), None | Some("message")) && seen_role(item.get("role").and_then(Value::as_str)) {
            sink_messages(&mut sink, item.get("content"), "input_text");
        }
        let Some(ty) = ty else { continue };
        if let Some(shape) = responses_result_shape(&ty) {
            let key = if shape == ResultShape::Approval { "approval_request_id" } else { "call_id" };
            // Only a `local_shell_call_output` refers to its call by `id`. For the other output
            // types, `id` is the id of the output item itself.
            let item_id = if ty == "local_shell_call_output" { opt_str(item.get("id"), "id")? } else { None };
            let call_id = opt_str(item.get(key), key)?;
            if let Some(s) = sink.as_deref_mut() {
                let (parts, complete) = responses_result_texts(item, shape);
                s.result(f.tool_results.len(), &parts, complete);
            }
            f.tool_results.push(ToolResultRef { call_id, name: None, item_id, shape });
            continue;
        }
        match ty.as_str() {
            // The referenced items are not in the request, so the history is unknown (D3, § 7.2).
            "item_reference" => return Err(Malformed("item_reference")),
            "additional_tools" | "tool_search_output" => f.definitions_complete = false,
            _ => {
                // An unknown type is client-run of kind `Other` for a response (fail closed), but in
                // a request it is just an item the gateway does not read (`message`, `reasoning`).
                if let ItemClass::ClientRun(kind) = item_class(&ty)
                    && kind != ToolCallKind::Other
                {
                    let c = tool_call(u32::try_from(i).map_err(|_| Malformed("input"))?, item, kind)?;
                    let local_shell = kind == ToolCallKind::LocalShell;
                    f.prior_calls.push(PriorCall {
                        // The display name, so a result is decided as its call is (P-7).
                        name: c.display_name(), // PRIOR-DISPLAY-NAME
                        id: c.id,
                        arguments: c.arguments,
                        kind,
                        local_shell,
                        item_id: if local_shell { opt_str(item.get("id"), "id")? } else { None },
                    });
                }
            }
        }
    }
    f.latest = responses_latest(items);
    Ok(f)
}

/// What a Responses input item is to the newest model turn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelItem {
    /// An assistant message.
    Text,
    Reasoning,
    /// A client-run call item.
    Call,
    /// A provider-run item (`web_search_call` and the like). It is part of the model run, but it is
    /// no client-run call and has no call id.
    ProviderRun,
    Other,
}

fn model_item(item: &Value) -> ModelItem {
    let ty = item.get("type").and_then(Value::as_str);
    match ty {
        None | Some("message") if role(item) == Some("assistant") => ModelItem::Text,
        Some("reasoning") => ModelItem::Reasoning,
        Some(t) if responses_result_shape(t).is_none() && item_class(t) == ItemClass::ProviderRun => ModelItem::ProviderRun,
        Some(t) if responses_result_shape(t).is_none() && matches!(item_class(t), ItemClass::ClientRun(k) if k != ToolCallKind::Other) => ModelItem::Call,
        _ => ModelItem::Other,
    }
}

/// The last model turn of a Responses request is the longest run of contiguous items that ends at
/// the last assistant message or client-run call, and holds only assistant messages, reasoning
/// items and client-run calls. The results are the result items after the start of the last such run
/// that holds a call item (P11), which can be an earlier run than this one. With no call item, every
/// result item counts.
fn responses_latest(items: &[Value]) -> LatestTurn {
    let mut l = LatestTurn::default();
    let kinds: Vec<ModelItem> = items.iter().map(model_item).collect();
    let end = kinds.iter().rposition(|k| matches!(k, ModelItem::Text | ModelItem::Call));
    let start = end.map(|e| kinds[..=e].iter().rposition(|k| *k == ModelItem::Other).map_or(0, |i| i + 1));
    let run = end.zip(start).map_or(0..0, |(e, s)| s..e + 1);
    for (item, kind) in items[run.clone()].iter().zip(&kinds[run]) {
        match kind {
            ModelItem::Text => l.content(item.get("content"), "output_text"),
            ModelItem::Call => {
                let key = if item.get("type").and_then(Value::as_str) == Some("mcp_approval_request") {
                    "id"
                } else {
                    "call_id"
                };
                if let Some(id) = item.get(key).and_then(Value::as_str) {
                    l.call(id);
                }
            }
            ModelItem::Reasoning | ModelItem::ProviderRun | ModelItem::Other => {}
        }
    }
    // The results anchor on the last run that holds a call item, not on the last run (P11). Without
    // a call item every result counts (a forged result has no call to match).
    let anchor = kinds
        .iter()
        .rposition(|k| *k == ModelItem::Call)
        .map_or(0, |call_end| kinds[..=call_end].iter().rposition(|k| *k == ModelItem::Other).map_or(0, |i| i + 1));
    for item in &items[anchor..] {
        let Some(ty) = item.get("type").and_then(Value::as_str) else { continue };
        let Some(shape) = responses_result_shape(ty) else { continue };
        let key = if shape == ResultShape::Approval { "approval_request_id" } else { "call_id" };
        let id = item
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| if ty == "local_shell_call_output" { item.get("id").and_then(Value::as_str) } else { None });
        if let Some(id) = id {
            l.result(id);
        }
    }
    l
}

/// A tool name from a `{"name": ..}` object, or `None` when it is absent or not a string.
fn name_of(o: &Map<String, Value>) -> Option<String> {
    o.get("name").and_then(Value::as_str).map(str::to_owned)
}

/// The `tool_choice` of a chat request, or the legacy `function_call` when `tool_choice` is absent.
fn chat_choice(top: &Map<String, Value>) -> ToolChoice {
    match top.get("tool_choice") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => return string_choice(s),
        Some(Value::Object(o)) => return chat_object_choice(o),
        Some(_) => return ToolChoice::Unknown,
    }
    match top.get("function_call") {
        None | Some(Value::Null) => ToolChoice::Auto,
        Some(Value::String(s)) if s == "auto" => ToolChoice::Auto,
        Some(Value::String(s)) if s == "none" => ToolChoice::None,
        Some(Value::Object(o)) => name_of(o).map_or(ToolChoice::Unknown, ToolChoice::Named),
        Some(_) => ToolChoice::Unknown,
    }
}

fn string_choice(s: &str) -> ToolChoice {
    match s {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" => ToolChoice::Required,
        _ => ToolChoice::Unknown,
    }
}

/// The name of a chat `{"type":"function","function":{"name":..}}` or `custom` entry.
fn chat_entry_name(v: &Value) -> Option<String> {
    chat_entry_name_of(v.as_object()?)
}

fn chat_entry_name_of(o: &Map<String, Value>) -> Option<String> {
    let ty = o.get("type")?.as_str()?;
    if !matches!(ty, "function" | "custom") {
        return None;
    }
    name_of(o.get(ty)?.as_object()?)
}

fn chat_object_choice(o: &Map<String, Value>) -> ToolChoice {
    let Some(ty) = o.get("type").and_then(Value::as_str) else { return ToolChoice::Unknown };
    match ty {
        "function" | "custom" => chat_entry_name_of(o).map_or(ToolChoice::Unknown, ToolChoice::Named),
        "allowed_tools" => {
            let tools = o.get("allowed_tools").and_then(Value::as_object).and_then(|a| a.get("tools")).and_then(Value::as_array);
            tools
                .and_then(|t| t.iter().map(chat_entry_name).collect::<Option<Vec<_>>>())
                .map_or(ToolChoice::Unknown, ToolChoice::AllowedTools)
        }
        _ => ToolChoice::Unknown,
    }
}

/// A Responses `function` or `custom` entry: its display name (`namespace.name` with a namespace).
fn responses_entry_name(o: &Map<String, Value>) -> Option<String> {
    let name = name_of(o)?;
    match o.get("namespace").and_then(Value::as_str) {
        Some(ns) => Some(format!("{ns}.{name}")),
        None => Some(name),
    }
}

fn responses_choice(v: Option<&Value>) -> ToolChoice {
    match v {
        None | Some(Value::Null) => ToolChoice::Auto,
        Some(Value::String(s)) => string_choice(s),
        Some(Value::Object(o)) => {
            let Some(ty) = o.get("type").and_then(Value::as_str) else { return ToolChoice::Unknown };
            match ty {
                "function" | "custom" => responses_entry_name(o).map_or(ToolChoice::Unknown, ToolChoice::Named),
                "allowed_tools" => {
                    let tools = o.get("tools").and_then(Value::as_array);
                    tools
                        .and_then(|t| {
                            t.iter()
                                .map(|e| {
                                    let e = e.as_object()?;
                                    matches!(e.get("type").and_then(Value::as_str)?, "function" | "custom").then(|| responses_entry_name(e)).flatten()
                                })
                                .collect::<Option<Vec<_>>>()
                        })
                        .map_or(ToolChoice::Unknown, ToolChoice::AllowedTools)
                }
                // A remote MCP tool is named by server and tool, not by a display name we can match.
                "mcp" => ToolChoice::Unknown,
                // A known built-in or hosted tool is named by its type; any other type is unknown.
                "local_shell"
                | "shell"
                | "apply_patch"
                | "computer"
                | "computer_use_preview"
                | "tool_search"
                | "web_search"
                | "web_search_preview"
                | "file_search"
                | "code_interpreter"
                | "image_generation" => ToolChoice::Named(ty.to_owned()),
                _ => ToolChoice::Unknown,
            }
        }
        Some(_) => ToolChoice::Unknown,
    }
}

fn anthropic_choice(v: Option<&Value>) -> ToolChoice {
    match v {
        None | Some(Value::Null) => ToolChoice::Auto,
        Some(Value::Object(o)) => match o.get("type").and_then(Value::as_str) {
            Some("auto") => ToolChoice::Auto,
            Some("any") => ToolChoice::Required,
            Some("none") => ToolChoice::None,
            Some("tool") => name_of(o).map_or(ToolChoice::Unknown, ToolChoice::Named),
            _ => ToolChoice::Unknown,
        },
        Some(_) => ToolChoice::Unknown,
    }
}

fn responses_definition(t: &Map<String, Value>, ty: &str, namespace: Option<String>) -> Result<ToolDefinition, Malformed> {
    let function = ty == "function";
    Ok(ToolDefinition {
        name: req_str(t, "name")?,
        namespace,
        kind: if function { ToolCallKind::Function } else { ToolCallKind::Custom },
        schema: if function { t.get("parameters").cloned().map(Arc::new) } else { None },
    })
}

fn anthropic<'a>(v: &'a Value, mut sink: Option<&mut (dyn TextSink<'a> + '_)>) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts {
        tool_choice: anthropic_choice(top.get("tool_choice")),
        ..TurnFacts::default()
    };
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            None | Some("custom") => f.definitions.push(ToolDefinition {
                name: req_str(t, "name")?,
                namespace: None,
                kind: ToolCallKind::AnthropicToolUse,
                schema: t.get("input_schema").cloned().map(Arc::new),
            }),
            Some(_) => f.server_tools = true, // a server tool
        }
    }
    sink_messages(&mut sink, top.get("system"), "text");
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        // Only `user` text is a seen source. Its `tool_result` blocks are results, not messages.
        if m.get("role").and_then(Value::as_str) == Some("user") {
            sink_messages(&mut sink, m.get("content"), "text");
        }
        let Some(Value::Array(blocks)) = m.get("content") else { continue };
        for b in blocks {
            let b = obj(b)?;
            match opt_str(b.get("type"), "type")?.as_deref() {
                Some("tool_use") => f.prior_calls.push(PriorCall {
                    id: opt_str(b.get("id"), "id")?,
                    name: req_str(b, "name")?,
                    arguments: b.get("input").map(Value::to_string).unwrap_or_default(),
                    kind: ToolCallKind::AnthropicToolUse,
                    local_shell: false,
                    item_id: None,
                }),
                Some("tool_result") => {
                    let call_id = opt_str(b.get("tool_use_id"), "tool_use_id")?;
                    if let Some(s) = sink.as_deref_mut() {
                        let (parts, complete) = result_texts(b.get("content"), "text", true);
                        s.result(f.tool_results.len(), &parts, complete);
                    }
                    f.tool_results.push(ToolResultRef {
                        call_id,
                        name: None,
                        item_id: None,
                        shape: ResultShape::Text,
                    });
                }
                _ => {}
            }
        }
    }
    f.latest = anthropic_latest(arr(top, "messages")?);
    Ok(f)
}

/// The last assistant message of an Anthropic request, and the `tool_result` blocks after the last
/// assistant message that has a `tool_use` block (the forged-history anchor, P11). With no such
/// message, every `tool_result` block counts.
fn anthropic_latest(messages: &[Value]) -> LatestTurn {
    let mut l = LatestTurn::default();
    let last = messages.iter().rposition(|m| role(m) == Some("assistant"));
    if let Some(i) = last {
        let content = messages[i].get("content");
        for b in content.and_then(Value::as_array).into_iter().flatten() {
            if b.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(id) = b.get("id").and_then(Value::as_str)
            {
                l.call(id);
            }
        }
        l.content(content, "text");
    }
    let anchor = messages.iter().rposition(|m| {
        role(m) == Some("assistant")
            && m.get("content")
                .and_then(Value::as_array)
                .is_some_and(|bs| bs.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")))
    });
    // Without an assistant message that has a `tool_use` block, every result counts.
    for m in &messages[anchor.map_or(0, |a| a + 1)..] {
        for b in m.get("content").and_then(Value::as_array).into_iter().flatten() {
            if b.get("type").and_then(Value::as_str) == Some("tool_result")
                && let Some(id) = b.get("tool_use_id").and_then(Value::as_str)
            {
                l.result(id);
            }
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts(format: Format, v: serde_json::Value) -> TurnFacts {
        match parse_request(format, v.to_string().as_bytes()) {
            ToolTurn::Parsed(f) => f,
            ToolTurn::Unavailable(r) => panic!("unavailable: {r}"),
        }
    }

    #[test]
    fn chat_and_anthropic_definitions_are_complete() {
        let oa = facts(
            Format::OpenAiChat,
            json!({"model":"m","tools":[{"type":"function","function":{"name":"read"}}],"messages":[{"role":"user","content":"hi"}]}),
        );
        let an = facts(
            Format::AnthropicMessages,
            json!({"model":"m","tools":[{"name":"read","input_schema":{}}],"messages":[{"role":"user","content":"hi"}]}),
        );
        for f in [oa, an] {
            assert!(f.definitions_complete);
            assert!(!f.definitions.is_empty());
            assert!(f.definitions.iter().all(|d| d.namespace.is_none()));
        }
    }

    #[test]
    fn an_anthropic_request_with_user_data_keys_parses() {
        let props = json!({"properties":{"Name":{},"ID":{}}});
        for body in [
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],
                   "tools":[{"name":"read","input_schema":props,"input_examples":[{"Name":1,"constructor":2}]}]}),
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"output_format":{"type":"json_schema","schema":props}}),
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"output_config":{"format":{"type":"json_schema","schema":props}}}),
        ] {
            assert!(matches!(parse_request(Format::AnthropicMessages, body.to_string().as_bytes()), ToolTurn::Parsed(_)), "{body}");
        }
        let bad = json!({"model":"m","messages":[],"Type":"x"});
        assert!(matches!(parse_request(Format::AnthropicMessages, bad.to_string().as_bytes()), ToolTurn::Unavailable("malformed")));
    }

    #[test]
    fn an_openai_request_yields_definitions_calls_and_results() {
        let f = facts(
            Format::OpenAiChat,
            json!({"model":"m","tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}},{"type":"custom","custom":{"name":"sql"}}],
                   "functions":[{"name":"old","parameters":{}}],
                   "messages":[{"role":"user","content":"hi"},
                               {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]},
                               {"role":"tool","tool_call_id":"c1","content":"SECRET RESULT"},
                               {"role":"assistant","function_call":{"name":"old","arguments":"{}"}},
                               {"role":"function","name":"old","content":"SECRET"}]}),
        );
        assert_eq!(
            f.definitions.iter().map(|d| (d.name.as_str(), d.kind)).collect::<Vec<_>>(),
            [("read", ToolCallKind::Function), ("sql", ToolCallKind::Custom), ("old", ToolCallKind::LegacyFunctionCall)]
        );
        assert_eq!(f.prior_calls.len(), 2);
        assert_eq!(
            f.tool_results,
            vec![
                ToolResultRef {
                    call_id: Some("c1".into()),
                    name: None,
                    item_id: None,
                    shape: ResultShape::Text
                },
                ToolResultRef {
                    call_id: None,
                    name: Some("old".into()),
                    item_id: None,
                    shape: ResultShape::Text
                }
            ]
        );
        assert!(!format!("{f:?}").contains("SECRET"), "tool result content is never kept");
    }

    #[test]
    fn each_responses_result_type_has_its_shape() {
        use ResultShape::{Approval, Screenshot, ShellChunks, Text};
        let table = [
            ("function_call_output", Some(Text)),
            ("custom_tool_call_output", Some(Text)),
            ("local_shell_call_output", Some(Text)),
            ("apply_patch_call_output", Some(Text)),
            ("shell_call_output", Some(ShellChunks)),
            ("computer_call_output", Some(Screenshot)),
            ("mcp_approval_response", Some(Approval)),
            ("function_call", None),
            ("computer_call", None),
            ("mcp_call", None),
            ("message", None),
        ];
        for (ty, shape) in table {
            assert_eq!(responses_result_shape(ty), shape, "{ty}");
        }
    }

    #[test]
    fn an_anthropic_request_yields_definitions_calls_and_results() {
        let f = facts(
            Format::AnthropicMessages,
            json!({"model":"c","tools":[{"name":"rm","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search"}],
                   "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{"p":"/"}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"SECRET"}]}]}),
        );
        assert!(f.tool_results.iter().all(|r| r.shape == ResultShape::Text));
        assert_eq!(f.definitions.len(), 1);
        assert!(f.server_tools);
        assert_eq!(f.prior_calls[0].arguments, r#"{"p":"/"}"#);
        assert_eq!(f.tool_results[0].call_id.as_deref(), Some("t1"));
        let plain = facts(Format::AnthropicMessages, json!({"model":"c","tools":[{"name":"rm","input_schema":{}}],"messages":[]}));
        assert!(!plain.server_tools);
    }

    /// Schemas and prior inputs are application data (I-1): PascalCase and `constructor` keys in
    /// them do not make the request unreadable. A duplicate key inside them still does.
    #[test]
    fn schema_and_input_keys_are_application_data() {
        let openai = br#"{"tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object","properties":{"Id":{"type":"string"},"constructor":{}}}}}],"functions":[{"name":"g","parameters":{"Name":1}}],"messages":[]}"#;
        assert!(matches!(parse_request(Format::OpenAiChat, openai), ToolTurn::Parsed(f) if f.definitions.len() == 2));
        let anthropic = br#"{"tools":[{"name":"f","input_schema":{"properties":{"Type":{},"prototype":{}}}}],"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"f","input":{"Name":"Alice","constructor":1}}]}]}"#;
        let ToolTurn::Parsed(f) = parse_request(Format::AnthropicMessages, anthropic) else {
            panic!("unavailable")
        };
        assert_eq!(f.prior_calls[0].arguments, r#"{"Name":"Alice","constructor":1}"#);
        for bad in [
            &br#"{"tools":[{"name":"f","input_schema":{"a":1,"a":2}}]}"#[..],
            br#"{"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"f","input":{"__proto__":1}}]}]}"#,
        ] {
            assert!(matches!(parse_request(Format::AnthropicMessages, bad), ToolTurn::Unavailable(_)), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_prior_custom_call_keeps_its_name_and_input() {
        let f = facts(
            Format::OpenAiChat,
            json!({"messages":[{"role":"assistant","tool_calls":[
                {"id":"c1","type":"custom","custom":{"name":"sql","input":"SELECT 1"}},
                {"id":"c2","type":"function","function":{"name":"read","arguments":"{}"}}]}]}),
        );
        assert_eq!(
            f.prior_calls.iter().map(|c| (c.id.as_deref(), c.name.as_str(), c.arguments.as_str())).collect::<Vec<_>>(),
            [(Some("c1"), "sql", "SELECT 1"), (Some("c2"), "read", "{}")]
        );
    }

    #[test]
    fn a_prior_call_without_a_valid_body_is_unavailable() {
        for bad in [
            &br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","type":"function"}]}]}"#[..],
            br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","type":"shell","shell":{"cmd":"ls"}}]}]}"#,
            br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","type":"custom","custom":{"input":"x"}}]}]}"#,
            br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","type":"custom","custom":{"name":"sql","input":1}}]}]}"#,
            br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","function":{"name":"a"},"custom":{"name":"b"}}]}]}"#,
            br#"{"messages":[{"role":"assistant","tool_calls":[{"id":"c","type":"custom","function":{"name":"a","arguments":"{}"}}]}]}"#,
        ] {
            assert_eq!(parse_request(Format::OpenAiChat, bad), ToolTurn::Unavailable("malformed"), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_malformed_request_is_unavailable_never_empty() {
        for bad in [&b"{"[..], br#"{"a":1,"a":2}"#, br#"{"messages":"x"}"#, br#"{"tools":[{"type":"function","function":{"name":1}}]}"#] {
            assert!(matches!(parse_request(Format::OpenAiChat, bad), ToolTurn::Unavailable(_)), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_responses_request_yields_definitions_calls_and_results() {
        let f = responses(
            &json!({"model":"m",
            "tools":[{"type":"function","name":"read","parameters":{"type":"object"}},
                     {"type":"custom","name":"sql"},
                     {"type":"namespace","name":"db","tools":[{"type":"function","name":"query","parameters":{}},{"type":"custom","name":"raw"}]},
                     {"type":"web_search"}],
            "input":[{"role":"user","content":"hi"},
                     {"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":"{\"p\":1}","status":"completed"},
                     {"type":"function_call_output","call_id":"c1","output":"SECRET"},
                     {"type":"custom_tool_call","id":"ctc_1","call_id":"c2","name":"sql","input":"SELECT 1"},
                     {"type":"custom_tool_call_output","call_id":"c2","output":"SECRET"},
                     {"type":"shell_call","id":"sh_1","call_id":"c3","status":"completed","action":{"commands":["ls"]}},
                     {"type":"shell_call_output","call_id":"c3","output":[]},
                     {"type":"computer_call_output","call_id":"c4","output":{}},
                     {"type":"local_shell_call_output","call_id":"c5","output":"x"},
                     {"type":"apply_patch_call_output","call_id":"c6","status":"completed"},
                     {"type":"mcp_approval_request","id":"mcpr_1","name":"rm","server_label":"srv","arguments":"{}"},
                     {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true},
                     {"type":"reasoning","id":"rs_1","summary":[]}]}),
            None,
        )
        .unwrap();
        assert!(f.definitions_complete);
        assert_eq!(
            f.definitions.iter().map(|d| (d.name.as_str(), d.namespace.as_deref(), d.kind)).collect::<Vec<_>>(),
            [
                ("read", None, ToolCallKind::Function),
                ("sql", None, ToolCallKind::Custom),
                ("query", Some("db"), ToolCallKind::Function),
                ("raw", Some("db"), ToolCallKind::Custom),
                ("web_search", None, ToolCallKind::Other),
            ]
        );
        assert!(f.definitions[0].schema.is_some() && f.definitions[1].schema.is_none());
        assert_eq!(
            f.prior_calls.iter().map(|c| (c.id.as_deref(), c.name.as_str())).collect::<Vec<_>>(),
            [(Some("c1"), "read"), (Some("c2"), "sql"), (Some("c3"), "shell"), (Some("mcpr_1"), "srv.rm")]
        );
        assert_eq!(f.prior_calls[0].arguments, "{\"p\":1}");
        assert_eq!(f.prior_calls[1].arguments, "SELECT 1");
        let ids: Vec<_> = f.tool_results.iter().map(|r| r.call_id.as_deref().unwrap()).collect();
        assert_eq!(ids, ["c1", "c2", "c3", "c4", "c5", "c6", "mcpr_1"]);
        assert!(!format!("{f:?}").contains("SECRET"), "tool result content is never kept");
        let shapes: Vec<_> = f.tool_results.iter().map(|r| r.shape).collect();
        use ResultShape::{Approval, Screenshot, ShellChunks, Text};
        assert_eq!(shapes, [Text, Text, ShellChunks, Screenshot, Text, Text, Approval]);
    }

    #[test]
    fn a_local_shell_call_and_output_keep_their_item_ids() {
        let f = responses(
            &json!({"input":[
            {"type":"local_shell_call","id":"ls_1","call_id":"c1","action":{"type":"exec","command":["ls"],"env":{}},"status":"completed"},
            {"type":"local_shell_call_output","id":"ls_1","output":"x"},
            {"type":"function_call_output","id":"fco_1","call_id":"c2","output":"x"}]}),
            None,
        )
        .unwrap();
        assert_eq!(
            f.prior_calls.iter().map(|c| (c.id.as_deref(), c.local_shell, c.item_id.as_deref())).collect::<Vec<_>>(),
            [(Some("c1"), true, Some("ls_1"))]
        );
        // Only a `local_shell_call_output` refers to its call by `id`.
        assert_eq!(
            f.tool_results.iter().map(|r| (r.call_id.as_deref(), r.item_id.as_deref())).collect::<Vec<_>>(),
            [(None, Some("ls_1")), (Some("c2"), None)]
        );
    }

    #[test]
    fn a_namespaced_prior_call_keeps_its_display_name() {
        let f = responses(&json!({"input":[{"type":"function_call","call_id":"c1","namespace":"db","name":"query","arguments":"{}"}]}), None).unwrap();
        assert_eq!(f.prior_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["db.query"]);
    }

    #[test]
    fn a_string_input_has_no_calls() {
        let f = responses(&json!({"model":"m","input":"hi"}), None).unwrap();
        assert_eq!(f, TurnFacts::default());
        assert!(f.definitions_complete);
    }

    #[test]
    fn run_time_tools_make_definitions_incomplete() {
        for v in [
            json!({"tools":[{"type":"tool_search"}],"input":"hi"}),
            json!({"input":[{"type":"additional_tools","id":"at_1","tools":[]}]}),
            json!({"input":[{"type":"tool_search_output","id":"tso_1","tools":[]}]}),
        ] {
            assert!(!responses(&v, None).unwrap().definitions_complete, "{v}");
        }
    }

    #[test]
    fn an_unreadable_responses_request_is_malformed() {
        for v in [
            json!([]),
            json!({"tools":"x"}),
            json!({"tools":[{"name":"f"}]}),
            json!({"tools":[{"type":"function"}]}),
            json!({"tools":[{"type":"namespace","name":"n","tools":[{"type":"web_search"}]}]}),
            json!({"input":["x"]}),
            json!({"input":[{"type":"item_reference","id":"x"}]}),
            json!({"input":1}),
            json!({"input":{"type":"function_call"}}),
            json!({"input":true}),
            json!({"input":[{"type":"function_call","name":"f"}]}),
        ] {
            assert!(responses(&v, None).is_err(), "{v}");
        }
    }

    #[test]
    fn parse_request_reads_a_responses_request() {
        let body = json!({"model":"m","stream":true,
            "tools":[{"type":"function","name":"read","parameters":{"type":"object","properties":{"Id":{"type":"integer","maximum":1e300},"constructor":{}}}}],
            "metadata":{"Id":"x"},
            "input":[{"type":"function_call","id":"fc_1","call_id":"c1","name":"read","arguments":"{}","status":"completed"},
                     {"type":"function_call_output","call_id":"c1","output":"SECRET"}]});
        let f = facts(Format::OpenAiResponses, body);
        assert_eq!(f.definitions.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["read"]);
        assert_eq!(f.prior_calls.iter().map(|c| c.id.as_deref()).collect::<Vec<_>>(), [Some("c1")]);
        assert_eq!(f.tool_results.len(), 1);
        for bad in [&b"{"[..], br#"{"a":1,"a":2}"#, br#"{"input":[{"type":"item_reference","id":"x"}]}"#, br#"{"Type":"x","type":"y"}"#] {
            assert!(
                matches!(parse_request(Format::OpenAiResponses, bad), ToolTurn::Unavailable("malformed")),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    fn choice(format: Format, v: serde_json::Value) -> ToolChoice {
        facts(format, v).tool_choice
    }

    #[test]
    fn chat_tool_choice() {
        let c = |tc: serde_json::Value| choice(Format::OpenAiChat, json!({"tool_choice": tc}));
        assert_eq!(choice(Format::OpenAiChat, json!({})), ToolChoice::Auto);
        assert_eq!(c(json!("auto")), ToolChoice::Auto);
        assert_eq!(c(json!("none")), ToolChoice::None);
        assert_eq!(c(json!("required")), ToolChoice::Required);
        assert_eq!(c(json!({"type":"function","function":{"name":"f"}})), ToolChoice::Named("f".into()));
        assert_eq!(c(json!({"type":"custom","custom":{"name":"c"}})), ToolChoice::Named("c".into()));
        assert_eq!(
            c(json!({"type":"allowed_tools","allowed_tools":{"mode":"auto","tools":[{"type":"function","function":{"name":"a"}}]}})),
            ToolChoice::AllowedTools(vec!["a".into()])
        );
        assert_eq!(c(json!("weird")), ToolChoice::Unknown);
        assert_eq!(c(json!({"type":"mystery"})), ToolChoice::Unknown);
        assert_eq!(c(json!({"type":"function"})), ToolChoice::Unknown);
        let l = |fc: serde_json::Value| choice(Format::OpenAiChat, json!({"function_call": fc}));
        assert_eq!(l(json!("auto")), ToolChoice::Auto);
        assert_eq!(l(json!("none")), ToolChoice::None);
        assert_eq!(l(json!({"name":"f"})), ToolChoice::Named("f".into()));
        assert_eq!(l(json!("other")), ToolChoice::Unknown);
        assert_eq!(l(json!(5)), ToolChoice::Unknown);
    }

    #[test]
    fn responses_tool_choice() {
        let c = |tc: serde_json::Value| choice(Format::OpenAiResponses, json!({"tool_choice": tc}));
        assert_eq!(choice(Format::OpenAiResponses, json!({})), ToolChoice::Auto);
        assert_eq!(c(json!("auto")), ToolChoice::Auto);
        assert_eq!(c(json!("none")), ToolChoice::None);
        assert_eq!(c(json!("required")), ToolChoice::Required);
        assert_eq!(c(json!({"type":"function","name":"f"})), ToolChoice::Named("f".into()));
        assert_eq!(c(json!({"type":"function","name":"f","namespace":"ns"})), ToolChoice::Named("ns.f".into()));
        assert_eq!(c(json!({"type":"custom","name":"c"})), ToolChoice::Named("c".into()));
        assert_eq!(
            c(json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"a"}]})),
            ToolChoice::AllowedTools(vec!["a".into()])
        );
        assert_eq!(c(json!({"type":"local_shell"})), ToolChoice::Named("local_shell".into()));
        assert_eq!(c(json!({"type":"mcp","server_label":"s"})), ToolChoice::Unknown);
        assert_eq!(c(json!({"type":"mystery"})), ToolChoice::Unknown);
        assert_eq!(c(json!({"type":"web_search"})), ToolChoice::Named("web_search".into()));
        assert_eq!(c(json!("weird")), ToolChoice::Unknown);
    }

    #[test]
    fn anthropic_tool_choice() {
        let c = |tc: serde_json::Value| choice(Format::AnthropicMessages, json!({"tool_choice": tc}));
        assert_eq!(choice(Format::AnthropicMessages, json!({})), ToolChoice::Auto);
        assert_eq!(c(json!({"type":"auto"})), ToolChoice::Auto);
        assert_eq!(c(json!({"type":"any"})), ToolChoice::Required);
        assert_eq!(c(json!({"type":"none"})), ToolChoice::None);
        assert_eq!(c(json!({"type":"tool","name":"t"})), ToolChoice::Named("t".into()));
        assert_eq!(c(json!({"type":"tool"})), ToolChoice::Unknown);
        assert_eq!(c(json!({"type":"x"})), ToolChoice::Unknown);
        assert_eq!(c(json!("auto")), ToolChoice::Unknown);
    }

    #[test]
    fn prior_calls_carry_their_kind() {
        let kinds = |f: &TurnFacts| f.prior_calls.iter().map(|c| c.kind).collect::<Vec<_>>();
        let chat = facts(
            Format::OpenAiChat,
            json!({"model":"m","messages":[{"role":"assistant","content":null,"tool_calls":[
                {"id":"a","type":"function","function":{"name":"read","arguments":"{}"}},
                {"id":"b","type":"custom","custom":{"name":"sql","input":"x"}}],
                "function_call":{"name":"old","arguments":"{}"}}]}),
        );
        assert_eq!(kinds(&chat), [ToolCallKind::Function, ToolCallKind::Custom, ToolCallKind::LegacyFunctionCall]);
        let anthropic = facts(
            Format::AnthropicMessages,
            json!({"model":"c","messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"f","input":{}}]}]}),
        );
        assert_eq!(kinds(&anthropic), [ToolCallKind::AnthropicToolUse]);
        let responses = facts(
            Format::OpenAiResponses,
            json!({"model":"m","input":[
                {"type":"function_call","call_id":"c1","name":"read","arguments":"{}"},
                {"type":"custom_tool_call","call_id":"c2","name":"sql","input":"x"},
                {"type":"local_shell_call","id":"i","call_id":"c3","action":{"type":"exec","command":["ls"]}}]}),
        );
        assert_eq!(kinds(&responses), [ToolCallKind::Function, ToolCallKind::Custom, ToolCallKind::LocalShell]);
    }

    fn notice(name: &str, id: uuid::Uuid) -> String {
        crate::notice_line(name, crate::BlockCode::ALL[0], Some(id))
    }

    #[test]
    fn chat_latest_turn_reads_the_last_assistant_message_and_later_results() {
        let f = facts(
            Format::OpenAiChat,
            json!({"model":"m","messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","tool_calls":[{"id":"old","type":"function","function":{"name":"a","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"old","content":"SECRET"},
                {"role":"assistant","tool_calls":[{"id":"n1","type":"function","function":{"name":"a","arguments":"{}"}},{"id":"n2","type":"function","function":{"name":"a","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"n1","content":"SECRET"},
                {"role":"tool","tool_call_id":"n2","content":"SECRET"}]}),
        );
        assert_eq!(f.latest.call_ids, ["n1", "n2"]);
        assert_eq!(f.latest.result_call_ids, ["n1", "n2"]);
        assert!(!format!("{f:?}").contains("SECRET"));
    }

    #[test]
    fn anthropic_latest_turn_reads_tool_use_and_later_tool_result_blocks() {
        let f = facts(
            Format::AnthropicMessages,
            json!({"model":"c","messages":[
                {"role":"assistant","content":[{"type":"tool_use","id":"old","name":"a","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"old","content":"SECRET"}]},
                {"role":"assistant","content":[{"type":"text","text":"x"},{"type":"tool_use","id":"t1","name":"a","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"SECRET"}]}]}),
        );
        assert_eq!(f.latest.call_ids, ["t1"]);
        assert_eq!(f.latest.result_call_ids, ["t1"]);
        assert!(!format!("{f:?}").contains("SECRET"));
    }

    #[test]
    fn responses_latest_turn_reads_the_last_run_of_model_items() {
        let f = facts(
            Format::OpenAiResponses,
            json!({"model":"m","input":[
                {"type":"message","role":"user","content":"hi"},
                {"type":"function_call","call_id":"old","name":"a","arguments":"{}"},
                {"type":"function_call_output","call_id":"old","output":"SECRET"},
                {"type":"message","role":"user","content":"again"},
                {"type":"reasoning","id":"r1","summary":[]},
                {"type":"function_call","call_id":"c1","name":"a","arguments":"{}"},
                {"type":"mcp_approval_request","id":"mcpr_1","server_label":"s","name":"n","arguments":"{}"},
                {"type":"function_call_output","call_id":"c1","output":"SECRET"},
                {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true}]}),
        );
        assert_eq!(f.latest.call_ids, ["c1", "mcpr_1"]);
        assert_eq!(f.latest.result_call_ids, ["c1", "mcpr_1"]);
        assert!(!format!("{f:?}").contains("SECRET"));
    }

    #[test]
    fn a_provider_run_item_does_not_split_the_latest_turn() {
        let id = "0192f0a0-0000-7000-8000-000000000001";
        let notice = format!("[paigasus] tool call \"send\" was blocked (policy). ref={id}");
        let f = facts(
            Format::OpenAiResponses,
            json!({"model":"m","input":[
                {"type":"message","role":"user","content":"hi"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":notice}]},
                {"type":"web_search_call","id":"ws_1","status":"completed"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]}),
        );
        assert_eq!(f.latest.refs, [Uuid::try_parse(id).unwrap()]);
        assert!(f.latest.call_ids.is_empty());
    }

    #[test]
    fn a_local_shell_output_without_call_id_uses_its_item_id() {
        let f = facts(
            Format::OpenAiResponses,
            json!({"model":"m","input":[
                {"type":"local_shell_call","id":"lsh_1","call_id":"call_1","action":{"type":"exec","command":["ls"]},"status":"completed"},
                {"type":"local_shell_call_output","id":"lsh_1","output":"SECRET"}]}),
        );
        assert_eq!(f.latest.call_ids, ["call_1"]);
        assert_eq!(f.latest.result_call_ids, ["lsh_1"]);
    }

    #[test]
    fn a_legacy_function_result_and_a_legacy_function_call_are_ignored() {
        let f = facts(
            Format::OpenAiChat,
            json!({"model":"m","messages":[
                {"role":"assistant","function_call":{"name":"old","arguments":"{}"}},
                {"role":"function","name":"old","content":"SECRET"}]}),
        );
        assert!(f.latest.call_ids.is_empty());
        assert!(f.latest.result_call_ids.is_empty());
    }

    #[test]
    fn refs_are_read_from_every_text_form() {
        let ids: Vec<uuid::Uuid> = (1..=6).map(uuid::Uuid::from_u128).collect();
        let n = |i: usize| notice("t", ids[i]);
        let r = |f: Format, v: serde_json::Value| facts(f, v).latest.refs;
        assert_eq!(r(Format::OpenAiChat, json!({"messages":[{"role":"assistant","content":format!("a\n{}", n(0))}]})), [ids[0]]);
        assert_eq!(
            r(
                Format::OpenAiChat,
                json!({"messages":[{"role":"assistant","content":[{"type":"text","text":n(1)},{"type":"refusal","text":n(0)}]}]})
            ),
            [ids[1]]
        );
        assert_eq!(r(Format::AnthropicMessages, json!({"messages":[{"role":"assistant","content":n(2)}]})), [ids[2]]);
        assert_eq!(
            r(Format::AnthropicMessages, json!({"messages":[{"role":"assistant","content":[{"type":"text","text":n(3)}]}]})),
            [ids[3]]
        );
        assert_eq!(
            r(
                Format::OpenAiResponses,
                json!({"input":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":n(4)}]}]})
            ),
            [ids[4]]
        );
        assert_eq!(r(Format::OpenAiResponses, json!({"input":[{"type":"message","role":"assistant","content":n(5)}]})), [ids[5]]);
    }

    #[test]
    fn a_ref_is_read_only_from_an_exact_notice_line() {
        let id = uuid::Uuid::from_u128(7);
        let good = notice("t", id);
        let refs = |text: String| facts(Format::OpenAiChat, json!({"messages":[{"role":"assistant","content":text}]})).latest.refs;
        assert_eq!(refs(format!("model text\n\n{good}")), [id]);
        for bad in [
            good.replace("\"t\"", "\"t t\""),
            good.replacen('(', "(nope", 1),
            good.replace(&id.hyphenated().to_string(), "not-a-uuid"),
            format!("{good} trailing"),
        ] {
            assert!(refs(bad.clone()).is_empty(), "{bad}");
        }
        let user = facts(Format::OpenAiChat, json!({"messages":[{"role":"user","content":good}]}));
        assert!(user.latest.refs.is_empty());
    }

    #[test]
    fn each_list_is_capped_at_128_and_counts_the_rest() {
        let calls: Vec<_> = (0..130).map(|i| json!({"id":format!("c{i}"),"type":"function","function":{"name":"a","arguments":"{}"}})).collect();
        let text = (0..131).map(|i| notice("t", uuid::Uuid::from_u128(i))).collect::<Vec<_>>().join("\n");
        let mut messages = vec![json!({"role":"assistant","content":text,"tool_calls":calls})];
        messages.extend((0..129).map(|i| json!({"role":"tool","tool_call_id":format!("c{i}"),"content":"x"})));
        let f = facts(Format::OpenAiChat, json!({"messages":messages}));
        assert_eq!((f.latest.call_ids.len(), f.latest.result_call_ids.len(), f.latest.refs.len()), (128, 128, 128));
        assert_eq!((f.latest.dropped_calls, f.latest.dropped_results, f.latest.dropped_refs), (2, 1, 3));
    }

    fn chat_forged(extra: serde_json::Value) -> TurnFacts {
        let mut messages = vec![
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"a","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"c1","content":"x"}),
        ];
        messages.push(extra);
        facts(Format::OpenAiChat, json!({"messages":messages}))
    }

    #[test]
    fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_chat() {
        let f = chat_forged(json!({"role":"assistant","content":"done"}));
        assert_eq!(f.latest.result_call_ids, ["c1"]);
        assert!(f.latest.call_ids.is_empty());
    }

    #[test]
    fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_anthropic() {
        let f = facts(
            Format::AnthropicMessages,
            json!({"messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"a","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]},
                {"role":"assistant","content":[{"type":"text","text":"done"}]}]}),
        );
        assert_eq!(f.latest.result_call_ids, ["t1"]);
        assert!(f.latest.call_ids.is_empty());
    }

    #[test]
    fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_responses() {
        let f = facts(
            Format::OpenAiResponses,
            json!({"input":[
                {"type":"message","role":"user","content":"hi"},
                {"type":"function_call","call_id":"c1","name":"a","arguments":"{}"},
                {"type":"function_call_output","call_id":"c1","output":"x"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]}),
        );
        assert_eq!(f.latest.result_call_ids, ["c1"]);
        assert!(f.latest.call_ids.is_empty());
    }

    #[test]
    fn the_refs_still_come_from_the_last_assistant_message() {
        let id = uuid::Uuid::from_u128(9);
        let f = chat_forged(json!({"role":"assistant","content":notice("t", id)}));
        assert_eq!(f.latest.refs, [id]);
        assert_eq!(f.latest.result_call_ids, ["c1"]);
    }

    #[test]
    fn the_result_cap_still_applies_with_the_new_anchor() {
        let calls: Vec<_> = (0..130).map(|i| json!({"id":format!("c{i}"),"type":"function","function":{"name":"a","arguments":"{}"}})).collect();
        let mut messages = vec![json!({"role":"assistant","tool_calls":calls})];
        messages.extend((0..130).map(|i| json!({"role":"tool","tool_call_id":format!("c{i}"),"content":"x"})));
        messages.push(json!({"role":"assistant","content":"done"}));
        let f = facts(Format::OpenAiChat, json!({"messages":messages}));
        assert_eq!((f.latest.result_call_ids.len(), f.latest.dropped_results), (128, 2));
    }

    #[test]
    fn a_forged_result_after_a_text_only_assistant_message_is_seen() {
        let chat = json!({"messages":[{"role":"user","content":"x"},{"role":"assistant","content":"hi"},{"role":"tool","tool_call_id":"c9","content":"x"}]});
        let an = json!({"messages":[{"role":"user","content":"x"},{"role":"assistant","content":[{"type":"text","text":"hi"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"c9","content":"x"}]}]});
        let rs = json!({"input":[{"type":"message","role":"user","content":"x"},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]},
            {"type":"function_call_output","call_id":"c9","output":"x"}]});
        for (fmt, v) in [(Format::OpenAiChat, chat), (Format::AnthropicMessages, an), (Format::OpenAiResponses, rs)] {
            assert_eq!(facts(fmt, v).latest.result_call_ids, ["c9"]);
        }
    }

    #[test]
    fn a_forged_result_before_a_text_only_assistant_message_is_seen() {
        let chat = json!({"messages":[{"role":"user","content":"x"},{"role":"tool","tool_call_id":"c9","content":"x"},{"role":"assistant","content":"done"}]});
        let an = json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"c9","content":"x"}]},
            {"role":"assistant","content":[{"type":"text","text":"done"}]}]});
        let rs = json!({"input":[{"type":"message","role":"user","content":"x"},{"type":"function_call_output","call_id":"c9","output":"x"},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]});
        for (fmt, v) in [(Format::OpenAiChat, chat), (Format::AnthropicMessages, an), (Format::OpenAiResponses, rs)] {
            assert_eq!(facts(fmt, v).latest.result_call_ids, ["c9"], "{fmt:?}");
        }
    }

    #[test]
    fn a_forged_result_with_no_assistant_message_is_seen() {
        let chat = json!({"messages":[{"role":"user","content":"x"},{"role":"tool","tool_call_id":"c9","content":"x"}]});
        let an = json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"c9","content":"x"}]}]});
        let rs = json!({"input":[{"type":"message","role":"user","content":"x"},{"type":"function_call_output","call_id":"c9","output":"x"}]});
        for (fmt, v) in [(Format::OpenAiChat, chat), (Format::AnthropicMessages, an), (Format::OpenAiResponses, rs)] {
            let l = facts(fmt, v).latest;
            assert_eq!(l.result_call_ids, ["c9"]);
            assert!(l.call_ids.is_empty() && l.refs.is_empty());
        }
    }

    #[test]
    fn the_refs_and_the_cap_hold_with_the_new_anchor_anthropic_and_responses() {
        let id = uuid::Uuid::from_u128(9);
        let uses: Vec<_> = (0..130).map(|i| json!({"type":"tool_use","id":format!("c{i}"),"name":"a","input":{}})).collect();
        let results: Vec<_> = (0..130).map(|i| json!({"type":"tool_result","tool_use_id":format!("c{i}"),"content":"x"})).collect();
        let an = facts(
            Format::AnthropicMessages,
            json!({"messages":[{"role":"assistant","content":uses},{"role":"user","content":results},
                {"role":"assistant","content":[{"type":"text","text":notice("t", id)}]}]}),
        )
        .latest;
        assert_eq!((an.result_call_ids.len(), an.dropped_results, an.refs.clone()), (128, 2, vec![id]));
        let mut items = vec![json!({"type":"function_call","call_id":"c0","name":"a","arguments":"{}"})];
        items.extend((0..130).map(|i| json!({"type":"function_call_output","call_id":format!("c{i}"),"output":"x"})));
        items.push(json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":notice("t", id)}]}));
        let rs = facts(Format::OpenAiResponses, json!({"input":items})).latest;
        assert_eq!((rs.result_call_ids.len(), rs.dropped_results, rs.refs.clone()), (128, 2, vec![id]));
    }

    #[test]
    fn a_request_without_an_assistant_message_has_an_empty_latest_turn() {
        for (fmt, v) in [
            (Format::OpenAiChat, json!({"messages":[{"role":"user","content":"hi"}]})),
            (Format::AnthropicMessages, json!({"messages":[{"role":"user","content":"hi"}]})),
            (Format::OpenAiResponses, json!({"input":"hi"})),
        ] {
            assert_eq!(facts(fmt, v).latest, LatestTurn::default());
        }
    }

    #[test]
    fn a_local_shell_call_gives_its_item_id() {
        let item = json!({"type":"local_shell_call","id":"lsh_1","call_id":"call_1","action":{"type":"exec","command":["ls"]},"status":"completed"});
        let call = tool_call(0, item.as_object().unwrap(), ToolCallKind::LocalShell).unwrap();
        assert_eq!(call.id.as_deref(), Some("call_1"));
        assert_eq!(call.item_id().as_deref(), Some("lsh_1"));
        assert_eq!(crate::decision_log::CallFacts::of(&call).item_id.as_deref(), Some("lsh_1"));
        let f = json!({"type":"function_call","call_id":"c","name":"f","arguments":"{}"});
        assert_eq!(tool_call(0, f.as_object().unwrap(), ToolCallKind::Function).unwrap().item_id(), None);
    }
}
