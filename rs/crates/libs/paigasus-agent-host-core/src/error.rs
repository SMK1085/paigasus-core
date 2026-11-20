// SPDX-License-Identifier: Apache-2.0

//! The domain error of the agent-host core crate.

/// A domain rule refused a value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    /// A raw string is not a valid ID of the named kind.
    #[error("invalid {kind} id")]
    InvalidId { kind: &'static str },
}
