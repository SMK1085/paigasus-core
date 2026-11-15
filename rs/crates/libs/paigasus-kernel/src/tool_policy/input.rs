// SPDX-License-Identifier: Apache-2.0
//! The mapping of tool arguments into `context.input` (§ 6.1.4).

use std::collections::HashSet;
use std::fmt;

use cedar_policy::RestrictedExpression;
use serde::de::{Deserialize, Deserializer, Error as _, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::ToolInput;

/// A value nested deeper than this becomes a string of its raw JSON text (§ 6.1.4).
const MAX_DEPTH: usize = 16;
/// Arguments nested deeper than this are unparsed. `RawValue` skips the 128 level limit of
/// `serde_json`, so the walk below checks the depth itself before it recurses.
const MAX_NESTING: usize = 128;
/// The largest exponent the number reader tracks. A larger one saturates here. No text length
/// reaches this bound, so the length of the fraction never cancels a saturated exponent.
const EXPONENT_CAP: i64 = i64::MAX / 4;

/// How a JSON number maps into Cedar, by value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NumberClass {
    Long(i64),
    Decimal(String),
    Text(String),
}

/// The result of mapping the arguments of one call.
pub(crate) struct MappedInput {
    pub input: RestrictedExpression,
    pub input_text: Option<String>,
    pub unparsed: bool,
}

/// Classify a JSON number by its value, from its text, with no float. The text comes from a valid
/// JSON document. A value that `Long` or `decimal` cannot hold exactly, or text that this reader
/// cannot split into digits and an exponent, gives `Text` with the original text.
pub(crate) fn classify_number(text: &str) -> NumberClass {
    classify(text).unwrap_or_else(|| NumberClass::Text(text.to_owned()))
}

fn is_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// The `i64` for a sign and a magnitude, if it fits.
fn to_i64(negative: bool, magnitude: u128) -> Option<i64> {
    if negative {
        (magnitude <= 1u128 << 63).then(|| (-(magnitude as i128)) as i64)
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// `Some(class)` for a number that `Long` or `decimal` holds exactly; `None` means "keep the text".
fn classify(text: &str) -> Option<NumberClass> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int_part.is_empty() || !is_digits(int_part) || !is_digits(frac_part) {
        return None;
    }
    let mut exp: i64 = 0;
    if let Some(e) = exponent {
        let (exp_negative, digits) = match e.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, e.strip_prefix('+').unwrap_or(e)),
        };
        if digits.is_empty() || !is_digits(digits) {
            return None;
        }
        for b in digits.bytes() {
            exp = exp.saturating_mul(10).saturating_add(i64::from(b - b'0')).min(EXPONENT_CAP);
        }
        if exp_negative {
            exp = -exp;
        }
    }
    // value = sign * digits * 10^scale, with no leading and no trailing zero in `digits`.
    let mut digits: String = format!("{int_part}{frac_part}").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some(NumberClass::Long(0));
    }
    let mut scale = exp.saturating_sub(i64::try_from(frac_part.len()).ok()?);
    while digits.ends_with('0') {
        digits.pop();
        scale += 1;
    }
    if digits.len() > 19 {
        return None;
    }
    let base: u128 = digits.parse().ok()?;
    if scale >= 0 {
        if i64::try_from(digits.len()).ok()? + scale > 19 {
            return None;
        }
        let magnitude = base * 10u128.pow(u32::try_from(scale).ok()?);
        return to_i64(negative, magnitude).map(NumberClass::Long);
    }
    let fraction_digits = usize::try_from(-scale).ok()?;
    if fraction_digits > 4 {
        return None;
    }
    // Cedar `decimal` holds value * 10^4 in an i64.
    let scaled = base * 10u128.pow(u32::try_from(4 - fraction_digits).ok()?);
    to_i64(negative, scaled)?;
    let (int_digits, frac_digits) = if digits.len() <= fraction_digits {
        ("0".to_owned(), format!("{}{digits}", "0".repeat(fraction_digits - digits.len())))
    } else {
        let (i, f) = digits.split_at(digits.len() - fraction_digits);
        (i.to_owned(), f.to_owned())
    };
    let sign = if negative { "-" } else { "" };
    Some(NumberClass::Decimal(format!("{sign}{int_digits}.{frac_digits}")))
}

/// The arguments are not one strict JSON object.
struct Unparsed;

/// `Ok(None)` is a JSON null: the key is left out.
type Walk = Result<Option<RestrictedExpression>, Unparsed>;

/// The members of one JSON object, in order. A duplicate key is an error.
struct Entries<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for Entries<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entries<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(A::Error::custom("duplicate key"));
                    }
                    out.push((key, map.next_value::<&'de RawValue>()?));
                }
                Ok(Entries(out))
            }
        }
        deserializer.deserialize_map(V)
    }
}

fn first_byte(text: &str) -> u8 {
    text.as_bytes().first().copied().unwrap_or(0)
}

fn entries(text: &str) -> Result<Vec<(String, &RawValue)>, Unparsed> {
    serde_json::from_str::<Entries<'_>>(text).map(|e| e.0).map_err(|_| Unparsed)
}

fn elements(text: &str) -> Result<Vec<&RawValue>, Unparsed> {
    serde_json::from_str::<Vec<&RawValue>>(text).map_err(|_| Unparsed)
}

/// Check a container that is too deep to map: a duplicate key at any depth still makes the
/// arguments unparsed.
fn validate(raw: &RawValue) -> Result<(), Unparsed> {
    let text = raw.get();
    match first_byte(text) {
        b'{' => entries(text)?.into_iter().try_for_each(|(_, v)| validate(v)),
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
            for (key, value) in entries(text)? {
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

/// Whether the brackets of one valid JSON text nest at most `MAX_NESTING` levels. Iterative, so a
/// deep text cannot overflow the stack. A bracket inside a string does not count.
fn nesting_ok(text: &str) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for b in text.bytes() {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return false;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
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
    fn integers_in_any_spelling_are_long() {
        for t in ["5", "5.0", "5e0", "0.5e1", "50e-1"] {
            assert_eq!(classify_number(t), NumberClass::Long(5), "{t}");
        }
        assert_eq!(classify_number("-0"), NumberClass::Long(0));
        assert_eq!(classify_number("9223372036854775807"), NumberClass::Long(i64::MAX));
        assert_eq!(classify_number("-9223372036854775808"), NumberClass::Long(i64::MIN));
        assert_eq!(classify_number("9223372036854775808"), NumberClass::Text("9223372036854775808".into()));
    }

    #[test]
    fn decimal_edges() {
        assert_eq!(classify_number("1.5"), NumberClass::Decimal("1.5".into()));
        assert_eq!(classify_number("0.1234"), NumberClass::Decimal("0.1234".into()));
        assert_eq!(classify_number("0.12345"), NumberClass::Text("0.12345".into()));
        assert_eq!(classify_number("922337203685477.5807"), NumberClass::Decimal("922337203685477.5807".into()));
        assert_eq!(classify_number("922337203685477.5808"), NumberClass::Text("922337203685477.5808".into()));
        assert_eq!(classify_number("-922337203685477.5808"), NumberClass::Decimal("-922337203685477.5808".into()));
        assert_eq!(classify_number("-0.5"), NumberClass::Decimal("-0.5".into()));
        assert_eq!(classify_number("5e-3"), NumberClass::Decimal("0.005".into()));
        assert_eq!(classify_number("125e-1"), NumberClass::Decimal("12.5".into()));
    }

    #[test]
    fn a_big_integer_keeps_its_text() {
        assert_eq!(classify_number("12345678901234567890"), NumberClass::Text("12345678901234567890".into()));
        assert_eq!(classify_number("1e400"), NumberClass::Text("1e400".into()));
        assert_eq!(classify_number("1e99999999999999999999"), NumberClass::Text("1e99999999999999999999".into()));
        assert_eq!(classify_number("0e999999999"), NumberClass::Long(0));
        assert_eq!(classify_number("0.0e-99999999999999999999"), NumberClass::Long(0));
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
    fn an_inflated_fraction_or_exponent_does_not_change_the_value() {
        let long_fraction = format!("0.{}1e2000000", "0".repeat(1_000_000));
        assert_eq!(classify_number(&long_fraction), NumberClass::Text(long_fraction.clone()));
        let long_integer = format!("1{}e-2000000", "0".repeat(1_000_001));
        assert_eq!(classify_number(&long_integer), NumberClass::Text(long_integer.clone()));
        let huge_exponent = "1e999999999999999999999999";
        assert_eq!(classify_number(huge_exponent), NumberClass::Text(huge_exponent.into()));
        let tiny_exponent = "1e-999999999999999999999999";
        assert_eq!(classify_number(tiny_exponent), NumberClass::Text(tiny_exponent.into()));
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
