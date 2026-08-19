// SPDX-License-Identifier: Apache-2.0

//! Authn HTTP surface: `POST /v1/authn/introspect` plus the dedicated `AuthnError` →
//! response funnel (spec §6.3, D12). This funnel is deliberately SEPARATE from the tenancy
//! `ApiError`/`ErrorClass` machinery — that path expresses 400/404/409/500 plus a single
//! generic 403 (`forbidden`, SMA-444 task-16), while authn needs 401 (+ `WWW-Authenticate`),
//! several DISTINCT 403 subcodes (`identity-not-provisioned`/`provisioning-failed`/
//! `principal-inactive`), and 503. Bodies reuse the same `{"error":{code,message}}`
//! envelope; every message is STATIC per code — no claim values, token fragments, or
//! upstream error text ever reach the response (spec §6.3).

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, FromRequest, OptionalFromRequest, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
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
            AuthnError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, "invalid-token", "invalid bearer token"),
            AuthnError::IdentityNotProvisioned => (StatusCode::FORBIDDEN, "identity-not-provisioned", "identity not provisioned"),
            AuthnError::ProvisioningFailed(_) => (StatusCode::FORBIDDEN, "provisioning-failed", "provisioning failed"),
            AuthnError::PrincipalInactive => (StatusCode::FORBIDDEN, "principal-inactive", "principal inactive"),
            // `authn-unavailable`, NOT a bare `unavailable`: a rename, not a recasing, so it does
            // not read as a generic service-down code alongside the gateway's `iam-unavailable`
            // and `upstream-unavailable`, which name different failures (ADR-0019 A1.3).
            AuthnError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "authn-unavailable", "authentication backend unavailable"),
            AuthnError::Backend(_) => {
                // Debug carries the boxed repository/infra source (never token or claim
                // material by `AuthnError`'s own contract) — logged here, never surfaced.
                tracing::error!(error = ?self.0, "internal error handling an authn request");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
            }
        };

        let mut response = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
        response.headers_mut().insert(
            paigasus_observability::correlation::RETRYABLE_HEADER,
            HeaderValue::from_static(crate::adapters::retryable::authn_retryable(&self.0).as_wire()),
        );
        if matches!(self.0, AuthnError::InvalidToken(_)) {
            // RFC 6750 §3.1 standardises this value. NOT ours to rename — only the body's code is.
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(BEARER_CHALLENGE));
        }
        response
    }
}

/// `Json<T>` with the authn error envelope on rejection (spec H1): axum's default
/// plain-text rejections (malformed JSON, wrong content-type, oversized body) become the
/// same `{"error":{code,message}}` shape every other authn response uses. The status is
/// the rejection's own; messages are static — nothing ever echoes the request body.
/// `pub(crate)` (rather than private): `adapters::http::api_keys`'s introspect handler
/// (SMA-445 Task 20) reuses this SAME envelope for `POST /v1/authn/api-keys/introspect`
/// rather than duplicating the rejection-mapping logic — the two routes share one
/// unauthenticated-body-limited posture (spec H1). `adapters::http::system_retirement`
/// (SMA-481) reuses it too, via `Option<EnvelopeJson<T>>` below, for a route whose body is
/// optional but whose malformed-body response must still match this envelope.
#[derive(Debug)]
pub(crate) struct EnvelopeJson<T>(pub(crate) T);

/// Every `(code, message)` pair [`envelope_rejection`] can put on the wire.
///
/// Extracted from the `if` that used to inline both literals so the membership test can enumerate
/// them rather than restate them. The test previously hand-copied `"request-too-large"` and
/// `"invalid-request-body"`, so a third branch here would have escaped both it and
/// `repo:error-code-single-site` (SMA-507 E3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
enum RejectionKind {
    /// The body exceeded the configured byte limit.
    TooLarge,
    /// The body could not be deserialized.
    Invalid,
}

impl RejectionKind {
    /// This kind's canonical registry code and its static, caller-safe message.
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            // `invalid-request-body` is merged with the gateway's identical case: one code for one
            // condition across both services (ADR-0019 A1.3).
            RejectionKind::Invalid => ("invalid-request-body", "invalid request body"),
            RejectionKind::TooLarge => ("request-too-large", "request body too large"),
        }
    }
}

/// Maps a `JsonRejection` into the stable `{"error":{code,message}}` envelope — shared by both
/// `EnvelopeJson`'s required (`FromRequest`) and optional (`OptionalFromRequest`) extraction
/// paths below so the two can never drift apart on status/code/message.
fn envelope_rejection(rejection: JsonRejection) -> Response {
    let kind = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        RejectionKind::TooLarge
    } else {
        RejectionKind::Invalid
    };
    let (code, message) = kind.parts();
    let mut response = (rejection.status(), Json(json!({ "error": { "code": code, "message": message } }))).into_response();
    response.headers_mut().insert(
        paigasus_observability::correlation::RETRYABLE_HEADER,
        HeaderValue::from_static(paigasus_observability::Retryable::No.as_wire()),
    );
    response
}

impl<S, T> FromRequest<S> for EnvelopeJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(EnvelopeJson(value)),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}

/// `Option<EnvelopeJson<T>>` support (SMA-481): mirrors axum's own `Json<T>:
/// OptionalFromRequest` impl exactly for the "is there a body at all" question — no
/// `Content-Type` header means `Ok(None)` (never an attempt to parse zero bytes as JSON) — but
/// a body that DOES declare `Content-Type: application/json` and fails to parse still gets the
/// SAME stable envelope the required `FromRequest` impl above produces, rather than axum's bare
/// `JsonRejection` text escaping the house error contract.
impl<S, T> OptionalFromRequest<S> for EnvelopeJson<T>
where
    Json<T>: OptionalFromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(Some(Json(value))) => Ok(Some(EnvelopeJson(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}

/// The introspect sub-router. Merged alongside (NOT inside) the tenancy `/v1` sub-router
/// in `super::router` so Task 11's bearer-enforcement layer, which wraps the tenancy
/// sub-router only, never covers this route (middleware-exempt, spec §7.4). `body_limit`
/// (from `AppState::introspect_body_limit`) caps the request body at
/// `max_token_bytes` + envelope headroom — the only legitimate payload is
/// `{"token":"<= max_token_bytes>"}`, so anything larger is rejected before JSON parsing
/// (H1; deliberately far below axum's 2 MB default).
pub fn router(body_limit: usize) -> Router<AppState> {
    Router::new().route("/v1/authn/introspect", post(introspect)).route_layer(DefaultBodyLimit::max(body_limit))
}

/// `POST /v1/authn/introspect` (spec §7.2): the full `PrincipalContext` for a presented
/// token. READ-ONLY (D10): `AuthenticateToken::introspect` resolves with
/// `Provisioning::Disabled`, so an unknown identity is 403 `identity-not-provisioned` and
/// this unauthenticated endpoint never has a user-creation side effect. The body carries
/// the credential itself and is NEVER logged (nothing here logs, and an oversized token is
/// rejected by the validator's own length cap — no pre-filtering, no echo).
async fn introspect(State(state): State<AppState>, EnvelopeJson(body): EnvelopeJson<IntrospectBody>) -> Result<Json<IntrospectResponseDto>, AuthnApiError> {
    let ctx = state.authn.introspect(&body.token).await?;
    Ok(Json(ctx.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use paigasus_iam_core::{ProvisioningDefect, TokenDefect};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Probe {
        x: i32,
    }

    /// `Option<EnvelopeJson<T>>`'s "no body at all" branch (SMA-481): no `Content-Type` header
    /// must yield `Ok(None)`, mirroring axum's own `Json<T>: OptionalFromRequest` behavior
    /// exactly — never an attempt to parse zero bytes as JSON.
    #[tokio::test]
    async fn optional_envelope_json_yields_none_when_no_content_type_is_present() {
        let req = Request::builder().method("POST").uri("/").body(Body::empty()).unwrap();
        let extracted = <Option<EnvelopeJson<Probe>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect("an absent body must never be a 400/415");
        assert!(extracted.is_none());
    }

    /// The malformed-body case a fix-round review flagged: `Content-Type: application/json`
    /// declared but the body doesn't parse must still render the SAME `{"error":{code,message}}`
    /// envelope the required `EnvelopeJson` extraction produces — not axum's bare `JsonRejection`
    /// text escaping the house error contract.
    #[tokio::test]
    async fn optional_envelope_json_maps_a_malformed_body_to_the_stable_envelope() {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from("{not json"))
            .unwrap();
        let rejection = <Option<EnvelopeJson<Probe>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect_err("malformed JSON must be rejected");
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(rejection.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("invalid-request-body"));
        assert_eq!(body["error"]["message"], json!("invalid request body"));
    }

    /// The happy path: a present, well-formed body still extracts to `Some`.
    #[tokio::test]
    async fn optional_envelope_json_extracts_some_for_a_well_formed_body() {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"x": 1}"#))
            .unwrap();
        let extracted = <Option<EnvelopeJson<Probe>> as FromRequest<()>>::from_request(req, &())
            .await
            .expect("a well-formed body must not be rejected");
        assert!(matches!(extracted, Some(EnvelopeJson(Probe { x: 1 }))));
    }

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
        for defect in [TokenDefect::Malformed, TokenDefect::Expired, TokenDefect::Oversized, TokenDefect::BadSignature] {
            let (status, challenge, body) = rendered(AuthnError::InvalidToken(defect)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            // AC 7: RFC 6750 §3.1 standardises `invalid_token` in the CHALLENGE. It is not ours
            // to rename — only the JSON body's code becomes canonical.
            assert_eq!(challenge.as_deref(), Some("Bearer error=\"invalid_token\""));
            assert_eq!(body["error"]["code"], "invalid-token");
            assert_eq!(body["error"]["message"], "invalid bearer token");
        }
    }

    #[tokio::test]
    async fn forbidden_family_is_403_with_stable_codes_and_no_challenge() {
        let cases = [
            (AuthnError::IdentityNotProvisioned, "identity-not-provisioned", "identity not provisioned"),
            (AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail), "provisioning-failed", "provisioning failed"),
            (AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict), "provisioning-failed", "provisioning failed"),
            (AuthnError::PrincipalInactive, "principal-inactive", "principal inactive"),
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
        assert_eq!(body["error"]["code"], "authn-unavailable");
        assert_eq!(body["error"]["message"], "authentication backend unavailable");
    }

    /// AC 1: every code this funnel and its extractor can emit is in the canonical registry.
    ///
    /// The `AuthnError` half is driven off `all_authn_errors()`, so a new variant is covered
    /// automatically. The extractor half used to hand-restate `envelope_rejection`'s two literals
    /// here, which meant a third branch there would have escaped this test AND
    /// `repo:error-code-single-site` (this file is on the manifest) — SMA-507 E3. It now
    /// enumerates `RejectionKind`, so a new kind must state its parts or fail to compile.
    #[tokio::test]
    async fn every_authn_http_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        let mut codes: Vec<String> = RejectionKind::iter().map(|kind| kind.parts().0.to_owned()).collect();
        assert!(!codes.is_empty(), "RejectionKind must yield at least one code, or this half asserts nothing");
        for err in crate::adapters::retryable::tests_support::all_authn_errors() {
            let (_, _, body) = rendered(err).await;
            codes.push(body["error"]["code"].as_str().expect("a code").to_owned());
        }
        for code in codes {
            assert!(ErrorReason::from_wire_reason(&code).is_some(), "{code} is not declared in common/v1/error.proto");
        }
    }

    /// D4: the header is present on EVERY error response, carrying the literal `false` where the
    /// error is not retryable — a client must never have to read absence as `false`.
    #[tokio::test]
    async fn every_authn_error_carries_a_retryable_header() {
        let cases = [
            (AuthnError::InvalidToken(TokenDefect::Malformed), "false"),
            (AuthnError::Unavailable, "true"),
            (AuthnError::Backend("x".into()), "unknown"),
        ];
        for (err, want) in cases {
            let response = AuthnApiError(err).into_response();
            assert_eq!(response.headers()["paigasus-retryable"], want);
        }
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
