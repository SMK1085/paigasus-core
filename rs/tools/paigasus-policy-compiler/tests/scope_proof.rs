// SPDX-License-Identifier: Apache-2.0
//! The scope-lowering proof (spec 2026-11-15 iam-tool-policy-sets, section 3.8 and D1).
//!
//! The compiler lowers the whole policy set once and reads `@scope` from each policy. That is
//! correct only if the lowered output of one policy does not depend on the other policies in the
//! set. These tests prove it against the pinned Dogwood commit.

use dogwood_language::{LoweredPolicySet, ParsedPolicySet, PolicySchema, ServiceSchema};
use paigasus_policy_compiler::lower::{distincter_for, lower_units, read_annotations, service_schema, slot_prefix, split_policies};

const SCHEMA: &str = include_str!("fixtures/test_schema.cedarschema");

const POLICY_A: &str = r#"
@id("a-count")
@scope("session")
permit (principal, action == Pgs::Gateway::Action::"search", resource)
when temporal {
    bind(c, count_within(1h, Pgs::Gateway::Action::"search"::request{ input.q: "x" }), c > 2)
}
when temporal {
    formerly within 10m Pgs::Gateway::Action::"send"::response{}
};
"#;

const POLICY_C: &str = r#"
@id("c-plain")
permit (principal, action == Pgs::Gateway::Action::"search", resource);
"#;

fn policy_b(scope: &str) -> String {
    format!(
        r#"
@id("b-formerly")
@scope("{scope}")
forbid (principal, action == Pgs::Gateway::Action::"send", resource)
when temporal {{
    formerly within 1h Pgs::Gateway::Action::"search"::request{{ input.q: "y" }}
}};
"#
    )
}

fn policy_schema() -> PolicySchema {
    PolicySchema::from_cedarschema_str(SCHEMA).expect("the test schema is valid")
}

fn lower_source(source: &str, svc: &ServiceSchema) -> LoweredPolicySet {
    let units = split_policies(source, svc).expect("the source parses");
    lower_units(&units, svc, &policy_schema()).expect("the set lowers")
}

/// What the proof compares for one policy: its Cedar text, its slot ids, and the `Debug`
/// form of each slot condition.
#[derive(Debug, PartialEq)]
struct PolicyOutput {
    cedar: String,
    slot_ids: Vec<String>,
    conditions: Vec<String>,
}

fn output_of(lowered: &LoweredPolicySet, id: &str) -> PolicyOutput {
    let prefix = slot_prefix(id);
    let policy_id = cedar_policy::PolicyId::new(prefix.clone());
    let cedar = lowered.as_cedar().policy(&policy_id).unwrap_or_else(|| panic!("no Cedar policy {prefix}")).to_string();
    let slot_start = format!("{prefix}__temporal_");
    let fields: Vec<_> = lowered.temporal_fields().filter(|field| field.id.starts_with(&slot_start)).collect();
    PolicyOutput {
        cedar,
        slot_ids: fields.iter().map(|field| field.id.clone()).collect(),
        conditions: fields.iter().map(|field| format!("{:?}", field.condition)).collect(),
    }
}

#[test]
fn lowering_a_policy_does_not_depend_on_the_scope_of_other_policies() {
    let svc = service_schema();
    let s1 = format!("{POLICY_A}{}{POLICY_C}", policy_b("principal"));
    let s2 = format!("{POLICY_A}{}{POLICY_C}", policy_b("key"));
    let s3 = POLICY_A.to_string();

    let lowered1 = lower_source(&s1, &svc);
    let lowered2 = lower_source(&s2, &svc);
    let a1 = output_of(&lowered1, "a-count");
    let a2 = output_of(&lowered2, "a-count");
    let a3 = output_of(&lower_source(&s3, &svc), "a-count");

    // Control: the scope change reaches the output of B, so the comparison of A is not vacuous.
    assert_ne!(
        output_of(&lowered1, "b-formerly").cedar,
        output_of(&lowered2, "b-formerly").cedar,
        "the @scope of B shows in its lowered Cedar"
    );
    assert_eq!(a1.slot_ids.len(), 2, "policy A has two temporal slots: {a1:#?}");
    assert_eq!(a1.cedar, a2.cedar, "Cedar text S1 vs S2");
    assert_eq!(a1.cedar, a3.cedar, "Cedar text S1 vs S3");
    assert_eq!(a1.slot_ids, a2.slot_ids, "slot ids S1 vs S2");
    assert_eq!(a1.slot_ids, a3.slot_ids, "slot ids S1 vs S3");
    assert_eq!(a1.conditions, a2.conditions, "slot conditions S1 vs S2");
    assert_eq!(a1.conditions, a3.conditions, "slot conditions S1 vs S3");
}

#[test]
fn slot_names_survive_deleting_an_earlier_policy() {
    let svc = service_schema();
    let with_b = format!("{}{POLICY_A}", policy_b("principal"));

    let before = output_of(&lower_source(&with_b, &svc), "a-count");
    let after = output_of(&lower_source(POLICY_A, &svc), "a-count");

    assert!(!before.slot_ids.is_empty(), "policy A has temporal slots");
    assert_eq!(before, after, "policy A output with B before it vs A alone");
    let prefix = slot_prefix("a-count");
    for id in &after.slot_ids {
        assert!(id.starts_with(&prefix), "slot {id} starts with {prefix}");
    }
}

#[test]
fn one_positional_lowering_renumbers_after_a_delete() {
    let svc = service_schema();
    let schema = policy_schema();
    let with_b = format!("{}{POLICY_A}", policy_b("principal"));

    let rule_key_of_a = |source: &str, index: usize| -> String {
        let lowered = ParsedPolicySet::parse(source, &svc)
            .expect("the source parses")
            .lower_with_distincter(&schema, "p")
            .expect("the set lowers");
        lowered.rules().nth(index).expect("policy A is lowered").cedar_policy_id
    };

    let before = rule_key_of_a(&with_b, 1);
    let after = rule_key_of_a(POLICY_A, 0);

    assert_eq!(before, "p_1");
    assert_eq!(after, "p_0");
    assert_ne!(before, after);
}

#[test]
fn annotations_read_from_the_rendering_match_the_lowered_cedar() {
    let svc = service_schema();
    let s1 = format!("{POLICY_A}{}{POLICY_C}", policy_b("principal"));
    let units = split_policies(&s1, &svc).expect("the source parses");
    let lowered = lower_units(&units, &svc, &policy_schema()).expect("the set lowers");
    assert_eq!(units.len(), 3);
    let annotation = |key: &str, value: &str| (key.to_string(), Some(value.to_string()));
    let expected = [
        vec![annotation("id", "a-count"), annotation("scope", "session")],
        vec![annotation("id", "b-formerly"), annotation("scope", "principal")],
        vec![annotation("id", "c-plain")],
    ];

    for (unit, expected) in units.iter().zip(expected) {
        let id = unit.id.as_deref().expect("every test policy has an @id");
        let policy_id = cedar_policy::PolicyId::new(slot_prefix(id));
        let policy = lowered.as_cedar().policy(&policy_id).unwrap_or_else(|| panic!("no Cedar policy for {id}"));
        let mut from_cedar: Vec<(String, Option<String>)> = policy
            .annotations()
            .map(|(key, value)| {
                let value = (!value.is_empty()).then(|| value.to_string());
                (key.to_string(), value)
            })
            .collect();
        let mut from_rendering = read_annotations(&unit.source);
        assert_eq!(from_rendering, expected, "annotations of the rendering of {id}");
        from_cedar.sort();
        from_rendering.sort();
        assert_eq!(from_rendering, from_cedar, "annotations of {id}");
    }
    assert_eq!(units[0].id.as_deref(), Some("a-count"));
    assert_eq!(units[0].scope.as_deref(), Some("session"));
    assert_eq!(units[1].scope.as_deref(), Some("principal"));
    assert_eq!(units[2].scope, None);
}

#[test]
fn lowering_the_rendering_gives_the_cedar_of_the_original_text() {
    let svc = service_schema();
    let s1 = format!("{POLICY_A}{}{POLICY_C}", policy_b("principal"));
    let from_units = output_of(&lower_source(&s1, &svc), "a-count");

    let original = ParsedPolicySet::parse(POLICY_A, &svc)
        .expect("policy A parses")
        .lower_with_distincter(&policy_schema(), distincter_for("a-count").as_str())
        .expect("policy A lowers");
    let from_original = output_of(&original, "a-count");

    assert_eq!(from_units.cedar, from_original.cedar);
}

#[test]
fn distincters_are_injective_and_valid() {
    assert_eq!(distincter_for("floor-no-shell").as_str(), "p_floor_hno_hshell");
    assert_ne!(distincter_for("a-b").as_str(), distincter_for("a_b").as_str());
    assert_eq!(distincter_for("0abc").as_str(), "p_0abc");
}
