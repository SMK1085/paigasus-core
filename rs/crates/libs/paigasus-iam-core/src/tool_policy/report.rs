// SPDX-License-Identifier: Apache-2.0

//! The report of a held tool policy version (plan H17). It lists names only, never content, so
//! its size stays bounded by the catalog cap. `diff_outputs` compares two compiled outputs.
//! `failed_report` holds the rejections of a failed compile.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use cedar_policy::PolicySet;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::compare::{canonical_json, outputs_identical};
use super::protocol::CompiledOutput;

/// The key in `policies.changed` when a Cedar text does not parse and the two texts differ.
const UNPARSED_KEY: &str = "<unparsed>";
/// The key in `policies.changed` when only the text differs, for example in the formatting.
const TEXT_KEY: &str = "<text>";

/// The result of a held compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldResult {
    Different,
    Failed,
}

/// The names that changed between two outputs. Each list is sorted and has no duplicate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl NameDiff {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// One rejection of a failed compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRejection {
    pub policy_id: Option<String>,
    pub code: String,
    pub message: String,
}

/// The report of one held version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldReport {
    pub result: HeldResult,
    pub policies: NameDiff,
    pub conditions: NameDiff,
    pub schema: NameDiff,
    pub rejections: Vec<ReportRejection>,
}

/// The report of a failed compile: the rejections and three empty diffs.
pub fn failed_report(rejections: Vec<ReportRejection>) -> HeldReport {
    HeldReport {
        result: HeldResult::Failed,
        policies: NameDiff::default(),
        conditions: NameDiff::default(),
        schema: NameDiff::default(),
        rejections,
    }
}

/// The report of two outputs that differ. The result is `Different` and `rejections` is empty.
///
/// When `outputs_identical` is false but the three diffs are empty (for example a compiler bump
/// that changes only the formatting), `policies.changed` holds `"<text>"`, so the report never
/// hides a change.
pub fn diff_outputs(old: &CompiledOutput, new: &CompiledOutput) -> HeldReport {
    let mut policies = match (policy_texts(&old.cedar_policies), policy_texts(&new.cedar_policies)) {
        (Some(a), Some(b)) => diff_maps(&a, &b),
        _ if old.cedar_policies == new.cedar_policies => NameDiff::default(),
        _ => NameDiff {
            changed: vec![UNPARSED_KEY.to_owned()],
            ..NameDiff::default()
        },
    };
    let conditions = diff_maps(&condition_map(old), &condition_map(new));
    let schema = diff_maps(&schema_map(&old.cedar_schema_json), &schema_map(&new.cedar_schema_json));
    if policies.is_empty() && conditions.is_empty() && schema.is_empty() && !outputs_identical(old, new) {
        policies.changed.push(TEXT_KEY.to_owned());
    }
    HeldReport {
        result: HeldResult::Different,
        policies,
        conditions,
        schema,
        rejections: Vec::new(),
    }
}

/// The policy texts keyed by `@id`, or by the Cedar policy id when a policy has no `@id`.
/// `None` when the text does not parse.
fn policy_texts(text: &str) -> Option<BTreeMap<String, String>> {
    let set = PolicySet::from_str(text).ok()?;
    Some(set.policies().map(|p| (p.annotation("id").map_or_else(|| p.id().to_string(), str::to_owned), p.to_string())).collect())
}

fn condition_map(output: &CompiledOutput) -> BTreeMap<String, String> {
    output.conditions.iter().map(|c| (c.slot.clone(), format!("{c:?}"))).collect()
}

/// The schema entries keyed by `<namespace>::<entityTypes|actions|commonTypes>::<name>`. The
/// value is the canonical JSON of the entry, so a key order change is not a change.
fn schema_map(schema: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(namespaces) = schema.as_object() else { return out };
    for (namespace, body) in namespaces {
        let Some(sections) = body.as_object() else { continue };
        for (section, entries) in sections {
            let Some(entries) = entries.as_object() else { continue };
            for (name, entry) in entries {
                out.insert(format!("{namespace}::{section}::{name}"), canonical_json(entry));
            }
        }
    }
    out
}

fn diff_maps(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> NameDiff {
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut diff = NameDiff::default();
    for key in keys {
        match (old.get(key), new.get(key)) {
            (None, Some(_)) => diff.added.push(key.clone()),
            (Some(_), None) => diff.removed.push(key.clone()),
            (Some(a), Some(b)) if a != b => diff.changed.push(key.clone()),
            _ => {}
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_policy::protocol::{CmpOp, HistoryCondition, Predicate, Scope, Shape};
    use serde_json::json;

    const RATE: &str = "@id(\"rate\")\nforbid(principal, action, resource) when { context.p_rate_0__temporal_0 };";
    const OTHER: &str = "@id(\"other\")\npermit(principal, action, resource);";

    fn condition(threshold: i64) -> HistoryCondition {
        HistoryCondition {
            slot: "p_rate_0__temporal_0".into(),
            policy_id: "rate".into(),
            scope: Scope::Session,
            window_ms: 60_000,
            shape: Shape::CountWithin {
                predicate: Predicate::AnyCatalogTool {},
                op: CmpOp::Le,
                threshold,
            },
        }
    }

    fn output(policies: &str, schema: Value, threshold: i64) -> CompiledOutput {
        CompiledOutput {
            cedar_policies: policies.to_owned(),
            cedar_schema_json: schema,
            conditions: vec![condition(threshold)],
        }
    }

    fn base() -> CompiledOutput {
        output(RATE, json!({"Pgs::Gateway": {"entityTypes": {}, "actions": {"read": {}}}}), 3)
    }

    #[test]
    fn identical_outputs_give_three_empty_diffs() {
        let report = diff_outputs(&base(), &base());
        assert_eq!(report.result, HeldResult::Different);
        assert!(report.policies.is_empty() && report.conditions.is_empty() && report.schema.is_empty());
        assert!(report.rejections.is_empty());
    }

    #[test]
    fn a_new_policy_id_is_added_and_a_missing_one_is_removed() {
        let both = output(&format!("{RATE}\n{OTHER}"), base().cedar_schema_json, 3);
        let added = diff_outputs(&base(), &both);
        assert_eq!(added.policies.added, vec!["other"]);
        assert!(added.policies.removed.is_empty() && added.policies.changed.is_empty());
        let removed = diff_outputs(&both, &base());
        assert_eq!(removed.policies.removed, vec!["other"]);
        assert!(removed.policies.added.is_empty());
    }

    #[test]
    fn the_same_id_with_another_text_is_changed() {
        let other = output(&RATE.replace("forbid", "permit"), base().cedar_schema_json, 3);
        let report = diff_outputs(&base(), &other);
        assert_eq!(report.policies.changed, vec!["rate"]);
        assert!(report.policies.added.is_empty() && report.policies.removed.is_empty());
    }

    #[test]
    fn a_condition_is_keyed_by_slot() {
        let report = diff_outputs(&base(), &output(RATE, base().cedar_schema_json, 4));
        assert_eq!(report.conditions.changed, vec!["p_rate_0__temporal_0"]);
        assert!(report.conditions.added.is_empty() && report.conditions.removed.is_empty());
        assert!(report.policies.is_empty() && report.schema.is_empty());
    }

    #[test]
    fn a_new_tool_action_is_an_added_schema_entry() {
        let schema = json!({"Pgs::Gateway": {"entityTypes": {}, "actions": {"read": {}, "search": {}}}});
        let report = diff_outputs(&base(), &output(RATE, schema, 3));
        assert_eq!(report.schema.added, vec!["Pgs::Gateway::actions::search"]);
        assert!(report.schema.removed.is_empty() && report.schema.changed.is_empty());
    }

    #[test]
    fn a_key_order_change_in_the_schema_is_not_a_change() {
        let a: Value = serde_json::from_str(r#"{"Pgs::Gateway": {"actions": {"read": {"appliesTo": null, "memberOf": []}}, "entityTypes": {}}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"Pgs::Gateway": {"entityTypes": {}, "actions": {"read": {"memberOf": [], "appliesTo": null}}}}"#).unwrap();
        let report = diff_outputs(&output(RATE, a, 3), &output(RATE, b, 3));
        assert!(report.schema.is_empty() && report.policies.is_empty() && report.conditions.is_empty());
    }

    #[test]
    fn an_unparsable_policy_text_still_reports_a_change() {
        let report = diff_outputs(&output("garbage", json!({}), 3), &output(RATE, json!({}), 3));
        assert!(report.policies.changed.contains(&"<unparsed>".to_owned()));
        let same = diff_outputs(&output("garbage", json!({}), 3), &output("garbage", json!({}), 3));
        assert!(same.policies.is_empty());
    }

    #[test]
    fn a_text_only_change_is_still_reported() {
        let spaced = output(&format!("{RATE}\n\n"), base().cedar_schema_json, 3);
        assert_ne!(base().cedar_policies, spaced.cedar_policies, "the control");
        let report = diff_outputs(&base(), &spaced);
        assert_eq!(report.policies.changed, vec!["<text>"]);
        assert!(report.conditions.is_empty() && report.schema.is_empty());
    }

    #[test]
    fn the_report_round_trips_through_json_with_snake_case_keys() {
        let report = diff_outputs(&base(), &output(&format!("{RATE}\n{OTHER}"), base().cedar_schema_json, 3));
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["result"], "different");
        assert_eq!(value["policies"]["added"], json!(["other"]));
        assert_eq!(serde_json::from_value::<HeldReport>(value).unwrap(), report);

        let failed = failed_report(vec![ReportRejection {
            policy_id: Some("p".into()),
            code: "c".into(),
            message: "m".into(),
        }]);
        let value = serde_json::to_value(&failed).unwrap();
        assert_eq!(value["result"], "failed");
        assert_eq!(value["rejections"][0]["policy_id"], "p");
        assert!(failed.policies.is_empty() && failed.conditions.is_empty() && failed.schema.is_empty());
        assert_eq!(serde_json::from_value::<HeldReport>(value).unwrap(), failed);
    }
}
