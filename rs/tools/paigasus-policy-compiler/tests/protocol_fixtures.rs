// SPDX-License-Identifier: Apache-2.0
//! The shared protocol fixture (spec 2026-11-15 iam-tool-policy-sets, section 5.2). IAM reads
//! the same files. `PAIGASUS_WRITE_FIXTURES=1` rewrites the `result_*.json` files.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use paigasus_policy_compiler::compile::ReasonCode;
use paigasus_policy_compiler::protocol::{ProtocolError, compile_request, parse_request};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/fixtures")
}

fn write_mode() -> bool {
    std::env::var("PAIGASUS_WRITE_FIXTURES").is_ok_and(|v| v == "1")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Every `request_*.json` file name, sorted.
fn request_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixtures())
        .expect("the fixture directory exists")
        .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("request_") && name.ends_with(".json"))
        .collect();
    names.sort();
    names
}

const ALL_REASONS: [ReasonCode; 6] = [
    ReasonCode::ToolPolicyInvalid,
    ReasonCode::ToolPolicyShapeUnsupported,
    ReasonCode::ToolPolicyLoweringFailed,
    ReasonCode::ToolPolicyFloorPermit,
    ReasonCode::ToolPolicyIdInvalid,
    ReasonCode::ToolPolicyWindowTooLong,
];

#[test]
fn every_fixture_request_produces_its_fixture_result() {
    let names = request_names();
    assert_eq!(names.len(), 4, "{names:?}");
    for name in names {
        let request_text = fs::read_to_string(fixtures().join(&name)).expect("the request reads");
        let request = parse_request(&request_text).unwrap_or_else(|error| panic!("{name}: {error}"));
        let result = compile_request(&request).unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual = serde_json::to_value(&result).expect("the result serializes");
        let result_path = fixtures().join(name.replacen("request_", "result_", 1));
        if write_mode() {
            fs::write(&result_path, serde_json::to_string_pretty(&actual).expect("pretty JSON") + "\n").expect("the result writes");
        }
        assert_eq!(read_json(&result_path), actual, "{} is not current; run with PAIGASUS_WRITE_FIXTURES=1", result_path.display());
        let outcome = if name.ends_with("_accepted.json") { "accepted" } else { "rejected" };
        assert_eq!(actual["outcome"], outcome, "{name}");
    }
}

#[test]
fn every_rejection_reason_matches_the_shared_fixture() {
    let mut seen = BTreeSet::new();
    for name in request_names().iter().filter(|name| name.ends_with("_rejected.json")) {
        let result = read_json(&fixtures().join(name.replacen("request_", "result_", 1)));
        let rejections = result["rejections"].as_array().unwrap_or_else(|| panic!("{name} has no rejections"));
        assert!(!rejections.is_empty(), "{name}");
        for rejection in rejections {
            seen.insert(rejection["reason"].as_str().expect("a string reason").to_string());
        }
    }
    let floor = read_json(&fixtures().join("result_floor_rejected.json"));
    let floor_permit = ReasonCode::ToolPolicyFloorPermit.as_wire();
    assert!(
        floor["rejections"].as_array().expect("rejections").iter().any(|r| r["reason"] == floor_permit),
        "the floor pair must show {floor_permit}"
    );

    // No source reaches `tool-policy-lowering-failed` (see reason_codes.json), so that file
    // lists the reason codes that no fixture result shows.
    let unreached = read_json(&fixtures().join("reason_codes.json"));
    let unreached: BTreeSet<String> = unreached["unreached_by_any_source"]
        .as_array()
        .expect("an unreached list")
        .iter()
        .map(|code| code.as_str().expect("a string code").to_string())
        .collect();
    assert!(
        seen.is_disjoint(&unreached),
        "a reason in reason_codes.json is reached by a fixture: {:?}",
        seen.intersection(&unreached).collect::<Vec<_>>()
    );
    let union: BTreeSet<String> = seen.union(&unreached).cloned().collect();
    let all: BTreeSet<String> = ALL_REASONS.iter().map(|code| code.as_wire().to_string()).collect();
    assert_eq!(union, all);
    for code in ALL_REASONS {
        assert_eq!(serde_json::to_value(code).expect("the code serializes"), Value::String(code.as_wire().to_string()));
    }
}

fn accepted_request() -> Value {
    read_json(&fixtures().join("request_tenant_accepted.json"))
}

#[test]
fn an_unknown_protocol_version_is_a_protocol_error() {
    for version in [0, 2, 99] {
        let mut request = accepted_request();
        request["protocol_version"] = Value::from(version);
        // A later version may add fields; the version is checked before the fields.
        request["field_of_a_later_version"] = Value::from(true);
        match parse_request(&request.to_string()) {
            Err(ProtocolError::UnsupportedVersion(found)) => assert_eq!(found, version),
            other => panic!("version {version}: {other:?}"),
        }
    }
    let mut request = accepted_request();
    request.as_object_mut().expect("an object").remove("protocol_version");
    assert!(matches!(parse_request(&request.to_string()), Err(ProtocolError::Malformed(_))));
}

#[test]
fn an_unknown_request_field_is_a_protocol_error() {
    let mut request = accepted_request();
    request["extra"] = Value::from(1);
    assert!(matches!(parse_request(&request.to_string()), Err(ProtocolError::Malformed(_))));
    let mut request = accepted_request();
    request["kind"] = Value::from("admin");
    assert!(matches!(parse_request(&request.to_string()), Err(ProtocolError::Malformed(_))));
    for text in ["", "{", "[]", "null"] {
        assert!(matches!(parse_request(text), Err(ProtocolError::Malformed(_))), "{text:?}");
    }
}

/// IAM generates `schema_json`, so a schema that does not convert is an IAM defect, not a
/// tenant error: a protocol error, never a rejection.
#[test]
fn a_schema_that_does_not_convert_is_a_protocol_error() {
    for schema in [serde_json::json!({ "Pgs::Gateway": { "entityTypes": 5 } }), serde_json::json!([]), serde_json::json!("x")] {
        for source in ["", "@id(\"a\")\npermit (principal, action, resource);\n"] {
            let mut request = accepted_request();
            request["schema_json"] = schema.clone();
            request["source"] = Value::from(source);
            let request = parse_request(&request.to_string()).expect("the request parses");
            match compile_request(&request) {
                Err(ProtocolError::Malformed(detail)) => assert!(detail.contains("schema_json"), "{detail}"),
                other => panic!("{schema} with {source:?}: {other:?}"),
            }
        }
    }
}
