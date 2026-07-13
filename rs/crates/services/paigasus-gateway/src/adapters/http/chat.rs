// SPDX-License-Identifier: Apache-2.0

//! The `POST /v1/chat/completions` proxy handler — the integration crux of the M0 slice.
//!
//! By the time this runs, the G5 [`require_iam_auth`](super::auth::require_iam_auth) middleware has
//! already authenticated + authorized the caller and attached a [`CallerContext`] to the request
//! extensions. This handler's job is narrow and security-sensitive:
//!
//! 1. Read the full request body as raw [`Bytes`] (the body-size limit is enforced by the
//!    [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer, which fails oversized bodies with
//!    a `413` before this handler is reached).
//! 2. Parse a COPY of the body into [`ChatCompletionRequest`] ONLY to read `model` (logging) and
//!    `stream` (which egress path to take). A JSON parse failure → `400`.
//! 3. **Egress hygiene (load-bearing):** forward ONLY the raw body bytes to the OpenAI client —
//!    NEVER the caller's inbound headers. The caller's `Authorization: Bearer <paigasus-key>` is
//!    still on the inbound request; it must never reach OpenAI. The client builds fresh upstream
//!    headers from its own real key, so simply never handing it caller headers guarantees this.
//! 4. Forward the upstream response verbatim: a full body (non-stream) or an UNBUFFERED SSE stream.
//!    A non-2xx upstream (OpenAI's own error envelope) passes through unchanged.
//!
//! ## Mid-stream terminal SSE error (spec §5)
//! Once a stream's `200 OK` head has been sent and `data:` frames are flowing, the HTTP status can
//! no longer change — so a mid-stream upstream failure cannot become a `502`. Instead the stream is
//! adapted so that on the FIRST inner error it emits one terminal
//! `data: {"error":…}` SSE event and then ends. The adapted stream is returned DIRECTLY (never
//! spawned onto a detached task) so reqwest's cancel-on-drop stays intact: if the downstream client
//! disconnects, axum drops the response body, which drops the inner reqwest stream and cancels the
//! upstream request (G8 proves this abort).

use std::convert::Infallible;
use std::time::Instant;

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};

use super::AppState;
use super::error::GatewayError;
use crate::adapters::http::dto::ChatCompletionRequest;
use crate::adapters::openai::{ChatResponse, OpenAiByteStream};
use crate::domain::CallerContext;

/// The single terminal SSE event emitted when a stream fails mid-flight. Static, caller-safe (no
/// upstream detail), and shaped like the OpenAI error envelope wrapped in an SSE `data:` frame so
/// SDKs that parse the stream see a well-formed terminal error.
const TERMINAL_SSE_ERROR: &str = "data: {\"error\":{\"message\":\"upstream stream error\",\"type\":\"api_error\",\"param\":null,\"code\":\"upstream_error\"}}\n\n";

/// Proxy a chat-completion request to the OpenAI upstream.
///
/// Extractor order matters: [`State`] and [`Extension`] are `FromRequestParts` (they read only the
/// head), while [`Bytes`] is `FromRequest` (it consumes the body) and so must come LAST. The
/// [`Bytes`] extractor also honours the [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer,
/// returning `413` for an over-limit body before this function body runs.
///
/// `CallerContext` is taken as `Option<Extension<_>>` for defence in depth: the G5 middleware
/// guarantees its presence on any request routed here, so its absence is an unreachable internal
/// bug surfaced as a `500` (rendered through the OpenAI envelope) rather than axum's default
/// extension-missing response.
pub async fn chat_completions(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, body: Bytes) -> Response {
    let Some(Extension(caller)) = caller else {
        // Unreachable in practice — the auth middleware always attaches a CallerContext.
        return GatewayError::Internal.into_response();
    };

    // Parse a COPY only to read `model` + `stream`; the ORIGINAL `body` bytes flow upstream verbatim.
    let dto: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(dto) => dto,
        Err(_) => return GatewayError::BadRequestBody.into_response(),
    };
    let model = dto.model;
    let stream = dto.stream;

    let started = Instant::now();
    // EGRESS HYGIENE: only the raw body bytes cross the boundary — never the caller's headers.
    let result = state.openai.chat_completion(body, stream).await;

    let (response, status) = match result {
        Ok(ChatResponse::Full { status, body }) => {
            // Forward the upstream status + body VERBATIM, including a non-2xx OpenAI error envelope.
            let resp = (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
            (resp, status)
        }
        Ok(ChatResponse::Stream { status, stream }) => {
            let resp = if status.is_success() {
                // Success stream: SSE passthrough with the mid-stream terminal-error adapter.
                (status, [(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(terminal_sse_error_stream(stream))).into_response()
            } else {
                // Non-2xx `stream:true` request: OpenAI answers with a JSON error body (NOT SSE), so
                // forward it as an error passthrough — no SSE terminal-error wrapping applies.
                (status, [(header::CONTENT_TYPE, "application/json")], Body::from_stream(stream)).into_response()
            };
            (resp, status)
        }
        Err(err) => {
            // Connect/transport/build → 502; timeout → 504 (see `GatewayError::from`).
            let resp = GatewayError::from(err).into_response();
            let status = resp.status();
            (resp, status)
        }
    };

    // One structured line per request — model/stream/status/latency/principal ONLY. NEVER the
    // prompt, messages, body, or the OpenAI key. `principal`/`key_id` are the caller's
    // service-account PRN + non-secret API-key id: internal *service-account* identifiers (not
    // end-user PII, and not the key secret), retained deliberately for request attribution/audit —
    // the "never PII" bar is about prompt/message content, not the SA the call was made as.
    tracing::info!(
        model = %model,
        stream = stream,
        status = status.as_u16(),
        latency_ms = started.elapsed().as_millis() as u64,
        principal = %caller.principal_prn,
        key_id = %caller.key_id,
        "chat completion proxied"
    );

    response
}

/// The small state machine [`terminal_sse_error_stream`] unfolds over: still forwarding upstream
/// chunks, or terminated (after emitting the terminal error event, or after the upstream ended).
enum StreamState {
    Streaming(OpenAiByteStream),
    Done,
}

/// Adapt an [`OpenAiByteStream`] into a body stream that forwards each upstream chunk UNBUFFERED
/// and, on the FIRST upstream error, emits one terminal [`TERMINAL_SSE_ERROR`] event and ends.
///
/// Items are `Result<Bytes, Infallible>`: the outgoing stream never errors (a mid-stream upstream
/// failure becomes a data event, not a transport error), which keeps `Body::from_stream` from
/// aborting the response body. The inner stream is owned by the unfold state and dropped when this
/// stream is dropped — preserving reqwest's cancel-on-drop of the upstream request.
fn terminal_sse_error_stream(inner: OpenAiByteStream) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    futures::stream::unfold(StreamState::Streaming(inner), |state| async move {
        match state {
            StreamState::Streaming(mut inner) => match inner.next().await {
                // Forward each chunk exactly as it arrived (unbuffered).
                Some(Ok(chunk)) => Some((Ok(chunk), StreamState::Streaming(inner))),
                // First upstream error: emit the terminal event, then end.
                Some(Err(_)) => Some((Ok(Bytes::from_static(TERMINAL_SSE_ERROR.as_bytes())), StreamState::Done)),
                // Clean upstream end.
                None => None,
            },
            StreamState::Done => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_stream_forwards_chunks_then_ends_cleanly() {
        // A clean upstream stream (no error) is forwarded chunk-for-chunk with no terminal event.
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: a\n\n")), Ok(Bytes::from_static(b"data: b\n\n"))]).boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner).map(|r| r.unwrap()).collect().await;
        let assembled: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
        assert_eq!(String::from_utf8(assembled).unwrap(), "data: a\n\ndata: b\n\n");
    }

    #[tokio::test]
    async fn terminal_stream_emits_terminal_event_on_first_error_then_stops() {
        // After the first upstream error the adapter emits exactly one terminal event and ends —
        // any items the upstream would have produced afterwards are never observed.
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: a\n\n"))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner).map(|r| r.unwrap()).collect().await;
        let assembled: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
        let text = String::from_utf8(assembled).unwrap();
        assert!(text.starts_with("data: a\n\n"), "the pre-error chunk is forwarded: {text}");
        assert!(text.ends_with(TERMINAL_SSE_ERROR), "the stream ends with exactly the terminal SSE error event: {text}");
        assert_eq!(text.matches("\"code\":\"upstream_error\"").count(), 1, "exactly one terminal error event");
    }

    /// Produce a real `reqwest::Error` (there is no public constructor) by forcing a connection
    /// failure against an unroutable address.
    async fn make_reqwest_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .timeout(std::time::Duration::from_millis(50))
            .send()
            .await
            .expect_err("connection to a dead port fails")
    }
}
