// SPDX-License-Identifier: Apache-2.0

//! The domain error of the agent-host core crate.

use uuid::Uuid;

use crate::deferral::PendingToolCallState;

/// A domain rule refused a value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum DomainError {
    /// A raw string is not a valid ID of the named kind.
    #[error("invalid {kind} id")]
    InvalidId { kind: &'static str },
    /// The agent and the inbox belong to different organizations.
    #[error("the inbox org {inbox_org} and the agent org {agent_org} differ")]
    OrgMismatch { inbox_org: Uuid, agent_org: Uuid },
    /// The state machine of a pending tool call does not allow the change.
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: PendingToolCallState, to: PendingToolCallState },
}
