// SPDX-License-Identifier: Apache-2.0

//! The gateway's client-facing error surface: the OpenAI-compatible error envelope and the
//! [`GatewayError`] enum every failed request renders through.
//!
//! SDKs (the OpenAI client libraries our callers use) branch on `error.type`, so the envelope
//! shape is a compatibility contract: `{"error":{"message","type","param","code"}}`. The
//! `message` is always a STATIC, caller-safe string — never IAM/token text, an upstream body, or
//! any other detail that could leak a credential or internal state (mirrors iam's `authn_status`
//! posture of a uniform, non-revealing error body).
//!
//! [`GatewayError`] carries only the auth-path cases G5 needs today; it is deliberately left open
//! for G6/G7 to EXTEND with egress (OpenAI-upstream) cases. Status codes here are binding; the
//! `type`/`code` strings are stable diagnostics.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The OpenAI-compatible error envelope: a single `error` object. `#[derive(Serialize)]` only —
/// the gateway never deserializes its own error bodies.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// The body of the OpenAI error envelope. `param` is always `null` for gateway-originated errors
/// (no request field is ever at fault in the auth path); `code` is a stable machine-readable
/// diagnostic or `null`. `r#type` serializes as `"type"` (serde strips the raw-identifier prefix).
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub message: String,
    pub r#type: String,
    pub param: Option<String>,
    pub code: Option<String>,
}

/// Every way a gateway request can fail with a client-facing error. Auth-path cases only for now;
/// G6/G7 extend this enum with egress cases (upstream unavailable, bad gateway, …). The HTTP
/// status each maps to is a binding part of the contract (see [`GatewayError::into_response`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayError {
    /// No usable `Authorization: Bearer <key>` credential on the request → 401.
    MissingBearer,
    /// The credential was rejected by IAM (invalid/expired/revoked/inactive) → 401.
    InvalidCredential,
    /// IAM authorized the caller's identity but denied the action → 403 (IAM audited the denial).
    AuthzDenied,
    /// The introspection succeeded but returned no scope PRN — a plumbing bug, surfaced as a
    /// distinct 500 diagnostic rather than a silent deny.
    MissingScope,
    /// IAM was unreachable or returned a transport/backend error → 503 (retryable).
    IamUnavailable,
    /// An unexpected internal fault (e.g. our self-query hit IAM's cross-principal exposure gate,
    /// which should be impossible) → 500. `message` stays generic; details are logged, not echoed.
    Internal,
}

impl GatewayError {
    /// The bound `(status, type, code)` triple plus a static, caller-safe message for each case.
    /// `type` is one of OpenAI's coarse error kinds (SDKs read it); `code` is a finer stable
    /// diagnostic or `None` (→ `null`).
    fn parts(self) -> (StatusCode, &'static str, Option<&'static str>, &'static str) {
        match self {
            GatewayError::MissingBearer => (
                StatusCode::UNAUTHORIZED,
                "invalid_request_error",
                Some("missing_authorization"),
                "Missing bearer credentials in the Authorization header.",
            ),
            GatewayError::InvalidCredential => (StatusCode::UNAUTHORIZED, "invalid_request_error", Some("invalid_api_key"), "Invalid API key."),
            GatewayError::AuthzDenied => (
                StatusCode::FORBIDDEN,
                "invalid_request_error",
                Some("insufficient_permissions"),
                "The caller is not permitted to perform this action.",
            ),
            GatewayError::MissingScope => (StatusCode::INTERNAL_SERVER_ERROR, "api_error", Some("missing_scope"), "Internal error."),
            GatewayError::IamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                Some("iam_unavailable"),
                "The authorization service is temporarily unavailable.",
            ),
            GatewayError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "api_error", None, "Internal error."),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, r#type, code, message) = self.parts();
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                message: message.to_owned(),
                r#type: r#type.to_owned(),
                param: None,
                code: code.map(str::to_owned),
            },
        };
        (status, Json(envelope)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn each_case_maps_to_its_bound_status() {
        assert_eq!(GatewayError::MissingBearer.into_response().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(GatewayError::InvalidCredential.into_response().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(GatewayError::AuthzDenied.into_response().status(), StatusCode::FORBIDDEN);
        assert_eq!(GatewayError::MissingScope.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(GatewayError::IamUnavailable.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(GatewayError::Internal.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn body_is_the_openai_envelope_shape() {
        let body = body_json(GatewayError::InvalidCredential.into_response()).await;
        // Exact OpenAI shape: a single `error` object with message/type/param/code.
        let err = body.get("error").expect("envelope has a top-level `error` object");
        assert!(err.get("message").and_then(|m| m.as_str()).is_some_and(|m| !m.is_empty()));
        assert_eq!(err["type"], "invalid_request_error");
        assert_eq!(err["code"], "invalid_api_key");
        assert!(err["param"].is_null(), "param is always null for gateway-originated errors");
    }

    #[tokio::test]
    async fn internal_case_serializes_a_null_code() {
        let body = body_json(GatewayError::Internal.into_response()).await;
        assert_eq!(body["error"]["type"], "api_error");
        assert!(body["error"]["code"].is_null(), "the Internal case carries a null code");
    }

    #[tokio::test]
    async fn error_message_never_echoes_internal_detail() {
        // Belt-and-braces: the static messages are caller-safe (no token/IAM text). Assert the
        // generic 500s do not reveal which internal path failed.
        for err in [GatewayError::MissingScope, GatewayError::Internal] {
            let body = body_json(err.into_response()).await;
            assert_eq!(body["error"]["message"], "Internal error.");
        }
    }
}
