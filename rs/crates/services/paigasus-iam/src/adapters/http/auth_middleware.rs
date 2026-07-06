// SPDX-License-Identifier: Apache-2.0

//! Bearer-enforcement middleware for the protected `/v1` HTTP surface (spec §7.4, D14).
//! Extracts `Authorization: Bearer <token>` — the ONLY accepted credential source (no
//! cookies, no query parameters) — runs it through `AuthnSvc::resolve(.., Enabled)` (which
//! JIT-provisions an unknown identity, AC 2), and, on success, inserts an [`AuthContext`]
//! request extension for downstream handlers. Every rejection short-circuits through the
//! shared `AuthnApiError` funnel (D12): status and body are always 401 `invalid_token`;
//! only the `WWW-Authenticate` challenge distinguishes a fully-absent `Authorization`
//! header (bare `Bearer`, RFC 6750 §3.1) from a present-but-rejected credential
//! (`Bearer error="invalid_token"`). The token itself is never logged (nothing here logs
//! it, and `AuthnError`'s own contract keeps claim/token material out of its `Display`).

use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use paigasus_iam_core::{AuthnError, TokenDefect};

use super::AppState;
use super::authn::AuthnApiError;
use crate::adapters::auth::{AuthContext, bearer_from_headers};
use crate::application::authenticate_token::Provisioning;

/// Enforces a valid bearer token on the request before it reaches a protected handler
/// (D14 — wired via `route_layer` inside `router()`, so the `oneshot` test harness
/// exercises it too). Applied only to the tenancy sub-router; `/healthz`, `/readyz`, and
/// `POST /v1/authn/introspect` stay outside it (spec §7.4).
pub async fn require_bearer(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let header_present = request.headers().contains_key(header::AUTHORIZATION);
    let Some(token) = bearer_from_headers(request.headers()) else {
        // Absent or unusable credentials get the SAME 401 status and `invalid_token` body
        // (the error contract stays uniform, D12); only the challenge header distinguishes
        // a client that sent NO credentials at all (bare `Bearer`, RFC 6750 §3.1 — no error
        // attribute when the request lacks authentication information) from one whose
        // header or token was present but rejected (`Bearer error="invalid_token"`).
        let mut response = AuthnApiError(AuthnError::InvalidToken(TokenDefect::Malformed)).into_response();
        if !header_present {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        return response;
    };

    match state.authn.resolve(&token, Provisioning::Enabled).await {
        Ok(principal) => {
            request.extensions_mut().insert(AuthContext {
                principal_id: principal.principal_id,
                issuer: principal.issuer,
                subject: principal.subject,
            });
            next.run(request).await
        }
        Err(err) => AuthnApiError(err).into_response(),
    }
}
