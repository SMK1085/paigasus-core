// SPDX-License-Identifier: Apache-2.0
//! The mapping of tool arguments into `context.input` (§ 6.1.4).

use cedar_policy::RestrictedExpression;
use serde_json::value::RawValue;

use super::ToolInput;
use crate::detect::input::{NumberClass, Unparsed, classify_number, elements, first_byte, nesting_ok, strict_entries};

/// A value nested deeper than this becomes a string of its raw JSON text (§ 6.1.4).
const MAX_DEPTH: usize = 16;

/// The result of mapping the arguments of one call.
pub(crate) struct MappedInput {
    pub input: RestrictedExpression,
    pub input_text: Option<String>,
    pub unparsed: bool,
}

/// `Ok(None)` is a JSON null: the key is left out.
type Walk = Result<Option<RestrictedExpression>, Unparsed>;
/// Check a container that is too deep to map: a duplicate key at any depth still makes the
/// arguments unparsed.
fn validate(raw: &RawValue) -> Result<(), Unparsed> {
    let text = raw.get();
    match first_byte(text) {
        b'{' => strict_entries(text)?.into_iter().try_for_each(|(_, v)| validate(v)),
        b'[' => elements(text)?.into_iter().try_for_each(validate),
        _ => Ok(()),
    }
}

fn walk(raw: &RawValue, depth: usize) -> Walk {
    let text = raw.get();
    let first = first_byte(text);
    // A null member is left out at every depth. Any other value deeper than MAX_DEPTH, a scalar
    // too, becomes a string of its raw JSON text.
    if depth > MAX_DEPTH && first != b'n' {
        validate(raw)?;
        return Ok(Some(RestrictedExpression::new_string(text.to_owned())));
    }
    match first {
        b'{' => {
            let mut fields = Vec::new();
            for (key, value) in strict_entries(text)? {
                if let Some(expr) = walk(value, depth + 1)? {
                    fields.push((key, expr));
                }
            }
            RestrictedExpression::new_record(fields).map(Some).map_err(|_| Unparsed)
        }
        b'[' => {
            let mut items = Vec::new();
            for value in elements(text)? {
                if let Some(expr) = walk(value, depth + 1)? {
                    items.push(expr);
                }
            }
            Ok(Some(RestrictedExpression::new_set(items)))
        }
        b'"' => serde_json::from_str::<String>(text).map(|s| Some(RestrictedExpression::new_string(s))).map_err(|_| Unparsed),
        b't' => Ok(Some(RestrictedExpression::new_bool(true))),
        b'f' => Ok(Some(RestrictedExpression::new_bool(false))),
        b'n' => Ok(None),
        _ => Ok(Some(match classify_number(text) {
            NumberClass::Long(n) => RestrictedExpression::new_long(n),
            NumberClass::Decimal(d) => RestrictedExpression::new_decimal(d),
            NumberClass::Text(t) => RestrictedExpression::new_string(t),
        })),
    }
}

fn empty_record() -> RestrictedExpression {
    RestrictedExpression::new_record(std::iter::empty()).expect("an empty record has no duplicate key")
}

fn parse_object(raw: &str) -> Option<RestrictedExpression> {
    let value: &RawValue = serde_json::from_str(raw).ok()?;
    if first_byte(value.get()) != b'{' || !nesting_ok(value.get()) {
        return None;
    }
    walk(value, 1).ok().flatten()
}

/// Map the arguments of one call into `context.input` and `context.input_text`.
///
/// Anything but one strict JSON object (invalid text, another JSON type, a duplicate key at any
/// depth, or free text) gives the empty record and `unparsed`. `input_text` is set only for free
/// text. A null member, or a null array element, is left out. The top-level object is depth 1;
/// a value (object, array or scalar) deeper than 16 becomes a string of its raw JSON text.
/// Arguments that nest deeper than 128 levels are unparsed.
pub(crate) fn map_input(input: &ToolInput) -> MappedInput {
    match input {
        ToolInput::Text(text) => MappedInput {
            input: empty_record(),
            input_text: Some(text.clone()),
            unparsed: true,
        },
        ToolInput::Json(raw) => match parse_object(raw) {
            Some(input) => MappedInput {
                input,
                input_text: None,
                unparsed: false,
            },
            None => MappedInput {
                input: empty_record(),
                input_text: None,
                unparsed: true,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use cedar_policy::{Authorizer, Context, Decision, Entities, EntityUid, PolicySet, Request};

    use super::*;

    /// Evaluate `permit(...) when { <cond> }` with `context.input` from the mapped arguments.
    fn allows(cond: &str, raw: &str) -> bool {
        let m = map_input(&ToolInput::Json(raw.into()));
        let ctx = Context::from_pairs([("input".to_owned(), m.input)]).expect("context");
        let uid = |s: &str| s.parse::<EntityUid>().expect("uid");
        let req = Request::new(uid("Pgs::Iam::Principal::\"u\""), uid("Pgs::Gateway::Action::\"x\""), uid("Pgs::Iam::Org::\"o\""), ctx, None).expect("request");
        let ps: PolicySet = format!("permit(principal, action, resource) when {{ {cond} }};").parse().expect("policy");
        Authorizer::new().is_authorized(&req, &ps, &Entities::empty()).decision() == Decision::Allow
    }

    #[test]
    fn every_mapping_row() {
        let raw = r#"{"o":{"k":"v"},"s":"str","t":true,"f":false,"n":5,"big":12345678901234567890,"d":1.5,"a":[1,"x",[2]],"z":null}"#;
        assert!(allows(r#"context.input.o.k == "v""#, raw), "object to record, keys unchanged");
        assert!(allows(r#"context.input.s == "str""#, raw), "string");
        assert!(allows("context.input.t == true && context.input.f == false", raw), "bool");
        assert!(allows("context.input.n == 5", raw), "integer to Long");
        assert!(allows(r#"context.input.big == "12345678901234567890""#, raw), "big integer keeps its text");
        assert!(allows(r#"context.input.d == decimal("1.5")"#, raw), "decimal");
        assert!(
            allows(r#"context.input.a.contains(1) && context.input.a.contains("x") && context.input.a.contains([2])"#, raw),
            "array to set"
        );
        assert!(allows("!(context.input has z)", raw), "null key left out");
        assert!(allows("context.input has o", raw));
    }

    #[test]
    fn entity_and_extension_escapes_stay_records() {
        let ent = r#"{"v":{"__entity":{"type":"Pgs::Iam::Principal","id":"x"}}}"#;
        assert!(allows(r#"context.input.v.__entity.id == "x""#, ent));
        assert!(allows(r#"context.input.v.__entity["type"] == "Pgs::Iam::Principal""#, ent));
        assert!(!allows("context.input.v.__entity is Pgs::Iam::Principal", ent), "an `is` test on a record never allows");
        assert!(!allows("context.input.v is Pgs::Iam::Principal", ent));
        let ext = r#"{"v":{"__extn":{"fn":"decimal","arg":"1.0"}}}"#;
        assert!(allows(r#"context.input.v.__extn.fn == "decimal""#, ext));
        assert!(!allows(r#"context.input.v == decimal("1.0")"#, ext), "not an extension value");
    }

    #[test]
    fn depth_seventeen_becomes_json_text() {
        // Sixteen nested objects map to records; the value at depth 17 is a string of its raw text.
        let mut raw = String::from(r#"{"x":1,  "y":2}"#);
        for _ in 0..16 {
            raw = format!(r#"{{"a":{raw}}}"#);
        }
        let path16: String = std::iter::repeat_n(".a", 16).collect();
        assert!(allows(&format!(r#"context.input{path16} == "{{\"x\":1,  \"y\":2}}""#), &raw), "raw text, original spacing");
        let mut ok = String::from(r#"{"x":1}"#);
        for _ in 0..15 {
            ok = format!(r#"{{"a":{ok}}}"#);
        }
        let path15: String = std::iter::repeat_n(".a", 15).collect();
        assert!(allows(&format!("context.input{path15} has x"), &ok), "depth 16 is still a record");
        assert!(allows(&format!(r#"context.input{path15}.x == "1""#), &ok), "its member is at depth 17: raw text");
    }

    #[test]
    fn a_scalar_at_depth_seventeen_becomes_its_raw_text() {
        let mut raw = String::from(r#"{"s":"v","n":12345678901234567890,"t":true,"z":null,"a":[1,null,"q"]}"#);
        for _ in 0..15 {
            raw = format!(r#"{{"a":{raw}}}"#);
        }
        let p: String = std::iter::repeat_n(".a", 15).collect();
        assert!(allows(&format!(r#"context.input{p}.s == "\"v\"""#), &raw), "string keeps its quotes");
        assert!(allows(&format!(r#"context.input{p}.n == "12345678901234567890""#), &raw));
        assert!(allows(&format!(r#"context.input{p}.t == "true""#), &raw));
        assert!(allows(&format!("!(context.input{p} has z)"), &raw), "a null member is left out at any depth");
        assert!(allows(&format!(r#"context.input{p}.a == "[1,null,\"q\"]""#), &raw), "an array at depth 17 is raw text");
    }

    #[test]
    fn a_null_array_element_is_dropped() {
        // Cedar compares sets as sets, so equality proves that the set holds exactly these elements.
        assert!(allows(r#"context.input.a == [1, "x"]"#, r#"{"a":[null,1,null,"x"]}"#));
        assert!(allows("context.input.a == []", r#"{"a":[null,null]}"#), "only nulls: the empty set");
    }

    #[test]
    fn very_deep_arguments_are_unparsed_and_do_not_overflow_the_stack() {
        for n in [129usize, 20_000, 100_000] {
            let raw = format!("{{\"a\":{}1{}}}", "[".repeat(n), "]".repeat(n));
            let m = map_input(&ToolInput::Json(raw));
            assert!(m.unparsed, "depth {n}");
            assert_eq!(m.input_text, None);
        }
        let raw = format!("{{\"a\":{}1{}}}", "{\"b\":".repeat(100_000), "}".repeat(100_000));
        assert!(map_input(&ToolInput::Json(raw)).unparsed);
        let ok = format!("{{\"a\":{}1{}}}", "[".repeat(126), "]".repeat(126));
        assert!(!map_input(&ToolInput::Json(ok)).unparsed, "127 levels still parse");
        let in_string = format!("{{\"a\":\"{}\"}}", "[".repeat(1000));
        assert!(!map_input(&ToolInput::Json(in_string)).unparsed, "brackets in a string do not count");
    }

    #[test]
    fn a_duplicate_key_below_the_depth_limit_is_unparsed() {
        let mut raw = String::from(r#"{"k":1,"k":2}"#);
        for _ in 0..20 {
            raw = format!(r#"{{"a":{raw}}}"#);
        }
        let m = map_input(&ToolInput::Json(raw));
        assert!(m.unparsed);
    }

    #[test]
    fn non_object_json_is_unparsed() {
        for raw in ["[1]", "\"x\"", "{", "{\"a\":1,\"a\":2}", "{\"a\":{\"b\":1,\"b\":2}}", "{\"a\":[{\"b\":1,\"b\":2}]}", ""] {
            let m = map_input(&ToolInput::Json(raw.into()));
            assert!(m.unparsed, "{raw}");
            assert_eq!(m.input_text, None, "{raw}");
            assert!(allows("!(context.input has a)", raw), "input is the empty record: {raw}");
        }
        let m = map_input(&ToolInput::Text("*** Begin Patch".into()));
        assert!(m.unparsed);
        assert_eq!(m.input_text.as_deref(), Some("*** Begin Patch"));
    }

    #[test]
    fn a_strict_object_is_parsed_and_has_no_input_text() {
        let m = map_input(&ToolInput::Json(r#" {"a": 1} "#.into()));
        assert!(!m.unparsed);
        assert_eq!(m.input_text, None);
    }
}
