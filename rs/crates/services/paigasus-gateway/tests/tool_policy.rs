// SPDX-License-Identifier: Apache-2.0
//! SMA-752 § 6.5, § 7.2, D17, D19, D30, D33, D34: the tool policy through the real router, the
//! Cedar decider and a file policy source, against the mock upstream.
//!
//! A test that names no endpoint runs for each endpoint of [`EPS`]: Chat non-stream, Chat stream,
//! Responses non-stream and Responses stream. The Anthropic route (`/v1/messages`, SMA-750) has
//! its own test at the end of this file.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::history::HmacHistoryDigester;
use paigasus_gateway::adapters::history::memory::MemorySessionStore;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::application::tool_history::ToolHistory;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, Lookup, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
use paigasus_gateway::domain::decision_log::OsEntropy;
use paigasus_gateway::domain::tool_history::{CallEvent, EventKind, EventVerdict, Histories, ReserveRequest, Scope, SessionStore, SettleItem, SettleRequest};
use paigasus_gateway::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies};
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::{AllowAll, Limits as CoreLimits, OnUnavailable, TenantMode, ToolCallDecider};
use paigasus_observability::names;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::*;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const MIB: usize = 1 << 20;
/// The floor of these tests: it forbids `floor_rm` in every mode (D33).
const FLOOR: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"floor_rm\", resource);\n";
/// The shipped floor (D15, P-14).
const SHIPPED_FLOOR: &str = include_str!("../tool-policies/floor.cedar");
/// A floor that forbids the `computer` action in every mode (SMA-776).
const FLOOR_COMPUTER: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-computer\") forbid(principal, action == Pgs::Gateway::Action::\"computer\", resource);\n";
/// A tenant set: permit every IAM principal, forbid `computer`.
const TENANT_COMPUTER: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"computer", resource);"#;
/// The tenant set: permit every IAM principal, forbid `blocked_rm`.
const TENANT: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"blocked_rm", resource);"#;
/// A team and a project in `ORG_A`, and a team in another org.
const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";
const TEAM_IN_B: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000b2:team/0190a1b2-0000-7000-8000-0000000000c3";
/// The calls most tests answer with: an allowed one and a tenant-denied one (with the canary).
const CALLS: &[(&str, &str)] = &[("read_file", "{\"p\":1}"), ("blocked_rm", "{\"p\":\"CANARY\"}")];
const NOTICE_PREFIX: &str = "[paigasus] tool result for";

// --- policy fixtures -------------------------------------------------------------------------

/// A policy directory: `floor.cedar` and, for `ORG_A`, `policies.cedar`.
fn policy_dir(floor: &str, tenant: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("floor.cedar"), floor).expect("floor");
    write_tenant(&dir, ORG_A, tenant, None);
    dir
}

/// Write the policies (and, when given, the catalog) of `org`.
fn write_tenant(dir: &TempDir, org: &str, text: &str, catalog: Option<&str>) {
    let t = dir.path().join("tenants").join(org);
    std::fs::create_dir_all(&t).expect("tenant dir");
    std::fs::write(t.join("policies.cedar"), text).expect("tenant");
    if let Some(c) = catalog {
        std::fs::write(t.join("catalog.json"), c).expect("catalog");
    }
}

fn org_a() -> Uuid {
    Uuid::try_parse(ORG_A).expect("a fixture uuid")
}

fn tp_settings(mode: TenantMode) -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode: mode,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        // Far above any real load: the time limit also covers the floor load (D31), and a loaded
        // host must never miss it. Only the hanging-source test sets a short value.
        load_timeout: Duration::from_secs(60),
        orgs: HashMap::new(),
        settings_from_source: false,
    }
}

/// `tp_settings(mode)` with the global `on_policy_unavailable` set to `on`.
fn tp_settings_on(mode: TenantMode, on: OnUnavailable) -> ToolPolicySettings {
    ToolPolicySettings {
        on_policy_unavailable: on,
        ..tp_settings(mode)
    }
}

/// The request side over `source`, as `runtime::build_tool_policy` builds it.
fn tp_with(source: Arc<dyn PolicySource>, settings: ToolPolicySettings) -> Option<Arc<RequestToolPolicy>> {
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings), Arc::new(HmacHistoryDigester::keyless()));
    Some(Arc::new(RequestToolPolicy::new(
        cache,
        settings,
        Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()),
    )))
}

/// `tp_with`, with the floor loaded into the cache before the first request. A request then
/// spends its `load_timeout` on the tenant load only.
async fn tp_preloaded(source: Arc<dyn PolicySource>, settings: ToolPolicySettings) -> Option<Arc<RequestToolPolicy>> {
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings), Arc::new(HmacHistoryDigester::keyless()));
    assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the floor loads");
    Some(Arc::new(RequestToolPolicy::new(
        cache,
        settings,
        Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()),
    )))
}

/// The request side over the file source in `dir`.
fn tool_policy(dir: &TempDir, mode: TenantMode) -> Option<Arc<RequestToolPolicy>> {
    tp_with(Arc::new(FilePolicySource::new(dir.path())), tp_settings(mode))
}

/// A fault of a policy source call.
#[derive(Debug, Clone, Copy)]
enum Fault {
    /// The call fails.
    Fail,
    /// The call never answers (a source slower than any `load_timeout_ms`).
    Hang,
}

/// The file source with a fault on the floor call, the tenant call, or both.
struct FaultySource {
    inner: FilePolicySource,
    floor: Option<Fault>,
    tenant: Option<Fault>,
}

impl FaultySource {
    fn arc(dir: &TempDir, floor: Option<Fault>, tenant: Option<Fault>) -> Arc<dyn PolicySource> {
        Arc::new(FaultySource {
            inner: FilePolicySource::new(dir.path()),
            floor,
            tenant,
        })
    }
}

async fn fault(f: Option<Fault>) -> Result<(), SourceError> {
    match f {
        None => Ok(()),
        Some(Fault::Fail) => Err(SourceError::Io("test failure".to_owned())),
        Some(Fault::Hang) => std::future::pending().await,
    }
}

#[async_trait::async_trait]
impl PolicySource for FaultySource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        fault(self.floor).await?;
        self.inner.floor().await
    }

    async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError> {
        fault(self.tenant).await?;
        self.inner.tenant(org).await
    }
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception {
        limits: CoreLimits::defaults(),
        decider,
        recorder: None,
        history: None,
    }))
}

fn cedar() -> Option<Arc<ToolInterception>> {
    ti(Arc::new(CedarDecider::new(Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()))))
}

// --- the router and the endpoints ------------------------------------------------------------

/// A router with every option this file needs, for a caller with `scope`.
fn app_for(scope: &str, base_url: &str, limits: Option<Arc<Limits>>, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, scope),
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
        anthropic: None,
        messages: None,
    })
}

/// A router for an org-scoped caller: limits, the tool policy and the Responses route.
fn full_app(base_url: &str, limits: Option<Arc<Limits>>, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>) -> Router {
    app_for(ORG_A_PRN, base_url, limits, ti, tp)
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

/// An intercepted endpoint, stream or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ep {
    ChatJson,
    ChatSse,
    RespJson,
    RespSse,
}

const EPS: [Ep; 4] = [Ep::ChatJson, Ep::ChatSse, Ep::RespJson, Ep::RespSse];

impl Ep {
    fn chat(self) -> bool {
        matches!(self, Ep::ChatJson | Ep::ChatSse)
    }

    fn stream(self) -> bool {
        matches!(self, Ep::ChatSse | Ep::RespSse)
    }

    fn path(self) -> &'static str {
        if self.chat() { "/v1/chat/completions" } else { "/v1/responses" }
    }

    /// A request with these function tools.
    fn tools_body(self, tools: &[&str]) -> String {
        if self.chat() { chat_body(tools, self.stream()) } else { responses_body(tools, self.stream()) }
    }

    /// `v` with `stream` set for this endpoint.
    fn body(self, mut v: Value) -> String {
        v["stream"] = self.stream().into();
        v.to_string()
    }

    /// A mock upstream that answers with these function calls (none is a plain answer).
    async fn mock(self, calls: &[(&str, &str)]) -> MockOpenAi {
        match self {
            Ep::ChatJson => MockOpenAi::spawn_json(StatusCode::OK, chat_json(calls)).await,
            Ep::ChatSse => MockOpenAi::spawn_raw_sse(chat_sse(calls)).await,
            Ep::RespJson => MockOpenAi::spawn_json(StatusCode::OK, responses_json(calls)).await,
            Ep::RespSse => MockOpenAi::spawn_raw_sse(responses_sse(calls)).await,
        }
    }
}

/// One request through the gateway: the answer, the report entries and the upstream request body.
struct Ex {
    status: StatusCode,
    /// The `: paigasus-decision` comments of a stream, or the `paigasus-tool-decisions` header.
    entries: Vec<Value>,
    text: String,
    upstream: Option<Vec<u8>>,
    sent: Sent,
}

impl Ex {
    fn upstream_json(&self) -> Value {
        serde_json::from_slice(self.upstream.as_deref().expect("a request went upstream")).expect("json")
    }
}

async fn exchange(ep: Ep, scope: &str, tp: Option<Arc<RequestToolPolicy>>, body: &str, calls: &[(&str, &str)]) -> Ex {
    exchange_with(ep, scope, cedar(), tp, body, calls).await
}

/// `exchange` with the tool interception `ti`.
async fn exchange_with(ep: Ep, scope: &str, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>, body: &str, calls: &[(&str, &str)]) -> Ex {
    let mock = ep.mock(calls).await;
    exchange_on(ep, mock, scope, ti, tp, body).await
}

/// One request through the gateway against `mock`.
async fn exchange_on(ep: Ep, mock: MockOpenAi, scope: &str, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>, body: &str) -> Ex {
    let app = app_for(scope, &mock.base_url, None, ti, tp);
    let sent = send_to(&app, ep.path(), body).await;
    let text = sent.text();
    let entries = if ep.stream() && sent.status.is_success() {
        comments(&text)
    } else {
        sent.headers
            .get("paigasus-tool-decisions")
            .map(|v| serde_json::from_str(v.to_str().expect("ascii")).expect("a JSON header"))
            .unwrap_or_default()
    };
    Ex {
        status: sent.status,
        entries,
        text,
        upstream: mock.recorded().map(|r| r.body.to_vec()),
        sent,
    }
}

// --- request bodies and upstream answers -----------------------------------------------------

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

fn chat_tools(tools: &[&str]) -> Value {
    tools.iter().map(|n| json!({"type":"function","function":{"name":n,"parameters":{}}})).collect()
}

fn responses_tools(tools: &[&str]) -> Value {
    tools.iter().map(|n| json!({"type":"function","name":n,"parameters":{}})).collect()
}

fn tool_chunk(index: usize, id: &str, name: &str, args: &str) -> String {
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

/// A chat stream that calls `calls` (`call_<i>`).
fn chat_sse(calls: &[(&str, &str)]) -> Vec<String> {
    let mut out = vec![chunk("Sure.", true)];
    for (i, (name, args)) in calls.iter().enumerate() {
        out.push(tool_chunk(i, &format!("call_{i}"), name, args));
    }
    if !calls.is_empty() {
        out.push(finish_chunk());
    }
    out.push(usage_chunk(11));
    out.push("data: [DONE]\n\n".to_owned());
    out
}

/// A chat completion that calls `calls`.
fn chat_json(calls: &[(&str, &str)]) -> String {
    if calls.is_empty() {
        return USAGE_BODY.to_owned();
    }
    let tool_calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (n, a))| json!({"id":format!("call_{i}"),"type":"function","function":{"name":n,"arguments":a}}))
        .collect();
    json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":tool_calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})
    .to_string()
}

/// A Responses answer that calls `calls`.
fn responses_json(calls: &[(&str, &str)]) -> String {
    let output: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (n, a))| json!({"type":"function_call","id":format!("fc_{i}"),"call_id":format!("c{i}"),"name":n,"arguments":a,"status":"completed"}))
        .collect();
    json!({"id":"resp_1","object":"response","status":"completed","output":output,"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":13}}).to_string()
}

/// A Responses stream that calls `calls`.
fn responses_sse(calls: &[(&str, &str)]) -> Vec<String> {
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
    for (index, (name, args)) in calls.iter().enumerate() {
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

// --- report readers --------------------------------------------------------------------------

/// Every `: paigasus-decision` comment of a stream, parsed.
fn comments(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| l.strip_prefix(": paigasus-decision "))
        .map(|j| serde_json::from_str(j).expect("a JSON comment"))
        .collect()
}

/// The stream text without its `: paigasus-decision` frames.
fn without_comments(text: &str) -> String {
    text.split_inclusive("\n\n").filter(|f| !f.starts_with(": paigasus-decision ")).collect()
}

/// The `paigasus-tool-decisions` header, parsed.
fn header_entries(sent: &Sent) -> Vec<Value> {
    serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("a JSON header")
}

/// The call entry for `name`.
fn call_entry<'a>(entries: &'a [Value], name: &str) -> &'a Value {
    entries
        .iter()
        .find(|e| e["side"] == "response" && e["name"] == name)
        .unwrap_or_else(|| panic!("no call entry for {name}: {entries:?}"))
}

/// `(decision, code, enforced, reason)` of the call entry for `name`.
fn call_of<'a>(entries: &'a [Value], name: &str) -> (Option<&'a str>, Option<&'a str>, Option<bool>, Option<&'a str>) {
    let e = call_entry(entries, name);
    (e["decision"].as_str(), e["code"].as_str(), e["enforced"].as_bool(), e["reason"].as_str())
}

/// `(tool, action, enforced)` of one request entry.
type RequestEntryRow = (String, String, bool);

/// `(tool, action, enforced)` of every request entry.
fn request_entries(entries: &[Value]) -> Vec<RequestEntryRow> {
    entries
        .iter()
        .filter(|e| e["side"] == "request")
        .map(|e| {
            (
                e["tool"].as_str().expect("tool").to_owned(),
                e["action"].as_str().expect("action").to_owned(),
                e["enforced"].as_bool().expect("enforced"),
            )
        })
        .collect()
}

fn req(tool: &str, action: &str, enforced: bool) -> (String, String, bool) {
    (tool.to_owned(), action.to_owned(), enforced)
}

const ALLOWED: (Option<&str>, Option<&str>, Option<bool>, Option<&str>) = (Some("allow"), None, None, None);
const BLOCKED: (Option<&str>, Option<&str>, Option<bool>, Option<&str>) = (Some("block"), Some("policy"), Some(true), Some("tool-call-denied"));
const OBSERVED: (Option<&str>, Option<&str>, Option<bool>, Option<&str>) = (Some("allow"), Some("policy"), Some(false), Some("tool-call-denied"));
const UNAVAILABLE_BLOCKED: (Option<&str>, Option<&str>, Option<bool>, Option<&str>) = (Some("block"), Some("policy-unavailable"), Some(true), Some("tool-policy-unavailable"));
const UNAVAILABLE_OBSERVED: (Option<&str>, Option<&str>, Option<bool>, Option<&str>) = (Some("allow"), Some("policy-unavailable"), Some(false), Some("tool-policy-unavailable"));

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

/// The increase of the counter `name` with exactly `labels` since the last snapshot, or 0. A
/// `DebuggingRecorder` snapshot drains the counters, so each call reads only the new increments.
fn counter_value(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> u64 {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find(|(k, _, _, _)| {
            let key = k.key();
            let have: Vec<(&str, &str)> = key.labels().map(|l| (l.key(), l.value())).collect();
            key.name() == name && have.len() == labels.len() && labels.iter().all(|l| have.contains(l))
        })
        .map_or(0, |(_, _, _, v)| match v {
            DebugValue::Counter(c) => c,
            _ => 0,
        })
}

/// The function tool names of an upstream request (`None` when it has no `tools`).
fn upstream_tools(ep: Ep, up: &Value) -> Option<Vec<String>> {
    let tools = up.get("tools")?.as_array().expect("tools");
    Some(
        tools
            .iter()
            .map(|t| if ep.chat() { &t["function"]["name"] } else { &t["name"] })
            .map(|n| n.as_str().expect("name").to_owned())
            .collect(),
    )
}

// --- Task 15: the wiring ---------------------------------------------------------------------

/// § 6.5.1, § 6.5.3: in enforce a denied call is blocked, and the request entries go out before
/// the first upstream frame.
#[tokio::test]
async fn a_denied_call_is_blocked_in_enforce() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_raw_sse(chat_sse(CALLS)).await;
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

/// D17, P-10, § 6.5.0: an unreadable turn in enforce is a 400 before the limits and the upstream.
/// In log-only the same body is forwarded unchanged and counted as "enforce would refuse". The
/// turn is unreadable, so no plan exists: the floor tool stays in log-only too (the Task 8 note
/// for Sven: the floor is checked at call time only).
#[tokio::test]
async fn an_unavailable_turn_is_refused_in_enforce_and_counted_in_log_only() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let dir = policy_dir(FLOOR, TENANT);
    let refusals = |mode: &str| {
        counter_value(
            &snapshotter,
            names::GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL,
            &[("mode", mode), ("reason", "unreadable_turn"), ("refused", if mode == "enforce" { "true" } else { "false" })],
        )
    };
    for ep in EPS {
        let body = if ep.chat() {
            // A duplicate key the chat DTO ignores, but the strict request parse refuses (C13).
            let tools = ep.tools_body(&["floor_rm"]);
            format!("{},\"x\":1,\"x\":2}}", tools.strip_suffix('}').expect("an object"))
        } else {
            // The Responses route refuses a duplicate key itself (SMA-769), so this turn is
            // unreadable another way: a namespace may hold only function and custom tools.
            let mut tools = responses_tools(&["floor_rm"]);
            tools.as_array_mut().expect("tools").push(json!({"type":"namespace","name":"ns","tools":[{"type":"web_search"}]}));
            ep.body(json!({"model":"gpt-6","input":"hi","tools":tools}))
        };
        let mock = ep.mock(&[]).await;
        let store = RecordingStore::new();
        let app = app_for(
            ORG_A_PRN,
            &mock.base_url,
            Some(limits(rules(Some(100), None, None), store.clone())),
            cedar(),
            tool_policy(&dir, TenantMode::Enforce),
        );
        let sent = send_to(&app, ep.path(), &body).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{ep:?}: {}", sent.text());
        assert_eq!(sent.json()["error"]["code"], "invalid-request-body", "{ep:?}");
        assert!(sent.headers.get("paigasus-tool-decisions").is_none(), "no plan, no header ({ep:?})");
        assert_eq!(mock.request_count(), 0, "nothing goes upstream ({ep:?})");
        assert_eq!(store.checks(), 0, "the limits never ran ({ep:?})");
        assert_eq!(refusals("enforce"), 1, "{ep:?}");
        // The same body in log-only goes through the limits and upstream unchanged (P-10), and is counted.
        let app = app_for(
            ORG_A_PRN,
            &mock.base_url,
            Some(limits(rules(Some(100), None, None), store.clone())),
            cedar(),
            tool_policy(&dir, TenantMode::LogOnly),
        );
        let sent = send_to(&app, ep.path(), &body).await;
        assert_eq!(sent.status, StatusCode::OK, "{ep:?}: {}", sent.text());
        assert_eq!(store.checks(), 1, "{ep:?}");
        assert_eq!(
            mock.recorded().expect("recorded").body,
            body.as_bytes(),
            "the floor tool stays: no plan for an unreadable turn ({ep:?})"
        );
        assert_eq!(refusals("log_only"), 1, "log-only counts what enforce would refuse ({ep:?})");
    }
}

/// A Responses request with a computer call and its screenshot output.
fn computer_body(ep: Ep) -> String {
    ep.body(json!({"model":"gpt-6","tools":responses_tools(&["read_file"]),"input":[
        {"role":"user","content":"hi"},
        {"type":"computer_call","id":"cc_1","call_id":"cc1","actions":[{"type":"screenshot"}],"status":"completed"},
        {"type":"computer_call_output","call_id":"cc1","output":{"type":"computer_screenshot","image_url":"SECRET-SCREEN"}}]}))
}

/// SMA-776 G2, § 6.5: an enforced computer block is a 400 before the upstream, with the header.
/// The floor enforces it in log-only too (D33). A log-only tenant block is forwarded unchanged.
#[tokio::test]
async fn a_blocked_computer_result_is_refused_before_the_upstream() {
    const MESSAGE: &str = "A blocked computer_call_output cannot be replaced. Remove the computer call and its output from the input.";
    for ep in [Ep::RespJson, Ep::RespSse] {
        for (floor, tenant, mode) in [(FLOOR, TENANT_COMPUTER, TenantMode::Enforce), (FLOOR_COMPUTER, TENANT, TenantMode::LogOnly)] {
            let dir = policy_dir(floor, tenant);
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, mode), &computer_body(ep), &[]).await;
            let case = format!("{ep:?} {mode:?}");
            assert_eq!(ex.status, StatusCode::BAD_REQUEST, "{case}: {}", ex.text);
            // A refused stream request is a JSON answer, not SSE.
            assert!(ex.sent.header("content-type").starts_with("application/json"), "{case}");
            let err = &ex.sent.json()["error"];
            assert_eq!(
                (err["code"].as_str(), err["param"].as_str(), err["type"].as_str(), err["message"].as_str()),
                (Some("tool-result-unreplaceable"), Some("input"), Some("invalid_request_error"), Some(MESSAGE)),
                "{case}"
            );
            assert_eq!(request_entries(&ex.entries), [req("computer", "replace-result", true)], "{case}");
            assert!(ex.upstream.is_none(), "{case}: nothing goes upstream");
            assert!(!ex.text.contains("SECRET"), "{case}");
        }
        // A log-only tenant block is not enforced: the original bytes go upstream.
        let dir = policy_dir(FLOOR, TENANT_COMPUTER);
        let body = computer_body(ep);
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::LogOnly), &body, &[]).await;
        assert_eq!(ex.status, StatusCode::OK, "{ep:?}: {}", ex.text);
        assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{ep:?}");
        assert_eq!(request_entries(&ex.entries), [req("computer", "replace-result", false)], "{ep:?}");
    }
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
    assert_eq!(upstream_tools(Ep::ChatJson, &up).expect("tools"), ["read_file"]);
    // The body had no calls, so the header holds the request entries alone (§ 6.5.3).
    let h = header_entries(&sent);
    let tools: Vec<(&str, bool)> = h.iter().map(|e| (e["tool"].as_str().expect("tool"), e["enforced"].as_bool().expect("enforced"))).collect();
    assert_eq!(tools, [("blocked_rm", true), ("floor_rm", true)], "{h:?}");
    // Log-only (D33, Task 16 ruling (c)): the floor removal still applies, the tenant-blocked tool
    // stays and is only reported.
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::LogOnly));
    let sent = send(&app, &body).await;
    let up: Value = serde_json::from_slice(&mock.recorded().expect("recorded").body).expect("json");
    assert_eq!(upstream_tools(Ep::ChatJson, &up).expect("tools"), ["read_file", "blocked_rm"]);
    assert_eq!(
        request_entries(&header_entries(&sent)),
        [req("blocked_rm", "remove-definition", false), req("floor_rm", "remove-definition", true)]
    );
    // Without the floor tool, log-only sends the original bytes.
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

/// § 6.5.3 "whenever an entry exists", Task 16 ruling (a): a limit refusal after a successful
/// `apply` carries the request entries, stream or not.
#[tokio::test]
async fn a_limit_refusal_after_the_request_side_carries_the_request_entries_header() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = full_app(
        &mock.base_url,
        Some(limits(rules(Some(1), None, None), RecordingStore::new())),
        cedar(),
        tool_policy(&dir, TenantMode::Enforce),
    );
    assert_eq!(send(&app, &chat_body(&["read_file"], false)).await.status, StatusCode::OK);
    for stream in [false, true] {
        let sent = send(&app, &chat_body(&["read_file", "blocked_rm"], stream)).await;
        assert_eq!(sent.status, StatusCode::TOO_MANY_REQUESTS, "stream {stream}");
        assert_eq!(request_entries(&header_entries(&sent)), [req("blocked_rm", "remove-definition", true)], "stream {stream}");
    }
    assert_eq!(mock.request_count(), 1, "the refused requests never went upstream");
}

/// SMA-753 D4: the request side over the file source in `dir`, with a memory session history.
fn tool_policy_with_history(dir: &TempDir, store: &Arc<MemorySessionStore>) -> Option<Arc<RequestToolPolicy>> {
    let settings = tp_settings(TenantMode::Enforce);
    let cache = PolicyCache::new(
        Arc::new(FilePolicySource::new(dir.path())),
        CacheSettings::from_settings(&settings),
        Arc::new(HmacHistoryDigester::keyless()),
    );
    let store: Arc<dyn SessionStore> = store.clone();
    let history = ToolHistory::new(
        store,
        Arc::new(FixedClock(at(NOON))),
        Arc::new(OsEntropy),
        Duration::from_secs(1),
        Arc::new(paigasus_gateway::adapters::history::HmacHistoryDigester::keyless()),
    );
    Some(Arc::new(
        RequestToolPolicy::new(cache, settings, Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing())).with_history(Arc::new(history)),
    ))
}

/// The `c:` link of call `c1` of `read_file` in `session`, as an earlier response leaves it (F10).
async fn seed_link(store: &MemorySessionStore, session: Uuid) {
    let now_ms = u64::try_from(at(NOON).duration_since(std::time::UNIX_EPOCH).expect("after the epoch").as_millis()).expect("fits");
    let histories = Histories {
        org: org_a(),
        principal: PRINCIPAL_1.to_owned(),
        key: None,
        session,
    };
    let event_id = Uuid::from_u128(0x77);
    let calls = vec![CallEvent {
        id: event_id,
        tool: "read_file".to_owned(),
        call_id: None,
        item_id: None,
        fields: paigasus_kernel::tool_policy::history::Fields::Values(std::collections::BTreeMap::new()),
        key: None,
    }];
    store
        .reserve(&ReserveRequest {
            histories,
            turn: 1,
            calls,
            now_ms,
            read: None,
        })
        .await
        .expect("reserve");
    let items = vec![SettleItem {
        event_id,
        verdict: EventVerdict::Allow,
        tool: "read_file".to_owned(),
        call_id: Some("c1".to_owned()),
        item_id: None,
    }];
    store
        .settle(&SettleRequest {
            org: org_a(),
            principal: PRINCIPAL_1.to_owned(),
            session,
            items,
            now_ms,
        })
        .await
        .expect("settle");
}

/// A follow-up chat request: a result for call `c1`.
fn linked_chat_body() -> String {
    json!({"model":"gpt-4o-mini","messages":[
        {"role":"user","content":"hi"},
        {"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"c1","content":"out"}],
        "tools":chat_tools(&["read_file"])})
    .to_string()
}

fn result_events(store: &MemorySessionStore, session: Uuid) -> usize {
    store
        .events(org_a(), Scope::Session, &session.to_string(), "read_file")
        .iter()
        .filter(|e| e.kind == EventKind::Result)
        .count()
}

/// SMA-753 D4 (control): an admitted request with a linked result writes the result event.
#[tokio::test]
async fn an_admitted_request_writes_its_result_event() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = Arc::new(MemorySessionStore::new());
    let session = Uuid::from_u128(0x5e55);
    seed_link(&store, session).await;
    let app = full_app(
        &mock.base_url,
        Some(limits(rules(Some(5), None, None), RecordingStore::new())),
        cedar(),
        tool_policy_with_history(&dir, &store),
    );
    assert_eq!(send(&app, &linked_chat_body()).await.status, StatusCode::OK);
    assert_eq!(result_events(&store, session), 1);
}

/// SMA-753 D4, F10: a rate-limited request writes no history. The `c:` link is seeded first, so
/// the result event would be written if the write ran before the admission.
#[tokio::test]
async fn a_rate_limited_request_writes_no_history() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = Arc::new(MemorySessionStore::new());
    let session = Uuid::from_u128(0x5e55);
    seed_link(&store, session).await;
    let app = full_app(
        &mock.base_url,
        Some(limits(rules(Some(1), None, None), RecordingStore::new())),
        cedar(),
        tool_policy_with_history(&dir, &store),
    );
    // The first request uses the only admission. It has no result to link.
    assert_eq!(send(&app, &chat_body(&["read_file"], false)).await.status, StatusCode::OK);
    let sent = send(&app, &linked_chat_body()).await;
    assert_eq!(sent.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(result_events(&store, session), 0, "a refused request writes no result event");
}

/// § 7.2 "Mode `off` everywhere": with every mode off and an empty floor, the tool policy changes
/// no byte: the same upstream request, client bytes and header as `tool_policy: None`.
#[tokio::test]
async fn mode_off_everywhere_keeps_request_and_response_bytes() {
    let dir = policy_dir("", "");
    for ep in EPS {
        let body = ep.tools_body(&["read_file", "blocked_rm"]);
        let mut seen = Vec::new();
        for (decider, tp) in [
            (Arc::new(AllowAll) as Arc<dyn ToolCallDecider>, None),
            (
                Arc::new(CedarDecider::new(Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()))) as Arc<dyn ToolCallDecider>,
                tool_policy(&dir, TenantMode::Off),
            ),
        ] {
            let mock = ep.mock(CALLS).await;
            let app = app_for(ORG_A_PRN, &mock.base_url, None, ti(decider), tp);
            let sent = send_to(&app, ep.path(), &body).await;
            assert_eq!(sent.status, StatusCode::OK, "{ep:?}");
            seen.push((mock.recorded().expect("recorded").body, sent.body, sent.headers.get("paigasus-tool-decisions").cloned()));
        }
        assert_eq!(seen[0].0, body.as_bytes(), "the original request goes upstream ({ep:?})");
        assert!(String::from_utf8_lossy(&seen[1].1).contains("CANARY"), "nothing is blocked ({ep:?})");
        assert_eq!(seen[0], seen[1], "{ep:?}");
    }
}

/// D30 for chat, non-stream: the decider decides with the request's snapshot.
#[tokio::test]
async fn chat_non_stream_decides_with_the_snapshot() {
    let dir = policy_dir(FLOOR, TENANT);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, chat_json(&[("read_file", "{}"), ("blocked_rm", "{\"p\":\"CANARY\"}")])).await;
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
    let mock = MockOpenAi::spawn_raw_sse(chat_sse(CALLS)).await;
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
    let mock = MockOpenAi::spawn_json(StatusCode::OK, responses_json(&[("read_file", "{}"), ("blocked_rm", "{\"p\":\"CANARY\"}")])).await;
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
    let mock = MockOpenAi::spawn_raw_sse(responses_sse(CALLS)).await;
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

    let mock = MockOpenAi::spawn_raw_sse(chat_sse(CALLS)).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, cedar(), tool_policy(&dir, TenantMode::Enforce));
    assert_eq!(send(&app, &chat_body(&["read_file"], true)).await.status, StatusCode::OK);
    assert_eq!(overhead_samples(&snapshotter), 1, "stream with calls");
}

// --- Task 16: the § 7.2 matrix ---------------------------------------------------------------

/// § 7.2: log-only output is byte-equal to mode `off`, apart from the gateway's
/// `: paigasus-decision` comments and headers. The tenant denies `blocked_rm`, so log-only has
/// something to report.
#[tokio::test]
async fn log_only_is_byte_equal_to_off_apart_from_comments_and_headers() {
    let dir = policy_dir(FLOOR, TENANT);
    for ep in EPS {
        let body = ep.tools_body(&["read_file", "blocked_rm"]);
        let off = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Off), &body, CALLS).await;
        let log = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::LogOnly), &body, CALLS).await;
        assert_eq!((off.status, log.status), (StatusCode::OK, StatusCode::OK), "{ep:?}");
        assert_eq!(off.upstream.as_deref(), Some(body.as_bytes()), "off sends the original ({ep:?})");
        assert_eq!(log.upstream, off.upstream, "the same upstream request ({ep:?})");
        assert_eq!(without_comments(&log.text), without_comments(&off.text), "{ep:?}");
        if !ep.stream() {
            assert_eq!(log.sent.body, off.sent.body, "{ep:?}");
        }
        assert!(log.text.contains("CANARY"), "log-only blocks nothing ({ep:?})");
        // The comparison is not vacuous: log-only reports what enforce would do.
        assert_eq!(call_of(&log.entries, "blocked_rm"), OBSERVED, "{ep:?}");
        assert_eq!(request_entries(&log.entries), [req("blocked_rm", "remove-definition", false)], "{ep:?}");
        assert!(request_entries(&off.entries).is_empty(), "{ep:?}");
    }
}

/// § 6.4: enforce blocks a denied call and allows the other; log-only observes the denied call.
#[tokio::test]
async fn enforce_blocks_and_log_only_observes_a_denied_call() {
    let dir = policy_dir(FLOOR, TENANT);
    for ep in EPS {
        let body = ep.tools_body(&["read_file"]);
        let enforce = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &body, CALLS).await;
        assert_eq!(enforce.status, StatusCode::OK, "{ep:?}");
        assert_eq!(call_of(&enforce.entries, "read_file"), ALLOWED, "{ep:?}");
        assert_eq!(call_of(&enforce.entries, "blocked_rm"), BLOCKED, "{ep:?}");
        assert!(!enforce.text.contains("CANARY"), "{ep:?}: {}", enforce.text);
        let log = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::LogOnly), &body, CALLS).await;
        assert_eq!(call_of(&log.entries, "read_file"), ALLOWED, "{ep:?}");
        assert_eq!(call_of(&log.entries, "blocked_rm"), OBSERVED, "{ep:?}");
        assert!(log.text.contains("CANARY"), "{ep:?}");
    }
}

/// The call entry a tenant-unavailable call gets under `mode` and the effective `on` (§ 6.4).
fn unavailable_tenant(mode: TenantMode, on: OnUnavailable) -> (Option<&'static str>, Option<&'static str>, Option<bool>, Option<&'static str>) {
    match (mode, on) {
        (TenantMode::Enforce, OnUnavailable::Closed) => UNAVAILABLE_BLOCKED,
        _ => UNAVAILABLE_OBSERVED,
    }
}

/// Run the tenant-fault matrix: modes log-only and enforce, `closed` and `open`, every endpoint.
/// The floor still applies (`floor_rm` is blocked). The request defines no floor-denied tool, so
/// the request side changes nothing and the original bytes go upstream. A floor-denied definition
/// is removed during a tenant outage (controller ruling I3); see
/// `a_tenant_outage_still_removes_a_floor_denied_definition`.
async fn tenant_fault_matrix(fault: Fault, load_timeout: Duration) {
    let dir = policy_dir(FLOOR, TENANT);
    let calls = &[("read_file", "{}"), ("blocked_rm", "{}"), ("floor_rm", "{}")];
    let mut cases = Vec::new();
    for ep in EPS {
        for mode in [TenantMode::LogOnly, TenantMode::Enforce] {
            for on in [OnUnavailable::Closed, OnUnavailable::Open] {
                cases.push((ep, mode, on));
            }
        }
    }
    // The floor is in the cache before the request, so `load_timeout` covers the tenant load only.
    // The cases run at once: with a hanging source each one waits the whole `load_timeout`.
    let mut runs = Vec::new();
    for &(ep, mode, on) in &cases {
        let settings = ToolPolicySettings {
            load_timeout,
            ..tp_settings_on(mode, on)
        };
        let tp = tp_preloaded(FaultySource::arc(&dir, None, Some(fault)), settings).await;
        let body = ep.tools_body(&["read_file", "blocked_rm"]);
        runs.push(async move {
            let ex = exchange(ep, ORG_A_PRN, tp, &body, calls).await;
            (body, ex)
        });
    }
    let results = futures::future::join_all(runs).await;
    for (&(ep, mode, on), (body, ex)) in cases.iter().zip(&results) {
        let case = format!("{ep:?} {mode:?} {on:?}");
        assert_eq!(ex.status, StatusCode::OK, "{case}");
        assert_eq!(call_of(&ex.entries, "read_file"), unavailable_tenant(mode, on), "{case}");
        assert_eq!(call_of(&ex.entries, "blocked_rm"), unavailable_tenant(mode, on), "{case}");
        assert_eq!(call_of(&ex.entries, "floor_rm"), BLOCKED, "the floor applies ({case})");
        assert!(request_entries(&ex.entries).is_empty(), "{case}");
        assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{case}");
    }
}

/// § 6.4, § 7.2: a tenant source that fails.
#[tokio::test]
async fn fail_closed_and_fail_open_with_an_unavailable_source() {
    tenant_fault_matrix(Fault::Fail, Duration::from_secs(60)).await;
}

/// § 6.4, D31, § 7.2: a tenant source that is slower than `load_timeout_ms` (it never answers).
/// The floor is preloaded, so the short time limit only sets how long the test runs.
#[tokio::test]
async fn fail_closed_and_fail_open_with_a_slow_source() {
    tenant_fault_matrix(Fault::Hang, Duration::from_millis(200)).await;
}

/// Controller ruling I3 (D33): with the tenant part unavailable, the request side plans the floor
/// alone in every mode. A floor-denied definition is removed (enforced), and the tenant-only deny
/// of `blocked_rm` is not planned.
#[tokio::test]
async fn a_tenant_outage_still_removes_a_floor_denied_definition() {
    let dir = policy_dir(FLOOR, TENANT);
    let calls = &[("read_file", "{}")];
    for ep in EPS {
        let body = ep.tools_body(&["read_file", "blocked_rm", "floor_rm"]);
        for mode in [TenantMode::LogOnly, TenantMode::Enforce] {
            for on in [OnUnavailable::Closed, OnUnavailable::Open] {
                let tp = tp_preloaded(FaultySource::arc(&dir, None, Some(Fault::Fail)), tp_settings_on(mode, on)).await;
                let ex = exchange(ep, ORG_A_PRN, tp, &body, calls).await;
                let case = format!("{ep:?} {mode:?} {on:?}");
                assert_eq!(ex.status, StatusCode::OK, "{case}");
                assert_eq!(request_entries(&ex.entries), [req("floor_rm", "remove-definition", true)], "{case}");
                let up = String::from_utf8(ex.upstream.clone().expect("upstream")).expect("utf-8");
                assert!(!up.contains("floor_rm"), "the floor tool is removed ({case}): {up}");
                assert!(up.contains("blocked_rm") && up.contains("read_file"), "the other tools stay ({case}): {up}");
            }
        }
    }
}

/// P-6, § 7.2: an unavailable floor under `closed` blocks every call in every tenant mode; under
/// `open` every call is allowed and reported. The request side changes nothing.
#[tokio::test]
async fn an_unavailable_floor_blocks_every_call_under_closed_and_reports_under_open() {
    let dir = policy_dir(FLOOR, TENANT);
    std::fs::remove_file(dir.path().join("floor.cedar")).expect("remove the floor");
    let calls = &[("read_file", "{}"), ("floor_rm", "{}")];
    for ep in EPS {
        let body = ep.tools_body(&["read_file", "floor_rm"]);
        for mode in [TenantMode::Off, TenantMode::LogOnly, TenantMode::Enforce] {
            for (on, want) in [(OnUnavailable::Closed, UNAVAILABLE_BLOCKED), (OnUnavailable::Open, UNAVAILABLE_OBSERVED)] {
                let tp = tp_with(Arc::new(FilePolicySource::new(dir.path())), tp_settings_on(mode, on));
                let ex = exchange(ep, ORG_A_PRN, tp, &body, calls).await;
                let case = format!("{ep:?} {mode:?} {on:?}");
                assert_eq!(ex.status, StatusCode::OK, "{case}");
                assert_eq!(call_of(&ex.entries, "read_file"), want, "{case}");
                assert_eq!(call_of(&ex.entries, "floor_rm"), want, "{case}");
                assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{case}");
            }
        }
    }
}

/// D34, § 7.2: the per-org `on_policy_unavailable` wins over the global value, both ways. An org
/// entry with no override follows the global value.
#[tokio::test]
async fn a_per_org_override_wins_over_the_global_value() {
    let dir = policy_dir(FLOOR, TENANT);
    for ep in EPS {
        let body = ep.tools_body(&["read_file"]);
        for (global, over, want) in [
            (OnUnavailable::Closed, Some(OnUnavailable::Open), UNAVAILABLE_OBSERVED),
            (OnUnavailable::Open, Some(OnUnavailable::Closed), UNAVAILABLE_BLOCKED),
            (OnUnavailable::Closed, None, UNAVAILABLE_BLOCKED),
            (OnUnavailable::Open, None, UNAVAILABLE_OBSERVED),
        ] {
            let mut settings = tp_settings_on(TenantMode::Off, global);
            settings.orgs.insert(org_a(), (TenantMode::Enforce, over));
            let tp = tp_preloaded(FaultySource::arc(&dir, None, Some(Fault::Fail)), settings).await;
            let ex = exchange(ep, ORG_A_PRN, tp, &body, CALLS).await;
            let case = format!("{ep:?} global {global:?} org {over:?}");
            assert_eq!(call_of(&ex.entries, "read_file"), want, "{case}");
        }
    }
}

/// § 7.2: an argument policy permits `read_file` only under `/workspace/`.
#[tokio::test]
async fn an_argument_policy_allows_workspace_paths_only() {
    let tenant =
        r#"@id("ws") permit(principal is Pgs::Iam::Principal, action == Pgs::Gateway::Action::"read_file", resource) when { context.input has path && context.input.path like "/workspace/*" };"#;
    let dir = policy_dir(FLOOR, tenant);
    for ep in EPS {
        let body = ep.tools_body(&["read_file"]);
        let ok = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &body, &[("read_file", r#"{"path":"/workspace/a.txt"}"#)]).await;
        assert_eq!(call_of(&ok.entries, "read_file"), ALLOWED, "{ep:?}");
        assert!(ok.text.contains("a.txt"), "{ep:?}");
        let bad = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &body, &[("read_file", r#"{"path":"/etc/passwd"}"#)]).await;
        assert_eq!(call_of(&bad.entries, "read_file"), BLOCKED, "{ep:?}");
        assert!(!bad.text.contains("passwd"), "{ep:?}");
    }
}

/// § 6.5.1 steps 2 and 4: definition removal and each `tool_choice` case, in enforce mode.
#[tokio::test]
async fn definition_removal_and_each_tool_choice_case() {
    let dir = policy_dir(FLOOR, TENANT);
    for ep in EPS {
        let named = |n: &str| {
            if ep.chat() {
                json!({"type":"function","function":{"name":n}})
            } else {
                json!({"type":"function","name":n})
            }
        };
        let allowed = |names: &[&str]| {
            if ep.chat() {
                json!({"type":"allowed_tools","allowed_tools":{"mode":"required","tools":names.iter().map(|n| json!({"type":"function","function":{"name":n}})).collect::<Vec<_>>()}})
            } else {
                json!({"type":"allowed_tools","mode":"required","tools":names.iter().map(|n| json!({"type":"function","name":n})).collect::<Vec<_>>()})
            }
        };
        // (tool_choice in, tool_choice upstream, whether a reset entry is reported)
        let cases: Vec<(Value, Value, bool)> = vec![
            (named("blocked_rm"), json!("auto"), true),
            (named("read_file"), named("read_file"), false),
            (allowed(&["read_file", "blocked_rm"]), allowed(&["read_file"]), true),
            (allowed(&["blocked_rm"]), json!("auto"), true),
            (json!("required"), json!("required"), false),
            (json!("none"), json!("none"), false),
        ];
        for (choice, want, reset) in cases {
            let mut v = if ep.chat() {
                json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"tools":chat_tools(&["read_file","blocked_rm"])})
            } else {
                json!({"model":"gpt-6","input":"hi","tools":responses_tools(&["read_file","blocked_rm"])})
            };
            v["tool_choice"] = choice.clone();
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.body(v), &[]).await;
            let case = format!("{ep:?} {choice}");
            assert_eq!(ex.status, StatusCode::OK, "{case}: {}", ex.text);
            let up = ex.upstream_json();
            assert_eq!(upstream_tools(ep, &up).expect("tools"), ["read_file"], "{case}");
            assert_eq!(up["tool_choice"], want, "{case}");
            let mut expected = vec![req("blocked_rm", "remove-definition", true)];
            if reset {
                expected.push(req("blocked_rm", "reset-tool-choice", true));
            }
            assert_eq!(request_entries(&ex.entries), expected, "{case}");
        }
        // An unknown choice with an enforced removal is refused (D17).
        let mut v = if ep.chat() {
            json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"tools":chat_tools(&["read_file","blocked_rm"])})
        } else {
            json!({"model":"gpt-6","input":"hi","tools":responses_tools(&["read_file","blocked_rm"])})
        };
        v["tool_choice"] = json!({"type":"a_future_kind"});
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.body(v.clone()), &[]).await;
        assert_eq!(ex.status, StatusCode::BAD_REQUEST, "{ep:?}");
        assert_eq!(ex.sent.json()["error"]["code"], "invalid-request-body", "{ep:?}");
        assert!(ex.upstream.is_none(), "{ep:?}");
        assert_eq!(request_entries(&ex.entries), [req("blocked_rm", "remove-definition", true)], "{ep:?}");
        // The same unknown choice with no removal passes unchanged.
        v["tools"] = if ep.chat() { chat_tools(&["read_file"]) } else { responses_tools(&["read_file"]) };
        let body = ep.body(v);
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &body, &[]).await;
        assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{ep:?}");
    }
}

/// § 6.5.1 step 5: the last definition is removed (Chat, the legacy Chat form, Responses and a
/// Responses namespace).
#[tokio::test]
async fn each_last_definition_case() {
    let tenant = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"blocked_rm", resource); @id("ns") forbid(principal, action == Pgs::Gateway::Action::"ns.blocked", resource);"#;
    let dir = policy_dir(FLOOR, tenant);
    for ep in EPS {
        let removed_and_reset = || vec![req("blocked_rm", "remove-definition", true), req("blocked_rm", "reset-tool-choice", true)];
        let cases: Vec<(&str, Value, &[&str], Vec<RequestEntryRow>)> = if ep.chat() {
            vec![
                (
                    "tools",
                    json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"tools":chat_tools(&["blocked_rm"]),"tool_choice":{"type":"function","function":{"name":"blocked_rm"}},"parallel_tool_calls":true}),
                    &["tools", "tool_choice", "parallel_tool_calls"],
                    removed_and_reset(),
                ),
                (
                    "legacy functions",
                    json!({"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"functions":[{"name":"blocked_rm","parameters":{}}],"function_call":{"name":"blocked_rm"}}),
                    &["functions", "function_call"],
                    removed_and_reset(),
                ),
            ]
        } else {
            vec![
                (
                    "tools",
                    json!({"model":"gpt-6","input":"hi","tools":responses_tools(&["blocked_rm"]),"tool_choice":"required","parallel_tool_calls":false}),
                    &["tools", "tool_choice", "parallel_tool_calls"],
                    vec![req("blocked_rm", "remove-definition", true)],
                ),
                (
                    "namespace",
                    json!({"model":"gpt-6","input":"hi","tools":[{"type":"namespace","name":"ns","tools":[{"type":"function","name":"blocked","parameters":{}}]}],"tool_choice":"auto","parallel_tool_calls":true}),
                    &["tools", "tool_choice", "parallel_tool_calls"],
                    vec![req("ns.blocked", "remove-definition", true)],
                ),
            ]
        };
        for (case, v, gone, want) in cases {
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.body(v), &[]).await;
            assert_eq!(ex.status, StatusCode::OK, "{ep:?} {case}: {}", ex.text);
            let up = ex.upstream_json();
            for key in gone {
                assert!(up.get(*key).is_none(), "{ep:?} {case}: {key} stays in {up}");
            }
            assert!(up["model"].as_str().is_some(), "{ep:?} {case}");
            assert_eq!(request_entries(&ex.entries), want, "{ep:?} {case}");
        }
        // A namespace that keeps one inner tool stays, with that tool only.
        if !ep.chat() {
            let v = json!({"model":"gpt-6","input":"hi","tools":[{"type":"namespace","name":"ns","tools":[{"type":"function","name":"blocked","parameters":{}},{"type":"function","name":"kept","parameters":{}}]}]});
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.body(v), &[]).await;
            let up = ex.upstream_json();
            let inner: Vec<&str> = up["tools"][0]["tools"].as_array().expect("inner").iter().map(|t| t["name"].as_str().expect("name")).collect();
            assert_eq!(inner, ["kept"], "{ep:?}");
        }
    }
}

/// § 6.5.1 step 2, § 4.4: a permit that reads the input can allow some call, so the definition
/// stays; a tool with no permit at all is removed.
#[tokio::test]
async fn an_input_reading_permit_keeps_the_definition() {
    let tenant =
        r#"@id("ws") permit(principal is Pgs::Iam::Principal, action == Pgs::Gateway::Action::"read_file", resource) when { context.input has path && context.input.path like "/workspace/*" };"#;
    let dir = policy_dir(FLOOR, tenant);
    for ep in EPS {
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.tools_body(&["read_file", "send_email"]), &[]).await;
        assert_eq!(upstream_tools(ep, &ex.upstream_json()).expect("tools"), ["read_file"], "{ep:?}");
        assert_eq!(request_entries(&ex.entries), [req("send_email", "remove-definition", true)], "{ep:?}");
    }
}

fn is_notice(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.starts_with(NOTICE_PREFIX) && s.ends_with("(tool-result-blocked)."))
}

/// § 6.5.1 step 3, § 6.5.2: tool-result replacement for each result type. A chat `role:"tool"`
/// result takes its name from the earlier call with that id (and from each call when two share
/// the id); a legacy `role:"function"` result carries its own name.
#[tokio::test]
async fn tool_result_replacement_for_each_result_type() {
    let tenant = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource);
        @id("t") forbid(principal, action in [Pgs::Gateway::Action::"blocked_rm", Pgs::Gateway::Action::"blocked_sql", Pgs::Gateway::Action::"computer", Pgs::Gateway::Action::"local_shell", Pgs::Gateway::Action::"shell", Pgs::Gateway::Action::"apply_patch", Pgs::Gateway::Action::"srv.rm"], resource);"#;
    let dir = policy_dir(FLOOR, tenant);
    for ep in EPS {
        let v = if ep.chat() {
            json!({"model":"gpt-4o-mini","tools":chat_tools(&["read_file"]),"messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"call_a","type":"function","function":{"name":"read_file","arguments":"{}"}},
                    {"id":"call_b","type":"function","function":{"name":"blocked_rm","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_a","content":"OK-A"},
                {"role":"tool","tool_call_id":"call_b","content":"SECRET-B"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"dup","type":"function","function":{"name":"read_file","arguments":"{}"}}]},
                {"role":"assistant","content":null,"tool_calls":[{"id":"dup","type":"function","function":{"name":"blocked_rm","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"dup","content":"SECRET-DUP"},
                {"role":"tool","tool_call_id":"call_unknown","content":"OK-UNKNOWN"},
                {"role":"function","name":"blocked_rm","content":"SECRET-LEGACY"},
                {"role":"function","name":"read_file","content":"OK-LEGACY"}]})
        } else {
            json!({"model":"gpt-6","tools":responses_tools(&["read_file"]),"input":[
                {"role":"user","content":"hi"},
                {"type":"function_call","id":"fc_0","call_id":"f0","name":"read_file","arguments":"{}","status":"completed"},
                {"type":"function_call_output","call_id":"f0","output":"OK-F0"},
                {"type":"function_call","id":"fc_1","call_id":"f1","name":"blocked_rm","arguments":"{}","status":"completed"},
                {"type":"function_call_output","call_id":"f1","output":"SECRET-F1"},
                {"type":"custom_tool_call","id":"ctc_1","call_id":"cu1","name":"blocked_sql","input":"SELECT 1"},
                {"type":"custom_tool_call_output","call_id":"cu1","output":"SECRET-CUSTOM"},
                {"type":"local_shell_call","id":"lsh_1","call_id":"ls1","action":{"type":"exec","command":["ls"]},"status":"completed"},
                {"type":"local_shell_call_output","call_id":"ls1","output":"SECRET-LOCAL"},
                {"type":"shell_call","id":"sh_1","call_id":"sh1","action":{"commands":["ls"]},"status":"completed"},
                {"type":"shell_call_output","call_id":"sh1","output":[{"stdout":"SECRET-SHELL"}]},
                {"type":"apply_patch_call","id":"apc_1","call_id":"ap1","operation":{"type":"create_file","diff":"+hi\n","path":"a.txt"},"status":"completed"},
                {"type":"apply_patch_call_output","call_id":"ap1","status":"completed","output":"SECRET-PATCH"},
                {"type":"mcp_approval_request","id":"mcpr_1","name":"rm","server_label":"srv","arguments":"{}"},
                {"type":"mcp_approval_response","approval_request_id":"mcpr_1","approve":true}]})
        };
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, TenantMode::Enforce), &ep.body(v), &[]).await;
        assert_eq!(ex.status, StatusCode::OK, "{ep:?}: {}", ex.text);
        let raw = String::from_utf8(ex.upstream.clone().expect("upstream")).expect("utf8");
        assert!(!raw.contains("SECRET"), "{ep:?}: {raw}");
        let up = ex.upstream_json();
        if ep.chat() {
            let m = up["messages"].as_array().expect("messages");
            let contents: Vec<(bool, &Value)> = m.iter().map(|x| (is_notice(&x["content"]), &x["content"])).collect();
            for (i, notice) in [(2, false), (3, true), (6, true), (7, false), (8, true), (9, false)] {
                assert_eq!(contents[i].0, notice, "{ep:?} message {i}: {:?}", contents[i].1);
            }
            assert!(m[3]["content"].as_str().expect("content").contains("\"blocked_rm\""), "the name comes from the earlier call");
            assert_eq!(
                request_entries(&ex.entries),
                [
                    req("blocked_rm", "replace-result", true),
                    req("blocked_rm", "replace-result", true),
                    req("blocked_rm", "replace-result", true)
                ],
                "{ep:?}"
            );
        } else {
            let items = up["input"].as_array().expect("input");
            assert_eq!(items[2]["output"], "OK-F0", "{ep:?}");
            for i in [4, 6, 8, 12] {
                assert!(is_notice(&items[i]["output"]), "{ep:?} item {i}: {}", items[i]);
            }
            let notice = items[10]["output"][0]["stderr"].clone();
            assert!(is_notice(&notice), "{ep:?}: {}", items[10]);
            assert_eq!(items[10]["output"], json!([{"stdout": "", "stderr": notice, "outcome": {"type": "exit", "exit_code": 1}}]), "{ep:?}");
            assert_eq!(items[14]["approve"], false, "{ep:?}: an mcp approval becomes a refusal");
            let mut have = request_entries(&ex.entries);
            have.sort();
            let mut want: Vec<_> = ["blocked_rm", "blocked_sql", "local_shell", "shell", "apply_patch", "srv.rm"]
                .iter()
                .map(|t| req(t, "replace-result", true))
                .collect();
            want.sort();
            assert_eq!(have, want, "{ep:?}");
        }
    }
}

/// D15, § 7.2: the shipped floor forbids every call when two definitions share a name, in every
/// tenant mode. The forbid reads the findings, so the definitions are not removed.
#[tokio::test]
async fn name_collision_with_the_shipped_floor() {
    let dir = policy_dir(SHIPPED_FLOOR, TENANT);
    for ep in EPS {
        for mode in [TenantMode::Off, TenantMode::LogOnly, TenantMode::Enforce] {
            let body = ep.tools_body(&["read_file", "Read_File"]);
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, mode), &body, &[("read_file", "{}")]).await;
            let case = format!("{ep:?} {mode:?}");
            assert_eq!(call_of(&ex.entries, "read_file"), BLOCKED, "{case}");
            assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{case}");
            // The control: one definition, the call is allowed.
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, mode), &ep.tools_body(&["read_file"]), &[("read_file", "{}")]).await;
            assert_eq!(call_of(&ex.entries, "read_file").0, Some("allow"), "{case}");
        }
    }
}

/// D22, § 7.2: with an empty catalog, a tenant that permits only `InvokeTool` (catalog tools)
/// denies every tool, so enforce removes every definition. With a catalog, its tools stay.
#[tokio::test]
async fn an_empty_catalog_in_enforce_removes_every_definition() {
    let tenant = r#"@id("catalog") permit(principal is Pgs::Iam::Principal, action in Pgs::Gateway::Action::"InvokeTool", resource);"#;
    let empty = policy_dir(FLOOR, tenant);
    let listed = policy_dir(FLOOR, "");
    write_tenant(&listed, ORG_A, tenant, Some(r#"["read_file"]"#));
    for ep in EPS {
        let body = ep.tools_body(&["read_file", "send_email"]);
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&empty, TenantMode::Enforce), &body, &[]).await;
        assert_eq!(ex.status, StatusCode::OK, "{ep:?}");
        assert_eq!(upstream_tools(ep, &ex.upstream_json()), None, "{ep:?}: every definition is removed");
        assert_eq!(
            request_entries(&ex.entries),
            [req("read_file", "remove-definition", true), req("send_email", "remove-definition", true)],
            "{ep:?}"
        );
        let ex = exchange(ep, ORG_A_PRN, tool_policy(&listed, TenantMode::Enforce), &body, &[]).await;
        assert_eq!(upstream_tools(ep, &ex.upstream_json()).expect("tools"), ["read_file"], "{ep:?}");
    }
}

/// D7, § 7.2: a team-scoped and a project-scoped key use their org's mode. A key of another org
/// gets the default mode.
#[tokio::test]
async fn team_and_project_scoped_keys_use_their_org_mode() {
    let dir = policy_dir(FLOOR, TENANT);
    for ep in EPS {
        for (scope, want) in [(TEAM_IN_A, BLOCKED), (PROJECT_IN_A, BLOCKED), (TEAM_IN_B, ALLOWED)] {
            let mut settings = tp_settings(TenantMode::Off);
            settings.orgs.insert(org_a(), (TenantMode::Enforce, None));
            let tp = tp_with(Arc::new(FilePolicySource::new(dir.path())), settings);
            let ex = exchange(ep, scope, tp, &ep.tools_body(&["read_file"]), CALLS).await;
            assert_eq!(call_of(&ex.entries, "blocked_rm"), want, "{ep:?} {scope}");
            assert_eq!(call_of(&ex.entries, "read_file"), ALLOWED, "{ep:?} {scope}");
        }
    }
}

/// D33, § 7.2: in tenant mode off and log-only a floor forbid still blocks the call and removes
/// the definition. The red-first proof of Task 16 deletes the floor rule in `policy/calls.rs`.
#[tokio::test]
async fn a_floor_forbid_blocks_in_tenant_off_and_log_only() {
    let dir = policy_dir(FLOOR, TENANT);
    let calls = &[("read_file", "{}"), ("floor_rm", "{\"p\":\"CANARY\"}")];
    for ep in EPS {
        for mode in [TenantMode::Off, TenantMode::LogOnly] {
            let ex = exchange(ep, ORG_A_PRN, tool_policy(&dir, mode), &ep.tools_body(&["read_file", "floor_rm"]), calls).await;
            let case = format!("{ep:?} {mode:?}");
            assert_eq!(call_of(&ex.entries, "floor_rm"), BLOCKED, "{case}");
            assert_eq!(call_of(&ex.entries, "read_file"), ALLOWED, "{case}");
            assert!(!ex.text.contains("CANARY"), "{case}");
            assert_eq!(upstream_tools(ep, &ex.upstream_json()).expect("tools"), ["read_file"], "{case}");
            assert_eq!(request_entries(&ex.entries), [req("floor_rm", "remove-definition", true)], "{case}");
        }
    }
}

// --- SMA-855 Task 23: the tool-call detectors end to end (§ 7.4) -------------------------------

mod detectors {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use paigasus_gateway::adapters::decision_log::proto;
    use paigasus_gateway::adapters::detectors::BuiltInDetector;
    use paigasus_gateway::application::decision_log::DecisionRecorder;
    use paigasus_gateway::application::detectors::DetectorPipeline;
    use paigasus_gateway::application::tool_policy::FindingLabels;
    use paigasus_gateway::config::{DETECTOR_MARGIN_MS, DetectorSettings, GatewayConfig};
    use paigasus_gateway::domain::decision_log::{DecisionLog, OsEntropy};
    use paigasus_gateway::domain::detectors::{BuiltInStage, DetectBatch, Detector, DetectorError, PipelineOutput, Side as DetectSide, Subject};
    use paigasus_gateway::domain::limits::SystemClock;
    use paigasus_gateway::runtime;
    use paigasus_gateway_core::decision_log::{AppliedEffect, DecisionRecord, RecordBasis, RecordedAction, Side as RecordSide};
    use paigasus_gateway_core::{Format, SeenHosts, ToolTurn, parse_request, screen_request};
    use paigasus_kernel::detect::{CallSubject, Declared, Finding, HostSet, ToolInput, detect_call, injection_markers};
    use paigasus_kernel::tool_policy::ToolInvocation;
    use prost::Message;

    use super::*;

    /// The tenant guide policies (Task 17): one shared fixture, one policy per paragraph.
    const DETECTOR_POLICIES: &str = include_str!("../../../../../docs/tool-policies/detector-policies.cedar");
    /// A tenant set that permits every IAM principal and forbids nothing.
    const PERMIT_ALL: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource);"#;
    /// The canary host of the call arguments (one per test, A2).
    const CANARY_HOST: &str = "canary-host-4c1d.example";
    /// The visible part of the canary injection phrase.
    const CANARY_PHRASE: &str = "visible-canary-phrase-8a2e";
    /// The hidden part, as plain ASCII. The result holds it as Unicode tag characters.
    const CANARY_TAG_TEXT: &str = "canary-tag-9d4f";
    /// The distinctive part of the canary secret. The whole secret is an AWS key id form.
    const CANARY_SECRET_BODY: &str = "Q7CANARY3ZX9W2KL";
    /// The canary in the panic payload of the panicking stage.
    const CANARY_PANIC: &str = "canary-panic-payload-61b0";

    /// The policy of the fixture with this `@id`.
    fn fixture_policy(id: &str) -> &'static str {
        let head = format!("@id(\"{id}\")");
        DETECTOR_POLICIES.split("\n\n").find(|p| p.starts_with(&head)).unwrap_or_else(|| panic!("no fixture policy {id}"))
    }

    /// The tenant set of the starter policy: permit all, and `detectors-block-high`.
    fn starter() -> String {
        format!("{PERMIT_ALL}\n{}", fixture_policy("detectors-block-high"))
    }

    /// The canary secret: an AWS access key id (`secret_in_args`, high).
    fn canary_secret() -> String {
        ["AKIA", CANARY_SECRET_BODY].concat()
    }

    /// `s` as Unicode tag characters (U+E0020 to U+E007E).
    fn tags(s: &str) -> String {
        s.chars().map(|c| char::from_u32(0xE0000 + c as u32).expect("a tag character")).collect()
    }

    /// A tool result with the canary phrase and at least 4 tag characters (Task 22: a single tag
    /// character gives no finding). `injection_markers` is high for this text.
    fn injected() -> String {
        format!("{CANARY_PHRASE} {}", tags(CANARY_TAG_TEXT))
    }

    /// The arguments of the secret call: the canary secret and the canary host.
    fn secret_args() -> String {
        json!({"url": format!("https://{CANARY_HOST}/x"), "key": canary_secret()}).to_string()
    }

    /// Every byte string that no output may hold.
    fn canaries() -> Vec<String> {
        vec![
            CANARY_SECRET_BODY.to_owned(),
            CANARY_HOST.to_owned(),
            CANARY_PHRASE.to_owned(),
            CANARY_TAG_TEXT.to_owned(),
            tags(CANARY_TAG_TEXT),
            CANARY_PANIC.to_owned(),
        ]
    }

    fn assert_no_canary(what: &str, bytes: &[u8]) {
        for c in canaries() {
            assert!(!bytes.windows(c.len()).any(|w| w == c.as_bytes()), "{what} holds a canary");
        }
    }

    // --- the stack -------------------------------------------------------------------------------

    /// A decision log that keeps every record, for the assertions.
    #[derive(Default)]
    struct CaptureLog(Mutex<Vec<DecisionRecord>>);

    impl DecisionLog for CaptureLog {
        fn wants_digest(&self) -> bool {
            false
        }

        fn record(&self, record: DecisionRecord) {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(record);
        }
    }

    impl CaptureLog {
        fn records(&self) -> Vec<DecisionRecord> {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
        }

        fn clear(&self) {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        }
    }

    /// A recorder over `log`.
    fn recorder_over(log: &Arc<CaptureLog>) -> Arc<DecisionRecorder> {
        Arc::new(DecisionRecorder::new(Arc::clone(log) as Arc<dyn DecisionLog>, Arc::new(SystemClock), Arc::new(OsEntropy), None))
    }

    /// The detectors of one stack.
    struct Det {
        /// `None`: the built-in detectors, behind a stage that counts the request screens (F9).
        built_in: Option<Arc<dyn BuiltInStage>>,
        webhooks: Vec<Arc<dyn Detector>>,
        /// The opt-in list of `ORG_A`. `None`: every webhook of `webhooks` (SMA-856 D8).
        opt_in: Option<Vec<String>>,
        enabled: bool,
        timeout_ms: u64,
        max_scan_bytes: usize,
    }

    impl Default for Det {
        fn default() -> Self {
            Det {
                built_in: None,
                webhooks: Vec::new(),
                opt_in: None,
                enabled: true,
                timeout_ms: 500,
                max_scan_bytes: 16 * MIB,
            }
        }
    }

    /// The request side and the interception over one policy directory, with one recorder over a
    /// [`CaptureLog`], as `runtime::build_tool_policy` wires them.
    struct Stack {
        ti: Option<Arc<ToolInterception>>,
        tp: Option<Arc<RequestToolPolicy>>,
        log: Arc<CaptureLog>,
        /// The request screens that the counting built-in stage saw.
        screens: Arc<AtomicUsize>,
    }

    impl Stack {
        async fn run(&self, ep: Ep, body: &str, calls: &[(&str, &str)]) -> Ex {
            exchange_with(ep, ORG_A_PRN, self.ti.clone(), self.tp.clone(), body, calls).await
        }

        fn screens(&self) -> usize {
            self.screens.load(Ordering::SeqCst)
        }
    }

    fn build(dir: &TempDir, mode: TenantMode, det: Det, limits: CoreLimits) -> Stack {
        let log = Arc::new(CaptureLog::default());
        let recorder = recorder_over(&log);
        let screens = Arc::new(AtomicUsize::new(0));
        let built_in = det.built_in.unwrap_or_else(|| {
            Arc::new(CountingStage {
                inner: BuiltInDetector::new(det.max_scan_bytes),
                requests: Arc::clone(&screens),
            })
        });
        let settings = DetectorSettings {
            enabled: det.enabled,
            timeout: Duration::from_millis(det.timeout_ms),
            max_scan_bytes: det.max_scan_bytes,
            margin: Duration::from_millis(DETECTOR_MARGIN_MS),
        };
        let opt_in: Vec<String> = det.opt_in.unwrap_or_else(|| det.webhooks.iter().map(|d| d.name().to_owned()).collect());
        let pipeline = Arc::new(DetectorPipeline::new(built_in, det.webhooks, None, settings.timeout));
        let mut cedar = CedarDecider::new(Arc::clone(&recorder));
        // The runtime gives the decider no pipeline with the detectors off.
        if det.enabled {
            cedar = cedar.with_detectors(Arc::clone(&pipeline), limits.decide_timeout, settings);
        }
        let tp_settings = tp_settings(mode);
        let cache = PolicyCache::new(
            Arc::new(FilePolicySource::new(dir.path())),
            CacheSettings::from_settings(&tp_settings),
            Arc::new(HmacHistoryDigester::keyless()),
        );
        // The request side keeps the pipeline also with `enabled = false`, so a test can prove that
        // the switch alone stops the screen.
        let tp = RequestToolPolicy::new(cache, tp_settings, Arc::clone(&recorder))
            .with_detectors(Some(pipeline), settings)
            .with_org_webhooks(std::collections::HashMap::from([(org_a(), Arc::from(opt_in))]));
        Stack {
            ti: Some(Arc::new(ToolInterception {
                limits,
                decider: Arc::new(cedar),
                recorder: Some(recorder),
                history: None,
            })),
            tp: Some(Arc::new(tp)),
            log,
            screens,
        }
    }

    fn stack(dir: &TempDir, mode: TenantMode, det: Det) -> Stack {
        build(dir, mode, det, CoreLimits::defaults())
    }

    // --- requests --------------------------------------------------------------------------------

    /// A request that declares `fetch` and holds one tool result `(tool, text)` per entry, each
    /// after its call. The result `i` is at `result_slot(ep, _, i)`.
    fn results_body(ep: Ep, results: &[(&str, &str)]) -> String {
        if ep.chat() {
            let calls: Vec<Value> = results
                .iter()
                .enumerate()
                .map(|(i, (n, _))| json!({"id":format!("call_r{i}"),"type":"function","function":{"name":n,"arguments":"{}"}}))
                .collect();
            let mut messages = vec![json!({"role":"user","content":"hi"}), json!({"role":"assistant","content":null,"tool_calls":calls})];
            for (i, (_, text)) in results.iter().enumerate() {
                messages.push(json!({"role":"tool","tool_call_id":format!("call_r{i}"),"content":text}));
            }
            ep.body(json!({"model":"gpt-4o-mini","tools":chat_tools(&["fetch"]),"messages":messages}))
        } else {
            let mut input = vec![json!({"role":"user","content":"hi"})];
            for (i, (n, text)) in results.iter().enumerate() {
                input.push(json!({"type":"function_call","id":format!("fc_r{i}"),"call_id":format!("r{i}"),"name":n,"arguments":"{}","status":"completed"}));
                input.push(json!({"type":"function_call_output","call_id":format!("r{i}"),"output":text}));
            }
            ep.body(json!({"model":"gpt-6","tools":responses_tools(&["fetch"]),"input":input}))
        }
    }

    /// The upstream copy of result `i` of a [`results_body`] request.
    fn result_slot(ep: Ep, up: &Value, i: usize) -> Value {
        if ep.chat() {
            up["messages"][2 + i]["content"].clone()
        } else {
            up["input"][2 + 2 * i]["output"].clone()
        }
    }

    /// The request of most tests: one injected result of `read_page`.
    fn injected_body(ep: Ep) -> String {
        results_body(ep, &[("read_page", &injected())])
    }

    // --- record readers --------------------------------------------------------------------------

    /// `(kind, detector)` of each finding of a record.
    fn findings(r: &DecisionRecord) -> Vec<(String, String)> {
        r.cedar.as_ref().map(|c| c.findings.iter().map(|f| (f.kind.clone(), f.detector.clone())).collect()).unwrap_or_default()
    }

    fn has(r: &DecisionRecord, kind: &str, detector: &str) -> bool {
        findings(r).iter().any(|(k, d)| k == kind && d == detector)
    }

    /// The response-side record of the call `name`.
    fn call_record(records: &[DecisionRecord], name: &str) -> DecisionRecord {
        records
            .iter()
            .find(|r| r.side == RecordSide::Response && r.tool.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no response record for {name}: {records:#?}"))
    }

    /// The records of one request action.
    fn action_records(records: &[DecisionRecord], action: RecordedAction) -> Vec<DecisionRecord> {
        records.iter().filter(|r| r.request_action == Some(action)).cloned().collect()
    }

    /// The `SCREEN_RESULT` records.
    fn screen_records(records: &[DecisionRecord]) -> Vec<DecisionRecord> {
        action_records(records, RecordedAction::ScreenResult)
    }

    /// The report entries as text: the header of a JSON answer, the comments of a stream.
    fn report_text(ex: &Ex) -> String {
        serde_json::to_string(&ex.entries).expect("json")
    }

    fn keep_alives(text: &str) -> usize {
        text.matches(": keep-alive\n\n").count()
    }

    // --- § 7.4 signals only and the starter policy ------------------------------------------------

    /// A1, D2: with the shipped floor and an allow-all tenant, a call with a secret and an
    /// injected result pass unchanged in enforce and in log-only. The screen runs (the mode counts),
    /// the decision log holds the findings, and the report holds none.
    #[tokio::test]
    async fn findings_are_signals_only() {
        let dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let secret = secret_args();
        let calls = [("fetch", secret.as_str())];
        for mode in [TenantMode::Enforce, TenantMode::LogOnly] {
            for ep in EPS {
                let s = stack(&dir, mode, Det::default());
                let body = injected_body(ep);
                let ex = s.run(ep, &body, &calls).await;
                let case = format!("{ep:?} {mode:?}");
                assert_eq!(ex.status, StatusCode::OK, "{case}: {}", ex.text);
                assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "the result goes upstream unchanged ({case})");
                assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED, "{case}");
                assert!(ex.text.contains(CANARY_SECRET_BODY), "the call goes to the client unchanged ({case})");
                assert!(request_entries(&ex.entries).is_empty(), "{case}");
                assert_eq!(s.screens(), 1, "the screen runs once ({case})");
                let records = s.log.records();
                let call = call_record(&records, "fetch");
                assert!(has(&call, "secret_in_args", "secret_in_args"), "{case}: {:?}", findings(&call));
                let screened = screen_records(&records);
                assert_eq!(screened.len(), 1, "{case}: {records:#?}");
                assert!(has(&screened[0], "injection_markers", "injection_markers"), "{case}: {:?}", findings(&screened[0]));
                let report = report_text(&ex);
                for word in ["finding", "secret_in_args", "injection_markers", "severity"] {
                    assert!(!report.contains(word), "the report holds no findings ({case}): {report}");
                }
            }
        }
    }

    /// § 6.12, SMA-752 A2: the starter policy and a `high` finding. Enforce blocks the call and
    /// replaces the result with the notice. Log-only reports `Observe` and changes no byte.
    #[tokio::test]
    async fn the_starter_policy_blocks_in_enforce_and_observes_in_log_only() {
        let dir = policy_dir(SHIPPED_FLOOR, &starter());
        let secret = secret_args();
        let calls = [("fetch", secret.as_str())];
        for ep in EPS {
            let body = injected_body(ep);
            let enforce = stack(&dir, TenantMode::Enforce, Det::default()).run(ep, &body, &calls).await;
            assert_eq!(enforce.status, StatusCode::OK, "{ep:?}: {}", enforce.text);
            assert_eq!(call_of(&enforce.entries, "fetch"), BLOCKED, "{ep:?}");
            assert!(!enforce.text.contains(CANARY_SECRET_BODY), "{ep:?}");
            assert!(is_notice(&result_slot(ep, &enforce.upstream_json(), 0)), "the result is replaced ({ep:?})");
            assert_eq!(request_entries(&enforce.entries), [req("read_page", "replace-result", true)], "{ep:?}");

            let log = stack(&dir, TenantMode::LogOnly, Det::default());
            let log_ex = log.run(ep, &body, &calls).await;
            let off = stack(&dir, TenantMode::Off, Det::default()).run(ep, &body, &calls).await;
            assert_eq!(call_of(&log_ex.entries, "fetch"), OBSERVED, "{ep:?}");
            assert_eq!(request_entries(&log_ex.entries), [req("read_page", "replace-result", false)], "{ep:?}");
            assert_eq!(log_ex.upstream.as_deref(), Some(body.as_bytes()), "log-only sends the original ({ep:?})");
            assert_eq!(off.upstream, log_ex.upstream, "{ep:?}");
            assert_eq!(without_comments(&log_ex.text), without_comments(&off.text), "{ep:?}");
            if !ep.stream() {
                assert_eq!(log_ex.sent.body, off.sent.body, "{ep:?}");
            }
            let records = log.log.records();
            assert_eq!(call_record(&records, "fetch").applied, AppliedEffect::Observe, "{ep:?}");
            assert_eq!(action_records(&records, RecordedAction::ReplaceResult).len(), 1, "{ep:?}: {records:#?}");
        }
    }

    // --- § 7.4 timeouts --------------------------------------------------------------------------

    /// § 7.4: a webhook slower than `timeout_ms` gives a `detector_error` with its name. The starter
    /// policy blocks the call in enforce; without it the call is allowed. A stream sends keep-alive
    /// comments while the hold waits on the pipeline.
    #[tokio::test]
    async fn a_slow_detector_gives_detector_error_and_keepalives() {
        let starter_dir = policy_dir(SHIPPED_FLOOR, &starter());
        let allow_dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let limits = CoreLimits {
            keepalive: Duration::from_millis(100),
            ..CoreLimits::defaults()
        };
        let mut runs = Vec::new();
        for ep in EPS {
            for (dir, want) in [(&starter_dir, BLOCKED), (&allow_dir, ALLOWED)] {
                let det = Det {
                    webhooks: vec![Arc::new(SlowDetector("hook_slow", Duration::from_secs(30)))],
                    timeout_ms: 500,
                    ..Det::default()
                };
                let s = build(dir, TenantMode::Enforce, det, limits);
                runs.push(async move {
                    let ex = s.run(ep, &ep.tools_body(&["fetch"]), &[("fetch", "{}")]).await;
                    (ep, want, ex, s.log.records())
                });
            }
        }
        for (ep, want, ex, records) in futures::future::join_all(runs).await {
            assert_eq!(ex.status, StatusCode::OK, "{ep:?}: {}", ex.text);
            assert_eq!(call_of(&ex.entries, "fetch"), want, "{ep:?}");
            let call = call_record(&records, "fetch");
            assert!(has(&call, "detector_error", "hook_slow"), "{ep:?}: {:?}", findings(&call));
            assert_eq!(call.basis, RecordBasis::Policy, "the decider decided, not on_failure ({ep:?})");
            if ep.stream() {
                assert!(keep_alives(&ex.text) >= 1, "keep-alives while the hold waits ({ep:?}): {}", ex.text);
            }
        }
    }

    // The two-hold case of § 7.4 (D35, S3) is `two_holds_with_a_slow_detector_use_the_absolute_deadline`
    // in `src/adapters/http/interception.rs`. An OpenAI chat stream carries tool calls in choice 0
    // only, and a Responses stream has one hold, so no route of the router has two holds. Only an
    // Anthropic stream has, and `tests/messages_interception.rs` covers it through `/v1/messages`.

    // --- § 7.4 errors and panics -----------------------------------------------------------------

    /// § 7.4, § 6.2.4: an erroring webhook gives a `detector_error` with its name, and a panicking
    /// built-in stage gives `detector_error` of `built_in` on both sides. The canary of the panic
    /// payload reaches no log line, no metric and no decision record. stderr is not checked: the
    /// default panic hook prints the payload (the recorded residual).
    #[tokio::test]
    async fn an_erroring_detector_and_a_panicking_stage_leak_no_canary() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (logs, _guard) = paigasus_logging::test_support::capture_logs();
        let allow_dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let starter_dir = policy_dir(SHIPPED_FLOOR, &starter());
        let mut all = Vec::new();
        for ep in EPS {
            // The erroring webhook.
            for (dir, want) in [(&allow_dir, ALLOWED), (&starter_dir, BLOCKED)] {
                let det = Det {
                    webhooks: vec![Arc::new(ErrDetector("hook_err"))],
                    ..Det::default()
                };
                let s = stack(dir, TenantMode::Enforce, det);
                let ex = s.run(ep, &ep.tools_body(&["fetch"]), &[("fetch", "{}")]).await;
                assert_eq!(call_of(&ex.entries, "fetch"), want, "{ep:?}");
                let call = call_record(&s.log.records(), "fetch");
                assert!(has(&call, "detector_error", "hook_err"), "{ep:?}: {:?}", findings(&call));
                all.extend(s.log.records());
            }
            // The panicking built-in stage, with an allow-all tenant: the call and the result each
            // get `detector_error` of `built_in`.
            let det = Det {
                built_in: Some(Arc::new(PanicStage(CANARY_PANIC))),
                ..Det::default()
            };
            let s = stack(&allow_dir, TenantMode::Enforce, det);
            let ex = s.run(ep, &results_body(ep, &[("read_page", "plain text")]), &[("fetch", "{}")]).await;
            assert_eq!(ex.status, StatusCode::OK, "{ep:?}");
            let records = s.log.records();
            assert!(has(&call_record(&records, "fetch"), "detector_error", "built_in"), "{ep:?}: {records:#?}");
            let screened = screen_records(&records);
            assert_eq!(screened.len(), 1, "{ep:?}: {records:#?}");
            assert!(has(&screened[0], "detector_error", "built_in"), "{ep:?}");
            assert_no_canary("the report", report_text(&ex).as_bytes());
            all.extend(records);
        }
        let text = logs.text();
        assert!(text.contains("a detector stage panicked") && text.contains("stage=\"built_in\""), "{text}");
        assert_no_canary("a log line", text.as_bytes());
        let mut keys = 0;
        for (key, _, _, _) in snapshotter.snapshot().into_vec() {
            keys += 1;
            let key = key.key();
            assert_no_canary("a metric name", key.name().as_bytes());
            for l in key.labels() {
                assert_no_canary("a metric label", l.value().as_bytes());
            }
        }
        assert!(keys > 0, "the metric check is not vacuous");
        for r in &all {
            assert_no_canary("a decision record", format!("{r:?}").as_bytes());
            assert_no_canary("a decision event", &proto::to_event(r).encode_to_vec());
        }
    }

    // --- § 7.4 scan limit ------------------------------------------------------------------------

    /// § 6.10, F10: with `max_scan_bytes` 65536 and three 30 KiB results, the two newest fit and are
    /// scanned, and the oldest gets `detector_error` (`scan_limit`). The newest result holds an
    /// injection with a tag run, so its `injection_markers` finding proves that it was scanned (review
    /// Minor 2). Under the starter policy the oldest and the newest results are replaced, and the
    /// middle one is not.
    #[tokio::test]
    async fn a_request_past_max_scan_bytes_gets_scan_limit_on_old_results() {
        let mut text: Vec<String> = (0..3).map(|i| format!("{i}").repeat(30 * 1024)).collect();
        // At least 4 tag characters (Task 22): `injection_markers`, high. The two newest still fit.
        text[2].push_str(&format!(" {}", tags("hidden-order")));
        assert!(text[1].len() + text[2].len() <= 65_536, "the two newest fit in the budget");
        let results = [("res_0", text[0].as_str()), ("res_1", text[1].as_str()), ("res_2", text[2].as_str())];
        let allow_dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let starter_dir = policy_dir(SHIPPED_FLOOR, &starter());
        let det = || Det {
            max_scan_bytes: 65_536,
            ..Det::default()
        };
        for ep in EPS {
            let body = results_body(ep, &results);
            let s = stack(&allow_dir, TenantMode::Enforce, det());
            let ex = s.run(ep, &body, &[]).await;
            assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{ep:?}");
            let screened = screen_records(&s.log.records());
            let mut got: Vec<(&str, Vec<(String, String)>)> = screened.iter().map(|r| (r.tool.name.as_str(), findings(r))).collect();
            got.sort();
            assert_eq!(
                got,
                [
                    ("res_0", vec![("detector_error".to_owned(), "built_in".to_owned())]),
                    ("res_2", vec![("injection_markers".to_owned(), "injection_markers".to_owned())]),
                ],
                "the oldest is not scanned, the newest is ({ep:?})"
            );

            let s = stack(&starter_dir, TenantMode::Enforce, det());
            let ex = s.run(ep, &body, &[]).await;
            let up = ex.upstream_json();
            assert!(is_notice(&result_slot(ep, &up, 0)), "the oldest result is replaced: scan_limit ({ep:?})");
            assert_eq!(result_slot(ep, &up, 1), text[1].as_str(), "{ep:?}");
            assert!(is_notice(&result_slot(ep, &up, 2)), "the newest result is replaced: its injection ({ep:?})");
            let mut entries = request_entries(&ex.entries);
            entries.sort();
            assert_eq!(entries, [req("res_0", "replace-result", true), req("res_2", "replace-result", true)], "{ep:?}");
        }
    }

    // --- § 7.4 the request-side gate and `enabled = false` ----------------------------------------

    /// § 6.8.0: mode `off` with the shipped floor runs no screen, so no `unseen_host` appears on the
    /// response side (the control: log-only screens and finds it). A test floor that reads
    /// `max_severity` makes the screen run in mode `off` and blocks a tag-smuggling result.
    #[tokio::test]
    async fn mode_off_screens_nothing_and_a_screen_dependent_floor_does() {
        let shipped = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        // A test fixture only: the shipped floor never reads `max_severity` (N2).
        let floor = format!("// SPDX-License-Identifier: Apache-2.0\n{}\n", fixture_policy("detectors-block-high"));
        let severity_floor = policy_dir(&floor, PERMIT_ALL);
        let calls = [("fetch", r#"{"url":"https://other.example/x"}"#)];
        for ep in EPS {
            let body = results_body(ep, &[("read_page", "see https://seen.example/")]);
            let off = stack(&shipped, TenantMode::Off, Det::default());
            let ex = off.run(ep, &body, &calls).await;
            assert_eq!(ex.status, StatusCode::OK, "{ep:?}");
            assert_eq!(off.screens(), 0, "mode off screens nothing ({ep:?})");
            assert!(!has(&call_record(&off.log.records(), "fetch"), "unseen_host", "unseen_host"), "{ep:?}");
            let log_only = stack(&shipped, TenantMode::LogOnly, Det::default());
            log_only.run(ep, &body, &calls).await;
            assert_eq!(log_only.screens(), 1, "{ep:?}");
            assert!(has(&call_record(&log_only.log.records(), "fetch"), "unseen_host", "unseen_host"), "the control ({ep:?})");

            let gated = stack(&severity_floor, TenantMode::Off, Det::default());
            let ex = gated.run(ep, &injected_body(ep), &[]).await;
            assert_eq!(gated.screens(), 1, "the floor reads max_severity, so the screen runs ({ep:?})");
            assert!(is_notice(&result_slot(ep, &ex.upstream_json(), 0)), "the floor blocks in mode off ({ep:?})");
            assert_eq!(request_entries(&ex.entries), [req("read_page", "replace-result", true)], "{ep:?}");
        }
    }

    /// A gateway config from TOML text, as `GatewayConfig::figment` reads it. The large `Err` is
    /// `figment::Error`, the type that `Jail::expect_with` fixes (as in `decision_log_e2e.rs`).
    #[allow(clippy::result_large_err)]
    fn config(toml: &str) -> Result<GatewayConfig, String> {
        let mut out = None;
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", toml)?;
            out = Some(GatewayConfig::figment().extract::<GatewayConfig>().map_err(|e| e.to_string()));
            Ok(())
        });
        out.expect("a result")
    }

    /// A config with the tool policy on `dir` in enforce mode, and this `[tool_policy.detectors]` body.
    fn detectors_config(dir: &TempDir, decide_secs: u64, detectors: &str) -> Result<GatewayConfig, String> {
        config(&format!(
            "[upstream.openai]\napi_key = \"sk-test-key\"\n[tool_interception]\nenabled = true\ndecide_timeout_secs = {decide_secs}\n\
             [tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"{}\" }}\n\
             [tool_policy.detectors]\n{detectors}\n",
            dir.path().display()
        ))
    }

    /// § 6.10: `enabled = false` through the composition root. No pipeline, no finding of a
    /// detector, `max_severity` is -1, and the starter policy on `max_severity` does not fire. The
    /// switch alone stops the screen, also with a pipeline in place.
    #[tokio::test]
    async fn detectors_off_never_fires_a_severity_policy() {
        let dir = policy_dir(SHIPPED_FLOOR, &starter());
        let cfg = detectors_config(&dir, 5, "enabled = false").expect("loads");
        cfg.validate().expect("validates");
        let log = Arc::new(CaptureLog::default());
        let rt = runtime::build_tool_policy(&cfg, recorder_over(&log), None).expect("builds");
        assert!(rt.detectors.is_none(), "no pipeline");
        let ti = runtime::tool_interception(&cfg, &rt);
        let tp = rt.policy.map(|(_, tp)| tp);
        let secret = secret_args();
        let calls = [("fetch", secret.as_str())];
        for ep in EPS {
            log.clear();
            let body = injected_body(ep);
            let ex = exchange_with(ep, ORG_A_PRN, ti.clone(), tp.clone(), &body, &calls).await;
            assert_eq!(ex.status, StatusCode::OK, "{ep:?}: {}", ex.text);
            assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED, "{ep:?}");
            assert_eq!(ex.upstream.as_deref(), Some(body.as_bytes()), "{ep:?}");
            assert!(request_entries(&ex.entries).is_empty(), "{ep:?}");
            let records = log.records();
            assert!(screen_records(&records).is_empty(), "{ep:?}: {records:#?}");
            let call = call_record(&records, "fetch");
            let cedar = call.cedar.as_ref().expect("a Cedar detail");
            assert_eq!(cedar.max_severity, None, "max_severity is -1 ({ep:?})");
            assert!(cedar.detection_disabled, "{ep:?}");
            assert!(findings(&call).is_empty(), "{ep:?}: {:?}", findings(&call));
        }
        // The switch alone stops the screen: a stack with a counting pipeline and `enabled = false`.
        let s = stack(&dir, TenantMode::Enforce, Det { enabled: false, ..Det::default() });
        s.run(Ep::ChatJson, &injected_body(Ep::ChatJson), &calls).await;
        assert_eq!(s.screens(), 0);
    }

    /// § 7.4 config (Task 19 has the unit cases): the gateway does not start with a `timeout_ms`
    /// inside the margin, an unknown key, or `max_scan_bytes` below 65536. The defaults pass.
    #[test]
    fn a_bad_detectors_table_fails_at_start() {
        let dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        detectors_config(&dir, 2, "").expect("loads").validate().expect("the defaults pass");
        let edge = 2000 - DETECTOR_MARGIN_MS;
        detectors_config(&dir, 2, &format!("timeout_ms = {edge}")).expect("loads").validate().expect("at the margin");
        let err = detectors_config(&dir, 2, &format!("timeout_ms = {}", edge + 1))
            .expect("loads")
            .validate()
            .expect_err("inside the margin");
        assert!(err.contains("tool_policy.detectors.timeout_ms"), "{err}");
        let err = detectors_config(&dir, 2, "timeot_ms = 100").expect_err("an unknown key");
        assert!(err.contains("timeot_ms"), "{err}");
        let err = detectors_config(&dir, 2, "max_scan_bytes = 65535").expect("loads").validate().expect_err("a small scan budget");
        assert!(err.contains("tool_policy.detectors.max_scan_bytes"), "{err}");
    }

    // --- § 7.4 no evidence leak (A2) -------------------------------------------------------------

    /// A2: one canary secret, one canary host and one canary injection phrase. After a blocked run
    /// (the starter policy, enforce) and an observed run (log-only) on every endpoint, no captured
    /// log line, report header, report comment, metric line or decision record holds a canary, and
    /// neither does the `{:?}` of each type of § 6.2.4. stderr is not checked (§ 6.2.4).
    #[tokio::test]
    async fn no_canary_reaches_any_output() {
        let metrics = paigasus_observability::init("paigasus-gateway");
        let (logs, _guard) = paigasus_logging::test_support::capture_logs();
        let starter_dir = policy_dir(SHIPPED_FLOOR, &starter());
        let allow_dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let secret = secret_args();
        let calls = [("fetch", secret.as_str())];
        let mut records = Vec::new();
        let mut blocked = 0;
        for ep in EPS {
            for (dir, mode) in [(&starter_dir, TenantMode::Enforce), (&allow_dir, TenantMode::LogOnly)] {
                let s = stack(dir, mode, Det::default());
                let ex = s.run(ep, &injected_body(ep), &calls).await;
                assert_eq!(ex.status, StatusCode::OK, "{ep:?} {mode:?}");
                if call_of(&ex.entries, "fetch") == BLOCKED {
                    blocked += 1;
                }
                for (name, value) in &ex.sent.headers {
                    assert_no_canary("a header", name.as_str().as_bytes());
                    assert_no_canary("a header", value.as_bytes());
                }
                for line in ex.text.lines().filter(|l| l.starts_with(": ")) {
                    assert_no_canary("a report comment", line.as_bytes());
                }
                let got = s.log.records();
                assert!(got.iter().any(|r| has(r, "secret_in_args", "secret_in_args")), "the check is not vacuous ({ep:?} {mode:?})");
                records.extend(got);
            }
        }
        assert_eq!(blocked, 4, "the starter policy blocked on every endpoint");
        for r in &records {
            assert_no_canary("a decision record", format!("{r:?}").as_bytes());
            assert_no_canary("a decision event", &proto::to_event(r).encode_to_vec());
        }
        let text = logs.text();
        assert!(text.contains("tool-policy decision"), "the log capture saw the decision lines: {text}");
        assert_no_canary("a log line", text.as_bytes());
        let rendered = metrics.render();
        assert!(rendered.contains(names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL), "the metric check is not vacuous");
        assert_no_canary("a metric line", rendered.as_bytes());

        // The `{:?}` of each type of § 6.2.4, built from the canaries.
        let mut debug = Vec::new();
        let input = ToolInput::Json(secret.clone());
        // No seen host: `unseen_host` fires, so an evidence entry holds the canary host.
        let none_seen = HostSet::new();
        let subject = CallSubject::new("fetch", &input, Declared::Yes).with_seen_hosts(&none_seen);
        let detection = detect_call(&subject);
        assert!(detection.findings.iter().any(|f| f.kind == "secret_in_args"), "{detection:?}");
        assert!(
            detection.findings.iter().flat_map(|f| &f.evidence).any(|e| e.host.as_deref() == Some(CANARY_HOST)),
            "the Debug check sees an evidence entry with the canary host"
        );
        assert!(detection.hosts.contains(CANARY_HOST));
        debug.push(format!("{subject:?}"));
        debug.push(format!("{detection:?}"));
        debug.push(format!("{:?}", detection.hosts));
        let marker = injection_markers(&injected(), 0).expect("an injection finding");
        for f in detection.findings.iter().chain([&marker]) {
            debug.push(format!("{f:?}"));
            for e in &f.evidence {
                debug.push(format!("{e:?}"));
                debug.push(format!("{:?}", e.location));
            }
        }
        let call = Subject::Call {
            index: 0,
            invocation: ToolInvocation::new("fetch", ToolInput::Json(secret.clone()), Declared::Yes),
            schema: None,
        };
        let result = Subject::Result {
            index: 1,
            tool: Some("read_page".to_owned()),
            text: Arc::from(injected()),
        };
        debug.push(format!("{call:?}"));
        debug.push(format!("{result:?}"));
        let batch = DetectBatch::new(DetectSide::Response, Some(org_a()), Some("req".to_owned()), vec![call, result], Some(Arc::new(detection.hosts.clone())));
        debug.push(format!("{batch:?}"));
        for e in [DetectorError::Failed, DetectorError::WrongShape, DetectorError::Timeout, DetectorError::Panicked] {
            debug.push(format!("{e:?}"));
        }
        let body = injected_body(Ep::ChatJson);
        let ToolTurn::Parsed(facts) = parse_request(Format::OpenAiChat, body.as_bytes()) else {
            panic!("a parsed turn")
        };
        let screen = screen_request(Format::OpenAiChat, body.as_bytes(), &facts, 1 << 20).expect("a screen");
        assert!(screen.result_texts.iter().flatten().any(|t| t.contains(CANARY_PHRASE)), "the screen holds the text");
        debug.push(format!("{screen:?}"));
        let mut seen = HostSet::new();
        seen.insert(CANARY_HOST);
        debug.push(format!("{:?}", SeenHosts(Arc::new(seen))));
        for d in &debug {
            assert_no_canary("a Debug output", d.as_bytes());
        }
    }

    // --- § 7.4 metrics ---------------------------------------------------------------------------

    /// One counter series: the name and the sorted labels.
    type Series = (String, Vec<(String, String)>);

    /// Every counter series of one snapshot, with its value.
    fn counters(snapshotter: &Snapshotter) -> HashMap<Series, u64> {
        let mut out = HashMap::new();
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            if let DebugValue::Counter(c) = value {
                let key = key.key();
                let mut labels: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
                labels.sort();
                out.insert((key.name().to_owned(), labels), c);
            }
        }
        out
    }

    fn finding_series(side: &str, kind: &str, detector: &str, severity: &str) -> Series {
        let mut labels: Vec<(String, String)> = [("side", side), ("kind", kind), ("detector", detector), ("severity", severity)]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        labels.sort();
        (names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL.to_owned(), labels)
    }

    /// § 6.11.3: the composition root primes every valid triple at zero on both sides. A finding
    /// increments its triple, and a webhook finding counts under `webhook`. The overhead sample of
    /// a request leaves out the webhook wait (D28).
    #[tokio::test]
    async fn every_triple_is_primed_and_the_overhead_excludes_the_webhook_wait() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
        let cfg = detectors_config(&dir, 5, "").expect("loads");
        cfg.validate().expect("validates");
        let rt = runtime::build_tool_policy(&cfg, Arc::new(DecisionRecorder::tracing()), None).expect("builds");
        let primed = counters(&snapshotter);
        for side in ["request", "response"] {
            for (kind, detector, severity) in FindingLabels::built_in().triples(side) {
                assert_eq!(primed.get(&finding_series(side, kind, detector, severity)), Some(&0), "{side}/{kind}/{detector}/{severity}");
            }
        }
        // The built-in findings of one request, through the composition root.
        let ti = runtime::tool_interception(&cfg, &rt);
        let tp = rt.policy.map(|(_, tp)| tp);
        let secret = secret_args();
        let ex = exchange_with(Ep::ChatJson, ORG_A_PRN, ti, tp, &injected_body(Ep::ChatJson), &[("fetch", secret.as_str())]).await;
        assert_eq!(ex.status, StatusCode::OK);
        let after = counters(&snapshotter);
        assert_eq!(after.get(&finding_series("response", "secret_in_args", "secret_in_args", "high")), Some(&1), "{after:?}");
        assert_eq!(after.get(&finding_series("request", "injection_markers", "injection_markers", "high")), Some(&1), "{after:?}");

        // A webhook finding counts under `webhook`, whatever its detector name.
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let det = Det {
            webhooks: vec![Arc::new(ErrDetector("hook_err"))],
            ..Det::default()
        };
        let s = stack(&dir, TenantMode::Enforce, det);
        s.run(Ep::ChatJson, &Ep::ChatJson.tools_body(&["fetch"]), &[("fetch", "{}")]).await;
        let after = counters(&snapshotter);
        assert_eq!(after.get(&finding_series("response", "detector_error", "webhook", "high")), Some(&1), "{after:?}");
        // Only the failure counter carries the provider name (SMA-856 D17). No other series does.
        let failures = "gateway_tool_policy_webhook_failures_total";
        assert!(
            after.keys().filter(|(name, _)| name != failures).all(|(_, labels)| labels.iter().all(|(_, v)| v != "hook_err")),
            "a webhook name is never a label of another series: {after:?}"
        );
        assert_eq!(after.iter().filter(|((name, _), v)| name == failures && **v == 1).count(), 1, "{after:?}");

        // D28: the overhead sample leaves out the webhook wait: a webhook that takes 300 ms and finishes.
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let det = Det {
            webhooks: vec![Arc::new(SlowDetector("hook_slow", Duration::from_millis(300)))],
            timeout_ms: 4000,
            ..Det::default()
        };
        let s = stack(&dir, TenantMode::Enforce, det);
        s.run(Ep::ChatJson, &Ep::ChatJson.tools_body(&["fetch"]), &[("fetch", "{}")]).await;
        let samples: Vec<f64> = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter(|(k, _, _, _)| k.key().name() == names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS)
            .flat_map(|(_, _, _, v)| match v {
                DebugValue::Histogram(h) => h.into_iter().map(|x| x.into_inner()).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(samples.len(), 1, "{samples:?}");
        assert!(samples[0] < 0.3, "the overhead leaves out the 300 ms of the webhook: {samples:?}");
    }

    // --- test-only detectors (P-2: integration tests cannot see `#[cfg(test)]` modules) ----------

    /// The built-in detectors, counting the request screens (F9).
    struct CountingStage {
        inner: BuiltInDetector,
        requests: Arc<AtomicUsize>,
    }

    impl BuiltInStage for CountingStage {
        fn run(&self, batch: &DetectBatch) -> PipelineOutput {
            if batch.side == DetectSide::Request {
                self.requests.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.run(batch)
        }
    }

    /// A webhook that waits, then finds nothing.
    struct SlowDetector(&'static str, Duration);

    #[async_trait::async_trait]
    impl Detector for SlowDetector {
        fn name(&self) -> &str {
            self.0
        }

        async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError> {
            tokio::time::sleep(self.1).await;
            Ok(vec![Vec::new(); batch.subjects.len()])
        }
    }

    /// A webhook that always fails.
    struct ErrDetector(&'static str);

    #[async_trait::async_trait]
    impl Detector for ErrDetector {
        fn name(&self) -> &str {
            self.0
        }

        async fn detect(&self, _batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError> {
            Err(DetectorError::Failed)
        }
    }

    /// A built-in stage that panics with a canary payload, as a dependency could.
    struct PanicStage(&'static str);

    impl BuiltInStage for PanicStage {
        fn run(&self, _batch: &DetectBatch) -> PipelineOutput {
            std::panic::panic_any(self.0)
        }
    }

    // --- SMA-856 § 7.3: the webhook detector against a mock provider -------------------------------

    mod webhooks {
        use std::time::Instant;

        use paigasus_kernel::detect::Severity;
        use support::webhook_mock::{MockProvider, Reply};
        use tokio::sync::Barrier;

        use super::*;

        /// The secret of every mock provider. It holds a canary, and it is 32 bytes or more.
        const SECRET: &str = "paigasus-webhook-secret-canary-7f3a-0001";
        const SECRET_CANARY: &str = "secret-canary-7f3a";
        const ARGS_CANARY: &str = "args-canary-5c2e";
        const BODY_CANARY: &str = "body-canary-91d0";
        /// The tenant forbid that reads the kind of the provider.
        const NO_EMAIL: &str = r#"@id("no-email") forbid (principal, action, resource) when { context.findings.contains("pii_email") };"#;
        const EMPTY: &str = r#"{"findings":[]}"#;
        const EMAIL_MEDIUM: &str = r#"{"findings":[{"kind":"pii_email","severity":"medium","version":"v7"}]}"#;

        /// The starter policy and `no-email`.
        fn tenant() -> String {
            format!("{}\n{NO_EMAIL}", starter())
        }

        /// One `[[tool_policy.detectors.webhook]]` table.
        struct Prov {
            name: &'static str,
            url: String,
            kinds: &'static str,
            /// More keys of the table, one per line.
            extra: &'static str,
        }

        impl Prov {
            fn new(name: &'static str, url: &str) -> Self {
                Prov {
                    name,
                    url: url.to_owned(),
                    kinds: "\"pii_email\"",
                    extra: "",
                }
            }

            fn env(&self) -> String {
                format!("{}_SECRET", self.name.to_uppercase())
            }
        }

        /// The gateway config of one test.
        struct Wh {
            providers: Vec<Prov>,
            /// The opt-in list of `ORG_A`.
            opt_in: Vec<&'static str>,
            org_mode: &'static str,
            decide_secs: u64,
            timeout_ms: u64,
            keepalive_secs: u64,
        }

        impl Wh {
            /// One provider `acme_dlp` that `ORG_A` opts in to. The timing rule of § 7.
            fn one(url: &str) -> Self {
                Wh {
                    providers: vec![Prov::new("acme_dlp", url)],
                    opt_in: vec!["acme_dlp"],
                    org_mode: "enforce",
                    decide_secs: 5,
                    timeout_ms: 2000,
                    keepalive_secs: 15,
                }
            }

            fn with_extra(mut self, extra: &'static str) -> Self {
                self.providers[0].extra = extra;
                self
            }

            fn toml(&self, dir: &TempDir) -> String {
                let mut t = format!(
                    "[upstream.openai]\napi_key = \"sk-test-key\"\n[tool_interception]\nenabled = true\ndecide_timeout_secs = {}\nkeepalive_secs = {}\n\
                     [tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"enforce\"\nsource = {{ kind = \"file\", dir = \"{}\" }}\n\
                     [tool_policy.detectors]\ntimeout_ms = {}\n",
                    self.decide_secs,
                    self.keepalive_secs,
                    dir.path().display(),
                    self.timeout_ms
                );
                for p in &self.providers {
                    t.push_str(&format!(
                        "[[tool_policy.detectors.webhook]]\nname = \"{}\"\nurl = \"{}\"\norgs = [\"{ORG_A}\"]\nsecret_env = \"{}\"\nkinds = [{}]\n{}\n",
                        p.name,
                        p.url,
                        p.env(),
                        p.kinds,
                        p.extra
                    ));
                }
                let opt_in: Vec<String> = self.opt_in.iter().map(|n| format!("\"{n}\"")).collect();
                t.push_str(&format!("[[tool_policy.org]]\nid = \"{ORG_A}\"\nmode = \"{}\"\nwebhooks = [{}]\n", self.org_mode, opt_in.join(", ")));
                t
            }

            fn env(&self) -> HashMap<String, String> {
                self.providers.iter().map(|p| (p.env(), SECRET.to_owned())).collect()
            }

            fn config(&self, dir: &TempDir) -> GatewayConfig {
                let cfg = config(&self.toml(dir)).expect("loads");
                cfg.validate().expect("validates");
                cfg
            }

            /// The gateway as `main` builds it: `runtime::build_tool_policy_with_env` over the file
            /// policy directory, with one recorder over a capture log.
            fn boot(&self, dir: &TempDir) -> Stack {
                let cfg = self.config(dir);
                let log = Arc::new(CaptureLog::default());
                let rt = runtime::build_tool_policy_with_env(&cfg, recorder_over(&log), None, &self.env()).expect("builds");
                Stack {
                    ti: runtime::tool_interception(&cfg, &rt),
                    tp: rt.policy.map(|(_, tp)| tp),
                    log,
                    screens: Arc::default(),
                }
            }
        }

        /// A request with the function tool `fetch` and a model answer that calls `fetch` with `args`.
        async fn fetch(s: &Stack, ep: Ep, args: &str) -> Ex {
            s.run(ep, &ep.tools_body(&["fetch"]), &[("fetch", args)]).await
        }

        /// The finding of `detector` and `kind` in a record.
        fn finding(r: &DecisionRecord, detector: &str, kind: &str) -> Option<paigasus_gateway_core::decision_log::FindingRecord> {
            r.cedar.as_ref()?.findings.iter().find(|f| f.detector == detector && f.kind == kind).cloned()
        }

        /// `(severity, version)` of the finding of `detector` and `kind` in a record.
        fn sev_ver(r: &DecisionRecord, detector: &str, kind: &str) -> (Option<Severity>, String) {
            let f = finding(r, detector, kind).unwrap_or_else(|| panic!("no {kind} finding of {detector}: {:?}", findings(r)));
            (f.severity, f.version)
        }

        fn failure_series(provider: &str, reason: &str) -> Series {
            (
                names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL.to_owned(),
                vec![("provider".to_owned(), provider.to_owned()), ("reason".to_owned(), reason.to_owned())],
            )
        }

        /// A closed loopback port.
        fn closed_url() -> String {
            let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
            format!("http://127.0.0.1:{port}/hook")
        }

        // --- the provider answers ----------------------------------------------------------------

        /// The provider answers `pii_email`: the call is blocked by the tenant forbid. The record
        /// holds the finding with its detector, kind and severity. The mock saw a request with a
        /// good MAC, the principal, no session and the built-in findings. The counter counts it under
        /// `webhook`.
        #[tokio::test]
        async fn a_good_provider_denies_by_its_finding() {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _local = metrics::set_default_local_recorder(&recorder);
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMAIL_MEDIUM));
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let s = Wh::one(&mock.url).boot(&dir);
            // `send_mail` is not declared: the built-in `undeclared_tool` (medium) reaches the provider.
            let ex = s.run(Ep::ChatJson, &Ep::ChatJson.tools_body(&["fetch"]), &[("send_mail", r#"{"to":"a@b.example"}"#)]).await;
            assert_eq!(ex.status, StatusCode::OK, "{}", ex.text);
            assert_eq!(call_of(&ex.entries, "send_mail"), BLOCKED);

            let call = call_record(&s.log.records(), "send_mail");
            assert_eq!(sev_ver(&call, "acme_dlp", "pii_email"), (Some(Severity::Medium), "v7".to_owned()));
            let seen = mock.requests();
            assert_eq!(seen.len(), 1);
            assert!(seen[0].signature_ok, "the request MAC verifies");
            assert_eq!(seen[0].body["subject"], PRINCIPAL_1);
            assert!(seen[0].body["session"].is_null(), "the stack keeps no session: {}", seen[0].body);
            assert_eq!(seen[0].body["tool"]["name"], "send_mail");
            let built_in: Vec<&str> = seen[0].body["findings"].as_array().expect("findings").iter().filter_map(|f| f["kind"].as_str()).collect();
            assert!(built_in.contains(&"undeclared_tool"), "the provider sees the built-in findings: {}", seen[0].body);

            // The finding counts under `webhook`, whatever the provider name. Parse the number.
            let after = counters(&snapshotter);
            assert_eq!(after.get(&finding_series("response", "pii_email", "webhook", "medium")), Some(&1), "{after:?}");
            assert!(
                after
                    .keys()
                    .filter(|(n, _)| n != names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL)
                    .all(|(_, l)| l.iter().all(|(_, v)| v != "acme_dlp"))
            );
        }

        /// The negative control of the test above: the same stack and policy, and an empty answer.
        #[tokio::test]
        async fn negative_control_an_empty_answer_passes() {
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMPTY));
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let s = Wh::one(&mock.url).boot(&dir);
            let ex = s.run(Ep::ChatJson, &Ep::ChatJson.tools_body(&["fetch"]), &[("send_mail", r#"{"to":"a@b.example"}"#)]).await;
            assert_eq!(call_of(&ex.entries, "send_mail"), ALLOWED);
            assert_eq!(mock.requests().len(), 1, "the provider was asked");
            let call = call_record(&s.log.records(), "send_mail");
            assert!(!findings(&call).iter().any(|(_, d)| d == "acme_dlp"), "{:?}", findings(&call));
        }

        // --- the failures ------------------------------------------------------------------------

        /// A bad signature gives a `detector_error` of the provider with the version `signature`. With
        /// the failure severity `high` the starter policy blocks the call.
        #[tokio::test]
        async fn a_bad_signature_gives_detector_error_signature() {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _local = metrics::set_default_local_recorder(&recorder);
            // The finding in the answer must not count: only the signature is wrong.
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMAIL_MEDIUM).signed_with("paigasus-webhook-WRONG-secret-0000000001"));
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let s = Wh::one(&mock.url).with_extra("failure_severity = \"high\"").boot(&dir);
            let ex = fetch(&s, Ep::ChatJson, "{}").await;
            assert_eq!(call_of(&ex.entries, "fetch"), BLOCKED);
            let call = call_record(&s.log.records(), "fetch");
            assert_eq!(sev_ver(&call, "acme_dlp", "detector_error"), (Some(Severity::High), "signature".to_owned()));
            assert!(finding(&call, "acme_dlp", "pii_email").is_none(), "an unsigned answer gives no finding");
            assert_eq!(mock.requests().len(), 1);
            let after = counters(&snapshotter);
            assert_eq!(after.get(&failure_series("acme_dlp", "signature")), Some(&1), "{after:?}");
        }

        /// A provider that is slower than its `timeout_ms` (300): with `failure_severity` high the
        /// call is blocked, with `low` it is allowed and the record holds the low `detector_error`
        /// with the version `timeout`.
        #[tokio::test]
        async fn a_slow_provider_and_the_failure_severity() {
            for (severity, want, sev) in [("high", BLOCKED, Severity::High), ("low", ALLOWED, Severity::Low)] {
                let recorder = DebuggingRecorder::new();
                let snapshotter = recorder.snapshotter();
                let _local = metrics::set_default_local_recorder(&recorder);
                let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMPTY).after(Duration::from_millis(1500)));
                let dir = policy_dir(SHIPPED_FLOOR, &tenant());
                let extra: &'static str = if severity == "high" {
                    "timeout_ms = 300\nfailure_severity = \"high\""
                } else {
                    "timeout_ms = 300\nfailure_severity = \"low\""
                };
                let s = Wh::one(&mock.url).with_extra(extra).boot(&dir);
                let started = Instant::now();
                let ex = fetch(&s, Ep::ChatJson, "{}").await;
                assert!(started.elapsed() < Duration::from_millis(1400), "the call does not wait for the answer: {:?}", started.elapsed());
                assert_eq!(call_of(&ex.entries, "fetch"), want, "{severity}");
                let call = call_record(&s.log.records(), "fetch");
                assert_eq!(sev_ver(&call, "acme_dlp", "detector_error"), (Some(sev), "timeout".to_owned()), "{severity}");
                let after = counters(&snapshotter);
                assert_eq!(after.get(&failure_series("acme_dlp", "timeout")), Some(&1), "{severity}: {after:?}");
            }
        }

        /// A hold that waits for a provider: the stream carries keep-alives while it waits (1 s
        /// keep-alive, an answer after 1.5 s).
        #[tokio::test]
        async fn a_provider_wait_keeps_the_stream_alive() {
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMAIL_MEDIUM).after(Duration::from_millis(1500)));
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let mut w = Wh::one(&mock.url).with_extra("timeout_ms = 3000");
            w.keepalive_secs = 1;
            w.decide_secs = 5;
            w.timeout_ms = 3500;
            let s = w.boot(&dir);
            let ex = fetch(&s, Ep::ChatSse, "{}").await;
            assert_eq!(ex.status, StatusCode::OK, "{}", ex.text);
            assert!(keep_alives(&ex.text) >= 1, "a keep-alive while the hold waits: {}", ex.text);
            assert_eq!(call_of(&ex.entries, "fetch"), BLOCKED, "the late answer still counts");
            let call = call_record(&s.log.records(), "fetch");
            assert_eq!(
                sev_ver(&call, "acme_dlp", "pii_email").0,
                Some(Severity::Medium),
                "the finding is there, not a load-induced cut: {:?}",
                findings(&call)
            );
        }

        /// R1: the end of one call does not touch the other calls of the batch. The provider answers
        /// `fetch_a` and `fetch_b` at once and holds `fetch_c` past its `timeout_ms`: the first two
        /// keep their findings and only `fetch_c` gets a failure finding.
        ///
        /// The reason is `timeout`, not `deadline` (ruling C2). The cut of the hold deadline is
        /// `hold_start + decide_timeout - margin`, and an OpenAI hold starts its decision at the
        /// moment it completes, so no route of the router has a hold that waited in a queue, and the
        /// provider timeout (at most `timeout_ms - 50`, D21) always ends first. The `deadline`
        /// reason has its own proofs in the adapter and pipeline unit tests (Task 5 and Task 8).
        #[tokio::test]
        async fn a_timeout_stops_one_call_only() {
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, body| {
                if body["tool"]["name"] == "fetch_c" {
                    Reply::signed(EMAIL_MEDIUM).after(Duration::from_secs(20))
                } else {
                    Reply::signed(EMAIL_MEDIUM)
                }
            });
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let mut w = Wh::one(&mock.url).with_extra("timeout_ms = 900");
            w.timeout_ms = 1000;
            let s = w.boot(&dir);
            let calls = [("fetch_a", "{}"), ("fetch_b", "{}"), ("fetch_c", "{}")];
            let ex = s.run(Ep::ChatJson, &Ep::ChatJson.tools_body(&["fetch_a", "fetch_b", "fetch_c"]), &calls).await;
            assert_eq!(ex.status, StatusCode::OK, "{}", ex.text);
            let records = s.log.records();
            for name in ["fetch_a", "fetch_b"] {
                let r = call_record(&records, name);
                assert_eq!(sev_ver(&r, "acme_dlp", "pii_email").0, Some(Severity::Medium), "{name} keeps its finding: {:?}", findings(&r));
                assert!(finding(&r, "acme_dlp", "detector_error").is_none(), "{name}");
            }
            let c = call_record(&records, "fetch_c");
            assert_eq!(sev_ver(&c, "acme_dlp", "detector_error"), (Some(Severity::High), "timeout".to_owned()));
            assert!(finding(&c, "acme_dlp", "pii_email").is_none());
        }

        /// Every other adapter reason reaches the record and the counter, with the provider name.
        #[tokio::test]
        async fn every_other_reason_reaches_the_record_and_the_counter() {
            let long_pad = format!(r#"{{"findings":[],"pad":"{}"}}"#, "x".repeat(70 * 1024));
            // (case, reason, answer, extra keys, requests that the mock sees)
            let cases: Vec<(&str, &str, Reply, &'static str, usize)> = vec![
                ("invalid json", "malformed", Reply::signed("this is not json"), "", 1),
                ("undeclared kind", "malformed", Reply::signed(r#"{"findings":[{"kind":"pii_iban","severity":"low"}]}"#), "", 1),
                ("status 500", "status", Reply::signed(EMPTY).with_status(500), "", 1),
                ("status 302", "status", Reply::signed(EMPTY).with_status(302), "", 1),
                ("a 70 KiB answer", "response_too_large", Reply::signed(long_pad), "", 1),
                ("a request over max_request_bytes", "request_too_large", Reply::signed(EMPTY), "max_request_bytes = 64", 0),
                ("a closed port", "transport", Reply::signed(EMPTY), "", 0),
            ];
            for (case, reason, reply, extra, want_requests) in cases {
                let recorder = DebuggingRecorder::new();
                let snapshotter = recorder.snapshotter();
                let _local = metrics::set_default_local_recorder(&recorder);
                let mock = MockProvider::spawn("acme_dlp", SECRET, move |_, _| reply.clone());
                let url = if reason == "transport" { closed_url() } else { mock.url.clone() };
                let dir = policy_dir(SHIPPED_FLOOR, &tenant());
                let s = Wh::one(&url).with_extra(extra).boot(&dir);
                let ex = fetch(&s, Ep::ChatJson, "{}").await;
                assert_eq!(ex.status, StatusCode::OK, "{case}: {}", ex.text);
                assert_eq!(call_of(&ex.entries, "fetch"), BLOCKED, "{case}: the failure is high, the starter blocks");
                let call = call_record(&s.log.records(), "fetch");
                assert_eq!(sev_ver(&call, "acme_dlp", "detector_error"), (Some(Severity::High), reason.to_owned()), "{case}");
                assert_eq!(mock.requests().len(), want_requests, "{case}");
                let after = counters(&snapshotter);
                assert_eq!(after.get(&failure_series("acme_dlp", reason)), Some(&1), "{case}: {after:?}");
                assert_eq!(after.get(&finding_series("response", "detector_error", "webhook", "high")), Some(&1), "{case}: {after:?}");
            }
        }

        /// Two providers: both requests are open at the same time (a barrier of three, with the
        /// test as the third party). The fast one adds its finding, the slow one gives its failure.
        #[tokio::test]
        async fn two_providers_one_slow() {
            let barrier = Arc::new(Barrier::new(3));
            let fast = MockProvider::spawn("fast_dlp", SECRET, |_, _| Reply::signed(r#"{"findings":[{"kind":"pii_email","severity":"low","version":"f1"}]}"#)).with_barrier(Arc::clone(&barrier));
            let slow = MockProvider::spawn("slow_dlp", SECRET, |_, _| Reply::signed(EMPTY).after(Duration::from_millis(1500))).with_barrier(Arc::clone(&barrier));
            let dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
            let mut w = Wh::one(&fast.url);
            w.providers = vec![Prov::new("fast_dlp", &fast.url), Prov::new("slow_dlp", &slow.url)];
            w.providers[0].extra = "timeout_ms = 1500";
            w.providers[1].extra = "timeout_ms = 300\nfailure_severity = \"low\"";
            w.providers[1].kinds = "\"pii_phone\"";
            w.opt_in = vec!["fast_dlp", "slow_dlp"];
            let s = w.boot(&dir);
            let (ex, ()) = tokio::join!(fetch(&s, Ep::ChatJson, "{}"), async {
                tokio::time::timeout(Duration::from_secs(5), barrier.wait()).await.expect("both requests are open at once");
            });
            assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED);
            let call = call_record(&s.log.records(), "fetch");
            assert_eq!(sev_ver(&call, "fast_dlp", "pii_email"), (Some(Severity::Low), "f1".to_owned()));
            assert_eq!(sev_ver(&call, "slow_dlp", "detector_error"), (Some(Severity::Low), "timeout".to_owned()));
            assert_eq!((fast.requests().len(), slow.requests().len()), (1, 1));
        }

        // --- who is asked ------------------------------------------------------------------------

        /// D8 and D27: no opt-in, a tenant in mode off and a tenant set that does not compile send no
        /// request. A retired provider sends none either, and gives `retired` at its failure severity.
        #[tokio::test]
        async fn the_opt_in_rules() {
            // No opt-in.
            let mock = MockProvider::spawn("acme_dlp", SECRET, |_, _| Reply::signed(EMAIL_MEDIUM));
            let dir = policy_dir(SHIPPED_FLOOR, &tenant());
            let mut w = Wh::one(&mock.url);
            w.opt_in = vec![];
            let s = w.boot(&dir);
            let ex = fetch(&s, Ep::ChatJson, "{}").await;
            assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED, "without an opt-in the provider is not asked");
            assert_eq!(mock.requests().len(), 0, "no opt-in");

            // Mode off.
            let mut w = Wh::one(&mock.url);
            w.org_mode = "off";
            let s = w.boot(&dir);
            let ex = fetch(&s, Ep::ChatJson, "{}").await;
            assert_eq!(ex.status, StatusCode::OK, "{}", ex.text);
            assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED, "mode off");
            assert_eq!(mock.requests().len(), 0, "mode off");

            // A tenant set that does not compile.
            let bad = policy_dir(SHIPPED_FLOOR, "this is not cedar");
            let s = Wh::one(&mock.url).boot(&bad);
            let ex = fetch(&s, Ep::ChatJson, "{}").await;
            assert_eq!(ex.status, StatusCode::OK, "{}", ex.text);
            assert_eq!(call_of(&ex.entries, "fetch"), UNAVAILABLE_BLOCKED, "a tenant set that does not compile");
            assert_eq!(mock.requests().len(), 0, "a tenant set that does not compile");

            // Retired: no request, and `retired` at the failure severity of the provider.
            let s = Wh::one(&mock.url).with_extra("retired = true\nfailure_severity = \"low\"").boot(&dir);
            let ex = fetch(&s, Ep::ChatJson, "{}").await;
            assert_eq!(call_of(&ex.entries, "fetch"), ALLOWED, "a low failure does not block");
            assert_eq!(mock.requests().len(), 0, "retired");
            let call = call_record(&s.log.records(), "fetch");
            assert_eq!(sev_ver(&call, "acme_dlp", "detector_error"), (Some(Severity::Low), "retired".to_owned()));
        }

        // --- no leaks and priming ----------------------------------------------------------------

        /// A4: the secret, a canary in the arguments, a canary in the answer and the URL path appear
        /// in no log line, no decision record, no proto event, no metric label and no config dump.
        #[tokio::test]
        async fn no_canary_leaks() {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _local = metrics::set_default_local_recorder(&recorder);
            let (logs, _guard) = paigasus_logging::test_support::capture_logs();
            let canaries = [SECRET_CANARY, SECRET, ARGS_CANARY, BODY_CANARY, "/hook"];
            let leak = |what: &str, bytes: &[u8]| {
                for c in canaries {
                    assert!(!bytes.windows(c.len()).any(|w| w == c.as_bytes()), "{what} holds the canary {c:?}");
                }
            };
            let args = json!({"note": ARGS_CANARY}).to_string();
            let mut all = Vec::new();
            for answer in [
                // A good answer with an extra field, and an answer that is not JSON. Both hold the canary.
                format!(r#"{{"findings":[{{"kind":"pii_email","severity":"low","version":"v1"}}],"note":"{BODY_CANARY}"}}"#),
                format!("{BODY_CANARY} is not json"),
            ] {
                let mock = MockProvider::spawn("acme_dlp", SECRET, move |_, _| Reply::signed(answer.clone()));
                let dir = policy_dir(SHIPPED_FLOOR, &tenant());
                let w = Wh::one(&mock.url);
                let cfg = w.config(&dir);
                leak("the config JSON", serde_json::to_string(&cfg).expect("json").as_bytes());
                leak("the config Debug", format!("{cfg:?}").as_bytes());
                let s = w.boot(&dir);
                let ex = fetch(&s, Ep::ChatJson, &args).await;
                assert_eq!(mock.requests().len(), 1);
                leak("the report", report_text(&ex).as_bytes());
                leak("the client answer", ex.text.as_bytes());
                all.extend(s.log.records());
            }
            assert!(!all.is_empty());
            for r in &all {
                leak("a decision record", format!("{r:?}").as_bytes());
                leak("a decision event", &proto::to_event(r).encode_to_vec());
            }
            leak("a log line", logs.text().as_bytes());
            let mut keys = 0;
            for (key, _, _, _) in snapshotter.snapshot().into_vec() {
                keys += 1;
                let key = key.key();
                leak("a metric name", key.name().as_bytes());
                for l in key.labels() {
                    leak("a metric label", l.value().as_bytes());
                }
            }
            assert!(keys > 0, "the metric check is not vacuous");
        }

        /// P5: every provider and every adapter reason reads 0 after the boot, before a request.
        #[tokio::test]
        async fn the_failure_counter_is_primed_before_the_first_request() {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _local = metrics::set_default_local_recorder(&recorder);
            let dir = policy_dir(SHIPPED_FLOOR, PERMIT_ALL);
            let mut w = Wh::one("http://127.0.0.1:9/hook");
            w.providers.push(Prov::new("other_dlp", "http://127.0.0.1:9/other"));
            let _s = w.boot(&dir);
            let primed = counters(&snapshotter);
            let reasons = [
                "request_too_large",
                "overloaded",
                "transport",
                "timeout",
                "deadline",
                "status",
                "response_too_large",
                "signature",
                "malformed",
            ];
            for provider in ["acme_dlp", "other_dlp"] {
                for reason in reasons {
                    assert_eq!(primed.get(&failure_series(provider, reason)), Some(&0), "{provider}/{reason}");
                }
            }
        }
    }
}

// --- SMA-750: the Anthropic route ------------------------------------------------------------

/// SMA-750 D9, § 6.5: the request policy applies through `POST /v1/messages`. The denied definition
/// leaves the upstream body, the request entry is the first frame of the stream, and the denied
/// call is blocked and never reaches the client.
#[tokio::test]
async fn the_anthropic_route_applies_the_request_policy() {
    use paigasus_gateway::adapters::anthropic::AnthropicClient;
    use paigasus_gateway::adapters::http::MessagesSettings;
    use paigasus_gateway::config::AnthropicConfig;

    let dir = policy_dir(FLOOR, TENANT);
    let record = |v: Value| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().expect("a type"));
    let mut records = vec![record(
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[],"usage":{"input_tokens":3,"output_tokens":1}}}),
    )];
    for (i, (name, input)) in CALLS.iter().enumerate() {
        records.push(record(
            json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("toolu_{i}"),"name":name,"input":{}}}),
        ));
        records.push(record(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":input}})));
        records.push(record(json!({"type":"content_block_stop","index":i})));
    }
    records.push(record(
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":5}}),
    ));
    records.push(record(json!({"type":"message_stop"})));
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let cfg = AnthropicConfig {
        base_url: mock.base_url.clone(),
        api_key: secrecy::SecretString::from("operator-anthropic-test-key".to_owned()),
        extra_ca_bundle_path: None,
        default_version: "2023-06-01".to_owned(),
        non_stream_timeout_secs: 600,
    };
    let app = router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(&mock.base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: true },
        limits: None,
        dpop_enabled: false,
        tool_interception: cedar(),
        tool_policy: tool_policy(&dir, TenantMode::Enforce),
        responses: None,
        anthropic: Some(Arc::new(AnthropicClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(300)).expect("client builds"))),
        messages: Some(Arc::new(MessagesSettings {
            max_request_bytes: 32 * MIB,
            max_record_bytes: 8 * MIB,
            max_held_bytes: 32 * MIB,
            max_hold: Duration::from_secs(600),
        })),
    });
    let tools: Vec<Value> = ["read_file", "blocked_rm"]
        .iter()
        .map(|n| json!({"name":n,"description":"d","input_schema":{"type":"object"}}))
        .collect();
    let body = json!({"model":"claude-x","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}],"tools":tools}).to_string();
    let sent = send_to(&app, "/v1/messages", &body).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#), "{text}");
    let first = text.split("\n\n").next().expect("a first frame");
    let entry: Value = serde_json::from_str(first.strip_prefix(": paigasus-decision ").expect("a decision comment first")).expect("json");
    assert_eq!(
        (entry["side"].as_str(), entry["tool"].as_str(), entry["action"].as_str(), entry["enforced"].as_bool()),
        (Some("request"), Some("blocked_rm"), Some("remove-definition"), Some(true)),
        "{entry}"
    );
    let up: Value = serde_json::from_slice(&mock.recorded().expect("recorded").body).expect("json");
    let names: Vec<&str> = up["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().expect("name")).collect();
    assert_eq!(names, ["read_file"], "the denied definition is not sent upstream");
    assert_decided_with_the_snapshot(&comments(&text));
}
