// SPDX-License-Identifier: Apache-2.0
//! OpenAI stream-chunk classification by a positive allowlist (spec § 4.2, D6).
//!
//! A chunk is tool-free only when it matches the strict tool-free shape. A non-empty
//! `tool_calls` or a `function_call` object makes it tool-bearing. Anything else is malformed.
//! "Has a `tool_calls` key" would be wrong both ways: it misses `tool_calls` as an object, and it
//! holds every text delta of an upstream that sends `"tool_calls": []`.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

use serde_json::{Map, Value};

use crate::json::{MAX_OPENAI_INDEX, Malformed, index_of, opt_obj, opt_str};

pub(crate) enum Chunk {
    Choices(Vec<ChoiceView>),
    UpstreamError,
}

pub(crate) struct ChoiceView {
    pub index: u32,
    pub fragments: Vec<Fragment>,
    pub legacy: Option<NameArgs>,
    pub finish_reason: Option<String>,
    pub has_text: bool,
}

pub(crate) struct Fragment {
    pub index: u32,
    pub id: Option<String>,
    pub ty: Option<String>,
    pub function: Option<NameArgs>,
    pub custom: Option<NameArgs>,
    /// The whole entry, for kind Other (§ 4.1).
    pub raw: Value,
}

pub(crate) struct NameArgs {
    pub name: Option<String>,
    pub args: Option<String>,
}

pub(crate) fn chunk(v: &Value) -> Result<Chunk, Malformed> {
    let Value::Object(top) = v else { return Err(Malformed("not_object")) };
    if top.contains_key("error") {
        return if top.contains_key("choices") {
            Err(Malformed("error_and_choices"))
        } else {
            Ok(Chunk::UpstreamError)
        };
    }
    let choices = match top.get("choices") {
        None => Vec::new(),
        Some(Value::Array(a)) => {
            let v: Vec<ChoiceView> = a.iter().map(choice).collect::<Result<_, _>>()?;
            // LangChain takes choices[0] of every chunk, so a repeated index could be merged by it.
            if v.iter().enumerate().any(|(i, x)| v[..i].iter().any(|y| y.index == x.index)) {
                return Err(Malformed("duplicate_choice_index"));
            }
            v
        }
        Some(_) => return Err(Malformed("choices")),
    };
    Ok(Chunk::Choices(choices))
}

fn choice(v: &Value) -> Result<ChoiceView, Malformed> {
    let Value::Object(c) = v else { return Err(Malformed("choice")) };
    if c.contains_key("message") {
        return Err(Malformed("message_in_chunk"));
    }
    let index = index_of(c.get("index"), MAX_OPENAI_INDEX)?;
    let finish_reason = opt_str(c.get("finish_reason"), "finish_reason")?;
    let (fragments, legacy, has_text) = match c.get("delta") {
        None => (Vec::new(), None, false),
        Some(Value::Object(d)) => delta(d)?,
        Some(_) => return Err(Malformed("delta")),
    };
    Ok(ChoiceView {
        index,
        fragments,
        legacy,
        finish_reason,
        has_text,
    })
}

fn delta(d: &Map<String, Value>) -> Result<(Vec<Fragment>, Option<NameArgs>, bool), Malformed> {
    let fragments = match d.get("tool_calls") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(fragment).collect::<Result<_, _>>()?,
        Some(_) => return Err(Malformed("tool_calls")),
    };
    let legacy = opt_obj(d.get("function_call"), "function_call")?.map(|f| name_args(f, "arguments")).transpose()?;
    let has_text = match d.get("content") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => return Err(Malformed("content")),
    };
    Ok((fragments, legacy, has_text))
}

fn fragment(v: &Value) -> Result<Fragment, Malformed> {
    let Value::Object(e) = v else { return Err(Malformed("tool_call")) };
    let index = index_of(e.get("index"), MAX_OPENAI_INDEX)?;
    let id = opt_str(e.get("id"), "id")?;
    let ty = opt_str(e.get("type"), "type")?;
    let function = opt_obj(e.get("function"), "function")?.map(|f| name_args(f, "arguments")).transpose()?;
    let custom = opt_obj(e.get("custom"), "custom")?.map(|f| name_args(f, "input")).transpose()?;
    if function.is_some() && custom.is_some() {
        return Err(Malformed("function_and_custom"));
    }
    Ok(Fragment {
        index,
        id,
        ty,
        function,
        custom,
        raw: v.clone(),
    })
}

pub(crate) fn name_args(f: &Map<String, Value>, args_key: &'static str) -> Result<NameArgs, Malformed> {
    Ok(NameArgs {
        name: opt_str(f.get("name"), "name")?,
        args: opt_str(f.get(args_key), args_key)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_strict;

    fn c(s: &str) -> Result<Chunk, Malformed> {
        chunk(&parse_strict(s.as_bytes()).unwrap())
    }
    fn choices(s: &str) -> Vec<ChoiceView> {
        match c(s).unwrap() {
            Chunk::Choices(v) => v,
            Chunk::UpstreamError => panic!("not an error"),
        }
    }

    #[test]
    fn a_text_delta_is_tool_free() {
        let v = choices(r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}"#);
        assert_eq!(v.len(), 1);
        assert!(v[0].fragments.is_empty() && v[0].legacy.is_none() && v[0].has_text);
    }

    #[test]
    fn empty_or_null_tool_calls_are_tool_free() {
        for d in [r#"{"tool_calls":[]}"#, r#"{"tool_calls":null}"#, r#"{"function_call":null}"#, r#"{}"#] {
            let v = choices(&format!(r#"{{"choices":[{{"index":0,"delta":{d}}}]}}"#));
            assert!(v[0].fragments.is_empty() && v[0].legacy.is_none(), "{d}");
        }
    }

    #[test]
    fn a_usage_chunk_has_no_choices() {
        assert!(choices(r#"{"choices":[],"usage":{"total_tokens":3}}"#).is_empty());
        assert!(choices(r#"{"usage":{"total_tokens":3}}"#).is_empty());
    }

    #[test]
    fn a_tool_fragment_is_read() {
        let v = choices(r#"{"choices":[{"index":1,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"rm","arguments":""}}]}}]}"#);
        let f = &v[0].fragments[0];
        assert_eq!((v[0].index, f.index, f.id.as_deref(), f.ty.as_deref()), (1, 0, Some("call_1"), Some("function")));
        assert_eq!(f.function.as_ref().unwrap().name.as_deref(), Some("rm"));
    }

    #[test]
    fn a_legacy_function_call_is_read() {
        let v = choices(r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]}"#);
        assert_eq!(v[0].legacy.as_ref().unwrap().args.as_deref(), Some("{"));
    }

    #[test]
    fn an_error_record_is_an_upstream_error() {
        assert!(matches!(c(r#"{"error":{"message":"x"}}"#).unwrap(), Chunk::UpstreamError));
        assert!(c(r#"{"error":{},"choices":[]}"#).is_err());
    }

    #[test]
    fn every_non_canonical_shape_is_malformed() {
        for bad in [
            r#"[]"#,
            r#"{"choices":null}"#,
            r#"{"choices":{"0":{}}}"#,
            r#"{"choices":[1]}"#,
            r#"{"choices":[{"delta":{}}]}"#,
            r#"{"choices":[{"index":0.0,"delta":{}}]}"#,
            r#"{"choices":[{"index":0,"message":{"tool_calls":[]}}]}"#,
            r#"{"choices":[{"index":0,"delta":"x"}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":{"0":{}}}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":"x"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[1]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":true}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":{}}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{},"custom":{}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"function_call":"rm"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":["x"]}}]}"#,
            r#"{"choices":[{"index":0,"finish_reason":1}]}"#,
        ] {
            assert!(c(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_repeated_choice_index_in_one_chunk_is_malformed() {
        assert!(c(r#"{"choices":[{"index":0,"delta":{}},{"index":0,"delta":{}}]}"#).is_err());
        assert_eq!(choices(r#"{"choices":[{"index":0,"delta":{}},{"index":1,"delta":{"content":"t"}}]}"#).len(), 2);
    }
}
