// SPDX-License-Identifier: Apache-2.0
//! The decision corpus (spec 2026-11-15 iam-tool-policy-sets, section 10). For each accepted
//! shape it records a policy, an event sequence, and the decisions of Dogwood's own
//! `Authorizer` over that sequence. SMA-753 replays it with the gateway evaluator.
//! `PAIGASUS_WRITE_FIXTURES=1` rewrites `corpus/*.json`.

use std::fs;
use std::path::{Path, PathBuf};

use dogwood_language::{Authorizer, Decision, Event, PolicySchema, Value as DwValue};
use paigasus_policy_compiler::compile::{SetKind, compile_cedarschema};
use paigasus_policy_compiler::lower::{lower_units, service_schema, split_policies};
use serde_json::{Value, json};

const SCHEMA: &str = include_str!("fixtures/test_schema.cedarschema");
const DAY_MS: u64 = 86_400_000;
const GW: &str = "Pgs::Gateway::Action";
const PRINCIPAL: &str = "Pgs::Iam::Principal::\"p1\"";
const RESOURCE: &str = "Pgs::Iam::Organization::\"o1\"";

/// One recorded tool call. `ts` is in seconds.
struct Call {
    ts: i64,
    tool: &'static str,
    input: Value,
}

fn call(ts: i64, tool: &'static str, input: Value) -> Call {
    Call { ts, tool, input }
}

fn to_dogwood(value: &Value) -> DwValue {
    match value {
        Value::String(s) => DwValue::String(s.clone()),
        Value::Number(n) => DwValue::Int(n.as_i64().expect("a corpus number is a Long")),
        Value::Bool(b) => DwValue::Bool(*b),
        other => panic!("the corpus uses no {other}"),
    }
}

/// A `request` event of `call`. The input goes in the logged record (for history) and in the
/// request context (for the Cedar decision).
fn event(call: &Call) -> Event {
    let mut builder = Event::builder(&format!("{GW}::{}", call.tool), "request").timestamp(call.ts).principal(PRINCIPAL).resource(RESOURCE);
    for (name, value) in call.input.as_object().expect("an input object") {
        builder = builder.field("input", name, to_dogwood(value)).request_context("input", name, to_dogwood(value));
    }
    builder.build()
}

/// The Dogwood decision at each call. The `Authorizer` records each event before it decides it.
fn dogwood_decisions(source: &str, calls: &[Call]) -> Vec<&'static str> {
    let svc = service_schema();
    let units = split_policies(source, &svc).expect("the corpus source parses");
    let schema = PolicySchema::from_cedarschema_str(SCHEMA).expect("the test schema parses");
    let mut authorizer = Authorizer::new(lower_units(&units, &svc, &schema).expect("the corpus source lowers"));
    calls
        .iter()
        .map(|call| {
            let response = authorizer.is_authorized(&event(call)).expect("a request event is a decision");
            let errors: Vec<&str> = response.diagnostics().errors().collect();
            assert!(errors.is_empty(), "{source}: {errors:?}");
            match response.decision() {
                Decision::Allow => "allow",
                Decision::Deny => "deny",
            }
        })
        .collect()
}

/// Whether `formerly` sees the event being decided: one call, and a policy that needs an
/// earlier matching call.
fn formerly_sees_the_decided_event() -> bool {
    let source = format!("@id(\"probe\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 1m {GW}::\"search\"::request{{ input.q: \"probe\" }} }};\n");
    dogwood_decisions(&source, &[call(0, "search", json!({ "q": "probe", "n": 1 }))]) == ["allow"]
}

struct Case {
    shape: &'static str,
    source: String,
    calls: Vec<Call>,
    notes: Vec<&'static str>,
    /// `(call index, the Paigasus decision, the rule)` where Paigasus differs from Dogwood.
    differs: Vec<(usize, &'static str, &'static str)>,
}

fn cases() -> Vec<Case> {
    let search = |ts, q: &str| call(ts, "search", json!({ "q": q, "n": 1 }));
    let send = |ts, amount: i64| call(ts, "send", json!({ "to": "x", "amount": amount }));
    vec![
        Case {
            shape: "formerly",
            source: format!(
                "@id(\"corpus-formerly\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 10m {GW}::\"search\"::request{{ input.q: \"unlock\" }} }};\n"
            ),
            calls: vec![search(0, "hello"), search(100, "unlock"), search(200, "hello"), search(700, "hello"), search(701, "hello")],
            notes: vec![
                "call 1 is the first matching call: its decision shows whether `formerly` sees the event being decided",
                "call 3 is exactly 600 s after the matching call and call 4 is 601 s after it: the window is closed",
            ],
            differs: Vec::new(),
        },
        Case {
            shape: "count_within",
            source: format!("@id(\"corpus-count\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ count_within(1m, {GW}::\"search\"::request{{}}) <= 3 }};\n"),
            calls: vec![search(0, "a"), search(10, "b"), search(20, "c"), search(30, "d"), search(60, "e"), search(91, "f")],
            notes: vec![
                "call 3 is the fourth call in the window: the event being decided counts",
                "call 4 is a denied call at 60 s; it still counts, and the window [0, 60] holds five calls",
                "call 5 at 91 s: the window [31, 91] holds calls 4 and 5 only",
            ],
            differs: Vec::new(),
        },
        Case {
            shape: "sum_within",
            source: format!(
                "@id(\"corpus-sum\")\npermit (principal, action == {GW}::\"send\", resource)\nwhen temporal {{ sum_within(a, 1h, {GW}::\"send\"::request{{ input.amount: a }}) <= 100 }};\n"
            ),
            calls: vec![send(0, 60), send(10, 50), send(20, -30), send(3_611, 10)],
            notes: vec![
                "call 1: 60 + 50 = 110, more than 100",
                "call 2 has a negative amount: Dogwood sums 110 - 30 = 80 and allows",
                "call 3 at 3611 s: the window [11, 3611] holds calls 2 and 3 only",
            ],
            differs: vec![(2, "deny", "a negative value adds 0 (spec section 5.4): Paigasus sums 110 + 0 = 110 and denies")],
        },
    ]
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

#[test]
fn the_decision_corpus_is_current() {
    let formerly_sees = formerly_sees_the_decided_event();
    for case in cases() {
        let compiled = compile_cedarschema(SetKind::Tenant, &case.source, SCHEMA, DAY_MS).unwrap_or_else(|rejections| panic!("{}: {rejections:#?}", case.shape));
        assert_eq!(compiled.conditions.len(), 1, "{}", case.shape);
        let decisions = dogwood_decisions(&case.source, &case.calls);
        let events: Vec<Value> = case
            .calls
            .iter()
            .map(|call| json!({ "ts": call.ts, "kind": "request", "tool": call.tool, "principal": PRINCIPAL, "resource": RESOURCE, "input": call.input }))
            .collect();
        let differs: Vec<Value> = case
            .differs
            .iter()
            .map(|(index, paigasus, rule)| {
                assert_ne!(decisions[*index], *paigasus, "{}: call {index} does not differ", case.shape);
                json!({ "event": index, "dogwood": decisions[*index], "paigasus": paigasus, "rule": rule })
            })
            .collect();
        let actual = json!({
            "shape": case.shape,
            "schema": "tests/fixtures/test_schema.cedarschema",
            "timestamp_unit": "seconds",
            "formerly_sees_the_decided_event": formerly_sees,
            "policy": case.source,
            "compiled": compiled,
            "events": events,
            "decisions": decisions,
            "notes": case.notes,
            "paigasus_differs": differs,
        });
        let path = corpus_dir().join(format!("{}.json", case.shape));
        if std::env::var("PAIGASUS_WRITE_FIXTURES").is_ok_and(|v| v == "1") {
            fs::create_dir_all(corpus_dir()).expect("the corpus directory");
            fs::write(&path, serde_json::to_string_pretty(&actual).expect("pretty JSON") + "\n").expect("the corpus writes");
        }
        let committed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))).expect("corpus JSON");
        assert_eq!(committed, actual, "{} is not current; run with PAIGASUS_WRITE_FIXTURES=1", path.display());
    }
}
