// SPDX-License-Identifier: Apache-2.0

use jiff::Timestamp;
use serde_json::{Map, Value};

use crate::event::{ID_MAX_CHARS, TYPE_MAX_CHARS, URI_MAX_CHARS, check_absolute_uri, check_extension_name, check_extension_value, check_text};
use crate::{CloudEvent, ExtensionValue, ParseError, SchemaError};

/// Read one CloudEvents 1.0 JSON event (structured mode).
///
/// A JSON `null` counts as an absent attribute. `data_base64` is not supported.
///
/// # Errors
/// Returns [`ParseError::Json`] for text that is not JSON. Returns [`ParseError::Schema`] for an
/// event that breaks the spec or a limit of this crate.
pub fn parse(bytes: &[u8]) -> Result<CloudEvent, ParseError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| ParseError::Json(e.to_string()))?;
    let Value::Object(mut map) = value else {
        return Err(SchemaError::WrongType("event").into());
    };
    map.retain(|_, v| !v.is_null());
    Ok(from_map(&mut map)?)
}

fn take_string(map: &mut Map<String, Value>, name: &'static str) -> Result<Option<String>, SchemaError> {
    match map.remove(name) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(SchemaError::WrongType(name)),
    }
}

fn required(map: &mut Map<String, Value>, name: &'static str) -> Result<String, SchemaError> {
    take_string(map, name)?.ok_or(SchemaError::MissingAttribute(name))
}

fn optional_text(map: &mut Map<String, Value>, name: &'static str) -> Result<Option<String>, SchemaError> {
    let v = take_string(map, name)?;
    if let Some(s) = &v {
        check_text(name, s, URI_MAX_CHARS)?;
    }
    Ok(v)
}

/// Parse an RFC 3339 `date-time` (RFC 3339 section 5.6). The jiff `Timestamp` parser is wider:
/// it also accepts a time with no seconds (`12:30Z`) and a bracketed annotation (`Z[UTC]`). So the
/// shape is checked first: `YYYY-MM-DD`, `T` (or `t`, or a space, which RFC 3339 allows),
/// `HH:MM:SS`, an optional `.` and one or more digits, then `Z`, `z` or `+HH:MM` / `-HH:MM`.
fn parse_rfc3339(s: &str) -> Result<Timestamp, SchemaError> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let mut ok = b.len() >= 20
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && matches!(b[10], b'T' | b't' | b' ')
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    let mut i = 19;
    if ok && b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        ok = i > start;
    }
    ok = ok
        && match b.get(i) {
            Some(b'Z' | b'z') => i + 1 == b.len(),
            Some(b'+' | b'-') => i + 6 == b.len() && digits(i + 1..i + 3) && b[i + 3] == b':' && digits(i + 4..i + 6),
            _ => false,
        };
    if !ok {
        return Err(SchemaError::InvalidTime);
    }
    s.parse::<Timestamp>().map_err(|_| SchemaError::InvalidTime)
}

/// Refuse a `data` value that holds U+0000 in an object key or a string, at any depth. Postgres
/// cannot store U+0000 in `jsonb`, so such an event can never be inserted: over HTTP it would be a
/// `503` on every retry, and over NATS (no delivery limit) a redelivery with no end. So it is a
/// malformed event. Other control characters stay valid in `data`: JSON and `jsonb` allow them.
fn check_data_has_no_nul(value: &Value) -> Result<(), SchemaError> {
    let bad = || SchemaError::InvalidCharacter("data".to_string());
    match value {
        Value::String(s) if s.contains('\0') => Err(bad()),
        Value::Array(items) => items.iter().try_for_each(check_data_has_no_nul),
        Value::Object(members) => members.iter().try_for_each(|(k, v)| if k.contains('\0') { Err(bad()) } else { check_data_has_no_nul(v) }),
        _ => Ok(()),
    }
}

fn from_map(map: &mut Map<String, Value>) -> Result<CloudEvent, SchemaError> {
    match map.remove("specversion") {
        None => return Err(SchemaError::MissingAttribute("specversion")),
        Some(Value::String(s)) if s == "1.0" => {}
        Some(Value::String(_)) => return Err(SchemaError::SpecVersion),
        Some(_) => return Err(SchemaError::WrongType("specversion")),
    }
    let id = required(map, "id")?;
    let source = required(map, "source")?;
    let ty = required(map, "type")?;
    check_text("id", &id, ID_MAX_CHARS)?;
    check_text("source", &source, URI_MAX_CHARS)?;
    check_text("type", &ty, TYPE_MAX_CHARS)?;

    let subject = optional_text(map, "subject")?;
    let datacontenttype = optional_text(map, "datacontenttype")?;
    let dataschema = optional_text(map, "dataschema")?;
    if let Some(s) = &dataschema {
        check_absolute_uri("dataschema", s)?;
    }
    let time = match take_string(map, "time")? {
        None => None,
        Some(s) => Some(parse_rfc3339(&s)?),
    };
    let data = map.remove("data");
    if let Some(d) = &data {
        check_data_has_no_nul(d)?;
    }
    if map.contains_key("data_base64") {
        return Err(SchemaError::DataBase64NotSupported);
    }

    let mut extensions = Vec::new();
    for (name, value) in std::mem::take(map) {
        check_extension_name(&name)?;
        let value = match value {
            Value::String(s) => ExtensionValue::String(s),
            Value::Bool(b) => ExtensionValue::Boolean(b),
            Value::Number(n) => ExtensionValue::Integer(n.as_i64().ok_or(SchemaError::WrongType("extension"))?),
            _ => return Err(SchemaError::WrongType("extension")),
        };
        check_extension_value(&name, &value)?;
        extensions.push((name, value));
    }
    Ok(CloudEvent {
        id,
        source,
        ty,
        time,
        subject,
        datacontenttype,
        dataschema,
        data,
        extensions,
    })
}

#[cfg(test)]
mod tests {
    use crate::{ExtensionValue, ParseError, SchemaError, parse};
    use serde_json::{Value, json};

    fn minimal() -> Value {
        json!({"specversion": "1.0", "id": "a", "source": "s", "type": "t"})
    }

    fn parse_value(v: &Value) -> Result<crate::CloudEvent, ParseError> {
        parse(&serde_json::to_vec(v).unwrap())
    }

    fn with(key: &str, value: Value) -> Value {
        let mut v = minimal();
        v[key] = value;
        v
    }

    #[test]
    fn parses_a_minimal_event() {
        let e = parse_value(&minimal()).unwrap();
        assert_eq!((e.id(), e.source(), e.ty()), ("a", "s", "t"));
        assert!(e.data().is_none());
        assert!(e.time().is_none());
    }

    #[test]
    fn rejects_a_missing_required_attribute() {
        for name in ["specversion", "id", "source", "type"] {
            let mut v = minimal();
            v.as_object_mut().unwrap().remove(name);
            match parse_value(&v) {
                Err(ParseError::Schema(SchemaError::MissingAttribute(n))) => assert_eq!(n, name),
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_spec_version_0_3() {
        let r = parse_value(&with("specversion", json!("0.3")));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::SpecVersion))), "{r:?}");
    }

    #[test]
    fn rejects_data_base64() {
        let r = parse_value(&with("data_base64", json!("AAAA")));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::DataBase64NotSupported))), "{r:?}");
    }

    #[test]
    fn invalid_json_is_a_json_error() {
        assert!(matches!(parse(b"{"), Err(ParseError::Json(_))));
        assert!(matches!(parse(b"[]"), Err(ParseError::Schema(_))));
    }

    #[test]
    fn a_null_attribute_is_absent() {
        let mut v = with("subject", Value::Null);
        v["data"] = Value::Null;
        v["time"] = Value::Null;
        v["schemaversion"] = Value::Null;
        let e = parse_value(&v).unwrap();
        assert!(e.subject().is_none());
        assert!(e.data().is_none());
        assert!(e.time().is_none());
        assert!(e.extension("schemaversion").is_none());
    }

    #[test]
    fn extensions_are_typed() {
        let mut v = with("schemaversion", json!(1));
        v["flag"] = json!(true);
        v["threadid"] = json!(7);
        v["actorprn"] = json!("prn:x");
        let e = parse_value(&v).unwrap();
        assert_eq!(e.extension("schemaversion"), Some(&ExtensionValue::Integer(1)));
        assert_eq!(e.schemaversion(), Some(1));
        assert_eq!(e.extension("flag"), Some(&ExtensionValue::Boolean(true)));
        assert_eq!(e.threadid(), None);
        assert_eq!(e.actorprn(), Some("prn:x"));
        assert_eq!(e.correlationid(), None);
    }

    #[test]
    fn an_extension_name_must_be_lowercase_alnum_up_to_20() {
        for bad in ["Bad", "a_b", &"a".repeat(21)] {
            let r = parse_value(&with(bad, json!("v")));
            assert!(matches!(r, Err(ParseError::Schema(SchemaError::InvalidExtensionName(_)))), "{bad}: {r:?}");
        }
        parse_value(&with(&"a".repeat(20), json!("v"))).unwrap();
    }

    #[test]
    fn an_object_extension_value_is_rejected() {
        let r = parse_value(&with("ext", json!({"a": 1})));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::WrongType(_)))), "{r:?}");
    }

    #[test]
    fn length_limits_count_characters() {
        let cases: [(&str, usize); 6] = [("id", 256), ("type", 256), ("source", 1024), ("subject", 1024), ("dataschema", 1024), ("ext", 1024)];
        for (attr, max) in cases {
            // `dataschema` must be an absolute URI, so its value starts with a two-character
            // scheme prefix. The total is still `max` (and `max + 1`) characters.
            let value = |n: usize| if attr == "dataschema" { format!("x:{}", "é".repeat(n - 2)) } else { "é".repeat(n) };
            let ok = parse_value(&with(attr, json!(value(max))));
            assert!(ok.is_ok(), "{attr} at {max}: {ok:?}");
            let bad = parse_value(&with(attr, json!(value(max + 1))));
            assert!(matches!(bad, Err(ParseError::Schema(SchemaError::TooLong { .. }))), "{attr}: {bad:?}");
        }
    }

    #[test]
    fn time_must_be_rfc3339() {
        let r = parse_value(&with("time", json!("yesterday")));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::InvalidTime))), "{r:?}");
        let e = parse_value(&with("time", json!("2026-08-07T12:30:45.5Z"))).unwrap();
        assert!(e.time().is_some());
        // jiff accepts these two forms, and RFC 3339 does not: a time with no seconds, and a
        // bracketed annotation.
        for bad in ["2026-08-07T12:30Z", "2026-08-07T12:30:45Z[UTC]", "2026-08-07T12:30:45.Z", "2026-08-07T12:30:45+0100", "2026-08-07"] {
            let r = parse_value(&with("time", json!(bad)));
            assert!(matches!(r, Err(ParseError::Schema(SchemaError::InvalidTime))), "{bad}: {r:?}");
        }
        for good in ["2026-08-07T12:30:45Z", "2026-08-07t12:30:45z", "2026-08-07T12:30:45.123456+01:00", "2026-08-07 12:30:45-05:30"] {
            assert!(parse_value(&with("time", json!(good))).is_ok(), "{good}");
        }
    }

    #[test]
    fn dataschema_must_be_an_absolute_uri() {
        let r = parse_value(&with("dataschema", json!("schemas/order.json")));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::NotAbsoluteUri("dataschema")))), "{r:?}");
        let e = parse_value(&with("dataschema", json!("https://example.com/order.json"))).unwrap();
        assert_eq!(e.dataschema(), Some("https://example.com/order.json"));
        // `source` is a URI-reference: a relative value stays valid.
        assert!(parse_value(&with("source", json!("/relative/source"))).is_ok());
    }

    #[test]
    fn dataschema_brackets_and_fragments_follow_the_uri_components() {
        for bad in ["https://example.com/[schema]", "https://example.com/p?q=[1]", "https://example.com/p#[f]", "urn:x:a#b#c", "urn:x:[a]"] {
            let r = parse_value(&with("dataschema", json!(bad)));
            assert!(matches!(r, Err(ParseError::Schema(SchemaError::NotAbsoluteUri("dataschema")))), "{bad}: {r:?}");
        }
        for good in ["https://example.com/order.json", "https://[::1]:8443/s.json?v=1#f", "urn:paigasus:schema:order#v1"] {
            let e = parse_value(&with("dataschema", json!(good))).unwrap_or_else(|r| panic!("{good}: {r:?}"));
            assert_eq!(e.dataschema(), Some(good));
        }
    }

    #[test]
    fn a_nul_in_data_is_refused_at_any_depth() {
        let nul = |data: Value| parse_value(&with("data", data));
        let refused = |r: Result<crate::CloudEvent, ParseError>| matches!(r, Err(ParseError::Schema(SchemaError::InvalidCharacter(ref a))) if a == "data");
        // A nested string value, a nested key, a string in an array, and a top-level string.
        assert!(refused(nul(json!({"a": {"b": "x\u{0}y"}}))));
        assert!(refused(nul(json!({"a": {"k\u{0}": 1}}))));
        assert!(refused(nul(json!({"a": [1, ["ok", "\u{0}"]]}))));
        assert!(refused(nul(json!("\u{0}"))));
        // The same shapes with no NUL are valid, and other control characters stay valid in data.
        assert!(nul(json!({"a": {"b": "x\u{1}y", "k": [1, "ok"]}})).is_ok());
    }

    #[test]
    fn a_control_character_in_a_context_string_is_refused() {
        for (key, value) in [("id", json!("a\u{0}")), ("subject", json!("x\ny")), ("correlationid", json!("c\u{7f}")), ("type", json!("t\u{fffe}"))] {
            let r = parse_value(&with(key, value));
            assert!(matches!(r, Err(ParseError::Schema(SchemaError::InvalidCharacter(ref a))) if a == key), "{key}: {r:?}");
        }
        // The rule is for context strings only: `data` keeps any JSON string.
        let e = parse_value(&with("data", json!({"text": "a\u{1}b"}))).unwrap();
        assert!(e.data().is_some());
        // A normal id still passes.
        assert!(parse_value(&with("id", json!("order-42/é"))).is_ok());
    }
}
