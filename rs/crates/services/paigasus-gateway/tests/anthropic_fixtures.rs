// SPDX-License-Identifier: Apache-2.0
//! The D21 scrub check for the recorded Anthropic answers (SMA-750 spec § 10.1, D21).
//!
//! The repository is public. A recorded file keeps only four response headers, a fixed
//! `request-id`, and never an Anthropic key.

use std::path::Path;

const KEPT_HEADERS: [&str; 4] = ["content-type", "request-id", "retry-after", "x-should-retry"];
const FIXED_REQUEST_ID: &str = "req_paigasus_fixture";

/// The problems of one fixture file. The key prefix is built from two parts, so that this file
/// does not hold it.
fn scrub_problems(name: &str, text: &str) -> Vec<String> {
    let prefix = format!("{}-", "sk-ant");
    let mut problems = Vec::new();
    if text.contains(&prefix) {
        problems.push(format!("{name}: holds the key prefix"));
    }
    if name.ends_with(".response.json") {
        match serde_json::from_str::<serde_json::Value>(text) {
            Err(e) => problems.push(format!("{name}: not JSON: {e}")),
            Ok(v) => {
                if !v["status"].is_u64() {
                    problems.push(format!("{name}: no numeric status"));
                }
                match v["headers"].as_object() {
                    None => problems.push(format!("{name}: no headers object")),
                    Some(h) => {
                        for (k, val) in h {
                            if !KEPT_HEADERS.contains(&k.as_str()) {
                                problems.push(format!("{name}: header {k} is not a kept header"));
                            }
                            if k == "request-id" && val != FIXED_REQUEST_ID {
                                problems.push(format!("{name}: request-id is not the fixed value"));
                            }
                        }
                    }
                }
            }
        }
    }
    problems
}

#[test]
fn every_anthropic_fixture_is_scrubbed() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anthropic");
    let mut seen = 0;
    let mut responses = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
        let problems = scrub_problems(&name, &text);
        assert!(problems.is_empty(), "{problems:?}");
        seen += 1;
        responses += usize::from(name.ends_with(".response.json"));
    }
    assert!(seen > 0 && responses > 0, "no fixture was scanned");
}

#[test]
fn the_scrub_check_refuses_an_extra_header_and_a_key() {
    let extra = r#"{"status":429,"headers":{"anthropic-ratelimit-requests-limit":"50","request-id":"req_paigasus_fixture"}}"#;
    assert!(!scrub_problems("x.response.json", extra).is_empty());
    let real_id = r#"{"status":200,"headers":{"request-id":"req_011CabC"}}"#;
    assert!(!scrub_problems("x.response.json", real_id).is_empty());
    let key = format!("{}-x", "sk-ant");
    assert!(!scrub_problems("x.body.json", &key).is_empty());
    let good = r#"{"status":200,"headers":{"content-type":"application/json","request-id":"req_paigasus_fixture"}}"#;
    assert!(scrub_problems("x.response.json", good).is_empty());
}
