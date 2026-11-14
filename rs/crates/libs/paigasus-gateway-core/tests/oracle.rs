// SPDX-License-Identifier: Apache-2.0
//! Tests of the oracle models themselves (SMA-757).

mod support;

use paigasus_gateway_core::{Decision, Format, OnFailure, ToolCall, parse_body};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::driver::{allow_all, drive, limits};
use support::oracle::{Run, check_i1, partial_json};
use support::responses_oracle::{RESPONSES_RUNNERS, run_langchain, run_node, run_python_raw, run_python_stream};

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
        // In P-M3 (shape 0) only LangChain holds a prefix (spec § 2.2), and it parses partial
        // JSON: a prefix that lacks only the closing `"}` parses to the full value, so it is not a
        // strict prefix in value. So the cut is narrowed for shape 0 only. In the other shapes
        // openai-python holds the raw prefix, so every cut counts.
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
    // A non-string token at a key position raises, even in partial mode (jiter 0.19.0).
    assert!(partial_json("{-").is_err());
    assert!(partial_json(r#"{"a":1,1e"#).is_err());
    assert_eq!(partial_json(r#"{"a":1,"b":1e"#).unwrap(), json!({"a":1}));
}

/// Only a proper prefix of a valid JSON number is dropped at the end of the input. jiter 0.19.0
/// raises InvalidNumber on a token that no continuation makes valid.
#[test]
fn partial_json_raises_on_a_token_that_is_no_number_prefix() {
    assert_eq!(partial_json(r#"{"n":1e-"#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"n":-1.5E+"#).unwrap(), json!({}));
    for bad in [r#"{"n":01"#, r#"{"n":1e--"#, r#"{"n":+1"#, r#"{"n":--"#, r#"{"n":1.e"#, r#"{"n":-0.."#] {
        assert!(partial_json(bad).is_err(), "{bad} must raise");
    }
}

// ---- Responses (SMA-769 § 10.1) ----------------------------------------------------------------

/// The D13 frame the gateway emits when the upstream stream fails.
const D13: &str = "\n\nevent: error\ndata: {\"type\":\"error\",\"code\":\"upstream-error\",\"message\":\"upstream stream error\",\"param\":null}\n\n";

fn frame(v: &Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().expect("a typed event"))
}

fn lifecycle(t: &str, output: Value) -> Value {
    let status = if t == "response.completed" { "completed" } else { "in_progress" };
    json!({"type": t, "response": {"id": "resp_1", "object": "response", "status": status, "output": output}})
}

fn scoped(t: &str, i: u32, item_id: &str, extra: Value) -> Value {
    let mut v = json!({"type": t, "output_index": i, "item_id": item_id});
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}

/// One output item: its events and its `done` item.
struct Item {
    evs: Vec<Value>,
    done: Value,
}

fn with_status(item: &Value, status: &str) -> Value {
    let mut v = item.clone();
    v["status"] = json!(status);
    v
}

fn added(i: u32, item: Value) -> Value {
    json!({"type": "response.output_item.added", "output_index": i, "item": item})
}

fn item_done(i: u32, item: &Value) -> Value {
    json!({"type": "response.output_item.done", "output_index": i, "item": item})
}

fn function_item(i: u32, id: &str, call_id: &str, name: &str, namespace: Option<&str>, deltas: &[&str]) -> Item {
    let args = deltas.concat();
    let mut done = json!({"type": "function_call", "id": id, "status": "completed", "call_id": call_id, "name": name, "arguments": args});
    if let Some(ns) = namespace {
        done["namespace"] = json!(ns);
    }
    let mut open = with_status(&done, "in_progress");
    open["arguments"] = json!("");
    let mut evs = vec![added(i, open)];
    for d in deltas {
        evs.push(scoped("response.function_call_arguments.delta", i, id, json!({"delta": d})));
    }
    evs.push(scoped("response.function_call_arguments.done", i, id, json!({"arguments": args})));
    evs.push(item_done(i, &done));
    Item { evs, done }
}

fn custom_item(i: u32, id: &str, call_id: &str, name: &str, input: &str) -> Item {
    let done = json!({"type": "custom_tool_call", "id": id, "status": "completed", "call_id": call_id, "name": name, "input": input});
    let mut open = with_status(&done, "in_progress");
    open["input"] = json!("");
    let evs = vec![
        added(i, open),
        scoped("response.custom_tool_call_input.delta", i, id, json!({"delta": input})),
        scoped("response.custom_tool_call_input.done", i, id, json!({"input": input})),
        item_done(i, &done),
    ];
    Item { evs, done }
}

/// An `apply_patch_call` that deletes a file: no diff, so no diff events (R14).
fn delete_patch_item(i: u32) -> Item {
    let done = json!({"type": "apply_patch_call", "id": "apc_1", "status": "completed", "call_id": "call_3", "operation": {"type": "delete_file", "path": "old.txt"}});
    let evs = vec![added(i, with_status(&done, "in_progress")), item_done(i, &done)];
    Item { evs, done }
}

/// An `apply_patch_call` that creates a file, with its diff events (R14, as recorded).
fn diff_patch_item(i: u32) -> Item {
    let done = json!({"type": "apply_patch_call", "id": "apc_1", "status": "completed", "call_id": "call_3", "operation": {"type": "create_file", "diff": "+hi\n", "path": "a.txt"}});
    let mut open = with_status(&done, "in_progress");
    open["operation"]["diff"] = json!("");
    let evs = vec![
        added(i, open),
        scoped("response.apply_patch_call_operation_diff.delta", i, "apc_1", json!({"delta": "+hi"})),
        scoped("response.apply_patch_call_operation_diff.delta", i, "apc_1", json!({"delta": "\n"})),
        scoped("response.apply_patch_call_operation_diff.done", i, "apc_1", json!({"diff": "+hi\n"})),
        item_done(i, &done),
    ];
    Item { evs, done }
}

/// A `shell_call` as recorded: the commands stream as `shell_call_command.*` events (R13).
fn shell_item(i: u32) -> Item {
    let done = json!({"type": "shell_call", "id": "sh_1", "status": "completed", "call_id": "call_4",
        "action": {"commands": ["ls -la"], "max_output_length": 20000, "timeout_ms": null}, "environment": null});
    let open = json!({"type": "shell_call", "id": "sh_1", "status": "in_progress", "call_id": "call_4",
        "action": {"commands": [], "max_output_length": null, "timeout_ms": null}, "environment": null});
    let cmd = |t: &str, extra: Value| {
        let mut v = json!({"type": t, "command_index": 0, "output_index": i});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    let evs = vec![
        added(i, open),
        cmd("response.shell_call_command.added", json!({"command": ""})),
        cmd("response.shell_call_command.delta", json!({"delta": "ls"})),
        cmd("response.shell_call_command.delta", json!({"delta": " -la"})),
        cmd("response.shell_call_command.done", json!({"command": "ls -la"})),
        item_done(i, &done),
    ];
    Item { evs, done }
}

fn web_item(i: u32) -> Item {
    let done = json!({"type": "web_search_call", "id": "ws_1", "status": "completed", "action": {"type": "search", "query": "q"}});
    let evs = vec![
        added(i, json!({"type": "web_search_call", "id": "ws_1", "status": "in_progress"})),
        scoped("response.web_search_call.in_progress", i, "ws_1", json!({})),
        scoped("response.web_search_call.searching", i, "ws_1", json!({})),
        scoped("response.web_search_call.completed", i, "ws_1", json!({})),
        item_done(i, &done),
    ];
    Item { evs, done }
}

/// `created`, `in_progress`, the items, and `completed` with their `done` items.
fn conforming(items: Vec<Item>) -> Vec<Value> {
    let mut evs = vec![lifecycle("response.created", json!([])), lifecycle("response.in_progress", json!([]))];
    let mut output = Vec::new();
    for it in items {
        evs.extend(it.evs);
        output.push(it.done);
    }
    evs.push(lifecycle("response.completed", Value::Array(output)));
    evs
}

/// The events as SSE frames, with `extra` (raw SSE text) before event `at`, then `[DONE]`.
fn render_with(evs: &[Value], at: usize, extra: &str) -> Vec<u8> {
    let mut s = String::new();
    for (i, e) in evs.iter().enumerate() {
        if i == at {
            s += extra;
        }
        s += &frame(e);
    }
    if at >= evs.len() {
        s += extra;
    }
    s += "data: [DONE]\n\n";
    s.into_bytes()
}

fn render_responses(evs: &[Value]) -> Vec<u8> {
    render_with(evs, usize::MAX, "")
}

/// The calls the decider allowed: the non-stream body with the same items.
fn decided_of(output: &[Value]) -> Vec<(ToolCall, Decision)> {
    let body = json!({"id": "resp_1", "object": "response", "status": "completed", "output": output});
    let parsed = parse_body(Format::OpenAiResponses, body.to_string().as_bytes(), 1 << 20).expect("the body parses");
    parsed.calls.into_iter().map(|c| (c, Decision::Allow)).collect()
}

type View = (&'static str, String, Option<u32>, Option<String>, Option<String>);

/// What the matcher compares besides the arguments, sorted by call id.
fn view(r: &Run) -> Vec<View> {
    let mut v: Vec<View> = r.calls.iter().map(|c| (c.kind, c.name.clone(), c.position, c.id.clone(), c.namespace.clone())).collect();
    v.sort_by(|a, b| a.3.cmp(&b.3));
    v
}

fn s(x: &str) -> Option<String> {
    Some(x.to_owned())
}

fn runner(name: &str) -> fn(&[u8]) -> Run {
    RESPONSES_RUNNERS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r).expect("a responses model")
}

/// Every `data:` JSON payload of `out`, changed by `f`; the rest of the bytes stays.
fn mutate(out: &[u8], f: impl Fn(&mut Value)) -> Vec<u8> {
    let text = String::from_utf8(out.to_vec()).expect("UTF-8 output");
    let mut lines = Vec::new();
    for line in text.split('\n') {
        match line.strip_prefix("data: ").map(serde_json::from_str::<Value>) {
            Some(Ok(mut v)) => {
                f(&mut v);
                lines.push(format!("data: {v}"));
            }
            _ => lines.push(line.to_owned()),
        }
    }
    lines.join("\n").into_bytes()
}

#[test]
fn each_model_reads_a_conforming_stream() {
    let evs = conforming(vec![
        function_item(0, "fc_1", "call_1", "read_file", Some("fs"), &[r#"{"path":"#, r#""a"}"#]),
        custom_item(1, "ctc_1", "call_2", "sql", "SELECT 1"),
        delete_patch_item(2),
        web_item(3),
    ]);
    let input = render_responses(&evs);
    let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&input], &allow_all);
    assert!(!o.core_failed(), "{:?}", o.metrics);
    assert_eq!(o.decided.len(), 3, "the web search is provider-run, not decided");
    assert_eq!(check_i1(Format::OpenAiResponses, &input, &o.out, &o.decided, false), Ok(()));
    let full: Vec<View> = vec![
        ("function", "read_file".into(), Some(0), s("call_1"), s("fs")),
        ("custom", "sql".into(), Some(1), s("call_2"), None),
        ("apply_patch", "apply_patch".into(), Some(2), s("call_3"), None),
    ];
    // LangChain's tool calls carry no output index; the namespace is on the call's content
    // block; the patch is a content block.
    let langchain: Vec<View> = vec![
        ("function", "read_file".into(), None, s("call_1"), s("fs")),
        ("custom", "sql".into(), None, s("call_2"), None),
        ("apply_patch", "apply_patch".into(), None, s("call_3"), None),
    ];
    for (name, run) in RESPONSES_RUNNERS {
        let r = run(&o.out);
        assert_eq!(r.error, None, "{name}");
        let want = if *name == "langchain-responses" { &langchain } else { &full };
        assert_eq!(&view(&r), want, "{name}: {:?}", r.calls);
    }
    let lc = run_langchain(&o.out);
    let args: Vec<&str> = lc.calls.iter().map(|c| c.arguments.as_str()).collect();
    assert!(args.contains(&r#"{"path":"a"}"#) && args.contains(&r#"{"__arg1":"SELECT 1"}"#), "{args:?}");
}

#[test]
fn each_raise_path_raises() {
    let base = || conforming(vec![function_item(0, "fc_1", "call_1", "read_file", None, &["{}"]), web_item(1)]);
    let evs = base();
    let terminal = evs.len() - 1;
    let failed = {
        let mut e = base();
        e[terminal]["type"] = json!("response.failed");
        e
    };
    let other_item_id = {
        let mut e = base();
        e[3]["item_id"] = json!("fc_other"); // the arguments delta of item 0
        e
    };
    let null_item = "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":2,\"item\":null}\n\n";
    let truthy_error = "data: {\"error\":{\"message\":\"boom\"}}\n\n";
    let patch = render_responses(&conforming(vec![diff_patch_item(0)]));
    let rows: Vec<(&str, &str, Vec<u8>)> = vec![
        ("openai-node-responses", "event: error", render_with(&evs, 2, "event: error\ndata: {\"type\":\"keepalive\"}\n\n")),
        ("openai-node-responses", "unknown event type", render_with(&evs, 2, "data: {\"type\":\"response.unknown_kind\"}\n\n")),
        ("openai-node-responses", "apply_patch diff event", patch.clone()),
        ("openai-node-responses", "item_id differs from the item id", render_responses(&other_item_id)),
        ("openai-node-responses", "type: error record", render_with(&evs, terminal, D13)),
        ("openai-python-responsestream", "truthy error", render_with(&evs, 2, truthy_error)),
        ("openai-python-raw", "truthy error", render_with(&evs, 2, truthy_error)),
        ("openai-node-responses", "truthy error", render_with(&evs, 2, truthy_error)),
        ("langchain-responses", "truthy error", render_with(&evs, 2, truthy_error)),
        ("langchain-responses", "item: null", render_with(&evs, terminal, null_item)),
        ("langchain-responses", "type: error record", render_with(&evs, terminal, D13)),
        ("langchain-responses", "response.failed", render_responses(&failed)),
        ("openai-python-responsestream", "response.failed at get_final_response()", render_responses(&failed)),
        ("openai-python-responsestream", "no response.created first", render_responses(&evs[1..])),
        (
            "openai-python-raw",
            "invalid UTF-8",
            render_with(&evs, 2, "data: {\"type\":\"keepalive\",\"x\":\"\u{0}\"}\n\n")
                .into_iter()
                .map(|b| if b == 0 { 0xff } else { b })
                .collect(),
        ),
        ("openai-python-raw", "invalid JSON", render_with(&evs, 2, "data: {x\n\n")),
    ];
    for (model, what, bytes) in &rows {
        let r = runner(model)(bytes);
        assert!(r.error.is_some(), "{model} must raise on {what}: {:?}", r.calls);
    }
    // The raw stream raises only at the SSE layer.
    let quiet: Vec<(&str, Vec<u8>)> = vec![
        ("response.failed", render_responses(&failed)),
        ("type: error record", render_with(&evs, terminal, D13)),
        ("item: null", render_with(&evs, terminal, null_item)),
        ("unknown event type", render_with(&evs, 2, "data: {\"type\":\"response.unknown_kind\"}\n\n")),
        ("apply_patch diff event", patch),
    ];
    for (what, bytes) in &quiet {
        assert_eq!(run_python_raw(bytes).error, None, "the raw stream must not raise on {what}");
    }
}

#[test]
fn models_see_every_surface() {
    // R-a, R-b: the terminal `output` item differs from the `done` item.
    let item = function_item(0, "fc_1", "call_1", "read_file", None, &[r#"{"path":"a"}"#]);
    let decided = decided_of(std::slice::from_ref(&item.done));
    let mut evs = conforming(vec![item]);
    let last = evs.len() - 1;
    evs[last]["response"]["output"][0]["arguments"] = json!(r#"{"path":"b"}"#);
    let bytes = render_responses(&evs);
    for run in [run_python_stream, run_node, run_python_raw] {
        let r = run(&bytes);
        let mut args: Vec<&str> = r.calls.iter().map(|c| c.arguments.as_str()).collect();
        args.sort_unstable();
        assert_eq!(args, [r#"{"path":"a"}"#, r#"{"path":"b"}"#], "{:?}", r.error);
    }
    let e = check_i1(Format::OpenAiResponses, &bytes, &bytes, &decided, false).expect_err("the terminal view was never decided");
    assert!(e.contains(r#"{\"path\":\"b\"}"#), "{e}");

    // R-e: LangChain adds a function-call arguments delta to the chunk at its index, whatever
    // the item type there.
    let item = custom_item(0, "ctc_1", "call_2", "sql", "SELECT 1");
    let decided = decided_of(std::slice::from_ref(&item.done));
    let evs = conforming(vec![item]);
    let delta = frame(&scoped("response.function_call_arguments.delta", 0, "ctc_1", json!({"delta": r#"{"x":1}"#})));
    let bytes = render_with(&evs, evs.len() - 2, &delta); // before the custom call's `done`
    let lc = run_langchain(&bytes);
    assert_eq!(lc.error, None);
    assert!(lc.calls.iter().any(|c| c.name == "sql" && c.arguments == r#"{"x":1}"#), "{:?}", lc.calls);
    let e = check_i1(Format::OpenAiResponses, &bytes, &bytes, &decided, true).expect_err("the delta became call data");
    assert!(e.starts_with("langchain-responses"), "{e}");
}

/// The mutation proof of § 10.1: a changed command or patch in the emitted item must fail P1.
fn changed_item_fails(item: Item, change: fn(&mut Value)) {
    let input = render_responses(&conforming(vec![item]));
    let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&input], &allow_all);
    assert_eq!(o.decided.len(), 1);
    assert_eq!(check_i1(Format::OpenAiResponses, &input, &o.out, &o.decided, o.core_failed()), Ok(()), "the control passes");
    let mutated = mutate(&o.out, |v| match v["type"].as_str() {
        Some("response.output_item.done") => change(&mut v["item"]),
        Some("response.completed") => change(&mut v["response"]["output"][0]),
        _ => {}
    });
    assert_ne!(mutated, o.out);
    let e = check_i1(Format::OpenAiResponses, &input, &mutated, &o.decided, o.core_failed()).expect_err("a changed item must fail P1");
    assert!(e.contains("undecided or blocked call"), "{e}");
}

#[test]
fn a_changed_shell_command_fails_p1() {
    changed_item_fails(shell_item(0), |item| item["action"]["commands"][0] = json!("rm -rf /"));
}

#[test]
fn a_changed_patch_fails_p1() {
    changed_item_fails(diff_patch_item(0), |item| item["operation"]["diff"] = json!("+evil\n"));
}

type Change = (&'static str, fn(&mut Value));

/// LangChain reads a function call's namespace from the `added` item, into the `function_call`
/// content block at the call's content index (langchain-openai 1.8.0 `base.py:5587-5588`). A
/// namespace that changes at `added` only reaches LangChain alone, and must fail P1.
#[test]
fn langchain_reads_the_namespace_from_the_content_block() {
    let item = function_item(0, "fc_1", "call_1", "read_file", Some("fs"), &["{}"]);
    let input = render_responses(&conforming(vec![item]));
    let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&input], &allow_all);
    let lc = run_langchain(&o.out);
    assert_eq!(lc.calls.iter().map(|c| c.namespace.clone()).collect::<Vec<_>>(), [s("fs")]);
    let mutated = mutate(&o.out, |v| {
        if v["type"] == "response.output_item.added" {
            v["item"]["namespace"] = json!("other");
        }
    });
    assert_eq!(run_langchain(&mutated).calls[0].namespace, s("other"));
    let e = check_i1(Format::OpenAiResponses, &input, &mutated, &o.decided, false).expect_err("LangChain runs another namespace");
    assert!(e.starts_with("langchain-responses runs an undecided or blocked call"), "{e}");
}

#[test]
fn a_changed_position_id_or_namespace_fails() {
    let item = function_item(0, "fc_1", "call_1", "read_file", Some("fs"), &["{}"]);
    let input = render_responses(&conforming(vec![item]));
    let o = drive(Format::OpenAiResponses, limits(OnFailure::Block), &[&input], &allow_all);
    assert_eq!(check_i1(Format::OpenAiResponses, &input, &o.out, &o.decided, false), Ok(()), "the control passes");
    let changes: [Change; 3] = [
        // On every surface, so that only the position differs: the item moves to index 1 behind
        // a message in the terminal `output`.
        ("position", |v| {
            if v.get("output_index").is_some() {
                v["output_index"] = json!(1);
            }
            if v["type"] == "response.completed" {
                let msg = json!({"type": "message", "id": "msg_1", "status": "completed", "role": "assistant", "content": []});
                v["response"]["output"].as_array_mut().unwrap().insert(0, msg);
            }
        }),
        // On every surface: openai-python raises when only the `done` item changes its identity.
        ("id", |v| match v["type"].as_str() {
            Some("response.output_item.added" | "response.output_item.done") => v["item"]["call_id"] = json!("call_9"),
            Some("response.completed") => v["response"]["output"][0]["call_id"] = json!("call_9"),
            _ => {}
        }),
        ("namespace", |v| match v["type"].as_str() {
            Some("response.output_item.done") => v["item"]["namespace"] = json!("other"),
            Some("response.completed") => v["response"]["output"][0]["namespace"] = json!("other"),
            _ => {}
        }),
    ];
    for (what, change) in changes {
        let mutated = mutate(&o.out, change);
        let e = check_i1(Format::OpenAiResponses, &input, &mutated, &o.decided, false).expect_err(what);
        assert!(e.starts_with("openai-python-responsestream runs an undecided or blocked call"), "{what}: {e}");
    }
}

fn split_call_stream(args: &str, k: usize) -> (Vec<u8>, Vec<(ToolCall, Decision)>) {
    let item = function_item(0, "fc_1", "call_1", "read_file", None, &[&args[..k], &args[k..]]);
    let decided = decided_of(std::slice::from_ref(&item.done));
    let evs = conforming(vec![item]);
    // created, in_progress, added, delta 1, | D13 |, delta 2, ...
    (render_with(&evs, 4, D13), decided)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// 757 D2 on the Responses form: an `error` event between two argument deltas, with the
    /// raise excused (`core_failed`), must still fail, because LangChain holds a prefix of the
    /// allowed arguments in its tool call. openai-node rejects `finalResponse()` at the raise, so
    /// it shows no call there (ruling for § 10.1); it raises with an empty snapshot.
    #[test]
    fn a_raise_between_two_argument_deltas_is_an_err(s in "[a-z]{4,16}", cut in 0usize..1000) {
        let args = format!(r#"{{"v":"{s}"}}"#);
        // A prefix that lacks only `"}` or `}` parses to the full value in `parse_partial_json`,
        // so it is no strict prefix in value: the cut leaves out at least the last letter.
        let k = 1 + cut % (args.len() - 3);
        let (bytes, decided) = split_call_stream(&args, k);
        let e = check_i1(Format::OpenAiResponses, &bytes, &bytes, &decided, true).expect_err("a prefix at a raise must fail");
        prop_assert!(e.starts_with("langchain-responses"), "{e}");
        prop_assert!(e.contains("undecided or blocked call"), "{e}");
        let lc = run_langchain(&bytes);
        prop_assert!(lc.error.is_some());
        let node = run_node(&bytes);
        prop_assert!(node.error.is_some());
        prop_assert!(node.calls.is_empty(), "{:?}", node.calls);
    }
}
