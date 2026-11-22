// SPDX-License-Identifier: Apache-2.0

//! The policy texts of the tenant guide section "Detector findings" (SMA-855 spec section 6.12).
//!
//! One shared fixture holds every text. This test validates each one against the IAM tool schema
//! and checks that the guide holds each one verbatim. The gateway end-to-end tests read the same
//! fixture, so the guide and the tests cannot drift.

use cedar_policy::{PolicySet, Schema, ValidationMode, Validator};
use paigasus_iam_core::tool_policy::{CatalogLimits, TOOL_SCHEMA_REVISION, ToolEntry, ToolKind, build_manual, tenant_schema_json};
use serde_json::json;

const POLICIES: &str = include_str!("../../../services/paigasus-gateway/tests/fixtures/detector-policies.cedar");
const GUIDE: &str = include_str!("../../../../../docs/tool-policies/tenant-guide.md");

/// The `@id` values of the fixture, in file order.
const EXPECTED_IDS: [&str; 4] = ["detectors-block-high", "detectors-block-kinds", "fetch-hosts", "fetch-deny-hosts"];

/// The policy texts of the fixture: the paragraphs that start with `@id(`. The comment header
/// is not a policy.
fn policy_texts() -> Vec<&'static str> {
    POLICIES.split("\n\n").map(str::trim).filter(|p| p.starts_with("@id(")).collect()
}

/// A catalog with the one tool that the host policies name.
fn fetch_catalog() -> Vec<ToolEntry> {
    let schema = json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]});
    vec![build_manual("fetch", ToolKind::Function, Some(schema), &CatalogLimits::default()).expect("the tool builds")]
}

/// Validate `source` in strict mode against the tenant schema of [`fetch_catalog`]. Return the
/// error and warning texts.
fn validate(source: &str) -> (Vec<String>, Vec<String>) {
    let schema = Schema::from_json_value(tenant_schema_json(&fetch_catalog())).expect("the schema parses");
    let policies: PolicySet = source.parse().unwrap_or_else(|e| panic!("the policy does not parse: {e}\n{source}"));
    let result = Validator::new(schema).validate(&policies, ValidationMode::Strict);
    (
        result.validation_errors().map(ToString::to_string).collect(),
        result.validation_warnings().map(ToString::to_string).collect(),
    )
}

fn assert_valid(source: &str) {
    let (errors, warnings) = validate(source);
    assert!(errors.is_empty(), "validation errors for\n{source}\n{errors:?}");
    assert!(warnings.is_empty(), "validation warnings for\n{source}\n{warnings:?}");
}

#[test]
fn the_fixture_holds_the_four_policies_in_order() {
    let texts = policy_texts();
    let ids: Vec<&str> = texts
        .iter()
        .map(|t| t.strip_prefix("@id(\"").and_then(|r| r.split_once('"')).map(|(id, _)| id).unwrap_or_else(|| panic!("no @id in\n{t}")))
        .collect();
    assert_eq!(ids, EXPECTED_IDS);
}

#[test]
fn every_detector_policy_validates_against_revision_2() {
    assert_eq!(TOOL_SCHEMA_REVISION, 2, "the detector attributes came with schema revision 2");
    for text in policy_texts() {
        assert_valid(text);
    }
    // All four in one set, as a tenant can save them.
    assert_valid(&policy_texts().join("\n\n"));
    // The plain test for one host.
    assert_valid("@id(\"one-host\")\nforbid (principal, action == Pgs::Gateway::Action::\"fetch\", resource)\nwhen { context.call.hosts.contains(\"a\") };");
}

#[test]
fn a_policy_that_reads_an_unknown_context_attribute_does_not_validate() {
    // The control: the validator is strict, so the positive test above can fail.
    let (errors, _) = validate("@id(\"x\")\nforbid (principal, action, resource)\nwhen { context.min_severity >= 3 };");
    assert!(!errors.is_empty(), "an unknown attribute validated");
    let (errors, _) = validate("@id(\"x\")\nforbid (principal, action, resource)\nwhen { context.max_severity == \"high\" };");
    assert!(!errors.is_empty(), "a String comparison with a Long validated");
}

#[test]
fn the_tenant_guide_holds_each_policy_verbatim() {
    for text in policy_texts() {
        assert!(GUIDE.contains(text), "docs/tool-policies/tenant-guide.md does not hold this policy verbatim:\n{text}");
    }
}
