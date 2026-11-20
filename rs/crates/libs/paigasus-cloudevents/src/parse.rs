// SPDX-License-Identifier: Apache-2.0

use jiff::Timestamp;
use serde_json::{Map, Value};

use crate::event::{ID_MAX_CHARS, TYPE_MAX_CHARS, URI_MAX_CHARS, check_extension_name, check_extension_value, check_text};
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
    let time = match take_string(map, "time")? {
        None => None,
        Some(s) => Some(s.parse::<Timestamp>().map_err(|_| SchemaError::InvalidTime)?),
    };
    let data = map.remove("data");
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
            let ok = parse_value(&with(attr, json!("é".repeat(max))));
            assert!(ok.is_ok(), "{attr} at {max}: {ok:?}");
            let bad = parse_value(&with(attr, json!("é".repeat(max + 1))));
            assert!(matches!(bad, Err(ParseError::Schema(SchemaError::TooLong { .. }))), "{attr}: {bad:?}");
        }
    }

    #[test]
    fn time_must_be_rfc3339() {
        let r = parse_value(&with("time", json!("yesterday")));
        assert!(matches!(r, Err(ParseError::Schema(SchemaError::InvalidTime))), "{r:?}");
        let e = parse_value(&with("time", json!("2026-08-07T12:30:45.5Z"))).unwrap();
        assert!(e.time().is_some());
    }
}
