// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "detectors")]
//! Properties of the detectors: the host extraction never panics and is deterministic, and
//! `invalid_args` gives no finding for a value that its schema accepts (A7).

use paigasus_kernel::detect::{CallSubject, Declared, ToolInput, call_hosts, invalid_args};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const POOL: [&str; 14] = [
    "https:",
    "://",
    ":::::::::::::::::::::::::::",
    "///////////////////////",
    "[",
    "]",
    "\\u00",
    "\\ud800",
    "é",
    "日本",
    "http://a.example/",
    "https://B.example:80/x?y=1",
    "a b",
    "ftp:\\\\h.example",
];

fn json_text() -> impl Strategy<Value = String> {
    let leaf = prop::collection::vec(prop::sample::select(POOL.to_vec()), 0..6).prop_map(|v| v.concat());
    let value = leaf.clone().prop_map(|s| format!("\"{s}\""));
    let pair = (leaf, value.clone()).prop_map(|(k, v)| format!("\"{k}\":{v}"));
    prop_oneof![
        value.clone(),
        prop::collection::vec(pair, 0..5).prop_map(|p| format!("{{{}}}", p.join(","))),
        prop::collection::vec(value, 0..5).prop_map(|p| format!("[{}]", p.join(","))),
    ]
}

proptest! {
    #[test]
    fn call_hosts_never_panics(s in ".{0,400}", j in json_text()) {
        let _ = call_hosts(&ToolInput::Text(s.clone()));
        let _ = call_hosts(&ToolInput::Json(s));
        let _ = call_hosts(&ToolInput::Json(j));
    }

    #[test]
    fn call_hosts_is_deterministic(j in json_text()) {
        let (a, b) = (call_hosts(&ToolInput::Json(j.clone())), call_hosts(&ToolInput::Json(j)));
        prop_assert_eq!(a.iter().collect::<Vec<_>>(), b.iter().collect::<Vec<_>>());
    }
}

// --- A7: no false `invalid_args` (§ 7.2) ---

/// A small deterministic source of choices for the schema derivation (splitmix64).
struct Choices(u64);

impl Choices {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// True one time in `n`.
    fn one_in(&mut self, n: u64) -> bool {
        self.next().is_multiple_of(n)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Long number texts, as (text, integral). `serde_json` with no `float_roundtrip` and
/// `str::parse` can read such a text to two different doubles. A `Value` cannot hold the text, so
/// the generator puts the placeholder `"#D<k>#"` in the value and in the schema, and the text
/// replaces it after both are serialised.
fn long_numbers() -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = [
        ("0.20082962974712488058302", false),
        ("0.334274685957130011297", false),
        ("1e300", true),
        ("123456789012345678901234567890", true),
        ("9007199254740993", true),
        ("18446744073709551616", true),
        ("1.00000000000000000000000000001", false),
        ("5.000000000000000000000000", true),
        ("-0.000000000000000000000000000000017", false),
        ("0.1000000000000000055511151231257827", false),
    ]
    .iter()
    .map(|(t, i)| ((*t).to_owned(), *i))
    .collect();
    out.push((format!("{:.0}", 1e300_f64), true));
    out
}

/// The placeholder of the `k`-th long number.
fn placeholder(k: usize) -> String {
    format!("#D{k}#")
}

/// Whether `v` is a long number placeholder, and if so whether its number is integral.
fn long_number(v: &Value) -> Option<bool> {
    let k: usize = v.as_str()?.strip_prefix("#D")?.strip_suffix('#')?.parse().ok()?;
    long_numbers().get(k).map(|(_, integral)| *integral)
}

/// The JSON text of `v` with each placeholder replaced by its long number.
fn with_long_numbers(v: &Value) -> String {
    let mut text = v.to_string();
    for (k, (number, _)) in long_numbers().iter().enumerate() {
        text = text.replace(&format!("\"{}\"", placeholder(k)), number);
    }
    text
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        (0..long_numbers().len()).prop_map(|k| Value::from(placeholder(k))),
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1000i64..1000).prop_map(Value::from),
        // An integer as a float, so the text is `5.0`.
        (-1000i64..1000).prop_map(|n| Value::from(n as f64)),
        (-1.0e6f64..1.0e6).prop_map(Value::from),
        "[a-c~/]{0,4}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map("[a-d~/]{1,2}", inner, 0..6).prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// A type name that `v` does not have.
fn wrong_type(v: &Value) -> &'static str {
    if v.is_null() || long_number(v).is_some() { "string" } else { "null" }
}

/// A type name that `v` has. An integer-valued number can be `integer` or `number`.
fn type_of(v: &Value, c: &mut Choices) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => match long_number(v) {
            Some(true) if c.one_in(2) => "integer",
            Some(_) => "number",
            None => "string",
        },
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(n) => {
            let integral = n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0);
            if integral && c.one_in(2) { "integer" } else { "number" }
        }
    }
}

/// A value equal to `v` by JSON Schema: an integer in the other number spelling, if it has one.
fn respelled(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.is_i64() => Value::from(n.as_i64().unwrap_or(0) as f64),
        Value::Number(n) if n.as_f64().is_some_and(|f| f.fract() == 0.0 && f.abs() < 1e15) => Value::from(n.as_f64().unwrap_or(0.0) as i64),
        other => other.clone(),
    }
}

/// An `enum` that holds each of `values` (or an equal value) among noise.
fn enum_with(values: &[Value], c: &mut Choices, large: bool) -> Value {
    let count = if large { 500 + c.below(1500) } else { c.below(4) };
    let mut out: Vec<Value> = (0..count).map(|i| if c.one_in(2) { json!({ "noise": i }) } else { json!(format!("noise-{i}")) }).collect();
    for v in values {
        let at = c.below(out.len() + 1);
        out.insert(at, if c.one_in(2) { respelled(v) } else { v.clone() });
    }
    Value::Array(out)
}

/// A schema that `v` satisfies under some JSON Schema draft, with noise that the full JSON Schema
/// accepts and that the subset validator must skip.
fn derive(v: &Value, c: &mut Choices) -> Value {
    let mut s = Map::new();
    if c.one_in(2) {
        let t = type_of(v, c);
        s.insert("type".into(), if c.one_in(3) { json!([wrong_type(v), t]) } else { json!(t) });
    }
    if c.one_in(5) {
        s.insert("enum".into(), enum_with(std::slice::from_ref(v), c, false));
    }
    match v {
        Value::Object(members) => {
            if c.one_in(2) {
                let required: Vec<&String> = members.keys().filter(|_| c.one_in(2)).collect();
                s.insert("required".into(), json!(required));
            }
            let mut properties = Map::new();
            for (k, x) in members {
                if c.one_in(4) {
                    continue;
                }
                properties.insert(k.clone(), if c.one_in(5) { Value::Bool(true) } else { derive(x, c) });
            }
            let all_listed = properties.len() == members.len();
            match c.below(4) {
                0 if all_listed => {
                    s.insert("additionalProperties".into(), Value::Bool(false));
                }
                // `additionalProperties: false` that `v` can violate, next to `patternProperties`
                // that takes every key.
                1 => {
                    s.insert("additionalProperties".into(), Value::Bool(false));
                    s.insert("patternProperties".into(), json!({ "": true }));
                }
                2 => {
                    s.insert("additionalProperties".into(), json!({ "x-note": "any" }));
                }
                _ => {}
            }
            if !properties.is_empty() || c.one_in(2) {
                s.insert("properties".into(), Value::Object(properties));
            }
        }
        Value::Array(elements) => match c.below(6) {
            // `prefixItems` that covers every element, next to an `items` that every element violates.
            0 | 1 => {
                let prefix: Vec<Value> = elements.iter().map(|x| derive(x, c)).collect();
                s.insert("prefixItems".into(), Value::Array(prefix));
                s.insert("items".into(), if c.one_in(2) { Value::Bool(false) } else { json!({ "type": "null", "enum": [] }) });
            }
            // The tuple form of `items` (draft 2019 and before).
            2 => {
                let tuple: Vec<Value> = elements.iter().map(|x| derive(x, c)).collect();
                s.insert("items".into(), Value::Array(tuple));
            }
            // A large `enum` under `items`.
            3 => {
                s.insert("items".into(), json!({ "enum": enum_with(elements, c, true) }));
            }
            4 => {
                let mut types: Vec<&str> = elements.iter().map(|x| type_of(x, c)).collect();
                types.push("null");
                s.insert("items".into(), json!({ "type": types }));
            }
            _ => {
                s.insert("items".into(), if c.one_in(2) { Value::Bool(true) } else { json!({}) });
            }
        },
        // A nullable object type: the object keywords do not apply to `null`.
        Value::Null if c.one_in(3) => {
            s.insert("type".into(), json!(["object", "null"]));
            s.insert("required".into(), json!(["a", "b"]));
            s.insert("properties".into(), json!({ "a": { "type": "string" } }));
            s.insert("additionalProperties".into(), Value::Bool(false));
        }
        _ => {}
    }
    // Unknown keywords: JSON Schema ignores them.
    if c.one_in(3) {
        s.insert("x-unknown".into(), json!({ "type": "never", "required": ["zz"] }));
        s.insert("description".into(), json!("d"));
        s.insert("minLengthy".into(), json!(1_000_000));
    }
    // `$ref` next to keywords that `v` violates: draft 7 ignores the siblings of `$ref`.
    if c.one_in(6) {
        s.insert("$ref".into(), json!("#/$defs/any"));
        s.insert("type".into(), json!(wrong_type(v)));
        s.insert("enum".into(), json!([]));
        s.insert("required".into(), json!(["never-present-key"]));
    }
    Value::Object(s)
}

/// A random object as the arguments, and a schema derived from it that it satisfies.
/// The arguments as JSON text, and the schema read back from its text, as in production.
fn value_and_satisfied_schema() -> impl Strategy<Value = (String, Value)> {
    let root = prop::collection::btree_map("[a-d~/]{1,2}", json_value(), 0..6).prop_map(|m| Value::Object(m.into_iter().collect()));
    (root, any::<u64>()).prop_map(|(value, seed)| {
        let mut schema = derive(&value, &mut Choices(seed));
        if let Value::Object(s) = &mut schema {
            s.insert("$defs".into(), json!({ "any": true }));
        }
        let schema_text = with_long_numbers(&schema);
        let schema = serde_json::from_str(&schema_text).unwrap_or(Value::Null);
        (with_long_numbers(&value), schema)
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]
    #[test]
    fn no_false_invalid_args((value, schema) in value_and_satisfied_schema()) {
        prop_assert!(schema.is_object(), "the schema text did not read back");
        let input = ToolInput::Json(value.clone());
        let subject = CallSubject::new("t", &input, Declared::Yes).with_schema(&schema);
        let finding = invalid_args(&subject);
        prop_assert!(finding.is_none(), "{value} {schema} {finding:?}");
    }
}
