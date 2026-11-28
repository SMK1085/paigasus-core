// SPDX-License-Identifier: Apache-2.0

//! The house path extractor for one UUID segment. It answers a malformed segment in the error
//! envelope, not with the plain-text 400 of axum `Path<Uuid>`. It is a copy of the one-segment
//! part of `paigasus-iam/src/adapters/http/path.rs`.
//!
//! The marker type names the field. A `PathRejection` that axum classes as 500 means that the
//! route pattern and the handler do not agree, which is a server fault. Those keep the response
//! of axum.

use std::marker::PhantomData;

use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use super::error::ApiError;

/// Names the wire field that a path segment stands for.
pub trait PathField {
    const NAME: &'static str;
}

/// `{inbox}` on the events route.
pub struct InboxIdField;

impl PathField for InboxIdField {
    const NAME: &'static str = "inbox";
}

/// `{agent}` on the agent routes.
pub struct AgentIdField;

impl PathField for AgentIdField {
    const NAME: &'static str = "agent";
}

fn malformed_uuid(field: &'static str) -> Response {
    tracing::debug!(field, "a path segment is not a UUID");
    ApiError::InvalidUuid.into_response()
}

/// Did the caller cause this rejection? axum answers 400 for the kinds a request can cause and
/// 500 for the others.
fn is_client_error(rejection: &PathRejection) -> bool {
    match rejection {
        PathRejection::FailedToDeserializePathParams(e) => e.status() == StatusCode::BAD_REQUEST,
        _ => false,
    }
}

/// One UUID path segment, reported as `F::NAME` when it is malformed.
pub struct UuidPath<F: PathField> {
    pub id: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let raw = match Path::<String>::from_request_parts(parts, state).await {
            Ok(Path(raw)) => raw,
            Err(rejection) if is_client_error(&rejection) => return Err(malformed_uuid(F::NAME)),
            Err(rejection) => return Err(rejection.into_response()),
        };
        let id = Uuid::parse_str(&raw).map_err(|_| malformed_uuid(F::NAME))?;
        Ok(UuidPath { id, _marker: PhantomData })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn echo(UuidPath { id, .. }: UuidPath<InboxIdField>) -> String {
        id.to_string()
    }

    async fn call(path: &str) -> (StatusCode, String) {
        let app = Router::new().route("/x/{inbox}", get(echo));
        let resp = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn a_uuid_segment_passes() {
        let (status, body) = call("/x/00000000-0000-0000-0000-000000000001").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "00000000-0000-0000-0000-000000000001");
    }

    #[tokio::test]
    async fn a_bad_segment_answers_in_the_envelope() {
        let (status, body) = call("/x/nope").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_str(&body).expect("an envelope, not plain text");
        let (_status, code, _message) = ApiError::InvalidUuid.parts();
        assert_eq!(json["error"]["code"], code);
    }
}
