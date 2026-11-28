// SPDX-License-Identifier: Apache-2.0

//! A fake AI Gateway for the tests of the gateway model (SMA-851). It answers `POST /v1/messages`
//! with scripted replies and `GET /v1/models` with the models fixture. The SSE fixtures are the
//! files of the gateway tests (SMA-750 D21: one provenance rule).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use futures::StreamExt;
use futures::stream;
use serde_json::{Value, json};

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub const TEXT_STREAM: &str = include_str!("../../../paigasus-gateway/tests/fixtures/anthropic/text_stream.sse");
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub const PARALLEL_TOOL_USE: &str = include_str!("../../../paigasus-gateway/tests/fixtures/anthropic/parallel_tool_use.sse");
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub const PROMPT_TOO_LONG: &str = include_str!("../../../paigasus-gateway/tests/fixtures/anthropic/prompt_too_long.body.json");
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
const MODELS_BODY: &str = include_str!("../../../paigasus-gateway/tests/fixtures/anthropic/models.body.json");

/// One answer of `POST /v1/messages`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Clone)]
pub enum Reply {
    Sse(String),
    Error {
        status: u16,
        body: String,
        retry_after: Option<u64>,
    },
    /// Send the first frame of the SSE text, then keep the connection open with no more bytes.
    Stall(String),
    /// Send `count` ping frames, `gap` apart, after `message_start`, then the rest.
    Pings {
        sse: String,
        gap: Duration,
        count: usize,
    },
    Redirect {
        location: String,
    },
}

/// The answer of `GET /v1/models`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Clone)]
pub enum ModelsReply {
    Ok,
    Redirect { location: String },
}

/// One request of `POST /v1/messages`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Clone)]
pub struct Recorded {
    pub headers: axum::http::HeaderMap,
    pub body: serde_json::Value,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
struct Inner {
    replies: Vec<Reply>,
    models: ModelsReply,
    requests: Mutex<Vec<Recorded>>,
    models_requests: Mutex<Vec<HeaderMap>>,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct FakeGateway {
    pub addr: SocketAddr,
    inner: Arc<Inner>,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl FakeGateway {
    /// `POST /v1/messages` answers the replies in order and repeats the last one.
    /// `GET /v1/models` records its headers and answers 200 with `models.body.json`.
    pub async fn start(replies: Vec<Reply>) -> FakeGateway {
        Self::start_with_models(replies, ModelsReply::Ok).await
    }

    /// Like [`FakeGateway::start`], with a chosen answer for `GET /v1/models`.
    pub async fn start_with_models(replies: Vec<Reply>, models: ModelsReply) -> FakeGateway {
        assert!(!replies.is_empty(), "the fake gateway needs one reply at least");
        let inner = Arc::new(Inner {
            replies,
            models,
            requests: Mutex::default(),
            models_requests: Mutex::default(),
        });
        let app = Router::new().route("/v1/messages", post(messages)).route("/v1/models", get(models_route)).with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind the fake gateway");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve the fake gateway") });
        FakeGateway { addr, inner }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn models_requests(&self) -> Vec<axum::http::HeaderMap> {
        self.inner.models_requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn sse_response(body: Body) -> Response {
    Response::builder().status(StatusCode::OK).header("content-type", "text/event-stream").body(body).expect("response")
}

/// The first SSE frame (with its blank line) and the rest.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn split_first_frame(sse: &str) -> (String, String) {
    let end = sse.find("\n\n").map_or(sse.len(), |i| i + 2);
    (sse[..end].to_owned(), sse[end..].to_owned())
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
async fn messages(State(state): State<Arc<Inner>>, headers: HeaderMap, body: Bytes) -> Response {
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let index = {
        let mut requests = state.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        requests.push(Recorded { headers, body: parsed });
        requests.len() - 1
    };
    match state.replies[index.min(state.replies.len() - 1)].clone() {
        Reply::Sse(text) => sse_response(Body::from(text)),
        Reply::Error { status, body, retry_after } => {
            let mut builder = Response::builder().status(status).header("content-type", "application/json");
            if let Some(secs) = retry_after {
                builder = builder.header("retry-after", secs.to_string());
            }
            builder.body(Body::from(body)).expect("response")
        }
        Reply::Stall(text) => {
            let (first, _) = split_first_frame(&text);
            let stream = stream::once(async move { Ok::<_, Infallible>(Bytes::from(first)) }).chain(stream::pending());
            sse_response(Body::from_stream(stream))
        }
        Reply::Pings { sse, gap, count } => {
            let (first, rest) = split_first_frame(&sse);
            let mut chunks: Vec<(Duration, String)> = vec![(Duration::ZERO, first)];
            for _ in 0..count {
                chunks.push((gap, "event: ping\ndata: {\"type\": \"ping\"}\n\n".to_owned()));
            }
            chunks.push((gap, rest));
            let stream = stream::iter(chunks).then(|(wait, text)| async move {
                tokio::time::sleep(wait).await;
                Ok::<_, Infallible>(Bytes::from(text))
            });
            sse_response(Body::from_stream(stream))
        }
        Reply::Redirect { location } => redirect(&location),
    }
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn redirect(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header("location", location)
        .body(Body::empty())
        .expect("response")
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
async fn models_route(State(state): State<Arc<Inner>>, headers: HeaderMap) -> Response {
    state.models_requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(headers);
    match &state.models {
        ModelsReply::Ok => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(MODELS_BODY))
            .expect("response"),
        ModelsReply::Redirect { location } => redirect(location),
    }
}

/// A listener that counts the TCP connections it accepts. It answers nothing.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn counting_listener() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind the counting listener");
    let addr = listener.local_addr().expect("local addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        // The sockets stay open, so that a client that waits for an answer does not see a reset.
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            held.push(socket);
        }
    });
    (addr, accepted)
}

/// The SMA-750 D7 error envelope.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn error_body(error_type: &str, message: &str) -> String {
    json!({"type": "error", "error": {"type": error_type, "message": message}, "request_id": "req_paigasus_fixture"}).to_string()
}

/// The frames of an SSE text as `(event name, data JSON)`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn frames(sse: &str) -> Vec<(String, Value)> {
    sse.split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .map(|block| {
            let mut event = String::new();
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_owned();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = v.trim().to_owned();
                }
            }
            (event, serde_json::from_str(&data).expect("the data of a frame is JSON"))
        })
        .collect()
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn render(frames: &[(String, Value)]) -> String {
    frames.iter().map(|(event, data)| format!("event: {event}\ndata: {data}\n\n")).collect()
}

/// `TEXT_STREAM` with its four text deltas replaced by one.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn text_sse(text: &str) -> String {
    let mut out = Vec::new();
    let mut replaced = false;
    for (event, mut data) in frames(TEXT_STREAM) {
        if data["type"] == "content_block_delta" && data["delta"]["type"] == "text_delta" {
            if replaced {
                continue;
            }
            data["delta"]["text"] = json!(text);
            replaced = true;
        }
        out.push((event, data));
    }
    render(&out)
}

/// The `stop_reason` of the `message_delta` replaced.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn with_stop_reason(sse: &str, stop_reason: &str) -> String {
    let mut all = frames(sse);
    for (_, data) in &mut all {
        if data["type"] == "message_delta" {
            data["delta"]["stop_reason"] = json!(stop_reason);
        }
    }
    render(&all)
}

/// One tool call: `message_start`, block 0 of `PARALLEL_TOOL_USE` with the id and the name set,
/// one `input_json_delta` frame that holds `input`, and the closing frames.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn tool_use_sse(id: &str, name: &str, input: &Value) -> String {
    let mut out = Vec::new();
    let mut delta_written = false;
    for (event, mut data) in frames(PARALLEL_TOOL_USE) {
        if data.get("index").is_some_and(|i| *i != 0) {
            continue;
        }
        if data["type"] == "content_block_start" {
            data["content_block"]["id"] = json!(id);
            data["content_block"]["name"] = json!(name);
        }
        if data["type"] == "content_block_delta" {
            if delta_written {
                continue;
            }
            data["delta"]["partial_json"] = json!(input.to_string());
            delta_written = true;
        }
        out.push((event, data));
    }
    render(&out)
}
