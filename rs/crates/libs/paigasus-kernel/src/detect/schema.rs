// SPDX-License-Identifier: Apache-2.0
//! `invalid_args` (§ 6.3.2): a validator for a JSON Schema subset. It knows `type`, `enum`,
//! `required`, `properties`, `additionalProperties: false` and `items`, and it ignores every other
//! keyword.
//!
//! JSON Schema keywords are conjunctive, so a check of some keywords can miss a violation but
//! cannot invent one (A7). The validator skips each exception to that rule: a node with a `$ref`,
//! `additionalProperties: false` next to `patternProperties`, `items` next to `prefixItems`, a
//! schema-valued `additionalProperties`, the tuple form of `items`, and a malformed keyword value.
//! When in doubt, it gives no signal.
//!
//! The cost is linear: one parse of the arguments, and at most `WORK_BUDGET` steps. Past the
//! budget, the check gives no finding.

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::input::{NumberClass, classify_number};
use super::walk::{InputMode, escape_into, input_mode};
use super::{CallSubject, Evidence, Finding, INVALID_ARGS, Severity, ToolInput};

/// The most steps that one call of the validator takes. One step is one schema node visit, one
/// `enum` element or `type` entry compared (plus one per member of a compared value), or one
/// `required` entry. Each such step also costs one step per `BYTES_PER_STEP` bytes of the string,
/// key, type name or `required` name that it reads, so the budget bounds the bytes too.
pub(crate) const WORK_BUDGET: u32 = 100_000;

/// The bytes that one extra step pays for.
const BYTES_PER_STEP: usize = 64;

/// The version of the validator rules.
const VERSION: &str = "v1";

/// The most evidence entries that this detector collects.
const MAX_EVIDENCE: usize = 16;

/// The rule of an evidence entry for the boolean schema `false`.
const FALSE_SCHEMA: &str = "false";

/// A number of the arguments, read once by value from its original text.
#[derive(Clone, Copy)]
enum NumValue {
    /// An integer that fits an `i64`.
    Int(i64),
    /// A value with a fraction that is not zero.
    Fraction,
    /// A value that the reader cannot classify exactly.
    Unknown,
}

impl NumValue {
    fn read(text: &str) -> Self {
        match classify_number(text) {
            NumberClass::Long(n) => Self::Int(n),
            NumberClass::Decimal(_) => Self::Fraction,
            NumberClass::Text(_) => Self::Unknown,
        }
    }
}

/// One value of the arguments.
enum Inst {
    Null,
    Bool(bool),
    Num(NumValue),
    Str(String),
    Arr(Vec<Inst>),
    Obj(Vec<(String, Inst)>),
}

/// The text of every number in a valid JSON text, in document order.
fn number_texts(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut out, mut i, mut in_string, mut escaped) = (Vec::new(), 0, false, false);
    while i < b.len() {
        let c = b[i];
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            i += 1;
        } else if c == b'"' {
            in_string = true;
            i += 1;
        } else if c == b'-' || c.is_ascii_digit() {
            let start = i;
            while i < b.len() && matches!(b[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                i += 1;
            }
            // The bytes are ASCII, so `start..i` is on character boundaries.
            out.push(text.get(start..i).unwrap_or(""));
        } else {
            i += 1;
        }
    }
    out
}

/// The parse of the arguments. Each number takes the next text of `numbers`, because the parser
/// meets the numbers in document order.
struct Seed<'a, 't> {
    numbers: &'a [&'t str],
    next: &'a Cell<usize>,
}

impl<'t> Seed<'_, 't> {
    fn number<E: serde::de::Error>(&self) -> Result<Inst, E> {
        let i = self.next.get();
        self.next.set(i + 1);
        self.numbers.get(i).map(|t| Inst::Num(NumValue::read(t))).ok_or_else(|| E::custom("number count"))
    }
    fn child(&self) -> Self {
        Seed {
            numbers: self.numbers,
            next: self.next,
        }
    }
}

impl<'de, 't> DeserializeSeed<'de> for Seed<'_, 't> {
    type Value = Inst;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Inst, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de, 't> Visitor<'de> for Seed<'_, 't> {
    type Value = Inst;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Inst, E> {
        Ok(Inst::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Inst, E> {
        self.number()
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Inst, E> {
        self.number()
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Inst, E> {
        self.number()
    }
    fn visit_unit<E>(self) -> Result<Inst, E> {
        Ok(Inst::Null)
    }
    fn visit_str<E>(self, v: &str) -> Result<Inst, E> {
        Ok(Inst::Str(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Inst, E> {
        Ok(Inst::Str(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Inst, A::Error> {
        let mut out = Vec::new();
        while let Some(n) = seq.next_element_seed(self.child())? {
            out.push(n);
        }
        Ok(Inst::Arr(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Inst, A::Error> {
        let mut out = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            out.push((key, map.next_value_seed(self.child())?));
        }
        Ok(Inst::Obj(out))
    }
}

/// The arguments as a tree, or `None`. The caller has checked the mode, so the text is valid JSON
/// with unique keys and at most 128 levels.
fn parse(text: &str) -> Option<Inst> {
    let numbers = number_texts(text);
    let next = Cell::new(0);
    let mut de = serde_json::Deserializer::from_str(text);
    de.disable_recursion_limit();
    let inst = Seed { numbers: &numbers, next: &next }.deserialize(&mut de).ok()?;
    de.end().ok()?;
    (next.get() == numbers.len()).then_some(inst)
}

/// The work budget is spent: the check stops and gives no finding, also when it found violations
/// before (A7).
struct Stop;

struct State {
    steps: u64,
    evidence: Vec<Evidence>,
}

impl State {
    fn step(&mut self) -> Result<(), Stop> {
        self.charge_steps(1)
    }
    /// One step per `BYTES_PER_STEP` bytes that the check reads.
    fn charge(&mut self, bytes: usize) -> Result<(), Stop> {
        self.charge_steps(u64::try_from(bytes / BYTES_PER_STEP).unwrap_or(u64::MAX))
    }
    fn charge_steps(&mut self, steps: u64) -> Result<(), Stop> {
        self.steps = self.steps.saturating_add(steps);
        if self.steps > u64::from(WORK_BUDGET) { Err(Stop) } else { Ok(()) }
    }
    /// Keep one violation. Past the sixteenth, the check goes on, so that the budget still bounds
    /// the whole call, but it keeps no more evidence.
    fn violate(&mut self, rule: &'static str, pointer: &str) {
        if self.evidence.len() < MAX_EVIDENCE {
            self.evidence.push(Evidence::argument(rule, pointer.to_owned()));
        }
    }
}

/// Whether `inst` has the type `name`. `None`: the name is unknown, or the number cannot be read
/// by value, so the caller skips the keyword.
fn type_matches(name: &str, inst: &Inst) -> Option<bool> {
    Some(match name {
        "null" => matches!(inst, Inst::Null),
        "boolean" => matches!(inst, Inst::Bool(_)),
        "string" => matches!(inst, Inst::Str(_)),
        "array" => matches!(inst, Inst::Arr(_)),
        "object" => matches!(inst, Inst::Obj(_)),
        "number" => matches!(inst, Inst::Num(_)),
        "integer" => match inst {
            Inst::Num(NumValue::Int(_)) => true,
            Inst::Num(NumValue::Fraction) => false,
            Inst::Num(NumValue::Unknown) => return None,
            _ => false,
        },
        _ => return None,
    })
}

/// The `type` keyword. `Ok(None)`: skip.
fn check_type(keyword: &Value, inst: &Inst, st: &mut State) -> Result<Option<bool>, Stop> {
    let names: &[Value] = match keyword {
        Value::String(_) => std::slice::from_ref(keyword),
        Value::Array(names) if !names.is_empty() => names,
        _ => return Ok(None),
    };
    let mut matched = false;
    for name in names {
        st.step()?;
        let Value::String(name) = name else { return Ok(None) };
        st.charge(name.len())?;
        match type_matches(name, inst) {
            None => return Ok(None),
            Some(m) => matched |= m,
        }
    }
    Ok(Some(matched))
}

/// Whether two numbers can be equal by value. "Not equal" only when it is provable (A7): the
/// schema number is an exact integer, and the instance is another integer or has a fraction.
/// `serde_json` keeps a number with no fraction and no exponent exactly, as `i64` or `u64`. It
/// reads every other number to a double, which can differ from the double of the same text in
/// another reader, so such a number never gives a signal.
fn numbers_equal(inst: NumValue, n: &serde_json::Number) -> bool {
    let exact = n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from));
    match (inst, exact) {
        (NumValue::Int(a), Some(b)) => i128::from(a) == b,
        (NumValue::Fraction, Some(_)) => false,
        _ => true,
    }
}

/// JSON equality of an instance and a schema value, one step per compared value.
fn equal(inst: &Inst, value: &Value, st: &mut State) -> Result<bool, Stop> {
    st.step()?;
    Ok(match (inst, value) {
        (Inst::Null, Value::Null) => true,
        (Inst::Bool(a), Value::Bool(b)) => a == b,
        (Inst::Num(a), Value::Number(b)) => numbers_equal(*a, b),
        (Inst::Str(a), Value::String(b)) => {
            st.charge(a.len())?;
            a == b
        }
        (Inst::Arr(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (x, y) in a.iter().zip(b) {
                if !equal(x, y, st)? {
                    return Ok(false);
                }
            }
            true
        }
        (Inst::Obj(a), Value::Object(b)) => {
            // The instance keys are unique, so the same count and every key present is the same set.
            if a.len() != b.len() {
                return Ok(false);
            }
            for (k, x) in a {
                st.charge(k.len())?;
                match b.get(k) {
                    Some(y) if equal(x, y, st)? => {}
                    _ => return Ok(false),
                }
            }
            true
        }
        _ => false,
    })
}

/// The `enum` keyword. `Ok(None)`: skip.
fn check_enum(keyword: &Value, inst: &Inst, st: &mut State) -> Result<Option<bool>, Stop> {
    let Value::Array(choices) = keyword else { return Ok(None) };
    for choice in choices {
        if equal(inst, choice, st)? {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

/// The object keywords of one node.
fn check_object(node: &Map<String, Value>, members: &[(String, Inst)], pointer: &mut String, st: &mut State) -> Result<(), Stop> {
    if let Some(Value::Array(required)) = node.get("required") {
        let keys: HashSet<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
        let (mut missing, mut valid) = (false, true);
        for name in required {
            st.step()?;
            match name {
                Value::String(name) => {
                    st.charge(name.len())?;
                    missing |= !keys.contains(name.as_str());
                }
                _ => valid = false,
            }
        }
        if valid && missing {
            st.violate("required", pointer);
        }
    }
    // `None`: `properties` is malformed, so neither it nor `additionalProperties` is checked.
    let properties = match node.get("properties") {
        None => Some(None),
        Some(Value::Object(p)) => Some(Some(p)),
        Some(_) => None,
    };
    let closed = properties.is_some() && node.get("additionalProperties") == Some(&Value::Bool(false)) && !node.contains_key("patternProperties");
    for (key, value) in members {
        let len = pointer.len();
        pointer.push('/');
        escape_into(pointer, key);
        match properties.flatten().and_then(|p| p.get(key)) {
            Some(sub) => check(sub, value, pointer, st)?,
            None if closed => st.violate("additionalProperties", pointer),
            None => {}
        }
        pointer.truncate(len);
    }
    Ok(())
}

/// One schema node against one instance.
fn check(schema: &Value, inst: &Inst, pointer: &mut String, st: &mut State) -> Result<(), Stop> {
    st.step()?;
    let node = match schema {
        Value::Bool(false) => {
            st.violate(FALSE_SCHEMA, pointer);
            return Ok(());
        }
        Value::Object(node) => node,
        _ => return Ok(()),
    };
    if ["$ref", "$dynamicRef", "$recursiveRef"].iter().any(|k| node.contains_key(*k)) {
        return Ok(()); // SKIP-REF
    }
    // OpenAPI 3.0 `nullable: true` widens `type` and `enum`, so neither is checked.
    let nullable = node.get("nullable") == Some(&Value::Bool(true));
    if let Some(keyword) = node.get("type")
        && !nullable
        && check_type(keyword, inst, st)? == Some(false)
    {
        st.violate("type", pointer);
    }
    if let Some(keyword) = node.get("enum")
        && !nullable
        && check_enum(keyword, inst, st)? == Some(false)
    {
        st.violate("enum", pointer);
    }
    match inst {
        Inst::Obj(members) => check_object(node, members, pointer, st)?,
        Inst::Arr(elements) => {
            if let Some(items) = node.get("items")
                && !node.contains_key("prefixItems") // SKIP-PREFIX-ITEMS
                && matches!(items, Value::Object(_) | Value::Bool(_))
            {
                for (i, element) in elements.iter().enumerate() {
                    let len = pointer.len();
                    pointer.push('/');
                    pointer.push_str(&i.to_string());
                    check(items, element, pointer, st)?;
                    pointer.truncate(len);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The `invalid_args` finding for strict JSON arguments against a schema object, or `None`.
pub(crate) fn validate_args(input: &ToolInput, schema: &Value) -> Option<Finding> {
    let ToolInput::Json(text) = input else { return None };
    if !schema.is_object() || input_mode(input) != InputMode::Strict {
        return None;
    }
    let inst = parse(text)?;
    let mut st = State { steps: 0, evidence: Vec::new() };
    check(schema, &inst, &mut String::new(), &mut st).ok()?;
    if st.evidence.is_empty() {
        return None;
    }
    let mut finding = Finding::new(INVALID_ARGS, INVALID_ARGS, Severity::Low, VERSION);
    for e in st.evidence {
        finding = finding.with_evidence(e);
    }
    Some(finding)
}

/// `invalid_args`: the arguments of a call do not match the JSON Schema of its definition. One
/// finding per call, `low`, with one evidence entry per violation (at most 16): the keyword as the
/// rule and the JSON pointer of the value. No schema, a schema that is not an object, arguments
/// that are not one strict JSON object, or a spent work budget: no finding.
#[must_use]
pub fn invalid_args(subject: &CallSubject<'_>) -> Option<Finding> {
    validate_args(subject.input, subject.schema?)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;
    use crate::detect::{INVALID_ARGS, Location, Severity};

    /// The finding for `value` under the key `v`, with `schema` as the schema of that key.
    fn check(schema: Value, value: Value) -> Option<Finding> {
        validate_args(&ToolInput::Json(json!({ "v": value }).to_string()), &json!({ "properties": { "v": schema } }))
    }
    fn bad(schema: Value, value: Value) -> bool {
        check(schema, value).is_some()
    }
    /// As `bad`, with the value as JSON text, so that the spelling of a number stays.
    fn bad_text(schema: &str, value: &str) -> bool {
        let schema: Value = serde_json::from_str(schema).expect("test schema");
        validate_args(&ToolInput::Json(format!(r#"{{"v":{value}}}"#)), &json!({ "properties": { "v": schema } })).is_some()
    }
    fn pointers(f: &Finding) -> Vec<(&'static str, String)> {
        f.evidence
            .iter()
            .map(|e| match &e.location {
                Location::Argument { pointer } => (e.rule, pointer.clone()),
                other => panic!("not an argument: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn each_keyword_on_its_type_and_on_another_type() {
        assert!(bad(json!({"type":"string"}), json!(1)) && !bad(json!({"type":["string","null"]}), json!(null)));
        assert!(bad(json!({"enum":[1,"a"]}), json!(2)) && !bad(json!({"enum":[{"a":1,"b":2}]}), json!({"b":2,"a":1})));
        assert!(bad(json!({"required":["a"]}), json!({})) && !bad(json!({"required":["a"]}), json!("not an object")));
        assert!(bad(json!({"properties":{"a":{"type":"integer"}}}), json!({"a":"x"})) && !bad(json!({"properties":{"a":{"type":"integer"}}}), json!([1])));
        assert!(bad(json!({"additionalProperties":false,"properties":{"a":{}}}), json!({"b":1})));
        assert!(!bad(json!({"additionalProperties":false,"properties":{"a":{}}}), json!({"a":1})));
        assert!(bad(json!({"items":{"type":"string"}}), json!([1])) && !bad(json!({"items":{"type":"string"}}), json!({"0":1})));
        assert!(bad(json!({"enum":[[1,2]]}), json!([2, 1])) && !bad(json!({"enum":[[1,{"a":null}]]}), json!([1, {"a":null}])));
        for (t, v) in [
            ("null", json!(null)),
            ("boolean", json!(true)),
            ("number", json!(1.5)),
            ("string", json!("s")),
            ("array", json!([])),
            ("object", json!({})),
        ] {
            assert!(!bad(json!({ "type": t }), v.clone()), "{t}");
            let other = if t == "null" { "string" } else { "null" };
            assert!(bad(json!({ "type": other }), v), "{t}");
        }
    }

    #[test]
    fn integer_by_value() {
        for v in ["5", "5.0", "5e0", "-0", "50e-1", "1.0e2"] {
            assert!(!bad_text(r#"{"type":"integer"}"#, v), "{v}");
        }
        assert!(bad_text(r#"{"type":"integer"}"#, "5.5"));
        assert!(!bad_text(r#"{"type":"number"}"#, "5.5"));
        // A number that the reader cannot classify by value: no signal.
        assert!(!bad_text(r#"{"type":"integer"}"#, "12345678901234567890.5"));
    }

    #[test]
    fn enum_numbers_compare_by_value() {
        assert!(!bad_text(r#"{"enum":[1]}"#, "1.0"));
        assert!(!bad_text(r#"{"enum":[1.0]}"#, "1"));
        assert!(!bad_text(r#"{"enum":[0.1]}"#, "1e-1"));
        assert!(bad_text(r#"{"enum":[1]}"#, "1.5"));
    }

    #[test]
    fn skips() {
        assert!(!bad(json!({"$ref":"#/x","type":"string"}), json!(1)));
        assert!(!bad(json!({"$dynamicRef":"#x","type":"string"}), json!(1)));
        assert!(!bad(json!({"$recursiveRef":"#","type":"string"}), json!(1)));
        assert!(!bad(json!({"additionalProperties":false,"patternProperties":{"^x":{}}}), json!({"b":1})));
        assert!(!bad(json!({"items":{"type":"string"},"prefixItems":[{}]}), json!([1])));
        assert!(!bad(json!({"additionalProperties":{"type":"string"}}), json!({"b":1})));
        assert!(!bad(json!({"items":[{"type":"string"}]}), json!([1])));
        assert!(!bad(json!({"type":"uuid"}), json!(1)) && !bad(json!({"minLength":100}), json!("x")));
        assert!(!bad(json!({"type":["uuid","string"]}), json!(1)) && !bad(json!({"type":[]}), json!(1)));
        assert!(!bad(json!(true), json!(1)) && bad(json!(false), json!(1)));
        // A malformed keyword value: the keyword is skipped.
        assert!(!bad(json!({"required":["a", 1]}), json!({})) && !bad(json!({"enum":"a"}), json!("b")));
        assert!(!bad(json!({"additionalProperties":false,"properties":[]}), json!({"b":1})));
        assert!(!bad(json!({"type":1}), json!("x")) && !bad(json!(1), json!("x")));
        // OpenAPI 3.0 `nullable`: no type and no enum check at that node.
        assert!(!bad(json!({"type":"string","nullable":true}), json!(null)));
        assert!(!bad(json!({"enum":["a"],"nullable":true}), json!(null)));
    }

    #[test]
    fn null_counts_as_present_and_typed() {
        assert!(!bad(json!({"required":["a"]}), json!({"a":null})) && bad(json!({"properties":{"a":{"type":"string"}}}), json!({"a":null})));
        assert!(!bad(json!({"type":["object","null"],"required":["a"]}), json!(null)));
    }

    #[test]
    fn the_work_budget_stops_with_no_finding() {
        let choices: Vec<Value> = (0..2000).map(|i| json!({ "k": i })).collect();
        let schema = json!({ "items": { "enum": choices } });
        let start = Instant::now();
        assert!(check(schema.clone(), json!((0..1000).collect::<Vec<_>>())).is_none());
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        // The same schema over one element is in the budget, so the check gives a finding.
        assert!(check(schema, json!([0])).is_some());
        // A long `type` array counts too.
        let types: Vec<&str> = std::iter::repeat_n("string", 2000).collect();
        assert!(check(json!({ "items": { "type": types } }), json!((0..1000).collect::<Vec<_>>())).is_none());
        // Violations before the budget runs out do not give a finding either.
        let schema = json!({ "properties": { "a": { "type": "string" }, "b": { "items": { "enum": choices } } } });
        assert!(check(schema, json!({ "a": 1, "b": (0..1000).collect::<Vec<_>>() })).is_none());
        // A long `required` list counts too.
        let keys: Vec<String> = (0..200_000).map(|i| i.to_string()).collect();
        assert!(check(json!({ "required": keys }), json!({})).is_none());
    }

    #[test]
    fn no_finding_for_text_duplicate_keys_or_bad_json() {
        let schema = json!({ "type": "object", "required": ["z"] });
        for input in [
            ToolInput::Text("{}".into()),
            ToolInput::Json(r#"{"a":1,"a":2}"#.into()),
            ToolInput::Json("{".into()),
            ToolInput::Json("[]".into()),
        ] {
            assert!(validate_args(&input, &schema).is_none(), "{input:?}");
        }
        assert!(validate_args(&ToolInput::Json("{}".into()), &schema).is_some());
        for schema in [json!(false), json!(true), json!([]), json!("x")] {
            assert!(validate_args(&ToolInput::Json("{}".into()), &schema).is_none());
        }
        let deep = format!(r#"{{"a":{}1{}}}"#, "[".repeat(200), "]".repeat(200));
        assert!(validate_args(&ToolInput::Json(deep), &schema).is_none());
    }

    /// The schema comes from text, as in production. `serde_json` with no `float_roundtrip` can
    /// read the same long decimal to another double than `str::parse` does.
    #[test]
    fn enum_numbers_from_schema_text_never_give_a_false_finding() {
        let exact_1e300 = format!("{:.0}", 1e300_f64);
        let same = [
            "0.20082962974712488058302",
            "0.334274685957130011297",
            "1e300",
            exact_1e300.as_str(),
            "123456789012345678901234567890",
            "9007199254740993",
            "18446744073709551616",
            "1.00000000000000000000000000001",
            "-0.000000000000000000000000000000017",
        ];
        for text in same {
            assert!(!bad_text(&format!(r#"{{"enum":[{text}]}}"#), text), "{text}");
            assert!(!bad_text(&format!(r#"{{"enum":[[{text}],{{"k":{text}}}]}}"#), &format!(r#"{{"k":{text}}}"#)), "{text}");
        }
        assert!(!bad_text(&format!(r#"{{"enum":[{exact_1e300}]}}"#), "1e300"));
        // Provably different: an integer schema number and an instance of another value.
        assert!(bad_text(r#"{"enum":[2]}"#, "1") && bad_text(r#"{"enum":[2]}"#, "2.5") && bad_text(r#"{"enum":[-3]}"#, "3"));
        assert!(bad_text(r#"{"enum":[18446744073709551615]}"#, "9223372036854775807"));
        assert!(!bad_text(r#"{"enum":[18446744073709551615]}"#, "18446744073709551615"));
        assert!(!bad_text(r#"{"enum":[2]}"#, "2.0") && !bad_text(r#"{"enum":[2]}"#, "20e-1"));
        // The same class of input for `integer`: the reading is exact, so no false finding.
        for text in ["123456789012345678901234567890", "1.000000000000000000000000000000", "1e300", exact_1e300.as_str()] {
            assert!(!bad_text(r#"{"type":"integer"}"#, text), "{text}");
        }
        assert!(bad_text(r#"{"type":"integer"}"#, "0.5") && bad_text(r#"{"type":"integer"}"#, "-1.25"));
    }

    /// One step must not cost an unbounded number of bytes. Each case took 0.17 s to 160 s
    /// (release) before the byte cost counted against the budget.
    #[test]
    fn large_tokens_count_against_the_budget() {
        let mib = 1 << 20;
        let elements: Vec<Value> = (0..50_000).map(|_| json!({"b": 0})).collect();
        let cases: Vec<(Value, String)> = vec![
            (json!({"items": {"required": ["x".repeat(mib)]}}), json!(elements).to_string()),
            (json!({"type": vec!["integer"; 99_990]}), format!("1.{}", "0".repeat(mib))),
            (json!({"enum": vec![2; 99_990]}), format!("1.{}", "0".repeat(mib))),
            (json!({"enum": vec!["a".repeat(53_000); 300]}), json!(format!("{}b", "a".repeat(52_999))).to_string()),
            (json!({"enum": vec![json!({"k".repeat(53_000): 1}); 300]}), json!({format!("{}j", "k".repeat(52_999)): 1}).to_string()),
            (json!({"type": vec!["x".repeat(53_000); 300]}), "1".to_owned()),
        ];
        for (i, (schema, value)) in cases.into_iter().enumerate() {
            let input = ToolInput::Json(format!(r#"{{"v":{value}}}"#));
            let schema = json!({ "properties": { "v": schema } });
            let start = Instant::now();
            let found = validate_args(&input, &schema);
            let took = start.elapsed();
            // Cases 1 and 2 stay in the budget: 1.000... is an integer, and it is not 2. The other
            // cases spend the budget on bytes, so they give no finding.
            assert_eq!(found.is_some(), i == 2, "case {i}");
            // Measured: at most 140 ms in a debug build, 16 ms in release. Before the byte cost,
            // cases 0 to 2 took 14 s to 160 s in release.
            assert!(took < Duration::from_secs(2), "case {i}: {took:?}");
        }
    }

    #[test]
    fn pointers_and_keywords_are_the_evidence() {
        let schema = json!({"properties":{"a":{"properties":{"b":{"type":"string"}}}}});
        let f = validate_args(&ToolInput::Json(r#"{"a":{"b":1}}"#.into()), &schema).expect("a finding");
        assert_eq!(
            (f.detector.as_str(), f.kind.as_str(), f.severity, f.version.as_str()),
            (INVALID_ARGS, INVALID_ARGS, Severity::Low, "v1")
        );
        assert_eq!(pointers(&f), [("type", "/a/b".to_owned())]);

        let schema = json!({
            "required": ["r"],
            "additionalProperties": false,
            "properties": { "a/b": { "enum": [1] }, "c": { "items": false } }
        });
        let f = validate_args(&ToolInput::Json(r#"{"a/b":2,"c":[0],"x~":3}"#.into()), &schema).expect("a finding");
        assert_eq!(
            pointers(&f),
            [
                ("required", String::new()),
                ("enum", "/a~1b".to_owned()),
                ("false", "/c/0".to_owned()),
                ("additionalProperties", "/x~0".to_owned()),
            ]
        );
    }

    #[test]
    fn at_most_16_evidence_entries() {
        let f = check(json!({"items":{"type":"string"}}), json!((0..40).collect::<Vec<_>>())).expect("a finding");
        assert_eq!(f.evidence.len(), 16);
        assert_eq!(pointers(&f)[15], ("type", "/v/15".to_owned()));
    }

    #[test]
    fn the_public_entry_needs_a_schema() {
        use crate::detect::Declared;
        let input = ToolInput::Json(r#"{"a":1}"#.into());
        let schema = json!({"properties":{"a":{"type":"string"}}});
        assert!(invalid_args(&CallSubject::new("t", &input, Declared::Yes)).is_none());
        assert!(invalid_args(&CallSubject::new("t", &input, Declared::Yes).with_schema(&schema)).is_some());
    }

    #[test]
    fn debug_of_the_finding_holds_no_content() {
        const CANARY: &str = "CANARY-5b1c";
        let f = check(json!({"properties":{ CANARY: {"type":"string"}}}), json!({ CANARY: 1 })).expect("a finding");
        assert!(!format!("{f:?}").contains(CANARY) && !format!("{:?}", f.evidence).contains(CANARY));
    }
}
