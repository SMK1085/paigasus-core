// SPDX-License-Identifier: Apache-2.0

//! The error of [`super::AgentAdmin`] (SMA-852 spec §6.6). Each variant is one answer of the
//! agent API. The transports map it to a status, a registered reason and a fixed message. A
//! caller value (a model name, a tool name, a field name) goes only into the error metadata.

use paigasus_agent_host_core::ids::InboxId;
use paigasus_agent_host_core::registry::RegistryError;

use crate::application::auth::AuthError;

/// An answer of the agent API that is not a success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    /// IAM refused the caller's credential.
    InvalidToken,
    /// The caller may not do this, or the id has no row that the caller can see.
    Forbidden,
    /// IAM cannot be reached.
    IamUnavailable,
    /// The database failed.
    StoreUnavailable,
    /// An unexpected fault. The detail stays in the logs.
    Internal,
    /// Another active agent of the org has the name.
    NameTaken,
    /// The agent already has an inbox for the principal. The value is the existing inbox.
    InboxExists(InboxId),
    /// The caller sent an old version. The value is the current version.
    VersionMismatch { current: u64 },
    /// The agent is archived.
    Archived,
    /// The host does not serve the model. The value is the model name.
    UnknownModel(String),
    /// The host does not have the tool. The value is the tool name.
    UnknownTool(String),
    /// A field breaks a rule. The value is the field name only.
    InvalidField(&'static str),
    /// The page size is out of range.
    InvalidPagination,
    /// The page token does not parse, or it belongs to another list.
    InvalidPageToken,
    /// The request body is not valid JSON (a transport sets this).
    InvalidRequestBody,
    /// The request body does not have the expected shape (a transport sets this).
    InvalidRequestSchema,
    /// A path segment or an id is not a valid UUID (a transport sets this).
    InvalidUuid,
    /// A query parameter is not valid (the REST transport sets this).
    InvalidQueryParameter,
}

impl From<AuthError> for AdminError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::InvalidToken => AdminError::InvalidToken,
            AuthError::Forbidden => AdminError::Forbidden,
            AuthError::IamUnavailable => AdminError::IamUnavailable,
            AuthError::Internal => AdminError::Internal,
        }
    }
}

impl From<RegistryError> for AdminError {
    fn from(err: RegistryError) -> Self {
        match err {
            RegistryError::Store(detail) => {
                tracing::error!(error = %detail, "the agent registry store failed");
                AdminError::StoreUnavailable
            }
            // `AgentAdmin` reads the row before every write, and no row is ever deleted. So a
            // `NotFound` from a write is a fault.
            RegistryError::NotFound => {
                tracing::error!("the agent registry returned NotFound for a row that the service read");
                AdminError::Internal
            }
            RegistryError::NameTaken => AdminError::NameTaken,
            RegistryError::InboxExists(inbox) => AdminError::InboxExists(inbox),
            RegistryError::VersionMismatch { current } => AdminError::VersionMismatch { current },
            RegistryError::Archived => AdminError::Archived,
            RegistryError::Invalid(field) => AdminError::InvalidField(field),
            RegistryError::UnknownModel(model) => AdminError::UnknownModel(model),
            RegistryError::UnknownTool(tool) => AdminError::UnknownTool(tool),
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn an_auth_error_maps_one_to_one() {
        assert_eq!(AdminError::from(AuthError::InvalidToken), AdminError::InvalidToken);
        assert_eq!(AdminError::from(AuthError::Forbidden), AdminError::Forbidden);
        assert_eq!(AdminError::from(AuthError::IamUnavailable), AdminError::IamUnavailable);
        assert_eq!(AdminError::from(AuthError::Internal), AdminError::Internal);
    }

    #[test]
    fn a_registry_error_maps_store_to_unavailable_and_not_found_to_internal() {
        let inbox = InboxId::new(Uuid::from_u128(1), Uuid::from_u128(2));
        let cases = [
            (RegistryError::Store("boom".to_owned()), AdminError::StoreUnavailable),
            (RegistryError::NotFound, AdminError::Internal),
            (RegistryError::NameTaken, AdminError::NameTaken),
            (RegistryError::InboxExists(inbox.clone()), AdminError::InboxExists(inbox)),
            (RegistryError::VersionMismatch { current: 7 }, AdminError::VersionMismatch { current: 7 }),
            (RegistryError::Archived, AdminError::Archived),
            (RegistryError::Invalid("name"), AdminError::InvalidField("name")),
            (RegistryError::UnknownModel("m".to_owned()), AdminError::UnknownModel("m".to_owned())),
            (RegistryError::UnknownTool("t".to_owned()), AdminError::UnknownTool("t".to_owned())),
        ];
        for (from, want) in cases {
            assert_eq!(AdminError::from(from), want);
        }
    }
}
