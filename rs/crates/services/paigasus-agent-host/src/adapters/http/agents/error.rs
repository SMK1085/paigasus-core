// SPDX-License-Identifier: Apache-2.0

//! The error answer of the agent API (SMA-852 spec §6.6): `{"error":{"code","message"}}` and, when
//! the error has metadata, `"metadata":{…}`. A reason that ingress already has is answered by
//! [`ApiError`], so its code, status and message exist once. Only the reasons of the agent API
//! have their own arms here. The caller's value goes only into the metadata, never into the
//! message.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::adapters::http::error::ApiError;
use crate::application::agents::AdminError;

/// An answer of the agent API that is not a success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentApiError(pub AdminError);

/// The metadata of an error: pairs of a key and a value.
pub type Metadata = Vec<(&'static str, String)>;

/// The answer of a reason that ingress has too. It has no metadata.
fn shared(error: ApiError) -> (StatusCode, &'static str, &'static str, Metadata) {
    let (status, code, message) = error.parts();
    (status, code, message, Vec::new())
}

impl AgentApiError {
    /// The status, the registered code, the fixed message and the metadata (spec §6.6).
    #[must_use]
    pub fn parts(&self) -> (StatusCode, &'static str, &'static str, Metadata) {
        match &self.0 {
            AdminError::InvalidToken => shared(ApiError::InvalidToken),
            AdminError::Forbidden => shared(ApiError::Forbidden),
            AdminError::IamUnavailable => shared(ApiError::IamUnavailable),
            AdminError::StoreUnavailable => shared(ApiError::StoreUnavailable),
            AdminError::Internal => shared(ApiError::Internal),
            AdminError::Archived => shared(ApiError::AgentArchived),
            AdminError::InvalidRequestBody => shared(ApiError::BadRequestBody),
            AdminError::InvalidRequestSchema => shared(ApiError::InvalidRequestSchema),
            AdminError::InvalidUuid => shared(ApiError::InvalidUuid),
            AdminError::NameTaken => (StatusCode::CONFLICT, "agent-name-taken", "an active agent of the organization has this name", Vec::new()),
            AdminError::InboxExists(inbox) => (
                StatusCode::CONFLICT,
                "inbox-exists",
                "the agent already has an inbox for this principal",
                vec![("inbox", inbox.as_prn().canonical())],
            ),
            AdminError::VersionMismatch { current } => (
                StatusCode::CONFLICT,
                "agent-version-mismatch",
                "the version is not the current version of the agent",
                vec![("version", current.to_string())],
            ),
            AdminError::UnknownModel(model) => (StatusCode::BAD_REQUEST, "unknown-model", "the host does not serve the model", vec![("model", model.clone())]),
            AdminError::UnknownTool(tool) => (StatusCode::BAD_REQUEST, "unknown-tool", "the host does not have the tool", vec![("tool", tool.clone())]),
            AdminError::InvalidField(field) => (
                StatusCode::BAD_REQUEST,
                "invalid-agent-field",
                "a field of the agent is not valid",
                vec![("field", (*field).to_owned())],
            ),
            AdminError::InvalidPagination => (StatusCode::BAD_REQUEST, "invalid-pagination", "the page size is out of range", Vec::new()),
            AdminError::InvalidPageToken => (StatusCode::BAD_REQUEST, "invalid-cursor", "the page token is not valid for this list", Vec::new()),
        }
    }
}

impl IntoResponse for AgentApiError {
    fn into_response(self) -> Response {
        let (status, code, message, metadata) = self.parts();
        let mut error = Map::new();
        error.insert("code".to_owned(), json!(code));
        error.insert("message".to_owned(), json!(message));
        if !metadata.is_empty() {
            let map: Map<String, Value> = metadata.into_iter().map(|(k, v)| (k.to_owned(), Value::String(v))).collect();
            error.insert("metadata".to_owned(), Value::Object(map));
        }
        (status, Json(json!({ "error": Value::Object(error) }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::ids::InboxId;
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use uuid::Uuid;

    use super::*;

    fn inbox() -> InboxId {
        InboxId::new(Uuid::from_u128(1), Uuid::from_u128(2))
    }

    #[test]
    fn every_agent_api_code_is_in_the_registry() {
        for (error, reason) in [
            (AdminError::InvalidToken, ErrorReason::InvalidToken),
            (AdminError::Forbidden, ErrorReason::Forbidden),
            (AdminError::IamUnavailable, ErrorReason::IamUnavailable),
            (AdminError::StoreUnavailable, ErrorReason::StoreUnavailable),
            (AdminError::Internal, ErrorReason::Internal),
            (AdminError::NameTaken, ErrorReason::AgentNameTaken),
            (AdminError::InboxExists(inbox()), ErrorReason::InboxExists),
            (AdminError::VersionMismatch { current: 3 }, ErrorReason::AgentVersionMismatch),
            (AdminError::Archived, ErrorReason::AgentArchived),
            (AdminError::UnknownModel("m".to_owned()), ErrorReason::UnknownModel),
            (AdminError::UnknownTool("t".to_owned()), ErrorReason::UnknownTool),
            (AdminError::InvalidField("name"), ErrorReason::InvalidAgentField),
            (AdminError::InvalidPagination, ErrorReason::InvalidPagination),
            (AdminError::InvalidPageToken, ErrorReason::InvalidCursor),
            (AdminError::InvalidRequestBody, ErrorReason::InvalidRequestBody),
            (AdminError::InvalidRequestSchema, ErrorReason::InvalidRequestSchema),
            (AdminError::InvalidUuid, ErrorReason::InvalidUuid),
        ] {
            let (_status, code, _message, _metadata) = AgentApiError(error).parts();
            assert_eq!(ErrorReason::from_wire_reason(code), Some(reason), "{code} is not declared in common/v1/error.proto");
        }
    }

    #[test]
    fn no_agent_api_message_holds_caller_input() {
        let existing = inbox();
        for error in [
            AdminError::UnknownModel("secret".to_owned()),
            AdminError::UnknownTool("secret".to_owned()),
            AdminError::InboxExists(existing.clone()),
        ] {
            let (_status, _code, message, metadata) = AgentApiError(error).parts();
            assert!(!message.contains("secret"), "{message}");
            assert!(!message.contains(&existing.as_prn().canonical()), "{message}");
            assert!(!metadata.is_empty());
        }
    }

    #[tokio::test]
    async fn the_metadata_key_is_present_only_when_the_metadata_is_not_empty() {
        let with = AgentApiError(AdminError::VersionMismatch { current: 7 }).into_response();
        assert_eq!(with.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(with.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["metadata"], json!({ "version": "7" }));

        let without = AgentApiError(AdminError::Forbidden).into_response();
        let bytes = axum::body::to_bytes(without.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["error"].get("metadata").is_none());
    }

    #[test]
    fn a_shared_reason_has_the_answer_of_ingress() {
        for (error, shared) in [
            (AdminError::Forbidden, ApiError::Forbidden),
            (AdminError::InvalidToken, ApiError::InvalidToken),
            (AdminError::IamUnavailable, ApiError::IamUnavailable),
            (AdminError::StoreUnavailable, ApiError::StoreUnavailable),
            (AdminError::Archived, ApiError::AgentArchived),
            (AdminError::InvalidRequestBody, ApiError::BadRequestBody),
        ] {
            let (status, code, message, _) = AgentApiError(error).parts();
            assert_eq!((status, code, message), shared.parts());
        }
    }
}
