// SPDX-License-Identifier: Apache-2.0

//! The gRPC answer of an [`AdminError`] (SMA-852 spec §6.6). The reason, the message and the
//! metadata are those of the REST side ([`AgentApiError::parts`]). This module only chooses the
//! gRPC code, and it puts the reason and the metadata into a `google.rpc.ErrorInfo` of the
//! `AGENTS_DOMAIN`.

use std::collections::HashMap;

use paigasus_proto::error::AGENTS_DOMAIN;
use paigasus_proto::paigasus::common::v1::ErrorReason;
use tonic::{Code, Status};
use tonic_types::{ErrorDetails, StatusExt};

use crate::adapters::boot::MIGRATING;
use crate::adapters::http::agents::error::AgentApiError;
use crate::adapters::http::error::ApiError;
use crate::application::agents::AdminError;

/// The gRPC code of a registered reason (spec §6.6). A reason that the agent API does not
/// produce is a fault.
fn code_of(reason: ErrorReason) -> Code {
    match reason {
        ErrorReason::InvalidToken => Code::Unauthenticated,
        ErrorReason::Forbidden => Code::PermissionDenied,
        ErrorReason::IamUnavailable | ErrorReason::StoreUnavailable | ErrorReason::ServiceMigrating => Code::Unavailable,
        ErrorReason::AgentNameTaken | ErrorReason::InboxExists => Code::AlreadyExists,
        ErrorReason::AgentVersionMismatch => Code::Aborted,
        ErrorReason::AgentArchived => Code::FailedPrecondition,
        ErrorReason::UnknownModel
        | ErrorReason::UnknownTool
        | ErrorReason::InvalidAgentField
        | ErrorReason::InvalidCursor
        | ErrorReason::InvalidPagination
        | ErrorReason::InvalidRequestBody
        | ErrorReason::InvalidRequestSchema
        | ErrorReason::InvalidUuid => Code::InvalidArgument,
        _ => Code::Internal,
    }
}

/// A `Status` with the `ErrorInfo` of the agents domain.
fn status(code: Code, reason: &str, message: &str, metadata: HashMap<String, String>) -> Status {
    Status::with_error_details(code, message, ErrorDetails::with_error_info(reason, &*AGENTS_DOMAIN, metadata))
}

/// The gRPC `Status` of an error of the agent API. The message is the fixed text of the REST
/// side. The caller's value is only in the `ErrorInfo` metadata.
#[must_use]
pub fn to_status(error: &AdminError) -> Status {
    let (_http, reason, message, metadata) = AgentApiError(error.clone()).parts();
    let code = ErrorReason::from_wire_reason(reason).map_or(Code::Internal, code_of);
    status(code, reason, message, metadata.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// The error of the authentication, as an error of the agent API. Authentication has no
/// registry error, so a variant that it never returns is a fault.
#[must_use]
pub fn from_api_error(error: ApiError) -> AdminError {
    match error {
        ApiError::InvalidToken => AdminError::InvalidToken,
        ApiError::Forbidden => AdminError::Forbidden,
        ApiError::IamUnavailable => AdminError::IamUnavailable,
        ApiError::StoreUnavailable => AdminError::StoreUnavailable,
        _ => AdminError::Internal,
    }
}

/// The answer of a call before boot has installed the admin: `UNAVAILABLE` with the reason of
/// the HTTP boot slot.
#[must_use]
pub fn migrating() -> Status {
    let (reason, message) = MIGRATING;
    status(code_of(ErrorReason::ServiceMigrating), reason, message, HashMap::new())
}

#[cfg(test)]
mod tests {
    use paigasus_agent_host_core::ids::InboxId;
    use tonic_types::StatusExt;
    use uuid::Uuid;

    use super::*;

    fn all_errors() -> Vec<AdminError> {
        vec![
            AdminError::InvalidToken,
            AdminError::Forbidden,
            AdminError::IamUnavailable,
            AdminError::StoreUnavailable,
            AdminError::Internal,
            AdminError::NameTaken,
            AdminError::InboxExists(InboxId::new(Uuid::from_u128(1), Uuid::from_u128(2))),
            AdminError::VersionMismatch { current: 3 },
            AdminError::Archived,
            AdminError::UnknownModel("m".to_owned()),
            AdminError::UnknownTool("t".to_owned()),
            AdminError::InvalidField("name"),
            AdminError::InvalidPagination,
            AdminError::InvalidPageToken,
            AdminError::InvalidRequestBody,
            AdminError::InvalidRequestSchema,
            AdminError::InvalidUuid,
        ]
    }

    #[test]
    fn each_error_has_the_error_info_of_the_agents_domain_and_the_rest_text() {
        for error in all_errors() {
            let (_http, reason, message, metadata) = AgentApiError(error.clone()).parts();
            let status = to_status(&error);
            assert_eq!(status.message(), message);
            let details = status.get_error_details();
            let info = details.error_info().expect("an ErrorInfo");
            assert_eq!(info.reason, reason);
            assert_eq!(info.domain, *AGENTS_DOMAIN);
            assert_eq!(info.metadata.len(), metadata.len());
        }
    }

    #[test]
    fn the_codes_are_those_of_the_spec() {
        let expected = [
            Code::Unauthenticated,
            Code::PermissionDenied,
            Code::Unavailable,
            Code::Unavailable,
            Code::Internal,
            Code::AlreadyExists,
            Code::AlreadyExists,
            Code::Aborted,
            Code::FailedPrecondition,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
            Code::InvalidArgument,
        ];
        let errors = all_errors();
        assert_eq!(errors.len(), expected.len());
        for (error, code) in errors.iter().zip(expected) {
            assert_eq!(to_status(error).code(), code, "{error:?}");
        }
    }

    #[test]
    fn the_migrating_status_is_unavailable_with_the_reason_of_the_boot_slot() {
        let status = migrating();
        assert_eq!(status.code(), Code::Unavailable);
        let details = status.get_error_details();
        let info = details.error_info().expect("an ErrorInfo");
        assert_eq!(info.reason, MIGRATING.0);
        assert_eq!(info.domain, *AGENTS_DOMAIN);
    }
}
