// SPDX-License-Identifier: Apache-2.0
//! The request side of a turn (spec D19): what tools the caller declared, what it called before,
//! and which results it sent back. Never the content of a tool result. No request rewrite.

use serde_json::{Map, Value};

use crate::json::{Malformed, opt_obj, opt_str, parse_strict};
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnFacts {
    pub definitions: Vec<ToolDefinition>,
    pub prior_calls: Vec<PriorCall>,
    pub tool_results: Vec<ToolResultRef>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub kind: ToolCallKind,
    pub schema: Option<Value>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorCall {
    pub id: Option<String>,
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
    let parsed = parse_strict(body).and_then(|v| match format {
        Format::OpenAiChat => openai(&v),
        Format::AnthropicMessages => anthropic(&v),
    });
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
    let mut f = TurnFacts::default();
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            Some("function") => {
                let func = opt_obj(t.get("function"), "function")?.ok_or(Malformed("function"))?;
                f.definitions.push(ToolDefinition {
                    name: req_str(func, "name")?,
                    kind: ToolCallKind::Function,
                    schema: func.get("parameters").cloned(),
                });
            }
            Some("custom") => {
                let c = opt_obj(t.get("custom"), "custom")?.ok_or(Malformed("custom"))?;
                f.definitions.push(ToolDefinition {
                    name: req_str(c, "name")?,
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
                    let func = opt_obj(c.get("function"), "function")?;
                    let name = func.map(|x| req_str(x, "name")).transpose()?.unwrap_or_default();
                    let arguments = func.map(|x| opt_str(x.get("arguments"), "arguments")).transpose()?.flatten().unwrap_or_default();
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

fn anthropic(v: &Value) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts::default();
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            None | Some("custom") => f.definitions.push(ToolDefinition {
                name: req_str(t, "name")?,
                kind: ToolCallKind::AnthropicToolUse,
                schema: t.get("input_schema").cloned(),
            }),
            Some(_) => {} // a server tool
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
        assert_eq!(f.prior_calls[0].arguments, r#"{"p":"/"}"#);
        assert_eq!(f.tool_results[0].call_id.as_deref(), Some("t1"));
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
    fn a_malformed_request_is_unavailable_never_empty() {
        for bad in [&b"{"[..], br#"{"a":1,"a":2}"#, br#"{"messages":"x"}"#, br#"{"tools":[{"type":"function","function":{"name":1}}]}"#] {
            assert!(matches!(parse_request(Format::OpenAiChat, bad), ToolTurn::Unavailable(_)), "{}", String::from_utf8_lossy(bad));
        }
    }
}
