// SPDX-License-Identifier: Apache-2.0
//! The request side of a turn (spec D19): what tools the caller declared, what it called before,
//! and which results it sent back. Never the content of a tool result. No request rewrite.

use serde_json::{Map, Value};

use crate::json::{Malformed, ResponsesDoc, opt_obj, opt_str, parse_strict, parse_strict_responses};
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
    pub schema: Option<Value>,
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
}

/// A reference to a tool result. It never holds the result content.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultRef {
    pub call_id: Option<String>,
    pub name: Option<String>,
}

#[must_use]
pub fn parse_request(format: Format, body: &[u8]) -> ToolTurn {
    let parsed = match format {
        Format::OpenAiResponses => parse_strict_responses(body, ResponsesDoc::Request).and_then(|v| responses(&v)),
        Format::OpenAiChat => parse_strict(body).and_then(|v| openai(&v)),
        Format::AnthropicMessages => parse_strict(body).and_then(|v| anthropic(&v)),
    };
    match parsed {
        Ok(f) => ToolTurn::Parsed(f),
        Err(_) => ToolTurn::Unavailable("malformed"),
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

fn openai(v: &Value) -> Result<TurnFacts, Malformed> {
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
                    schema: func.get("parameters").cloned(),
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
            schema: d.get("parameters").cloned(),
        });
    }
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        match opt_str(m.get("role"), "role")?.as_deref() {
            Some("assistant") => {
                for c in arr(m, "tool_calls")? {
                    let c = obj(c)?;
                    let (name, arguments) = prior_body(c)?;
                    f.prior_calls.push(PriorCall {
                        id: opt_str(c.get("id"), "id")?,
                        name,
                        arguments,
                    });
                }
                if let Some(fc) = opt_obj(m.get("function_call"), "function_call")? {
                    f.prior_calls.push(PriorCall {
                        id: None,
                        name: req_str(fc, "name")?,
                        arguments: opt_str(fc.get("arguments"), "arguments")?.unwrap_or_default(),
                    });
                }
            }
            Some("tool") => f.tool_results.push(ToolResultRef {
                call_id: opt_str(m.get("tool_call_id"), "tool_call_id")?,
                name: None,
            }),
            Some("function") => f.tool_results.push(ToolResultRef {
                call_id: None,
                name: opt_str(m.get("name"), "name")?,
            }),
            _ => {}
        }
    }
    Ok(f)
}

/// The (name, arguments) of a prior OpenAI tool call: a `function` body's name and arguments, or a
/// `custom` body's name and input. An entry with neither body, both, or a body that disagrees with
/// its `type` is malformed: an empty name would read as a call to no tool at all.
fn prior_body(c: &Map<String, Value>) -> Result<(String, String), Malformed> {
    let ty = opt_str(c.get("type"), "type")?;
    let (body, input) = match (opt_obj(c.get("function"), "function")?, opt_obj(c.get("custom"), "custom")?, ty.as_deref()) {
        (Some(x), None, None | Some("function")) => (x, "arguments"),
        (None, Some(x), None | Some("custom")) => (x, "input"),
        _ => return Err(Malformed("tool_call")),
    };
    Ok((req_str(body, "name")?, opt_str(body.get(input), input)?.unwrap_or_default()))
}

/// The request side of a Responses turn (spec § 7.2), read by `parse_request`.
///
/// `tools[]` gives the definitions: a `function` or `custom` tool by name, the tools of a
/// `namespace` tool with that namespace, and any other tool by its `type` (kind `Other`). A
/// `tool_search` tool makes the set incomplete, as do an `additional_tools` and a
/// `tool_search_output` input item. The `input` items of the client-run types of § 6.2 are the
/// prior calls (read as a response reads them, `assembly::tool_call`), and each output item is a
/// tool result. An `mcp_approval_response` refers to its request by `approval_request_id`, and the
/// id of an `mcp_approval_request` call is its item `id`. A string `input` has no items.
fn responses(v: &Value) -> Result<TurnFacts, Malformed> {
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
    // A string `input` is one user message. Only an array has items.
    let items: &[Value] = match top.get("input") {
        Some(Value::Array(a)) => a,
        None | Some(Value::Null | Value::String(_)) => &[],
        Some(_) => return Err(Malformed("input")),
    };
    for (i, item) in items.iter().enumerate() {
        let item = obj(item)?;
        // A message in the short form has no `type`.
        let Some(ty) = opt_str(item.get("type"), "type")? else { continue };
        match ty.as_str() {
            // The referenced items are not in the request, so the history is unknown (D3, § 7.2).
            "item_reference" => return Err(Malformed("item_reference")),
            "additional_tools" | "tool_search_output" => f.definitions_complete = false,
            "function_call_output" | "custom_tool_call_output" | "computer_call_output" | "local_shell_call_output" | "shell_call_output" | "apply_patch_call_output" => {
                f.tool_results.push(ToolResultRef {
                    call_id: opt_str(item.get("call_id"), "call_id")?,
                    name: None,
                });
            }
            "mcp_approval_response" => f.tool_results.push(ToolResultRef {
                call_id: opt_str(item.get("approval_request_id"), "approval_request_id")?,
                name: None,
            }),
            _ => {
                // An unknown type is client-run of kind `Other` for a response (fail closed), but in
                // a request it is just an item the gateway does not read (`message`, `reasoning`).
                if let ItemClass::ClientRun(kind) = item_class(&ty)
                    && kind != ToolCallKind::Other
                {
                    let c = tool_call(u32::try_from(i).map_err(|_| Malformed("input"))?, item, kind)?;
                    f.prior_calls.push(PriorCall {
                        // The display name, so a result is decided as its call is (P-7).
                        name: c.display_name(), // PRIOR-DISPLAY-NAME
                        id: c.id,
                        arguments: c.arguments,
                    });
                }
            }
        }
    }
    Ok(f)
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
        schema: if function { t.get("parameters").cloned() } else { None },
    })
}

fn anthropic(v: &Value) -> Result<TurnFacts, Malformed> {
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
                schema: t.get("input_schema").cloned(),
            }),
            Some(_) => f.server_tools = true, // a server tool
        }
    }
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        let Some(Value::Array(blocks)) = m.get("content") else { continue };
        for b in blocks {
            let b = obj(b)?;
            match opt_str(b.get("type"), "type")?.as_deref() {
                Some("tool_use") => f.prior_calls.push(PriorCall {
                    id: opt_str(b.get("id"), "id")?,
                    name: req_str(b, "name")?,
                    arguments: b.get("input").map(Value::to_string).unwrap_or_default(),
                }),
                Some("tool_result") => f.tool_results.push(ToolResultRef {
                    call_id: opt_str(b.get("tool_use_id"), "tool_use_id")?,
                    name: None,
                }),
                _ => {}
            }
        }
    }
    Ok(f)
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
                    name: None
                },
                ToolResultRef {
                    call_id: None,
                    name: Some("old".into())
                }
            ]
        );
        assert!(!format!("{f:?}").contains("SECRET"), "tool result content is never kept");
    }

    #[test]
    fn an_anthropic_request_yields_definitions_calls_and_results() {
        let f = facts(
            Format::AnthropicMessages,
            json!({"model":"c","tools":[{"name":"rm","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search"}],
                   "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{"p":"/"}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"SECRET"}]}]}),
        );
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
        let f = responses(&json!({"model":"m",
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
                     {"type":"reasoning","id":"rs_1","summary":[]}]}))
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
    }

    #[test]
    fn a_namespaced_prior_call_keeps_its_display_name() {
        let f = responses(&json!({"input":[{"type":"function_call","call_id":"c1","namespace":"db","name":"query","arguments":"{}"}]})).unwrap();
        assert_eq!(f.prior_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["db.query"]);
    }

    #[test]
    fn a_string_input_has_no_calls() {
        let f = responses(&json!({"model":"m","input":"hi"})).unwrap();
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
            assert!(!responses(&v).unwrap().definitions_complete, "{v}");
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
            assert!(responses(&v).is_err(), "{v}");
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
}
