// SPDX-License-Identifier: Apache-2.0

//! The outbound Anthropic egress client (SMA-750 § 5).
//!
//! Every upstream request is built fresh. The operator key goes in `x-api-key`. Of the caller's
//! headers, only the `anthropic-*` ones go upstream (I2). The URL is `base_url + route path`, so
//! no request value can change the host or the path (I3). The query is appended as raw text.
//! A `3xx` answer is an error and `location` is never followed (D16).

use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use secrecy::ExposeSecret;

use crate::adapters::upstream::{Provider, ResponseHead, UpstreamError, UpstreamResponse, build_client, classify};
use crate::config::AnthropicConfig;

/// The upstream routes the client can call. A closed set, so a caller never picks a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicRoute {
    Messages,
    CountTokens,
    Models,
}

impl AnthropicRoute {
    /// The path appended to the configured base URL.
    pub fn path(self) -> &'static str {
        match self {
            AnthropicRoute::Messages => "/v1/messages",
            AnthropicRoute::CountTokens => "/v1/messages/count_tokens",
            AnthropicRoute::Models => "/v1/models",
        }
    }
}

/// What the gateway forwards of one caller request: the raw query and the `anthropic-*` headers.
#[derive(Debug, Clone, Default)]
pub struct Forward {
    query: Option<String>,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl Forward {
    /// The `anthropic-beta` values joined with `,`, or `None` when there is none.
    pub fn beta_values(&self) -> Option<String> {
        let values: Vec<&str> = self.headers.iter().filter(|(n, _)| n.as_str() == "anthropic-beta").filter_map(|(_, v)| v.to_str().ok()).collect();
        if values.is_empty() { None } else { Some(values.join(",")) }
    }
}

/// The outbound Anthropic client. `Debug` is derived: `SecretString` redacts the key.
#[derive(Clone, Debug)]
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    /// The `x-api-key` value, checked in `new` and marked sensitive.
    api_key: HeaderValue,
    default_version: HeaderValue,
    non_stream_timeout: Duration,
}

impl AnthropicClient {
    /// Build the client. The read timeout is the larger of the stream idle timeout and the
    /// non-stream timeout, so a slow non-stream head does not hit the idle limit (§ 5).
    pub fn new(cfg: &AnthropicConfig, connect_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, UpstreamError> {
        let non_stream_timeout = Duration::from_secs(cfg.non_stream_timeout_secs);
        let http = build_client(Provider::Anthropic, cfg.extra_ca_bundle_path.as_deref(), connect_timeout, stream_idle_timeout.max(non_stream_timeout))?;
        // `AnthropicConfig::validate` checks both values at boot. A direct caller gets an error too.
        let default_version = HeaderValue::from_str(&cfg.default_version).map_err(|_| UpstreamError::InvalidHeaderValue {
            provider: Provider::Anthropic,
            setting: "default_version",
        })?;
        // The real key is exposed only to build this header value.
        let mut api_key = HeaderValue::from_str(cfg.api_key.expose_secret()).map_err(|_| UpstreamError::InvalidHeaderValue {
            provider: Provider::Anthropic,
            setting: "api_key",
        })?;
        api_key.set_sensitive(true);
        Ok(Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_owned(),
            api_key,
            default_version,
            non_stream_timeout,
        })
    }

    /// Pick what to forward from the caller's headers: every `anthropic-*` value in order, plus
    /// `default_version` when the caller sent no `anthropic-version`.
    pub fn forward(&self, headers: &HeaderMap, query: Option<&str>) -> Forward {
        let mut out: Vec<(HeaderName, HeaderValue)> = Vec::new();
        for name in headers.keys().filter(|n| n.as_str().starts_with("anthropic-")) {
            for value in headers.get_all(name) {
                out.push((name.clone(), value.clone()));
            }
        }
        if !out.iter().any(|(n, _)| n.as_str() == "anthropic-version") {
            out.push((HeaderName::from_static("anthropic-version"), self.default_version.clone()));
        }
        Forward {
            query: query.map(str::to_owned),
            headers: out,
        }
    }

    /// Send one request upstream. `body` is the caller's raw body. `stream` picks the SSE path
    /// (no total timeout) or the buffered path (`non_stream_timeout`).
    pub async fn send(&self, route: AnthropicRoute, method: Method, body: Bytes, stream: bool, fwd: &Forward) -> Result<UpstreamResponse, UpstreamError> {
        let mut url = format!("{}{}", self.base_url, route.path());
        if let Some(q) = fwd.query.as_deref().filter(|q| !q.is_empty()) {
            url.push('?');
            url.push_str(q);
        }
        let is_get = method == Method::GET;
        let accept = if stream { "text/event-stream" } else { "application/json" };

        let mut request = self.http.request(method, url).header("x-api-key", self.api_key.clone()).header(ACCEPT, accept);
        if !is_get {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        for (name, value) in &fwd.headers {
            request = request.header(name.clone(), value.clone());
        }
        if !stream {
            request = request.timeout(self.non_stream_timeout);
        }

        let response = request.send().await.map_err(|e| classify(Provider::Anthropic, e))?;
        let status: StatusCode = response.status();
        // D16: a redirect is never followed and `location` is never kept.
        if status.is_redirection() {
            return Err(UpstreamError::Redirect {
                provider: Provider::Anthropic,
                status,
            });
        }
        let head = ResponseHead::from_headers(response.headers());

        if stream {
            Ok(UpstreamResponse::Stream {
                status,
                head,
                stream: response.bytes_stream().boxed(),
            })
        } else {
            let body = response.bytes().await.map_err(|e| classify(Provider::Anthropic, e))?;
            Ok(UpstreamResponse::Full { status, head, body })
        }
    }
}
