// SPDX-License-Identifier: Apache-2.0
//! SMA-750 § 3, § 8, § 10.2: tool-call interception through `POST /v1/messages`, stream and body,
//! over the real router and the in-process mock upstream. The `BlockNamed` decider blocks every
//! call whose name starts with `blocked_`, as in `responses_proxy.rs`.
//!
//! The recorded type table ([`RECORDED_TYPES`]) lists every event, block and delta type of spec
//! section 15. The list of the Claude Code session is pending Task 14: it adds its rows there.

mod support;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::anthropic::AnthropicClient;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::messages::MESSAGES_TERMINAL_SSE_ERROR;
use paigasus_gateway::adapters::http::{AppState, MessagesSettings, router};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::{AnthropicConfig, ToolPolicySettings};
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::{AllowAll, BlockCode, DecideFuture, DecidedCall, Decision, DecisionContext, Limits as CoreLimits, OnUnavailable, TenantMode, ToolCall, ToolCallDecider, ToolTurn};
use paigasus_observability::names;
use secrecy::SecretString;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::{CALLER_KEY, ORG_A, ORG_A_PRN, PRINCIPAL_1, ScopedIam, Sent};
use tempfile::TempDir;
use tower::ServiceExt;

const MIB: usize = 1 << 20;
const STREAM: &str = r#"{"model":"claude-x","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#;
const NON_STREAM: &str = r#"{"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#;
const PING: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
/// The documented Anthropic mid-stream error record (spec § 15: no real one was recorded).
const OVERLOADED: &str = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
const NOTICE: &str = r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#;

// --- rig -------------------------------------------------------------------------------------

/// Blocks every call whose name starts with `blocked_`.
struct BlockNamed;

impl ToolCallDecider for BlockNamed {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        let d = calls
            .iter()
            .map(|c| {
                if c.name.starts_with("blocked_") {
                    DecidedCall::from(Decision::Block { code: BlockCode::Policy })
                } else {
                    DecidedCall::from(Decision::Allow)
                }
            })
            .collect();
        Box::pin(std::future::ready(Ok(d)))
    }
}

fn settings() -> MessagesSettings {
    MessagesSettings {
        max_request_bytes: 32 * MIB,
        max_record_bytes: 8 * MIB,
        max_held_bytes: 32 * MIB,
        max_hold: Duration::from_secs(600),
    }
}

fn anthropic(base_url: &str) -> AnthropicClient {
    let cfg = AnthropicConfig {
        base_url: base_url.to_owned(),
        api_key: SecretString::from("operator-anthropic-test-key".to_owned()),
        extra_ca_bundle_path: None,
        default_version: "2023-06-01".to_owned(),
        non_stream_timeout_secs: 600,
    };
    AnthropicClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(300)).expect("client builds")
}

fn ti_with(limits: CoreLimits, decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception {
        limits,
        decider,
        recorder: None,
        history: None,
    }))
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    ti_with(CoreLimits::defaults(), decider)
}

fn app(base_url: &str, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(support::limits::openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: true },
        limits: None,
        dpop_enabled: false,
        tool_interception: ti,
        tool_policy: tp,
        responses: None,
        anthropic: Some(Arc::new(anthropic(base_url))),
        messages: Some(Arc::new(settings())),
    })
}

async fn message(app: &Router, body: &str) -> Sent {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-api-key", CALLER_KEY)
        .body(Body::from(body.to_owned()))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

/// One SSE record of the Anthropic shape.
fn record(v: &Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().expect("a type"))
}

fn message_start() -> String {
    record(
        &json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[],
        "stop_reason":null,"usage":{"input_tokens":3,"output_tokens":1}}}),
    )
}

/// The records that open a `tool_use` block named `name` at `index` with its first input delta.
fn tool_use_open(index: usize, name: &str, partial: &str) -> Vec<String> {
    vec![
        record(&json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":format!("toolu_{index}"),"name":name,"input":{}}})),
        record(&json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":partial}})),
    ]
}

/// A full stream: a text block, then the calls `(name, input JSON)`, then the end records.
fn call_stream(calls: &[(&str, &str)]) -> Vec<String> {
    let mut out = vec![
        message_start(),
        record(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        record(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Sure."}})),
        record(&json!({"type":"content_block_stop","index":0})),
    ];
    for (i, (name, input)) in calls.iter().enumerate() {
        let index = i + 1;
        out.extend(tool_use_open(index, name, input));
        out.push(record(&json!({"type":"content_block_stop","index":index})));
    }
    out.push(record(
        &json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":5}}),
    ));
    out.push(record(&json!({"type":"message_stop"})));
    out
}

/// The stream text without its `: paigasus-decision` frames.
fn without_comments(text: &str) -> String {
    text.split_inclusive("\n\n").filter(|f| !f.starts_with(": paigasus-decision ")).collect()
}

/// Serve ONE connection with a chunked SSE answer: send `first`, wait `pause`, send `rest`, then
/// close with the chunked terminator when `finish` is set, and without it when not (a cut stream).
async fn spawn_paced(first: Vec<String>, pause: Duration, rest: Vec<String>, finish: bool) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let mut head = String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
            for r in &first {
                head.push_str(&format!("{:x}\r\n{r}\r\n", r.len()));
            }
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.flush().await;
            tokio::time::sleep(pause).await;
            let mut tail = String::new();
            for r in &rest {
                tail.push_str(&format!("{:x}\r\n{r}\r\n", r.len()));
            }
            if finish {
                tail.push_str("0\r\n\r\n");
            }
            let _ = sock.write_all(tail.as_bytes()).await;
            let _ = sock.flush().await;
            if !finish {
                // Drop the socket without the terminator.
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    format!("http://{addr}")
}

fn counter_total(snapshotter: &Snapshotter, name: &str) -> u64 {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(k, _, _, _)| k.key().name() == name)
        .map(|(_, _, _, v)| match v {
            DebugValue::Counter(c) => c,
            _ => 0,
        })
        .sum()
}

// --- the block rewrite (§ 3, § 8) --------------------------------------------------------------

/// A blocked `tool_use` block never reaches the client. Its records become a text block with the
/// notice, and the report comments go out.
#[tokio::test]
async fn a_streamed_blocked_tool_use_never_reaches_the_client() {
    let mock = MockOpenAi::spawn_raw_sse(call_stream(&[("blocked_rm", r#"{"p":"CANARY"}"#), ("read_file", r#"{"p":1}"#)])).await;
    let sent = message(&app(&mock.base_url, ti(Arc::new(BlockNamed)), None), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.header("content-type"), "text/event-stream");
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(!text.contains(r#""type":"tool_use","id":"toolu_1""#), "the blocked block start is gone: {text}");
    assert!(text.contains(NOTICE), "the notice text block: {text}");
    assert!(text.contains(r#""type":"tool_use","id":"toolu_2","name":"read_file""#), "the allowed call goes through: {text}");
    assert!(text.contains(": paigasus-decision {"), "{text}");
    assert!(text.contains(r#""stop_reason":"tool_use""#), "the allowed call keeps the stop reason: {text}");
}

/// A body whose only call is blocked is rewritten and carries the decisions header.
#[tokio::test]
async fn a_non_stream_blocked_tool_use_is_rewritten_with_the_header() {
    let body = json!({"id":"msg_1","type":"message","role":"assistant","model":"claude-x",
        "content":[{"type":"text","text":"Sure."},{"type":"tool_use","id":"toolu_1","name":"blocked_rm","input":{"p":"CANARY"}}],
        "stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":5}})
    .to_string();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body).await;
    let sent = message(&app(&mock.base_url, ti(Arc::new(BlockNamed)), None), NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    let v = sent.json();
    assert_eq!(v["content"][0], json!({"type":"text","text":"Sure."}), "{v}");
    assert_eq!(v["content"][1]["type"], "text", "the blocked slot becomes a text block: {v}");
    assert_eq!(v["content"][1]["text"], "[paigasus] tool call \"blocked_rm\" was blocked (policy).", "{v}");
    assert!(v["content"].as_array().expect("content").iter().all(|b| b["type"] != "tool_use"), "{v}");
    assert_eq!(v["stop_reason"], "end_turn", "no call is left, so the turn ends: {v}");
    let header = sent.header("paigasus-tool-decisions");
    assert!(!header.is_empty(), "the decisions header is present");
    assert!(header.contains("blocked_rm"), "{header}");
}

/// One blocked and one allowed call: the blocked slot becomes the notice, the allowed call stays
/// in its slot, and the stop reason stays `tool_use`.
#[tokio::test]
async fn a_non_stream_mixed_body_keeps_the_tool_use_stop_reason() {
    let body = json!({"id":"msg_1","type":"message","role":"assistant","model":"claude-x",
        "content":[{"type":"tool_use","id":"toolu_1","name":"blocked_rm","input":{"p":"CANARY"}},
                   {"type":"tool_use","id":"toolu_2","name":"read_file","input":{"p":1}}],
        "stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":5}})
    .to_string();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body).await;
    let sent = message(&app(&mock.base_url, ti(Arc::new(BlockNamed)), None), NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"), "{}", sent.text());
    let v = sent.json();
    assert_eq!(v["content"][0]["type"], "text", "{v}");
    assert_eq!(v["content"][0]["text"], "[paigasus] tool call \"blocked_rm\" was blocked (policy).", "{v}");
    assert_eq!(v["content"][1], json!({"type":"tool_use","id":"toolu_2","name":"read_file","input":{"p":1}}), "{v}");
    assert_eq!(v["stop_reason"], "tool_use", "{v}");
}

/// § 8, D16: a body that cannot be checked is a 502 under `block`.
#[tokio::test]
async fn a_malformed_body_is_a_502_with_x_should_retry_false() {
    let bad = r#"{"id":"msg_1","type":"message","role":"assistant","content":[],"a":1,"a":2}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, bad).await;
    let sent = message(&app(&mock.base_url, ti(Arc::new(AllowAll)), None), NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::BAD_GATEWAY, "{}", sent.text());
    assert_eq!(sent.header("x-should-retry"), "false");
    let v = sent.json();
    assert_eq!(v["type"], "error", "{v}");
    assert_eq!(v["error"]["type"], "api_error", "{v}");
    assert_eq!(v["error"]["code"], "tool-interception-malformed-response", "{v}");
    assert_eq!(v["request_id"], sent.header("paigasus-request-id"), "{v}");
    assert!(!sent.text().contains("\"a\":2"), "the body never reaches the client");
}

// --- D17, D13, D12 -----------------------------------------------------------------------------

/// D17: an upstream `error` record during a hold is the terminal. The client gets it byte for
/// byte as the last record, the held call never leaves, and the gateway adds no D13 frame.
#[tokio::test]
async fn an_upstream_error_event_during_a_hold_reaches_the_client_unchanged() {
    let mut records = vec![message_start()];
    records.extend(tool_use_open(0, "read_file", r#"{"p":"CANARY"#));
    records.push(OVERLOADED.to_owned());
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let sent = message(&app(&mock.base_url, ti(Arc::new(BlockNamed)), None), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    let body = without_comments(&text);
    assert!(body.ends_with(OVERLOADED), "the upstream record is the last record: {text}");
    assert_eq!(text.matches("event: error").count(), 1, "{text}");
    assert!(!text.contains("upstream-error"), "no D13 frame: {text}");
    assert!(!text.ends_with(MESSAGES_TERMINAL_SSE_ERROR), "{text}");
}

/// D13: a transport failure during a hold ends with the D13 frame, and the held call stays held.
#[tokio::test]
async fn a_transport_failure_during_a_hold_gives_the_d13_frame() {
    let mut records = vec![message_start()];
    records.extend(tool_use_open(0, "read_file", r#"{"p":"CANARY"#));
    let base = spawn_paced(records, Duration::from_millis(50), Vec::new(), false).await;
    let sent = message(&app(&base, ti(Arc::new(BlockNamed)), None), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.ends_with(MESSAGES_TERMINAL_SSE_ERROR), "{text}");
    assert_eq!(text.matches("\"code\":\"upstream-error\"").count(), 1, "{text}");
}

/// D12: while a call is held, the client sees Anthropic `ping` events, not the SSE comment that
/// the OpenAI routes use.
#[tokio::test]
async fn a_held_call_keeps_alive_with_ping_events() {
    let mut first = vec![message_start()];
    first.extend(tool_use_open(0, "read_file", r#"{"p":"#));
    let mut rest = vec![record(&json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"1}"}}))];
    rest.push(record(&json!({"type":"content_block_stop","index":0})));
    rest.push(record(
        &json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":5}}),
    ));
    rest.push(record(&json!({"type":"message_stop"})));
    let base = spawn_paced(first, Duration::from_millis(1200), rest, true).await;
    let limits = CoreLimits {
        keepalive: Duration::from_millis(200),
        ..CoreLimits::defaults()
    };
    let sent = message(&app(&base, ti_with(limits, Arc::new(BlockNamed)), None), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    let pings = sent.body.windows(PING.len()).filter(|w| *w == PING).count();
    assert!(pings >= 2, "ping events while the call is held ({pings}): {}", sent.text());
    assert!(!sent.text().contains(": keep-alive"), "{}", sent.text());
    assert!(sent.text().contains(r#""name":"read_file""#), "the allowed call is released at the end");
}

// --- every recorded type (§ 15) ----------------------------------------------------------------

/// `(kind, type)` of every event, block and delta type recorded on 2026-11-25 (spec § 15). The
/// Claude Code session of Task 14 adds its rows here.
const RECORDED_TYPES: &[(&str, &str)] = &[
    ("event", "message_start"),
    ("event", "content_block_start"),
    ("event", "content_block_delta"),
    ("event", "content_block_stop"),
    ("event", "message_delta"),
    ("event", "message_stop"),
    ("event", "ping"),
    ("block", "text"),
    ("block", "thinking"),
    ("block", "tool_use"),
    ("block", "server_tool_use"),
    ("block", "web_fetch_tool_result"),
    ("delta", "text_delta"),
    ("delta", "thinking_delta"),
    ("delta", "signature_delta"),
    ("delta", "input_json_delta"),
];

/// The recorded streams that this test needs, copied from the core fixtures (the web fetch one is
/// synthetic).
const STREAMS: &[(&str, &str)] = &[
    ("text_stream", include_str!("fixtures/anthropic/text_stream.sse")),
    ("parallel_tool_use", include_str!("fixtures/anthropic/parallel_tool_use.sse")),
    ("thinking_then_tool_use", include_str!("fixtures/anthropic/thinking_then_tool_use.sse")),
    ("server_tool_web_fetch", include_str!("fixtures/anthropic/server_tool_web_fetch.sse")),
];

/// The `(kind, type)` pairs of one stream.
fn types_of(stream: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for rec in stream.split_inclusive("\n\n") {
        let Some(data) = rec.lines().find_map(|l| l.strip_prefix("data: ")) else { continue };
        let v: Value = serde_json::from_str(data).expect("a JSON record");
        out.insert(("event".to_owned(), v["type"].as_str().expect("a type").to_owned()));
        if let Some(b) = v["content_block"]["type"].as_str() {
            out.insert(("block".to_owned(), b.to_owned()));
        }
        if v["type"] == "content_block_delta" {
            out.insert(("delta".to_owned(), v["delta"]["type"].as_str().expect("a delta type").to_owned()));
        }
    }
    out
}

/// With interception on and allow-all, every recorded stream passes with the same payload bytes
/// and only `: paigasus-decision` comments are added. No record is dropped, and the recorded
/// streams together hold exactly the types of [`RECORDED_TYPES`].
#[tokio::test]
async fn every_recorded_type_passes_with_interception_on() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let mut seen = BTreeSet::new();
    for (name, stream) in STREAMS {
        seen.extend(types_of(stream));
        let chunks: Vec<String> = stream.split_inclusive("\n\n").map(str::to_owned).collect();
        let mock = MockOpenAi::spawn_raw_sse(chunks).await;
        let sent = message(&app(&mock.base_url, ti(Arc::new(AllowAll)), None), STREAM).await;
        assert_eq!(sent.status, StatusCode::OK, "{name}");
        let text = sent.text();
        assert!(without_comments(&text) == *stream, "{name}: the payload bytes differ from the recording");
        let has_call = types_of(stream).iter().any(|(k, t)| k == "block" && (t == "tool_use" || t == "server_tool_use"));
        assert_eq!(text.contains(": paigasus-decision "), has_call, "{name}: a decision comment for a call only");
    }
    assert_eq!(counter_total(&snapshotter, names::GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL), 0, "no record is dropped");
    let table: BTreeSet<(String, String)> = RECORDED_TYPES.iter().map(|(k, t)| ((*k).to_owned(), (*t).to_owned())).collect();
    assert_eq!(seen, table, "the streams hold every type of the table and no other");
}

// --- policy: D18 and D9 ------------------------------------------------------------------------

fn policy_dir() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("floor.cedar"),
        "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"floor_rm\", resource);\n",
    )
    .expect("floor");
    let t = dir.path().join("tenants").join(ORG_A);
    std::fs::create_dir_all(&t).expect("tenant dir");
    std::fs::write(
        t.join("policies.cedar"),
        r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"blocked_rm", resource);"#,
    )
    .expect("tenant");
    dir
}

fn tool_policy(dir: &TempDir) -> Option<Arc<RequestToolPolicy>> {
    let settings = ToolPolicySettings {
        enabled: true,
        default_mode: TenantMode::Enforce,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(60),
        orgs: HashMap::new(),
        settings_from_source: false,
    };
    let cache = PolicyCache::new(
        Arc::new(FilePolicySource::new(dir.path())),
        CacheSettings::from_settings(&settings),
        Arc::new(paigasus_gateway::adapters::history::HmacHistoryDigester::keyless()),
    );
    Some(Arc::new(RequestToolPolicy::new(
        cache,
        settings,
        Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()),
    )))
}

fn cedar() -> Option<Arc<ToolInterception>> {
    ti(Arc::new(CedarDecider::new(Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()))))
}

const OK_BODY: &str =
    r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":5}}"#;

/// D18: a key such as `Name`, `ID` or `constructor` below a user-data subtree must not make the
/// strict parse refuse the request. One request for each subtree of spec section 15, under an
/// enforced policy that denies another tool.
#[tokio::test]
async fn user_data_keys_pass_an_enforced_policy() {
    let dir = policy_dir();
    let data = json!({"properties":{"Name":{},"ID":{}},"Name":1,"constructor":2});
    let base = |extra: Value| {
        let mut v = json!({"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}],
            "tools":[{"name":"read_file","description":"d","input_schema":{"type":"object","properties":{"Name":{},"ID":{}}}}]});
        for (k, val) in extra.as_object().expect("an object") {
            v[k] = val.clone();
        }
        v
    };
    let mut tool_with_examples = base(json!({}));
    tool_with_examples["tools"][0]["input_examples"] = json!([{"Name":"Ada","constructor":2}]);
    let followup = base(json!({"messages":[
        {"role":"user","content":"hi"},
        {"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"Name":"Ada","ID":1,"constructor":2}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"out"}]}]}));
    let cases = [
        ("output_format.schema", base(json!({"output_format":{"type":"json_schema","schema":data}}))),
        ("output_config.format.schema", base(json!({"output_config":{"format":{"type":"json_schema","schema":data}}}))),
        ("tools[].input_examples", tool_with_examples),
        ("tools[].input_schema", base(json!({}))),
        ("tool_use.input in a follow-up turn", followup),
    ];
    for (name, body) in cases {
        let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
        let sent = message(&app(&mock.base_url, cedar(), tool_policy(&dir)), &body.to_string()).await;
        assert_eq!(sent.status, StatusCode::OK, "{name}: {}", sent.text());
        assert_eq!(mock.request_count(), 1, "{name}: the request reaches the upstream");
    }
    // The same request with a case-folded key at the top is still refused: the rule is not off.
    let bad = base(json!({"Type":"x"}));
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let sent = message(&app(&mock.base_url, cedar(), tool_policy(&dir)), &bad.to_string()).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{}", sent.text());
    assert_eq!(mock.request_count(), 0);
}

/// D9: in enforce mode the denied tool definition is removed from the upstream body. The other
/// fields are byte-identical.
#[tokio::test]
async fn a_denied_tool_definition_is_removed_from_the_upstream_body() {
    let dir = policy_dir();
    let original = json!({"model":"claude-x","max_tokens":16,"temperature":0.25,"system":"Be brief.","messages":[{"role":"user","content":"hi"}],
        "tools":[{"name":"read_file","description":"d","input_schema":{"type":"object"}},{"name":"blocked_rm","description":"d","input_schema":{"type":"object"}}]});
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let sent = message(&app(&mock.base_url, cedar(), tool_policy(&dir)), &original.to_string()).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    let up: Value = serde_json::from_slice(&mock.recorded().expect("recorded").body).expect("json");
    assert_eq!(up["tools"].as_array().expect("tools").len(), 1, "{up}");
    assert_eq!(up["tools"][0]["name"], "read_file");
    let mut expected = original.clone();
    expected["tools"] = json!([original["tools"][0].clone()]);
    assert_eq!(up, expected, "only the denied definition is gone");
    // The same key order and bytes: the rewrite does not re-serialize the other fields.
    let text = String::from_utf8(mock.recorded().expect("recorded").body.to_vec()).expect("utf8");
    assert_eq!(text, expected.to_string(), "the upstream body is the original without the denied definition");
}
