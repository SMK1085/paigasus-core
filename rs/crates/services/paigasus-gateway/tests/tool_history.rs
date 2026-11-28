// SPDX-License-Identifier: Apache-2.0
//! SMA-753 PR 1: session linking end to end through the real HTTP surface. Each test sends one
//! request, reads the answer, builds the next request from the answer as a client does, and reads
//! the memory store, the decision records and the metrics.
//!
//! The Anthropic Messages route (`/v1/messages`, SMA-750) has its own test at the end of this file.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::history::HmacHistoryDigester;
use paigasus_gateway::adapters::history::memory::MemorySessionStore;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::decision_log::DecisionRecorder;
use paigasus_gateway::application::tool_history::ToolHistory;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
use paigasus_gateway::domain::decision_log::{DecisionLog, OsEntropy};
use paigasus_gateway::domain::limits::Clock;
use paigasus_gateway::domain::tool_history::{EventKind, EventVerdict, Scope, SessionStore, StoredEvent};
use paigasus_gateway::domain::tool_policy::PolicySource;
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::decision_log::{DecisionRecord, Side};
use paigasus_gateway_core::{Limits as CoreLimits, OnUnavailable, TenantMode};
use paigasus_observability::names;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::*;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const MIB: usize = 1 << 20;
/// The floor of these tests: it forbids a call of the tool `rm` in every mode.
const FLOOR: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"rm\", resource);\n";
/// The tenant set: permit every IAM principal.
const TENANT: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource);"#;

// --- the rig ---------------------------------------------------------------------------------

/// A clock that moves 10 ms at each read, so two sessions never share a millisecond.
struct StepClock(AtomicU64);

impl Clock for StepClock {
    fn now(&self) -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(1_790_000_000_000 + self.0.fetch_add(10, Ordering::SeqCst))
    }
}

/// A decision log that keeps every record.
#[derive(Default)]
struct Records(Mutex<Vec<DecisionRecord>>);

impl DecisionLog for Records {
    fn wants_digest(&self) -> bool {
        false
    }

    fn record(&self, record: DecisionRecord) {
        self.0.lock().expect("records lock").push(record);
    }
}

impl Records {
    fn all(&self) -> Vec<DecisionRecord> {
        self.0.lock().expect("records lock").clone()
    }

    /// The response-side records of the tool `tool`.
    fn calls_of(&self, tool: &str) -> Vec<DecisionRecord> {
        self.all().into_iter().filter(|r| r.tool.name == tool && r.side == Side::Response).collect()
    }
}

/// The shared parts: one store, one history, one recorder and one policy source for all requests.
struct Rig {
    store: Arc<MemorySessionStore>,
    history: Arc<ToolHistory>,
    records: Arc<Records>,
    tp: Arc<RequestToolPolicy>,
    ti: Arc<ToolInterception>,
    _dir: TempDir,
}

fn settings() -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode: TenantMode::Enforce,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(60),
        orgs: HashMap::new(),
        settings_from_source: false,
    }
}

/// A store over the memory store of a rig.
type StoreOver = fn(Arc<MemorySessionStore>) -> Arc<dyn SessionStore>;

/// What a rig can change (SMA-753 PR 2). The default is the PR 1 rig.
struct Options {
    /// The policy source. `None`: the file source with `FLOOR` and `TENANT`.
    source: Option<Arc<dyn PolicySource>>,
    /// The store that the history uses, over the memory store of the rig. `None`: the memory store.
    store: Option<StoreOver>,
    mode: TenantMode,
    limits: CoreLimits,
    /// The decider waits past the decide timeout after the reserve.
    slow_after_reserve: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            source: None,
            store: None,
            mode: TenantMode::Enforce,
            limits: CoreLimits::defaults(),
            slow_after_reserve: false,
        }
    }
}

fn rig() -> Rig {
    rig_with(Options::default())
}

fn rig_with(o: Options) -> Rig {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("floor.cedar"), FLOOR).expect("floor");
    let tenant = dir.path().join("tenants").join(ORG_A);
    std::fs::create_dir_all(&tenant).expect("tenant dir");
    std::fs::write(tenant.join("policies.cedar"), TENANT).expect("tenant");

    let store = Arc::new(MemorySessionStore::new());
    let used: Arc<dyn SessionStore> = match o.store {
        Some(wrap) => wrap(Arc::clone(&store)),
        None => store.clone(),
    };
    let clock: Arc<dyn Clock> = Arc::new(StepClock(AtomicU64::new(0)));
    let history = Arc::new(ToolHistory::new(
        used,
        Arc::clone(&clock),
        Arc::new(OsEntropy),
        Duration::from_secs(5),
        Arc::new(paigasus_gateway::adapters::history::HmacHistoryDigester::keyless()),
    ));
    let records = Arc::new(Records::default());
    let recorder = Arc::new(DecisionRecorder::new(records.clone(), clock, Arc::new(OsEntropy), None));

    let s = ToolPolicySettings { default_mode: o.mode, ..settings() };
    let source = o.source.unwrap_or_else(|| Arc::new(FilePolicySource::new(dir.path())));
    let cache = PolicyCache::new(source, CacheSettings::from_settings(&s), Arc::new(HmacHistoryDigester::keyless()));
    let tp = Arc::new(RequestToolPolicy::new(cache, s, Arc::clone(&recorder)).with_history(Arc::clone(&history)));
    let mut decider = CedarDecider::new(Arc::clone(&recorder)).with_history(Arc::clone(&history));
    if o.slow_after_reserve {
        decider = slow_pipeline(decider);
    }
    let ti = Arc::new(ToolInterception {
        limits: o.limits,
        decider: Arc::new(decider),
        recorder: Some(Arc::clone(&recorder)),
        history: Some(Arc::clone(&history)),
    });
    Rig {
        store,
        history,
        records,
        tp,
        ti,
        _dir: dir,
    }
}

impl Rig {
    fn app(&self, principal: &str, base_url: &str) -> Router {
        router(AppState {
            iam: ScopedIam::arc(principal, ORG_A_PRN),
            openai: Arc::new(openai(base_url, Duration::from_secs(30))),
            max_request_bytes: MIB,
            capabilities: Capabilities { chat_stream: true },
            limits: None,
            dpop_enabled: false,
            tool_interception: Some(Arc::clone(&self.ti)),
            tool_policy: Some(Arc::clone(&self.tp)),
            responses: Some(Arc::new(ResponsesSettings {
                max_request_bytes: 16 * MIB,
                max_record_bytes: 16 * MIB,
                max_held_bytes: 32 * MIB,
            })),
            anthropic: None,
            messages: None,
        })
    }

    /// Send `body` to `path` as `principal`, against an upstream that answers `answer`, then wait
    /// for the settle and the append tasks of the request.
    async fn send(&self, principal: &str, path: &str, body: &Value, answer: Answer) -> Sent {
        let mock = answer.spawn().await;
        let app = self.app(principal, &mock.base_url);
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
            .body(Body::from(body.to_string()))
            .expect("build request");
        let resp = app.oneshot(req).await.expect("the router answers");
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
        self.history.tasks().close();
        self.history.tasks().wait().await;
        // A closed tracker refuses the settle of the next request.
        self.history.tasks().reopen();
        Sent { status, headers, body }
    }

    /// The events of one tool in one session.
    fn events(&self, session: &str, tool: &str) -> Vec<StoredEvent> {
        self.store.events(org(), Scope::Session, session, tool)
    }
}

fn org() -> Uuid {
    Uuid::try_parse(ORG_A).expect("a fixture uuid")
}

// --- upstream answers ------------------------------------------------------------------------

/// What the mock upstream answers.
enum Answer {
    ChatJson(String),
    ChatSse(Vec<String>),
    ResponsesJson(String),
}

impl Answer {
    async fn spawn(self) -> MockOpenAi {
        match self {
            Answer::ChatJson(b) | Answer::ResponsesJson(b) => MockOpenAi::spawn_json(StatusCode::OK, b).await,
            Answer::ChatSse(c) => MockOpenAi::spawn_raw_sse(c).await,
        }
    }
}

const CHAT: &str = "/v1/chat/completions";
const RESPONSES: &str = "/v1/responses";

/// A chat answer that calls `calls`. The call ids are `<prefix>_<i>`.
fn chat_json(prefix: &str, calls: &[(&str, &str)]) -> Answer {
    let tool_calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (n, a))| json!({"id":format!("{prefix}_{i}"),"type":"function","function":{"name":n,"arguments":a}}))
        .collect();
    Answer::ChatJson(
        json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":tool_calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})
        .to_string(),
    )
}

/// A chat stream that calls `calls`. The call ids are `<prefix>_<i>`.
fn chat_sse(prefix: &str, calls: &[(&str, &str)]) -> Answer {
    let frame = |v: Value| format!("data: {v}\n\n");
    let mut out = vec![chunk("Sure.", true)];
    for (i, (name, args)) in calls.iter().enumerate() {
        let e = json!({"index": i, "id": format!("{prefix}_{i}"), "type": "function", "function": {"name": name, "arguments": args}});
        out.push(frame(
            json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"tool_calls":[e]},"finish_reason":null}]}),
        ));
    }
    out.push(frame(
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
    ));
    out.push(usage_chunk(11));
    out.push("data: [DONE]\n\n".to_owned());
    Answer::ChatSse(out)
}

/// A Responses answer with these output items.
fn responses_json(output: Vec<Value>) -> Answer {
    Answer::ResponsesJson(json!({"id":"resp_1","object":"response","status":"completed","output":output,"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}).to_string())
}

/// A Responses `function_call` item.
fn fn_item(prefix: &str, i: usize, name: &str, args: &str) -> Value {
    json!({"type":"function_call","id":format!("{prefix}_fc{i}"),"call_id":format!("{prefix}_{i}"),"name":name,"arguments":args,"status":"completed"})
}

// --- requests built from answers -------------------------------------------------------------

fn chat_tools(names: &[&str]) -> Value {
    names.iter().map(|n| json!({"type":"function","function":{"name":n,"parameters":{}}})).collect()
}

fn responses_tools(names: &[&str]) -> Value {
    names.iter().map(|n| json!({"type":"function","name":n,"parameters":{}})).collect()
}

fn chat_first(tools: &[&str], stream: bool) -> Value {
    json!({"model":"gpt-4o-mini","stream":stream,"messages":[{"role":"user","content":"hi"}],"tools":chat_tools(tools)})
}

fn responses_first(tools: &[&str]) -> Value {
    json!({"model":"gpt-6","input":"hi","tools":responses_tools(tools)})
}

/// The tool calls of a chat answer, whole or streamed: `(id, name, arguments)`.
fn chat_calls(sent: &Sent) -> Vec<(String, String, String)> {
    let text = sent.text();
    let mut out: Vec<(String, String, String)> = Vec::new();
    let tool_calls: Vec<Value> = if let Ok(v) = serde_json::from_str::<Value>(&text) {
        v["choices"][0]["message"]["tool_calls"].as_array().cloned().unwrap_or_default()
    } else {
        text.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter_map(|j| serde_json::from_str::<Value>(j).ok())
            .flat_map(|v| v["choices"][0]["delta"]["tool_calls"].as_array().cloned().unwrap_or_default())
            .collect()
    };
    for c in tool_calls {
        out.push((
            c["id"].as_str().expect("id").to_owned(),
            c["function"]["name"].as_str().expect("name").to_owned(),
            c["function"]["arguments"].as_str().unwrap_or("{}").to_owned(),
        ));
    }
    out
}

/// The client's next chat request: the calls of the answer, then their results.
fn chat_second(calls: &[(String, String, String)], tools: &[&str]) -> Value {
    let tool_calls: Vec<Value> = calls.iter().map(|(id, n, a)| json!({"id":id,"type":"function","function":{"name":n,"arguments":a}})).collect();
    let mut messages = vec![json!({"role":"user","content":"hi"}), json!({"role":"assistant","content":null,"tool_calls":tool_calls})];
    for (id, _, _) in calls {
        messages.push(json!({"role":"tool","tool_call_id":id,"content":"out"}));
    }
    json!({"model":"gpt-4o-mini","messages":messages,"tools":chat_tools(tools)})
}

/// The client's next Responses request: the output items of the answer, then their results.
fn responses_second(answer: &Sent, tools: &[&str]) -> Value {
    let output = answer.json()["output"].as_array().cloned().expect("an output");
    let mut input = vec![json!({"role":"user","content":"hi"})];
    for item in &output {
        input.push(item.clone());
    }
    for item in &output {
        if let Some(call_id) = item["call_id"].as_str() {
            input.push(json!({"type":"function_call_output","call_id":call_id,"output":"out"}));
        }
    }
    json!({"model":"gpt-6","input":input,"tools":responses_tools(tools)})
}

// --- readers ---------------------------------------------------------------------------------

/// `(session id, turn)` of the response-side records of `tool`, in order.
fn sessions_of(rig: &Rig, tool: &str) -> Vec<(String, i64)> {
    rig.records.calls_of(tool).into_iter().map(|r| (r.session.id, r.session.turn)).collect()
}

fn result_events(events: &[StoredEvent]) -> Vec<&StoredEvent> {
    events.iter().filter(|e| e.kind == EventKind::Result).collect()
}

/// The value of the counter `name` with exactly `labels`, or 0. It is the total since the test set its recorder.
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

/// A request-side history finding: detector `kernel` and the severity of its triple (SMA-855 P-9).
fn finding(snapshotter: &Snapshotter, kind: &str) -> u64 {
    let severity = match kind {
        "forged_history" => "high",
        "mixed_sessions" => "medium",
        _ => "low",
    };
    counter_value(
        snapshotter,
        names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
        &[("side", "request"), ("kind", kind), ("detector", "kernel"), ("severity", severity)],
    )
}

/// Both requests of a conversation joined one session: request 2 has turn 2 and the session of
/// request 1, and the session holds one `Result` event of `tool`.
fn assert_one_session(rig: &Rig, tool: &str) {
    let seen = sessions_of(rig, tool);
    assert_eq!(seen.len(), 2, "one response record for each request: {seen:?}");
    assert_eq!(seen[0].1, 1, "{seen:?}");
    assert_eq!(seen[1].0, seen[0].0, "request 2 joins the session of request 1: {seen:?}");
    assert_eq!(seen[1].1, 2, "{seen:?}");
    let events = rig.events(&seen[0].0, tool);
    let results = result_events(&events);
    assert_eq!(results.len(), 1, "one Result event: {events:?}");
    assert_eq!(results[0].tool, tool, "the stored tool name of the call: {events:?}");
    let pending: Vec<_> = events.iter().filter(|e| e.verdict == EventVerdict::Pending).collect();
    assert!(pending.is_empty(), "every call is settled: {events:?}");
}

// --- tests -----------------------------------------------------------------------------------

#[tokio::test]
async fn chat_a_forwarded_call_and_its_result_stay_in_one_session() {
    let rig = rig();
    let first = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["read_file"], false), chat_json("c", &[("read_file", "{}")])).await;
    assert_eq!(first.status, StatusCode::OK);
    let second = rig
        .send(PRINCIPAL_1, CHAT, &chat_second(&chat_calls(&first), &["read_file"]), chat_json("d", &[("read_file", "{}")]))
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_one_session(&rig, "read_file");
}

#[tokio::test]
async fn responses_a_forwarded_call_and_its_result_stay_in_one_session() {
    let rig = rig();
    let first = rig
        .send(PRINCIPAL_1, RESPONSES, &responses_first(&["read_file"]), responses_json(vec![fn_item("c", 0, "read_file", "{}")]))
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = rig
        .send(
            PRINCIPAL_1,
            RESPONSES,
            &responses_second(&first, &["read_file"]),
            responses_json(vec![fn_item("d", 0, "read_file", "{}")]),
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_one_session(&rig, "read_file");
}

#[tokio::test]
async fn a_stream_answer_links_like_a_body_answer() {
    let rig = rig();
    let first = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["read_file"], true), chat_sse("c", &[("read_file", "{}")])).await;
    assert_eq!(first.status, StatusCode::OK);
    let calls = chat_calls(&first);
    assert_eq!(calls.len(), 1, "{}", first.text());
    let second = rig.send(PRINCIPAL_1, CHAT, &chat_second(&calls, &["read_file"]), chat_json("d", &[("read_file", "{}")])).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_one_session(&rig, "read_file");
}

/// Request 1 blocks `rm` by the floor. Its notice carries the decision id. Request 2 holds the
/// notice text of the answer, and the ref links it to the session of request 1.
async fn blocked_first(rig: &Rig) -> String {
    let first = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["rm"], false), chat_json("c", &[("rm", "{}")])).await;
    assert_eq!(first.status, StatusCode::OK);
    let notice = first.json()["choices"][0]["message"]["content"].as_str().expect("the notice text").to_owned();
    assert!(notice.contains("was blocked"), "{notice}");
    notice
}

fn chat_after_notice(notice: &str) -> Value {
    json!({"model":"gpt-4o-mini","messages":[
        {"role":"user","content":"hi"},
        {"role":"assistant","content":notice},
        {"role":"user","content":"go on"}],
        "tools":chat_tools(&["read_file"])})
}

#[tokio::test]
async fn a_blocked_call_links_the_next_request_by_its_ref() {
    let rig = rig();
    let notice = blocked_first(&rig).await;
    let second = rig.send(PRINCIPAL_1, CHAT, &chat_after_notice(&notice), chat_json("d", &[("read_file", "{}")])).await;
    assert_eq!(second.status, StatusCode::OK);
    let first_rm = sessions_of(&rig, "rm");
    let second_read = sessions_of(&rig, "read_file");
    assert_eq!(first_rm.len(), 1, "{first_rm:?}");
    assert_eq!(second_read.len(), 1, "{second_read:?}");
    assert_eq!(second_read[0].0, first_rm[0].0, "the ref links request 2 to request 1");
    assert_eq!((first_rm[0].1, second_read[0].1), (1, 2));
}

#[tokio::test]
async fn a_ref_of_another_principal_starts_a_new_session() {
    let rig = rig();
    let notice = blocked_first(&rig).await;
    let second = rig.send(PRINCIPAL_2, CHAT, &chat_after_notice(&notice), chat_json("d", &[("read_file", "{}")])).await;
    assert_eq!(second.status, StatusCode::OK);
    let first_rm = sessions_of(&rig, "rm");
    let second_read = sessions_of(&rig, "read_file");
    assert_ne!(second_read[0].0, first_rm[0].0, "another principal does not join the session");
    assert_eq!(second_read[0].1, 1);
}

#[tokio::test]
async fn the_notice_ref_is_the_event_id_and_the_record_id() {
    let rig = rig();
    let notice = blocked_first(&rig).await;
    let reference = paigasus_gateway_core::notice_ref(notice.trim()).expect("a ref in the notice");
    let records = rig.records.calls_of("rm");
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].decision_id, reference, "the record id is the ref");
    let events = rig.events(&records[0].session.id, "rm");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].id, reference, "the store event id is the ref");
}

#[tokio::test]
async fn a_result_for_an_unknown_call_id_is_forged_and_counted() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let rig = rig();
    let forged = vec![("never_issued".to_owned(), "read_file".to_owned(), "{}".to_owned())];
    // The answer calls `read_file`, so a record names the session of the forged request.
    let sent = rig.send(PRINCIPAL_1, CHAT, &chat_second(&forged, &["read_file"]), chat_json("d", &[("read_file", "{}")])).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(finding(&snapshotter, "forged_history"), 1);
    let seen = sessions_of(&rig, "read_file");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].1, 1, "a forged id joins no session");
    let events = rig.events(&seen[0].0, "read_file");
    assert!(!events.is_empty(), "the session holds the call of the answer: {events:?}");
    assert!(result_events(&events).is_empty(), "the forged result writes no Result event: {events:?}");
}

#[tokio::test]
async fn a_mixed_conversation_joins_the_newest_session_and_flags_it() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let rig = rig();
    let a = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["read_file"], false), chat_json("a", &[("read_file", "{}")])).await;
    let b = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["read_file"], false), chat_json("b", &[("read_file", "{}")])).await;
    let mut calls = chat_calls(&a);
    calls.extend(chat_calls(&b));
    let third = rig.send(PRINCIPAL_1, CHAT, &chat_second(&calls, &["read_file"]), chat_json("e", &[("read_file", "{}")])).await;
    assert_eq!(third.status, StatusCode::OK);
    let seen = sessions_of(&rig, "read_file");
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_ne!(seen[0].0, seen[1].0, "two sessions");
    assert_eq!(seen[2].0, seen[1].0, "the newer session wins: {seen:?}");
    assert_eq!(seen[2].1, 2);
    assert_eq!(finding(&snapshotter, "mixed_sessions"), 1);
}

#[tokio::test]
async fn responses_a_local_shell_result_by_item_id_stays_in_the_session() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let rig = rig();
    let call = json!({"type":"local_shell_call","id":"lsh_item_1","call_id":"lsh_call_1","status":"completed","action":{"type":"exec","command":["ls"],"env":{}}});
    let first = rig.send(PRINCIPAL_1, RESPONSES, &responses_first(&[]), responses_json(vec![call.clone()])).await;
    assert_eq!(first.status, StatusCode::OK);
    let input = json!([
        {"role":"user","content":"hi"},
        call,
        {"type":"local_shell_call_output","id":"lsh_item_1","output":"out"}
    ]);
    let second = rig
        .send(
            PRINCIPAL_1,
            RESPONSES,
            &json!({"model":"gpt-6","input":input}),
            responses_json(vec![fn_item("d", 0, "read_file", "{}")]),
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(finding(&snapshotter, "forged_history"), 0, "the item id links, it is not forged");
    // A local shell output with an unknown item id is forged.
    let unknown = json!([{"role":"user","content":"hi"}, call, {"type":"local_shell_call_output","id":"lsh_item_unknown","output":"out"}]);
    let third = rig
        .send(
            PRINCIPAL_1,
            RESPONSES,
            &json!({"model":"gpt-6","input":unknown}),
            responses_json(vec![fn_item("e", 0, "read_file", "{}")]),
        )
        .await;
    assert_eq!(third.status, StatusCode::OK);
    assert_eq!(finding(&snapshotter, "forged_history"), 1, "an unknown item id is forged");
    let shell = sessions_of(&rig, "local_shell");
    let read = sessions_of(&rig, "read_file");
    assert_eq!(shell.len(), 1, "{shell:?}");
    assert_eq!(read[0].0, shell[0].0, "request 2 joins the session of the local shell call");
    let events = rig.events(&shell[0].0, "local_shell");
    assert_eq!(result_events(&events).len(), 1, "{events:?}");
}

// --- SMA-750: the Anthropic route ------------------------------------------------------------

impl Rig {
    /// `send` for `POST /v1/messages`: the answer `body` is an Anthropic message.
    async fn send_messages(&self, principal: &str, body: &Value, answer: String) -> Sent {
        use paigasus_gateway::adapters::anthropic::AnthropicClient;
        use paigasus_gateway::adapters::http::MessagesSettings;
        use paigasus_gateway::config::AnthropicConfig;

        let mock = MockOpenAi::spawn_json(StatusCode::OK, answer).await;
        let cfg = AnthropicConfig {
            base_url: mock.base_url.clone(),
            api_key: secrecy::SecretString::from("operator-anthropic-test-key".to_owned()),
            extra_ca_bundle_path: None,
            default_version: "2023-06-01".to_owned(),
            non_stream_timeout_secs: 600,
        };
        let app = router(AppState {
            iam: ScopedIam::arc(principal, ORG_A_PRN),
            openai: Arc::new(openai(&mock.base_url, Duration::from_secs(30))),
            max_request_bytes: MIB,
            capabilities: Capabilities { chat_stream: true },
            limits: None,
            dpop_enabled: false,
            tool_interception: Some(Arc::clone(&self.ti)),
            tool_policy: Some(Arc::clone(&self.tp)),
            responses: None,
            anthropic: Some(Arc::new(AnthropicClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(300)).expect("client builds"))),
            messages: Some(Arc::new(MessagesSettings {
                max_request_bytes: 32 * MIB,
                max_record_bytes: 8 * MIB,
                max_held_bytes: 32 * MIB,
                max_hold: Duration::from_secs(600),
            })),
        });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
            .body(Body::from(body.to_string()))
            .expect("build request");
        let resp = app.oneshot(req).await.expect("the router answers");
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
        self.history.tasks().close();
        self.history.tasks().wait().await;
        self.history.tasks().reopen();
        Sent { status, headers, body }
    }
}

/// An Anthropic message that calls `read_file` with the id `toolu_<prefix>`.
fn messages_answer(prefix: &str) -> String {
    json!({"id":"msg_1","type":"message","role":"assistant","model":"claude-x",
        "content":[{"type":"tool_use","id":format!("toolu_{prefix}"),"name":"read_file","input":{}}],
        "stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":5}})
    .to_string()
}

/// SMA-750 D9, SMA-753: a forwarded `tool_use` and its `tool_result` stay in one session through
/// `/v1/messages`. Request 2 joins the session of request 1 as turn 2.
#[tokio::test]
async fn the_anthropic_route_records_history() {
    let rig = rig();
    let tools = json!([{"name":"read_file","description":"d","input_schema":{"type":"object"}}]);
    let first = json!({"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"tools":tools});
    let one = rig.send_messages(PRINCIPAL_1, &first, messages_answer("c")).await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text());
    let call = one.json()["content"][0].clone();
    assert_eq!(call["id"], "toolu_c", "{call}");
    let second = json!({"model":"claude-x","max_tokens":16,"tools":tools,"messages":[
        {"role":"user","content":"hi"},
        {"role":"assistant","content":[call]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_c","content":"out"}]}]});
    let two = rig.send_messages(PRINCIPAL_1, &second, messages_answer("d")).await;
    assert_eq!(two.status, StatusCode::OK, "{}", two.text());
    assert_one_session(&rig, "read_file");
}

// --- SMA-753 PR 2: the history conditions --------------------------------------------------------

use paigasus_gateway::adapters::detectors::BuiltInDetector;
use paigasus_gateway::application::detectors::DetectorPipeline;
use paigasus_gateway::config::{DETECTOR_MARGIN_MS, DetectorSettings};
use paigasus_gateway::domain::detectors::{DetectBatch, Detector, DetectorError};
use paigasus_gateway::domain::limits::UnavailableKind;
use paigasus_gateway::domain::tool_history::{AppendReply, AppendRequest, HistoryStoreError, LookupReply, LookupRequest, ReserveRequest, SettleRequest, WriteRequest};
use paigasus_gateway::domain::tool_policy::{PolicyText, SourceError, TenantPolicies};
use paigasus_gateway_core::OnFailure;
use paigasus_kernel::detect::Finding;
use paigasus_proto::paigasus::iam::v1 as pb;

/// The test floor, and a floor forbid of `send` when the floor slot `fcap` is false or missing.
const CAP_FLOOR: &str = r#"@id("floor-rm") forbid(principal, action == Pgs::Gateway::Action::"rm", resource); @id("floor-cap") forbid(principal, action == Pgs::Gateway::Action::"send", resource) when { !context.fcap };"#;

/// `fcap`: the sum of the Long argument `amount` over the calls of `send` in the session, within
/// one minute, is at most 5000.
fn fcap() -> pb::HistoryCondition {
    use pb::{history_condition, history_predicate};
    let predicate = pb::HistoryPredicate {
        node: Some(history_predicate::Node::Tool(pb::ToolMatch {
            name_key: "send".to_owned(),
            any_catalog_tool: false,
        })),
    };
    pb::HistoryCondition {
        slot: "fcap".to_owned(),
        policy_id: "floor-cap".to_owned(),
        scope: pb::HistoryScope::Session as i32,
        window_ms: 60_000,
        shape: Some(history_condition::Shape::SumWithin(pb::SumWithinShape {
            predicate: Some(predicate),
            field_path: vec!["amount".to_owned()],
            op: pb::ComparisonOp::Le as i32,
            threshold: 5000,
        })),
    }
}

/// A source whose floor holds `CAP_FLOOR` with the condition `fcap`. The tenant set is `TENANT`.
struct FloorCapSource;

#[async_trait::async_trait]
impl PolicySource for FloorCapSource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        Ok(PolicyText {
            text: CAP_FLOOR.to_owned(),
            version: "f1".to_owned(),
            conditions: vec![fcap()],
            max_window_ms: 86_400_000,
        })
    }

    async fn tenant(&self, _org: Uuid) -> Result<TenantPolicies, SourceError> {
        Ok(TenantPolicies {
            text: TENANT.to_owned(),
            catalog: vec!["send".to_owned()],
            version: "t1".to_owned(),
            settings: None,
            conditions: Vec::new(),
            max_window_ms: 86_400_000,
        })
    }
}

/// The memory store, with a lookup that always fails.
struct LookupDown(Arc<MemorySessionStore>);

#[async_trait::async_trait]
impl SessionStore for LookupDown {
    async fn lookup(&self, _: &LookupRequest) -> Result<LookupReply, HistoryStoreError> {
        Err(HistoryStoreError::Unavailable {
            kind: UnavailableKind::Io,
            detail: "down".into(),
        })
    }
    async fn write(&self, req: &WriteRequest) -> Result<AppendReply, HistoryStoreError> {
        self.0.write(req).await
    }
    async fn reserve(&self, req: &ReserveRequest) -> Result<AppendReply, HistoryStoreError> {
        self.0.reserve(req).await
    }
    async fn settle(&self, req: &SettleRequest) -> Result<(), HistoryStoreError> {
        self.0.settle(req).await
    }
    async fn append(&self, req: &AppendRequest) -> Result<AppendReply, HistoryStoreError> {
        self.0.append(req).await
    }
}

/// A judge that sleeps 5 s, then gives no findings. The judge needs no opt-in, so the pipeline
/// waits for it in every context (SMA-856 C8).
struct SlowHook;

#[async_trait::async_trait]
impl Detector for SlowHook {
    fn name(&self) -> &str {
        "judge"
    }
    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(vec![Vec::new(); batch.subjects.len()])
    }
}

/// `decider` with a pipeline that waits 5 s after the reserve, past the decide timeout of the hold.
fn slow_pipeline(decider: CedarDecider) -> CedarDecider {
    let settings = DetectorSettings {
        enabled: true,
        timeout: Duration::from_secs(10),
        max_scan_bytes: 16 << 20,
        margin: Duration::from_millis(DETECTOR_MARGIN_MS),
    };
    let pipeline = Arc::new(DetectorPipeline::new(
        Arc::new(BuiltInDetector::new(settings.max_scan_bytes)),
        Vec::<Arc<dyn Detector>>::new(),
        Some(Arc::new(SlowHook)),
        settings.timeout,
    ));
    decider.with_detectors(pipeline, Duration::from_secs(20), settings)
}

/// The value of `gateway_tool_history_slots_missing_total{part, reason}`.
fn slots_missing(snapshotter: &Snapshotter, part: &str, reason: &str) -> u64 {
    counter_value(snapshotter, names::GATEWAY_TOOL_HISTORY_SLOTS_MISSING_TOTAL, &[("part", part), ("reason", reason)])
}

/// The chat answer holds a block notice.
fn is_blocked(sent: &Sent) -> bool {
    sent.text().contains("was blocked")
}

#[tokio::test]
async fn mode_off_with_a_floor_condition_links_and_counts() {
    let rig = rig_with(Options {
        source: Some(Arc::new(FloorCapSource)),
        mode: TenantMode::Off,
        ..Options::default()
    });
    let first = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["send"], false), chat_json("c", &[("send", r#"{"amount":3000}"#)])).await;
    assert_eq!(first.status, StatusCode::OK);
    // Without a link the request has no session, and the floor slot is missing: a block.
    assert!(!is_blocked(&first), "the request is linked, so the floor slot is true: {}", first.text());
    assert_eq!(chat_calls(&first).len(), 1, "{}", first.text());
    let second = rig
        .send(PRINCIPAL_1, CHAT, &chat_second(&chat_calls(&first), &["send"]), chat_json("d", &[("send", r#"{"amount":3000}"#)]))
        .await;
    assert_eq!(second.status, StatusCode::OK);
    assert!(is_blocked(&second), "3000 + 3000 is over the floor cap: {}", second.text());
    let seen = sessions_of(&rig, "send");
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[1].0, seen[0].0, "both requests are in one session: {seen:?}");
    let calls: Vec<_> = rig.events(&seen[0].0, "send").into_iter().filter(|e| e.kind == EventKind::Call).map(|e| e.verdict).collect();
    assert_eq!(calls, [EventVerdict::Allow, EventVerdict::Deny], "both calls count");
}

#[tokio::test]
async fn a_lookup_error_gives_no_session_and_misses_the_floor_slot() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let rig = rig_with(Options {
        source: Some(Arc::new(FloorCapSource)),
        store: Some(|m| Arc::new(LookupDown(m))),
        ..Options::default()
    });
    let sent = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["send"], false), chat_json("c", &[("send", r#"{"amount":1}"#)])).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(is_blocked(&sent), "the floor slot is missing, so the floor forbid denies: {}", sent.text());
    assert_eq!(slots_missing(&snapshotter, "floor", "no_session"), 1);
    assert_eq!(slots_missing(&snapshotter, "floor", "store"), 0);
    assert!(rig.store.events(org(), Scope::Principal, PRINCIPAL_1, "send").is_empty(), "no session, so no event");
}

/// One hold of `send`, decided after the decide timeout of 200 ms, under `on_failure`. The events
/// of `send` in the session of the response record.
async fn timed_out_send(on_failure: OnFailure) -> (Sent, Vec<StoredEvent>) {
    let rig = rig_with(Options {
        limits: CoreLimits {
            on_failure,
            decide_timeout: Duration::from_millis(200),
            ..CoreLimits::defaults()
        },
        slow_after_reserve: true,
        ..Options::default()
    });
    let sent = rig.send(PRINCIPAL_1, CHAT, &chat_first(&["send"], false), chat_json("c", &[("send", "{}")])).await;
    assert_eq!(sent.status, StatusCode::OK);
    let seen = sessions_of(&rig, "send");
    assert_eq!(seen.len(), 1, "{seen:?}");
    let events = rig.events(&seen[0].0, "send");
    (sent, events)
}

#[tokio::test]
async fn a_decide_timeout_under_allow_counts_the_call_once() {
    let (sent, events) = timed_out_send(OnFailure::Allow).await;
    assert!(!is_blocked(&sent), "allowed on failure: {}", sent.text());
    assert_eq!(
        events.iter().map(|e| e.verdict).collect::<Vec<_>>(),
        [EventVerdict::Allow],
        "the reserved event is settled, not appended again: {events:?}"
    );
}

#[tokio::test]
async fn an_interceptor_block_after_the_reserve_settles_deny() {
    let (sent, events) = timed_out_send(OnFailure::Block).await;
    assert!(is_blocked(&sent), "{}", sent.text());
    assert_eq!(events.iter().map(|e| e.verdict).collect::<Vec<_>>(), [EventVerdict::Deny], "one event, settled deny: {events:?}");
}
