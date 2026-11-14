// SPDX-License-Identifier: Apache-2.0
//! Tests of the oracle models themselves (SMA-757).

mod support;

use paigasus_gateway_core::{Decision, Format, ToolCall, parse_body};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::oracle::{check_i1, partial_json};

/// One SSE data record.
fn rec(v: Value) -> String {
    format!("data: {v}\n\n")
}

fn chunk(choices: Value) -> Value {
    json!({"id":"c","object":"chat.completion.chunk","created":1,"model":"m","choices":choices})
}

/// A fragment of call 0. The first one carries the id, the type and the name.
fn fragment(first: bool, args: &str) -> String {
    let call = if first {
        json!({"index":0,"id":"call_0","type":"function","function":{"name":"read_file","arguments":args}})
    } else {
        json!({"index":0,"function":{"arguments":args}})
    };
    rec(chunk(json!([{"index":0,"delta":{"tool_calls":[call]}}])))
}

/// The four raise records of spec § 2.2: P-M3, P-a, P-b, P-c.
fn raise_record(shape: usize) -> String {
    match shape {
        0 => rec(chunk(
            json!([{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_1","type":"custom","custom":{"name":"note","input":"hi"}}]}}]),
        )),
        1 => rec(json!({"id":"c","object":"chat.completion.chunk","created":1,"model":"m"})),
        2 => rec(chunk(json!([{"index":0}]))),
        _ => rec(chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"type":null}]}}]))),
    }
}

/// The calls the decider allowed: a `chat.completion` body with the same calls and full arguments.
fn decided(shape: usize, args: &str) -> Vec<(ToolCall, Decision)> {
    let mut calls = vec![json!({"id":"call_0","type":"function","function":{"name":"read_file","arguments":args}})];
    if shape == 0 {
        calls.push(json!({"id":"call_1","type":"custom","custom":{"name":"note","input":"hi"}}));
    }
    let body = json!({"id":"c","object":"chat.completion","created":1,"model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":calls},"finish_reason":"tool_calls"}]});
    let parsed = parse_body(Format::OpenAiChat, body.to_string().as_bytes(), 1 << 20).expect("the body parses");
    parsed.calls.into_iter().map(|c| (c, Decision::Allow)).collect()
}

fn stream(shape: usize, args: &str, k: usize, raise_last: bool) -> Vec<u8> {
    let role = rec(chunk(json!([{"index":0,"delta":{"role":"assistant","content":null}}])));
    let finish = rec(chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}])));
    let mut s = role + &fragment(true, &args[..k]);
    if raise_last {
        s += &fragment(false, &args[k..]);
        s += &raise_record(shape);
    } else {
        s += &raise_record(shape);
        s += &fragment(false, &args[k..]);
    }
    s += &finish;
    s += "data: [DONE]\n\n";
    s.into_bytes()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// A call in a snapshot at an excused raise must match an allowed call exactly: a strict
    /// prefix of the arguments is never accepted (spec § 4).
    #[test]
    fn a_prefix_at_an_excused_raise_is_rejected(s in "[a-z]{4,16}", cut in 0usize..1000, shape in 0usize..4) {
        let args = format!(r#"{{"v":"{s}"}}"#);
        // P-M3 (shape 0) is read by langchain, which parses partial JSON: a prefix that lacks only
        // the closing `"}` parses to the full value, so it is not a strict prefix in value. In the
        // other shapes openai-python holds the raw prefix, so every cut counts.
        let span = if shape == 0 { args.len() - 3 } else { args.len() - 1 };
        let k = 1 + cut % span;
        let bytes = stream(shape, &args, k, false);
        let d = decided(shape, &args);
        let e = check_i1(Format::OpenAiChat, &bytes, &bytes, &d, false).expect_err("a prefix must be rejected");
        prop_assert!(e.contains("undecided or blocked call"), "{e}");
        prop_assert!(e.contains("read_file"), "{e}");
        let model = if shape == 0 { "langchain" } else { "openai-python" };
        prop_assert!(e.starts_with(model), "{e}");
    }

    #[test]
    fn a_raise_after_the_last_fragment_is_excused(s in "[a-z]{4,16}", cut in 0usize..1000, shape in 0usize..4) {
        let args = format!(r#"{{"v":"{s}"}}"#);
        let k = 1 + cut % (args.len() - 1);
        let bytes = stream(shape, &args, k, true);
        let d = decided(shape, &args);
        prop_assert_eq!(check_i1(Format::OpenAiChat, &bytes, &bytes, &d, false), Ok(()));
    }
}

#[test]
fn partial_json_drops_an_unfinished_number() {
    assert_eq!(partial_json(r#"{"n":10."#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"n":1e"#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"n":-"#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"a":"x","n":10."#).unwrap(), json!({"a":"x"}));
    assert_eq!(partial_json(r#"{"n":10"#).unwrap(), json!({"n":10}));
    assert!(partial_json(r#"{"n":1x}"#).is_err()); // invalid, not at the end of input
}
