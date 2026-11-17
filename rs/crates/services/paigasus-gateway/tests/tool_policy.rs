// SPDX-License-Identifier: Apache-2.0
//! SMA-752 § 6.5, § 7.2, D17, D19, D30, D33, D34: the tool policy through the real router, the
//! Cedar decider and a file policy source, against the mock upstream.
//!
//! A test that names no endpoint runs for each endpoint of [`EPS`]: Chat non-stream, Chat stream,
//! Responses non-stream and Responses stream. The Anthropic endpoint does not exist in the gateway
//! yet (SMA-750), so it has no test here.

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
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, Lookup, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
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
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings));
    Some(Arc::new(RequestToolPolicy::new(cache, settings)))
}

/// `tp_with`, with the floor loaded into the cache before the first request. A request then
/// spends its `load_timeout` on the tenant load only.
async fn tp_preloaded(source: Arc<dyn PolicySource>, settings: ToolPolicySettings) -> Option<Arc<RequestToolPolicy>> {
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&settings));
    assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Ready(_)), "the floor loads");
    Some(Arc::new(RequestToolPolicy::new(cache, settings)))
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
    }))
}

fn cedar() -> Option<Arc<ToolInterception>> {
    ti(Arc::new(CedarDecider))
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
    let mock = ep.mock(calls).await;
    let app = app_for(scope, &mock.base_url, None, cedar(), tp);
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
            (Arc::new(CedarDecider) as Arc<dyn ToolCallDecider>, tool_policy(&dir, TenantMode::Off)),
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
