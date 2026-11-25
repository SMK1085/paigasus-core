// SPDX-License-Identifier: Apache-2.0

//! `POST /v1/messages/count_tokens` and `GET /v1/models` (SMA-750 D11, D20).
//!
//! Both are plain pass-through. They do not use [`pipeline::proxy`](super::pipeline::proxy) and
//! have no [`Endpoint`](super::pipeline::Endpoint) descriptor. Each handler:
//!
//! 1. admits the request with `Limits::admit_for` when `[limits]` is on (one unit of the request
//!    rate; a refusal is the usual limit error);
//! 2. marks the charge guard no-charge, so no token is ever charged;
//! 3. sends the body (`count_tokens`) or no body (`models`) with the D5 `Forward`;
//! 4. returns the upstream status, `content-type` and body as they came, with the D8 headers.
//!
//! The body is never parsed. A non-2xx upstream answer is forwarded unchanged.
//!
//! `GET /v1/models` serves the Anthropic list only when the request carries `anthropic-version`
//! (D20). [`require_anthropic_version`] checks that OUTSIDE auth and the re-render layer, so an
//! OpenAI caller always gets [`openai_not_found`], with any credential or none, and no upstream
//! request is made.

use std::time::Instant;

use axum::Extension;
use axum::body::Body;
use axum::extract::{OriginalUri, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use super::AppState;
use super::bytes::EnvelopeBytes;
use super::error::GatewayError;
use super::pipeline::record_upstream_call;
use crate::adapters::anthropic::AnthropicRoute;
use crate::adapters::upstream::UpstreamResponse;
use crate::domain::{CallerContext, Credential};

/// What differs between the two routes.
struct Call {
    route: AnthropicRoute,
    method: Method,
    body: Bytes,
    /// The metric label and log field.
    endpoint: &'static str,
}

/// D20: the 404 in the OpenAI envelope. It has no `RenderedError` extension, so no re-render
/// layer changes it.
pub fn openai_not_found() -> Response {
    let body = r#"{"error":{"message":"Not found.","type":"invalid_request_error","param":null,"code":null}}"#;
    (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// D20: the outermost layer of the `GET /v1/models` route. A request without `anthropic-version`
/// gets [`openai_not_found`] before auth runs, so a bad or missing credential is not challenged.
pub async fn require_anthropic_version(req: Request, next: Next) -> Response {
    if req.headers().contains_key("anthropic-version") {
        next.run(req).await
    } else {
        openai_not_found()
    }
}

/// Proxy `POST /v1/messages/count_tokens`. [`EnvelopeBytes`] consumes the body, so it comes last.
pub(crate) async fn count_tokens(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, headers: HeaderMap, uri: OriginalUri, body: EnvelopeBytes) -> Response {
    let EnvelopeBytes(body) = body;
    let call = Call {
        route: AnthropicRoute::CountTokens,
        method: Method::POST,
        body,
        endpoint: "messages_count_tokens",
    };
    pass_through(state, caller, &headers, uri.query(), call).await
}

/// Proxy `GET /v1/models`. The `anthropic-version` check is the layer
/// [`require_anthropic_version`], which runs before this handler and before auth.
pub(crate) async fn list_models(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, headers: HeaderMap, uri: OriginalUri) -> Response {
    let call = Call {
        route: AnthropicRoute::Models,
        method: Method::GET,
        body: Bytes::new(),
        endpoint: "models",
    };
    pass_through(state, caller, &headers, uri.query(), call).await
}

async fn pass_through(state: AppState, caller: Option<Extension<CallerContext>>, headers: &HeaderMap, query: Option<&str>, call: Call) -> Response {
    let Call { route, method, body, endpoint } = call;
    let Some(Extension(caller)) = caller else {
        // Unreachable in practice: the auth middleware always attaches a CallerContext.
        return GatewayError::Internal.into_response();
    };
    // Boot validation makes a missing client unreachable; it is refused before the limits.
    let Some(client) = state.anthropic.as_ref() else {
        return GatewayError::Internal.into_response();
    };
    let fwd = client.forward(headers, query);

    // D11: one unit of the request rate; no token is charged (the guard drops with no charge).
    let mut guard = match &state.limits {
        None => None,
        Some(limits) => match limits.admit_for(&caller, endpoint).await {
            Ok(guard) => Some(guard),
            Err(refusal) => {
                tracing::debug!(principal = %caller.principal_prn, scope = %caller.scope_prn, reason = refusal.label(), endpoint, "anthropic request refused by a limit");
                return GatewayError::from(refusal).into_response();
            }
        },
    };
    if let Some(guard) = guard.as_mut() {
        guard.set_endpoint(endpoint);
        guard.mark_no_charge();
    }

    let started = Instant::now();
    let (response, status) = match client.send(route, method, body, false, &fwd).await {
        Ok(UpstreamResponse::Full { status, head, body }) => {
            record_upstream_call(status, started, endpoint);
            let content_type = head.content_type.clone().unwrap_or_else(|| HeaderValue::from_static("application/json"));
            let mut resp = (status, [(header::CONTENT_TYPE, content_type)], body).into_response();
            head.apply(resp.headers_mut());
            (resp, status)
        }
        Ok(UpstreamResponse::Stream { status, head, stream }) => {
            record_upstream_call(status, started, endpoint);
            let content_type = head.content_type.clone().unwrap_or_else(|| HeaderValue::from_static("application/json"));
            let mut resp = (status, [(header::CONTENT_TYPE, content_type)], Body::from_stream(stream)).into_response();
            head.apply(resp.headers_mut());
            (resp, status)
        }
        Err(err) => {
            let resp = GatewayError::from(err).into_response();
            let status = resp.status();
            record_upstream_call(status, started, endpoint);
            (resp, status)
        }
    };

    // One structured line per request. Never the body or the key.
    let beta = fwd.beta_values();
    match &caller.credential {
        Credential::ApiKey { key_id } => tracing::info!(
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "api_key",
            key_id = %key_id,
            endpoint,
            anthropic_beta = beta.as_deref(),
            "anthropic request proxied"
        ),
        Credential::Oidc => tracing::info!(
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "oidc",
            endpoint,
            anthropic_beta = beta.as_deref(),
            "anthropic request proxied"
        ),
    }
    drop(guard);
    response
}
