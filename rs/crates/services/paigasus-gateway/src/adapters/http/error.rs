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
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use paigasus_observability::Retryable;
use paigasus_observability::correlation::RETRYABLE_HEADER;
use serde::Serialize;

use crate::adapters::openai::OpenAiError;

/// The OpenAI-compatible error envelope: a single `error` object. `#[derive(Serialize)]` only —
/// the gateway never deserializes its own error bodies.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// The body of the OpenAI error envelope. `param` names the request field at fault when there is
/// one — only `StreamingDisabled` sets it today (SMA-505 D9); every auth- and egress-path error
/// leaves it `null`, because no request field is at fault in those. `code` is a stable
/// machine-readable diagnostic drawn from the canonical registry (`common/v1/error.proto`,
/// SMA-504) — every [`GatewayError`] case emits one, so `code` is no longer `null` in practice;
/// the field stays `Option` because [`ErrorEnvelope`] has no other case that omits it today.
/// `r#type` serializes as `"type"`.
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
#[cfg_attr(test, derive(strum::EnumIter))]
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
    /// which should be impossible) → 500. The reachable auth-path 500s (`MissingScope`, an
    /// unexpected authz error mapped to this case) are logged at the middleware with non-secret
    /// context (`principal_prn`, `key_id`); the response body stays generic.
    Internal,
    // ---- egress (OpenAI-upstream) cases (G7) -------------------------------------------------
    /// The request body was not valid JSON — the gateway could not parse `model`/`stream` → 400.
    BadRequestBody,
    /// The OpenAI upstream could not be reached (connect/transport/build failure) → 502.
    UpstreamUnavailable,
    /// The OpenAI upstream did not respond within a configured timeout → 504.
    UpstreamTimeout,
    /// Streaming is disabled by configuration and the request asked for it → 400. Carries
    /// `param: "stream"` so the client sees exactly which field was refused (SMA-505 D9).
    StreamingDisabled,
}

/// Map an [`OpenAiError`] (egress send/connect/timeout/build failure) to its client-facing
/// [`GatewayError`]: a fired timeout is a `504`; every other transport/connect/build failure is a
/// `502` (upstream unreachable/misbuilt). A non-2xx upstream is NOT an `OpenAiError` (G6 returns it
/// as a `ChatResponse::Full` for verbatim passthrough), so it never reaches this mapping.
impl From<OpenAiError> for GatewayError {
    fn from(err: OpenAiError) -> Self {
        match err {
            OpenAiError::Timeout(_) => GatewayError::UpstreamTimeout,
            OpenAiError::Connect(_) | OpenAiError::Transport(_) | OpenAiError::Build(_) => GatewayError::UpstreamUnavailable,
        }
    }
}

impl GatewayError {
    /// The bound `(status, type, code, param)` plus a static, caller-safe message for each case.
    /// `type` is one of OpenAI's coarse error kinds (SDKs read it); `code` is the canonical
    /// registry spelling every case now emits (SMA-504 — `Internal` no longer emits `None`);
    /// `param` names the request field at fault, or `None`.
    fn parts(self) -> (StatusCode, &'static str, Option<&'static str>, Option<&'static str>, &'static str) {
        match self {
            GatewayError::MissingBearer => (
                StatusCode::UNAUTHORIZED,
                "invalid_request_error",
                Some("missing-authorization"),
                None,
                "Missing bearer credentials in the Authorization header.",
            ),
            GatewayError::InvalidCredential => (StatusCode::UNAUTHORIZED, "invalid_request_error", Some("invalid-api-key"), None, "Invalid API key."),
            GatewayError::AuthzDenied => (
                StatusCode::FORBIDDEN,
                "invalid_request_error",
                Some("insufficient-permissions"),
                None,
                "The caller is not permitted to perform this action.",
            ),
            GatewayError::MissingScope => (StatusCode::INTERNAL_SERVER_ERROR, "api_error", Some("missing-scope"), None, "Internal error."),
            GatewayError::IamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                Some("iam-unavailable"),
                None,
                "The authorization service is temporarily unavailable.",
            ),
            GatewayError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "api_error", Some("internal"), None, "Internal error."),
            GatewayError::BadRequestBody => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("invalid-request-body"),
                None,
                "The request body is not valid JSON.",
            ),
            GatewayError::UpstreamUnavailable => (
                StatusCode::BAD_GATEWAY,
                "api_error",
                Some("upstream-unavailable"),
                None,
                "The upstream model provider is temporarily unavailable.",
            ),
            GatewayError::UpstreamTimeout => (StatusCode::GATEWAY_TIMEOUT, "api_error", Some("upstream-timeout"), None, "The upstream model provider timed out."),
            GatewayError::StreamingDisabled => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("streaming-disabled"),
                Some("stream"),
                "Streamed completions are not enabled on this deployment.",
            ),
        }
    }

    /// Whether a client should retry (spec D4). `true` ONLY for transient dependency failures.
    /// The two internal cases are `Unknown` rather than `false`: the gateway cannot tell a
    /// transient fault from a bug, and a confident `false` there would be worse than the
    /// status-class guess this replaces.
    pub fn retryable(self) -> Retryable {
        match self {
            Self::IamUnavailable | Self::UpstreamUnavailable | Self::UpstreamTimeout => Retryable::Yes,
            Self::Internal | Self::MissingScope => Retryable::Unknown,
            Self::MissingBearer | Self::InvalidCredential | Self::AuthzDenied | Self::BadRequestBody | Self::StreamingDisabled => Retryable::No,
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, r#type, code, param, message) = self.parts();
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                message: message.to_owned(),
                r#type: r#type.to_owned(),
                param: param.map(str::to_owned),
                code: code.map(str::to_owned),
            },
        };
        let mut response = (status, Json(envelope)).into_response();
        response.headers_mut().insert(RETRYABLE_HEADER, HeaderValue::from_static(self.retryable().as_wire()));
        response
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
        assert_eq!(GatewayError::BadRequestBody.into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(GatewayError::UpstreamUnavailable.into_response().status(), StatusCode::BAD_GATEWAY);
        assert_eq!(GatewayError::UpstreamTimeout.into_response().status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(GatewayError::StreamingDisabled.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn openai_error_maps_timeout_to_504_and_the_rest_to_502() {
        // A real `reqwest::Error` has no public constructor; wrap a genuine one (fresh per variant,
        // since `reqwest::Error` is not `Clone`) and assert the mapping. The request-time
        // classification INTO these variants is G6's concern.
        assert_eq!(GatewayError::from(OpenAiError::Timeout(dead_port_error().await)), GatewayError::UpstreamTimeout);
        assert_eq!(GatewayError::from(OpenAiError::Connect(dead_port_error().await)), GatewayError::UpstreamUnavailable);
        assert_eq!(GatewayError::from(OpenAiError::Transport(dead_port_error().await)), GatewayError::UpstreamUnavailable);
        assert_eq!(GatewayError::from(OpenAiError::Build(dead_port_error().await)), GatewayError::UpstreamUnavailable);
    }

    /// Produce a genuine `reqwest::Error` (no public constructor) by dialing an unroutable port.
    async fn dead_port_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .timeout(std::time::Duration::from_millis(50))
            .send()
            .await
            .expect_err("connection to a dead port fails")
    }

    #[tokio::test]
    async fn body_is_the_openai_envelope_shape() {
        let body = body_json(GatewayError::InvalidCredential.into_response()).await;
        // Exact OpenAI shape: a single `error` object with message/type/param/code.
        let err = body.get("error").expect("envelope has a top-level `error` object");
        assert!(err.get("message").and_then(|m| m.as_str()).is_some_and(|m| !m.is_empty()));
        assert_eq!(err["type"], "invalid_request_error");
        assert_eq!(err["code"], "invalid-api-key");
        assert!(
            err["param"].is_null(),
            "InvalidCredential names no request field at fault, so param is null (StreamingDisabled is the one case that sets it, SMA-505 D9)"
        );
    }

    /// SMA-504 rename 16: `Internal` emitted a NULL code, so a client could not distinguish it
    /// from any other `api_error`. It now emits the registry's `internal`.
    #[tokio::test]
    async fn internal_case_serializes_the_canonical_internal_code() {
        let body = body_json(GatewayError::Internal.into_response()).await;
        assert_eq!(body["error"]["type"], "api_error");
        assert_eq!(body["error"]["code"], "internal");
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

    /// AC 6: every code `GatewayError` can emit is declared in the canonical registry.
    /// Enumerated via `strum::EnumIter` off the type itself, so a variant added later is
    /// included automatically — there is no second list that can be left un-extended.
    #[test]
    fn every_gateway_code_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        for err in GatewayError::iter() {
            let (_, _, code, _, _) = err.parts();
            let code = code.expect("SMA-504: the Internal case no longer emits a null code");
            assert!(ErrorReason::from_wire_reason(code).is_some(), "GatewayError::{err:?} emits {code:?}, absent from common/v1/error.proto");
        }
    }

    /// D4's table, asserted exhaustively so a new variant must state its retryability.
    #[test]
    fn retryability_matches_the_documented_table() {
        use paigasus_observability::Retryable;
        use strum::IntoEnumIterator;

        for err in GatewayError::iter() {
            let want = match err {
                GatewayError::IamUnavailable | GatewayError::UpstreamUnavailable | GatewayError::UpstreamTimeout => Retryable::Yes,
                GatewayError::Internal | GatewayError::MissingScope => Retryable::Unknown,
                _ => Retryable::No,
            };
            assert_eq!(err.retryable(), want, "{err:?}");
        }
    }

    #[tokio::test]
    async fn every_error_response_carries_a_retryable_header() {
        assert_eq!(GatewayError::IamUnavailable.into_response().headers()["paigasus-retryable"], "true");
        assert_eq!(GatewayError::InvalidCredential.into_response().headers()["paigasus-retryable"], "false");
        assert_eq!(GatewayError::Internal.into_response().headers()["paigasus-retryable"], "unknown");
    }

    /// AC 3: the OpenAI envelope's key set is EXACTLY message/type/param/code. SDKs branch on
    /// `type`, so this shape is a binding external contract — the ids ride in headers precisely
    /// so this assertion keeps holding.
    #[tokio::test]
    async fn the_openai_error_object_key_set_is_unchanged() {
        let body = body_json(GatewayError::InvalidCredential.into_response()).await;
        let keys: std::collections::BTreeSet<&str> = body["error"].as_object().expect("an object").keys().map(String::as_str).collect();
        assert_eq!(keys, ["code", "message", "param", "type"].into_iter().collect::<std::collections::BTreeSet<_>>());
    }
}
