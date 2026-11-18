// SPDX-License-Identifier: Apache-2.0
//! The Dogwood trace encoder (SMA-755 § 5.11): golden lines, the skip set, and the fixtures that
//! the policy-compiler workspace replays with the real Dogwood parser (Task 6, P-12).
//! `PAIGASUS_WRITE_FIXTURES=1` rewrites the fixture files.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use paigasus_gateway_core::decision_log::trace::{Encoded, SkipReason, encode, escape, is_safe_action_id, order_key};
use paigasus_gateway_core::decision_log::{CedarDetail, DecisionRecord, RecordHeader, ReportedCall, Side, build_response};
use paigasus_gateway_core::{DecisionContext, ReportDecision, ReportEntry};
use paigasus_kernel::CedarUid;
use paigasus_kernel::tool_policy::{ActionGroup, Declared, Effect};
use uuid::Uuid;

const PRN: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003";
const ORG: &str = "0190a100-0000-7000-8000-0000000000a1";
const PRINCIPAL_ID: &str = "0190a1e5-0000-7000-8000-000000000004";
/// The seed of the id generator (P-12). A change of the seed changes the fixture files.
const ID_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const ID_COUNT: usize = 512;

fn org() -> Uuid {
    Uuid::parse_str(ORG).unwrap()
}

/// A response record of the fixture org and principal.
fn rec(decided_at: Duration, action: &str, model: &str) -> DecisionRecord {
    let ctx = DecisionContext::new(PRN, "ignored", Some("req-1".into())).with_request(None, "chat_completions", model);
    let entry = ReportEntry {
        choice: 0,
        position: 0,
        name: action.into(),
        decision: ReportDecision::Allow,
        basis: None,
        code: None,
        enforced: None,
        reason: None,
    };
    let header = RecordHeader::new(Uuid::from_u128(1), SystemTime::UNIX_EPOCH + decided_at, Uuid::from_u128(2), 0, Some(org()));
    let mut r = build_response(header, &ctx, &ReportedCall::new(entry, None, None));
    r.principal = Some(CedarUid {
        entity_type: "Pgs::Iam::Principal".into(),
        entity_id: PRINCIPAL_ID.into(),
    });
    r.scope_prn = format!("prn:pgs:iam:::organization/{ORG}");
    r
}

fn record_a() -> DecisionRecord {
    let mut r = rec(Duration::from_secs(1_763_460_000), "read_file", "gpt-4o");
    r.cedar = Some(CedarDetail::new(
        Effect::Allow,
        vec![],
        vec![],
        false,
        BTreeSet::from(["name_collision".to_owned()]),
        Declared::Yes,
        ActionGroup::Catalog,
        Duration::ZERO,
    ));
    r
}

fn line(r: &DecisionRecord) -> String {
    match encode(r) {
        Encoded::Line(l) => l.to_string(),
        Encoded::Skipped(why) => panic!("skipped: {}", why.as_str()),
    }
}

fn skipped(r: &DecisionRecord) -> SkipReason {
    match encode(r) {
        Encoded::Skipped(why) => why,
        Encoded::Line(l) => panic!("encoded: {l}"),
    }
}

#[test]
fn a_response_record_gives_the_golden_line() {
    assert_eq!(
        line(&record_a()),
        concat!(
            r#"@1763460000 scope(principal: Pgs::Iam::Principal::"0190a1e5-0000-7000-8000-000000000004", resource: Pgs::Iam::Organization::"0190a100-0000-7000-8000-0000000000a1") "#,
            r#"request_context(input: {}, findings: ["name_collision"], call: { declared: "yes" }, request: { scope: "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", ingress: "chat_completions", model: "gpt-4o" }, session: { id: "", turn: 0 }) "#,
            r#"Pgs::Gateway::Action::"read_file"::request(input: {}, callerPrincipal: Pgs::Iam::Principal::"0190a1e5-0000-7000-8000-000000000004", callerResource: Pgs::Iam::Organization::"0190a100-0000-7000-8000-0000000000a1", requestId: "req-1", sessionId: "")"#
        )
    );
}

#[test]
fn no_cedar_detail_gives_empty_findings_and_unknown_declared() {
    let r = rec(Duration::from_secs(5), "search", "m");
    assert!(line(&r).contains(r#"findings: [], call: { declared: "unknown" }"#));
}

#[test]
fn several_findings_are_listed_in_order() {
    let mut r = record_a();
    r.cedar.as_mut().unwrap().findings = BTreeSet::from(["b".to_owned(), "a".to_owned()]);
    assert!(line(&r).contains(r#"findings: ["a", "b"]"#));
}

#[test]
fn the_second_is_rounded_down() {
    let r = rec(Duration::from_millis(1_763_460_000_999), "search", "m");
    assert!(line(&r).starts_with("@1763460000 "));
}

#[test]
fn hostile_request_context_strings() {
    let r = rec(Duration::from_secs(5), "read_file", "gpt (x) \"q\" \\ \n end");
    let l = line(&r);
    assert!(!l.contains('\n'));
    assert!(l.contains(r#"model: "gpt (x) \"q\" \\ \n end""#));
}

#[test]
fn each_skip_reason() {
    let mut r = record_a();
    r.side = Side::Request;
    assert_eq!(skipped(&r), SkipReason::RequestSide);
    let mut r = record_a();
    r.org = None;
    assert_eq!(skipped(&r), SkipReason::NoOrg);
    let mut r = record_a();
    r.principal = None;
    assert_eq!(skipped(&r), SkipReason::NoPrincipal);
    let mut r = record_a();
    r.tool.truncated = true;
    assert_eq!(skipped(&r), SkipReason::Truncated);
    for id in ["a(b", "a)b", "a\"b", "a\u{0}b", "a\rb"] {
        let mut r = record_a();
        r.tool.action_id = id.into();
        assert_eq!(skipped(&r), SkipReason::UnsafeActionId, "{id:?}");
    }
    let labels = [SkipReason::RequestSide, SkipReason::NoOrg, SkipReason::NoPrincipal, SkipReason::Truncated, SkipReason::UnsafeActionId].map(SkipReason::as_str);
    assert_eq!(labels, ["request side", "no org", "no principal", "truncated", "unsafe action id"]);
}

#[test]
fn safe_ids_include_namespaces_and_unicode() {
    for id in ["ns.read_file", "caf\u{e9}", "a::b", "a b"] {
        assert!(is_safe_action_id(id), "{id:?}");
    }
}

#[test]
fn the_order_key_is_second_then_time_then_instance_then_sequence() {
    let a = rec(Duration::from_millis(1_000_900), "a", "m");
    let mut b = rec(Duration::from_millis(1_001_100), "a", "m");
    assert!(order_key(&a) < order_key(&b));
    b.decided_at = a.decided_at;
    b.sequence = 1;
    assert!(order_key(&a) < order_key(&b));
    b.sequence = 0;
    b.instance_id = Uuid::from_u128(1);
    assert!(order_key(&b) < order_key(&a));
    assert_eq!(order_key(&a).0, 1000);
}

#[test]
fn escape_is_the_debug_escape() {
    assert_eq!(escape("a\"b\\c\nd'"), "a\\\"b\\\\c\\nd\\'");
}

// ---- the fixtures ----

/// xorshift64, deterministic from `ID_SEED`.
fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// The ids of the 512 generated candidates that `encode` does not skip.
fn generated_ids() -> Vec<String> {
    const ALPHABET: &[char] = &[
        '(',
        ')',
        '"',
        ':',
        ':',
        '\\',
        '\'',
        '\r',
        '\n',
        '\t',
        '\0',
        '\u{202e}',
        '\u{301}',
        ' ',
        '_',
        '-',
        '.',
        'a',
        'b',
        'c',
        'x',
        'Z',
        '0',
        '9',
        '\u{e9}',
        '\u{65e5}',
        '\u{672c}',
        '\u{1f600}',
    ];
    let mut state = ID_SEED;
    let mut out = Vec::new();
    for _ in 0..ID_COUNT {
        let len = 1 + (next(&mut state) % 12) as usize;
        let mut id = String::new();
        for i in 0..len {
            // A grapheme-extend char at the start is the case that `escape_debug` must escape.
            if i == 0 && next(&mut state).is_multiple_of(8) {
                id.push('\u{301}');
            } else {
                id.push(ALPHABET[(next(&mut state) % ALPHABET.len() as u64) as usize]);
            }
        }
        let mut r = rec(Duration::from_secs(1000), "x", "m");
        r.tool.action_id = id.clone();
        if matches!(encode(&r), Encoded::Line(_)) {
            out.push(id);
        }
    }
    out
}

const POLICY: &str = r#"// SPDX-License-Identifier: Apache-2.0
// SMA-755: the policy that the decision-trace fixture replays. It permits `search` while at most
// two searches fall in the last hour. The Authorizer records an event before it decides it, so the
// counts at the five `search` events are 1, 2, 3, 4 and 1. `read_file` has no permit.
@id("search-rate")
permit (principal, action == Pgs::Gateway::Action::"search", resource)
when temporal { count_within(1h, Pgs::Gateway::Action::"search"::request{}) <= 2 };
"#;

fn fixtures() -> Vec<(&'static str, String)> {
    let mut log = String::new();
    for secs in [1000, 1500, 2000, 2100, 9000] {
        writeln!(log, "{}", line(&rec(Duration::from_secs(secs), "search", "gpt-4o"))).unwrap();
    }
    writeln!(log, "{}", line(&rec(Duration::from_secs(9100), "read_file", "gpt (x) \"q\" \\ \n end"))).unwrap();

    let ids = generated_ids();
    let mut ids_log = String::new();
    for (i, id) in ids.iter().enumerate() {
        let mut r = rec(Duration::from_secs(1000 + i as u64), "x", "m");
        r.tool.action_id = id.clone();
        writeln!(ids_log, "{}", line(&r)).unwrap();
    }
    let ids_json = format!("{}\n", serde_json::to_string_pretty(&ids).unwrap());
    vec![
        ("decision-trace.log", log),
        ("decision-trace.dw", POLICY.to_owned()),
        ("decision-trace.expected.json", "[\"ALLOW\",\"ALLOW\",\"DENY\",\"DENY\",\"ALLOW\",\"DENY\"]\n".to_owned()),
        ("decision-trace-ids.log", ids_log),
        ("decision-trace-ids.json", ids_json),
    ]
}

#[test]
fn the_fixtures_match_the_encoder() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/tests/fixtures");
    let write = std::env::var_os("PAIGASUS_WRITE_FIXTURES").is_some();
    let mut drifted = Vec::new();
    for (name, want) in fixtures() {
        let path = dir.join(name);
        if write {
            fs::write(&path, &want).unwrap();
        } else if fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
            drifted.push(name);
        }
    }
    assert!(drifted.is_empty(), "fixture drift in {drifted:?}; run with PAIGASUS_WRITE_FIXTURES=1 to rewrite");
}

#[test]
fn the_id_generator_keeps_most_ids_and_the_hostile_ones_are_skipped() {
    let kept = generated_ids();
    assert!(kept.len() > ID_COUNT / 10 && kept.len() < ID_COUNT, "kept {}", kept.len());
    assert!(kept.iter().all(|id| is_safe_action_id(id)));
    assert!(kept.iter().any(|id| id.starts_with('\u{301}')));
}
