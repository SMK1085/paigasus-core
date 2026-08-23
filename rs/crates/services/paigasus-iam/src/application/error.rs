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
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum TenancyError {
    #[error("slug is already taken in this scope")]
    SlugConflict,
    #[error("principal is already a member of this scope")]
    DuplicateMembership,
    #[error("email address is already taken")]
    EmailConflict,
    /// A `ServiceAccountService::create` targeted a name already taken by another service
    /// account under the SAME owner node (`ConflictKind::ServiceAccountNameTaken`,
    /// `uq_service_account_org_name`/`_team_name`/`_project_name`, SMA-445 D7) — a genuine
    /// user-facing 409, not an `Internal` placeholder (Task 5 deferred this mapping; Task 16
    /// fixes it).
    #[error("service account name is already taken for this owner")]
    ServiceAccountNameConflict,
    #[error("invalid email address")]
    InvalidEmail(String),
    #[error("invalid slug")]
    InvalidSlug(String),
    #[error("invalid name")]
    InvalidName(String),
    #[error("invalid resource prn")]
    InvalidPrn(String),
    /// SMA-586. The six variants below replace `InvalidPrn`'s former use as a catch-all
    /// sentinel for any validation failure without a dedicated code. Each carries a
    /// `&'static str` naming the offending wire field, interpolated into `Display` and
    /// emitted as `ErrorInfo.metadata["field"]` on gRPC.
    ///
    /// The payload type is load-bearing: a `&'static str` cannot hold caller-supplied input,
    /// so "never reflect untrusted input into an error body" is enforced by the type rather
    /// than remembered by each call site. The pre-SMA-586 sites passed a `format!` carrying
    /// the caller's raw value; that is now unrepresentable.
    #[error("invalid timestamp for {0}")]
    InvalidTimestamp(&'static str),
    #[error("{0} must be a uuid")]
    InvalidUuid(&'static str),
    /// Distinct from [`TenancyError::InvalidUuid`] on purpose: a cursor is an opaque,
    /// server-issued token, so a client can recover by restarting pagination without
    /// involving the user. Collapsing the two would make that indistinguishable from
    /// "your input is wrong".
    #[error("{0} is not a valid pagination cursor")]
    InvalidCursor(&'static str),
    #[error("{0} is not a known audit outcome")]
    InvalidAuditOutcome(&'static str),
    #[error("{0} is required")]
    MissingRequiredField(&'static str),
    /// HTTP-only, structurally — see the registry comment on
    /// `ERROR_REASON_MUTUALLY_EXCLUSIVE_FIELDS`: the gRPC surface models the same choice as a
    /// proto3 `oneof`, which cannot carry two values.
    #[error("provide exactly one of {0}")]
    MutuallyExclusiveFields(&'static str),
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
    /// A bulk dead-letter replay arrived without an explicit, non-zero `max_rows`
    /// (SMA-469). The required row budget IS the guard on blast radius — an "at least one
    /// filter must be present" check was rejected because `parked_from = 1970-01-01`
    /// satisfies it while matching everything, which is how an operator naturally writes
    /// "replay everything".
    #[error("bulk replay requires an explicit non-zero max_rows")]
    InvalidBulkReplay,
    /// The row at this id exists but is not system-owned, so `RetireSystemPolicy` refuses it
    /// (SMA-481 D7). Retirement must not become a second, differently-audited delete path for
    /// operator-authored policies — `DeletePolicy` already serves those and applies its own
    /// authorization. Raised for a non-system `policy` row AND for a non-system `role` row at
    /// the same id.
    #[error("not a system-owned row: {0}")]
    NotSystemOwned(String),
    /// At least one remaining STARTER POLICY row — one whose id the code catalog still defines
    /// — was last written by a binary older than this one (or carries no revision at all), so
    /// the fleet has not converged past the release that dropped the retiring id (SMA-481 D11).
    /// Deliberately measured over the code-defined set rather than every system-owned row: the
    /// orphan under retirement is itself system-owned and always carries an older revision, so
    /// counting it would refuse every genuine orphan. Retiring now would be silently undone:
    /// `classify_starter_policy`
    /// classifies an absent row as `Absent` BEFORE the revision guard runs, so any replica
    /// whose catalog still defines the id re-seeds it unconditionally.
    #[error("the fleet has not converged past this binary's starter policy revision")]
    FleetNotConverged,
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
            Self::ServiceAccountNameConflict => "service-account-name-conflict",
            Self::InvalidEmail(_) => "invalid-email",
            Self::InvalidSlug(_) => "invalid-slug",
            Self::InvalidName(_) => "invalid-name",
            Self::InvalidPrn(_) => "invalid-prn",
            Self::InvalidTimestamp(_) => "invalid-timestamp",
            Self::InvalidUuid(_) => "invalid-uuid",
            Self::InvalidCursor(_) => "invalid-cursor",
            Self::InvalidAuditOutcome(_) => "invalid-audit-outcome",
            Self::MissingRequiredField(_) => "missing-required-field",
            Self::MutuallyExclusiveFields(_) => "mutually-exclusive-fields",
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
            Self::InvalidBulkReplay => "invalid-bulk-replay",
            Self::NotSystemOwned(_) => "not-system-owned",
            Self::FleetNotConverged => "fleet-not-converged",
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
            | Self::InvalidTimestamp(_)
            | Self::InvalidUuid(_)
            | Self::InvalidCursor(_)
            | Self::InvalidAuditOutcome(_)
            | Self::MissingRequiredField(_)
            | Self::MutuallyExclusiveFields(_)
            | Self::PrnMismatch
            | Self::InvalidPagination
            | Self::NothingToRename
            | Self::UnknownRole(_)
            | Self::InvalidScope(_)
            | Self::PolicyInvalid(_)
            | Self::InvalidAction(_)
            | Self::InvalidBulkReplay => ErrorClass::Validation,
            Self::NotFound => ErrorClass::NotFound,
            Self::SlugConflict | Self::DuplicateMembership | Self::EmailConflict | Self::ServiceAccountNameConflict | Self::PolicyConflict(_) => ErrorClass::Conflict,
            Self::ParentArchived | Self::NodeArchived | Self::MissingOrgMembership | Self::SystemImmutable(_) | Self::NotSystemOwned(_) | Self::FleetNotConverged => ErrorClass::Precondition,
            Self::Forbidden => ErrorClass::Forbidden,
            Self::Internal => ErrorClass::Internal,
        }
    }

    /// The wire field name this error names, for `ErrorInfo.metadata["field"]` (SMA-586).
    ///
    /// `None` for every variant that does not carry one — including `InvalidPrn`, whose
    /// `String` payload is a PRN error-kind token or a canonical PRN, not a field name.
    /// Returning it here rather than matching on variants inside `status_to_grpc` keeps the
    /// transport layer free of variant knowledge.
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::InvalidTimestamp(f) | Self::InvalidUuid(f) | Self::InvalidCursor(f) | Self::InvalidAuditOutcome(f) | Self::MissingRequiredField(f) | Self::MutuallyExclusiveFields(f) => Some(f),
            _ => None,
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
                // M4 (SMA-445) variant: `ServiceAccountService::create` is the one caller
                // that can actually hit this (Task 16) — a genuine 409, not a placeholder.
                ConflictKind::ServiceAccountNameTaken => Self::ServiceAccountNameConflict,
                // `ApiKeyRepository::issue`'s hash collision is a genuine internal
                // shouldn't-happen event (an HMAC collision), not a user-facing conflict —
                // stays `Internal` (Task 16 brief).
                ConflictKind::ApiKeyHashCollision => Self::Internal,
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
            // API-key-only variant (SMA-445): tenancy operations never produce it, but the
            // match must stay exhaustive as `DomainError` grows across milestones.
            DomainError::InvalidApiKeyToken(_) => Self::Internal,
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

    /// SMA-445 Task 16 fix: `ServiceAccountNameTaken` is a genuine user-facing 409 (the
    /// per-owner unique-name conflict), not the `Internal` placeholder Task 5 left it as —
    /// `ApiKeyHashCollision` stays `Internal` (an HMAC collision, a shouldn't-happen event,
    /// not a client-facing conflict).
    #[test]
    fn service_account_name_taken_is_a_conflict_not_internal() {
        let err = TenancyError::from(RepositoryError::Conflict(ConflictKind::ServiceAccountNameTaken));
        assert_eq!(err, TenancyError::ServiceAccountNameConflict);
        assert_eq!(err.class(), ErrorClass::Conflict);
        assert_eq!(err.code(), "service-account-name-conflict");

        let hash_collision = TenancyError::from(RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision));
        assert_eq!(hash_collision, TenancyError::Internal);
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

    /// Both retirement refusals are Preconditions, not Conflicts. They render 409 either way
    /// today, but ErrorClass is what a future gRPC mirror translates — and SystemImmutable, the
    /// third refusal this same endpoint can return, is already Precondition (see the assertion
    /// above). Two sibling refusals on one endpoint must not diverge in class.
    #[test]
    fn the_retirement_refusals_share_system_immutable_s_class_and_have_stable_codes() {
        assert_eq!(TenancyError::NotSystemOwned("p1".to_string()).class(), ErrorClass::Precondition);
        assert_eq!(TenancyError::FleetNotConverged.class(), ErrorClass::Precondition);
        assert_eq!(TenancyError::SystemImmutable("p1".to_string()).class(), ErrorClass::Precondition);

        assert_eq!(TenancyError::NotSystemOwned("p1".to_string()).code(), "not-system-owned");
        assert_eq!(TenancyError::FleetNotConverged.code(), "fleet-not-converged");
    }

    /// SMA-586: the six reasons that replace `invalid-prn`'s catch-all duty. All are
    /// `Validation` — the sites they migrate are 400/InvalidArgument today and must stay so.
    #[test]
    fn the_request_validation_codes_are_stable_and_all_validation() {
        for (err, code) in [
            (TenancyError::InvalidTimestamp("from"), "invalid-timestamp"),
            (TenancyError::InvalidUuid("membership_id"), "invalid-uuid"),
            (TenancyError::InvalidCursor("cursor"), "invalid-cursor"),
            (TenancyError::InvalidAuditOutcome("outcome"), "invalid-audit-outcome"),
            (TenancyError::MissingRequiredField("owner_prn"), "missing-required-field"),
            (TenancyError::MutuallyExclusiveFields("principal|node"), "mutually-exclusive-fields"),
        ] {
            assert_eq!(err.code(), code);
            assert_eq!(err.class(), ErrorClass::Validation, "{code} must stay a 400");
        }
    }

    /// The field name reaches `Display` — the inverse of the pre-SMA-586 behaviour, where every
    /// call site passed a detail and `InvalidPrn`'s static Display threw it away.
    #[test]
    fn the_request_validation_displays_carry_their_field_name() {
        assert_eq!(TenancyError::InvalidTimestamp("parked_to").to_string(), "invalid timestamp for parked_to");
        assert_eq!(TenancyError::InvalidUuid("api_key_id").to_string(), "api_key_id must be a uuid");
        assert_eq!(TenancyError::InvalidCursor("cursor").to_string(), "cursor is not a valid pagination cursor");
        assert_eq!(TenancyError::InvalidAuditOutcome("outcome").to_string(), "outcome is not a known audit outcome");
        assert_eq!(TenancyError::MissingRequiredField("scope_prn").to_string(), "scope_prn is required");
        assert_eq!(TenancyError::MutuallyExclusiveFields("principal|node").to_string(), "provide exactly one of principal|node");
    }

    /// `field()` is what `status_to_grpc` uses to populate `ErrorInfo.metadata["field"]` without
    /// matching on variants at the transport layer. It is `None` for everything else — including
    /// `InvalidPrn`, whose `String` payload is deliberately NOT a field name.
    #[test]
    fn field_is_some_only_for_the_request_validation_variants() {
        assert_eq!(TenancyError::InvalidTimestamp("from").field(), Some("from"));
        assert_eq!(TenancyError::MutuallyExclusiveFields("a|b").field(), Some("a|b"));
        assert_eq!(TenancyError::InvalidPrn("iam:bad".to_string()).field(), None);
        assert_eq!(TenancyError::NotFound.field(), None);
        assert_eq!(TenancyError::Internal.field(), None);
    }
}
