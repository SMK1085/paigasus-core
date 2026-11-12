// SPDX-License-Identifier: Apache-2.0
//! The golden streams that the real SDKs are checked against (spec § 8.2).
//!
//! `cargo nextest run -p paigasus-gateway-core --test golden` fails when a golden file is stale.
//! `UPDATE_GOLDEN=1` rewrites them. Review the diff before you commit it: a golden change is a
//! change to what every client receives.

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Decision, Format, OnFailure, notice_line};
use serde_json::{Value, json};
use support::driver::{block_named, drive, limits};
use support::oracle::{payloads, runners};

const SET: &[(&str, &str)] = &[
    ("openai", "one_call_blocked"),
    ("openai", "parallel_mixed"),
    ("openai", "n2_choices"),
    ("openai", "text_after_calls"),
    ("anthropic", "tool_use_blocked"),
    ("anthropic", "two_tools_mixed"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The finish reason of each choice, read from the golden output itself: the last non-null
/// `finish_reason` per OpenAI choice index, or the Anthropic `message_delta` `stop_reason` as choice 0.
fn finish_reasons(out: &[u8]) -> Value {
    let mut map = std::collections::BTreeMap::<u64, String>::new();
    for p in payloads(out) {
        let Ok(v) = serde_json::from_slice::<Value>(&p) else { continue };
        if let Some(choices) = v.get("choices").and_then(Value::as_array) {
            for c in choices {
                if let (Some(i), Some(f)) = (c.get("index").and_then(Value::as_u64), c.get("finish_reason").and_then(Value::as_str)) {
                    map.insert(i, f.to_owned());
                }
            }
        } else if let Some(f) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
            map.insert(0, f.to_owned());
        }
    }
    Value::Object(map.into_iter().map(|(k, v)| (k.to_string(), Value::String(v))).collect())
}

fn generate() -> (Vec<(PathBuf, Vec<u8>)>, Value) {
    let mut files = Vec::new();
    let mut manifest = Vec::new();
    for (fmt, name) in SET {
        let format = if *fmt == "openai" { Format::OpenAiChat } else { Format::AnthropicMessages };
        let input = std::fs::read(root().join(format!("fixtures/{fmt}/conforming/{name}.sse"))).expect("fixture exists");
        let o = drive(format, limits(OnFailure::Block), &[&input], &block_named);
        let allowed: Vec<Value> = o
            .decided
            .iter()
            .filter(|(_, d)| d.is_allow())
            .map(|(c, _)| json!({"name": c.name, "arguments": c.arguments, "choice": c.choice}))
            .collect();
        let notices: Vec<String> = o
            .decided
            .iter()
            .filter_map(|(c, d)| match d {
                Decision::Block { code } => Some(notice_line(&c.name, *code)),
                _ => None,
            })
            .collect();
        let notice_choices: Vec<u32> = o.decided.iter().filter(|(_, d)| !d.is_allow()).map(|(c, _)| c.choice).collect();
        let finish = finish_reasons(&o.out);
        assert!(finish.as_object().is_some_and(|m| !m.is_empty()), "{fmt}/{name}: no finish reason in the golden");
        assert!(!notices.is_empty(), "{fmt}/{name}: a golden must block at least one call");
        // Only the clients that read this format are paired with it: openai-node throws on an
        // Anthropic stream, and a model of the other format derives no calls.
        for (model, run) in runners(format) {
            let r = run(&o.out);
            assert!(r.error.is_none(), "{model} cannot parse golden {fmt}/{name}: {:?}", r.error);
            assert_eq!(r.calls.len(), allowed.len(), "{model} on {fmt}/{name}");
        }
        files.push((root().join(format!("golden/{fmt}/{name}.sse")), o.out));
        manifest.push(json!({"file": format!("{fmt}/{name}.sse"), "format": fmt, "allowed": allowed, "notices": notices, "notice_choices": notice_choices, "finish_reasons": finish}));
    }
    (files, Value::Array(manifest))
}

#[test]
fn golden_files_are_current() {
    let (files, manifest) = generate();
    let manifest_path = root().join("golden/manifest.json");
    let manifest_text = format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap());
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        for (path, bytes) in &files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        std::fs::write(&manifest_path, &manifest_text).unwrap();
        return;
    }
    for (path, bytes) in &files {
        let on_disk = std::fs::read(path).unwrap_or_else(|_| panic!("{} is missing: run UPDATE_GOLDEN=1", path.display()));
        assert_eq!(
            String::from_utf8_lossy(&on_disk),
            String::from_utf8_lossy(bytes),
            "{} is stale: run UPDATE_GOLDEN=1 and review the diff",
            path.display()
        );
    }
    assert_eq!(
        std::fs::read_to_string(&manifest_path).unwrap_or_default(),
        manifest_text,
        "manifest.json is stale: run UPDATE_GOLDEN=1"
    );
}
