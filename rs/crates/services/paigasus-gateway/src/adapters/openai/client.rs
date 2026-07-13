// SPDX-License-Identifier: Apache-2.0

//! The outbound OpenAI egress client.
//!
//! [`OpenAiClient`] forwards a caller's chat-completion request to the OpenAI upstream and returns
//! the upstream's status plus either the full body (non-stream) or an UNBUFFERED byte stream
//! (stream). It is the sole holder of the real OpenAI API key.
//!
//! ## Split timeouts (D6 / §5)
//! A single global `.timeout()` would cap a legitimately long stream, so the client instead
//! splits the budget across the three phases the caller configures:
//! - **connect** — [`reqwest::ClientBuilder::connect_timeout`]: bounds the TCP+TLS handshake.
//! - **idle (between bytes)** — [`reqwest::ClientBuilder::read_timeout`]: the maximum gap between
//!   successive reads. It bounds a *stalled* stream without killing a long *active* one, and
//!   applies to both paths.
//! - **first byte** — applied as a per-request `.timeout()` on the NON-stream request only (a
//!   non-stream completion should return within it). The stream path deliberately sets NO overall
//!   `.timeout()`, relying on connect + idle instead.
//!
//! ## Header & secret hygiene (§5) — load-bearing
//! Every upstream request is built FRESH from a curated header set (`Authorization`,
//! `Content-Type`, `Accept`) — the caller's inbound headers are never forwarded (the client is
//! only ever handed the request `body`, never the caller's headers). The real key is exposed via
//! [`ExposeSecret::expose_secret`] ONLY at the instant the `Authorization` value is built, is held
//! as a [`SecretString`] otherwise, and never appears in `Debug`/log renders.
//!
//! ## Cancel-on-drop
//! reqwest cancels the in-flight upstream request when the [`reqwest::Response`] (and the byte
//! stream borrowed from it) is dropped. The stream path returns that stream boxed but otherwise
//! un-held, so when axum drops the response body on client disconnect (G7/G8) the upstream request
//! is cancelled — do not stash the response anywhere that outlives the returned stream.

use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};

use crate::config::OpenAiConfig;

/// The boxed, UNBUFFERED chunk stream the [`ChatResponse::Stream`] path yields — each item is a
/// raw upstream body chunk exactly as it arrived off the socket (no line-buffering, no
/// re-framing). `'static + Send` so it can be handed to axum's `Body::from_stream` and outlive
/// this call. Dropping it cancels the upstream request (cancel-on-drop).
pub type OpenAiByteStream = futures::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>;

/// The upstream response, shaped so G7 can handle both paths. A NON-2xx upstream is NOT an error —
/// it arrives here as [`ChatResponse::Full`] with the upstream status + body so G7 forwards
/// OpenAI's own error envelope verbatim.
pub enum ChatResponse {
    /// Non-stream: the upstream status and the fully-buffered response body.
    Full {
        /// The upstream HTTP status, forwarded verbatim by G7.
        status: StatusCode,
        /// The complete upstream response body.
        body: Bytes,
    },
    /// Stream: the upstream status and an UNBUFFERED chunk stream (SSE), forwarded as-is.
    Stream {
        /// The upstream HTTP status (the SSE stream begins after the response head).
        status: StatusCode,
        /// The unbuffered upstream byte stream; dropping it cancels the request.
        stream: OpenAiByteStream,
    },
}

impl std::fmt::Debug for ChatResponse {
    // Manual `Debug`: `OpenAiByteStream` is not `Debug`, and we would not want to consume/print
    // the streamed body anyway. Only the discriminant + status are shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatResponse::Full { status, body } => f.debug_struct("ChatResponse::Full").field("status", status).field("body_len", &body.len()).finish(),
            ChatResponse::Stream { status, .. } => f.debug_struct("ChatResponse::Stream").field("status", status).finish_non_exhaustive(),
        }
    }
}

/// Errors from the OpenAI egress client — reqwest send/connect/read failures ONLY. A non-2xx
/// upstream is deliberately NOT an error (it is returned as a [`ChatResponse::Full`] so G7
/// forwards it verbatim). G7 maps these to HTTP: connect/timeout are upstream-unreachable/slow
/// (→ 502/504), a build failure is a boot-time fault, and a bare transport error is a bad gateway.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiError {
    /// The `reqwest::Client` could not be constructed (TLS backend init, invalid builder config) —
    /// a boot-time fault, surfaced when G7 builds the client at startup.
    #[error("failed to build the OpenAI HTTP client")]
    Build(#[source] reqwest::Error),
    /// Failed to establish the connection to the upstream (DNS / TCP / TLS handshake).
    #[error("failed to connect to the OpenAI upstream")]
    Connect(#[source] reqwest::Error),
    /// A configured timeout fired (connect, first-byte, or idle-between-bytes).
    #[error("the OpenAI upstream request timed out")]
    Timeout(#[source] reqwest::Error),
    /// Any other transport-level failure talking to the upstream.
    #[error("transport error talking to the OpenAI upstream")]
    Transport(#[source] reqwest::Error),
}

impl OpenAiError {
    /// Classify a request-time `reqwest::Error` (from `send`/`bytes`) into connect / timeout /
    /// transport so G7 can map each to the right HTTP status. NOT a `From` impl on purpose — a
    /// client-*build* error must not be silently classified as a transport error.
    fn from_request(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            OpenAiError::Timeout(e)
        } else if e.is_connect() {
            OpenAiError::Connect(e)
        } else {
            OpenAiError::Transport(e)
        }
    }
}

/// The outbound OpenAI client: a shared `reqwest::Client` (connection-pooled; cheap to clone),
/// the upstream base URL, the real API key, and the first-byte budget applied per non-stream
/// request.
///
/// `Debug` is derived: `SecretString`'s own `Debug` redacts the key, so the derived output never
/// contains it (locked in by [`tests::debug_never_leaks_the_api_key`]).
#[derive(Clone, Debug)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    /// Overall deadline for a NON-stream completion (not applied to the stream path).
    first_byte_timeout: Duration,
}

impl OpenAiClient {
    /// Build the client from the OpenAI config and the three timeout budgets (threaded in
    /// explicitly by G7 from `GatewayConfig`, since [`OpenAiConfig`] alone does not carry them).
    ///
    /// `connect_timeout` and `stream_idle_timeout` (the read/between-bytes gap) are baked into the
    /// underlying `reqwest::Client`; `first_byte_timeout` is stored and applied per non-stream
    /// request. NO global client `.timeout()` is set (it would cap a legitimate long stream).
    pub fn new(cfg: &OpenAiConfig, connect_timeout: Duration, first_byte_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, OpenAiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .read_timeout(stream_idle_timeout)
            .build()
            .map_err(OpenAiError::Build)?;
        Ok(Self {
            http,
            // Trim a trailing slash so `{base_url}/v1/chat/completions` never doubles up.
            base_url: cfg.base_url.trim_end_matches('/').to_owned(),
            api_key: cfg.api_key.clone(),
            first_byte_timeout,
        })
    }

    /// Forward a chat-completion request upstream. `body` is the caller's ORIGINAL raw request
    /// bytes (byte-lossless passthrough); `stream` selects the path (G7 reads it off the parsed
    /// [`ChatCompletionRequest`](crate::adapters::http::dto::ChatCompletionRequest)).
    ///
    /// The request is built FRESH with only the curated headers below — the caller's inbound
    /// headers are never present here to forward. The real key is exposed solely to build the
    /// `Authorization` value.
    pub async fn chat_completion(&self, body: Bytes, stream: bool) -> Result<ChatResponse, OpenAiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        // `Accept` advertises the path: SSE for stream, JSON otherwise.
        let accept = if stream { "text/event-stream" } else { "application/json" };

        let mut request = self
            .http
            .post(url)
            // Real key exposed ONLY here, to build the header value; dropped immediately after.
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key.expose_secret()))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, accept)
            .body(body);

        // First-byte budget bounds a non-stream completion; the stream path relies on
        // connect + idle timeouts only (a global timeout would kill a long active stream).
        if !stream {
            request = request.timeout(self.first_byte_timeout);
        }

        let response = request.send().await.map_err(OpenAiError::from_request)?;
        let status = response.status();

        if stream {
            // UNBUFFERED: hand back the raw chunk stream as-is — no `.collect()`, no line-buffering.
            // Boxing erases the opaque `bytes_stream()` type but does not buffer; dropping the
            // boxed stream drops the response and cancels the upstream request (cancel-on-drop).
            Ok(ChatResponse::Stream {
                status,
                stream: response.bytes_stream().boxed(),
            })
        } else {
            let body = response.bytes().await.map_err(OpenAiError::from_request)?;
            Ok(ChatResponse::Full { status, body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(api_key: &str) -> OpenAiClient {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from(api_key.to_string()),
        };
        OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds")
    }

    #[test]
    fn debug_never_leaks_the_api_key() {
        // Secret hygiene (§5): the real key must never surface in a `Debug`/log render. The field
        // is a `SecretString`, whose own `Debug` redacts — this locks that in against a future
        // refactor that might swap the field type.
        let secret = "sk-super-secret-real-key-abc123";
        let client = test_client(secret);
        let rendered = format!("{client:?}");
        assert!(!rendered.contains(secret), "the API key must never appear in Debug output: {rendered}");
    }

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        // A configured trailing slash must not produce `//v1/...`.
        let client = test_client("sk-x");
        assert_eq!(client.base_url, "https://api.openai.com");
    }
}
