// SPDX-License-Identifier: Apache-2.0
//! Strict JSON (spec D5).
//!
//! A record that parses everywhere but means different things in different clients is worse than
//! one that fails to parse. serde and JavaScript keep the last of two duplicate keys, Go's
//! `encoding/json` merges struct-typed duplicates, and Go and Newtonsoft match field names
//! case-insensitively. JavaScript gives `__proto__` meaning. Rejecting all of these costs nothing
//! on a conforming upstream.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// A record the gateway cannot classify with certainty. The label is for tests and logs only;
/// it never reaches a metric label with a value from the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed record: {0}")]
pub(crate) struct Malformed(pub &'static str);

pub(crate) const MAX_OPENAI_INDEX: u64 = 127;
pub(crate) const MAX_ANTHROPIC_INDEX: u64 = 4095;

/// Keys the classifiers read, in every format. A key that folds to one of these without being
/// byte-equal to it is a case-folding differential.
const CLASSIFIER_KEYS: &[&str] = &[
    "arguments",
    "choices",
    "content",
    "content_block",
    "custom",
    "delta",
    "error",
    "finish_reason",
    "function",
    "function_call",
    "functions",
    "id",
    "index",
    "input",
    "input_schema",
    "logprobs",
    "message",
    "messages",
    "name",
    "parameters",
    "partial_json",
    "role",
    "stop_reason",
    "tool_call_id",
    "tool_calls",
    "tool_use_id",
    "tools",
    "type",
    "usage",
];

const PROTOTYPE_KEYS: &[&str] = &["__proto__", "constructor", "prototype"];

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\u{0130}' | '\u{0131}' => 'i'.to_lowercase(),
            '\u{017F}' => 's'.to_lowercase(),
            '\u{212A}' => 'k'.to_lowercase(),
            c => c.to_lowercase(),
        })
        .collect()
}

fn check_key(k: &str) -> Result<(), &'static str> {
    if PROTOTYPE_KEYS.contains(&k) {
        return Err("prototype_key");
    }
    if k.is_ascii() && k.bytes().all(|b| !b.is_ascii_uppercase()) {
        return Ok(()); // already folded; equal to a classifier key only if byte-equal
    }
    let folded = fold(k);
    if CLASSIFIER_KEYS.iter().any(|c| *c != k && folded == *c) {
        return Err("case_folded_key");
    }
    Ok(())
}

/// Parse one record strictly.
pub(crate) fn parse_strict(bytes: &[u8]) -> Result<Value, Malformed> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let value = Strict.deserialize(&mut de).map_err(|_| Malformed("json"))?;
    de.end().map_err(|_| Malformed("trailing"))?;
    Ok(value)
}

struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(Strict)? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        let mut seen = HashSet::new();
        while let Some(k) = map.next_key::<String>()? {
            check_key(&k).map_err(de::Error::custom)?;
            if !seen.insert(k.clone()) {
                return Err(de::Error::custom("duplicate key"));
            }
            let v = map.next_value_seed(Strict)?;
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

/// A JSON integer literal in `0..=max` (spec § 4.2). serde_json reads `0.0` and `1e0` as floats,
/// so `is_u64` already excludes them.
pub(crate) fn index_of(v: Option<&Value>, max: u64) -> Result<u32, Malformed> {
    match v {
        Some(Value::Number(n)) if n.is_u64() => n.as_u64().filter(|i| *i <= max).and_then(|i| u32::try_from(i).ok()).ok_or(Malformed("index")),
        _ => Err(Malformed("index")),
    }
}

pub(crate) fn opt_str(v: Option<&Value>, what: &'static str) -> Result<Option<String>, Malformed> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Malformed(what)),
    }
}

pub(crate) fn opt_obj<'a>(v: Option<&'a Value>, what: &'static str) -> Result<Option<&'a Map<String, Value>>, Malformed> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err(Malformed(what)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plain_record_parses() {
        assert_eq!(parse_strict(br#"{"a":[1,"x",null,true,{"b":2.5}]}"#).unwrap(), json!({"a":[1,"x",null,true,{"b":2.5}]}));
    }

    #[test]
    fn duplicate_keys_are_malformed_at_any_depth() {
        assert!(parse_strict(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_strict(br#"{"choices":[{"delta":{"tool_calls":[]},"delta":{"content":"hi"}}]}"#).is_err());
        assert!(parse_strict(br#"[{"x":{"y":1,"y":1}}]"#).is_err());
    }

    #[test]
    fn prototype_keys_are_malformed() {
        for k in ["__proto__", "constructor", "prototype"] {
            assert!(parse_strict(format!(r#"{{"choices":[{{"{k}":{{}}}}]}}"#).as_bytes()).is_err(), "{k}");
        }
    }

    #[test]
    fn case_folded_classifier_keys_are_malformed() {
        for k in [
            "Tool_Calls",
            "TOOL_CALLS",
            "tool_callſ",
            "Delta",
            "Index",
            "ındex",
            "İndex",
            "tool_call_ıd",
            "choıces",
            "\u{212A}ey_not_a_classifier_key",
        ] {
            let r = parse_strict(format!(r#"{{"{k}":1}}"#).as_bytes());
            if k.starts_with('\u{212A}') {
                assert!(r.is_ok(), "{k} folds to no classifier key");
            } else {
                assert!(r.is_err(), "{k}");
            }
        }
        // An unrelated key with capitals is fine.
        assert!(parse_strict(br#"{"Model":"x","X-Extra":1}"#).is_ok());
    }

    #[test]
    fn escaped_duplicates_and_escaped_prototype_keys_are_malformed() {
        assert!(parse_strict(br#"{"tool_calls":[],"tool_calls":[{}]}"#).is_err());
        assert!(parse_strict(br#"{"tool\u005fcalls":[],"tool_calls":[{}]}"#).is_err());
        assert!(parse_strict(br#"{"__proto__":{}}"#).is_err());
        assert!(parse_strict(br#"{"\u005f_proto__":{}}"#).is_err());
    }

    #[test]
    fn non_json_and_trailing_bytes_are_malformed() {
        for bad in [&b"NaN"[..], b"{\"a\":NaN}", b"{\"a\":Infinity}", b"{} {}", b"{\"a\":1", b"", b"\"\\ud800\""] {
            assert!(parse_strict(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_unicode_escaped_key_is_read_as_the_key_it_spells() {
        // There is no substring prefilter (spec D4): the escaped spelling IS the classifier key.
        let v = parse_strict(br#"{"tool\u005fcalls":[1]}"#).unwrap();
        assert_eq!(v, json!({"tool_calls":[1]}));
    }

    #[test]
    fn index_must_be_a_plain_integer_in_range() {
        assert_eq!(index_of(Some(&json!(0)), MAX_OPENAI_INDEX).unwrap(), 0);
        assert_eq!(index_of(Some(&json!(127)), MAX_OPENAI_INDEX).unwrap(), 127);
        let floaty = parse_strict(b"[0.0, 1e0, -1, true, \"0\", 128]").unwrap();
        for v in floaty.as_array().unwrap() {
            assert!(index_of(Some(v), MAX_OPENAI_INDEX).is_err(), "{v}");
        }
        assert!(index_of(None, MAX_OPENAI_INDEX).is_err());
    }

    #[test]
    fn opt_helpers_treat_null_as_absent() {
        assert_eq!(opt_str(None, "x").unwrap(), None);
        assert_eq!(opt_str(Some(&json!(null)), "x").unwrap(), None);
        assert_eq!(opt_str(Some(&json!("a")), "x").unwrap(), Some("a".to_owned()));
        assert!(opt_str(Some(&json!(1)), "x").is_err());
        assert!(opt_obj(Some(&json!({})), "x").unwrap().is_some());
        assert!(opt_obj(Some(&json!([])), "x").is_err());
    }
}
