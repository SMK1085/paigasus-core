// SPDX-License-Identifier: Apache-2.0

//! Service-layer error taxonomy, mapping domain/repository errors into a stable API.

use paigasus_iam_core::{AuthzError, ConflictKind, DomainError, PreconditionKind, RepositoryError};

/// Classification of errors for routing to client handlers (HTTP status, gRPC code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Validation,
    NotFound,
    Conflict,
    Precondition,
    Forbidden,
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
    /// Cedar denied the request (or the caller lacks a matching grant). The message is
    /// deliberately STATIC — never interpolated with the denying policy id or resource
    /// detail; that detail belongs in the audit log / `IsAuthorized` response for
    /// authorized callers, never in a 403 wire body (SMA-444 task-16 brief).
    #[error("access denied")]
    Forbidden,
    /// `RoleService::grant` was asked to grant a role key `authz::roles::role` doesn't
    /// recognize (SMA-444 Task 17).
    #[error("unknown role: {0}")]
    UnknownRole(String),
    /// `RoleService::grant`'s scope PRN parsed fine, but its `NodeKind` isn't in the role's
    /// `scope_kinds` allow-list (e.g. granting an `Organization`-scoped role at a `Team`) —
    /// SMA-444 Task 17.
    #[error("invalid grant scope: {0}")]
    InvalidScope(String),
    /// `PolicyService::put`/`delete` targeted an already-persisted `system = true` policy
    /// row — immutable via the CRUD API (`AuthzError::SystemImmutable`, SMA-444 Task 17).
    #[error("system-owned resource is immutable: {0}")]
    SystemImmutable(String),
    /// `PolicyService::put`'s document failed Cedar parse/schema/template-link validation
    /// (`AuthzError::PolicyParse`/`SchemaValidation`/`TemplateLink`, SMA-444 Task 17).
    #[error("invalid policy: {0}")]
    PolicyInvalid(String),
    /// `PolicyService::put` lost a concurrent-create race against a DIFFERENT document for
    /// the same `policy_id` (`AuthzError::Conflict`, SMA-444 review fix): the stored row
    /// belongs to the race's winner, not this caller's write — a 409, not a silent success.
    #[error("policy conflict: {0}")]
    PolicyConflict(String),
    /// `POST /v1/authz/is-authorized`'s `action` field didn't name a known `Action` variant
    /// (`Action::parse` returned `None`, SMA-444 Task 18) — a client error, not an authz
    /// decision, so it's a 400 rather than a `Deny`.
    #[error("unknown action: {0}")]
    InvalidAction(String),
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
            Self::Forbidden => "forbidden",
            Self::UnknownRole(_) => "unknown-role",
            Self::InvalidScope(_) => "invalid-scope",
            Self::SystemImmutable(_) => "system-immutable",
            Self::PolicyInvalid(_) => "policy-invalid",
            Self::PolicyConflict(_) => "policy-conflict",
            Self::InvalidAction(_) => "invalid-action",
            Self::Internal => "internal",
        }
    }

    /// Returns the error's classification for routing to client handlers.
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::InvalidEmail(_)
            | Self::InvalidSlug(_)
            | Self::InvalidName(_)
            | Self::InvalidPrn(_)
            | Self::PrnMismatch
            | Self::InvalidPagination
            | Self::NothingToRename
            | Self::UnknownRole(_)
            | Self::InvalidScope(_)
            | Self::PolicyInvalid(_)
            | Self::InvalidAction(_) => ErrorClass::Validation,
            Self::NotFound => ErrorClass::NotFound,
            Self::SlugConflict | Self::DuplicateMembership | Self::EmailConflict | Self::PolicyConflict(_) => ErrorClass::Conflict,
            Self::ParentArchived | Self::NodeArchived | Self::MissingOrgMembership | Self::SystemImmutable(_) => ErrorClass::Precondition,
            Self::Forbidden => ErrorClass::Forbidden,
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

/// Maps the authz core's error taxonomy onto `TenancyError` (SMA-444 Task 17) — used by
/// `Authorize::check`/`RoleService`/`PolicyService` via `?`. `Evaluation`/`Backend` are
/// genuine engine/storage failures, never a denial — they surface as `Internal`, NOT
/// `Forbidden` (a deny is `Authorize::check`'s own `Effect::Deny` branch, not this impl).
/// `Conflict` (a lost concurrent-create race, e.g. `PolicyStore::put`) maps to
/// `PolicyConflict`, a 409. `ResourceNotFound` is caught and turned into a fail-closed `Deny`
/// by `CedarAuthorizer::is_authorized` before it ever reaches this funnel (SMA-444 review
/// fix); if it somehow leaked past that — a future `AuthzError`-returning call site that
/// doesn't handle it — that is a bug, not an expected client-facing case, so it maps to
/// `Internal` rather than `NotFound`: an authz-layer error must never double as a
/// resource-existence oracle.
impl From<AuthzError> for TenancyError {
    fn from(err: AuthzError) -> Self {
        match err {
            AuthzError::UnknownRole(s) => Self::UnknownRole(s),
            AuthzError::InvalidScope(s) => Self::InvalidScope(s),
            AuthzError::SystemImmutable(s) => Self::SystemImmutable(s),
            AuthzError::PolicyParse(s) | AuthzError::SchemaValidation(s) | AuthzError::TemplateLink(s) => Self::PolicyInvalid(s),
            AuthzError::Conflict(s) => Self::PolicyConflict(s),
            AuthzError::Evaluation(_) | AuthzError::Backend(_) | AuthzError::ResourceNotFound(_) => Self::Internal,
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
        assert_eq!(TenancyError::Forbidden.code(), "forbidden");
        assert_eq!(TenancyError::from(RepositoryError::Conflict(ConflictKind::Other)).code(), "internal");
    }

    #[test]
    fn error_classes_are_correct() {
        assert_eq!(TenancyError::SlugConflict.class(), ErrorClass::Conflict);
        assert_eq!(TenancyError::NotFound.class(), ErrorClass::NotFound);
        assert_eq!(TenancyError::InvalidEmail("test".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::ParentArchived.class(), ErrorClass::Precondition);
        assert_eq!(TenancyError::Forbidden.class(), ErrorClass::Forbidden);
        assert_eq!(TenancyError::Internal.class(), ErrorClass::Internal);
        assert_eq!(TenancyError::UnknownRole("x".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::InvalidScope("x".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::PolicyInvalid("x".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::SystemImmutable("x".to_string()).class(), ErrorClass::Precondition);
        assert_eq!(TenancyError::InvalidAction("x".to_string()).class(), ErrorClass::Validation);
        assert_eq!(TenancyError::PolicyConflict("x".to_string()).class(), ErrorClass::Conflict);
    }

    #[test]
    fn invalid_action_code_is_stable() {
        assert_eq!(TenancyError::InvalidAction("Bogus".to_string()).code(), "invalid-action");
    }

    #[test]
    fn from_authz_error_maps_correctly() {
        assert_eq!(TenancyError::from(AuthzError::UnknownRole("r".to_string())), TenancyError::UnknownRole("r".to_string()));
        assert_eq!(TenancyError::from(AuthzError::InvalidScope("s".to_string())), TenancyError::InvalidScope("s".to_string()));
        assert_eq!(TenancyError::from(AuthzError::SystemImmutable("p".to_string())), TenancyError::SystemImmutable("p".to_string()));
        assert_eq!(TenancyError::from(AuthzError::PolicyParse("bad".to_string())), TenancyError::PolicyInvalid("bad".to_string()));
        assert_eq!(TenancyError::from(AuthzError::SchemaValidation("bad".to_string())), TenancyError::PolicyInvalid("bad".to_string()));
        assert_eq!(TenancyError::from(AuthzError::TemplateLink("bad".to_string())), TenancyError::PolicyInvalid("bad".to_string()));
        assert_eq!(TenancyError::from(AuthzError::Evaluation("boom".to_string())), TenancyError::Internal);
        let backend: Box<dyn std::error::Error + Send + Sync> = "boom".into();
        assert_eq!(TenancyError::from(AuthzError::Backend(backend)), TenancyError::Internal);
        assert_eq!(TenancyError::from(AuthzError::Conflict("p1".to_string())), TenancyError::PolicyConflict("p1".to_string()));
        assert_eq!(TenancyError::from(AuthzError::ResourceNotFound("org 1".to_string())), TenancyError::Internal);
    }

    #[test]
    fn forbidden_message_is_static_and_generic() {
        // The Display never carries interpolated data (mirrors `Internal`, D7-style
        // contract) — the denying policy id belongs in the audit log, not the wire body.
        assert_eq!(TenancyError::Forbidden.to_string(), "access denied");
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
