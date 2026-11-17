// SPDX-License-Identifier: Apache-2.0

//! Cedar schema generation for tool policies (spec section 4.3).
//!
//! The names come from the SMA-752 spec (D20, D22) with the D5 correction: the principal is
//! `Pgs::Iam::Principal`, the resource is `Pgs::Iam::Organization`, and every call is an action
//! `Pgs::Gateway::Action::"<name_key>"`. The two groups `InvokeTool` and `Unregistered` apply
//! to themselves, so `action in InvokeTool` and `action in Unregistered` validate with an
//! empty catalog.

use serde_json::{Map, Value, json};

use super::model::{ToolEntry, ToolKind};

/// The revision of the generated schema and of the JSON Schema mapping (spec section 6.2).
/// Bump it when `tool_schema_is_pinned_to_the_declared_revision` fails.
pub const TOOL_SCHEMA_REVISION: u32 = 1;

/// The Cedar entity type of every tool action.
pub const ACTION_NAMESPACE: &str = "Pgs::Gateway::Action";

const INVOKE_TOOL: &str = "InvokeTool";
const UNREGISTERED: &str = "Unregistered";

fn empty_record() -> Value {
    json!({ "type": "Record", "attributes": {} })
}

fn required(mut ty: Value, is_required: bool) -> Value {
    if let Some(o) = ty.as_object_mut() {
        o.insert("required".into(), Value::Bool(is_required));
    }
    ty
}

/// The `appliesTo` block of one action, with its `input` type and whether `input_text` is required.
fn applies_to(input: Value, input_text_required: bool) -> Value {
    json!({
        "principalTypes": ["Pgs::Iam::Principal"],
        "resourceTypes": ["Pgs::Iam::Organization"],
        "context": {
            "type": "Record",
            "attributes": {
                "input": required(input, true),
                "input_text": { "type": "String", "required": input_text_required },
                "findings": { "type": "Set", "element": { "type": "String" }, "required": true },
                "call": { "type": "CallInfo", "required": true },
                "request": { "type": "RequestInfo", "required": true },
                "session": { "type": "SessionInfo", "required": true }
            }
        }
    })
}

/// The fixed base schema in the Cedar JSON schema form (spec section 4.3).
pub fn base_schema_json() -> Value {
    json!({
        "Pgs::Iam": {
            "entityTypes": {
                "Principal": {},
                "Organization": {}
            },
            "actions": {}
        },
        "Pgs::Gateway": {
            "commonTypes": {
                "RequestInfo": {
                    "type": "Record",
                    "attributes": {
                        "scope": { "type": "String", "required": true },
                        "ingress": { "type": "String", "required": true },
                        "model": { "type": "String", "required": true }
                    }
                },
                "SessionInfo": {
                    "type": "Record",
                    "attributes": {
                        "id": { "type": "String", "required": true },
                        "turn": { "type": "Long", "required": true }
                    }
                },
                "CallInfo": {
                    "type": "Record",
                    "attributes": {
                        "declared": { "type": "String", "required": true }
                    }
                }
            },
            "entityTypes": {},
            "actions": {
                (INVOKE_TOOL): { "appliesTo": applies_to(empty_record(), false) },
                (UNREGISTERED): { "appliesTo": applies_to(empty_record(), false) }
            }
        }
    })
}

/// The tenant schema: the base plus one action for each catalog entry, a member of `InvokeTool`.
///
/// The actions are added in `name_key` order. A `Function` tool has its mapped input type and
/// an optional `input_text`. A `Custom` tool has an empty `input` record and a required
/// `input_text`.
///
/// The function depends on unique `name_key` values: for two entries with the same key, the
/// later one replaces the earlier one. The catalog layer rejects duplicates (`CatalogProblem::Conflict`).
///
/// An entry must never replace a group. An entry whose `name_key` is a key of the base actions
/// (`InvokeTool`, `Unregistered`) or is not ASCII lowercase is left out, so a policy that names
/// it fails validation (fail closed). Such an entry cannot come from the catalog layer, so a
/// debug build also fails a `debug_assert!`.
pub fn tenant_schema_json(catalog: &[ToolEntry]) -> Value {
    tenant_schema_with(catalog, true)
}

fn tenant_schema_with(catalog: &[ToolEntry], assert_valid_keys: bool) -> Value {
    let mut schema = base_schema_json();
    let mut entries: Vec<&ToolEntry> = catalog.iter().collect();
    entries.sort_by(|a, b| a.name.name_key.cmp(&b.name.name_key));
    let actions = schema["Pgs::Gateway"]["actions"].as_object_mut().expect("the base schema has an actions map");
    let base_keys: Vec<String> = actions.keys().cloned().collect();
    for entry in entries {
        let key = &entry.name.name_key;
        if base_keys.contains(key) || *key != key.to_ascii_lowercase() {
            debug_assert!(
                !assert_valid_keys,
                "tool name_key {key:?} is a base action or is not ASCII lowercase; the catalog layer must reject it before schema generation"
            );
            continue;
        }
        let applies = match entry.kind {
            ToolKind::Function => applies_to(entry.cedar_input_type.clone(), false),
            ToolKind::Custom => applies_to(empty_record(), true),
        };
        let mut action = Map::new();
        action.insert("memberOf".into(), json!([{ "id": INVOKE_TOOL }]));
        action.insert("appliesTo".into(), applies);
        actions.insert(entry.name.name_key.clone(), Value::Object(action));
    }
    schema
}

/// The sorted `name_key` list of the catalog.
pub fn catalog_names(catalog: &[ToolEntry]) -> Vec<String> {
    let mut names: Vec<String> = catalog.iter().map(|e| e.name.name_key.clone()).collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_policy::{CatalogLimits, build_manual, map_input_schema};
    use cedar_policy::{PolicySet, Schema, ValidationMode, Validator};

    /// A copy of `v` with the object keys in sorted order at every level. The order of a
    /// `serde_json::Map` depends on the `preserve_order` feature, so this makes the output stable.
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&o[k]))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }

    /// Compact JSON with sorted keys.
    fn canonical_json(v: &Value) -> String {
        serde_json::to_string(&sorted(v)).expect("a JSON value serializes")
    }

    fn function_tool(name: &str, schema: Value) -> ToolEntry {
        build_manual(name, ToolKind::Function, Some(schema), &CatalogLimits::default()).unwrap()
    }

    fn custom_tool(name: &str) -> ToolEntry {
        build_manual(name, ToolKind::Custom, None, &CatalogLimits::default()).unwrap()
    }

    /// Validate `source` in strict mode. Return the error and warning texts.
    fn validate(source: &str, schema: Value) -> (Vec<String>, Vec<String>) {
        let schema = Schema::from_json_value(schema).expect("the schema parses");
        let policies: PolicySet = source.parse().expect("the policy parses");
        let result = Validator::new(schema).validate(&policies, ValidationMode::Strict);
        let errors = result.validation_errors().map(ToString::to_string).collect();
        let warnings = result.validation_warnings().map(ToString::to_string).collect();
        (errors, warnings)
    }

    fn assert_valid(source: &str, schema: Value) {
        let (errors, warnings) = validate(source, schema);
        assert!(errors.is_empty(), "validation errors: {errors:?}");
        assert!(warnings.is_empty(), "validation warnings: {warnings:?}");
    }

    #[test]
    fn the_base_schema_parses_under_cedar_4_14() {
        Schema::from_json_value(base_schema_json()).unwrap();
    }

    #[test]
    fn an_empty_catalog_gives_the_base_schema() {
        assert_eq!(tenant_schema_json(&[]), base_schema_json());
    }

    #[test]
    fn a_generic_floor_validates_against_the_base_schema() {
        let floor = "@id(\"floor-no-unregistered\")\n\
                     forbid (principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n\
                     @id(\"floor-name-collision\")\n\
                     forbid (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource)\n\
                     when { context.findings.contains(\"name_collision\") };\n";
        assert_valid(floor, base_schema_json());
    }

    #[test]
    fn a_floor_can_read_the_request_session_and_call_records() {
        let floor = "forbid (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource)\n\
                     when { context.request.scope == \"s\" && context.request.ingress == \"i\" && context.request.model == \"m\" \
                     && context.session.id == \"x\" && context.session.turn > 3 && context.call.declared == \"no\" };\n";
        assert_valid(floor, base_schema_json());
    }

    #[test]
    fn a_catalog_tool_is_a_member_of_invoke_tool() {
        let catalog = [function_tool("github.search", json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}))];
        let schema = tenant_schema_json(&catalog);
        assert_valid(
            "permit (principal, action == Pgs::Gateway::Action::\"github.search\", resource) when { context.input.q == \"x\" };",
            schema.clone(),
        );
        assert_eq!(schema["Pgs::Gateway"]["actions"]["github.search"]["memberOf"], json!([{"id": "InvokeTool"}]));
        assert_valid("permit (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);", schema);
    }

    #[test]
    fn an_unregistered_policy_cannot_read_input_fields() {
        let (errors, _) = validate(
            "permit (principal, action in Pgs::Gateway::Action::\"Unregistered\", resource) when { context.input.q == \"x\" };",
            base_schema_json(),
        );
        assert!(!errors.is_empty(), "a read of context.input.q under Unregistered must not validate");
    }

    #[test]
    fn an_unknown_action_does_not_validate() {
        let (errors, _) = validate("permit (principal, action == Pgs::Gateway::Action::\"github.search\", resource);", base_schema_json());
        assert!(!errors.is_empty(), "an action that is not in the schema must not validate");
    }

    #[test]
    fn a_custom_tool_requires_input_text() {
        let read_text = "permit (principal, action == Pgs::Gateway::Action::\"notes\", resource) when { context.input_text == \"a\" };";
        // A custom tool always has `input_text`, so a read with no `has` check validates.
        let schema = tenant_schema_json(&[custom_tool("notes")]);
        assert_eq!(schema["Pgs::Gateway"]["actions"]["notes"]["appliesTo"]["context"]["attributes"]["input"]["attributes"], json!({}));
        assert_valid(read_text, schema);
        // A function tool has an optional `input_text`, so the same read does not validate.
        let (errors, _) = validate(read_text, tenant_schema_json(&[function_tool("notes", json!({"type": "object", "properties": {}}))]));
        assert!(!errors.is_empty(), "a function tool must not have a required input_text");
    }

    #[test]
    fn catalog_names_are_sorted() {
        let catalog = [custom_tool("zeta"), function_tool("Alpha.Run", json!({"type": "object"})), custom_tool("mid")];
        assert_eq!(catalog_names(&catalog), vec!["alpha.run", "mid", "zeta"]);
    }

    #[test]
    fn tenant_actions_use_the_name_key() {
        let schema = tenant_schema_json(&[custom_tool("GitHub.Search")]);
        let actions = schema["Pgs::Gateway"]["actions"].as_object().unwrap();
        let mut ids: Vec<&String> = actions.keys().collect();
        ids.sort();
        assert_eq!(ids, ["InvokeTool", "Unregistered", "github.search"]);
    }

    #[test]
    fn the_action_namespace_matches_the_schema() {
        let namespace = ACTION_NAMESPACE.strip_suffix("::Action").unwrap();
        assert!(base_schema_json().get(namespace).is_some());
    }

    /// One JSON Schema for every row of the spec section 4.3 table.
    fn mapping_corpus() -> Vec<Value> {
        vec![
            json!({"type": "object", "properties": {
                "s": {"type": "string"},
                "f": {"type": "string", "format": "uri"},
                "i": {"type": "integer"},
                "b": {"type": "boolean"},
                "a": {"type": "array", "items": {"type": "integer"}},
                "o": {"type": "object", "properties": {"x": {"type": "string"}, "y": {"type": "boolean"}}, "required": ["x"]},
                "n1": {"type": ["string", "null"]},
                "n2": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                "num": {"type": "number"},
                "one": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "any": {"anyOf": [{"type": "string"}, {"type": "integer"}]},
                "all": {"allOf": [{"type": "string"}]},
                "ref": {"$ref": "#/$defs/x"},
                "map": {"type": "object", "additionalProperties": {"type": "string"}},
                "nul": {"type": "null"},
                "list": {"type": ["string", "integer"]},
                "empty": {"type": "object", "properties": {"z": {"type": "number"}}}
            }, "required": ["s", "a", "n1", "num"]}),
            json!({"type": "object", "additionalProperties": true}),
            json!({"properties": {"q": {"type": "string"}}}),
            nested_objects(8),
            nested_objects(9),
        ]
    }

    /// An object schema with `levels` nested object levels; the top level is level 1.
    fn nested_objects(levels: usize) -> Value {
        let mut schema = json!({"type": "object", "properties": {"leaf": {"type": "string"}}});
        for _ in 1..levels {
            schema = json!({"type": "object", "properties": {"n": schema}});
        }
        schema
    }

    #[test]
    fn the_depth_limit_accepts_8_levels_and_rejects_9() {
        let max = CatalogLimits::default().max_depth;
        assert_eq!(max, 8);
        assert!(map_input_schema(&nested_objects(8), max).is_ok());
        assert_eq!(map_input_schema(&nested_objects(9), max), Err(crate::tool_policy::MappingError::TooDeep));
    }

    /// The catalog of the pin test: one custom tool and one function tool for each corpus
    /// schema that maps.
    fn fixed_catalog() -> Vec<ToolEntry> {
        let mut catalog = vec![custom_tool("free_text")];
        for (i, schema) in mapping_corpus().into_iter().enumerate() {
            if let Ok(entry) = build_manual(&format!("pin.tool_{i}"), ToolKind::Function, Some(schema), &CatalogLimits::default()) {
                catalog.push(entry);
            }
        }
        catalog
    }

    /// An entry built by hand, with a `name_key` that the catalog layer never makes.
    fn entry_with_key(key: &str) -> ToolEntry {
        let mut entry = function_tool("placeholder", json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}));
        entry.name.name_key = key.to_owned();
        entry
    }

    fn assert_group_is_kept(key: &str) {
        let schema = tenant_schema_with(&[entry_with_key(key)], false);
        let base = base_schema_json();
        assert_eq!(schema["Pgs::Gateway"]["actions"][key], base["Pgs::Gateway"]["actions"][key], "the group {key} must keep its own entry");
        assert!(
            schema["Pgs::Gateway"]["actions"][key].get("memberOf").is_none(),
            "the group {key} must not become a member of InvokeTool"
        );
        assert_eq!(schema["Pgs::Gateway"]["actions"][key]["appliesTo"]["context"]["attributes"]["input"]["attributes"], json!({}));
        assert_eq!(schema, base, "an entry with the key {key} must be left out");
        Schema::from_json_value(schema).expect("the schema still parses");
    }

    #[test]
    fn an_entry_named_unregistered_does_not_replace_the_group() {
        assert_group_is_kept("Unregistered");
    }

    #[test]
    fn an_entry_named_invoke_tool_does_not_replace_the_group() {
        assert_group_is_kept("InvokeTool");
    }

    #[test]
    fn an_entry_that_is_not_lowercase_is_left_out() {
        let schema = tenant_schema_with(&[entry_with_key("GitHub.search")], false);
        assert_eq!(schema, base_schema_json());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "is a base action or is not ASCII lowercase")]
    fn the_public_function_asserts_on_a_group_key_in_debug_builds() {
        tenant_schema_json(&[entry_with_key("Unregistered")]);
    }

    #[test]
    fn a_policy_that_reads_a_dropped_field_fails_validation() {
        let tool = function_tool("calc", json!({"type": "object", "properties": {"num": {"type": "number"}, "s": {"type": "string"}}}));
        assert_eq!(tool.dropped_fields.len(), 1);
        assert_eq!(tool.dropped_fields[0].path, "num");
        let schema = tenant_schema_json(&[tool]);
        let (errors, _) = validate(
            "permit (principal, action == Pgs::Gateway::Action::\"calc\", resource) when { context.input.num == 1 };",
            schema.clone(),
        );
        assert!(!errors.is_empty(), "a read of the dropped field num must not validate");
        // Measured on cedar-policy 4.14: with a `has` guard, the validator types the guard as
        // `False`: no error, but an "impossible policy" warning. The caller must treat that
        // warning as a rejection if a guarded read of a dropped field must not save.
        let (errors, warnings) = validate(
            "permit (principal, action == Pgs::Gateway::Action::\"calc\", resource) when { context.input has num && context.input.num == 1 };",
            schema.clone(),
        );
        assert!(errors.is_empty(), "cedar now reports a has-guarded read as an error: {errors:?}");
        assert!(warnings.iter().any(|w| w.contains("policy is impossible")), "expected an impossible-policy warning: {warnings:?}");
        assert_valid(
            "permit (principal, action == Pgs::Gateway::Action::\"calc\", resource) when { context.input has s && context.input.s == \"x\" };",
            schema,
        );
    }

    /// The `blake3` hash of the schema output that `TOOL_SCHEMA_REVISION` pins.
    const EXPECTED_TOOL_SCHEMA_HASH: &str = "7e3d7fe6c945b4fc96bb33e3cac2b497e2166e2600f39ab94a147efdc0f8946c";

    /// The pin test for `TOOL_SCHEMA_REVISION` (spec section 6.2), in the style of
    /// `starter_policy_content_is_pinned_to_the_declared_revision` in `authz/roles.rs`.
    #[test]
    fn tool_schema_is_pinned_to_the_declared_revision() {
        let mappings: Vec<Value> = mapping_corpus()
            .iter()
            .map(|schema| match map_input_schema(schema, CatalogLimits::default().max_depth) {
                Ok((record, dropped)) => json!({
                    "ok": record,
                    "dropped": dropped.into_iter().map(|d| json!([d.path, d.reason])).collect::<Vec<_>>()
                }),
                Err(e) => json!({ "err": format!("{e:?}") }),
            })
            .collect();
        let pinned = json!([base_schema_json(), tenant_schema_json(&fixed_catalog()), mappings]);
        let actual = blake3::hash(canonical_json(&pinned).as_bytes()).to_hex().to_string();
        assert_eq!(
            actual, EXPECTED_TOOL_SCHEMA_HASH,
            "\n\nThe tool policy schema output changed (the base schema, the tenant schema or the JSON Schema mapping).\n\
             Every stored tool policy set was compiled against the older schema.\n\n\
             Do BOTH of these, in this order:\n\
             1. Bump `TOOL_SCHEMA_REVISION` (currently {TOOL_SCHEMA_REVISION}) by one.\n\
             2. Replace `EXPECTED_TOOL_SCHEMA_HASH` with:\n     {actual}\n\n\
             If you skip step 1, the re-compile job does not find the stale sets (spec section 6.2).\n"
        );
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let v = json!({"b": 1, "a": {"d": [{"z": 1, "y": 2}], "c": null}});
        assert_eq!(canonical_json(&v), r#"{"a":{"c":null,"d":[{"y":2,"z":1}]},"b":1}"#);
    }

    /// The drift control for the compiler side: the compiler tests read this file.
    #[test]
    fn the_base_schema_fixture_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/protocol/fixtures/base_schema.json");
        let expected = format!("{}\n", serde_json::to_string_pretty(&sorted(&base_schema_json())).unwrap());
        if std::env::var_os("PAIGASUS_WRITE_FIXTURES").is_some_and(|v| v == "1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}. Run this test with PAIGASUS_WRITE_FIXTURES=1 to write it.", path.display()));
        assert_eq!(actual, expected, "{} is not current. Run this test with PAIGASUS_WRITE_FIXTURES=1 to write it again.", path.display());
    }
}
