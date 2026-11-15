// SPDX-License-Identifier: Apache-2.0
//! SMA-752 § 6.5, D17, D19, D30: the tool policy through the real router, the Cedar decider and a
//! file policy source, against the mock upstream.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::{AllowAll, Limits as CoreLimits, OnUnavailable, TenantMode, ToolCallDecider};
use paigasus_observability::names;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::*;
use tempfile::TempDir;
use tower::ServiceExt;

const MIB: usize = 1 << 20;
/// The floor of these tests: it forbids `floor_rm` in every mode (D33).
const FLOOR: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"floor_rm\", resource);\n";
/// The tenant set: permit every IAM principal, forbid `blocked_rm`.
const TENANT: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"blocked_rm", resource);"#;

/// A policy directory: `floor.cedar` and, for `ORG_A`, `policies.cedar`.
fn policy_dir(floor: &str, tenant: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("floor.cedar"), floor).expect("floor");
    let t = dir.path().join("tenants").join(ORG_A);
    std::fs::create_dir_all(&t).expect("tenant dir");
    std::fs::write(t.join("policies.cedar"), tenant).expect("tenant");
    dir
}

fn tp_settings(mode: TenantMode) -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode: mode,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(1),
        orgs: HashMap::new(),
    }
}

/// The request side over the file source in `dir`, as `main.rs` builds it.
fn tool_policy(dir: &TempDir, mode: TenantMode) -> Option<Arc<RequestToolPolicy>> {
    let settings = tp_settings(mode);
    let cache = PolicyCache::new(Arc::new(FilePolicySource::new(dir.path())), CacheSettings::from_settings(&settings));
    Some(Arc::new(RequestToolPolicy::new(cache, settings)))
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception {
        limits: CoreLimits::defaults(),
        decider,
    }))
}

fn cedar() -> Option<Arc<ToolInterception>> {
    ti(Arc::new(CedarDecider))
}

/// A router with every option this file needs: limits, the tool policy and the Responses route.
fn full_app(base_url: &str, limits: Option<Arc<Limits>>, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: true },
        limits,
        dpop_enabled: false,
        tool_interception: ti,
        tool_policy: tp,
        responses: Some(Arc::new(ResponsesSettings {
            max_request_bytes: 16 * MIB,
            max_record_bytes: 16 * MIB,
            max_held_bytes: 32 * MIB,
        })),
    })
}

async fn send_to(app: &Router, path: &str, body: &str) -> Sent {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(body.to_owned()))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

/// A chat request with these function tools.
fn chat_body(tools: &[&str], stream: bool) -> String {
    let tools: Vec<Value> = tools.iter().map(|n| json!({"type":"function","function":{"name":n,"parameters":{}}})).collect();
    json!({"model":"gpt-4o-mini","stream":stream,"messages":[{"role":"user","content":"hi"}],"tools":tools}).to_string()
}

/// A Responses request with these function tools.
fn responses_body(tools: &[&str], stream: bool) -> String {
    let tools: Vec<Value> = tools.iter().map(|n| json!({"type":"function","name":n,"parameters":{}})).collect();
    json!({"model":"gpt-6","input":"hi","stream":stream,"tools":tools}).to_string()
}

fn tool_chunk(index: u32, id: &str, name: &str, args: &str) -> String {
    let e = json!({"index": index, "id": id, "type": "function", "function": {"name": name, "arguments": args}});
    format!(
        "data: {}\n\n",
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[e]},"finish_reason":null}]})
    )
}

fn finish_chunk() -> String {
    format!(
        "data: {}\n\n",
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})
    )
}

/// A chat stream that calls `read_file` and `blocked_rm` (with the canary).
fn chat_stream_with_calls() -> Vec<String> {
    vec![
        chunk("Sure.", true),
        tool_chunk(0, "call_0", "read_file", "{\"p\":1}"),
        tool_chunk(1, "call_1", "blocked_rm", "{\"p\":\"CANARY\"}"),
        finish_chunk(),
        usage_chunk(11),
        "data: [DONE]\n\n".to_owned(),
    ]
}

/// A chat completion that calls `read_file` and `blocked_rm` (with the canary).
fn chat_body_with_calls() -> String {
    json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[
        {"id":"call_0","type":"function","function":{"name":"read_file","arguments":"{}"}},
        {"id":"call_1","type":"function","function":{"name":"blocked_rm","arguments":"{\"p\":\"CANARY\"}"}}
    ]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})
    .to_string()
}

/// A Responses answer that calls `read_file` and `blocked_rm` (with the canary).
fn responses_body_with_calls() -> String {
    json!({"id":"resp_1","object":"response","status":"completed","output":[
        {"type":"function_call","id":"fc_1","call_id":"c1","name":"read_file","arguments":"{}","status":"completed"},
        {"type":"function_call","id":"fc_2","call_id":"c2","name":"blocked_rm","arguments":"{\"p\":\"CANARY\"}","status":"completed"}
    ],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":13}})
    .to_string()
}

/// A Responses stream that calls `read_file` and `blocked_rm` (with the canary).
fn responses_stream_with_calls() -> Vec<String> {
    let mut seq = 0u64;
    let mut out = Vec::new();
    let mut push = |mut v: Value| {
        v["sequence_number"] = seq.into();
        seq += 1;
        let ty = v["type"].as_str().expect("a type").to_owned();
        out.push(format!("event: {ty}\ndata: {v}\n\n"));
    };
    let response = json!({"id":"resp_1","object":"response","status":"in_progress","output":[]});
    push(json!({"type":"response.created","response":response}));
    push(json!({"type":"response.in_progress","response":response}));
    let mut done = Vec::new();
    for (index, name, args) in [(0u32, "read_file", "{\"p\":1}"), (1, "blocked_rm", "{\"p\":\"CANARY\"}")] {
        let (id, call_id) = (format!("fc_{index}"), format!("call_{index}"));
        let added = json!({"type":"function_call","id":id,"status":"in_progress","call_id":call_id,"name":name,"arguments":""});
        push(json!({"type":"response.output_item.added","item":added,"output_index":index}));
        push(json!({"type":"response.function_call_arguments.delta","delta":args,"item_id":id,"output_index":index}));
        push(json!({"type":"response.function_call_arguments.done","arguments":args,"item_id":id,"output_index":index}));
        let item = json!({"type":"function_call","id":id,"status":"completed","call_id":call_id,"name":name,"arguments":args});
        push(json!({"type":"response.output_item.done","item":item.clone(),"output_index":index}));
        done.push(item);
    }
    let response = json!({"id":"resp_1","object":"response","status":"completed","output":done,"usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8}});
    push(json!({"type":"response.completed","response":response}));
    out
}

/// Every `: paigasus-decision` comment of a stream, parsed.
fn comments(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| l.strip_prefix(": paigasus-decision "))
        .map(|j| serde_json::from_str(j).expect("a JSON comment"))
        .collect()
}

/// The `paigasus-tool-decisions` header, parsed.
fn header_entries(sent: &Sent) -> Vec<Value> {
    serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("a JSON header")
}

/// The call entry for `name`.
fn call_entry<'a>(entries: &'a [Value], name: &str) -> &'a Value {
    entries.iter().find(|e| e["name"] == name).unwrap_or_else(|| panic!("no entry for {name}: {entries:?}"))
}

/// D30: the decider used the request's snapshot. Without it every call would be blocked
/// `policy-unavailable` (fail closed): `read_file` would be blocked too.
fn assert_decided_with_the_snapshot(entries: &[Value]) {
    let read = call_entry(entries, "read_file");
    assert_eq!((read["decision"].as_str(), read["code"].as_str()), (Some("allow"), None), "{entries:?}");
    let blocked = call_entry(entries, "blocked_rm");
    assert_eq!((blocked["decision"].as_str(), blocked["code"].as_str()), (Some("block"), Some("policy")), "{entries:?}");
}

fn overhead_samples(snapshotter: &Snapshotter) -> usize {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find(|(k, _, _, _)| k.key().name() == names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS)
        .map_or(0, |(_, _, _, v)| match v {
            DebugValue::Histogram(h) => h.len(),
            _ => 0,
        })
}

/// § 6.5.1, § 6.5.3: in enforce a denied call is blocked, and the request entries go out before
/// the first upstream frame.
#[tokio::test]
async fn a_denied_call_is_blocked_in_enforce() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_raw_sse(chat_stream_with_calls()).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let sent = send(&app, &chat_body(&["read_file", "blocked_rm"], true)).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#), "{text}");
    // The request entry is the first frame, before any upstream frame.
    let first = text.split("\n\n").next().expect("a first frame");
    let entry: Value = serde_json::from_str(first.strip_prefix(": paigasus-decision ").expect("a decision comment first")).expect("json");
    assert_eq!(
        (entry["side"].as_str(), entry["tool"].as_str(), entry["action"].as_str(), entry["enforced"].as_bool()),
        (Some("request"), Some("blocked_rm"), Some("remove-definition"), Some(true)),
        "{entry}"
    );
    assert_decided_with_the_snapshot(&comments(&text));
}

/// D17, § 6.5.0: an unreadable turn in enforce is a 400 before the limits and the upstream.
#[tokio::test]
async fn an_unavailable_turn_is_refused_before_the_limits() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = RecordingStore::new();
    let app = full_app(
        &mock.base_url,
        Some(limits(rules(Some(100), None, None), store.clone())),
        cedar(),
        tool_policy(&dir, TenantMode::Enforce),
    );
    // A duplicate key the chat DTO ignores, but the strict request parse refuses (C13).
    let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"x":1,"x":2}"#;
    let sent = send(&app, body).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{}", sent.text());
    assert_eq!(sent.json()["error"]["code"], "invalid-request-body");
    assert_eq!(mock.request_count(), 0, "nothing goes upstream");
    assert_eq!(store.checks(), 0, "the limits never ran");
    // The same body in log-only goes through the limits and upstream unchanged (P-10).
    let app = full_app(
        &mock.base_url,
        Some(limits(rules(Some(100), None, None), store.clone())),
        cedar(),
        tool_policy(&dir, TenantMode::LogOnly),
    );
    let sent = send(&app, body).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(store.checks(), 1);
    assert_eq!(mock.recorded().expect("recorded").body, body.as_bytes());
}

/// § 6.5.1, D19: the enforced rewrite is what goes upstream, never the original body.
#[tokio::test]
async fn the_rewritten_body_goes_upstream() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let body = chat_body(&["read_file", "blocked_rm", "floor_rm"], false);
    let sent = send(&app, &body).await;
    assert_eq!(sent.status, StatusCode::OK);
    let up = mock.recorded().expect("recorded").body;
    assert_ne!(up, body.as_bytes(), "the original body was sent");
    let up: Value = serde_json::from_slice(&up).expect("json");
    let names: Vec<&str> = up["tools"].as_array().expect("tools").iter().map(|t| t["function"]["name"].as_str().expect("name")).collect();
    assert_eq!(names, ["read_file"]);
    // The body had no calls, so the header holds the request entries alone (§ 6.5.3).
    let h = header_entries(&sent);
    let tools: Vec<(&str, bool)> = h.iter().map(|e| (e["tool"].as_str().expect("tool"), e["enforced"].as_bool().expect("enforced"))).collect();
    assert_eq!(tools, [("blocked_rm", true), ("floor_rm", true)], "{h:?}");
    // Log-only sends the original bytes and still removes the floor tool (D33).
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::LogOnly));
    send(&app, &chat_body(&["read_file", "blocked_rm"], false)).await;
    assert_eq!(mock.recorded().expect("recorded").body, chat_body(&["read_file", "blocked_rm"], false).as_bytes());
}

/// § 6.5.3: a non-2xx answer carries the request entries in the header, stream or not.
#[tokio::test]
async fn a_non_2xx_answer_carries_the_request_entries_header() {
    let dir = policy_dir(FLOOR, TENANT);
    let err = r#"{"error":{"message":"bad","type":"invalid_request_error"}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::BAD_REQUEST, err).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    for stream in [false, true] {
        let sent = send(&app, &chat_body(&["read_file", "blocked_rm"], stream)).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "stream {stream}");
        assert_eq!(sent.text(), err, "the error body passes through (stream {stream})");
        let h = header_entries(&sent);
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!((h[0]["side"].as_str(), h[0]["tool"].as_str()), (Some("request"), Some("blocked_rm")), "{h:?}");
    }
}

/// With every mode off and an empty floor, the tool policy changes no byte: the same upstream
/// request and the same client bytes as `tool_policy: None`.
#[tokio::test]
async fn off_everywhere_is_byte_identical() {
    let dir = policy_dir("", "");
    let body = chat_body(&["read_file", "blocked_rm"], true);
    let mut seen = Vec::new();
    for (decider, tp) in [
        (Arc::new(AllowAll) as Arc<dyn ToolCallDecider>, None),
        (Arc::new(CedarDecider) as Arc<dyn ToolCallDecider>, tool_policy(&dir, TenantMode::Off)),
    ] {
        let mock = MockOpenAi::spawn_raw_sse(chat_stream_with_calls()).await;
        let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, ti(decider), tp);
        let sent = send(&app, &body).await;
        assert_eq!(sent.status, StatusCode::OK);
        seen.push((mock.recorded().expect("recorded").body, sent.body, sent.headers.get("paigasus-tool-decisions").cloned()));
    }
    assert_eq!(seen[0].0, body.as_bytes(), "the original request goes upstream");
    assert_eq!(seen[0], seen[1]);
}

/// D30 for chat, non-stream: the decider decides with the request's snapshot.
#[tokio::test]
async fn chat_non_stream_decides_with_the_snapshot() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, chat_body_with_calls()).await;
    let app = full_app(&mock.base_url, None, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let sent = send_to(&app, "/v1/chat/completions", &chat_body(&["read_file"], false)).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    assert_decided_with_the_snapshot(&header_entries(&sent));
}

/// D30 for chat, stream.
#[tokio::test]
async fn chat_stream_decides_with_the_snapshot() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_raw_sse(chat_stream_with_calls()).await;
    let app = full_app(&mock.base_url, None, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let sent = send_to(&app, "/v1/chat/completions", &chat_body(&["read_file"], true)).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    assert_decided_with_the_snapshot(&comments(&sent.text()));
}

/// D30 for Responses, non-stream.
#[tokio::test]
async fn responses_non_stream_decides_with_the_snapshot() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, responses_body_with_calls()).await;
    let app = full_app(&mock.base_url, None, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let sent = send_to(&app, "/v1/responses", &responses_body(&["read_file"], false)).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert!(!sent.text().contains("CANARY"));
    assert_decided_with_the_snapshot(&header_entries(&sent));
}

/// D30 for Responses, stream.
#[tokio::test]
async fn responses_stream_decides_with_the_snapshot() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_raw_sse(responses_stream_with_calls()).await;
    let app = full_app(&mock.base_url, None, cedar(), tool_policy(&dir, TenantMode::Enforce));
    let sent = send_to(&app, "/v1/responses", &responses_body(&["read_file"], true)).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert_decided_with_the_snapshot(&comments(&text));
}

/// Preflight F14: a request with no call records one overhead sample, stream or not, and a
/// request with calls records one too.
#[tokio::test]
async fn each_request_records_one_overhead_sample() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let dir = policy_dir(FLOOR, TENANT);

    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(overhead_samples(&snapshotter), 1, "non-stream, no call");

    let mock = MockOpenAi::spawn_raw_sse(vec![chunk("hello", true), usage_chunk(5), "data: [DONE]\n\n".to_owned()]).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    assert_eq!(send(&app, STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(overhead_samples(&snapshotter), 1, "stream, no call");

    let mock = MockOpenAi::spawn_raw_sse(chat_stream_with_calls()).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    assert_eq!(send(&app, &chat_body(&["read_file"], true)).await.status, StatusCode::OK);
    assert_eq!(overhead_samples(&snapshotter), 1, "stream with calls");
}
