// SPDX-License-Identifier: Apache-2.0

//! The house raw-body extractor. `EnvelopeBytes` answers a refused body in the error envelope,
//! not with the plain-text rejection of axum `Bytes`. It is a copy of
//! `paigasus-gateway/src/adapters/http/bytes.rs`.
//!
//! The file has its own module on purpose. The `ALLOW` table of `ci/http-extractor/check.py` is
//! per file, so an exemption here does not switch the gate off for the handler.
//!
//! The classification uses the status, not the variant. `BytesRejection` is `#[non_exhaustive]`.
//! A 413 maps to `RequestTooLarge` and a 400 to `BadRequestBody`. Every other status returns the
//! response of axum: this module cannot name that condition.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::error::ApiError;

fn classify(status: StatusCode) -> Option<ApiError> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(ApiError::RequestTooLarge),
        StatusCode::BAD_REQUEST => Some(ApiError::BadRequestBody),
        _ => None,
    }
}

/// `Bytes` with the error envelope on rejection.
#[derive(Debug)]
pub struct EnvelopeBytes(pub Bytes);

impl<S: Send + Sync> FromRequest<S> for EnvelopeBytes {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Bytes::from_request(req, state).await {
            Ok(bytes) => Ok(EnvelopeBytes(bytes)),
            Err(rejection) => Err(envelope_rejection(rejection)),
        }
    }
}

fn envelope_rejection(rejection: BytesRejection) -> Response {
    match classify(rejection.status()) {
        Some(err) => err.into_response(),
        None => rejection.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::DefaultBodyLimit;
    use axum::routing::post;
    use tower::ServiceExt;

    async fn ok(EnvelopeBytes(b): EnvelopeBytes) -> String {
        b.len().to_string()
    }

    #[test]
    fn classify_maps_each_status_class() {
        assert_eq!(classify(StatusCode::PAYLOAD_TOO_LARGE), Some(ApiError::RequestTooLarge));
        assert_eq!(classify(StatusCode::BAD_REQUEST), Some(ApiError::BadRequestBody));
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), None);
        assert_eq!(classify(StatusCode::UNPROCESSABLE_ENTITY), None);
    }

    #[tokio::test]
    async fn an_oversized_body_answers_in_the_envelope() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(8));
        let resp = app.oneshot(Request::post("/").body(Body::from(vec![b'x'; 64])).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("an envelope, not plain text");
        let (_status, code, _message) = ApiError::RequestTooLarge.parts();
        assert_eq!(body["error"]["code"], code);
    }

    #[tokio::test]
    async fn a_body_within_the_limit_passes_unchanged() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(64));
        let resp = app.oneshot(Request::post("/").body(Body::from(vec![b'x'; 8])).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), b"8");
    }
}
