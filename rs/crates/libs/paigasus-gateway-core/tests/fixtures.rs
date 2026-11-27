// SPDX-License-Identifier: Apache-2.0
//! Fixture replays (spec § 8.3, AC 2, AC 3, AC 5).

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Format, Metric, OnFailure};
use support::driver::{DECIDERS, Outcome, allow_all, block_named, drive, limits};
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
    // A stream-level failure under `allow` passes the held records through, the blocked ones too.
    let observe_may_leak = ["index_reuse", "fragment_after_blocked_bodyless_entry"];
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
        row("fragment_after_blocked_bodyless_entry", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("chunk_without_choices", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("choice_without_delta", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("choice_without_delta_in_opening_record", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("type_null", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("chunk_key", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("content_delta_type", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("array_outside_tool_calls", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
        row("function_call_key", &["(malformed)"], &[], &[PASS_THROUGH], &[], true),
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

/// Real OpenAI recordings (2026-11-14) pass the interceptor unchanged (spec § 7.3).
///
/// One push per file and decider: the stream ends, no record is dropped, no `on_failure` path
/// runs, and I1 holds. Under `allow-all` every client also derives the decided calls from the
/// output, and the output is payload-identical to the input. Then every single split point under
/// `allow-all`: the output must equal the one-push output, so it is payload-identical to the
/// input too, and the client runs of the one-push pass hold for it.
#[test]
fn recorded_streams_pass_unchanged() {
    let clean = |o: &Outcome| !o.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_) | Metric::OnFailure(_)));
    let mut splits = Vec::new();
    for (name, bytes) in files("openai/recorded") {
        for (decider, decide) in DECIDERS {
            let o = drive(Format::OpenAiChat, limits(OnFailure::Block), &[&bytes], decide);
            let ctx = format!("recorded/{name} ({decider})");
            assert!(o.finished, "{ctx} never ended");
            assert!(clean(&o), "{ctx}: {:?}", o.metrics);
            check_i1(Format::OpenAiChat, &bytes, &o.out, &o.decided, o.core_failed()).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            if *decider != "allow-all" {
                continue;
            }
            assert_eq!(payloads(&o.out), payloads(&bytes), "{ctx}");
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
        // The one-push output is payload-identical to the input (checked above), so a split whose
        // output equals it byte for byte is payload-identical too.
        splits.push((name, bytes));
    }
    // About 74 000 drives in a debug build: one thread per file, so the files run in parallel.
    std::thread::scope(|scope| {
        for (name, bytes) in &splits {
            scope.spawn(move || {
                let whole = drive(Format::OpenAiChat, limits(OnFailure::Block), &[bytes], &allow_all).out;
                for i in 0..=bytes.len() {
                    let o = drive(Format::OpenAiChat, limits(OnFailure::Block), &[&bytes[..i], &bytes[i..]], &allow_all);
                    let ctx = format!("recorded/{name} split at {i}");
                    assert!(o.finished, "{ctx} never ended");
                    assert!(clean(&o), "{ctx}: {:?}", o.metrics);
                    assert!(o.out == whole, "{ctx}: the output differs from the one-push output");
                }
            });
        }
    });
}

// ---- OpenAI Responses (SMA-769 spec § 10.4) ---------------------------------------------------------
//
// The Responses sets have their own functions, so the chat and Anthropic tests above stay as they
// are. `responses/edge` holds the cases whose outcome is not a `Malformed` stream failure (an
// upstream error, a truncation, a limit, a record after the terminal event, the SSE layer).

const RESPONSES_SETS: [&str; 3] = ["responses/conforming", "responses/malformed", "responses/edge"];

/// The one conforming fixture that allow-all still rewrites: its call is cut (`status:
/// "incomplete"`), which is blocked as `Truncated` whatever the decider says (A3, ruling F6).
const ALLOW_ALL_REWRITES: [&str; 1] = ["truncated_item"];

fn limits_for_responses(on_failure: OnFailure) -> paigasus_gateway_core::Limits {
    paigasus_gateway_core::Limits {
        max_record_bytes: 16_777_216,
        max_held_bytes: 33_554_432,
        on_failure,
        ..paigasus_gateway_core::Limits::defaults()
    }
}

/// The lines of `bytes` that start with `prefix`.
fn lines_with(bytes: &[u8], prefix: &str) -> Vec<String> {
    s(bytes).lines().filter(|l| l.starts_with(prefix)).map(str::to_owned).collect()
}

/// The item types of the provider-run items in a stream (spec § 6.2): each one is reported once as
/// an `unenforceable` comment.
fn provider_run_items(bytes: &[u8]) -> usize {
    const PROVIDER: [&str; 5] = ["web_search_call", "file_search_call", "code_interpreter_call", "image_generation_call", "mcp_call"];
    payloads(bytes)
        .iter()
        .filter_map(|p| serde_json::from_slice::<serde_json::Value>(p).ok())
        .filter(|v| v["type"] == "response.output_item.added" && v.pointer("/item/type").and_then(|t| t.as_str()).is_some_and(|t| PROVIDER.contains(&t)))
        .count()
}

/// AC 6: with allow-all, every conforming Responses fixture comes out with the same data payloads,
/// the same `event:` lines, and nothing else but one `unenforceable` comment per provider-run item.
#[test]
fn responses_allow_all_is_payload_identical_for_every_conforming_fixture() {
    for (name, bytes) in files("responses/conforming") {
        let o = drive(Format::OpenAiResponses, limits_for_responses(OnFailure::Block), &[&bytes], &allow_all);
        if ALLOW_ALL_REWRITES.contains(&name.as_str()) {
            assert!(s(&o.out).contains("(truncated)."), "{name}: a cut call must be rewritten:\n{}", s(&o.out));
            assert!(o.metrics.contains(&Metric::OnFailure("truncated")), "{name}: {:?}", o.metrics);
            continue;
        }
        assert_eq!(payloads(&o.out), payloads(&bytes), "{name}");
        assert_eq!(lines_with(&o.out, "event: "), lines_with(&bytes, "event: "), "{name}: the event lines");
        // The input has no comment. The output has one decision comment per client-run call
        // (`allow`) and one `unenforceable` comment per provider-run item.
        let comments = lines_with(&o.out, ":");
        assert!(lines_with(&bytes, ":").is_empty(), "{name}: the input has a comment");
        assert!(comments.iter().all(|c| c.starts_with(": paigasus-decision ")), "{name}: {comments:?}");
        let unenforceable = comments.iter().filter(|c| c.contains("\"decision\":\"unenforceable\"")).count();
        assert_eq!(unenforceable, provider_run_items(&bytes), "{name}: one comment per provider-run item:\n{comments:?}");
        assert!(
            comments.iter().all(|c| c.contains("\"decision\":\"unenforceable\"") || c.contains("\"decision\":\"allow\"")),
            "{name}: {comments:?}"
        );
        assert!(!o.core_failed(), "{name}: {:?}", o.metrics);
    }
}

/// AC 3: a blocked call's canary never appears, for every Responses fixture. Observe mode passes
/// the held records through after a stream-level failure by design, so it is only checked where the
/// run took no failure path.
#[test]
fn responses_blocked_canaries_never_appear() {
    for rel in RESPONSES_SETS {
        for (name, bytes) in files(rel) {
            let blocked = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes], &block_named);
            assert!(!s(&blocked.out).contains("CANARY"), "{rel}/{name} (block):\n{}", s(&blocked.out));
            let observed = drive(Format::OpenAiResponses, limits(OnFailure::Allow), &[&bytes], &block_named);
            if !observed.core_failed() {
                assert!(!s(&observed.out).contains("CANARY"), "{rel}/{name} (allow):\n{}", s(&observed.out));
            }
        }
    }
}

/// I1 for the four Responses client models on every fixture, under every decider and both
/// `on_failure` modes; the stream always ends.
#[test]
fn responses_every_fixture_holds_i1_for_every_client() {
    for rel in RESPONSES_SETS {
        for (name, bytes) in files(rel) {
            for (decider, decide) in DECIDERS {
                for on_failure in [OnFailure::Block, OnFailure::Allow] {
                    let o = drive(Format::OpenAiResponses, limits(on_failure), &[&bytes], decide);
                    assert!(o.finished, "{rel}/{name} ({decider}, {on_failure:?}) never ended:\n{}", s(&o.out));
                    if on_failure == OnFailure::Block {
                        // An upstream error record inside a hold ends the stream with the D13 frame (D17),
                        // which openai-python reports at EOF: a failure path, as in properties.rs.
                        check_i1(Format::OpenAiResponses, &bytes, &o.out, &o.decided, o.core_failed() || o.aborted).unwrap_or_else(|e| panic!("{rel}/{name} ({decider}): {e}"));
                    }
                }
            }
        }
    }
}

/// § 8.3: every split point gives the same output as one push, for every Responses fixture. A
/// fixture over 6 KiB (the limit paths: `hold_limit`, `oversized_*`) gets the points of
/// [`split_points`]. One thread per fixture.
#[test]
fn responses_every_split_point_gives_the_same_output() {
    let mut total = 0;
    let mut covered = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for rel in RESPONSES_SETS {
            for (name, bytes) in files(rel) {
                total += 1;
                let covered = &covered;
                scope.spawn(move || {
                    let whole = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes], &block_named).out;
                    let points = if bytes.len() > 6144 { split_points(&bytes) } else { (0..=bytes.len()).collect() };
                    for i in points {
                        let split = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&bytes[..i], &bytes[i..]], &block_named).out;
                        assert!(split == whole, "{rel}/{name} split at {i}");
                    }
                    covered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                });
            }
        }
    });
    assert_eq!(*covered.get_mut(), total, "every fixture is split-tested");
}

/// AC 5 for Responses: each malformed shape follows § 6 under both `on_failure` values.
///
/// A fixture that names `(malformed)` in `block_has` had a hold open at the bad record, so that
/// hold is blocked with the `malformed` code. A stream-level failure with no hold open (a bad
/// lifecycle record) blocks nothing yet: later holds are blocked, and the stream still ends.
#[test]
fn responses_malformed_shapes_follow_section_6() {
    let m: &[&str] = &["(malformed)"];
    let none: &[&str] = &[];
    let pt: &[&str] = &[PASS_THROUGH];
    let rows: Vec<Row> = [
        "added_arguments_not_empty",
        "added_not_subset",
        "added_not_subset_value",
        "arguments_done_mismatch",
        "arguments_mismatch",
        "delta_before_added",
        "diff_mismatch",
        "done_trailing",
        "duplicate_call_id",
        "duplicate_id",
        "event_after_done",
        "event_error_name",
        "index_gap",
        "input_mismatch",
        "invalid_json_arguments",
        "item_id_mismatch",
        "item_interleaved",
        "item_interleaved_no_done",
        "item_null",
        "repeated_created",
        "repeated_index",
        "reserved_id",
        "sequence_not_increasing",
        "shell_command_mismatch",
        "terminal_args_differ",
        "terminal_item_never_streamed",
        "terminal_item_never_streamed_then_unknown_record",
        "terminal_new_name",
        "terminal_new_namespace",
        "terminal_output_absent",
        "terminal_output_null",
        "type_changes_at_done",
        "type_changes_in_terminal",
        "unknown_type",
        "wrong_item_type_event",
    ]
    .into_iter()
    .map(|n| row(n, m, &["blocked_hidden"], pt, none, true))
    .chain([
        // The bad record is a lifecycle record: no hold is open, so no hold is blocked by it.
        row("early_output", none, &["blocked_hidden"], pt, none, true),
        row("first_not_created", none, &["blocked_hidden"], pt, none, true),
        row("response_id_changed", none, &["blocked_hidden"], pt, none, true),
        // The renamed record is dropped before any hold: the stream goes on, and the blocked call
        // after it is blocked as usual.
        row("event_error_name_before_hold", &["(policy)", "event: response.created"], &["event: error"], none, none, false),
    ])
    .collect();
    let all = files("responses/malformed");
    assert_eq!(all.len(), rows.len(), "every responses/malformed fixture has a row");
    for r in &rows {
        let name = r.name;
        let bytes = &all.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no fixture {name}")).1;
        let blocked = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[bytes], &block_named);
        let b = s(&blocked.out);
        assert!(blocked.finished, "{name} never ended");
        for want in r.block_has {
            assert!(b.contains(want), "responses/malformed/{name} (block) lacks {want:?}:\n{b}");
        }
        for bad in r.block_lacks {
            assert!(!b.contains(bad), "responses/malformed/{name} (block) holds {bad:?}:\n{b}");
        }
        let failed = blocked.metrics.contains(&Metric::OnFailure("malformed"));
        assert_eq!(failed, r.stream_failure, "responses/malformed/{name}: stream-level failure");
        let a = s(&drive(Format::OpenAiResponses, limits(OnFailure::Allow), &[bytes], &block_named).out);
        for want in r.allow_has {
            assert!(a.contains(want), "responses/malformed/{name} (allow) lacks {want:?}:\n{a}");
        }
        for bad in r.allow_lacks {
            assert!(!a.contains(bad), "responses/malformed/{name} (allow) holds {bad:?}:\n{a}");
        }
    }
}

/// One row of the `responses/edge` table: the failure path the fixture reaches (a metric), or an
/// abort with the D13 frame, or neither.
struct EdgeRow {
    name: &'static str,
    metrics: &'static [Metric],
    aborted: bool,
    block_has: &'static [&'static str],
    block_lacks: &'static [&'static str],
}

/// The cases of `responses/edge`, each with the outcome of its own rule (not `Malformed`): an
/// upstream error aborts an open hold (D17); a truncation blocks the call as `truncated` (R15, A3);
/// a limit blocks as `hold_limit` or drops an `oversized` record (D13, D16); a record after the
/// terminal event is dropped and counted (R3); a BOM, a missing final blank line, and an absent
/// `status` on a custom call (which has none in the model) are clean.
#[test]
fn responses_edge_shapes_follow_section_6() {
    const D13: &str = "\"code\":\"upstream-error\"";
    let rows = [
        EdgeRow {
            name: "top_level_error",
            metrics: &[],
            aborted: true,
            block_has: &[D13],
            block_lacks: &["blocked_hidden"],
        },
        EdgeRow {
            name: "error_event_mid_item",
            metrics: &[],
            aborted: true,
            block_has: &[D13],
            block_lacks: &[],
        },
        EdgeRow {
            name: "record_after_terminal",
            metrics: &[Metric::RecordDropped("after_done")],
            aborted: false,
            block_has: &["event: response.completed"],
            block_lacks: &["fc_hidden"],
        },
        EdgeRow {
            name: "failed_with_open_item",
            metrics: &[Metric::OnFailure("truncated")],
            aborted: false,
            block_has: &["(truncated)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "eof_in_item",
            metrics: &[Metric::OnFailure("truncated")],
            aborted: false,
            block_has: &["(truncated)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "no_terminal",
            metrics: &[Metric::OnFailure("truncated")],
            aborted: false,
            block_has: &["(truncated)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "absent_status_function",
            metrics: &[Metric::OnFailure("truncated")],
            aborted: false,
            block_has: &["(truncated)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "absent_status_custom",
            metrics: &[],
            aborted: false,
            block_has: &["(policy)", "\"name\":\"note\""],
            block_lacks: &[],
        },
        EdgeRow {
            name: "oversized_delta",
            metrics: &[Metric::RecordDropped("oversized")],
            aborted: false,
            block_has: &["(malformed)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "oversized_terminal",
            metrics: &[Metric::RecordDropped("oversized")],
            aborted: false,
            block_has: &["(malformed)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "hold_limit",
            metrics: &[Metric::OnFailure("hold_limit")],
            aborted: false,
            block_has: &["(hold-limit)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "bom",
            metrics: &[],
            aborted: false,
            block_has: &["(policy)"],
            block_lacks: &[],
        },
        EdgeRow {
            name: "no_final_blank_line",
            metrics: &[],
            aborted: false,
            block_has: &["(policy)"],
            block_lacks: &[],
        },
    ];
    let all = files("responses/edge");
    assert_eq!(all.len(), rows.len(), "every responses/edge fixture has a row");
    for r in &rows {
        let bytes = &all.iter().find(|(n, _)| n == r.name).unwrap_or_else(|| panic!("no fixture {}", r.name)).1;
        let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[bytes], &block_named);
        let out = s(&o.out);
        assert!(o.finished, "{} never ended", r.name);
        assert_eq!(o.aborted, r.aborted, "{}: abort\n{out}", r.name);
        for m in r.metrics {
            assert!(o.metrics.contains(m), "{}: lacks {m:?}: {:?}", r.name, o.metrics);
        }
        if r.metrics.is_empty() && !r.aborted {
            assert!(!o.core_failed(), "{}: {:?}", r.name, o.metrics);
        }
        for want in r.block_has {
            assert!(out.contains(want), "{} (block) lacks {want:?}:\n{out}", r.name);
        }
        for bad in r.block_lacks {
            assert!(!out.contains(bad), "{} (block) holds {bad:?}:\n{out}", r.name);
        }
    }
}

/// A stream with a `response.created` and a `response.completed` of 2 MiB each passes, under the
/// Responses limits (16 MiB per record, 32 MiB held): spec M7 measured 60 KB, and D15 allows far more.
#[test]
fn a_large_lifecycle_event_passes() {
    let big = "x".repeat(2 * 1024 * 1024);
    let ev = |v: serde_json::Value| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap());
    let call = serde_json::json!({"type": "function_call", "id": "fc_0", "status": "completed", "call_id": "call_0", "name": "read_file", "arguments": "{}"});
    let mut stream = String::new();
    stream +=
        &ev(serde_json::json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "object": "response", "status": "in_progress", "instructions": big, "output": []}}));
    stream += &ev(
        serde_json::json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"type": "function_call", "id": "fc_0", "status": "in_progress", "call_id": "call_0", "name": "read_file", "arguments": ""}}),
    );
    stream += &ev(serde_json::json!({"type": "response.function_call_arguments.delta", "sequence_number": 2, "output_index": 0, "item_id": "fc_0", "delta": "{}"}));
    stream += &ev(serde_json::json!({"type": "response.function_call_arguments.done", "sequence_number": 3, "output_index": 0, "item_id": "fc_0", "arguments": "{}"}));
    stream += &ev(serde_json::json!({"type": "response.output_item.done", "sequence_number": 4, "output_index": 0, "item": call}));
    stream +=
        &ev(serde_json::json!({"type": "response.completed", "sequence_number": 5, "response": {"id": "resp_1", "object": "response", "status": "completed", "instructions": big, "output": [call]}}));
    assert!(stream.len() > 4 * 1024 * 1024);
    for decide in [&allow_all as &dyn Fn(&paigasus_gateway_core::ToolCall) -> paigasus_gateway_core::Decision, &block_named] {
        let o = drive(Format::OpenAiResponses, limits_for_responses(OnFailure::Block), &[stream.as_bytes()], decide);
        assert!(o.finished && !o.core_failed() && !o.aborted, "{:?}", o.metrics);
        assert_eq!(payloads(&o.out), payloads(stream.as_bytes()));
    }
}

/// Spec § 7.1: a structured-output schema with properties `Name`, `ID`, `Type` and `constructor`, a
/// metadata key `Id` and a schema `maximum: 1e300` pass unchanged on the stream.
#[test]
fn structured_output_keys_pass_on_the_stream() {
    let bytes = std::fs::read(dir("responses/conforming/structured_output_keys.sse")).unwrap();
    let text = s(&bytes);
    for key in ["\"Name\"", "\"ID\"", "\"Type\"", "\"constructor\"", "\"Id\"", "1e+300"] {
        assert!(text.contains(key), "the fixture lacks {key}");
    }
    for decide in [&allow_all as &dyn Fn(&paigasus_gateway_core::ToolCall) -> paigasus_gateway_core::Decision, &block_named] {
        let o = drive(Format::OpenAiResponses, limits_for_responses(OnFailure::Block), &[&bytes], decide);
        assert!(o.finished && !o.core_failed(), "{:?}", o.metrics);
        assert!(o.out == bytes, "the stream changed:\n{}", s(&o.out));
    }
}

// ---- Responses recordings (spec § 10.5) -----------------------------------------------------------

/// Whether a recorded file is one of the cut streams (`max_output_tokens` inside a call).
fn is_cut(name: &str) -> bool {
    name.ends_with("__max_output_tokens_in_call")
}

/// The split points to try for a recording. A file up to 12 KiB gets every point. A larger one (up
/// to 185 KB, where one drive costs milliseconds in a debug build, and the cost of all points grows
/// with the square of the size) gets every point within 48 bytes of a record boundary and of both
/// ends, and every 499th point.
fn split_points(bytes: &[u8]) -> Vec<usize> {
    if bytes.len() <= 12 * 1024 {
        return (0..=bytes.len()).collect();
    }
    let mut at = std::collections::BTreeSet::new();
    let near = |at: &mut std::collections::BTreeSet<usize>, c: usize| at.extend(c.saturating_sub(48)..=(c + 48).min(bytes.len()));
    near(&mut at, 0);
    near(&mut at, bytes.len());
    for (i, w) in bytes.windows(2).enumerate() {
        if w == b"\n\n" {
            near(&mut at, i + 2);
        }
    }
    at.extend((0..=bytes.len()).step_by(499));
    at.into_iter().collect()
}

/// Real OpenAI Responses recordings pass the interceptor unchanged (spec § 10.5), but the cut
/// streams (see [`recorded_responses_cut_streams_are_rewritten`]).
///
/// One push per file and decider: the stream ends, no record is dropped, no `on_failure` path
/// runs, and I1 holds for every client model. Under allow-all the output is payload-identical to
/// the input. Then every split point under allow-all (see [`split_points`]) gives the one-push output.
#[test]
fn recorded_responses_streams_pass_unchanged() {
    let clean = |o: &Outcome| !o.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_) | Metric::OnFailure(_)));
    let l = || limits_for_responses(OnFailure::Block);
    let mut kept = Vec::new();
    for (name, bytes) in files("responses/recorded") {
        if is_cut(&name) {
            continue;
        }
        for (decider, decide) in DECIDERS {
            let o = drive(Format::OpenAiResponses, l(), &[&bytes], decide);
            let ctx = format!("responses/recorded/{name} ({decider})");
            assert!(o.finished, "{ctx} never ended");
            assert!(clean(&o) && !o.aborted, "{ctx}: {:?}", o.metrics);
            check_i1(Format::OpenAiResponses, &bytes, &o.out, &o.decided, false).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            if *decider == "allow-all" {
                assert_eq!(payloads(&o.out), payloads(&bytes), "{ctx}");
                assert_eq!(lines_with(&o.out, "event: "), lines_with(&bytes, "event: "), "{ctx}: the event lines");
            }
        }
        kept.push((name, bytes));
    }
    assert!(kept.len() >= 17, "only {} recordings are replayed", kept.len());
    std::thread::scope(|scope| {
        for (name, bytes) in &kept {
            scope.spawn(move || {
                let whole = drive(Format::OpenAiResponses, l(), &[bytes], &allow_all).out;
                for i in split_points(bytes) {
                    let o = drive(Format::OpenAiResponses, l(), &[&bytes[..i], &bytes[i..]], &allow_all);
                    let ctx = format!("responses/recorded/{name} split at {i}");
                    assert!(o.finished, "{ctx} never ended");
                    assert!(clean(&o), "{ctx}: {:?}", o.metrics);
                    assert!(o.out == whole, "{ctx}: the output differs from the one-push output");
                }
            });
        }
    });
}

/// The recorded `max_output_tokens_in_call` streams end with a cut function call: `done` with
/// `status: "incomplete"`, partial arguments, then `response.incomplete` (spec § 3.3 M5). The
/// interceptor blocks it as `Truncated` even under allow-all, and rewrites it as § 6.5 says
/// (amendment A3, ruling F6): the exact four events, with the `sequence_number` of the replaced
/// frame, and the completed message at the blocked index of the terminal `output`.
#[test]
fn recorded_responses_cut_streams_are_rewritten() {
    use serde_json::{Value, json};
    let cut: Vec<_> = files("responses/recorded").into_iter().filter(|(n, _)| is_cut(n)).collect();
    assert!(cut.len() >= 3, "only {} cut recordings", cut.len());
    for (name, bytes) in cut {
        let ctx = format!("responses/recorded/{name}");
        let events: Vec<Value> = payloads(&bytes).iter().map(|p| serde_json::from_slice(p).unwrap()).collect();
        let done = events
            .iter()
            .find(|v| v["type"] == "response.output_item.done" && v.pointer("/item/type") == Some(&json!("function_call")) && v.pointer("/item/status") == Some(&json!("incomplete")))
            .unwrap_or_else(|| panic!("{ctx}: no cut call"));
        let i = done["output_index"].as_u64().unwrap();
        let fname = done.pointer("/item/name").and_then(Value::as_str).unwrap().to_owned();
        let seq = |ty: &str| events.iter().find(|v| v["type"] == ty && v["output_index"] == i).map(|v| v["sequence_number"].clone()).unwrap();
        let (s_added, s_done) = (seq("response.output_item.added"), done["sequence_number"].clone());
        let notice = format!("[paigasus] tool call \"{fname}\" was blocked (truncated).");
        let id = format!("msg_paigasus_{i}");
        let message = json!({"type": "message", "id": id, "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": notice, "annotations": [], "logprobs": []}]});
        let want = [
            json!({"type": "response.output_item.added", "output_index": i, "sequence_number": s_added, "item": {"type": "message", "id": id, "role": "assistant", "status": "in_progress", "content": []}}),
            json!({"type": "response.content_part.added", "item_id": id, "output_index": i, "content_index": 0, "sequence_number": s_added, "part": {"type": "output_text", "text": "", "annotations": [], "logprobs": []}}),
            json!({"type": "response.output_text.delta", "item_id": id, "output_index": i, "content_index": 0, "sequence_number": s_added, "delta": notice, "logprobs": []}),
            json!({"type": "response.output_item.done", "output_index": i, "sequence_number": s_done, "item": message}),
        ];
        for (decider, decide) in DECIDERS {
            for (label, on_failure) in [("block", OnFailure::Block)] {
                let o = drive(Format::OpenAiResponses, limits_for_responses(on_failure), &[&bytes], decide);
                let ctx = format!("{ctx} ({decider}, {label})");
                assert!(o.finished, "{ctx} never ended");
                assert!(o.metrics.contains(&Metric::OnFailure("truncated")), "{ctx}: {:?}", o.metrics);
                assert!(!o.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_))), "{ctx}: {:?}", o.metrics);
                let out: Vec<Value> = payloads(&o.out).iter().map(|p| serde_json::from_slice(p).unwrap()).collect();
                let at = out.iter().position(|v| *v == want[0]).unwrap_or_else(|| panic!("{ctx}: no rewritten added event"));
                assert_eq!(&out[at..at + 4], &want[..], "{ctx}: the four events of § 6.5");
                // No other record names the cut call: its deltas and its `done` are gone.
                assert!(
                    !s(&o.out).contains(done.pointer("/item/call_id").and_then(Value::as_str).unwrap())
                        || out.iter().all(|v| v["type"] != "response.function_call_arguments.delta" || v["output_index"] != i),
                    "{ctx}"
                );
                assert!(
                    out.iter().all(|v| v["output_index"] != i || v.pointer("/item/id") == Some(&json!(id)) || v["item_id"] == json!(id)),
                    "{ctx}: a record of the cut call survives"
                );
                // The terminal event keeps every upstream field but the replaced item.
                let end_in = events.last().unwrap();
                let end_out = out.last().unwrap();
                assert_eq!(end_out["type"], end_in["type"], "{ctx}");
                assert_eq!(end_out["response"]["output"][i as usize], message, "{ctx}");
                for key in ["status", "usage", "incomplete_details", "error", "id"] {
                    assert_eq!(end_out["response"][key], end_in["response"][key], "{ctx}: {key}");
                }
                // The records before and after the cut call are unchanged and in order.
                let kept_in: Vec<&Value> = events.iter().filter(|v| v["output_index"] != i && v["type"] != end_in["type"]).collect();
                let kept_out: Vec<&Value> = out.iter().filter(|v| v["output_index"] != i && v["type"] != end_in["type"]).collect();
                assert_eq!(kept_out, kept_in, "{ctx}: the other records");
                check_i1(Format::OpenAiResponses, &bytes, &o.out, &o.decided, true).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            }
        }
        // Every split point gives the one-push output.
        let whole = drive(Format::OpenAiResponses, limits_for_responses(OnFailure::Block), &[&bytes], &allow_all).out;
        for k in split_points(&bytes) {
            let o = drive(Format::OpenAiResponses, limits_for_responses(OnFailure::Block), &[&bytes[..k], &bytes[k..]], &allow_all);
            assert!(o.out == whole, "{ctx} split at {k}");
        }
    }
}

// ---- Anthropic recordings (SMA-750 spec § 10.1, D15, D21) -------------------------------------------

/// What one recorded Anthropic stream must give: the number of decided tool calls, and whether the
/// stream is a cut call (`max_tokens` inside a `tool_use` block), which the interceptor blocks as
/// incomplete arguments whatever the decider says.
struct Recorded {
    calls: usize,
    cut: bool,
}

fn expected_recording(name: &str) -> Recorded {
    match name {
        "text_stream" => Recorded { calls: 0, cut: false },
        "parallel_tool_use" => Recorded { calls: 4, cut: false },
        "thinking_then_tool_use" => Recorded { calls: 1, cut: false },
        "large_tool_input" => Recorded { calls: 1, cut: false },
        // A server tool runs at Anthropic: the gateway reports it, it never decides it.
        "server_tool_web_fetch" | "server_tool_web_search" => Recorded { calls: 0, cut: false },
        "max_tokens_in_tool_use" => Recorded { calls: 0, cut: true },
        other => panic!("anthropic/recorded/{other}.sse has no expectation in `expected_recording`"),
    }
}

/// The real Anthropic streams pass through the interceptor under the D15 limits (8 MiB per record,
/// 32 MiB held). Every `mid_stream_error*` stream is skipped until the D17 change (Task 12).
#[test]
fn anthropic_recorded_streams_replay() {
    let limits = |on_failure| paigasus_gateway_core::Limits {
        max_record_bytes: 8_388_608,
        max_held_bytes: 33_554_432,
        on_failure,
        ..paigasus_gateway_core::Limits::defaults()
    };
    let clean = |o: &Outcome| !o.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_) | Metric::OnFailure(_)));
    for (name, bytes) in files("anthropic/recorded") {
        if name.starts_with("mid_stream_error") {
            continue;
        }
        let want = expected_recording(&name);
        for (decider, decide) in DECIDERS {
            let o = drive(Format::AnthropicMessages, limits(OnFailure::Block), &[&bytes], decide);
            let ctx = format!("anthropic/recorded/{name} ({decider})");
            assert!(o.finished, "{ctx} never ended");
            assert_eq!(o.decided.len(), want.calls, "{ctx}: decided calls");
            if want.cut {
                assert!(o.metrics.iter().any(|m| matches!(m, Metric::OnFailure("truncated"))), "{ctx}: {:?}", o.metrics);
            } else {
                assert!(clean(&o), "{ctx}: {:?}", o.metrics);
            }
            check_i1(Format::AnthropicMessages, &bytes, &o.out, &o.decided, o.core_failed()).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            if *decider == "allow-all" && !want.cut {
                assert_eq!(payloads(&o.out), payloads(&bytes), "{ctx}");
            }
        }
        // One split in the middle of the stream gives the one-push output.
        let whole = drive(Format::AnthropicMessages, limits(OnFailure::Block), &[&bytes], &allow_all).out;
        let mid = bytes.len() / 2;
        let o = drive(Format::AnthropicMessages, limits(OnFailure::Block), &[&bytes[..mid], &bytes[mid..]], &allow_all);
        assert!(o.out == whole, "anthropic/recorded/{name}: split at {mid}");
    }
}

/// D21: a recorded data file never holds the Anthropic key prefix. The prefix is built from two
/// parts, so that this file does not hold it.
#[test]
fn anthropic_recordings_hold_no_key() {
    let prefix = format!("{}-", "sk-ant");
    let mut seen = 0;
    for e in std::fs::read_dir(dir("anthropic/recorded")).unwrap() {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x == "sse" || x == "json") {
            continue;
        }
        seen += 1;
        let bytes = std::fs::read(&p).unwrap();
        assert!(!bytes.windows(prefix.len()).any(|w| w == prefix.as_bytes()), "{} holds the key prefix", p.display());
    }
    assert!(seen > 0, "no recorded data file was scanned");
}
