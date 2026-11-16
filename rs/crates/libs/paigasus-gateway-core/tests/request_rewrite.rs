// SPDX-License-Identifier: Apache-2.0
//! The request rewrite (SMA-752 § 6.5, P-8, P-9). Each fixture pair in
//! `tests/fixtures/request_rewrite/` is built from the provider API reference, not from a live
//! call: every case is "not measured" (see the README there).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use paigasus_gateway_core::{
    ChoiceRewrite, Format, OnUnavailable, PolicySet, PolicySnapshot, RequestPlan, RequestReason, TenantMode, TenantPart, ToolChoice, ToolTurn, TurnFacts, ingress, parse_request, plan_request,
    result_notice, rewrite_request,
};
use paigasus_kernel::tool_policy::{Facts, RequestInfo, SessionInfo, ToolPolicies, normalise};
use paigasus_kernel::{Prn, to_cedar_uid};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use uuid::Uuid;

const PERMIT_ALL: &str = r#"@id("all") permit(principal, action, resource);"#;

fn forbid_text(tools: &[&str]) -> String {
    let forbids: String = tools
        .iter()
        .enumerate()
        .map(|(i, t)| format!(r#" @id("x{i}") forbid(principal, action == Pgs::Gateway::Action::"{t}", resource);"#))
        .collect();
    format!("{PERMIT_ALL}{forbids}")
}

fn facts(format: Format) -> Facts {
    let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("prn");
    Facts::new(
        to_cedar_uid(&prn),
        Uuid::nil(),
        RequestInfo::new("prn:pgs:gateway:::scope/x", ingress(format), "m"),
        SessionInfo::placeholder(),
    )
}

fn snapshot(format: Format, floor: &str, tenant: &str, mode: TenantMode) -> PolicySnapshot {
    let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
    let set = PolicySet::Available {
        policies: Arc::new(ToolPolicies::compile(floor, tenant, &[]).expect("compile")),
        tenant: TenantPart::Evaluated,
        facts: facts(format),
    };
    PolicySnapshot::new(set, mode, OnUnavailable::Closed)
}

/// An enforce-mode tenant that permits everything and forbids `tools` for every input.
fn enforce(format: Format, tools: &[&str]) -> PolicySnapshot {
    snapshot(format, "", &forbid_text(tools), TenantMode::Enforce)
}

fn turn(format: Format, body: &[u8]) -> TurnFacts {
    match parse_request(format, body) {
        ToolTurn::Parsed(f) => f,
        other => panic!("not parsed: {other:?}"),
    }
}

fn plan(snap: &PolicySnapshot, format: Format, body: &[u8]) -> RequestPlan {
    plan_request(snap, format, &turn(format, body))
}

fn rewrite(format: Format, body: &[u8], plan: &RequestPlan) -> String {
    let out = rewrite_request(format, body, plan).expect("the plan changes the body");
    String::from_utf8(out.to_vec()).expect("utf-8")
}

/// The fixture cases: name, format, and the tools that the fixture snapshot forbids.
const CASES: &[(&str, Format, &[&str])] = &[
    ("chat_remove_one_of_two", Format::OpenAiChat, &["delete_file"]),
    ("chat_remove_last_tool", Format::OpenAiChat, &["delete_file"]),
    ("chat_legacy_functions_last", Format::OpenAiChat, &["delete_file"]),
    ("chat_named_reset_to_auto", Format::OpenAiChat, &["delete_file"]),
    ("chat_allowed_tools_narrowed", Format::OpenAiChat, &["delete_file"]),
    ("chat_tool_result_replaced", Format::OpenAiChat, &["delete_file"]),
    ("chat_legacy_function_result_replaced", Format::OpenAiChat, &["delete_file"]),
    ("responses_remove_function_tool", Format::OpenAiResponses, &["delete_file"]),
    ("responses_remove_last_tool", Format::OpenAiResponses, &["delete_file"]),
    ("responses_namespace_emptied", Format::OpenAiResponses, &["db.query", "fs.delete"]),
    (
        "responses_outputs_replaced",
        Format::OpenAiResponses,
        &["delete_file", "run_sql", "computer", "local_shell", "shell", "apply_patch"],
    ),
    ("responses_mcp_approval_refused", Format::OpenAiResponses, &["srv.search"]),
    ("anthropic_remove_tool", Format::AnthropicMessages, &["delete_file"]),
    ("anthropic_last_tool_no_blocks", Format::AnthropicMessages, &["delete_file"]),
    ("anthropic_last_tool_kept_with_tool_result", Format::AnthropicMessages, &["delete_file"]),
    ("anthropic_tool_result_replaced", Format::AnthropicMessages, &["delete_file"]),
];

fn fixture(name: &str, suffix: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/request_rewrite").join(format!("{name}.{suffix}.json"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn every_fixture_rewrites_to_its_expected_bytes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/request_rewrite");
    let on_disk = std::fs::read_dir(&dir)
        .expect("dir")
        .filter(|e| e.as_ref().is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".in.json")))
        .count();
    assert_eq!(on_disk, CASES.len(), "every fixture pair has a case row");
    for (name, format, tools) in CASES {
        let input = fixture(name, "in");
        let p = plan(&enforce(*format, tools), *format, input.as_bytes());
        assert_eq!(rewrite(*format, input.as_bytes(), &p), fixture(name, "out"), "{name}");
    }
}

#[test]
fn the_output_parses_back_to_the_planned_turn() {
    for (name, format, tools) in CASES {
        let input = fixture(name, "in");
        let before = turn(*format, input.as_bytes());
        let p = plan_request(&enforce(*format, tools), *format, &before);
        let after = turn(*format, rewrite(*format, input.as_bytes(), &p).as_bytes());
        let gone: Vec<&str> = p.removed.iter().filter(|i| i.enforced).map(|i| i.target.as_str()).collect();
        let names = |t: &TurnFacts| t.definitions.iter().map(|d| d.display_name()).collect::<Vec<_>>();
        let kept: Vec<String> = names(&before).into_iter().filter(|n| !gone.contains(&normalise(n).as_str())).collect();
        assert_eq!(names(&after), kept, "{name}: no removed definition is left, every other one is");
        let dropped = after.definitions.is_empty() && !before.definitions.is_empty();
        let choice = match (&p.choice, dropped) {
            // `tool_choice` was dropped with the last definition: absent reads as `auto`.
            (_, true) | (ChoiceRewrite::Auto, _) => ToolChoice::Auto,
            (ChoiceRewrite::Allowed(n), _) => ToolChoice::AllowedTools(n.clone()),
            (_, _) => before.tool_choice.clone(),
        };
        assert_eq!(after.tool_choice, choice, "{name}");
        assert_eq!((after.prior_calls, after.tool_results), (before.prior_calls, before.tool_results), "{name}");
    }
}

/// A `local_shell_call_output` with only `id` (the provider schema gives it no `call_id`) is
/// replaced when that `id` is the `call_id` or the item `id` of a denied `local_shell_call`.
#[test]
fn an_id_only_local_shell_output_is_replaced() {
    let notice = Value::from(result_notice("local_shell", RequestReason::ToolResultBlocked));
    for out_id in ["ls_1", "c1"] {
        let body = json!({"model": "m", "input": [
            {"type": "local_shell_call", "id": "ls_1", "call_id": "c1", "action": {"type": "exec", "command": ["env"], "env": {}}, "status": "completed"},
            {"type": "local_shell_call_output", "id": out_id, "output": "SECRET"},
            {"type": "local_shell_call_output", "id": "ls_2", "output": "kept"}
        ]})
        .to_string();
        let p = plan(&enforce(Format::OpenAiResponses, &["local_shell"]), Format::OpenAiResponses, body.as_bytes());
        let out: Value = serde_json::from_str(&rewrite(Format::OpenAiResponses, body.as_bytes(), &p)).expect("json");
        assert_eq!(out["input"][1]["output"], notice, "{out_id}");
        assert_eq!(out["input"][2]["output"], json!("kept"), "{out_id}: an id that matches no call stays");
        assert!(!out.to_string().contains("SECRET"), "{out_id}");
        // An allowed `local_shell` changes nothing.
        assert!(plan(&enforce(Format::OpenAiResponses, &["other"]), Format::OpenAiResponses, body.as_bytes()).is_noop());
    }
}

/// A `local_shell_call_output` with a `call_id` that matches a call denied only in log-only and an
/// `id` that matches a floor-denied `local_shell_call`: the enforced replacement wins, once.
#[test]
fn an_enforced_local_shell_id_wins_over_an_unenforced_call_id() {
    let notice = Value::from(result_notice("local_shell", RequestReason::ToolResultBlocked));
    let body = json!({"model": "m", "input": [
        {"type": "function_call", "call_id": "c1", "name": "x", "arguments": "{}"},
        {"type": "local_shell_call", "id": "ls_2", "call_id": "c2", "action": {"type": "exec", "command": ["env"], "env": {}}, "status": "completed"},
        {"type": "local_shell_call_output", "call_id": "c1", "id": "ls_2", "output": "SECRET"}
    ]})
    .to_string();
    let floor = r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"local_shell", resource);"#;
    let snap = snapshot(Format::OpenAiResponses, floor, &forbid_text(&["x"]), TenantMode::LogOnly);
    let p = plan(&snap, Format::OpenAiResponses, body.as_bytes());
    assert!(!p.is_noop());
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiResponses, body.as_bytes(), &p)).expect("json");
    assert_eq!(out["input"][2]["output"], notice);
    assert!(!out.to_string().contains("SECRET"));
}

#[test]
fn a_noop_plan_returns_none() {
    let body = fixture("chat_tool_result_replaced", "in");
    let noop = plan(&enforce(Format::OpenAiChat, &[]), Format::OpenAiChat, body.as_bytes());
    assert!(noop.is_noop());
    assert!(rewrite_request(Format::OpenAiChat, body.as_bytes(), &noop).is_none());
    // A log-only plan has items, but none is enforced.
    let log_only = snapshot(Format::OpenAiChat, "", &forbid_text(&["delete_file"]), TenantMode::LogOnly);
    let p = plan(&log_only, Format::OpenAiChat, body.as_bytes());
    assert_eq!((p.removed.len(), p.replaced.len()), (1, 1));
    assert!(rewrite_request(Format::OpenAiChat, body.as_bytes(), &p).is_none());
}

/// Review Focus 2: every member except `tools` keeps its bytes.
#[test]
fn untouched_members_keep_their_bytes() {
    let body = concat!(
        "\n { \"mod\\u0065l\" :\"gpt-4.1\",  \"seed\":12345678901234567890 ,\n\t\"top_p\": 1.50,\"logit_scale\" : 1e400,",
        " \"tools\" : [ {\"type\":\"function\",\"function\":{\"name\":\"delete_file\"}} ,\n {\"type\":\"function\",\"function\":{\"name\":\"get_weather\"}} ],",
        "\"messages\":[ {\"role\":\"user\",\"content\":\"caf\\u00e9 \\ud83d\\ude00\"} ] , \"temperature\" :0.10e1 }\n"
    );
    // The strict parse refuses `1e400` (not finite), so the plan comes from a twin body with the
    // same tools; the rewrite reads only the plan.
    let twin = json!({"model":"m","tools":[{"type":"function","function":{"name":"delete_file"}},{"type":"function","function":{"name":"get_weather"}}],"messages":[]});
    let p = plan(&enforce(Format::OpenAiChat, &["delete_file"]), Format::OpenAiChat, twin.to_string().as_bytes());
    let out = rewrite(Format::OpenAiChat, body.as_bytes(), &p);
    let expected = concat!(
        "\n { \"mod\\u0065l\" :\"gpt-4.1\",  \"seed\":12345678901234567890 ,\n\t\"top_p\": 1.50,\"logit_scale\" : 1e400,",
        " \"tools\" : [ \n {\"type\":\"function\",\"function\":{\"name\":\"get_weather\"}} ],",
        "\"messages\":[ {\"role\":\"user\",\"content\":\"caf\\u00e9 \\ud83d\\ude00\"} ] , \"temperature\" :0.10e1 }\n"
    );
    assert_eq!(out, expected);
    let members = |s: &str| {
        serde_json::from_str::<BTreeMap<String, &RawValue>>(s)
            .expect("object")
            .into_iter()
            .map(|(k, v)| (k, v.get().to_owned()))
            .collect::<BTreeMap<_, _>>()
    };
    let (mut a, mut b) = (members(body), members(&out));
    assert_ne!(a.remove("tools"), b.remove("tools"));
    assert_eq!(a, b);
    assert_eq!(a.len(), 6);
    assert!(out.contains("\"mod\\u0065l\""), "the escaped key keeps its spelling");
}

/// Review Focus 4.
#[test]
fn a_namespace_removal_keeps_the_flat_tool_and_drops_an_empty_namespace() {
    let body = fixture("responses_namespace_emptied", "in");
    let p = plan(&enforce(Format::OpenAiResponses, &["db.query", "fs.delete"]), Format::OpenAiResponses, body.as_bytes());
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiResponses, body.as_bytes(), &p)).expect("json");
    let tools = out["tools"].as_array().expect("tools");
    // The flat `query` stays: only `db.query` is denied.
    assert_eq!(tools[0], json!({"type": "function", "name": "query", "parameters": {"type": "object", "properties": {}}}));
    // `db` has no inner tool left and is gone; `fs` keeps `read`.
    assert_eq!(tools.len(), 2);
    assert_eq!((tools[1]["type"].as_str(), tools[1]["name"].as_str()), (Some("namespace"), Some("fs")));
    assert_eq!(tools[1]["tools"].as_array().map(|t| t.iter().map(|x| x["name"].clone()).collect::<Vec<_>>()), Some(vec![json!("read")]));
}

#[test]
fn only_enforced_items_are_applied() {
    // The floor forbids `a` (enforced in every mode, D33); the log-only tenant would forbid `b`.
    let s = snapshot(
        Format::OpenAiChat,
        r#"@id("f") forbid(principal, action == Pgs::Gateway::Action::"a", resource);"#,
        &forbid_text(&["b"]),
        TenantMode::LogOnly,
    );
    let body = json!({"model":"m",
        "tools":[{"type":"function","function":{"name":"a"}},{"type":"function","function":{"name":"b"}},{"type":"function","function":{"name":"ok"}}],
        "messages":[{"role":"assistant","content":null,"tool_calls":[
                        {"id":"c1","type":"function","function":{"name":"a","arguments":"{}"}},
                        {"id":"c2","type":"function","function":{"name":"b","arguments":"{}"}}]},
                    {"role":"tool","tool_call_id":"c1","content":"from a"},
                    {"role":"tool","tool_call_id":"c2","content":"from b"}]})
    .to_string();
    let p = plan(&s, Format::OpenAiChat, body.as_bytes());
    assert_eq!(p.removed.iter().map(|i| (i.target.as_str(), i.enforced)).collect::<Vec<_>>(), [("a", true), ("b", false)]);
    assert_eq!(p.replaced.iter().map(|i| i.enforced).collect::<Vec<_>>(), [true, false]);
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiChat, body.as_bytes(), &p)).expect("json");
    let names: Vec<&str> = out["tools"].as_array().expect("tools").iter().filter_map(|t| t["function"]["name"].as_str()).collect();
    assert_eq!(names, ["b", "ok"]);
    assert_eq!(out["messages"][1]["content"], json!(result_notice("a", RequestReason::ToolResultBlocked)));
    assert_eq!(out["messages"][2]["content"], json!("from b"));
}

#[test]
fn a_legacy_result_is_matched_by_its_raw_name() {
    // The plan keys a legacy `role:"function"` result by its raw name, as it dedups them (`x` and
    // `X` are two items). The rewrite matches the same way: exact, not normalised.
    let plan_body = json!({"model":"m","messages":[{"role":"function","name":"delete_file","content":"a"}]}).to_string();
    let p = plan(&enforce(Format::OpenAiChat, &["delete_file"]), Format::OpenAiChat, plan_body.as_bytes());
    let body = json!({"model":"m","messages":[
        {"role":"function","name":"delete_file","content":"a"},
        {"role":"function","name":"Delete_File","content":"b"}]})
    .to_string();
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiChat, body.as_bytes(), &p)).expect("json");
    assert_eq!(out["messages"][0]["content"], json!(result_notice("delete_file", RequestReason::ToolResultBlocked)));
    assert_eq!(out["messages"][1]["content"], json!("b"));
    // Planned from both names, both are replaced.
    let p = plan(&enforce(Format::OpenAiChat, &["delete_file"]), Format::OpenAiChat, body.as_bytes());
    assert_eq!(p.replaced.len(), 2);
    let out: Value = serde_json::from_str(&rewrite(Format::OpenAiChat, body.as_bytes(), &p)).expect("json");
    assert_eq!(out["messages"][1]["content"], json!(result_notice("Delete_File", RequestReason::ToolResultBlocked)));
}

#[test]
fn the_result_notice_is_fixed_text_with_a_clean_name() {
    assert_eq!(
        result_notice("delete_file", RequestReason::ToolResultBlocked),
        "[paigasus] tool result for \"delete_file\" was replaced (tool-result-blocked)."
    );
    assert_eq!(
        result_notice("a\"b\nc}", RequestReason::ToolResultBlocked),
        "[paigasus] tool result for \"a_b_c_\" was replaced (tool-result-blocked)."
    );
}

#[test]
fn an_anthropic_named_choice_resets_to_auto_and_keeps_its_other_members() {
    let body = r#"{"model":"c","max_tokens":10,"tools":[{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}],"tool_choice":{"type":"tool","name":"a","disable_parallel_tool_use":true},"messages":[]}"#;
    let p = plan(&enforce(Format::AnthropicMessages, &["a"]), Format::AnthropicMessages, body.as_bytes());
    assert_eq!(
        rewrite(Format::AnthropicMessages, body.as_bytes(), &p),
        r#"{"model":"c","max_tokens":10,"tools":[{"name":"b","input_schema":{}}],"tool_choice":{"type":"auto","disable_parallel_tool_use":true},"messages":[]}"#
    );
}

#[test]
fn a_responses_choice_is_reset_or_narrowed_and_keeps_its_mode() {
    let tools = r#""tools":[{"type":"function","name":"a"},{"type":"function","name":"b"},{"type":"function","name":"c"}]"#;
    let allowed = format!(r#"{{"model":"m",{tools},"tool_choice":{{"type":"allowed_tools","mode":"required","tools":[{{"type":"function","name":"a"}},{{"type":"function","name":"b"}}]}}}}"#);
    let p = plan(&enforce(Format::OpenAiResponses, &["a"]), Format::OpenAiResponses, allowed.as_bytes());
    assert_eq!(
        rewrite(Format::OpenAiResponses, allowed.as_bytes(), &p),
        r#"{"model":"m","tools":[{"type":"function","name":"b"},{"type":"function","name":"c"}],"tool_choice":{"type":"allowed_tools","mode":"required","tools":[{"type":"function","name":"b"}]}}"#
    );
    let named = format!(r#"{{"model":"m",{tools},"tool_choice":{{"type":"function","name":"a"}},"parallel_tool_calls":false}}"#);
    let p = plan(&enforce(Format::OpenAiResponses, &["a"]), Format::OpenAiResponses, named.as_bytes());
    assert_eq!(
        rewrite(Format::OpenAiResponses, named.as_bytes(), &p),
        r#"{"model":"m","tools":[{"type":"function","name":"b"},{"type":"function","name":"c"}],"tool_choice":"auto","parallel_tool_calls":false}"#
    );
}

#[test]
fn a_hosted_tool_with_a_removed_name_stays() {
    // A function named `web_search` is removed; the hosted `web_search` tool is never removed.
    let body = r#"{"model":"m","tools":[{"type":"function","name":"web_search"},{"type":"web_search"},{"type":"local_shell"}]}"#;
    let p = plan(&enforce(Format::OpenAiResponses, &["web_search", "local_shell"]), Format::OpenAiResponses, body.as_bytes());
    assert_eq!(rewrite(Format::OpenAiResponses, body.as_bytes(), &p), r#"{"model":"m","tools":[{"type":"web_search"}]}"#);
}
