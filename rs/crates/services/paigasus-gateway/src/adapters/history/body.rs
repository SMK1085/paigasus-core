// SPDX-License-Identifier: Apache-2.0

//! The JSON body of a history event (SMA-753 spec P3), shared by both store adapters.
//!
//! `{"v":1,"id":"<uuid>","ts":<ms>,"k":"request|response","t":"<tool>"` and then the stored
//! fields: `"f":{path: value}` (and `"kid":"<hex>"` when a digest was made), or `"b":1` for
//! unparsed arguments. A `Long` is a JSON number, a digest is base64url with no padding, `Absent`
//! is `null`, and `Mistyped` is `{"x":1}`. A reader ignores unknown members, and a body with no
//! `f` and no `b` (a PR 1 event) reads as no fields.
//!
//! The stored fields are always the LAST members of the body, after `t`. The Redis write script
//! copies them from a call body into its result body by text (`,"f":` or `,"b":` up to the end), so
//! it never parses a number: Lua reads a JSON number as a double, which cannot hold every `i64`.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use paigasus_kernel::tool_policy::history::{Digest, Field, FieldPath, Fields, KeyId};
use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::domain::limits::UnavailableKind;
use crate::domain::tool_history::{EventKind, HistoryStoreError};

/// The version of the event body encoding. A reader ignores unknown fields (rolling upgrade).
pub const BODY_VERSION: u64 = 1;

/// A decoded body: everything of a `StoredEvent` but the verdict, which the store keeps apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBody {
    pub id: Uuid,
    pub ts_ms: u64,
    pub kind: EventKind,
    pub tool: String,
    pub fields: Fields,
    pub key: Option<KeyId>,
}

/// The body of an event. `stored` is `None` for a body whose fields the store copies from another
/// body (a result event in Redis): then the body ends after `t`.
///
/// Keep `f`/`b` and then `kid` the LAST members: `copy_fields` in `APPEND_SCRIPT` (`redis.rs`)
/// copies everything from `,"f":` or `,"b":` up to the closing brace into the result body. A
/// member added after them would be copied too; `the_stored_fields_are_the_last_members` reds.
pub fn encode_body(id: Uuid, ts_ms: u64, kind: EventKind, tool: &str, stored: Option<(&Fields, Option<KeyId>)>) -> String {
    let mut out = format!(r#"{}{id}","ts":{ts_ms},"k":"{}","t":{}"#, id_prefix(), kind.as_str(), Value::String(tool.to_owned()));
    if let Some((fields, key)) = stored {
        match fields {
            Fields::Values(map) => {
                let f: Map<String, Value> = map.iter().map(|(path, field)| (path.joined(), encode_field(field))).collect();
                out.push_str(r#","f":"#);
                out.push_str(&Value::Object(f).to_string());
            }
            // `Unparsed`, and a later variant: fail closed, as no values.
            _ => out.push_str(r#","b":1"#),
        }
        if let Some(key) = key {
            out.push_str(&format!(r#","kid":"{}""#, key.to_hex()));
        }
    }
    out.push('}');
    out
}

/// The text of every body before its id: `{"v":1,"id":"`.
fn id_prefix() -> String {
    format!(r#"{{"v":{BODY_VERSION},"id":""#)
}

/// The byte offset of the id in every body that `encode_body` writes. The id is the 36-character
/// hyphenated text of the uuid. The Redis window read compares these 36 bytes with the member.
pub fn id_offset() -> usize {
    id_prefix().len()
}

fn encode_field(field: &Field) -> Value {
    match field {
        Field::Long(n) => Value::from(*n),
        Field::Digest(d) => Value::String(URL_SAFE_NO_PAD.encode(d.0)),
        Field::Absent => Value::Null,
        // `Mistyped`, and a later variant: a value that matches no literal.
        _ => serde_json::json!({ "x": 1 }),
    }
}

/// The JSON body. serde ignores unknown keys, so a body of a later version still decodes.
#[derive(Deserialize)]
struct Body {
    v: u64,
    id: String,
    ts: u64,
    k: String,
    t: String,
    #[serde(default)]
    f: Option<Map<String, Value>>,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    b: Option<u64>,
}

pub fn decode_error(detail: impl Into<String>) -> HistoryStoreError {
    HistoryStoreError::Unavailable {
        kind: UnavailableKind::Decode,
        detail: detail.into(),
    }
}

/// Decode a body. An error never holds a stored value: it names the member only.
///
/// # Errors
///
/// A body that is not valid JSON, that has an older version, an unknown kind, a bad id, a bad path,
/// a bad key id or a field value of an unknown shape.
pub fn decode_body(raw: &str) -> Result<DecodedBody, HistoryStoreError> {
    let body: Body = serde_json::from_str(raw).map_err(|_| decode_error("event body is not valid JSON"))?;
    if body.v < BODY_VERSION {
        return Err(decode_error(format!("event body has version {}", body.v)));
    }
    let id = Uuid::try_parse(&body.id).map_err(|_| decode_error("event body id is not a uuid"))?;
    let kind = match body.k.as_str() {
        "request" => EventKind::Call,
        "response" => EventKind::Result,
        _ => return Err(decode_error("event body has an unknown kind")),
    };
    let fields = if body.b.is_some_and(|b| b != 0) {
        Fields::Unparsed
    } else {
        let mut map = BTreeMap::new();
        for (path, value) in body.f.unwrap_or_default() {
            let path = FieldPath::new(path.split('.').map(str::to_owned).collect()).map_err(|_| decode_error("event body has a bad field path"))?;
            map.insert(path, decode_field(&value)?);
        }
        Fields::Values(map)
    };
    let key = body.kid.map(|k| KeyId::from_hex(&k).ok_or_else(|| decode_error("event body has a bad key id"))).transpose()?;
    Ok(DecodedBody {
        id,
        ts_ms: body.ts,
        kind,
        tool: body.t,
        fields,
        key,
    })
}

fn decode_field(value: &Value) -> Result<Field, HistoryStoreError> {
    match value {
        Value::Null => Ok(Field::Absent),
        Value::Number(n) => n.as_i64().map(Field::Long).ok_or_else(|| decode_error("event body has a field number that is not an i64")),
        Value::String(s) => {
            let bytes = URL_SAFE_NO_PAD.decode(s).map_err(|_| decode_error("event body has a field digest that is not base64url"))?;
            let bytes: [u8; 16] = bytes.try_into().map_err(|_| decode_error("event body has a field digest that is not 16 bytes"))?;
            Ok(Field::Digest(Digest::new(bytes)))
        }
        Value::Object(m) if m.len() == 1 && m.get("x") == Some(&Value::from(1)) => Ok(Field::Mistyped),
        _ => Err(decode_error("event body has a field value of an unknown shape")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> FieldPath {
        FieldPath::new(text.split('.').map(str::to_owned).collect()).expect("a path")
    }

    fn sample() -> Fields {
        let mut m = BTreeMap::new();
        m.insert(path("a.b"), Field::Long(i64::MIN));
        m.insert(path("d"), Field::Digest(Digest::new([0xff; 16])));
        m.insert(path("n"), Field::Absent);
        m.insert(path("x"), Field::Mistyped);
        Fields::Values(m)
    }

    #[test]
    fn the_encoding_has_the_stated_members() {
        let id = Uuid::from_u128(9);
        let key = KeyId::new([0xab; 8]);
        let body = encode_body(id, 42, EventKind::Call, "send", Some((&sample(), Some(key))));
        assert_eq!(
            body,
            format!(r#"{{"v":1,"id":"{id}","ts":42,"k":"request","t":"send","f":{{"a.b":-9223372036854775808,"d":"_____________________w","n":null,"x":{{"x":1}}}},"kid":"abababababababab"}}"#)
        );
        let unparsed = encode_body(id, 42, EventKind::Call, "send", Some((&Fields::Unparsed, None)));
        assert!(unparsed.ends_with(r#""t":"send","b":1}"#), "{unparsed}");
        let head = encode_body(id, 42, EventKind::Result, "send", None);
        assert!(head.ends_with(r#""t":"send"}"#), "{head}");
    }

    /// The Redis window read checks the member against these bytes of the body.
    #[test]
    fn the_id_is_at_the_id_offset_of_every_body() {
        let id = Uuid::from_u128(0x0190_a1e5_0000_7000_8000_0000_0000_0042);
        let at = id_offset();
        assert_eq!(at, r#"{"v":1,"id":""#.len());
        for stored in [Some((&sample(), Some(KeyId::new([1; 8])))), Some((&Fields::Unparsed, None)), None] {
            let body = encode_body(id, 7, EventKind::Call, r#"x","id":"y"#, stored);
            assert_eq!(&body[at..at + 36], id.to_string(), "{body}");
        }
    }

    #[test]
    fn a_body_round_trips() {
        let id = Uuid::from_u128(9);
        let key = Some(KeyId::new([1, 2, 3, 4, 5, 6, 7, 8]));
        for (fields, key) in [(sample(), key), (Fields::Unparsed, None), (Fields::Values(BTreeMap::new()), None)] {
            let decoded = decode_body(&encode_body(id, 7, EventKind::Call, "a\"b", Some((&fields, key)))).expect("decodes");
            assert_eq!(
                decoded,
                DecodedBody {
                    id,
                    ts_ms: 7,
                    kind: EventKind::Call,
                    tool: "a\"b".into(),
                    fields,
                    key
                }
            );
        }
    }

    /// The Redis write script finds the stored fields by text. A tool name cannot fake the start.
    #[test]
    fn a_tool_name_cannot_hold_the_text_of_the_fields_member() {
        let body = encode_body(Uuid::from_u128(9), 7, EventKind::Call, r#"x","f":{"y":1}"#, Some((&Fields::Unparsed, None)));
        assert!(!body.contains(r#","f":"#), "{body}");
        assert_eq!(body.find(r#","b":"#), Some(body.len() - r#","b":1}"#.len()));
    }

    /// The Rust form of `copy_fields` in `redis.rs`: the text from the first `,"f":` (else `,"b":`)
    /// up to the closing brace.
    fn fragment(body: &str) -> &str {
        let at = body.find(r#","f":"#).or_else(|| body.find(r#","b":"#)).expect("a fields member");
        &body[at..body.len() - 1]
    }

    /// `copy_fields` relies on this: the stored fields, then `kid`, end the body. A member added
    /// after them changes the fragment.
    #[test]
    fn the_stored_fields_are_the_last_members() {
        let id = Uuid::from_u128(9);
        let key = KeyId::new([0xab; 8]);
        let tool = r#"x","f":{"amount":1}"#;
        let f_json = r#"{"a.b":-9223372036854775808,"d":"_____________________w","n":null,"x":{"x":1}}"#;
        let cases = [
            (sample(), Some(key), format!(r#","f":{f_json},"kid":"abababababababab""#)),
            (sample(), None, format!(r#","f":{f_json}"#)),
            (Fields::Unparsed, Some(key), r#","b":1,"kid":"abababababababab""#.to_owned()),
            (Fields::Unparsed, None, r#","b":1"#.to_owned()),
        ];
        for (fields, key, want) in cases {
            let call = encode_body(id, 7, EventKind::Call, tool, Some((&fields, key)));
            assert_eq!(fragment(&call), want);
            // The result: its head, the fragment, and the closing brace. It decodes to the same fields.
            let head = encode_body(Uuid::from_u128(10), 8, EventKind::Result, tool, None);
            let result = format!("{}{}}}", &head[..head.len() - 1], fragment(&call));
            let decoded = decode_body(&result).expect("decodes");
            assert_eq!((decoded.tool.as_str(), decoded.fields, decoded.key), (tool, fields, key));
        }
    }

    #[test]
    fn a_pr1_body_reads_as_no_fields() {
        let id = Uuid::from_u128(9);
        let decoded = decode_body(&format!(r#"{{"v":1,"id":"{id}","ts":42,"k":"response","t":"send","later":true}}"#)).expect("decodes");
        assert_eq!((decoded.fields, decoded.key), (Fields::Values(BTreeMap::new()), None));
    }

    #[test]
    fn a_bad_member_is_a_decode_error_that_names_no_value() {
        let id = Uuid::from_u128(9);
        let with = |extra: &str| format!(r#"{{"v":1,"id":"{id}","ts":42,"k":"request","t":"send",{extra}}}"#);
        for extra in [
            r#""f":{"a":1.5}"#,
            r#""f":{"a":"c2VjcmV0"}"#,
            r#""f":{"a":"!!"}"#,
            r#""f":{"a":true}"#,
            r#""f":{"a":{"x":2}}"#,
            r#""f":{"a..b":1}"#,
            r#""kid":"ABABABABABABABAB""#,
        ] {
            let err = decode_body(&with(extra)).expect_err(extra);
            let HistoryStoreError::Unavailable { kind, detail } = err;
            assert_eq!(kind, UnavailableKind::Decode);
            assert!(!detail.contains("c2VjcmV0") && !detail.contains("1.5"), "{detail}");
        }
    }
}
