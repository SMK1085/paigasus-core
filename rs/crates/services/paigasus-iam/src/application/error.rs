// SPDX-License-Identifier: Apache-2.0

//! Service-layer error taxonomy, mapping domain/repository errors into a stable API.

use paigasus_iam_core::{ConflictKind, DomainError, PreconditionKind, RepositoryError};

/// Classification of errors for routing to client handlers (HTTP status, gRPC code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Validation,
    NotFound,
    Conflict,
    Precondition,
    Internal,
}

/// Service-layer error taxonomy, combining domain and repository failures.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TenancyError {
    #[error("slug is already taken in this scope")]
    SlugConflict,
    #[error("principal is already a member of this scope")]
    DuplicateMembership,
    #[error("email address is already taken")]
    EmailConflict,
    #[error("invalid email address")]
    InvalidEmail(String),
    #[error("invalid slug")]
    InvalidSlug(String),
    #[error("invalid name")]
    InvalidName(String),
    #[error("invalid resource prn")]
    InvalidPrn(String),
    #[error("prn does not match stored resource")]
    PrnMismatch,
    #[error("invalid pagination parameters")]
    InvalidPagination,
    #[error("nothing to rename")]
    NothingToRename,
    #[error("resource not found")]
    NotFound,
    #[error("parent resource is archived")]
    ParentArchived,
    #[error("resource is archived")]
    NodeArchived,
    #[error("principal is not a member of the organization")]
    MissingOrgMembership,
    #[error("internal server error")]
    Internal,
}

impl TenancyError {
    /// Returns a stable, kebab-case error code (load-bearing for API contracts).
    pub fn code(&self) -> &'static str {
        match self {
            Self::SlugConflict => "slug-conflict",
            Self::DuplicateMembership => "duplicate-membership",
            Self::EmailConflict => "email-conflict",
            Self::InvalidEmail(_) => "invalid-email",
            Self::InvalidSlug(_) => "invalid-slug",
            Self::InvalidName(_) => "invalid-name",
            Self::InvalidPrn(_) => "invalid-prn",
            Self::PrnMismatch => "prn-mismatch",
            Self::InvalidPagination => "invalid-pagination",
            Self::NothingToRename => "nothing-to-rename",
            Self::NotFound => "not-found",
            Self::ParentArchived => "parent-archived",
            Self::NodeArchived => "node-archived",
            Self::MissingOrgMembership => "missing-org-membership",
            Self::Internal => "internal",
        }
    }

    /// Returns the error's classification for routing to client handlers.
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::InvalidEmail(_) | Self::InvalidSlug(_) | Self::InvalidName(_) | Self::InvalidPrn(_) | Self::PrnMismatch | Self::InvalidPagination | Self::NothingToRename => ErrorClass::Validation,
            Self::NotFound => ErrorClass::NotFound,
            Self::SlugConflict | Self::DuplicateMembership | Self::EmailConflict => ErrorClass::Conflict,
            Self::ParentArchived | Self::NodeArchived | Self::MissingOrgMembership => ErrorClass::Precondition,
            Self::Internal => ErrorClass::Internal,
        }
    }
}

impl From<RepositoryError> for TenancyError {
    fn from(err: RepositoryError) -> Self {
        match err {
            RepositoryError::Conflict(kind) => match kind {
                ConflictKind::SlugTaken => Self::SlugConflict,
                ConflictKind::DuplicateMembership => Self::DuplicateMembership,
                ConflictKind::EmailTaken => Self::EmailConflict,
                // Authn-only variant (SMA-443): tenancy operations never produce it, but the
                // match must stay exhaustive as `ConflictKind` grows across milestones.
                ConflictKind::ExternalIdentityExists => Self::Internal,
                ConflictKind::Other => Self::Internal,
            },
            RepositoryError::NotFound => Self::NotFound,
            RepositoryError::PrnMismatch => Self::PrnMismatch,
            RepositoryError::Precondition(kind) => match kind {
                PreconditionKind::ParentArchived => Self::ParentArchived,
                PreconditionKind::NodeArchived => Self::NodeArchived,
                PreconditionKind::MissingOrgMembership => Self::MissingOrgMembership,
            },
            RepositoryError::Backend(_) => Self::Internal,
        }
    }
}

impl From<DomainError> for TenancyError {
    fn from(err: DomainError) -> Self {
        match err {
            DomainError::InvalidEmail(s) => Self::InvalidEmail(s),
            DomainError::InvalidSlug(s) => Self::InvalidSlug(s),
            DomainError::InvalidName(s) => Self::InvalidName(s),
            DomainError::InvalidNodePrn(s) => Self::InvalidPrn(s),
            // Authn-only variant (SMA-443): tenancy operations never produce it, but the
            // match must stay exhaustive as `DomainError` grows across milestones.
            DomainError::InvalidIssuer(_) => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_kebab_and_stable() {
        assert_eq!(TenancyError::SlugConflict.code(), "slug-conflict");
        assert_eq!(TenancyError::MissingOrgMembership.code(), "missing-org-membership");
        assert_eq!(TenancyError::from(RepositoryError::Conflict(ConflictKind::Other)).code(), "internal");
    }

    #[test]
    fn error_classes_are_correct() {
        assert_eq!(TenancyError::SlugConflict.class(), ErrorClass::Conflict);
        assert_eq!(TenancyError::NotFound.class(), ErrorClass::NotFound);
        assert_eq!(TenancyError::InvalidEmail("test".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::ParentArchived.class(), ErrorClass::Precondition);
        assert_eq!(TenancyError::Internal.class(), ErrorClass::Internal);
    }

    #[test]
    fn from_repository_error_maps_correctly() {
        let err = TenancyError::from(RepositoryError::Conflict(ConflictKind::SlugTaken));
        assert_eq!(err, TenancyError::SlugConflict);

        let err = TenancyError::from(RepositoryError::NotFound);
        assert_eq!(err, TenancyError::NotFound);

        let err = TenancyError::from(RepositoryError::PrnMismatch);
        assert_eq!(err, TenancyError::PrnMismatch);

        let err = TenancyError::from(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        assert_eq!(err, TenancyError::NodeArchived);
    }

    #[test]
    fn from_domain_error_maps_correctly() {
        let err = TenancyError::from(DomainError::InvalidEmail("bad@".to_string()));
        assert!(matches!(err, TenancyError::InvalidEmail(_)));

        let err = TenancyError::from(DomainError::InvalidSlug("bad-".to_string()));
        assert!(matches!(err, TenancyError::InvalidSlug(_)));

        let err = TenancyError::from(DomainError::InvalidName("".to_string()));
        assert!(matches!(err, TenancyError::InvalidName(_)));

        let err = TenancyError::from(DomainError::InvalidNodePrn("bad-prn".to_string()));
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }
}
