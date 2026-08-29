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
//!
//! `classify` admits only the two statuses this file can name: `413` maps to `RequestTooLarge`
//! and `400` to `BadRequestBody`. Everything else — 5xx **and any unexpected client status** —
//! returns `None` and hands axum's own response back. `BytesRejection` is `#[non_exhaustive]`,
//! so a status this module cannot yet name is a real possibility, not a defensive-only branch;
//! answering it with a fixed `BadRequestBody` would name a condition (an invalid body) that
//! did not occur.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::adapters::http::error::GatewayError;

/// Maps a rejection's status to the client-facing error, or `None` when this module cannot name
/// the condition — a 5xx, since it is not the caller's mistake, or a client status other than
/// the two this file knows about, since answering `BadRequestBody` for an unnamed status would
/// claim a cause it cannot know. In both cases axum's own response is handed back rather than a
/// fixed code being stamped onto a status it does not describe. IAM's `path.rs` and `json.rs`
/// make the identical choice.
fn classify(status: StatusCode) -> Option<GatewayError> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => Some(GatewayError::RequestTooLarge),
        StatusCode::BAD_REQUEST => Some(GatewayError::BadRequestBody),
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

    /// `BytesRejection` is `#[non_exhaustive]`: axum can add a variant carrying a client status
    /// other than 400/413. `classify` must hand that back to axum rather than guess
    /// `BadRequestBody` — pinned directly here since no such variant exists to drive through a
    /// real rejection today.
    #[test]
    fn classify_hands_back_an_unnamed_client_status() {
        assert_eq!(classify(StatusCode::UNPROCESSABLE_ENTITY), None, "not 400 or 413 — this module cannot name the condition");
        assert_eq!(classify(StatusCode::UNAUTHORIZED), None);
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
