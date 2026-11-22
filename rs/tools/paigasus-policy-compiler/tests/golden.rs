// SPDX-License-Identifier: Apache-2.0
//! The golden output, the generation pin (spec 2026-11-15 iam-tool-policy-sets, section 6.2),
//! the base schema under cedar 4.14, and the binary over stdin.
//!
//! A golden `floor_*.dw` and `starter.dw` compile against `protocol/fixtures/base_schema.json`.
//! Every other golden is a tenant set and compiles against `tests/golden/tenant_schema.json`.
//! `PAIGASUS_WRITE_FIXTURES=1` rewrites the `*.result.json` files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cedar_policy::{PolicySet, Schema, ValidationMode, Validator};
use paigasus_policy_compiler::compile::SetKind;
use paigasus_policy_compiler::protocol::{CompileRequest, compile_request, parse_request};
use paigasus_policy_compiler::{COMPILER_GENERATION, revision_string};
use serde_json::Value;

const DAY_MS: u64 = 86_400_000;
const BIN: &str = env!("CARGO_BIN_EXE_paigasus-policy-compiler");

/// The `blake3` hash of the golden results that `COMPILER_GENERATION` pins.
const EXPECTED_GOLDEN_HASH: &str = "b163461c7d755ea5de7aa2704244f1446a3b026e9f4651a2d37a895344bef415";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn golden_dir() -> PathBuf {
    root().join("tests/golden")
}

fn write_mode() -> bool {
    std::env::var("PAIGASUS_WRITE_FIXTURES").is_ok_and(|v| v == "1")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn base_schema() -> Value {
    read_json(&root().join("protocol/fixtures/base_schema.json"))
}

fn request(kind: SetKind, source: &str, schema_json: Value) -> CompileRequest {
    CompileRequest {
        protocol_version: 1,
        kind,
        source: source.to_string(),
        schema_json,
        max_window_ms: DAY_MS,
    }
}

/// The kind and the schema of a golden source, from its file name.
fn golden_request(name: &str, source: &str) -> CompileRequest {
    if name.starts_with("floor_") {
        request(SetKind::Floor, source, base_schema())
    } else if name == "starter.dw" {
        request(SetKind::Tenant, source, base_schema())
    } else {
        request(SetKind::Tenant, source, read_json(&golden_dir().join("tenant_schema.json")))
    }
}

fn compile_golden(name: &str) -> Value {
    let source = fs::read_to_string(golden_dir().join(name)).expect("the golden source reads");
    let result = compile_request(&golden_request(name, &source)).unwrap_or_else(|error| panic!("{name}: {error}"));
    serde_json::to_value(&result).expect("the result serializes")
}

#[test]
fn the_base_schema_loads_under_cedar_4_14_and_a_starter_like_policy_validates() {
    let schema = Schema::from_json_value(base_schema()).expect("the base schema parses under cedar 4.14");
    let source = fs::read_to_string(golden_dir().join("starter.dw")).expect("the starter reads");
    let policies: PolicySet = source.parse().expect("the starter parses under cedar 4.14");
    let result = Validator::new(schema).validate(&policies, ValidationMode::Strict);
    assert!(result.validation_passed(), "{:?}", result.validation_errors().map(ToString::to_string).collect::<Vec<_>>());
    assert_eq!(result.validation_warnings().count(), 0);
}

#[test]
fn the_starter_compiles_against_the_base_schema() {
    let source = fs::read_to_string(golden_dir().join("starter.dw")).expect("the starter reads");
    assert!(
        source.ends_with("@id(\"starter-allow-all\")\npermit (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);\n"),
        "{source}"
    );
    let result = compile_golden("starter.dw");
    assert_eq!(result["outcome"], "accepted", "{result:#}");
    assert_eq!(result["compiled"]["conditions"], Value::Array(Vec::new()), "{result:#}");
}

#[test]
fn a_generic_floor_compiles_against_the_base_schema() {
    let result = compile_golden("floor_generic.dw");
    assert_eq!(result["outcome"], "accepted", "{result:#}");
    assert_eq!(result["compiled"]["conditions"], Value::Array(Vec::new()), "{result:#}");
}

/// Every golden `.dw` name, sorted.
fn golden_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(golden_dir())
        .expect("the golden directory exists")
        .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".dw"))
        .collect();
    names.sort();
    names
}

/// spec §6.2: golden output pinned to COMPILER_GENERATION.
#[test]
fn golden_output_is_pinned_to_the_compiler_generation() {
    let names = golden_names();
    assert!(names.len() >= 9, "{names:?}");
    let mut hasher = blake3::Hasher::new();
    for name in &names {
        let actual = compile_golden(name);
        assert_eq!(actual["outcome"], "accepted", "{name}: every golden must compile: {actual:#}");
        let result_path = golden_dir().join(name.replace(".dw", ".result.json"));
        if write_mode() {
            fs::write(&result_path, serde_json::to_string_pretty(&actual).expect("pretty JSON") + "\n").expect("the result writes");
        }
        assert_eq!(read_json(&result_path), actual, "{} is not current; run with PAIGASUS_WRITE_FIXTURES=1", result_path.display());
        // The hash covers the name and the compact JSON, so a reformat of a file does not move it.
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(serde_json::to_string(&actual).expect("compact JSON").as_bytes());
        hasher.update(&[0]);
    }
    let actual_hash = hasher.finalize().to_hex().to_string();
    assert_eq!(
        (COMPILER_GENERATION, actual_hash.as_str()),
        (3, EXPECTED_GOLDEN_HASH),
        "the golden compiler output changed. Do these three steps in order: \
         (1) bump COMPILER_GENERATION in src/lib.rs; \
         (2) rewrite the result files with PAIGASUS_WRITE_FIXTURES=1 cargo test --locked; \
         (3) replace EXPECTED_GOLDEN_HASH with {actual_hash} and the expected generation literal in this assert with the new COMPILER_GENERATION",
    );
}

#[test]
fn two_policies_keep_their_slots_when_one_is_deleted() {
    let both = compile_golden("stable_slots.dw");
    let source = fs::read_to_string(golden_dir().join("stable_slots.dw")).expect("the golden reads");
    let second = source.split_once("\n\n").expect("two policies, split by a blank line").1;
    let alone = serde_json::to_value(compile_request(&golden_request("stable_slots.dw", second)).expect("compiles")).expect("serializes");
    let slots = |result: &Value, id: &str| -> Vec<Value> {
        result["compiled"]["conditions"]
            .as_array()
            .expect("conditions")
            .iter()
            .filter(|c| c["policy_id"] == id)
            .map(|c| c["slot"].clone())
            .collect()
    };
    let kept = slots(&both, "slots-second");
    assert!(!kept.is_empty(), "{both:#}");
    assert_eq!(slots(&alone, "slots-second"), kept);
}

/// Cedar strict validation types a `has` guard on an undeclared field as `False` and gives only
/// an impossible-policy warning. The compiler rejects such a policy, with its `@id`.
#[test]
fn a_has_guarded_read_of_a_missing_field_is_invalid() {
    let tenant_schema = read_json(&golden_dir().join("tenant_schema.json"));
    let source = "@id(\"guarded-read\")\npermit (principal, action == Pgs::Gateway::Action::\"search\", resource)\nwhen { context.input has zz && context.input.zz == 1 };\n";
    let result = serde_json::to_value(compile_request(&request(SetKind::Tenant, source, tenant_schema.clone())).expect("compiles")).expect("serializes");
    assert_eq!(result["outcome"], "rejected", "{result:#}");
    let rejections = result["rejections"].as_array().expect("rejections");
    assert_eq!(rejections.len(), 1, "{result:#}");
    assert_eq!(rejections[0]["reason"], "tool-policy-invalid");
    assert_eq!(rejections[0]["policy_id"], "guarded-read");
    assert!(rejections[0]["message"].as_str().expect("a message").contains("policy is impossible"), "{result:#}");

    // The guard on a declared field stays accepted.
    let declared = "@id(\"guarded-read\")\npermit (principal, action == Pgs::Gateway::Action::\"search\", resource)\nwhen { context.input has q && context.input.q == \"x\" };\n";
    let result = serde_json::to_value(compile_request(&request(SetKind::Tenant, declared, tenant_schema)).expect("compiles")).expect("serializes");
    assert_eq!(result["outcome"], "accepted", "{result:#}");
}

#[test]
fn the_dogwood_commit_is_the_manifest_pin() {
    let manifest = fs::read_to_string(root().join("Cargo.toml")).expect("the manifest reads");
    assert!(
        manifest.contains(&format!("rev = \"{}\"", paigasus_policy_compiler::DOGWOOD_COMMIT)),
        "DOGWOOD_COMMIT is not the Dogwood rev in Cargo.toml"
    );
}

fn run_binary(args: &[&str], stdin: &[u8]) -> std::process::Output {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    let mut pipe = child.stdin.take().expect("a stdin pipe");
    let input = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        // The binary can exit before it reads all of stdin; a broken pipe is fine here.
        let _ = pipe.write_all(&input);
    });
    let output = child.wait_with_output().expect("the binary exits");
    writer.join().expect("the writer thread");
    output
}

/// The binary gives the same result as `compile_request`. It compares with `compile_request`,
/// not with the `result_*.json` files: `protocol_fixtures.rs` already pins those files, and a
/// write run there must not race this test binary.
#[test]
fn the_binary_round_trips_over_stdin() {
    for name in ["tenant_accepted", "tenant_rejected", "floor_accepted", "floor_rejected"] {
        let request = fs::read(root().join(format!("protocol/fixtures/request_{name}.json"))).expect("the request reads");
        let parsed = parse_request(std::str::from_utf8(&request).expect("UTF-8 request")).expect("the request parses");
        let expected = serde_json::to_value(compile_request(&parsed).expect("compiles")).expect("serializes");
        let output = run_binary(&["compile"], &request);
        assert_eq!(output.status.code(), Some(0), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
        assert_eq!(stdout.matches('\n').count(), 1, "{name}: one JSON line");
        assert!(stdout.ends_with('\n'), "{name}");
        let actual: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn the_binary_exits_2_on_a_bad_request() {
    let mut wrong_version = read_json(&root().join("protocol/fixtures/request_tenant_accepted.json"));
    wrong_version["protocol_version"] = Value::from(2);
    let wrong_version = wrong_version.to_string();
    let mut bad_schema = read_json(&root().join("protocol/fixtures/request_tenant_accepted.json"));
    bad_schema["schema_json"] = serde_json::json!({ "Pgs::Gateway": { "entityTypes": 5 } });
    let bad_schema = bad_schema.to_string();
    for input in [&b"not json"[..], b"{}", &[0xff, 0xfe][..], wrong_version.as_bytes(), bad_schema.as_bytes()] {
        let output = run_binary(&["compile"], input);
        assert_eq!(output.status.code(), Some(2), "{:?}", String::from_utf8_lossy(input));
        assert!(output.stdout.is_empty(), "{:?}", String::from_utf8_lossy(&output.stdout));
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert_eq!(stderr.matches('\n').count(), 1, "one stderr line: {stderr:?}");
    }
}

#[test]
fn an_unknown_argument_exits_64() {
    for args in [&[][..], &["compile", "x"][..], &["help"][..]] {
        let output = run_binary(args, b"");
        assert_eq!(output.status.code(), Some(64), "{args:?}");
    }
}

#[test]
fn revision_prints_generation_and_commit() {
    let output = run_binary(&["revision"], b"");
    assert_eq!(output.status.code(), Some(0));
    let actual: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(
        actual,
        serde_json::json!({ "compiler_generation": 3, "protocol_version": 1, "revision": "3+dogwood.8ffd2106daf0+proto.1" })
    );
    assert_eq!(revision_string(), "3+dogwood.8ffd2106daf0+proto.1");
}
