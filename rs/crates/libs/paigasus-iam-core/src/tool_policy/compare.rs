// SPDX-License-Identifier: Apache-2.0

//! The one comparator of two compiled outputs (spec section 8.1), for the catalog coupling
//! (section 4.6) and the re-compile job (section 8). It compares the Cedar text, the schema and
//! the conditions only. It never compares versions, generations or revisions, which always
//! differ.

use serde_json::Value;

use super::protocol::CompiledOutput;

/// True when `a` and `b` give the same enforcement: the same `cedar_policies` text, the same
/// `cedar_schema_json` after canonical serialisation (sorted keys), and the same `conditions`.
pub fn outputs_identical(a: &CompiledOutput, b: &CompiledOutput) -> bool {
    a.cedar_policies == b.cedar_policies && canonical_json(&a.cedar_schema_json) == canonical_json(&b.cedar_schema_json) && a.conditions == b.conditions
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
}
