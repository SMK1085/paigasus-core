// SPDX-License-Identifier: Apache-2.0

//! JSON Schema to Cedar record type mapping (spec section 4.3).

use serde_json::{Map, Value, json};

use super::model::DroppedField;

/// Why a schema cannot be mapped at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MappingError {
    #[error("the input schema nests deeper than the limit")]
    TooDeep,
    #[error("the input schema is not an object schema")]
    NotAnObject,
}

/// A mapped type, and whether the schema allowed `null` (so the attribute is optional).
struct Mapped {
    ty: Value,
    nullable: bool,
}

/// Map the input schema of a function tool to a Cedar JSON record type.
///
/// The top level must be an object schema. The top-level record is level 1. Each nested
/// object or array adds one level. A schema over `max_depth` levels is `TooDeep`.
///
/// A top-level object with `additionalProperties` and no `properties` maps to an empty
/// record with no `dropped_fields` entry, because the top level has no path to report.
/// A nested one is dropped with a reason.
pub fn map_input_schema(schema: &Value, max_depth: usize) -> Result<(Value, Vec<DroppedField>), MappingError> {
    let obj = schema.as_object().ok_or(MappingError::NotAnObject)?;
    match obj.get("type") {
        Some(Value::String(t)) if t == "object" => {}
        None if obj.contains_key("properties") => {}
        _ => return Err(MappingError::NotAnObject),
    }
    let mut dropped = Vec::new();
    let record = map_record(obj, 1, max_depth, "", &mut dropped)?;
    Ok((record, dropped))
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_owned() } else { format!("{path}.{key}") }
}

fn map_record(obj: &Map<String, Value>, depth: usize, max_depth: usize, path: &str, dropped: &mut Vec<DroppedField>) -> Result<Value, MappingError> {
    if depth > max_depth {
        return Err(MappingError::TooDeep);
    }
    let required: Vec<&str> = obj.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut attributes = Map::new();
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (key, sub) in props {
            let p = join(path, key);
            if let Some(m) = map_type(sub, depth, max_depth, &p, dropped)? {
                let is_required = required.contains(&key.as_str()) && !m.nullable;
                let mut attr = m.ty;
                if let Some(a) = attr.as_object_mut() {
                    a.insert("required".into(), Value::Bool(is_required));
                }
                attributes.insert(key.clone(), attr);
            }
        }
    }
    Ok(json!({ "type": "Record", "attributes": Value::Object(attributes) }))
}

fn drop_field(path: &str, reason: &str, dropped: &mut Vec<DroppedField>) -> Option<Mapped> {
    dropped.push(DroppedField {
        path: path.to_owned(),
        reason: reason.to_owned(),
    });
    None
}

fn map_type(schema: &Value, depth: usize, max_depth: usize, path: &str, dropped: &mut Vec<DroppedField>) -> Result<Option<Mapped>, MappingError> {
    let Some(obj) = schema.as_object() else {
        return Ok(drop_field(path, "the schema is not an object", dropped));
    };
    if obj.contains_key("$ref") {
        return Ok(drop_field(path, "$ref is not mapped", dropped));
    }
    for kw in ["oneOf", "allOf"] {
        if obj.contains_key(kw) {
            return Ok(drop_field(path, &format!("{kw} is not mapped"), dropped));
        }
    }
    if let Some(any) = obj.get("anyOf") {
        return nullable_any_of(any, depth, max_depth, path, dropped);
    }
    match obj.get("type") {
        Some(Value::String(t)) => map_named(t, obj, depth, max_depth, path, dropped, false),
        Some(Value::Array(list)) => {
            let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
            if names.len() == 2 && list.len() == 2 && names.contains(&"null") {
                let other = names.iter().find(|n| **n != "null").copied().unwrap_or("null");
                map_named(other, obj, depth, max_depth, path, dropped, true)
            } else {
                Ok(drop_field(path, "this type list is not mapped", dropped))
            }
        }
        _ => Ok(drop_field(path, "the schema has no type", dropped)),
    }
}

fn nullable_any_of(any: &Value, depth: usize, max_depth: usize, path: &str, dropped: &mut Vec<DroppedField>) -> Result<Option<Mapped>, MappingError> {
    let Some(list) = any.as_array() else {
        return Ok(drop_field(path, "anyOf is not mapped", dropped));
    };
    let is_null = |v: &Value| v.get("type").and_then(Value::as_str) == Some("null");
    if list.len() == 2
        && list.iter().filter(|v| is_null(v)).count() == 1
        && let Some(other) = list.iter().find(|v| !is_null(v))
    {
        return Ok(map_type(other, depth, max_depth, path, dropped)?.map(|m| Mapped { ty: m.ty, nullable: true }));
    }
    Ok(drop_field(path, "this anyOf is not mapped", dropped))
}

fn map_named(t: &str, obj: &Map<String, Value>, depth: usize, max_depth: usize, path: &str, dropped: &mut Vec<DroppedField>, nullable: bool) -> Result<Option<Mapped>, MappingError> {
    let ty = match t {
        "string" => json!({ "type": "String" }),
        "integer" => json!({ "type": "Long" }),
        "boolean" => json!({ "type": "Bool" }),
        "number" => return Ok(drop_field(path, "number is not mapped", dropped)),
        "null" => return Ok(drop_field(path, "null alone is not mapped", dropped)),
        "array" => {
            if depth + 1 > max_depth {
                return Err(MappingError::TooDeep);
            }
            let Some(items) = obj.get("items") else {
                return Ok(drop_field(path, "array has no items", dropped));
            };
            // An element that does not map drops the whole array attribute.
            let mut inner = Vec::new();
            let Some(m) = map_type(items, depth + 1, max_depth, path, &mut inner)? else {
                let reason = match inner.first() {
                    Some(d) => format!("array items are not mapped: {}", d.reason),
                    None => "array items are not mapped".to_owned(),
                };
                return Ok(drop_field(path, &reason, dropped));
            };
            // A Cedar set cannot hold null, so an element that allows null has no form.
            if m.nullable {
                return Ok(drop_field(path, "array items that allow null are not mapped", dropped));
            }
            dropped.append(&mut inner);
            json!({ "type": "Set", "element": m.ty })
        }
        "object" => {
            if !obj.contains_key("properties") && obj.contains_key("additionalProperties") {
                return Ok(drop_field(path, "additionalProperties without properties is not mapped", dropped));
            }
            map_record(obj, depth + 1, max_depth, path, dropped)?
        }
        other => return Ok(drop_field(path, &format!("type {other} is not mapped"), dropped)),
    };
    Ok(Some(Mapped { ty, nullable }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(schema: Value) -> (Value, Vec<DroppedField>) {
        map_input_schema(&schema, 8).unwrap()
    }

    fn attrs(record: &Value) -> &Map<String, Value> {
        record["attributes"].as_object().unwrap()
    }

    #[test]
    fn every_mapped_form_maps() {
        let (rec, dropped) = map(json!({
            "type": "object",
            "properties": {
                "s": {"type": "string"},
                "d": {"type": "string", "format": "date-time"},
                "i": {"type": "integer"},
                "b": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "n": {"type": "object", "properties": {"x": {"type": "integer"}}, "required": ["x"]}
            },
            "required": ["s", "tags"]
        }));
        assert!(dropped.is_empty());
        let a = attrs(&rec);
        assert_eq!(a["s"], json!({"type": "String", "required": true}));
        assert_eq!(a["d"], json!({"type": "String", "required": false}));
        assert_eq!(a["i"], json!({"type": "Long", "required": false}));
        assert_eq!(a["b"], json!({"type": "Bool", "required": false}));
        assert_eq!(a["tags"], json!({"type": "Set", "element": {"type": "String"}, "required": true}));
        assert_eq!(a["n"]["type"], "Record");
        assert_eq!(a["n"]["required"], false);
        assert_eq!(a["n"]["attributes"]["x"], json!({"type": "Long", "required": true}));
    }

    #[test]
    fn nullable_forms_become_optional() {
        let (rec, dropped) = map(json!({
            "type": "object",
            "properties": {
                "a": {"type": ["string", "null"]},
                "b": {"anyOf": [{"type": "string"}, {"type": "null"}]}
            },
            "required": ["a", "b"]
        }));
        assert!(dropped.is_empty());
        assert_eq!(attrs(&rec)["a"], json!({"type": "String", "required": false}));
        assert_eq!(attrs(&rec)["b"], json!({"type": "String", "required": false}));
    }

    #[test]
    fn every_unmapped_form_is_dropped_with_a_reason() {
        let (rec, dropped) = map(json!({
            "type": "object",
            "properties": {
                "keep": {"type": "string"},
                "num": {"type": "number"},
                "one": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "all": {"allOf": [{"type": "string"}]},
                "any": {"anyOf": [{"type": "string"}, {"type": "integer"}]},
                "ref": {"$ref": "#/$defs/x"},
                "map": {"type": "object", "additionalProperties": {"type": "string"}},
                "nul": {"type": "null"},
                "mix": {"type": ["string", "integer"]},
                "nested": {"type": "object", "properties": {"deep": {"type": "number"}}}
            }
        }));
        let a = attrs(&rec);
        let mut keys: Vec<&str> = a.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["keep", "nested"]);
        assert_eq!(attrs(&a["nested"]).len(), 0);
        let mut paths: Vec<&str> = dropped.iter().map(|d| d.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["all", "any", "map", "mix", "nested.deep", "nul", "num", "one", "ref"]);
        assert!(dropped.iter().all(|d| !d.reason.is_empty()));
    }

    #[test]
    fn an_array_of_unmapped_items_is_dropped() {
        let (rec, dropped) = map(json!({"type": "object", "properties": {"xs": {"type": "array", "items": {"type": "number"}}}}));
        assert!(attrs(&rec).is_empty());
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].path, "xs");
    }

    #[test]
    fn an_array_of_nullable_items_is_dropped() {
        let (rec, dropped) = map(json!({
            "type": "object",
            "properties": {
                "a": {"type": "array", "items": {"type": ["integer", "null"]}},
                "b": {"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "null"}]}},
                "keep": {"type": ["array", "null"], "items": {"type": "integer"}}
            }
        }));
        let a = attrs(&rec);
        assert_eq!(a.keys().map(String::as_str).collect::<Vec<_>>(), ["keep"]);
        assert_eq!(a["keep"], json!({"type": "Set", "element": {"type": "Long"}, "required": false}));
        let mut paths: Vec<&str> = dropped.iter().map(|d| d.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["a", "b"]);
        assert!(dropped.iter().all(|d| d.reason == "array items that allow null are not mapped"));
    }

    #[test]
    fn an_empty_nested_record_stays() {
        let (rec, dropped) = map(json!({"type": "object", "properties": {"e": {"type": "object", "properties": {}}}}));
        assert!(dropped.is_empty());
        assert_eq!(attrs(&rec)["e"], json!({"type": "Record", "attributes": {}, "required": false}));
    }

    #[test]
    fn a_nested_bare_object_is_an_empty_record_with_no_drop() {
        let (rec, dropped) = map(json!({"type": "object", "properties": {"o": {"type": "object"}}}));
        assert!(dropped.is_empty());
        assert_eq!(attrs(&rec)["o"], json!({"type": "Record", "attributes": {}, "required": false}));
    }

    #[test]
    fn a_top_level_additional_properties_only_schema_is_an_empty_record() {
        let (rec, dropped) = map(json!({"type": "object", "additionalProperties": {"type": "string"}}));
        assert!(dropped.is_empty());
        assert!(attrs(&rec).is_empty());
    }

    #[test]
    fn a_non_object_top_level_is_rejected() {
        assert_eq!(map_input_schema(&json!({"type": "string"}), 8).unwrap_err(), MappingError::NotAnObject);
        assert_eq!(map_input_schema(&json!([1]), 8).unwrap_err(), MappingError::NotAnObject);
    }

    fn nested(levels: usize) -> Value {
        let mut s = json!({"type": "object", "properties": {"leaf": {"type": "string"}}});
        for _ in 1..levels {
            s = json!({"type": "object", "properties": {"n": s}});
        }
        s
    }

    #[test]
    fn depth_over_eight_is_too_deep() {
        assert!(map_input_schema(&nested(8), 8).is_ok());
        assert_eq!(map_input_schema(&nested(9), 8).unwrap_err(), MappingError::TooDeep);
    }
}
