// SPDX-License-Identifier: Apache-2.0

//! The comparators of two compiled outputs (spec section 8.1). They compare the Cedar text, the
//! schema and the conditions only. They never compare versions, generations or revisions, which
//! always differ.
//!
//! - [`outputs_identical`] is exact. The catalog coupling (section 4.6) and the diff report use it.
//! - [`outputs_equivalent_for_recompile`] also accepts a schema that only adds Record
//!   attributes (SMA-855 spec section 6.13.2). Only the re-compile job uses it.

use serde_json::{Map, Value};

use super::protocol::CompiledOutput;

/// True when `a` and `b` give the same enforcement: the same `cedar_policies` text, the same
/// `cedar_schema_json` after canonical serialisation (sorted keys), and the same `conditions`.
pub fn outputs_identical(a: &CompiledOutput, b: &CompiledOutput) -> bool {
    a.cedar_policies == b.cedar_policies && canonical_json(&a.cedar_schema_json) == canonical_json(&b.cedar_schema_json) && a.conditions == b.conditions
}

/// True when `new` has the same policy text and conditions as `old`, with a schema that only
/// adds attributes to Record types.
///
/// This is not a proof of the same enforcement. A policy that tests `context has <new
/// attribute>` (or `has` on a new attribute of a common type) can change its meaning when the
/// gateway fills the attribute. That is a known residual (SMA-855 spec section 6.13.2).
///
/// Only the re-compile job calls this (SMA-855 spec section 6.13.2, D28). The catalog coupling
/// and the diff report keep [`outputs_identical`], so a catalog write that adds an input
/// property still writes a `catalog_change` version.
pub fn outputs_equivalent_for_recompile(old: &CompiledOutput, new: &CompiledOutput) -> bool {
    old.cedar_policies == new.cedar_policies && old.conditions == new.conditions && schema_ext(&old.cedar_schema_json, &new.cedar_schema_json)
}

fn exact(old: &Value, new: &Value) -> bool {
    canonical_json(old) == canonical_json(new)
}

/// Two JSON objects, the old one first.
type ObjectPair<'a> = (&'a Map<String, Value>, &'a Map<String, Value>);

/// Both are objects with the same key set.
fn same_keys<'a>(old: &'a Value, new: &'a Value) -> Option<ObjectPair<'a>> {
    let (o, n) = (old.as_object()?, new.as_object()?);
    (o.len() == n.len() && o.keys().all(|k| n.contains_key(k))).then_some((o, n))
}

/// The schema level: the namespaces. Anything that is not the Cedar JSON schema grammar is
/// compared exactly.
fn schema_ext(old: &Value, new: &Value) -> bool {
    if exact(old, new) {
        return true;
    }
    let Some((o, n)) = same_keys(old, new) else { return false };
    o.iter().all(|(ns, ov)| namespace_ext(ov, &n[ns]))
}

/// One namespace: `commonTypes` values and action contexts are type places; every other key
/// (`entityTypes`, `annotations`, ...) is exact.
fn namespace_ext(old: &Value, new: &Value) -> bool {
    let Some((o, n)) = same_keys(old, new) else { return exact(old, new) };
    o.iter().all(|(key, ov)| {
        let nv = &n[key];
        match key.as_str() {
            "commonTypes" => match same_keys(ov, nv) {
                Some((oc, nc)) => oc.iter().all(|(name, t)| type_ext(t, &nc[name])),
                None => exact(ov, nv),
            },
            "actions" => match same_keys(ov, nv) {
                Some((oa, na)) => oa.iter().all(|(name, a)| action_ext(a, &na[name])),
                None => exact(ov, nv),
            },
            _ => exact(ov, nv),
        }
    })
}

/// One action: every key but `appliesTo` is exact; in `appliesTo`, every key but `context` is
/// exact, and `context` is a type place.
fn action_ext(old: &Value, new: &Value) -> bool {
    let Some((o, n)) = same_keys(old, new) else { return exact(old, new) };
    o.iter().all(|(key, ov)| {
        let nv = &n[key];
        if key != "appliesTo" {
            return exact(ov, nv);
        }
        let Some((oa, na)) = same_keys(ov, nv) else { return exact(ov, nv) };
        oa.iter().all(|(k, v)| if k == "context" { type_ext(v, &na[k]) } else { exact(v, &na[k]) })
    })
}

/// The `"type"` value of a type object.
fn type_name(value: &Value) -> Option<&str> {
    value.get("type")?.as_str()
}

/// A type place. A `Record` may add attributes; a `Set` follows its `element`; any other type is
/// exact. A key named `attributes` counts only on a `Record` type object.
fn type_ext(old: &Value, new: &Value) -> bool {
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else { return exact(old, new) };
    let (ot, nt) = (type_name(old), type_name(new));
    if ot != nt {
        return false;
    }
    let structural = match ot {
        Some("Record") => "attributes",
        Some("Set") => "element",
        _ => return exact(old, new),
    };
    let others_equal = o.len() == n.len() && o.iter().all(|(k, v)| k == structural || n.get(k).is_some_and(|nv| exact(v, nv))) && o.contains_key(structural) == n.contains_key(structural);
    if !others_equal {
        return false;
    }
    match (o.get(structural), n.get(structural)) {
        (None, None) => true,
        (Some(ov), Some(nv)) if structural == "element" => type_ext(ov, nv),
        (Some(ov), Some(nv)) => attributes_ext(ov, nv),
        _ => false,
    }
}

/// A Record's `attributes` map: the new map holds every old key, each shared value is exact or
/// an additive Record, and the new map may have more keys with any `required` value.
fn attributes_ext(old: &Value, new: &Value) -> bool {
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else { return exact(old, new) };
    o.iter().all(|(name, ov)| {
        n.get(name)
            .is_some_and(|nv| exact(ov, nv) || (type_name(ov) == Some("Record") && type_name(nv) == Some("Record") && type_ext(ov, nv)))
    })
}

/// Compact JSON with the object keys in sorted order at every level.
///
/// Array order is kept on purpose. A change of order only in a schema array (`memberOf`,
/// `principalTypes`, `resourceTypes`) makes the outputs "different". That is the safe direction
/// for the re-compile job: it stores one more version, and it never keeps an old version that
/// differs. Do not sort the arrays.
pub(crate) fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_policy::protocol::{CmpOp, HistoryCondition, Predicate, Scope, Shape};
    use serde_json::json;

    fn output(threshold: i64, policies: &str, schema: Value) -> CompiledOutput {
        CompiledOutput {
            cedar_policies: policies.to_owned(),
            cedar_schema_json: schema,
            conditions: vec![HistoryCondition {
                slot: "p_rate_0__temporal_0".into(),
                policy_id: "rate".into(),
                scope: Scope::Session,
                window_ms: 60_000,
                shape: Shape::CountWithin {
                    predicate: Predicate::AnyCatalogTool {},
                    op: CmpOp::Le,
                    threshold,
                },
            }],
        }
    }

    const POLICY: &str = "@id(\"rate\")\nforbid(principal, action, resource) when { context.p_rate_0__temporal_0 };";

    #[test]
    fn identical_ignores_key_order_in_the_schema() {
        let a: Value = serde_json::from_str(r#"{"Pgs::Gateway": {"entityTypes": {}, "actions": {"a": {"memberOf": [], "appliesTo": null}}}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"Pgs::Gateway": {"actions": {"a": {"appliesTo": null, "memberOf": []}}, "entityTypes": {}}}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert!(outputs_identical(&output(3, POLICY, a), &output(3, POLICY, b)));
    }

    #[test]
    fn a_changed_threshold_is_not_identical() {
        assert!(!outputs_identical(&output(3, POLICY, json!({})), &output(4, POLICY, json!({}))));
    }

    #[test]
    fn a_changed_policy_text_is_not_identical() {
        let other = POLICY.replace("forbid", "permit");
        assert!(!outputs_identical(&output(3, POLICY, json!({})), &output(3, &other, json!({}))));
    }

    #[test]
    fn a_changed_schema_value_is_not_identical() {
        assert!(!outputs_identical(&output(3, POLICY, json!({"a": [1, 2]})), &output(3, POLICY, json!({"a": [2, 1]}))));
    }

    #[test]
    fn an_order_only_change_in_a_schema_array_is_not_identical() {
        let a = json!({"Pgs::Gateway": {"actions": {"a": {"appliesTo": {"principalTypes": ["P", "Q"], "resourceTypes": ["R"]}}}}});
        let b = json!({"Pgs::Gateway": {"actions": {"a": {"appliesTo": {"principalTypes": ["Q", "P"], "resourceTypes": ["R"]}}}}});
        assert!(!outputs_identical(&output(3, POLICY, a), &output(3, POLICY, b)));
    }

    #[test]
    fn a_change_in_condition_order_is_not_identical() {
        let base = output(3, POLICY, json!({}));
        let mut second = base.conditions[0].clone();
        second.slot = "p_rate_1__temporal_0".into();
        let mut a = base.clone();
        a.conditions.push(second);
        let mut b = a.clone();
        b.conditions.reverse();
        assert!(outputs_identical(&a, &a.clone()), "the control");
        assert!(!outputs_identical(&a, &b));
    }

    #[test]
    fn a_change_in_only_scope_window_or_slot_is_not_identical() {
        let base = output(3, POLICY, json!({}));
        let changes: [fn(&mut HistoryCondition); 3] = [|c| c.scope = Scope::Principal, |c| c.window_ms = 60_001, |c| c.slot = "p_rate_9__temporal_0".into()];
        for (i, change) in changes.into_iter().enumerate() {
            let mut other = base.clone();
            change(&mut other.conditions[0]);
            assert!(!outputs_identical(&base, &other), "change {i} counts as identical");
        }
    }

    // --- outputs_equivalent_for_recompile (SMA-855 spec section 6.13.2) ---

    use crate::tool_policy::schema::{base_schema_json, tenant_schema_json};
    use crate::tool_policy::{CatalogLimits, ToolKind, build_manual};

    /// An output with the fixed policies and conditions and the given schema.
    fn out(schema: Value) -> CompiledOutput {
        output(3, POLICY, schema)
    }

    /// `schema` (revision 2) without the two attributes that revision 2 added: the revision-1
    /// shape. The tests below add them again, so a test of an added attribute does not only
    /// replace a key that is already there.
    fn revision_1(mut schema: Value) -> Value {
        for action in schema["Pgs::Gateway"]["actions"].as_object_mut().unwrap().values_mut() {
            assert!(action["appliesTo"]["context"]["attributes"].as_object_mut().unwrap().remove("max_severity").is_some());
        }
        assert!(schema["Pgs::Gateway"]["commonTypes"]["CallInfo"]["attributes"].as_object_mut().unwrap().remove("hosts").is_some());
        schema
    }

    /// The revision-1 base schema.
    fn base() -> Value {
        revision_1(base_schema_json())
    }

    fn eq(old: Value, new: Value) -> bool {
        outputs_equivalent_for_recompile(&out(old), &out(new))
    }

    /// The context record attributes of the group action `InvokeTool`.
    fn context_attrs(schema: &mut Value) -> &mut serde_json::Map<String, Value> {
        schema["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["context"]["attributes"].as_object_mut().unwrap()
    }

    /// `schema` with one more context attribute in every action.
    fn with_context_attr(mut schema: Value, name: &str, ty: Value) -> Value {
        for action in schema["Pgs::Gateway"]["actions"].as_object_mut().unwrap().values_mut() {
            action["appliesTo"]["context"]["attributes"].as_object_mut().unwrap().insert(name.to_owned(), ty.clone());
        }
        schema
    }

    /// `schema` with one more `CallInfo` attribute.
    fn with_call_attr(mut schema: Value, name: &str, ty: Value) -> Value {
        schema["Pgs::Gateway"]["commonTypes"]["CallInfo"]["attributes"].as_object_mut().unwrap().insert(name.to_owned(), ty);
        schema
    }

    /// The revision-2 shape of Task 17: both added attributes.
    fn revision_2(schema: Value) -> Value {
        let schema = with_context_attr(schema, "max_severity", json!({"type": "Long", "required": true}));
        with_call_attr(schema, "hosts", json!({"type": "Set", "element": {"type": "String"}, "required": true}))
    }

    /// A tenant schema with one function tool `t` that has `input` as its JSON Schema.
    fn tool_schema(input: Value) -> Value {
        let entry = build_manual("t", ToolKind::Function, Some(input), &CatalogLimits::default()).unwrap();
        revision_1(tenant_schema_json(&[entry]))
    }

    #[test]
    fn the_revision_2_shape_of_these_tests_is_the_generated_schema() {
        assert_eq!(revision_2(base()), base_schema_json());
        let entry = build_manual("t", ToolKind::Function, Some(json!({"type": "object"})), &CatalogLimits::default()).unwrap();
        assert_eq!(revision_2(revision_1(tenant_schema_json(std::slice::from_ref(&entry)))), tenant_schema_json(&[entry]));
    }

    #[test]
    fn the_same_output_is_equivalent() {
        assert!(eq(base(), base()));
        assert!(eq(json!({}), json!({})));
    }

    #[test]
    fn an_added_context_attribute_is_equivalent() {
        assert!(eq(base(), with_context_attr(base(), "max_severity", json!({"type": "Long", "required": true}))));
    }

    #[test]
    fn an_added_call_info_attribute_is_equivalent() {
        let hosts = json!({"type": "Set", "element": {"type": "String"}, "required": true});
        assert!(eq(base(), with_call_attr(base(), "hosts", hosts)));
    }

    #[test]
    fn the_revision_2_shape_is_equivalent_for_the_base_and_a_tenant_schema() {
        assert!(eq(base(), revision_2(base())));
        let tenant = tool_schema(json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}));
        assert!(eq(tenant.clone(), revision_2(tenant)));
    }

    #[test]
    fn an_optional_added_attribute_is_equivalent() {
        assert!(eq(base(), with_context_attr(base(), "note", json!({"type": "String", "required": false}))));
        assert!(eq(base(), with_call_attr(base(), "note", json!({"type": "String"}))));
    }

    #[test]
    fn an_added_attribute_in_a_nested_record_attribute_is_equivalent() {
        let nested = |extra: bool| {
            let mut attrs = json!({"a": {"type": "String", "required": true}});
            if extra {
                attrs["b"] = json!({"type": "Long", "required": false});
            }
            with_call_attr(base(), "origin", json!({"type": "Record", "attributes": attrs, "required": true}))
        };
        assert!(eq(nested(false), nested(true)));
        assert!(!eq(nested(true), nested(false)), "the removal of a nested attribute");
    }

    #[test]
    fn an_added_attribute_in_the_record_element_of_a_set_common_type_is_equivalent() {
        let set = |extra: bool| {
            let mut attrs = json!({"a": {"type": "String"}});
            if extra {
                attrs["b"] = json!({"type": "Long"});
            }
            let mut schema = base();
            schema["Pgs::Gateway"]["commonTypes"]["Items"] = json!({"type": "Set", "element": {"type": "Record", "attributes": attrs}});
            schema
        };
        assert!(eq(set(false), set(true)));
        assert!(!eq(set(true), set(false)));
    }

    #[test]
    fn a_removed_attribute_is_different() {
        let mut new = base();
        context_attrs(&mut new).remove("findings");
        assert!(!eq(base(), new));
        let mut new = base();
        new["Pgs::Gateway"]["commonTypes"]["CallInfo"]["attributes"].as_object_mut().unwrap().remove("declared");
        assert!(!eq(base(), new));
    }

    #[test]
    fn a_changed_attribute_type_is_different() {
        let mut new = base();
        context_attrs(&mut new).insert("input_text".into(), json!({"type": "Long", "required": false}));
        assert!(!eq(base(), new));
        let mut new = base();
        new["Pgs::Gateway"]["commonTypes"]["SessionInfo"]["attributes"]["turn"] = json!({"type": "String", "required": true});
        assert!(!eq(base(), new));
    }

    #[test]
    fn a_changed_required_is_different() {
        let mut new = base();
        context_attrs(&mut new).insert("call".into(), json!({"type": "CallInfo", "required": false}));
        assert!(!eq(base(), new.clone()), "required true to false");
        assert!(!eq(new, base()), "required false to true");
    }

    #[test]
    fn a_new_entity_type_or_common_type_or_action_is_different() {
        let mut new = base();
        new["Pgs::Iam"]["entityTypes"]["Team"] = json!({});
        assert!(!eq(base(), new));
        let mut new = base();
        new["Pgs::Gateway"]["commonTypes"]["Extra"] = json!({"type": "Record", "attributes": {}});
        assert!(!eq(base(), new));
        let mut new = base();
        new["Pgs::Gateway"]["actions"]["Other"] = json!({"appliesTo": null});
        assert!(!eq(base(), new));
        let mut new = base();
        new["Pgs::Other"] = json!({"entityTypes": {}, "actions": {}});
        assert!(!eq(base(), new));
    }

    #[test]
    fn an_added_entity_shape_attribute_is_different() {
        // An entity type shape is a Record, but `entityTypes` is outside the additive rule.
        let shape = |extra: bool| {
            let mut attrs = json!({"a": {"type": "String"}});
            if extra {
                attrs["b"] = json!({"type": "String"});
            }
            let mut schema = base();
            schema["Pgs::Iam"]["entityTypes"]["Principal"] = json!({"shape": {"type": "Record", "attributes": attrs}});
            schema
        };
        assert!(!eq(shape(false), shape(true)));
    }

    #[test]
    fn a_reordered_array_or_a_changed_member_of_is_different() {
        let mut new = base();
        new["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["principalTypes"] = json!(["Pgs::Iam::Principal", "Pgs::Iam::Other"]);
        let mut reordered = new.clone();
        reordered["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["principalTypes"] = json!(["Pgs::Iam::Other", "Pgs::Iam::Principal"]);
        assert!(!eq(new, reordered));
        let mut new = base();
        new["Pgs::Gateway"]["actions"]["InvokeTool"]["memberOf"] = json!([{"id": "Unregistered"}]);
        assert!(!eq(base(), new));
    }

    #[test]
    fn a_record_with_a_changed_non_attribute_key_is_different() {
        let mut new = base();
        new["Pgs::Gateway"]["commonTypes"]["CallInfo"]["additionalAttributes"] = json!(true);
        assert!(!eq(base(), new));
    }

    #[test]
    fn a_context_that_stops_being_a_record_is_different() {
        let mut new = base();
        new["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["context"] = json!({"type": "CallInfo"});
        assert!(!eq(base(), new));
        let mut old = base();
        old["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["context"] = json!({"type": "Ctx", "attributes": {}});
        let mut new = old.clone();
        new["Pgs::Gateway"]["actions"]["InvokeTool"]["appliesTo"]["context"] = json!({"type": "Ctx", "attributes": {"x": {"type": "Long"}}});
        assert!(!eq(old, new), "an attributes key on a non-Record type is not an attributes map");
    }

    #[test]
    fn changed_policy_text_is_different() {
        let new = revision_2(base());
        let other = POLICY.replace("forbid", "permit");
        assert!(!outputs_equivalent_for_recompile(&out(base()), &output(3, &other, new.clone())));
        assert!(!outputs_equivalent_for_recompile(&out(base()), &output(4, POLICY, new)), "a changed condition");
    }

    #[test]
    fn an_input_property_named_attributes() {
        let obj = |props: Value| json!({"type": "object", "properties": props});
        // An added property `attributes` is a key inside an attributes map: an added attribute.
        let old = tool_schema(obj(json!({"q": {"type": "string"}})));
        let added = tool_schema(obj(json!({"q": {"type": "string"}, "attributes": {"type": "string"}})));
        assert!(eq(old, added.clone()));
        // A changed type of the property `attributes` is a changed attribute.
        let changed = tool_schema(obj(json!({"q": {"type": "string"}, "attributes": {"type": "integer"}})));
        assert!(!eq(added.clone(), changed));
        // A removed property `attributes` is a removed attribute.
        assert!(!eq(added, tool_schema(obj(json!({"q": {"type": "string"}})))));
        // A property `attributes` that is an object: an added inner property is additive, a
        // removed one is not.
        let inner = |props: Value| tool_schema(obj(json!({"attributes": obj(props)})));
        assert!(eq(inner(json!({"x": {"type": "string"}})), inner(json!({"x": {"type": "string"}, "y": {"type": "string"}}))));
        assert!(!eq(inner(json!({"x": {"type": "string"}, "y": {"type": "string"}})), inner(json!({"x": {"type": "string"}}))));
    }

    #[test]
    fn an_action_named_attributes_does_not_make_the_actions_map_additive() {
        let custom = |name: &str| build_manual(name, ToolKind::Custom, None, &CatalogLimits::default()).unwrap();
        let old = tenant_schema_json(&[custom("attributes")]);
        let new = tenant_schema_json(&[custom("attributes"), custom("other")]);
        assert!(!eq(old, new));
    }

    #[test]
    fn outputs_identical_stays_exact() {
        let new = with_context_attr(base(), "max_severity", json!({"type": "Long", "required": true}));
        assert!(!outputs_identical(&out(base()), &out(new.clone())));
        assert!(outputs_equivalent_for_recompile(&out(base()), &out(new)));
    }
}
