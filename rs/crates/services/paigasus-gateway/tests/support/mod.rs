// SPDX-License-Identifier: Apache-2.0

//! Reusable in-process mock OpenAI upstream for the gateway's egress tests (G6 here; G7 reuses
//! it). It binds an ephemeral `127.0.0.1` port, serves `POST /v1/chat/completions`, RECORDS the
//! request headers + body it received (so a test can assert the caller's key never arrived and the
//! injected real key did), and returns either a canned JSON body (non-stream) or an SSE stream of
//! ordered `data:` frames. Model: the ephemeral-listener pattern from `paigasus-iam`'s
//! `tests/support::start_mock_idp` / `tests/grpc_health.rs`.

// Not every test binary that includes this module uses every helper; silence the per-binary
// dead-code lint rather than sprinkle `#[allow]` on each item.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;

/// What a request the mock received looked like (captured before any response is produced).
#[derive(Clone)]
pub struct RecordedRequest {
    /// The exact headers reqwest sent upstream — asserted against for header hygiene.
    pub headers: HeaderMap,
    /// The raw request body bytes (proves byte-lossless passthrough).
    pub body: Bytes,
}

impl RecordedRequest {
    /// The value of a request header as a `&str`, or `None` if absent / non-ASCII.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// How the mock should answer `POST /v1/chat/completions`.
#[derive(Clone)]
enum MockResponse {
    /// Non-stream: return `status` with the given JSON body.
    Json { status: StatusCode, body: String },
    /// Stream: return `200 text/event-stream`, emitting each element as a `data: <event>\n\n`
    /// frame IN ORDER, streamed (never buffered into one blob server-side).
    Sse { events: Vec<String> },
}

struct MockState {
    response: MockResponse,
    recorded: Mutex<Option<RecordedRequest>>,
}

/// A running mock OpenAI server. Its `base_url` is what you feed into [`OpenAiConfig`]; the server
/// task is aborted on drop.
pub struct MockOpenAi {
    /// `http://127.0.0.1:<port>` — pass straight to `OpenAiConfig::base_url`.
    pub base_url: String,
    state: Arc<MockState>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for MockOpenAi {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl MockOpenAi {
    /// Start a mock that returns a canned JSON body with the given status (the non-stream path).
    pub async fn spawn_json(status: StatusCode, body: impl Into<String>) -> Self {
        Self::spawn(MockResponse::Json { status, body: body.into() }).await
    }

    /// Start a mock that streams the given events IN ORDER as SSE `data:` frames (the stream path).
    pub async fn spawn_sse(events: Vec<String>) -> Self {
        Self::spawn(MockResponse::Sse { events }).await
    }

    async fn spawn(response: MockResponse) -> Self {
        let state = Arc::new(MockState { response, recorded: Mutex::new(None) });
        let router = Router::new().route("/v1/chat/completions", post(handle)).with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");

        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("mock openai server");
        });

        MockOpenAi { base_url, state, handle }
    }

    /// The request the mock last recorded (headers + body), or `None` if none arrived yet.
    pub fn recorded(&self) -> Option<RecordedRequest> {
        self.state.recorded.lock().expect("recorded lock not poisoned").clone()
    }
}

/// Records the inbound request, then answers per the configured [`MockResponse`]. `Bytes` is the
/// body-consuming extractor, so it comes last.
async fn handle(State(state): State<Arc<MockState>>, headers: HeaderMap, body: Bytes) -> Response {
    *state.recorded.lock().expect("recorded lock not poisoned") = Some(RecordedRequest {
        headers: headers.clone(),
        body: body.clone(),
    });

    match &state.response {
        MockResponse::Json { status, body } => (*status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.clone()).into_response(),
        MockResponse::Sse { events } => {
            // Stream each event as its own frame via `Body::from_stream` — genuinely streamed
            // (frame by frame) rather than concatenated into a single buffered body, so the client
            // side exercises real incremental delivery.
            let frames = events.clone();
            let stream = futures::stream::iter(frames.into_iter().map(|e| Ok::<Bytes, std::convert::Infallible>(Bytes::from(format!("data: {e}\n\n")))));
            Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(stream))
                .expect("build sse response")
        }
    }
}
