// SPDX-License-Identifier: Apache-2.0
//! Fixture replays (spec § 8.3, AC 2, AC 3, AC 5).

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Format, Metric, OnFailure};
use support::driver::cut;
use support::driver::{DECIDERS, allow_all, block_named, drive, limits};
use support::oracle::{as_model_call, check_i1, payloads, runners};

const SETS: [(Format, &str); 4] = [
    (Format::OpenAiChat, "openai/conforming"),
    (Format::OpenAiChat, "openai/malformed"),
    (Format::AnthropicMessages, "anthropic/conforming"),
    (Format::AnthropicMessages, "anthropic/malformed"),
];

fn dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn files(rel: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sse"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap()))
        .collect();
    v.sort();
    assert!(!v.is_empty(), "{rel} holds no fixtures");
    v
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The CRLF fixture keeps its CR bytes through git (`.gitattributes`).
#[test]
fn the_crlf_fixture_has_crlf_line_endings() {
    let bytes = std::fs::read(dir("openai/conforming/crlf_endings.sse")).unwrap();
    assert!(bytes.windows(2).any(|w| w == b"\r\n"), "crlf_endings.sse lost its CR bytes");
    assert!(
        !bytes.iter().enumerate().any(|(i, b)| *b == b'\n' && (i == 0 || bytes[i - 1] != b'\r')),
        "crlf_endings.sse has a bare LF"
    );
}

/// AC 2: with allow-all, every conforming fixture's data payloads come out byte-equal and in order.
#[test]
fn allow_all_is_payload_identical_for_every_conforming_fixture() {
    for (format, rel) in [(Format::OpenAiChat, "openai/conforming"), (Format::AnthropicMessages, "anthropic/conforming")] {
        for (name, bytes) in files(rel) {
            let o = drive(format, limits(OnFailure::Block), &[&bytes], &allow_all);
            assert_eq!(payloads(&o.out), payloads(&bytes), "{rel}/{name}");
        }
    }
}

/// AC 3: a blocked call's canary never appears, for every fixture and both on_failure modes
/// (except where observe mode passes the stream through by design).
#[test]
fn blocked_canaries_never_appear() {
    let observe_may_leak = ["index_reuse"];
    for (format, rel) in SETS {
        for (name, bytes) in files(rel) {
            let blocked = drive(format, limits(OnFailure::Block), &[&bytes], &block_named);
            assert!(!s(&blocked.out).contains("CANARY"), "{rel}/{name} (block):\n{}", s(&blocked.out));
            let observed = drive(format, limits(OnFailure::Allow), &[&bytes], &block_named);
            if !observe_may_leak.contains(&name.as_str()) {
                assert!(!s(&observed.out).contains("CANARY"), "{rel}/{name} (allow):\n{}", s(&observed.out));
            }
        }
    }
}

/// I1 on every fixture under every decider (see `oracle::check_i1`), and the stream ends. The
/// terminal error frame (D17) makes every OpenAI SDK raise and a dropped `message_start` makes
/// anthropic-python raise; both follow a dropped record, so the client running nothing is the
/// intended fail-closed outcome.
#[test]
fn every_fixture_holds_i1_for_every_client() {
    for (format, rel) in SETS {
        for (name, bytes) in files(rel) {
            for (decider, decide) in DECIDERS {
                for on_failure in [OnFailure::Block, OnFailure::Allow] {
                    let o = drive(format, limits(on_failure), &[&bytes], decide);
                    assert!(o.finished, "{rel}/{name} ({decider}, {on_failure:?}) never ended:\n{}", s(&o.out));
                    if on_failure == OnFailure::Block {
                        check_i1(format, &bytes, &o.out, &o.decided, o.core_failed()).unwrap_or_else(|e| panic!("{rel}/{name} ({decider}): {e}"));
                    }
                }
            }
        }
    }
}

/// § 8.3: every single split point gives the same output as one push, for fixtures up to 4 KiB.
#[test]
fn every_split_point_gives_the_same_output() {
    for (format, rel) in SETS {
        for (name, bytes) in files(rel) {
            if bytes.len() > 4096 {
                continue;
            }
            let whole = drive(format, limits(OnFailure::Block), &[&bytes], &block_named).out;
            for i in 0..=bytes.len() {
                let split = drive(format, limits(OnFailure::Block), &[&bytes[..i], &bytes[i..]], &block_named).out;
                assert_eq!(s(&split), s(&whole), "{rel}/{name} split at {i}");
            }
        }
    }
}

/// One AC 5 row: what the output must and must not hold under each `on_failure`, and whether
/// the shape is a stream-level failure (a malformed record mid-hold or a canonical-form deviation,
/// § 6 row 1) rather than a record dropped while nothing is held.
struct Row {
    name: &'static str,
    block_has: &'static [&'static str],
    block_lacks: &'static [&'static str],
    allow_has: &'static [&'static str],
    allow_lacks: &'static [&'static str],
    stream_failure: bool,
}

const fn row(
    name: &'static str,
    block_has: &'static [&'static str],
    block_lacks: &'static [&'static str],
    allow_has: &'static [&'static str],
    allow_lacks: &'static [&'static str],
    stream_failure: bool,
) -> Row {
    Row {
        name,
        block_has,
        block_lacks,
        allow_has,
        allow_lacks,
        stream_failure,
    }
}

const PASS_THROUGH: &str = ": paigasus-interception pass-through";

/// AC 5: each malformed shape shows the § 6 behaviour under both on_failure values.
///
/// Under `block`, a stream-level failure blocks every open and later hold (`(malformed)` notice
/// where a hold was open) and drops later tool-bearing frames, and the stream still ends with its
/// end marker. Under `allow` it switches to pass-through. A JSON body on a stream holds no SSE
/// record, so nothing is classified and nothing is forwarded in either mode.
#[test]
fn malformed_shapes_follow_section_6() {
    let openai: &[Row] = &[
        row("nan_record", &["\"b\""], &[], &["\"b\""], &[], false),
        row("duplicate_key_mid_hold", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("case_folded_key", &["\"ok\""], &[], &["\"ok\""], &[], false),
        row("proto_key", &["\"ok\""], &[], &["\"ok\""], &[], false),
        row("float_index", &["\"ok\""], &[], &["\"ok\""], &[], false),
        row("repeated_name", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("fragment_after_bodyless_entry", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("chunk_without_choices", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("choice_without_delta", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("choice_without_delta_in_opening_record", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("type_null", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("chunk_key", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("content_delta_type", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("array_outside_tool_calls", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("oversized", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("after_done", &["data: [DONE]"], &[], &["data: [DONE]"], &[], false),
        row("error_mid_hold", &["\"terminal\""], &[], &["\"terminal\""], &[], false),
        row("json_body", &[], &["data:", "blocked_json"], &[], &["data:", "blocked_json"], false),
        row("lax_line", &["\"ok\""], &[], &["\"ok\""], &[], false),
    ];
    let anthropic: &[Row] = &[
        row(
            "event_mismatch",
            &["event: message_start", "event: message_stop"],
            &["content_block", "blocked_hidden"],
            &[PASS_THROUGH],
            &[],
            true,
        ),
        row(
            "prefilled_input",
            &["event: message_start", "event: message_stop"],
            &["content_block", "blocked_hidden"],
            &[PASS_THROUGH],
            &[],
            true,
        ),
        row("index_reuse", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        // The deviating `message_start` itself is dropped; the stream still ends.
        row("prefilled_message", &["event: message_stop"], &["message_start", "blocked_hidden"], &[PASS_THROUGH], &[], true),
    ];
    for (format, rel, table) in [(Format::OpenAiChat, "openai/malformed", openai), (Format::AnthropicMessages, "anthropic/malformed", anthropic)] {
        let all = files(rel);
        assert_eq!(all.len(), table.len(), "every {rel} fixture has a row");
        for r in table {
            let name = r.name;
            let bytes = &all.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no fixture {name}")).1;
            let blocked = drive(format, limits(OnFailure::Block), &[bytes], &block_named);
            let b = s(&blocked.out);
            for want in r.block_has {
                assert!(b.contains(want), "{rel}/{name} (block) lacks {want:?}:\n{b}");
            }
            for bad in r.block_lacks {
                assert!(!b.contains(bad), "{rel}/{name} (block) holds {bad:?}:\n{b}");
            }
            let failed = blocked.metrics.contains(&Metric::OnFailure("malformed"));
            assert_eq!(failed, r.stream_failure, "{rel}/{name}: stream-level failure");
            let a = s(&drive(format, limits(OnFailure::Allow), &[bytes], &block_named).out);
            for want in r.allow_has {
                assert!(a.contains(want), "{rel}/{name} (allow) lacks {want:?}:\n{a}");
            }
            for bad in r.allow_lacks {
                assert!(!a.contains(bad), "{rel}/{name} (allow) holds {bad:?}:\n{a}");
            }
        }
    }
}

/// The split points to test for `bytes`. Every point costs three client runs (two processes
/// each), and a full sweep of the 12 recordings took over 100 s in a debug build. So a file up to
/// 4 KiB gets every point. A larger file gets a stride of 7 plus every record boundary (each
/// `\n\n` offset, and the offsets on both sides of it), where a split is most likely to matter.
fn split_points(bytes: &[u8]) -> Vec<usize> {
    if bytes.len() <= 4096 {
        return (0..=bytes.len()).collect();
    }
    let mut v: Vec<usize> = (0..=bytes.len()).step_by(7).collect();
    for (i, w) in bytes.windows(2).enumerate() {
        if w == b"\n\n" {
            v.extend([i, i + 1, i + 2]);
        }
    }
    v.push(bytes.len());
    v.sort_unstable();
    v.dedup();
    v.retain(|&i| i <= bytes.len());
    v
}

/// Real OpenAI recordings (2026-11-14) pass the interceptor unchanged: no record is dropped, no
/// `on_failure` path runs, and every client derives the decided calls from the output.
#[test]
fn recorded_streams_pass_unchanged() {
    for (name, bytes) in files("openai/recorded") {
        for i in split_points(&bytes) {
            let o = drive(Format::OpenAiChat, limits(OnFailure::Block), &cut(&bytes, &[i]), &allow_all);
            let ctx = format!("recorded/{name} split at {i}");
            assert!(o.finished, "{ctx} never ended");
            assert!(!o.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_) | Metric::OnFailure(_))), "{ctx}: {:?}", o.metrics);
            check_i1(Format::OpenAiChat, &bytes, &o.out, &o.decided, o.core_failed()).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let want: Vec<(&str, String)> = o.decided.iter().map(|(c, _)| as_model_call(c)).map(|m| (m.kind, m.name)).collect();
            assert!(!want.is_empty(), "{ctx}: no tool call was decided");
            for (model, run) in runners(Format::OpenAiChat) {
                let r = run(&o.out);
                if let Some(e) = &r.error {
                    // Only LangChain on a recording with a `custom` call may raise: its model
                    // raises on any custom entry (oracle.rs), also on the recorded bytes.
                    let excused = *model == "langchain" && want.iter().any(|(k, _)| *k == "custom") && run(&bytes).error.is_some();
                    assert!(excused, "{ctx}: {model} raised: {e}");
                    continue;
                }
                let got: Vec<(&str, String)> = r.calls.iter().map(|m| (m.kind, m.name.clone())).collect();
                assert_eq!(got, want, "{ctx}: {model}");
            }
        }
    }
}
