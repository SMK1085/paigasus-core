// SPDX-License-Identifier: Apache-2.0

//! The house JSON request extractor: `EnvelopeJson<T>` answers a refused body inside IAM's
//! stable `{"error":{code,message}}` envelope with a registered reason, instead of letting
//! axum's plain-text rejection escape the error contract (SMA-587).
//!
//! Sibling of `path.rs` by design — one extractor module per input kind, neither owned by a
//! handler module. It differs from `path.rs` on one axis deliberately (spec D2.1): `path.rs`
//! renders through `ApiError(TenancyError::…)`, while this module builds the envelope by hand
//! from literals. It must, because `EnvelopeJson` also serves `api_keys::introspect`, whose
//! every other failure is an `AuthnApiError` — a funnel deliberately separate from
//! `ApiError`/`TenancyError` (`authn.rs` module docs). An extractor emitting a `TenancyError`
//! there would make a route's error type depend on WHERE in the request it failed. The cost is
//! that this file carries code literals and therefore sits on
//! `ci/error-registry/check.py`'s MANIFEST; the mitigation is that the membership test
//! enumerates `RejectionKind` via `strum::EnumIter` rather than restating them.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, OptionalFromRequest, Request};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Every `(code, message)` pair this module can put on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub(crate) enum RejectionKind {
    /// The body exceeded the configured byte limit.
    TooLarge,
    /// The body could not be read, or was not syntactically valid JSON.
    Invalid,
    /// The request declared a `Content-Type` this endpoint does not accept.
    UnsupportedContentType,
    /// Syntactically valid JSON that did not match the target type.
    InvalidSchema,
}

impl RejectionKind {
    /// This kind's canonical registry code and its static, caller-safe message.
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            // Narrowed by SMA-587 to MALFORMED-or-unreadable only; the wrong-content-type and
            // schema-mismatch cases used to share this code and now have their own.
            RejectionKind::Invalid => ("invalid-request-body", "invalid request body"),
            RejectionKind::TooLarge => ("request-too-large", "request body too large"),
            RejectionKind::UnsupportedContentType => ("unsupported-content-type", "unsupported content type"),
            RejectionKind::InvalidSchema => ("invalid-request-schema", "request body did not match the expected schema"),
        }
    }
}

/// The status-only half of the classification rule (spec D1.1).
///
/// `None` means "this is not the caller's mistake" — the caller gets axum's own response rather
/// than a 4xx-flavoured code on a 5xx status. `path.rs:87-92` makes the identical choice for
/// `PathRejection`'s server-bug family, and two extractors answering server bugs differently
/// would be worse than one plain-text 500.
fn classify(status: StatusCode) -> Option<RejectionKind> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(RejectionKind::TooLarge),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => Some(RejectionKind::UnsupportedContentType),
        StatusCode::UNPROCESSABLE_ENTITY => Some(RejectionKind::InvalidSchema),
        s if s.is_client_error() => Some(RejectionKind::Invalid),
        _ => None,
    }
}

/// Renders one kind into the envelope, preserving the rejection's OWN status — no route's
/// status changes anywhere in SMA-587.
fn envelope(kind: RejectionKind, status: StatusCode) -> Response {
    let (code, message) = kind.parts();
    let mut response = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
    response.headers_mut().insert(
        paigasus_observability::correlation::RETRYABLE_HEADER,
        HeaderValue::from_static(paigasus_observability::Retryable::No.as_wire()),
    );
    response
}

/// Maps a `JsonRejection` into the stable envelope — shared by both extraction paths below so
/// the two can never drift on status, code or shape.
///
/// The rule is hybrid on purpose (spec D1.1): match the VARIANT where the variant determines the
/// status, dispatch on STATUS everywhere else. A variant-only match would be wrong, because
/// `JsonRejection::BytesRejection` wraps `FailedToBufferBody`, itself
/// `{LengthLimitError (413), UnknownBodyError (400)}` — so mapping that variant straight to
/// `request-too-large` would render a 413 code on a 400 response. And `JsonRejection` is
/// `#[non_exhaustive]`, so the fallback arm is mandatory rather than optional.
///
/// Restructured from a straight `match &rejection { … }` to compute `kind` first: matching on
/// `&rejection` while also moving `rejection` into the fallback arm (`rejection.into_response()`)
/// is a borrow conflict. Behaviour is identical to the variant-then-status rule described above.
fn envelope_rejection(rejection: JsonRejection) -> Response {
    let status = rejection.status();
    let kind = match &rejection {
        JsonRejection::JsonSyntaxError(_) => Some(RejectionKind::Invalid),
        JsonRejection::MissingJsonContentType(_) => Some(RejectionKind::UnsupportedContentType),
        JsonRejection::JsonDataError(_) => Some(RejectionKind::InvalidSchema),
        _ => classify(status),
    };
    match kind {
        Some(kind) => envelope(kind, status),
        None => rejection.into_response(),
    }
}

/// `Json<T>` with the IAM error envelope on rejection: axum's default plain-text rejections
/// (malformed JSON, wrong content-type, schema mismatch, oversized body) become the same
/// `{"error":{code,message}}` shape every other IAM response uses. The status is the
/// rejection's own; messages are static — nothing ever echoes the request body.
///
/// This is the house extractor for EVERY request body on this adapter (SMA-587). A handler
/// taking a bare `axum::Json` in request position is a bug, and `repo:http-extractor-envelope`
/// fails the build on one.
#[derive(Debug)]
pub(crate) struct EnvelopeJson<T>(pub(crate) T);

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
/// `Content-Type` header means `Ok(None)`, never an attempt to parse zero bytes as JSON — but a
/// body that DOES declare `Content-Type: application/json` and fails to parse still gets the
/// same envelope the required impl above produces.
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::extract::Request;
    use axum::http::StatusCode;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct Probe {
        x: i32,
    }

    /// D1.1a: `classify` is the status-only half of the rule, extracted so the fallback is
    /// reachable at all. The `match` in `envelope_rejection` is NOT — axum's rejections have
    /// `pub(crate)` constructors and `#[non_exhaustive]` enums, so no `BytesRejection` can be
    /// built outside axum. Without this function the fallback would be untestable.
    #[test]
    fn classify_maps_each_client_error_and_refuses_server_errors() {
        assert_eq!(classify(StatusCode::BAD_REQUEST), Some(RejectionKind::Invalid));
        assert_eq!(classify(StatusCode::PAYLOAD_TOO_LARGE), Some(RejectionKind::TooLarge));
        assert_eq!(classify(StatusCode::UNSUPPORTED_MEDIA_TYPE), Some(RejectionKind::UnsupportedContentType));
        assert_eq!(classify(StatusCode::UNPROCESSABLE_ENTITY), Some(RejectionKind::InvalidSchema));
        // Any other CLIENT error is still the caller's problem, so it stays in the envelope.
        assert_eq!(classify(StatusCode::CONFLICT), Some(RejectionKind::Invalid));
        // A SERVER error is OUR mistake. Answering it with a 4xx-flavoured code would report it
        // as the caller's — the exact inversion `path.rs:11-17` refuses. `None` means "hand
        // axum's own response back untouched".
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), None);
        assert_eq!(classify(StatusCode::BAD_GATEWAY), None);
    }

    /// Every code this module can put on the wire is in the canonical registry. Driven off
    /// `strum::EnumIter` rather than restated literals — the SMA-507 E3 lesson: a hand-copied
    /// list lets a new arm escape both this test and `repo:error-code-single-site`.
    #[test]
    fn every_request_extractor_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        let codes: Vec<&'static str> = RejectionKind::iter().map(|kind| kind.parts().0).collect();
        assert_eq!(codes.len(), 4, "all four kinds must be enumerated, or this asserts less than it claims");
        for code in codes {
            assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        }
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

    /// The 415 arm, end to end through the extractor: a declared content type the endpoint does
    /// not accept is refused before the body is read, and answers in the envelope.
    #[tokio::test]
    async fn a_wrong_content_type_is_unsupported_content_type() {
        let req = Request::builder().method("POST").uri("/").header("content-type", "text/plain").body(Body::from("{}")).unwrap();
        let rejection = <EnvelopeJson<Probe> as FromRequest<()>>::from_request(req, &())
            .await
            .expect_err("a wrong content type must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let bytes = to_bytes(rejection.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("unsupported-content-type"));
    }

    /// The 422 arm: syntactically valid JSON that does not match the target type. Distinct from
    /// the 400 syntax case above — before SMA-587 both answered `invalid-request-body`.
    #[tokio::test]
    async fn a_schema_mismatch_is_invalid_request_schema() {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"x": "not an integer"}"#))
            .unwrap();
        let rejection = <EnvelopeJson<Probe> as FromRequest<()>>::from_request(req, &()).await.expect_err("a schema mismatch must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(rejection.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("invalid-request-schema"));
    }

    /// The 413 arm, which `classify` alone cannot prove reachable: a genuine `LengthLimitError`
    /// only exists behind a real body limit, and `BytesRejection` cannot be constructed outside
    /// axum. Driving a router with `DefaultBodyLimit` is the only way to produce one.
    #[tokio::test]
    async fn an_oversized_body_is_request_too_large() {
        use axum::Router;
        use axum::extract::DefaultBodyLimit;
        use axum::routing::post;
        use tower::ServiceExt;

        async fn probe(EnvelopeJson(_): EnvelopeJson<Probe>) -> StatusCode {
            StatusCode::OK
        }
        let app = Router::new().route("/", post(probe)).layer(DefaultBodyLimit::max(8));

        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"x": 123456789}"#))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("request-too-large"));
    }
}
