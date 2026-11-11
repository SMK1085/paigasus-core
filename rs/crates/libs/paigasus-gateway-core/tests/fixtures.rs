// SPDX-License-Identifier: Apache-2.0
//! Fixture replays (spec § 8.3, AC 2, AC 3, AC 5).

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Format, OnFailure};
use support::driver::{DECIDERS, allow_all, block_named, drive, limits};
use support::oracle::{check_i1, payloads};

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

/// D13: the CRLF fixture keeps its CR bytes through git.
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

/// AC 5: each malformed shape shows the § 6 behaviour under both on_failure values.
#[test]
fn malformed_shapes_follow_section_6() {
    // (fixture, must appear with block, must appear with allow)
    let openai: &[(&str, &[&str], &[&str])] = &[
        ("nan_record", &["\"b\""], &["\"b\""]),
        ("duplicate_key_mid_hold", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("case_folded_key", &["\"ok\""], &["\"ok\""]),
        ("proto_key", &["\"ok\""], &["\"ok\""]),
        ("float_index", &["\"ok\""], &["\"ok\""]),
        ("repeated_name", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("oversized", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("after_done", &["data: [DONE]"], &["data: [DONE]"]),
        ("error_mid_hold", &["\"terminal\""], &["\"terminal\""]),
        ("json_body", &[], &[]),
        ("lax_line", &["\"ok\""], &["\"ok\""]),
    ];
    let anthropic: &[(&str, &[&str], &[&str])] = &[
        ("event_mismatch", &[], &[": paigasus-interception pass-through"]),
        ("prefilled_input", &[], &[": paigasus-interception pass-through"]),
        ("index_reuse", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("prefilled_message", &[], &[": paigasus-interception pass-through"]),
    ];
    for (format, rel, table) in [(Format::OpenAiChat, "openai/malformed", openai), (Format::AnthropicMessages, "anthropic/malformed", anthropic)] {
        let all = files(rel);
        assert_eq!(all.len(), table.len(), "every {rel} fixture has a row");
        for (name, with_block, with_allow) in table {
            let bytes = &all.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no fixture {name}")).1;
            let b = s(&drive(format, limits(OnFailure::Block), &[bytes], &block_named).out);
            for want in *with_block {
                assert!(b.contains(want), "{rel}/{name} (block) lacks {want:?}:\n{b}");
            }
            let a = s(&drive(format, limits(OnFailure::Allow), &[bytes], &block_named).out);
            for want in *with_allow {
                assert!(a.contains(want), "{rel}/{name} (allow) lacks {want:?}:\n{a}");
            }
        }
    }
}
