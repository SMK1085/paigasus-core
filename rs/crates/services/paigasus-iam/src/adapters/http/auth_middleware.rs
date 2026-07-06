// SPDX-License-Identifier: Apache-2.0

//! Bearer-enforcement middleware for the protected `/v1` HTTP surface (spec §7.4, D14).
//! Extracts `Authorization: Bearer <token>` — the ONLY accepted credential source (no
//! cookies, no query parameters) — runs it through `AuthnSvc::resolve(.., Enabled)` (which
//! JIT-provisions an unknown identity, AC 2), and, on success, inserts an [`AuthContext`]
//! request extension for downstream handlers. Every rejection short-circuits through the
//! shared `AuthnApiError` funnel (D12): a missing or malformed header, like any other
//! `InvalidToken`, is a 401 `invalid_token` with a `WWW-Authenticate` challenge; the token
//! itself is never logged (nothing here logs it, and `AuthnError`'s own contract keeps
//! claim/token material out of its `Display`).

use axum::extract::{Request, State};
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
    let Some(token) = bearer_from_headers(request.headers()) else {
        // A missing or malformed `Authorization` header is indistinguishable, to a caller,
        // from a rejected token: both are 401 `invalid_token` + `WWW-Authenticate` (D12).
        return AuthnApiError(AuthnError::InvalidToken(TokenDefect::Malformed)).into_response();
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
