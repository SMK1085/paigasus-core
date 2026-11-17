// SPDX-License-Identifier: Apache-2.0

//! The cross-version proof (spec 2026-11-15 iam-tool-policy-sets, section 10): every golden
//! output of the compiler (cedar 4.11) passes the cedar 4.14 checks that IAM runs. Also the
//! drift control between the compiler's golden tenant schema and the IAM schema generator.

use std::path::{Path, PathBuf};

use paigasus_iam_core::tool_policy::{CatalogLimits, CompileResult, SetKind, ToolEntry, ToolKind, build_manual, check_compiled, tenant_schema_json};
use serde_json::{Value, json};

fn compiler_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn every_golden_result_validates_under_cedar_4_14() {
    let golden = compiler_dir().join("tests/golden");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&golden)
        .expect("the golden directory exists")
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.to_str().is_some_and(|s| s.ends_with(".result.json")))
        .collect();
    paths.sort();
    let mut checked = Vec::new();
    for path in &paths {
        let name = path.file_name().and_then(|n| n.to_str()).expect("a UTF-8 file name").to_owned();
        let result: CompileResult = serde_json::from_value(read_json(path)).unwrap_or_else(|error| panic!("{name}: {error}"));
        let CompileResult::Accepted { compiled, .. } = result else {
            continue;
        };
        let kind = if name.starts_with("floor_") { SetKind::Floor } else { SetKind::Tenant };
        assert_eq!(check_compiled(kind, &compiled), Ok(()), "{name} fails the cedar 4.14 checks");
        checked.push((name, kind));
    }
    // Task 5 records that every golden is accepted. A golden that becomes a rejection must not
    // make this proof smaller without a failure.
    assert_eq!(checked.len(), paths.len(), "every golden must be accepted: {checked:?}");
    assert!(checked.len() >= 9, "{checked:?}");
    assert!(checked.iter().any(|(_, kind)| *kind == SetKind::Floor), "no floor golden");
}

#[test]
fn every_accepted_protocol_fixture_validates_under_cedar_4_14() {
    for (name, kind) in [("result_tenant_accepted.json", SetKind::Tenant), ("result_floor_accepted.json", SetKind::Floor)] {
        let result: CompileResult = serde_json::from_value(read_json(&compiler_dir().join("protocol/fixtures").join(name))).expect("a result parses");
        let CompileResult::Accepted { compiled, .. } = result else {
            panic!("{name} is not accepted");
        };
        assert_eq!(check_compiled(kind, &compiled), Ok(()), "{name}");
    }
}

/// The catalog that the compiler's golden `tests/golden/tenant_schema.json` describes. Task 5
/// wrote that file with `jq` from the base schema and recorded no catalog. This is the catalog,
/// derived from the file: two function tools, each a member of `InvokeTool`, every field
/// required.
/// - `search`: `q` (string), `n` (integer).
/// - `send`: `to` (string), `amount` (integer).
fn golden_catalog() -> Vec<ToolEntry> {
    let limits = CatalogLimits::default();
    let tool = |name: &str, schema: Value| build_manual(name, ToolKind::Function, Some(schema), &limits).expect("the tool builds");
    vec![
        tool(
            "search",
            json!({"type": "object", "properties": {"q": {"type": "string"}, "n": {"type": "integer"}}, "required": ["q", "n"]}),
        ),
        tool(
            "send",
            json!({"type": "object", "properties": {"to": {"type": "string"}, "amount": {"type": "integer"}}, "required": ["to", "amount"]}),
        ),
    ]
}

#[test]
fn the_compiler_golden_tenant_schema_matches_the_generator() {
    let golden = read_json(&compiler_dir().join("tests/golden/tenant_schema.json"));
    assert_eq!(
        golden,
        tenant_schema_json(&golden_catalog()),
        "tests/golden/tenant_schema.json is not the tenant_schema_json output for the catalog in this test"
    );
    // The accepted tenant protocol fixture uses the same schema.
    let request = read_json(&compiler_dir().join("protocol/fixtures/request_tenant_accepted.json"));
    assert_eq!(request["schema_json"], golden, "request_tenant_accepted.json has another schema");
}
