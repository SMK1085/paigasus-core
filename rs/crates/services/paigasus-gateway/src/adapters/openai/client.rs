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
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};

use crate::adapters::upstream::{Provider, UpstreamError, UpstreamResponse, build_client, classify};
use crate::config::OpenAiConfig;

/// The upstream routes the client can call (SMA-769 D7). A closed set: a caller never passes a
/// free path string, so no request value can steer the upstream URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamRoute {
    ChatCompletions,
    Responses,
}

impl UpstreamRoute {
    /// The path appended to the configured base URL.
    pub fn path(self) -> &'static str {
        match self {
            UpstreamRoute::ChatCompletions => "/v1/chat/completions",
            UpstreamRoute::Responses => "/v1/responses",
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
    pub fn new(cfg: &OpenAiConfig, connect_timeout: Duration, first_byte_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, UpstreamError> {
        let http = build_client(Provider::OpenAi, cfg.extra_ca_bundle_path.as_deref(), connect_timeout, stream_idle_timeout)?;
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
    pub async fn chat_completion(&self, body: Bytes, stream: bool) -> Result<UpstreamResponse, UpstreamError> {
        self.send(UpstreamRoute::ChatCompletions, body, stream).await
    }

    /// Forward a request to `route` (SMA-769 D7). Same headers, timeouts and cancel-on-drop for
    /// every route; see [`Self::chat_completion`] for the hygiene rules.
    pub async fn send(&self, route: UpstreamRoute, body: Bytes, stream: bool) -> Result<UpstreamResponse, UpstreamError> {
        let url = format!("{}{}", self.base_url, route.path());
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

        let response = request.send().await.map_err(|e| classify(Provider::OpenAi, e))?;
        let status = response.status();
        // D16: the client never follows a redirect, and `Location` is never forwarded.
        if status.is_redirection() {
            return Err(UpstreamError::Redirect { provider: Provider::OpenAi, status });
        }

        if stream {
            // UNBUFFERED: hand back the raw chunk stream as-is — no `.collect()`, no line-buffering.
            // Boxing erases the opaque `bytes_stream()` type but does not buffer; dropping the
            // boxed stream drops the response and cancels the upstream request (cancel-on-drop).
            Ok(UpstreamResponse::Stream {
                status,
                stream: response.bytes_stream().boxed(),
            })
        } else {
            let body = response.bytes().await.map_err(|e| classify(Provider::OpenAi, e))?;
            Ok(UpstreamResponse::Full { status, body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn test_client(api_key: &str) -> OpenAiClient {
        client_with_bundle(api_key, None).expect("client builds")
    }

    fn client_with_bundle(api_key: &str, extra_ca_bundle_path: Option<String>) -> Result<OpenAiClient, UpstreamError> {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from(api_key.to_string()),
            extra_ca_bundle_path,
        };
        OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300))
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

    #[tokio::test]
    async fn an_openai_redirect_is_a_502_error() {
        use axum::Router;
        use axum::http::{HeaderMap, HeaderValue, StatusCode as AxStatus};
        use axum::routing::any;

        let app = Router::new().fallback(any(|| async {
            let mut h = HeaderMap::new();
            h.insert("location", HeaderValue::from_static("http://127.0.0.1:1/elsewhere"));
            (AxStatus::TEMPORARY_REDIRECT, h)
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = OpenAiConfig {
            base_url: format!("http://{addr}"),
            api_key: SecretString::from("sk-x".to_string()),
            extra_ca_bundle_path: None,
        };
        let client = OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).unwrap();
        for stream in [false, true] {
            let err = client.send(UpstreamRoute::ChatCompletions, Bytes::from_static(b"{}"), stream).await.expect_err("a 3xx is an error");
            assert!(
                matches!(err, UpstreamError::Redirect { provider: Provider::OpenAi, status } if status == StatusCode::TEMPORARY_REDIRECT),
                "{err:?}"
            );
        }
    }
}
