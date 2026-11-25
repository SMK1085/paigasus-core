// SPDX-License-Identifier: Apache-2.0
//! SMA-750 § 10.2: `POST /v1/messages` through the real router, the auth middleware, the
//! Anthropic re-render layer, the limits and the pipeline, against the in-process mock upstream.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures::StreamExt;
use paigasus_gateway::adapters::anthropic::AnthropicClient;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::messages::MESSAGES_TERMINAL_SSE_ERROR;
use paigasus_gateway::adapters::http::{AppState, MessagesSettings, router};
use paigasus_gateway::adapters::iam::{CallerCredential, DpopContext, Iam, IamError};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::{AnthropicConfig, ToolPolicySettings};
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::{AllowAll, Limits as CoreLimits, OnUnavailable, TenantMode, ToolCallDecider};
use paigasus_logging::test_support::capture_logs;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
use secrecy::SecretString;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::{CALLER_KEY, ORG_A, ORG_A_PRN, PRINCIPAL_1, RecordingStore, Sent, limits, openai, rules};
use tempfile::TempDir;
use tonic::Status;
use tower::ServiceExt;

const MIB: usize = 1 << 20;
/// The operator key of the mock Anthropic upstream.
const OPERATOR_KEY: &str = "operator-anthropic-test-key";
const NON_STREAM: &str = r#"{"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#;
const STREAM: &str = r#"{"model":"claude-x","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#;
/// A non-stream answer whose D10 sum is 3 + 2 + 1 + 5 = 11.
const OK_BODY: &str = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"cache_creation_input_tokens":2,"cache_read_input_tokens":1,"output_tokens":5}}"#;
/// The recorded `prompt is too long` answer (Task 1, D21-scrubbed).
const PROMPT_TOO_LONG_BODY: &str = include_str!("fixtures/anthropic/prompt_too_long.body.json");
const PROMPT_TOO_LONG_HEAD: &str = include_str!("fixtures/anthropic/prompt_too_long.response.json");

// --- fixtures --------------------------------------------------------------------------------

/// An active API-key caller that records every token it was asked to introspect.
struct RecordingIam {
    tokens: Mutex<Vec<String>>,
}

impl RecordingIam {
    fn new() -> Arc<Self> {
        Arc::new(RecordingIam { tokens: Mutex::new(Vec::new()) })
    }

    fn tokens(&self) -> Vec<String> {
        self.tokens.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl Iam for RecordingIam {
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        self.tokens.lock().expect("not poisoned").push(token.to_owned());
        Ok(IntrospectApiKeyResponse {
            principal_prn: PRINCIPAL_1.to_owned(),
            status: "active".to_owned(),
            key_id: "key-messages".to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
            scope_prn: ORG_A_PRN.to_owned(),
        })
    }

    async fn is_authorized_self(&self, _caller: &CallerCredential, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(true)
    }

    async fn introspect_token(&self, _token: &str, _dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {
        Err(IamError::Rpc(Status::unauthenticated("not a token")))
    }
}

/// The `[messages]` defaults of the spec (D3, D15).
fn settings() -> MessagesSettings {
    MessagesSettings {
        max_request_bytes: 32 * MIB,
        max_record_bytes: 8 * MIB,
        max_held_bytes: 32 * MIB,
        max_hold: Duration::from_secs(600),
    }
}

fn anthropic(base_url: &str, idle: Duration) -> AnthropicClient {
    let cfg = AnthropicConfig {
        base_url: base_url.to_owned(),
        api_key: SecretString::from(OPERATOR_KEY.to_owned()),
        extra_ca_bundle_path: None,
        default_version: "2023-06-01".to_owned(),
        non_stream_timeout_secs: 600,
    };
    AnthropicClient::new(&cfg, Duration::from_secs(10), idle).expect("client builds")
}

/// The options of one test router.
struct Opts {
    iam: Arc<dyn Iam>,
    messages: Option<MessagesSettings>,
    limits: Option<Arc<Limits>>,
    ti: Option<Arc<ToolInterception>>,
    tp: Option<Arc<RequestToolPolicy>>,
    chat_stream: bool,
    dpop: bool,
    idle: Duration,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            iam: RecordingIam::new(),
            messages: Some(settings()),
            limits: None,
            ti: None,
            tp: None,
            chat_stream: true,
            dpop: false,
            idle: Duration::from_secs(300),
        }
    }
}

/// The router over one mock: OpenAI and Anthropic both point at it.
fn app(base_url: &str, o: Opts) -> Router {
    router(AppState {
        iam: o.iam,
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: o.chat_stream },
        limits: o.limits,
        dpop_enabled: o.dpop,
        tool_interception: o.ti,
        tool_policy: o.tp,
        responses: None,
        anthropic: o.messages.map(|_| Arc::new(anthropic(base_url, o.idle))),
        messages: o.messages.map(Arc::new),
    })
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception {
        limits: CoreLimits::defaults(),
        decider,
        recorder: None,
        history: None,
    }))
}

/// The credential headers of one request.
#[derive(Clone, Copy)]
enum Cred<'a> {
    None,
    XApiKey(&'a str),
    Bearer(&'a str),
}

fn request(method: &str, path: &str, body: impl Into<Body>, creds: &[Cred<'_>], extra: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
    for cred in creds {
        b = match cred {
            Cred::None => b,
            Cred::XApiKey(key) => b.header("x-api-key", *key),
            Cred::Bearer(key) => b.header(header::AUTHORIZATION, format!("Bearer {key}")),
        };
    }
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(body.into()).expect("build request")
}

async fn read(app: &Router, req: Request<Body>) -> Sent {
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

/// POST `body` to `/v1/messages` with an `x-api-key` credential.
async fn message(app: &Router, body: &str) -> Sent {
    read(app, request("POST", "/v1/messages", body.to_owned(), &[Cred::XApiKey(CALLER_KEY)], &[])).await
}

/// D7: the Anthropic envelope of a gateway error, with the § 6.1 `type`. The `request_id` is the
/// answer's `paigasus-request-id`. Returns `error.code`.
fn assert_envelope(sent: &Sent, status: StatusCode, error_type: &str) -> Option<String> {
    assert_eq!(sent.status, status, "{}", sent.text());
    assert_eq!(sent.header("content-type"), "application/json");
    let v = sent.json();
    assert_eq!(v["type"], "error", "{v}");
    assert_eq!(v["error"]["type"], error_type, "{v}");
    assert!(v["error"]["message"].is_string(), "{v}");
    assert_eq!(v["request_id"], sent.header("paigasus-request-id"), "{v}");
    v["error"].get("code").map(|c| c.as_str().expect("a string code").to_owned())
}

/// One SSE record of the Anthropic shape.
fn record(v: &Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().expect("a type"))
}

/// A full text stream in the recorded shape: the D10 sum is 19 + 1 + 2 + 15 = 37.
fn text_stream() -> Vec<String> {
    let start = json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[],
        "stop_reason":null,"usage":{"input_tokens":19,"cache_creation_input_tokens":1,"cache_read_input_tokens":2,"output_tokens":4}}});
    let mut out = vec![record(&start)];
    out.push(record(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})));
    // The recorded ping has a space after the colon; keep the bytes exact.
    out.push("event: ping\ndata: {\"type\": \"ping\"}\n\n".to_owned());
    for text in ["Hello,", " it's nice", " to meet you!"] {
        out.push(record(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})));
    }
    // Anthropic pads some records with spaces before the closing brace (spec § 15).
    out.push("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0      }\n\n".to_owned());
    out.push(record(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},
        "usage":{"input_tokens":19,"cache_creation_input_tokens":1,"cache_read_input_tokens":2,"output_tokens":15}})));
    out.push(record(&json!({"type":"message_stop"})));
    out
}

/// Serve ONE connection with a chunked SSE answer that sends `records`, then closes the socket
/// without the terminating chunk (a mid-stream transport failure after those records).
async fn spawn_cut_stream(records: Vec<String>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let mut response = String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
            for r in &records {
                response.push_str(&format!("{:x}\r\n{r}\r\n", r.len()));
            }
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}")
}

// --- auth (D6) -------------------------------------------------------------------------------

#[tokio::test]
async fn no_credential_is_401_in_the_anthropic_envelope() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let sent = read(&app(&mock.base_url, Opts::default()), request("POST", "/v1/messages", NON_STREAM, &[Cred::None], &[])).await;
    assert!(assert_envelope(&sent, StatusCode::UNAUTHORIZED, "authentication_error").is_some(), "a gateway error carries a code");
    assert_eq!(sent.json()["error"]["message"], "Missing credentials. Send the x-api-key header or an Authorization: Bearer header.");
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn x_api_key_is_accepted() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let iam = RecordingIam::new();
    let app = app(&mock.base_url, Opts { iam: iam.clone(), ..Opts::default() });
    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::XApiKey("pgs-key-x")], &[])).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(iam.tokens(), ["pgs-key-x"]);
}

#[tokio::test]
async fn a_bearer_is_accepted() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let iam = RecordingIam::new();
    let app = app(&mock.base_url, Opts { iam: iam.clone(), ..Opts::default() });
    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::Bearer("pgs-key-b")], &[])).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(iam.tokens(), ["pgs-key-b"]);
}

#[tokio::test]
async fn x_api_key_wins_over_a_bearer() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let iam = RecordingIam::new();
    let app = app(&mock.base_url, Opts { iam: iam.clone(), ..Opts::default() });
    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::Bearer("pgs-key-b"), Cred::XApiKey("pgs-key-x")], &[])).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(iam.tokens(), ["pgs-key-x"], "IAM sees the x-api-key value only");
}

// --- pass-through ----------------------------------------------------------------------------

/// The body bytes and the raw query go upstream unchanged, to the closed `/v1/messages` path
/// (I3), with interception off and on.
#[tokio::test]
async fn the_body_goes_upstream_byte_identical() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let raw = "{ \"model\":\"claude-x\",\n  \"max_tokens\":16, \"messages\":[{\"role\":\"user\",\"content\":\"hi\"}], \"x-unknown\":1.50 }";
    for ti in [None, ti(Arc::new(AllowAll))] {
        let app = app(&mock.base_url, Opts { ti, ..Opts::default() });
        let sent = read(&app, request("POST", "/v1/messages?beta=true", raw, &[Cred::XApiKey(CALLER_KEY)], &[])).await;
        assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
        assert_eq!(sent.text(), OK_BODY);
        let rec = mock.recorded().expect("the upstream got the request");
        assert_eq!((rec.method.as_str(), rec.path.as_str(), rec.query.as_deref()), ("POST", "/v1/messages", Some("beta=true")));
        assert_eq!(rec.body.as_ref(), raw.as_bytes(), "the body goes upstream byte-identical");
    }
}

#[tokio::test]
async fn a_stream_arrives_in_order_with_event_lines() {
    let records = text_stream();
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let sent = message(&app(&mock.base_url, Opts::default()), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.header("content-type"), "text/event-stream");
    assert_eq!(sent.text(), records.concat(), "every record, in order, byte for byte");
    assert_eq!(mock.recorded().expect("recorded").header("accept"), Some("text/event-stream"));
}

/// D13: a transport failure mid-stream ends the answer with the D13 frame.
#[tokio::test]
async fn a_mid_stream_transport_failure_ends_with_the_d13_frame() {
    let base_url = support::spawn_truncated_sse().await;
    let sent = message(&app(&base_url, Opts::default()), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = sent.text();
    assert!(text.starts_with("data: first\n\ndata: second\n\n"), "{text}");
    assert!(text.ends_with(MESSAGES_TERMINAL_SSE_ERROR), "{text}");
    assert_eq!(text.matches("\"code\":\"upstream-error\"").count(), 1, "{text}");
}

/// D15: the route obeys the shared `stream_enabled` switch, before any egress.
#[tokio::test]
async fn streaming_obeys_stream_enabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let app = app(
        &mock.base_url,
        Opts {
            chat_stream: false,
            ..Opts::default()
        },
    );
    let sent = message(&app, STREAM).await;
    assert_eq!(assert_envelope(&sent, StatusCode::BAD_REQUEST, "invalid_request_error").as_deref(), Some("streaming-disabled"));
    assert_eq!(mock.request_count(), 0);
    assert_eq!(message(&app, NON_STREAM).await.status, StatusCode::OK);
}

/// D3, D4: with `[messages] enabled = false` both Messages paths answer the Anthropic 404 with no
/// `code`, with or without a credential, and `/v1/models` is not mounted.
#[tokio::test]
async fn the_route_is_404_when_disabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let off = app(&mock.base_url, Opts { messages: None, ..Opts::default() });
    for path in ["/v1/messages", "/v1/messages/count_tokens"] {
        for creds in [&[Cred::XApiKey(CALLER_KEY)][..], &[Cred::None][..]] {
            let sent = read(&off, request("POST", path, NON_STREAM, creds, &[])).await;
            assert_eq!(assert_envelope(&sent, StatusCode::NOT_FOUND, "not_found_error"), None, "{path}: the fallback has no code");
        }
    }
    let models = read(&off, request("GET", "/v1/models", Body::empty(), &[Cred::XApiKey(CALLER_KEY)], &[("anthropic-version", "2023-06-01")])).await;
    assert_eq!(models.status, StatusCode::NOT_FOUND);
    assert_eq!(mock.request_count(), 0);
    assert_eq!(
        message(&app(&mock.base_url, Opts::default()), NON_STREAM).await.status,
        StatusCode::OK,
        "the same request is served when on"
    );
}

/// G8 for this route: a client disconnect cancels the upstream request.
#[tokio::test]
async fn client_abort_cancels_upstream() {
    let (mock, cancelled): (MockOpenAi, Arc<AtomicBool>) = MockOpenAi::spawn_abortable_stream().await;
    let app = app(&mock.base_url, Opts::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let _ = rustls::crypto::ring::default_provider().install_default();
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-api-key", CALLER_KEY)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(STREAM)
        .send()
        .await
        .expect("responds");
    assert_eq!(resp.status(), StatusCode::OK);
    let mut stream = resp.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("the first chunk arrives")
        .expect("a first chunk")
        .expect("not a transport error");
    assert!(first.starts_with(b"data: "), "{first:?}");
    assert!(!cancelled.load(Ordering::SeqCst));
    drop(stream);
    let propagated = tokio::time::timeout(Duration::from_secs(2), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(propagated.is_ok(), "client-abort must cancel the upstream request");
    server.abort();
}

// --- egress (I2, D5) -------------------------------------------------------------------------

/// I2: the upstream sees the operator key, every `anthropic-*` value in order, and no other
/// caller header. The default `anthropic-version` goes up when the caller sent none.
#[tokio::test]
async fn only_the_operator_key_and_anthropic_headers_reach_the_upstream() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let app = app(&mock.base_url, Opts::default());
    let extra = [
        ("authorization", "Bearer pgs-other"),
        ("paigasus-org", ORG_A),
        ("x-claude-code-session-id", "s-1"),
        ("anthropic-beta", "a"),
        ("anthropic-beta", "b"),
        ("anthropic-version", "2099-01-01"),
        ("anthropic-dangerous-direct-browser-access", "true"),
    ];
    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::XApiKey(CALLER_KEY)], &extra)).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    let rec = mock.recorded().expect("recorded");
    let all = |name: &str| rec.headers.get_all(name).iter().map(|v| v.to_str().expect("ascii").to_owned()).collect::<Vec<_>>();
    assert_eq!(all("x-api-key"), [OPERATOR_KEY], "only the operator key, once");
    assert_eq!(all("anthropic-beta"), ["a", "b"]);
    assert_eq!(all("anthropic-version"), ["2099-01-01"], "the caller's version, not the default");
    assert_eq!(all("anthropic-dangerous-direct-browser-access"), ["true"]);
    for gone in ["authorization", "paigasus-org", "x-claude-code-session-id", "dpop"] {
        assert!(rec.headers.get(gone).is_none(), "{gone} must not go upstream");
    }
    assert!(rec.headers.values().all(|v| !v.to_str().unwrap_or_default().contains(CALLER_KEY)), "the caller key never goes upstream");

    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::XApiKey(CALLER_KEY)], &[])).await;
    assert_eq!(sent.status, StatusCode::OK);
    let rec = mock.recorded().expect("recorded");
    assert_eq!(rec.header("anthropic-version"), Some("2023-06-01"), "the default version");
    assert!(rec.headers.get("anthropic-beta").is_none());
}

// --- errors (D7, D8, R5) ---------------------------------------------------------------------

/// D7, § 6.1: every gateway error of the route is in the Anthropic envelope.
#[tokio::test]
async fn gateway_errors_are_in_the_anthropic_envelope() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let on = app(&mock.base_url, Opts::default());

    let sent = read(&on, request("POST", "/v1/messages", NON_STREAM, &[Cred::None], &[])).await;
    assert_eq!(assert_envelope(&sent, StatusCode::UNAUTHORIZED, "authentication_error").as_deref(), Some("missing-authorization"));

    let sent = message(&on, r#"{"model":1,"messages":[]}"#).await;
    assert_eq!(assert_envelope(&sent, StatusCode::BAD_REQUEST, "invalid_request_error").as_deref(), Some("invalid-request-schema"));

    let small = app(
        &mock.base_url,
        Opts {
            messages: Some(MessagesSettings { max_request_bytes: 256, ..settings() }),
            ..Opts::default()
        },
    );
    let big = format!(r#"{{"model":"claude-x","messages":[{{"role":"user","content":"{}"}}]}}"#, "x".repeat(512));
    let sent = message(&small, &big).await;
    assert_eq!(assert_envelope(&sent, StatusCode::PAYLOAD_TOO_LARGE, "request_too_large").as_deref(), Some("request-too-large"));
    assert_eq!(message(&small, NON_STREAM).await.status, StatusCode::OK, "a body under the cap passes");

    let rate = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), RecordingStore::new())),
            ..Opts::default()
        },
    );
    assert_eq!(message(&rate, NON_STREAM).await.status, StatusCode::OK);
    let sent = message(&rate, NON_STREAM).await;
    assert_eq!(assert_envelope(&sent, StatusCode::TOO_MANY_REQUESTS, "rate_limit_error").as_deref(), Some("rate-limited"));
    let retry: u64 = sent.header("retry-after").parse().expect("an integer retry-after");
    assert!(retry >= 1, "{retry}");

    let budget = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(None, None, Some(1)), RecordingStore::new())),
            ..Opts::default()
        },
    );
    assert_eq!(message(&budget, NON_STREAM).await.status, StatusCode::OK);
    let sent = message(&budget, NON_STREAM).await;
    assert_eq!(assert_envelope(&sent, StatusCode::TOO_MANY_REQUESTS, "billing_error").as_deref(), Some("budget-exhausted"));
    let retry: u64 = sent.header("retry-after").parse().expect("an integer retry-after");
    assert!(retry > 60, "a budget refusal makes Claude Code stop at once (R6): {retry}");
}

/// R5, § 8: a non-2xx upstream answer arrives byte-identical, with its own `content-type`, its
/// D8 headers and no charge, stream request or not.
#[tokio::test]
async fn an_upstream_error_body_arrives_byte_identical() {
    let head: Value = serde_json::from_str(PROMPT_TOO_LONG_HEAD).expect("a fixture head");
    let status = StatusCode::from_u16(u16::try_from(head["status"].as_u64().expect("a status")).expect("fits")).expect("a status");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let headers = head["headers"].as_object().expect("headers");
    let content_type = headers["content-type"].as_str().expect("a content type");
    let others: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| k.as_str() != "content-type")
        .map(|(k, v)| (k.clone(), v.as_str().expect("a string").to_owned()))
        .collect();
    // A second content type proves that the gateway keeps the upstream's, not its own default.
    for ct in [content_type, "application/vnd.anthropic.error+json; charset=utf-8"] {
        let mock = MockOpenAi::spawn_json_with_headers(status, ct, others.clone(), PROMPT_TOO_LONG_BODY).await;
        for body in [NON_STREAM, STREAM] {
            let store = RecordingStore::new();
            let app = app(
                &mock.base_url,
                Opts {
                    limits: Some(limits(rules(None, None, Some(1_000_000)), store.clone())),
                    ..Opts::default()
                },
            );
            let sent = message(&app, body).await;
            assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(sent.body.as_ref(), PROMPT_TOO_LONG_BODY.as_bytes(), "never re-rendered ({body})");
            assert_eq!(sent.header("content-type"), ct, "{body}");
            for (k, v) in &others {
                assert_eq!(sent.header(k), v, "{k} ({body})");
            }
            assert_eq!(store.charges(), Vec::<u64>::new(), "a non-2xx answer charges nothing ({body})");
        }
    }
}

/// D8: `request-id`, `retry-after` and `x-should-retry` pass on a body and on a stream;
/// `anthropic-ratelimit-*` and other upstream headers do not.
#[tokio::test]
async fn the_d8_headers_pass_and_ratelimit_headers_do_not() {
    let upstream_headers: Vec<(String, String)> = [
        ("request-id", "req_upstream_1"),
        ("retry-after", "7"),
        ("x-should-retry", "true"),
        ("anthropic-ratelimit-requests-limit", "50"),
        ("anthropic-ratelimit-tokens-remaining", "1"),
        ("anthropic-organization-id", "org-secret"),
        ("x-upstream-other", "x"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let cases = [("application/json", OK_BODY.to_owned(), NON_STREAM), ("text/event-stream", text_stream().concat(), STREAM)];
    for (ct, upstream_body, request_body) in cases {
        let mock = MockOpenAi::spawn_json_with_headers(StatusCode::OK, ct, upstream_headers.clone(), upstream_body.clone()).await;
        let sent = message(&app(&mock.base_url, Opts::default()), request_body).await;
        assert_eq!(sent.status, StatusCode::OK, "{ct}");
        assert_eq!(sent.text(), upstream_body, "{ct}");
        assert_eq!(sent.header("content-type"), ct);
        assert_eq!(
            (sent.header("request-id"), sent.header("retry-after"), sent.header("x-should-retry")),
            ("req_upstream_1", "7", "true"),
            "{ct}"
        );
        for gone in [
            "anthropic-ratelimit-requests-limit",
            "anthropic-ratelimit-tokens-remaining",
            "anthropic-organization-id",
            "x-upstream-other",
        ] {
            assert!(sent.headers.get(gone).is_none(), "{gone} must not pass ({ct})");
        }
        assert!(sent.headers.keys().all(|k| !k.as_str().starts_with("anthropic-ratelimit")), "{ct}");
    }
}

/// § 6.1 exceptions: an axum 405 on the route is in the Anthropic envelope, with no `code`.
#[tokio::test]
async fn get_on_messages_is_a_405_in_the_anthropic_envelope() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let sent = read(&app(&mock.base_url, Opts::default()), request("GET", "/v1/messages", Body::empty(), &[Cred::XApiKey(CALLER_KEY)], &[])).await;
    assert_eq!(assert_envelope(&sent, StatusCode::METHOD_NOT_ALLOWED, "invalid_request_error"), None);
    assert_eq!(mock.request_count(), 0);
}

/// § 6.2: the re-render replaces the body only. A DPoP 401 keeps `WWW-Authenticate`; a refusal
/// after the request-side policy keeps `paigasus-tool-decisions`.
#[tokio::test]
async fn the_rerender_keeps_www_authenticate_and_tool_decisions() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let dpop = app(&mock.base_url, Opts { dpop: true, ..Opts::default() });
    let sent = read(&dpop, request("POST", "/v1/messages", NON_STREAM, &[Cred::None], &[])).await;
    assert_envelope(&sent, StatusCode::UNAUTHORIZED, "authentication_error");
    assert_eq!(sent.header("www-authenticate"), "DPoP algs=\"ES256 RS256\"");

    let dir = policy_dir();
    let app = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), RecordingStore::new())),
            ti: ti(Arc::new(CedarDecider::new(Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing())))),
            tp: tool_policy(&dir),
            ..Opts::default()
        },
    );
    assert_eq!(message(&app, &tools_body(&["read_file"])).await.status, StatusCode::OK);
    let sent = message(&app, &tools_body(&["read_file", "blocked_rm"])).await;
    assert_eq!(assert_envelope(&sent, StatusCode::TOO_MANY_REQUESTS, "rate_limit_error").as_deref(), Some("rate-limited"));
    let entries: Vec<Value> = serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("an ASCII JSON header");
    assert!(entries.iter().any(|e| e["side"] == "request" && e["tool"] == "blocked_rm"), "{entries:?}");
    assert_eq!(mock.request_count(), 1, "the refused request never went upstream");
}

/// A policy directory: a floor that forbids `floor_rm`, and for `ORG_A` a tenant set that
/// permits every principal and forbids `blocked_rm`.
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

/// The enforced request side over the file source in `dir`.
fn tool_policy(dir: &TempDir) -> Option<Arc<RequestToolPolicy>> {
    tool_policy_in(dir, TenantMode::Enforce)
}

/// The request side over the file source in `dir`, in `mode`.
fn tool_policy_in(dir: &TempDir, mode: TenantMode) -> Option<Arc<RequestToolPolicy>> {
    let settings = ToolPolicySettings {
        enabled: true,
        default_mode: mode,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(60),
        orgs: HashMap::new(),
        settings_from_source: false,
    };
    let cache = PolicyCache::new(Arc::new(FilePolicySource::new(dir.path())), CacheSettings::from_settings(&settings));
    Some(Arc::new(RequestToolPolicy::new(
        cache,
        settings,
        Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()),
    )))
}

/// A Messages request that defines the named tools.
fn tools_body(names: &[&str]) -> String {
    let tools: Vec<Value> = names.iter().map(|n| json!({"name":n,"description":"d","input_schema":{"type":"object"}})).collect();
    json!({"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"tools":tools}).to_string()
}

// --- limits (D10) ----------------------------------------------------------------------------

/// Send `body` once with a budget, and return the charges.
async fn charges(base_url: &str, body: &str) -> (Vec<u64>, Sent) {
    let store = RecordingStore::new();
    let app = app(
        base_url,
        Opts {
            limits: Some(limits(rules(None, None, Some(1_000_000_000)), store.clone())),
            ..Opts::default()
        },
    );
    let sent = message(&app, body).await;
    (store.charges(), sent)
}

#[tokio::test]
async fn a_non_stream_answer_is_charged_the_d10_sum() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let (charged, sent) = charges(&mock.base_url, NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(charged, vec![11], "3 + 2 + 1 + 5");
}

#[tokio::test]
async fn a_full_stream_is_charged_the_d10_sum() {
    let mock = MockOpenAi::spawn_raw_sse(text_stream()).await;
    let (charged, sent) = charges(&mock.base_url, STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(charged, vec![37], "19 + 1 + 2 + 15, from the last message_delta");
}

/// § 7.2: a stream cut after `message_start` charges the exact input and an output estimate:
/// `max(output_tokens, complete data records since message_start)`.
#[tokio::test]
async fn a_stream_cut_after_message_start_is_charged_input_plus_estimate() {
    let start = json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[],
        "usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}});
    let records = vec![
        record(&start),
        record(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        record(&json!({"type":"ping"})),
        record(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}})),
    ];
    let base_url = spawn_cut_stream(records.clone()).await;
    let (charged, sent) = charges(&base_url, STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(sent.text().ends_with(MESSAGES_TERMINAL_SSE_ERROR), "{}", sent.text());
    assert!(sent.text().starts_with(&records.concat()));
    assert_eq!(charged, vec![13], "10 input + max(1 output, 3 records)");
}

// --- hardening (D16, § 5, D19) ---------------------------------------------------------------

/// D16: an upstream redirect is a 502 in the Anthropic envelope, `location` is never forwarded,
/// and the redirect target sees no request (the operator key never reaches it).
#[tokio::test]
async fn an_upstream_redirect_is_a_502_and_the_target_sees_nothing() {
    let target = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    for status in [StatusCode::TEMPORARY_REDIRECT, StatusCode::FOUND, StatusCode::PERMANENT_REDIRECT] {
        let mock = MockOpenAi::spawn_redirect(status, format!("{}/v1/messages", target.base_url)).await;
        for body in [NON_STREAM, STREAM] {
            let sent = message(&app(&mock.base_url, Opts::default()), body).await;
            assert_eq!(assert_envelope(&sent, StatusCode::BAD_GATEWAY, "api_error").as_deref(), Some("upstream-unavailable"), "{status} {body}");
            assert!(sent.headers.get("location").is_none());
        }
        assert_eq!(mock.request_count(), 2);
    }
    assert_eq!(target.request_count(), 0, "the redirect target sees nothing");
}

/// § 5: a non-stream answer whose head arrives after the stream idle timeout is not cut.
#[tokio::test]
async fn a_slow_non_stream_head_is_not_cut_by_the_idle_timeout() {
    let mock = MockOpenAi::spawn_delayed_json(Duration::from_millis(1500), StatusCode::OK, OK_BODY).await;
    let app = app(
        &mock.base_url,
        Opts {
            idle: Duration::from_millis(500),
            ..Opts::default()
        },
    );
    let sent = message(&app, NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(sent.text(), OK_BODY);
}

/// D19: a duplicate top-level `stream` key is a 400 before admission, with interception off and
/// on; the upstream sees no request.
#[tokio::test]
async fn a_duplicate_stream_key_is_refused_before_admission() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    for ti in [None, ti(Arc::new(AllowAll))] {
        let store = RecordingStore::new();
        let app = app(
            &mock.base_url,
            Opts {
                limits: Some(limits(rules(Some(1), None, None), store.clone())),
                ti,
                ..Opts::default()
            },
        );
        for raw in [
            r#"{"model":"claude-x","max_tokens":16,"messages":[],"stream":false,"stream":true}"#,
            r#"{"model":"claude-x","max_tokens":16,"messages":[],"stream":true,"stream":false}"#,
        ] {
            let sent = message(&app, raw).await;
            assert_eq!(
                assert_envelope(&sent, StatusCode::BAD_REQUEST, "invalid_request_error").as_deref(),
                Some("invalid-request-body"),
                "{raw}"
            );
        }
        assert_eq!(store.checks(), 0, "no refusal reached the limits");
        assert_eq!(mock.request_count(), 0, "the upstream sees no request");
    }
}

// --- logs (§ 9) ------------------------------------------------------------------------------

/// § 9: the request line names the endpoint and carries the `anthropic-beta` values; the chat
/// line has no `anthropic_beta` field.
#[tokio::test]
async fn the_request_log_carries_the_endpoint_and_the_beta_values() {
    let (logs, _guard) = capture_logs();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let app = app(&mock.base_url, Opts::default());
    let extra = [("anthropic-beta", "a"), ("anthropic-beta", "b")];
    let sent = read(&app, request("POST", "/v1/messages", NON_STREAM, &[Cred::XApiKey(CALLER_KEY)], &extra)).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = logs.text();
    let line = text.lines().find(|l| l.contains("chat completion proxied")).unwrap_or_else(|| panic!("no proxied line:\n{text}"));
    assert!(line.contains("endpoint=\"messages\""), "{line}");
    assert!(line.contains("anthropic_beta=\"a,b\""), "{line}");

    let chat = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
    let sent = read(&app, request("POST", "/v1/chat/completions", chat, &[Cred::Bearer(CALLER_KEY)], &[("anthropic-beta", "a")])).await;
    assert_eq!(sent.status, StatusCode::OK);
    let text = logs.text();
    let line = text
        .lines()
        .find(|l| l.contains("chat completion proxied") && l.contains("endpoint=\"chat_completions\""))
        .unwrap_or_else(|| panic!("no chat line:\n{text}"));
    assert!(!line.contains("anthropic_beta"), "{line}");
}

/// D19 (fix round 1): with the tool policy on and interception off, a nested duplicate key is a
/// 400 `invalid-request-schema` in the Anthropic envelope before admission, in enforce and in
/// log-only mode, and the upstream sees no request. In log-only mode the policy forwards a turn it
/// cannot read, so only the D19 parse stops the body there. The same body without the duplicate
/// passes.
#[tokio::test]
async fn a_nested_duplicate_is_refused_with_the_policy_on_and_interception_off() {
    let dir = policy_dir();
    let dup = r#"{"model":"claude-x","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"tools":[{"name":"read_file","name":"blocked_rm","input_schema":{"type":"object"}}]}"#;
    for mode in [TenantMode::LogOnly, TenantMode::Enforce] {
        let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
        let store = RecordingStore::new();
        let app = app(
            &mock.base_url,
            Opts {
                limits: Some(limits(rules(Some(5), None, None), store.clone())),
                ti: None,
                tp: tool_policy_in(&dir, mode),
                ..Opts::default()
            },
        );
        let sent = message(&app, dup).await;
        assert_eq!(
            assert_envelope(&sent, StatusCode::BAD_REQUEST, "invalid_request_error").as_deref(),
            Some("invalid-request-schema"),
            "{mode:?}"
        );
        assert_eq!(store.checks(), 0, "nothing is admitted ({mode:?})");
        assert_eq!(mock.request_count(), 0, "the upstream sees no request ({mode:?})");
        let clean = message(&app, &tools_body(&["read_file"])).await;
        assert_eq!(clean.status, StatusCode::OK, "{mode:?}: {}", clean.text());
        assert_eq!(store.checks(), 1, "{mode:?}");
    }
}

/// D15 (fix round 1): the record cap of the route is `[messages] max_record_bytes`, not the
/// shared interception limit (1 MiB). With interception on, a record of about 8 KiB passes under
/// an 8 MiB route cap and is dropped under a 4 KiB route cap. The shared limit would pass it.
#[tokio::test]
async fn the_record_and_hold_caps_are_the_messages_limits() {
    let filler = format!("BIGRECORD{}", "x".repeat(8 * 1024));
    let start = json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-x","content":[],
        "usage":{"input_tokens":3,"output_tokens":1}}});
    let records = vec![
        record(&start),
        record(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        record(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":filler}})),
        record(&json!({"type":"content_block_stop","index":0})),
        record(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}})),
        record(&json!({"type":"message_stop"})),
    ];
    assert!(records[2].len() > 8 * 1024 && records[2].len() < CoreLimits::defaults().max_record_bytes);
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let run = |max_record_bytes: usize| {
        let app = app(
            &mock.base_url,
            Opts {
                messages: Some(MessagesSettings { max_record_bytes, ..settings() }),
                ti: ti(Arc::new(AllowAll)),
                ..Opts::default()
            },
        );
        async move { message(&app, STREAM).await }
    };
    let wide = run(8 * MIB).await;
    assert_eq!(wide.status, StatusCode::OK);
    assert_eq!(wide.text(), records.concat(), "under the route cap every record passes");
    let narrow = run(4096).await;
    assert_eq!(narrow.status, StatusCode::OK);
    assert!(!narrow.text().contains("BIGRECORD"), "over the route cap the record fails: {}", narrow.text());
    // With no call held, the over-cap record is dropped and the other records pass unchanged.
    let without_big: String = records.iter().enumerate().filter(|(i, _)| *i != 2).map(|(_, r)| r.as_str()).collect();
    assert_eq!(narrow.text(), without_big, "only the over-cap record is dropped");
}
