// SPDX-License-Identifier: Apache-2.0

//! The house raw-body extractor: `EnvelopeBytes` answers a refused body inside the gateway's
//! OpenAI-compatible envelope instead of letting axum's plain-text rejection escape it
//! (SMA-588).
//!
//! `chat_completions` reads its body as raw [`Bytes`] so the original bytes can be forwarded
//! upstream verbatim. That is still true — this wraps EXTRACTION only and hands the same
//! `Bytes` through untouched, so the egress-hygiene property `chat.rs` calls load-bearing is
//! unaffected.
//!
//! What it changes is the failure path. `Bytes` honours the [`DefaultBodyLimit`] layer, so an
//! over-limit body fails the extractor with a 413 — and axum's own rejection is plain text with
//! no `error.code`, which the OpenAI SDKs cannot read. This is the same class of hole SMA-586
//! and SMA-587 closed in IAM, in the other service.
//!
//! **This file has its own module deliberately.** `ci/http-extractor/check.py`'s ALLOW table is
//! per-FILE, so the exemption the definition site needs would, from inside `chat.rs`, switch the
//! gate off for the very handler its `Bytes` row exists to catch.
//!
//! Classification is by STATUS, not by variant — the rule `json.rs` established in IAM.
//! `BytesRejection` wraps `FailedToBufferBody`, itself `{LengthLimitError (413),
//! UnknownBodyError (400)}`, so mapping the variant straight to `RequestTooLarge` would render a
//! 413 code on a 400 response.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::adapters::http::error::GatewayError;

/// Maps a rejection's status to the client-facing error, or `None` when it is not the caller's
/// mistake — in which case axum's own response is handed back rather than a 4xx-flavoured code
/// being stamped onto a 5xx. IAM's `path.rs` and `json.rs` make the identical choice.
fn classify(status: StatusCode) -> Option<GatewayError> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(GatewayError::RequestTooLarge),
        s if s.is_client_error() => Some(GatewayError::BadRequestBody),
        _ => None,
    }
}

/// `Bytes` with the gateway's OpenAI-compatible envelope on rejection.
#[derive(Debug)]
pub(crate) struct EnvelopeBytes(pub(crate) Bytes);

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
        format!("{}", b.len())
    }

    /// `classify` is a free function precisely so the arms a real rejection cannot reach are
    /// still testable: `BytesRejection` is `#[non_exhaustive]` with `pub(crate)` constructors,
    /// so it cannot be built outside axum.
    #[test]
    fn classify_maps_each_status_class() {
        assert_eq!(classify(StatusCode::PAYLOAD_TOO_LARGE), Some(GatewayError::RequestTooLarge));
        assert_eq!(classify(StatusCode::BAD_REQUEST), Some(GatewayError::BadRequestBody));
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), None, "a server fault is not the caller's mistake");
    }

    /// The 413 path end-to-end through a real `DefaultBodyLimit` router — the only way to
    /// produce a genuine `LengthLimitError`.
    #[tokio::test]
    async fn an_oversized_body_answers_in_the_openai_envelope() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(8));
        let resp = app
            .oneshot(axum::http::Request::builder().method("POST").uri("/").body(Body::from(vec![b'x'; 64])).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("an envelope, not plain text");
        assert_eq!(body["error"]["code"], "request-too-large");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }

    /// A body within the limit still reaches the handler with its bytes intact — the property
    /// the egress path depends on.
    #[tokio::test]
    async fn a_body_within_the_limit_passes_through_unchanged() {
        let app = Router::new().route("/", post(ok)).layer(DefaultBodyLimit::max(64));
        let resp = app
            .oneshot(axum::http::Request::builder().method("POST").uri("/").body(Body::from(vec![b'x'; 8])).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "8");
    }
}
