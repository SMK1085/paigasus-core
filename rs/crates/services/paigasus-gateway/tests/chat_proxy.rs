// SPDX-License-Identifier: Apache-2.0

//! End-to-end proxy tests for `POST /v1/chat/completions`: the REAL G5 auth middleware + the REAL
//! G7 handler + the REAL G6 [`OpenAiClient`], driven against the in-process
//! [`support::MockOpenAi`] upstream via `tower`'s `oneshot`.
//!
//! ## Fake IAM, not a mock gRPC server (deliberate — noted for review)
//! The IAM side is a **fake** `Iam` implemented in this test, NOT a live tonic IAM server. The real
//! `IamClient`'s wire behaviour and the D9 self-query metadata are already unit-proven in G4
//! (`adapters::iam::client`) and G5 (`adapters::http::auth`), so re-standing a gRPC server here
//! would only re-test the transport. A fake keeps these handler-integration tests fast and
//! hermetic while still exercising the real middleware → handler → egress path.
//!
//! Covered: missing/invalid bearer → 401; authz denied → 403; allowed non-stream verbatim (incl.
//! a non-2xx passthrough); malformed body → 400; streaming (ordered, `text/event-stream`);
//! mid-stream error → terminal SSE event (status stays 200); oversized body → 413; IAM down → 503;
//! and the load-bearing egress-hygiene assertion (the caller's credentials never reach upstream).
//! Client-abort/cancel-on-drop is G8's — see the note on [`mid_stream_error_emits_terminal_sse_event`].

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use futures::StreamExt; // for `bytes_stream().next()`
use secrecy::SecretString;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::Status;
use tower::ServiceExt; // for `oneshot`

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{Iam, IamError};
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
use support::MockOpenAi;

/// The real OpenAI key the gateway is configured with — what the upstream MUST see.
const REAL_KEY: &str = "sk-real-openai-server-key-77aa";
/// The caller's own paigasus bearer — what the upstream must NEVER see.
const CALLER_KEY: &str = "sk-caller-secret";
const CALLER_SA: &str = "prn:paigasus:iam:default:sa/gw-caller";
const CALLER_SCOPE: &str = "prn:paigasus:iam:default:scope/team-a";
const CALLER_KEY_ID: &str = "key-abc123";

/// The gateway's default 1 MiB body cap for the tests that are not exercising the limit.
const ONE_MIB: usize = 1_048_576;

const NON_STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
const STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;

// ---- fake IAM ---------------------------------------------------------------------------------

/// The introspect outcome a [`FakeIam`] should produce.
#[derive(Clone, Copy)]
enum Introspect {
    /// An `active` caller (SA + scope + key_id).
    Active,
    /// A rejected credential (gRPC `Unauthenticated` → 401).
    Unauthenticated,
    /// IAM unreachable/backend error (gRPC `Unavailable` → 503).
    Unavailable,
}

/// A canned `Iam` for the proxy tests — no live IAM. `introspect` selects the auth outcome and
/// `allow` the self-query authorization result.
struct FakeIam {
    introspect: Introspect,
    allow: bool,
}

impl FakeIam {
    /// The common case: an active, authorized caller.
    fn allowed() -> Self {
        FakeIam {
            introspect: Introspect::Active,
            allow: true,
        }
    }
}

#[async_trait::async_trait]
impl Iam for FakeIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        match self.introspect {
            Introspect::Active => Ok(active_response()),
            Introspect::Unauthenticated => Err(IamError::Rpc(Status::unauthenticated("invalid key"))),
            Introspect::Unavailable => Err(IamError::Rpc(Status::unavailable("iam is down"))),
        }
    }

    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(self.allow)
    }

    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        unreachable!("the chat path (require_iam_auth) never calls introspect_token")
    }
}

/// An `active` introspect response for the canonical caller.
fn active_response() -> IntrospectApiKeyResponse {
    IntrospectApiKeyResponse {
        principal_prn: CALLER_SA.to_owned(),
        status: "active".to_owned(),
        key_id: CALLER_KEY_ID.to_owned(),
        expires_at: None,
        memberships: Vec::new(),
        role_grants: Vec::new(),
        scope_prn: CALLER_SCOPE.to_owned(),
    }
}

// ---- app + request builders -------------------------------------------------------------------

/// Assemble the real router over a fake IAM and a real `OpenAiClient` pointed at `base_url`, holding
/// [`REAL_KEY`].
fn app_for(fake: FakeIam, base_url: String, max_request_bytes: usize) -> Router {
    app_for_with_streaming(fake, base_url, max_request_bytes, true)
}

/// As [`app_for`], but with `stream_enabled` set explicitly — used by the streaming-rejection
/// test (SMA-505 AC 3) to build a router with streaming disabled.
fn app_for_with_streaming(fake: FakeIam, base_url: String, max_request_bytes: usize, stream_enabled: bool) -> Router {
    let cfg = OpenAiConfig {
        base_url,
        api_key: SecretString::from(REAL_KEY.to_string()),
        extra_ca_bundle_path: None,
    };
    let openai = OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds");
    let state = AppState {
        iam: Arc::new(fake),
        openai: Arc::new(openai),
        max_request_bytes,
        capabilities: paigasus_gateway::service_info::Capabilities { chat_stream: stream_enabled },
    };
    router(state)
}

/// A `POST /v1/chat/completions` request with the given body and optional bearer token.
fn chat_request(body: &str, bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/v1/chat/completions").header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    builder.body(Body::from(body.to_owned())).expect("build request")
}

// ---- auth-path rows (real middleware) ---------------------------------------------------------

#[tokio::test]
async fn missing_bearer_is_401() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(mock.recorded().is_none(), "an unauthenticated request must never reach the upstream");
}

#[tokio::test]
async fn invalid_bearer_is_401() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(
        FakeIam {
            introspect: Introspect::Unauthenticated,
            allow: true,
        },
        mock.base_url.clone(),
        ONE_MIB,
    );
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some("bad-key"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(mock.recorded().is_none());
}

#[tokio::test]
async fn authz_denied_is_403() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(
        FakeIam {
            introspect: Introspect::Active,
            allow: false,
        },
        mock.base_url.clone(),
        ONE_MIB,
    );
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(mock.recorded().is_none(), "a denied caller must never reach the upstream");
}

#[tokio::test]
async fn iam_unavailable_is_503() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(
        FakeIam {
            introspect: Introspect::Unavailable,
            allow: true,
        },
        mock.base_url.clone(),
        ONE_MIB,
    );
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(mock.recorded().is_none());
}

// ---- request-body handling --------------------------------------------------------------------

#[tokio::test]
async fn invalid_json_body_is_400() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request("not valid json", Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(mock.recorded().is_none(), "a malformed body must never reach the upstream");
}

#[tokio::test]
async fn oversized_body_is_413() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    // A tiny cap; the body below far exceeds it. Auth still passes first (M0: auth before 413),
    // then the handler's `Bytes` extractor rejects the over-limit body.
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), 64);
    let big = format!(r#"{{"model":"gpt-4o","messages":[{{"role":"user","content":"{}"}}]}}"#, "x".repeat(500));
    let resp = app.oneshot(chat_request(&big, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(mock.recorded().is_none(), "an over-limit body must never reach the upstream");
}

// ---- non-stream passthrough -------------------------------------------------------------------

#[tokio::test]
async fn allowed_non_stream_returns_upstream_body_verbatim() {
    let canned = r#"{"id":"chatcmpl-xyz","object":"chat.completion","choices":[{"index":0}]}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, canned).await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/json");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, Bytes::from(canned), "the upstream body is forwarded verbatim");

    // The caller's raw body reached the upstream byte-for-byte.
    assert_eq!(mock.recorded().unwrap().body, Bytes::from(NON_STREAM_BODY));
}

#[tokio::test]
async fn non_2xx_upstream_passes_through_verbatim() {
    // OpenAI's own 429 rate-limit envelope must be forwarded unchanged — same status, same body.
    let envelope = r#"{"error":{"message":"Rate limit reached","type":"rate_limit_error"}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::TOO_MANY_REQUESTS, envelope).await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, Bytes::from(envelope), "the upstream error envelope passes through verbatim");
}

// ---- streaming --------------------------------------------------------------------------------

#[tokio::test]
async fn stream_request_returns_ordered_event_stream() {
    let events = vec!["a".to_string(), "b".to_string(), "c".to_string(), "[DONE]".to_string()];
    let mock = MockOpenAi::spawn_sse(events.clone()).await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);
    let resp = app.oneshot(chat_request(STREAM_BODY, Some(CALLER_KEY))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "text/event-stream", "the stream path advertises SSE");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let expected: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), expected, "SSE frames arrive in the order the upstream emitted them");
}

/// Mid-stream upstream failure: the `200 OK` head is already committed, so the status can no longer
/// change — the adapter forwards the frames it received, then ends with the terminal SSE error
/// event. (Client-abort / reqwest cancel-on-drop is G8's; the handler returns the stream directly
/// precisely so that abort survives.)
#[tokio::test]
async fn mid_stream_error_emits_terminal_sse_event() {
    let base_url = spawn_truncated_sse_server().await;
    let app = app_for(FakeIam::allowed(), base_url, ONE_MIB);
    let resp = app.oneshot(chat_request(STREAM_BODY, Some(CALLER_KEY))).await.unwrap();

    // Status stayed 200 (the head committed before the stream broke).
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "text/event-stream");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.starts_with("data: first\n\ndata: second\n\n"), "the pre-error frames are forwarded in order: {text}");
    assert!(text.contains(r#""code":"upstream-error""#), "the stream ends with the terminal SSE error event: {text}");
    assert!(text.trim_end().ends_with("}}"), "the terminal error event is the last frame on the wire: {text}");
}

// ---- streaming toggle (SMA-505) ----------------------------------------------------------------

/// SMA-505 AC 3, gateway side: with streaming disabled a `stream: true` request is refused with
/// `400` and `param: "stream"`, and the upstream is never called. Needs no database — this
/// crate's harness drives the router via `oneshot` against a fake IAM and a fake upstream.
#[tokio::test]
async fn a_stream_request_is_refused_when_streaming_is_disabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for_with_streaming(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB, false);

    let resp = app.clone().oneshot(chat_request(STREAM_BODY, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["param"], "stream", "the refused field must be named: {json}");
    assert_eq!(json["error"]["code"], "streaming-disabled");
    assert!(mock.recorded().is_none(), "a refused streaming request must never reach the upstream");

    // Then repeat with `"stream": false` and assert the request still reaches the upstream —
    // otherwise this would pass against an implementation that broke chat completions entirely.
    let resp = app.oneshot(chat_request(NON_STREAM_BODY, Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a non-streaming request must be unaffected by streaming being disabled");
    assert!(mock.recorded().is_some(), "a non-streaming request must still reach the upstream when streaming is disabled");
}

// ---- egress hygiene (load-bearing) ------------------------------------------------------------

#[tokio::test]
async fn egress_never_forwards_caller_credentials() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);

    // The inbound request carries the caller's OWN bearer AND a cookie — neither may reach upstream.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .header(header::COOKIE, "session=abc-caller-cookie")
        .body(Body::from(NON_STREAM_BODY.to_owned()))
        .expect("build request");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let recorded = mock.recorded().expect("the upstream received the proxied request");
    // Upstream saw the REAL OpenAI key — NEVER the caller's paigasus bearer.
    assert_eq!(recorded.header("authorization"), Some(format!("Bearer {REAL_KEY}").as_str()), "upstream must see the real OpenAI key");
    assert_ne!(
        recorded.header("authorization"),
        Some(format!("Bearer {CALLER_KEY}").as_str()),
        "the caller's bearer must never reach the upstream"
    );
    // No caller cookie leaked upstream.
    assert!(recorded.header("cookie").is_none(), "the caller cookie must never reach the upstream");
    // The raw body flowed upstream byte-for-byte.
    assert_eq!(recorded.body, Bytes::from(NON_STREAM_BODY), "the caller's raw body is forwarded verbatim");
}

// ---- raw truncated-stream upstream (mid-stream error) -----------------------------------------

/// Bind an ephemeral port and serve ONE connection with a chunked `text/event-stream` response that
/// sends two `data:` frames then CLOSES the socket WITHOUT the terminating `0\r\n\r\n` chunk. reqwest
/// yields the two frames and then errors on the premature EOF — exactly the mid-stream failure the
/// handler's terminal-SSE-error adapter must handle. Returns the `http://…` base URL.
async fn spawn_truncated_sse_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            // Best-effort drain of the (small) request head+body so the client finishes sending.
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf).await;

            let frame1 = "data: first\n\n";
            let frame2 = "data: second\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{l1:x}\r\n{frame1}\r\n{l2:x}\r\n{frame2}\r\n",
                l1 = frame1.len(),
                l2 = frame2.len(),
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.flush().await;
            // Drop `sock` here -> the connection closes mid-stream (no terminating chunk).
        }
    });
    format!("http://{addr}")
}

// ---- client-abort cancel-on-drop (G8) ---------------------------------------------------------

/// Client disconnect mid-stream CANCELS the upstream request — proven end-to-end. A REAL
/// `reqwest::Client` starts a `stream:true` request through the gateway (bound on a real TCP
/// port), reads the first SSE frame (so the stream is live and the gateway↔upstream connection is
/// established + holding), then DROPS the response. The drop must propagate: axum drops the
/// gateway's response body → the `unfold` holding the reqwest stream drops → reqwest cancels the
/// upstream request → the mock's body future drops → its Drop guard flips the cancel flag.
///
/// ## Determinism (no fixed sleep)
/// The mock holds the stream open on `Poll::Pending` FOREVER after one frame, so the flag can flip
/// ONLY as a consequence of the drop — never on its own. We first assert the flag is still false
/// (nothing has been dropped), then, AFTER the drop, poll it inside a 2 s `tokio::time::timeout`
/// around a 10 ms retry loop: the test FAILS (rather than hangs) if cancellation never propagates,
/// and passes as soon as it does (typically within a few ms). The bound is a generous ceiling, not
/// a timed guess. A regression that detached the stream onto its own task (breaking cancel-on-drop)
/// would leave the flag false and fail this test at the 2 s bound.
#[tokio::test]
async fn client_abort_cancels_upstream_request() {
    let (mock, cancelled) = MockOpenAi::spawn_abortable_stream().await;
    let app = app_for(FakeIam::allowed(), mock.base_url.clone(), ONE_MIB);

    // Bind the gateway on a real ephemeral port and serve it in the background — a real socket is
    // required so reqwest's client disconnect actually reaches axum (unlike `oneshot`).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind gateway port");
    let gw_addr = listener.local_addr().expect("gateway addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // A REAL reqwest client starts the stream with a valid bearer.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(STREAM_BODY)
        .send()
        .await
        .expect("gateway responds to the streaming request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let mut stream = resp.bytes_stream();
    // Read the FIRST frame — proves the stream started and the upstream is connected + holding.
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("the first chunk must arrive within 2s")
        .expect("the stream yields a first chunk")
        .expect("the first chunk is not a transport error");
    assert!(first.starts_with(b"data: "), "the first SSE frame is forwarded from the upstream: {first:?}");

    // Sanity: nothing has been dropped yet, so the upstream must still be live.
    assert!(!cancelled.load(Ordering::SeqCst), "the upstream must still be live before the client disconnects");

    // Client disconnect: dropping the stream drops the reqwest response and closes the connection.
    drop(stream);

    // The drop must propagate all the way to the upstream. Poll the flag, bounded — FAIL (not
    // hang) if cancel-on-drop never propagates.
    let propagated = tokio::time::timeout(Duration::from_secs(2), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(propagated.is_ok(), "client-abort must cancel the upstream request within 2s (cancel-on-drop propagated end-to-end)");
    assert!(cancelled.load(Ordering::SeqCst), "the mock's Drop guard flipped the cancel flag");

    server.abort();
    drop(mock);
}
