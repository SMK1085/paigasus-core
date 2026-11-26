// SPDX-License-Identifier: Apache-2.0
//! The decision corpus (spec 2026-11-15 iam-tool-policy-sets, section 10; SMA-753 PR 2 spec P15).
//! For each accepted shape it records a policy, an event sequence, and the decisions of Dogwood's
//! own `Authorizer` over that sequence. SMA-753 replays it with the gateway evaluator.
//! `PAIGASUS_WRITE_FIXTURES=1` rewrites `corpus/*.json`.
//!
//! Each event holds two forms of the call arguments. `arguments` is the exact text that the
//! gateway reads. `input` is the value tree that Dogwood gets, because a JSON number cannot keep
//! the text of `1.5000`, `-0.0` or `1e19`. In `input`, `{"__decimal": "<text>"}` is a Dogwood
//! decimal, an integer text in the `i64` range is a Long, and another number text (a big
//! integer, `1e19`) is a string. Free text gives an empty `input`. A `response` event copies the
//! `arguments` and `input` of its call, and its `decisions` entry is `null`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dogwood_language::{Authorizer, Decision, Event, PolicySchema, Value as DwValue};
use paigasus_policy_compiler::compile::{SetKind, compile_cedarschema};
use paigasus_policy_compiler::lower::{lower_units, service_schema, split_policies};
use serde_json::{Value, json};

/// The schema path (relative to the crate) and its text.
type Schema = (&'static str, &'static str);
const PR1_SCHEMA: Schema = ("tests/fixtures/test_schema.cedarschema", include_str!("fixtures/test_schema.cedarschema"));
const SCHEMA: Schema = ("tests/fixtures/corpus_schema.cedarschema", include_str!("fixtures/corpus_schema.cedarschema"));
const DAY_MS: u64 = 86_400_000;
const GW: &str = "Pgs::Gateway::Action";
const PRINCIPAL: &str = "Pgs::Iam::Principal::\"p1\"";
const RESOURCE: &str = "Pgs::Iam::Organization::\"o1\"";
/// The rule of the `any_catalog_tool` differences (coordinator ruling, SMA-753 PR 2 Task 16).
const ANY_CATALOG_TOOL_RULE: &str = "The compiler maps InvokeTool::request{} to any_catalog_tool, which matches every catalog tool (SMA-751). Dogwood matches only an event whose action is InvokeTool itself, and no gateway call has that action, so Dogwood's rule could never match in the gateway.";
/// The member of a Dogwood decimal in an `input` tree.
const DECIMAL: &str = "__decimal";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Request,
    Response,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Request => "request",
            Kind::Response => "response",
        }
    }
}

/// One recorded tool call event. `ts` is in seconds.
#[derive(Clone)]
struct Call {
    ts: i64,
    kind: Kind,
    tool: &'static str,
    /// The argument text that the gateway reads.
    arguments: &'static str,
    /// The value tree that Dogwood gets (see the module comment).
    input: Value,
}

fn call(ts: i64, tool: &'static str, arguments: &'static str, input: Value) -> Call {
    Call {
        ts,
        kind: Kind::Request,
        tool,
        arguments,
        input,
    }
}

/// The `response` event of `request`, at `ts`. It copies the arguments of its call.
fn response(ts: i64, request: &Call) -> Call {
    Call {
        ts,
        kind: Kind::Response,
        ..request.clone()
    }
}

fn decimal(text: &str) -> Value {
    json!({ DECIMAL: text })
}

fn to_dogwood(value: &Value) -> DwValue {
    match value {
        Value::String(s) => DwValue::String(s.clone()),
        Value::Number(n) => DwValue::Int(n.as_i64().expect("a corpus number is a Long")),
        Value::Bool(b) => DwValue::Bool(*b),
        Value::Object(map) => match map.get(DECIMAL) {
            Some(Value::String(text)) if map.len() == 1 => DwValue::Decimal(text.clone()),
            _ => DwValue::Object(map.iter().map(|(name, value)| (name.clone(), to_dogwood(value))).collect()),
        },
        other => panic!("the corpus uses no {other}"),
    }
}

/// Whether `input` is the Dogwood value of `arguments` (`args`, read with serde_json): a Long
/// is the same integer, a decimal or a string of a number names the same number and appears
/// in the argument text as written.
fn names_the_same_value(input: &Value, args: &Value, text: &str) -> bool {
    match (input, args) {
        (Value::Object(map), Value::Number(n)) if map.len() == 1 => match map.get(DECIMAL) {
            Some(Value::String(d)) => !d.contains(['e', 'E']) && text.contains(d.as_str()) && d.parse::<f64>().ok() == n.as_f64(),
            _ => false,
        },
        (Value::Object(map), Value::Object(args)) => map.len() == args.len() && map.iter().all(|(k, v)| args.get(k).is_some_and(|a| names_the_same_value(v, a, text))),
        (Value::Number(i), Value::Number(n)) => i.as_i64().is_some() && i.as_i64() == n.as_i64(),
        (Value::String(s), Value::Number(n)) => s.parse::<i64>().is_err() && text.contains(s.as_str()) && s.parse::<f64>().ok() == n.as_f64(),
        _ => input == args,
    }
}

/// A Dogwood event of `call`. The input goes in the logged record (for history) and, for a
/// request, in the request context (for the Cedar decision).
fn event(call: &Call) -> Event {
    let mut builder = Event::builder(&format!("{GW}::{}", call.tool), call.kind.name())
        .timestamp(call.ts)
        .principal(PRINCIPAL)
        .resource(RESOURCE);
    let input = call.input.as_object().expect("an input object");
    for (name, value) in input {
        builder = builder.field("input", name, to_dogwood(value));
    }
    if call.kind == Kind::Request {
        let context: BTreeMap<String, DwValue> = input.iter().map(|(name, value)| (name.clone(), to_dogwood(value))).collect();
        builder = builder.request_context_field("input", DwValue::Object(context));
    }
    builder.build()
}

/// The Dogwood decision at each event, `None` at a response. The `Authorizer` records each event
/// before it decides it. A call in `nonconforming` must fail closed with a nonconforming
/// diagnostic. Every other decision must have no error.
fn dogwood_decisions(source: &str, schema: &str, calls: &[Call], nonconforming: &[usize]) -> Vec<Option<&'static str>> {
    let svc = service_schema();
    let units = split_policies(source, &svc).expect("the corpus source parses");
    let schema = PolicySchema::from_cedarschema_str(schema).expect("the corpus schema parses");
    let mut authorizer = Authorizer::new(lower_units(&units, &svc, &schema).expect("the corpus source lowers"));
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let submitted = authorizer.submit(&event(call)).unwrap_or_else(|rejection| panic!("{source}: event {index} is rejected: {rejection:?}"));
            let Some(response) = submitted else {
                assert!(call.kind == Kind::Response, "{source}: request {index} has no decision");
                return None;
            };
            assert!(call.kind == Kind::Request, "{source}: response {index} has a decision");
            let errors: Vec<&str> = response.diagnostics().errors().collect();
            if nonconforming.contains(&index) {
                assert!(!errors.is_empty() && errors.iter().all(|e| e.contains("nonconforming")), "{source}: call {index}: {errors:?}");
            } else {
                assert!(errors.is_empty(), "{source}: call {index}: {errors:?}");
            }
            Some(match response.decision() {
                Decision::Allow => "allow",
                Decision::Deny => "deny",
            })
        })
        .collect()
}

/// Whether `formerly` sees the event being decided: one call, and a policy that needs an
/// earlier matching call.
fn formerly_sees_the_decided_event() -> bool {
    let source = format!("@id(\"probe\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 1m {GW}::\"search\"::request{{ input.q: \"probe\" }} }};\n");
    let probe = call(0, "search", r#"{"q": "probe", "n": 1}"#, json!({ "q": "probe", "n": 1 }));
    dogwood_decisions(&source, PR1_SCHEMA.1, &[probe], &[]) == [Some("allow")]
}

struct Case {
    /// The file name, `corpus/<name>.json`.
    name: &'static str,
    shape: &'static str,
    schema: Schema,
    source: String,
    /// The number of history conditions the source compiles to.
    conditions: usize,
    calls: Vec<Call>,
    /// The API key of every event, for a `@scope("key")` case.
    api_key: Option<&'static str>,
    /// The calls that Dogwood refuses as nonconforming: it denies them and does not record them.
    nonconforming: Vec<usize>,
    notes: Vec<&'static str>,
    /// `(call index, the Paigasus decision, the rule)` where Paigasus differs from Dogwood.
    differs: Vec<(usize, &'static str, &'static str)>,
}

impl Case {
    /// A case on the corpus schema with one condition, no API key and no nonconforming call.
    fn new(name: &'static str, shape: &'static str, source: String, calls: Vec<Call>, notes: Vec<&'static str>) -> Case {
        Case {
            name,
            shape,
            schema: SCHEMA,
            source,
            conditions: 1,
            calls,
            api_key: None,
            nonconforming: Vec::new(),
            notes,
            differs: Vec::new(),
        }
    }
}

/// A permit on `action` with one temporal block.
fn permit(id: &str, action: &str, block: &str) -> String {
    format!("@id(\"{id}\")\npermit (principal, action == {GW}::\"{action}\", resource)\nwhen temporal {{ {block} }};\n")
}

fn pr1_cases() -> Vec<Case> {
    let search = |ts, arguments, q: &str| call(ts, "search", arguments, json!({ "q": q, "n": 1 }));
    let send = |ts, arguments, amount: i64| call(ts, "send", arguments, json!({ "to": "x", "amount": amount }));
    let pr1 = |name, source, calls, notes, differs| Case {
        differs,
        schema: PR1_SCHEMA,
        ..Case::new(name, name, source, calls, notes)
    };
    vec![
        pr1(
            "formerly",
            format!(
                "@id(\"corpus-formerly\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 10m {GW}::\"search\"::request{{ input.q: \"unlock\" }} }};\n"
            ),
            vec![
                search(0, r#"{"q": "hello", "n": 1}"#, "hello"),
                search(100, r#"{"q": "unlock", "n": 1}"#, "unlock"),
                search(200, r#"{"q": "hello", "n": 1}"#, "hello"),
                search(700, r#"{"q": "hello", "n": 1}"#, "hello"),
                search(701, r#"{"q": "hello", "n": 1}"#, "hello"),
            ],
            vec![
                "call 1 is the first matching call: its decision shows whether `formerly` sees the event being decided",
                "call 3 is exactly 600 s after the matching call and call 4 is 601 s after it: the window is closed",
            ],
            Vec::new(),
        ),
        pr1(
            "count_within",
            format!("@id(\"corpus-count\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ count_within(1m, {GW}::\"search\"::request{{}}) <= 3 }};\n"),
            vec![
                search(0, r#"{"q": "a", "n": 1}"#, "a"),
                search(10, r#"{"q": "b", "n": 1}"#, "b"),
                search(20, r#"{"q": "c", "n": 1}"#, "c"),
                search(30, r#"{"q": "d", "n": 1}"#, "d"),
                search(60, r#"{"q": "e", "n": 1}"#, "e"),
                search(91, r#"{"q": "f", "n": 1}"#, "f"),
            ],
            vec![
                "call 3 is the fourth call in the window: the event being decided counts",
                "call 4 is a denied call at 60 s; it still counts, and the window [0, 60] holds five calls",
                "call 5 at 91 s: the window [31, 91] holds calls 4 and 5 only",
            ],
            Vec::new(),
        ),
        pr1(
            "sum_within",
            format!("@id(\"corpus-sum\")\npermit (principal, action == {GW}::\"send\", resource)\nwhen temporal {{ sum_within(a, 1h, {GW}::\"send\"::request{{ input.amount: a }}) <= 100 }};\n"),
            vec![
                send(0, r#"{"to": "x", "amount": 60}"#, 60),
                send(10, r#"{"to": "x", "amount": 50}"#, 50),
                send(20, r#"{"to": "x", "amount": -30}"#, -30),
                send(3_611, r#"{"to": "x", "amount": 10}"#, 10),
            ],
            vec![
                "call 1: 60 + 50 = 110, more than 100",
                "call 2 has a negative amount: Dogwood sums 110 - 30 = 80 and allows",
                "call 3 at 3611 s: the window [11, 3611] holds calls 2 and 3 only",
            ],
            vec![(2, "deny", "a negative value adds 0 (spec section 5.4): Paigasus sums 110 + 0 = 110 and denies")],
        ),
    ]
}

/// The cases of SMA-753 PR 2 (spec P15).
fn pr2_cases() -> Vec<Case> {
    let pay = |ts, arguments, input| call(ts, "pay", arguments, input);
    let send = |ts, arguments, input| call(ts, "send", arguments, input);
    let search = |ts| call(ts, "search", r#"{"q": "a"}"#, json!({ "q": "a" }));
    let sum_send = |id: &str| permit(id, "send", &format!("sum_within(a, 1h, {GW}::\"send\"::request{{ input.amount: a }}) <= 100"));
    let count_search = |scope: &str, id: &str| format!("@scope(\"{scope}\")\n{}", permit(id, "search", &format!("count_within(1m, {GW}::\"search\"::request{{}}) <= 2")));

    let event_kind_call = call(0, "search", r#"{"q": "a"}"#, json!({ "q": "a" }));
    let event_kind_call_2 = call(20, "search", r#"{"q": "b"}"#, json!({ "q": "b" }));
    let copied_call = send(0, r#"{"to": "x", "amount": 1}"#, json!({ "to": "x", "amount": 1 }));

    let op_policy = |op: &str, token: &str| {
        permit(
            &format!("corpus-op-{op}"),
            &format!("op_{op}"),
            &format!("formerly within 1m (exists (v: Long). ({GW}::\"search\"::request{{ input.n: v }} && v {token} 5))"),
        )
    };
    let op_source: String = [("lt", "<"), ("le", "<="), ("eq", "=="), ("ge", ">="), ("gt", ">"), ("ne", "!=")]
        .iter()
        .map(|(op, token)| op_policy(op, token))
        .collect();
    let op_round = |ts: i64, arguments: &'static str, n: i64| {
        let mut calls = vec![call(ts, "search", arguments, json!({ "q": "a", "n": n }))];
        calls.extend(
            ["op_lt", "op_le", "op_eq", "op_ge", "op_gt", "op_ne"]
                .iter()
                .enumerate()
                .map(|(i, tool)| call(ts + 1 + i as i64, tool, "{}", json!({}))),
        );
        calls
    };

    let decimal_policy = |id: &str, literal: &str| permit(id, "pay", &format!("formerly within 10s {GW}::\"pay\"::request{{ input.price: decimal(\"{literal}\") }}"));

    vec![
        Case::new(
            "predicate_event_kind",
            "count_within",
            permit("corpus-event-kind", "search", &format!("count_within(1m, {GW}::\"search\"::response{{}}) < 2")),
            vec![
                event_kind_call.clone(),
                response(10, &event_kind_call),
                event_kind_call_2.clone(),
                response(30, &event_kind_call_2),
                call(40, "search", r#"{"q": "c"}"#, json!({ "q": "c" })),
            ],
            vec!["only `response` events count: the requests 0, 2 and 4 see 0, 1 and 2 responses"],
        ),
        Case::new(
            "predicate_all_of",
            "formerly",
            permit("corpus-all-of", "pay", &format!("formerly within 1m {GW}::\"pay\"::request{{ input.memo: \"x\", input.urgent: true }}")),
            vec![
                pay(0, r#"{"memo": "x", "urgent": false}"#, json!({ "memo": "x", "urgent": false })),
                pay(100, r#"{"memo": "y", "urgent": true}"#, json!({ "memo": "y", "urgent": true })),
                pay(200, r#"{"memo": "x", "urgent": true}"#, json!({ "memo": "x", "urgent": true })),
            ],
            vec!["each call is more than 1 m after the one before, so each call sees only itself", "only call 2 matches both field tests"],
        ),
        Case::new(
            "predicate_any_of",
            "formerly",
            permit(
                "corpus-any-of",
                "pay",
                &format!("formerly within 1m (!(!{GW}::\"pay\"::request{{ input.urgent: true }} && !{GW}::\"pay\"::request{{ input.memo: \"x\" }}))"),
            ),
            vec![
                pay(0, r#"{"memo": "y", "urgent": true}"#, json!({ "memo": "y", "urgent": true })),
                pay(100, r#"{"memo": "x", "urgent": false}"#, json!({ "memo": "x", "urgent": false })),
                pay(200, r#"{"memo": "y", "urgent": false}"#, json!({ "memo": "y", "urgent": false })),
                pay(300, "{}", json!({})),
            ],
            vec![
                "the Dogwood grammar has no `||` in a temporal block, so this case writes `A || B` as `!(!A && !B)`: the compiled predicate is a negation of an all_of, not an any_of node",
                "each call sees only itself: calls 0 and 1 match one side, calls 2 and 3 match neither",
            ],
        ),
        Case::new(
            "predicate_negation",
            "formerly",
            permit(
                "corpus-negation",
                "pay",
                &format!("formerly within 1m ({GW}::\"pay\"::request{{}} && !{GW}::\"pay\"::request{{ input.urgent: true }})"),
            ),
            vec![
                pay(0, r#"{"urgent": true}"#, json!({ "urgent": true })),
                pay(100, r#"{"urgent": false}"#, json!({ "urgent": false })),
                pay(200, "{}", json!({})),
                pay(300, r#"{"urgent": true}"#, json!({ "urgent": true })),
            ],
            vec!["each call sees only itself", "call 2 has no `urgent`: the negated field test is false, so the negation is true"],
        ),
        Case {
            conditions: 6,
            ..Case::new(
                "ops_long",
                "formerly",
                op_source,
                [
                    op_round(0, r#"{"q": "a", "n": 5}"#, 5),
                    op_round(100, r#"{"q": "a", "n": 4}"#, 4),
                    op_round(200, r#"{"q": "a", "n": 6}"#, 6),
                ]
                .concat(),
                vec![
                    "one policy for each op: the tool `op_<op>` is allowed when a `search` call in the last minute has `n <op> 5`",
                    "three rounds, 100 s apart: n = 5, n = 4, n = 6; each round is one `search` call (denied: no policy permits it) and one call of each `op_` tool",
                ],
            )
        },
        Case::new(
            "response_copies_fields",
            "formerly",
            permit("corpus-response", "send", &format!("formerly within 10m {GW}::\"send\"::response{{ input.to: \"x\" }}")),
            vec![
                copied_call.clone(),
                response(1, &copied_call),
                send(2, r#"{"to": "y", "amount": 1}"#, json!({ "to": "y", "amount": 1 })),
                send(602, r#"{"to": "y", "amount": 1}"#, json!({ "to": "y", "amount": 1 })),
            ],
            vec![
                "the response event copies the arguments of call 0, so it matches `input.to: \"x\"`",
                "call 0 is denied: its own response comes later",
                "call 3 is 601 s after the response: the window is closed",
            ],
        ),
        Case {
            differs: vec![(1, "deny", ANY_CATALOG_TOOL_RULE), (2, "deny", ANY_CATALOG_TOOL_RULE)],
            ..Case::new(
                "any_catalog_tool",
                "count_within",
                format!("@id(\"corpus-any-tool\")\npermit (principal, action in {GW}::\"InvokeTool\", resource)\nwhen temporal {{ count_within(1m, {GW}::\"InvokeTool\"::request{{}}) <= 1 }};\n"),
                vec![
                    search(0),
                    send(10, r#"{"to": "x", "amount": 1}"#, json!({ "to": "x", "amount": 1 })),
                    pay(20, r#"{"memo": "x"}"#, json!({ "memo": "x" })),
                ],
                vec!["Dogwood matches `InvokeTool::request` only on an event whose action is `InvokeTool` itself, not on a call of a member tool: it counts 0 and allows every call"],
            )
        },
        Case::new(
            "scope_principal",
            "count_within",
            count_search("principal", "corpus-scope-principal"),
            vec![search(0), search(10), search(20)],
            vec!["every event has one principal; call 2 is the third call in the window"],
        ),
        Case {
            api_key: Some("k1"),
            ..Case::new(
                "scope_key",
                "count_within",
                count_search("key", "corpus-scope-key"),
                vec![search(0), search(10), search(20)],
                vec!["every event has one principal and the API key `k1`; call 2 is the third call in the window"],
            )
        },
        Case::new(
            "nested_path",
            "formerly",
            permit(
                "corpus-nested",
                "pay",
                &format!("formerly within 1m {GW}::\"pay\"::request{{ input.meta.region: \"eu\", input.meta.limit.max: 3 }}"),
            ),
            vec![
                pay(0, r#"{"meta": {"region": "eu"}}"#, json!({ "meta": { "region": "eu" } })),
                pay(100, r#"{"meta": {"region": "eu", "limit": {"max": 3}}}"#, json!({ "meta": { "region": "eu", "limit": { "max": 3 } } })),
                pay(200, r#"{"meta": {"region": "us", "limit": {"max": 3}}}"#, json!({ "meta": { "region": "us", "limit": { "max": 3 } } })),
                pay(300, "{}", json!({})),
            ],
            vec!["each call sees only itself", "call 0 has no `meta.limit.max`, and call 3 has no `meta`"],
        ),
        Case::new(
            "field_string",
            "formerly",
            permit("corpus-string", "pay", &format!("formerly within 1m {GW}::\"pay\"::request{{ input.memo: \"x\" }}")),
            vec![
                pay(0, r#"{"memo": "X"}"#, json!({ "memo": "X" })),
                pay(100, r#"{"memo": "x "}"#, json!({ "memo": "x " })),
                pay(200, r#"{"memo": "x"}"#, json!({ "memo": "x" })),
            ],
            vec!["each call sees only itself", "a string compares exactly: no case folding and no trimming"],
        ),
        Case::new(
            "field_bool",
            "formerly",
            permit("corpus-bool", "pay", &format!("formerly within 1m {GW}::\"pay\"::request{{ input.urgent: true }}")),
            vec![
                pay(0, r#"{"urgent": false}"#, json!({ "urgent": false })),
                pay(100, "{}", json!({})),
                pay(200, r#"{"urgent": true}"#, json!({ "urgent": true })),
            ],
            vec!["each call sees only itself"],
        ),
        Case::new(
            "field_decimal",
            "formerly",
            decimal_policy("corpus-decimal", "2.25"),
            vec![
                pay(0, r#"{"price": 2.5}"#, json!({ "price": decimal("2.5") })),
                pay(100, r#"{"price": 2.25}"#, json!({ "price": decimal("2.25") })),
                pay(200, r#"{"price": 22.5}"#, json!({ "price": decimal("22.5") })),
            ],
            vec!["each call sees only itself"],
        ),
        Case {
            conditions: 3,
            nonconforming: vec![3],
            differs: vec![
                (
                    0,
                    "deny",
                    "spec P1 and P3: the kernel reads the argument `5.0` as Long(5) (F8); a Long value against a decimal literal is unknown, and an unknown event makes the slot missing, so Paigasus denies. The kernel classifies an integral number as a Long, as Cedar context.input does. A Long against a decimal literal is unknown (P1).",
                ),
                (
                    1,
                    "deny",
                    "spec P1 and P3: the kernel reads the argument `-0.0` as Long(0) (F8); a Long value against a decimal literal is unknown, and an unknown event makes the slot missing, so Paigasus denies. The kernel classifies an integral number as a Long, as Cedar context.input does. A Long against a decimal literal is unknown (P1).",
                ),
            ],
            ..Case::new(
                "decimal_edges",
                "formerly",
                [
                    decimal_policy("corpus-decimal-5", "5.0"),
                    decimal_policy("corpus-decimal-0", "0.0"),
                    decimal_policy("corpus-decimal-1-5", "1.5"),
                ]
                .concat(),
                vec![
                    pay(0, r#"{"price": 5.0}"#, json!({ "price": decimal("5.0") })),
                    pay(100, r#"{"price": -0.0}"#, json!({ "price": decimal("-0.0") })),
                    pay(200, r#"{"price": 1.5000}"#, json!({ "price": decimal("1.5000") })),
                    pay(300, r#"{"price": 5}"#, json!({ "price": 5 })),
                ],
                vec![
                    "three policies, with the literals decimal(\"5.0\"), decimal(\"0.0\") and decimal(\"1.5\"); each call sees only itself",
                    "Dogwood compares decimals by value: 5.0 = 5.0, -0.0 = 0.0, 1.5000 = 1.5",
                    "call 3: Dogwood refuses the Long 5 in the decimal field `price` as nonconforming and denies",
                ],
            )
        },
        Case {
            nonconforming: vec![1],
            differs: vec![(
                2,
                "deny",
                "spec P1: the kernel reads 99999999999999999999 as a Text number, which is stored as Mistyped and is unknown; `formerly` with no true event and one unknown event makes the slot missing, and the errored forbid denies",
            )],
            ..Case::new(
                "big_integer",
                "formerly",
                format!(
                    "@id(\"corpus-big-permit\")\npermit (principal, action == {GW}::\"send\", resource);\n@id(\"corpus-big\")\nforbid (principal, action == {GW}::\"send\", resource)\nwhen temporal {{ formerly within 10m {GW}::\"send\"::request{{ input.amount: 5 }} }};\n"
                ),
                vec![
                    send(0, r#"{"to": "x", "amount": 7}"#, json!({ "to": "x", "amount": 7 })),
                    send(10, r#"{"to": "x", "amount": 99999999999999999999}"#, json!({ "to": "x", "amount": "99999999999999999999" })),
                    send(20, r#"{"to": "x", "amount": 7}"#, json!({ "to": "x", "amount": 7 })),
                    send(700, r#"{"to": "x", "amount": 7}"#, json!({ "to": "x", "amount": 7 })),
                ],
                vec![
                    "a forbid when a call in the last 10 m had `amount: 5`, and a plain permit",
                    "call 1: Dogwood gets the big integer as a string, refuses it as nonconforming, denies it and does not record it",
                    "call 3 is 690 s after call 1: the window is closed",
                ],
            )
        },
        Case {
            nonconforming: vec![1],
            differs: vec![(
                2,
                "deny",
                "spec P1: the kernel reads `1e19` as a Text number, which is stored as Mistyped; a Mistyped value at the sum_within path of an event whose predicate is true makes the slot missing, so Paigasus denies",
            )],
            ..Case::new(
                "exponent_1e19",
                "sum_within",
                sum_send("corpus-1e19"),
                vec![
                    send(0, r#"{"to": "x", "amount": 60}"#, json!({ "to": "x", "amount": 60 })),
                    send(10, r#"{"to": "x", "amount": 1e19}"#, json!({ "to": "x", "amount": "1e19" })),
                    send(20, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                    send(3_620, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                ],
                vec![
                    "call 1: Dogwood gets `1e19` as a string, refuses it as nonconforming, denies it and does not record it",
                    "call 2: Dogwood sums 60 + 30 = 90",
                    "call 3 at 3620 s: the window [20, 3620] holds calls 2 and 3 only",
                ],
            )
        },
        Case::new(
            "missing_field",
            "sum_within",
            sum_send("corpus-missing"),
            vec![
                send(0, r#"{"to": "x", "amount": 60}"#, json!({ "to": "x", "amount": 60 })),
                send(10, r#"{"to": "x"}"#, json!({ "to": "x" })),
                send(20, r#"{"to": "x", "amount": 50}"#, json!({ "to": "x", "amount": 50 })),
            ],
            vec!["call 1 has no `amount`: it adds 0", "call 2: 60 + 50 = 110, more than 100"],
        ),
        Case {
            differs: vec![(1, "deny", "spec P1: a negative value adds 0 to sum_within, so Paigasus sums 0 + 120 = 120 and denies")],
            ..Case::new(
                "negative_value",
                "sum_within",
                sum_send("corpus-negative"),
                vec![
                    send(0, r#"{"to": "x", "amount": -50}"#, json!({ "to": "x", "amount": -50 })),
                    send(10, r#"{"to": "x", "amount": 120}"#, json!({ "to": "x", "amount": 120 })),
                ],
                vec!["call 1: Dogwood sums -50 + 120 = 70 and allows"],
            )
        },
        Case {
            nonconforming: vec![1],
            differs: vec![(
                2,
                "deny",
                "plan decision D-C and spec P1: the string amount \"4000\" is stored as a digest (Mistyped with no history key, D-B); a non-Long value at the sum_within path of an event whose predicate is true makes the slot missing, so Paigasus denies",
            )],
            ..Case::new(
                "unknown_value",
                "sum_within",
                sum_send("corpus-unknown"),
                vec![
                    send(0, r#"{"to": "x", "amount": 60}"#, json!({ "to": "x", "amount": 60 })),
                    send(10, r#"{"to": "x", "amount": "4000"}"#, json!({ "to": "x", "amount": "4000" })),
                    send(20, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                    send(3_620, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                ],
                vec![
                    "call 1: Dogwood refuses the string amount as nonconforming, denies it and does not record it",
                    "call 2: Dogwood sums 60 + 30 = 90",
                    "call 3 at 3620 s: the window [20, 3620] holds calls 2 and 3 only",
                ],
            )
        },
        Case {
            differs: vec![
                (
                    1,
                    "deny",
                    "spec P1, the decided-call rule: the decided call of the summed tool has unparsed arguments and its predicate is true, so the slot is missing and Paigasus denies",
                ),
                (
                    2,
                    "deny",
                    "plan decision D-C: an event in the window whose predicate is true has unparsed arguments, so the slot is missing and Paigasus denies",
                ),
            ],
            ..Case::new(
                "decided_call_unparsed",
                "sum_within",
                sum_send("corpus-unparsed"),
                vec![
                    send(0, r#"{"to": "x", "amount": 60}"#, json!({ "to": "x", "amount": 60 })),
                    send(10, "send 50 to x", json!({})),
                    send(20, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                    send(3_620, r#"{"to": "x", "amount": 30}"#, json!({ "to": "x", "amount": 30 })),
                ],
                vec![
                    "call 1 is free text: Dogwood gets no input fields and sums 60",
                    "call 2: Dogwood sums 60 + 30 = 90",
                    "call 3 at 3620 s: the window [20, 3620] holds calls 2 and 3 only",
                ],
            )
        },
    ]
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

#[test]
fn the_decision_corpus_is_current() {
    let formerly_sees = formerly_sees_the_decided_event();
    for case in pr1_cases().into_iter().chain(pr2_cases()) {
        let (schema_path, schema) = case.schema;
        let compiled = compile_cedarschema(SetKind::Tenant, &case.source, schema, DAY_MS).unwrap_or_else(|rejections| panic!("{}: {rejections:#?}", case.name));
        assert_eq!(compiled.conditions.len(), case.conditions, "{}", case.name);
        for (index, call) in case.calls.iter().enumerate() {
            let named = match serde_json::from_str::<Value>(call.arguments) {
                Ok(args) => names_the_same_value(&call.input, &args, call.arguments),
                Err(_) => call.input == json!({}),
            };
            assert!(named, "{}: event {index}: the input is not the Dogwood value of {:?}", case.name, call.arguments);
        }
        let decisions = dogwood_decisions(&case.source, schema, &case.calls, &case.nonconforming);
        let events: Vec<Value> = case
            .calls
            .iter()
            .map(|call| {
                let mut event = json!({ "ts": call.ts, "kind": call.kind.name(), "tool": call.tool, "principal": PRINCIPAL, "resource": RESOURCE });
                let members = event.as_object_mut().expect("an event object");
                if let Some(key) = case.api_key {
                    members.insert("api_key".to_string(), json!(key));
                }
                members.insert("input".to_string(), call.input.clone());
                members.insert("arguments".to_string(), json!(call.arguments));
                event
            })
            .collect();
        let differs: Vec<Value> = case
            .differs
            .iter()
            .map(|(index, paigasus, rule)| {
                let dogwood = decisions[*index].unwrap_or_else(|| panic!("{}: event {index} is a response", case.name));
                assert_ne!(dogwood, *paigasus, "{}: call {index} does not differ", case.name);
                json!({ "event": index, "dogwood": dogwood, "paigasus": paigasus, "rule": rule })
            })
            .collect();
        let actual = json!({
            "shape": case.shape,
            "schema": schema_path,
            "timestamp_unit": "seconds",
            "formerly_sees_the_decided_event": formerly_sees,
            "policy": case.source,
            "compiled": compiled,
            "events": events,
            "decisions": decisions,
            "notes": case.notes,
            "paigasus_differs": differs,
        });
        let path = corpus_dir().join(format!("{}.json", case.name));
        if std::env::var("PAIGASUS_WRITE_FIXTURES").is_ok_and(|v| v == "1") {
            fs::create_dir_all(corpus_dir()).expect("the corpus directory");
            fs::write(&path, serde_json::to_string_pretty(&actual).expect("pretty JSON") + "\n").expect("the corpus writes");
        }
        let committed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))).expect("corpus JSON");
        assert_eq!(committed, actual, "{} is not current; run with PAIGASUS_WRITE_FIXTURES=1", path.display());
    }
}
