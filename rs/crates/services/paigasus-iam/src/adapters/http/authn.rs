// SPDX-License-Identifier: Apache-2.0

//! Authn HTTP surface: `POST /v1/authn/introspect` plus the dedicated `AuthnError` →
//! response funnel (spec §6.3, D12). This funnel is deliberately SEPARATE from the tenancy
//! `ApiError`/`ErrorClass` machinery — that path can only express 400/404/409/500, while
//! authn needs 401 (+ `WWW-Authenticate`), 403 subcodes, and 503. Bodies reuse the same
//! `{"error":{code,message}}` envelope; every message is STATIC per code — no claim
//! values, token fragments, or upstream error text ever reach the response (spec §6.3).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router, extract::State};
use paigasus_iam_core::AuthnError;
use serde_json::json;

use super::AppState;
use super::dto::{IntrospectBody, IntrospectResponseDto};

/// The `WWW-Authenticate` challenge attached to every 401 (RFC 6750 §3).
const BEARER_CHALLENGE: &str = "Bearer error=\"invalid_token\"";

/// Wraps an `AuthnError` so handlers can return it via `?` (see `From` below) and axum
/// renders the spec §6.3 mapping. Task 11's middleware reuses this for its own rejects.
pub struct AuthnApiError(pub AuthnError);

impl From<AuthnError> for AuthnApiError {
    fn from(err: AuthnError) -> Self {
        AuthnApiError(err)
    }
}

impl IntoResponse for AuthnApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self.0 {
            AuthnError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, "invalid_token", "invalid bearer token"),
            AuthnError::IdentityNotProvisioned => (StatusCode::FORBIDDEN, "identity_not_provisioned", "identity not provisioned"),
            AuthnError::ProvisioningFailed(_) => (StatusCode::FORBIDDEN, "provisioning_failed", "provisioning failed"),
            AuthnError::PrincipalInactive => (StatusCode::FORBIDDEN, "principal_inactive", "principal inactive"),
            AuthnError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable", "authentication backend unavailable"),
            AuthnError::Backend(_) => {
                // Debug carries the boxed repository/infra source (never token or claim
                // material by `AuthnError`'s own contract) — logged here, never surfaced.
                tracing::error!(error = ?self.0, "internal error handling an authn request");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
            }
        };

        let mut response = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
        if matches!(self.0, AuthnError::InvalidToken(_)) {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(BEARER_CHALLENGE));
        }
        response
    }
}

/// The introspect sub-router. Merged alongside (NOT inside) the tenancy `/v1` sub-router
/// in `super::router` so Task 11's bearer-enforcement layer, which wraps the tenancy
/// sub-router only, never covers this route (middleware-exempt, spec §7.4).
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/authn/introspect", post(introspect))
}

/// `POST /v1/authn/introspect` (spec §7.2): the full `PrincipalContext` for a presented
/// token. READ-ONLY (D10): `AuthenticateToken::introspect` resolves with
/// `Provisioning::Disabled`, so an unknown identity is 403 `identity_not_provisioned` and
/// this unauthenticated endpoint never has a user-creation side effect. The body carries
/// the credential itself and is NEVER logged (nothing here logs, and an oversized token is
/// rejected by the validator's own length cap — no pre-filtering, no echo).
async fn introspect(State(state): State<AppState>, Json(body): Json<IntrospectBody>) -> Result<Json<IntrospectResponseDto>, AuthnApiError> {
    let ctx = state.authn.introspect(&body.token).await?;
    Ok(Json(ctx.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use paigasus_iam_core::{ProvisioningDefect, TokenDefect};

    /// Renders the funnel and returns `(status, WWW-Authenticate header, json body)`.
    async fn rendered(err: AuthnError) -> (StatusCode, Option<String>, serde_json::Value) {
        let response = AuthnApiError(err).into_response();
        let status = response.status();
        let challenge = response.headers().get(header::WWW_AUTHENTICATE).map(|v| v.to_str().unwrap().to_string());
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, challenge, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn invalid_token_is_401_with_bearer_challenge() {
        // Every `TokenDefect` renders identically — the defect kind never leaks (spec §6.3).
        for defect in [TokenDefect::Malformed, TokenDefect::Expired, TokenDefect::Oversized, TokenDefect::BadSignature] {
            let (status, challenge, body) = rendered(AuthnError::InvalidToken(defect)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(challenge.as_deref(), Some("Bearer error=\"invalid_token\""));
            assert_eq!(body["error"]["code"], "invalid_token");
            assert_eq!(body["error"]["message"], "invalid bearer token");
        }
    }

    #[tokio::test]
    async fn forbidden_family_is_403_with_stable_codes_and_no_challenge() {
        let cases = [
            (AuthnError::IdentityNotProvisioned, "identity_not_provisioned", "identity not provisioned"),
            (AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail), "provisioning_failed", "provisioning failed"),
            (AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict), "provisioning_failed", "provisioning failed"),
            (AuthnError::PrincipalInactive, "principal_inactive", "principal inactive"),
        ];
        for (err, code, message) in cases {
            let (status, challenge, body) = rendered(err).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(challenge, None, "403s carry no WWW-Authenticate challenge");
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["message"], message);
        }
    }

    #[tokio::test]
    async fn unavailable_is_503() {
        let (status, challenge, body) = rendered(AuthnError::Unavailable).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(challenge, None);
        assert_eq!(body["error"]["code"], "unavailable");
        assert_eq!(body["error"]["message"], "authentication backend unavailable");
    }

    #[tokio::test]
    async fn backend_is_500_and_never_leaks_details() {
        let (status, challenge, body) = rendered(AuthnError::Backend("secret db detail".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(challenge, None);
        assert_eq!(body["error"]["code"], "internal");
        assert_eq!(body["error"]["message"], "internal error");
        assert!(!body.to_string().contains("secret db detail"), "backend detail must never reach the response");
    }
}
