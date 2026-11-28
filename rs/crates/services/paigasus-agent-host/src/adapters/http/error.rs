// SPDX-License-Identifier: Apache-2.0

//! The error envelope of the HTTP surface: `{"error":{"code":"…","message":"…"}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The registered code and the message of the 404 answer. The membership test below checks the
/// code against the registry in `common/v1/error.proto`.
pub const NOT_FOUND: (&str, &str) = ("not-found", "no route matches this path");

/// The fallback of the router: 404 in the error envelope.
pub async fn not_found() -> Response {
    let (code, message) = NOT_FOUND;
    envelope(StatusCode::NOT_FOUND, code, message)
}

/// An error answer of the HTTP surface. Each variant has one status, one registered code and one
/// fixed message. The message names no caller input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    /// 401: no credential, a credential that IAM refuses, or a scheme other than `Bearer`.
    InvalidToken,
    /// 403: the caller may not write to this inbox.
    Forbidden,
    /// 503: IAM cannot be reached. The client can retry.
    IamUnavailable,
    /// 415: the content type is not `application/cloudevents+json`.
    UnsupportedContentType,
    /// 413: the body is larger than `ingress.max_body_bytes`.
    RequestTooLarge,
    /// 400: the body cannot be read, or it is not JSON.
    BadRequestBody,
    /// 400: the body is JSON, and it is not a valid CloudEvent for this route.
    InvalidRequestSchema,
    /// 400: a path segment is not a UUID.
    InvalidUuid,
    /// 409: the agent of the inbox is archived. It takes no new events.
    AgentArchived,
    /// 503: the database failed. The client can retry.
    StoreUnavailable,
    /// 500: an unexpected fault. The detail stays in the logs.
    Internal,
}

impl ApiError {
    /// The status, the registered code and the message of the error.
    #[must_use]
    pub fn parts(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            ApiError::InvalidToken => (StatusCode::UNAUTHORIZED, "invalid-token", "the bearer credential is missing or not valid"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "the caller may not write to this inbox"),
            ApiError::IamUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "iam-unavailable", "the identity service cannot be reached; retry later"),
            ApiError::UnsupportedContentType => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported-content-type", "the content type must be application/cloudevents+json"),
            ApiError::RequestTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "request-too-large", "the request body is larger than the limit"),
            ApiError::BadRequestBody => (StatusCode::BAD_REQUEST, "invalid-request-body", "the request body is not valid JSON"),
            ApiError::InvalidRequestSchema => (StatusCode::BAD_REQUEST, "invalid-request-schema", "the request body is not a valid CloudEvent for this route"),
            ApiError::InvalidUuid => (StatusCode::BAD_REQUEST, "invalid-uuid", "a path segment is not a valid UUID"),
            ApiError::AgentArchived => (StatusCode::CONFLICT, "agent-archived", "the agent of this inbox is archived"),
            ApiError::StoreUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "store-unavailable", "the store cannot be reached; retry later"),
            ApiError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "an internal error occurred"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        envelope(status, code, message)
    }
}

/// The error envelope `{"error":{"code":"…","message":"…"}}`.
fn envelope(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_host_http_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let (code, _message) = NOT_FOUND;
        assert_eq!(ErrorReason::from_wire_reason(code), Some(ErrorReason::NotFound), "{code} is not declared in common/v1/error.proto");
        for (error, reason) in [
            (ApiError::InvalidToken, ErrorReason::InvalidToken),
            (ApiError::Forbidden, ErrorReason::Forbidden),
            (ApiError::IamUnavailable, ErrorReason::IamUnavailable),
            (ApiError::UnsupportedContentType, ErrorReason::UnsupportedContentType),
            (ApiError::RequestTooLarge, ErrorReason::RequestTooLarge),
            (ApiError::BadRequestBody, ErrorReason::InvalidRequestBody),
            (ApiError::InvalidRequestSchema, ErrorReason::InvalidRequestSchema),
            (ApiError::InvalidUuid, ErrorReason::InvalidUuid),
            (ApiError::AgentArchived, ErrorReason::AgentArchived),
            (ApiError::StoreUnavailable, ErrorReason::StoreUnavailable),
            (ApiError::Internal, ErrorReason::Internal),
        ] {
            let (_status, code, _message) = error.parts();
            assert_eq!(ErrorReason::from_wire_reason(code), Some(reason), "{code} is not declared in common/v1/error.proto");
        }
    }

    #[tokio::test]
    async fn an_api_error_renders_the_envelope() {
        for error in [
            ApiError::InvalidToken,
            ApiError::Forbidden,
            ApiError::IamUnavailable,
            ApiError::UnsupportedContentType,
            ApiError::RequestTooLarge,
            ApiError::BadRequestBody,
            ApiError::InvalidRequestSchema,
            ApiError::InvalidUuid,
            ApiError::AgentArchived,
            ApiError::StoreUnavailable,
            ApiError::Internal,
        ] {
            let (status, code, message) = error.parts();
            let response = error.into_response();
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body, json!({ "error": { "code": code, "message": message } }));
        }
    }
}
