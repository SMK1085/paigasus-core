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
    ("responses", "function_blocked_deltas"),
    ("responses", "builtin_blocked_added_done"),
    ("responses", "truncated_item"),
    ("responses", "blocked_after_reasoning"),
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

/// The terminal `response.status` of a Responses golden, as choice 0's finish reason.
fn responses_finish_reasons(out: &[u8]) -> Value {
    let status = payloads(out)
        .iter()
        .filter_map(|p| serde_json::from_slice::<Value>(p).ok())
        .filter(|v| matches!(v["type"].as_str(), Some("response.completed" | "response.incomplete" | "response.failed")))
        .filter_map(|v| v.pointer("/response/status").and_then(Value::as_str).map(str::to_owned))
        .next_back();
    json!({"0": status.expect("a terminal response.status in the golden")})
}

/// The notices of a Responses golden, read from the golden itself: the completed `msg_paigasus_<i>`
/// message of every rewritten item, in stream order, with its `output_index`. A truncated item is
/// blocked by `on_failure`, so it never reaches the decider and `decided` does not list it.
fn responses_notices(out: &[u8]) -> Vec<(u64, String)> {
    payloads(out)
        .iter()
        .filter_map(|p| serde_json::from_slice::<Value>(p).ok())
        .filter(|v| v["type"] == "response.output_item.done" && v.pointer("/item/id").and_then(Value::as_str).is_some_and(|id| id.starts_with("msg_paigasus_")))
        .map(|v| {
            (
                v["output_index"].as_u64().expect("output_index"),
                v.pointer("/item/content/0/text").and_then(Value::as_str).expect("notice text").to_owned(),
            )
        })
        .collect()
}

/// One Responses golden (spec § 10.4): the interceptor's output under `block_named` and
/// `on_failure = block`, and its manifest entry.
fn responses_golden(name: &str) -> (PathBuf, Vec<u8>, Value) {
    let input = std::fs::read(root().join(format!("fixtures/responses/conforming/{name}.sse"))).expect("fixture exists");
    let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&input], &block_named);
    let allowed: Vec<Value> = o
        .decided
        .iter()
        .filter(|(_, d)| d.is_allow())
        .map(|(c, _)| json!({"name": c.name, "arguments": c.arguments, "choice": c.choice, "position": c.position, "id": c.id, "namespace": c.namespace}))
        .collect();
    let found = responses_notices(&o.out);
    assert!(!found.is_empty(), "responses/{name}: a golden must block at least one call");
    for (c, d) in &o.decided {
        if let Decision::Block { code } = d {
            let want = notice_line(&c.display_name(), *code, None);
            assert!(
                found.iter().any(|(i, t)| *t == want && *i == u64::from(c.position)),
                "responses/{name}: no notice {want:?} at {}",
                c.position
            );
        }
    }
    for (model, run) in runners(Format::OpenAiResponses) {
        let r = run(&o.out);
        // A client may raise on a stream that does not end in `response.completed` (openai-python
        // does so for `response.incomplete`). It must then raise on the upstream bytes too.
        if let Some(e) = &r.error {
            assert!(run(&input).error.is_some(), "{model} cannot parse golden responses/{name}: {e}");
        }
        assert_eq!(r.calls.len(), allowed.len(), "{model} on responses/{name}");
    }
    let entry = json!({
        "file": format!("responses/{name}.sse"),
        "format": "responses",
        "allowed": allowed,
        "notices": found.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(),
        "notice_choices": found.iter().map(|_| 0).collect::<Vec<u32>>(),
        "notice_positions": found.iter().map(|(i, _)| *i).collect::<Vec<u64>>(),
        "finish_reasons": responses_finish_reasons(&o.out),
    });
    (root().join(format!("golden/responses/{name}.sse")), o.out, entry)
}

fn generate() -> (Vec<(PathBuf, Vec<u8>)>, Value) {
    let mut files = Vec::new();
    let mut manifest = Vec::new();
    for (fmt, name) in SET {
        if *fmt == "responses" {
            let (path, bytes, entry) = responses_golden(name);
            files.push((path, bytes));
            manifest.push(entry);
            continue;
        }
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
                Decision::Block { code } => Some(notice_line(&c.name, *code, None)),
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
