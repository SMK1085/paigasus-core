// SPDX-License-Identifier: Apache-2.0

//! Postgres persistence adapter: entities, migrations, and the repository impls.

pub mod entities;
pub mod migration;
pub mod pg_external_identities;
pub mod pg_memberships;
pub mod pg_organizations;
pub mod pg_policies;
pub mod pg_projects;
pub mod pg_repository;
pub mod pg_role_grants;
pub mod pg_teams;

pub use migration::Migrator;
pub use pg_external_identities::PgExternalIdentityRepository;
pub use pg_memberships::PgMembershipRepository;
pub use pg_organizations::PgOrganizationRepository;
pub use pg_policies::PgPolicyStore;
pub use pg_projects::PgProjectRepository;
pub use pg_repository::PgPrincipalRepository;
pub use pg_role_grants::PgRoleGrantStore;
pub use pg_teams::PgTeamRepository;

use paigasus_iam_core::{ConflictKind, RepositoryError};
use sea_orm::{DbErr, SqlErr};

/// Maps a SeaORM backend error into the core's `RepositoryError`: a uniqueness violation is
/// attributed to a `ConflictKind` by constraint name (D7), a foreign-key violation means the
/// referenced parent row is gone (`NotFound`), anything else is an opaque `Backend` error.
/// Shared by every Postgres repository adapter under this module.
pub(crate) fn map_err(e: DbErr) -> RepositoryError {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(msg)) => RepositoryError::Conflict(conflict_kind(&msg)),
        Some(SqlErr::ForeignKeyConstraintViolation(_)) => RepositoryError::NotFound,
        _ => RepositoryError::Backend(Box::new(e)),
    }
}

/// D7: attribute conflicts by constraint name only; never surface PG text (e.g. the raw
/// message embeds `DETAIL: Key (email)=(...)` — PII).
pub(crate) fn conflict_kind(msg: &str) -> ConflictKind {
    const SLUG: [&str; 3] = ["uq_organization_slug", "uq_team_org_slug", "uq_project_team_slug"];
    if SLUG.iter().any(|c| msg.contains(c)) {
        ConflictKind::SlugTaken
    } else if msg.contains("uq_membership_") {
        ConflictKind::DuplicateMembership
    } else if msg.contains("user_email_key") {
        ConflictKind::EmailTaken
    } else if msg.contains("uq_external_identity_issuer_subject") {
        ConflictKind::ExternalIdentityExists
    } else {
        ConflictKind::Other // includes uq_*_prn: a UUIDv7 collision is an internal error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fast, Docker-independent coverage of the constraint-name -> ConflictKind mapping; the
    // round-trip tests (tests/roundtrip.rs, tests/tenancy_orgs.rs) additionally prove it
    // end-to-end against real Postgres, but only run when Docker is available.
    #[test]
    fn conflict_kind_maps_constraint_names() {
        assert_eq!(conflict_kind("uq_organization_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_team_org_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_project_team_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_membership_principal_node"), ConflictKind::DuplicateMembership);
        assert_eq!(conflict_kind("user_email_key"), ConflictKind::EmailTaken);
        assert_eq!(conflict_kind("uq_external_identity_issuer_subject"), ConflictKind::ExternalIdentityExists);
        assert_eq!(conflict_kind("uq_organization_prn"), ConflictKind::Other);
        assert_eq!(conflict_kind("some other constraint"), ConflictKind::Other);
    }
}
