// SPDX-License-Identifier: Apache-2.0

//! Hexagonal ports (traits) the service's adapters implement. Kept in the pure core so
//! use cases depend on abstractions, not on SeaORM/axum (ADR-0005).

use crate::authn::{AuthnError, ExternalIdentity, Issuer, ValidatedClaims};
use crate::authz::model::RoleGrant;
use crate::principal::Principal;
use crate::tenancy::{Membership, NodeStatus, Organization, OrganizationId, Project, ProjectId, Slug, Team, TeamId, TenancyNodeRef};
use crate::user::User;
use crate::value::PrincipalId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Kinds of uniqueness conflicts a repository can report (D7: never surface raw backend text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    SlugTaken,
    DuplicateMembership,
    EmailTaken,
    ExternalIdentityExists,
    Other,
}

/// Kinds of precondition failures a repository can report (in-txn guards, D8/D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreconditionKind {
    ParentArchived,
    NodeArchived,
    MissingOrgMembership,
}

/// Persistence errors, source-preserving. The adapter maps its backend error (e.g. SeaORM
/// `DbErr`) into these; the core never imports the backend.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("conflict: {0:?}")]
    Conflict(ConflictKind),
    #[error("not found")]
    NotFound,
    #[error("prn does not match stored resource")]
    PrnMismatch,
    #[error("precondition failed: {0:?}")]
    Precondition(PreconditionKind),
    #[error("backend error")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// A tenancy node together with its effective status (own status folded with ancestors, D1/D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeView<T> {
    pub node: T,
    pub effective_status: NodeStatus,
}

/// A persisted membership row (D5: plain UUIDv7 id, no PRN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipRecord {
    pub id: Uuid,
    pub principal_prn: String,
    pub node_prn: String,
    pub created_at: DateTime<Utc>,
}

/// Persistence port for user-principals.
#[async_trait]
pub trait PrincipalRepository: Send + Sync {
    async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError>;
    async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError>;
    async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError>;
}

/// Persistence port for external (IdP) identities linked to a principal.
#[async_trait]
pub trait ExternalIdentityRepository: Send + Sync {
    async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError>;
    /// One transaction spanning principal + user + external_identity (D9).
    async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError>;
}

/// Persistence port for organizations.
#[async_trait]
pub trait OrganizationRepository: Send + Sync {
    /// One transaction: org + auto-provisioned default team (ADR-0014) + the creating
    /// principal's `org_admin` owner grant, scoped to the new org (spec D8) — a grant is a
    /// policy change, so implementations must also bump `policy_gen` (in addition to the
    /// usual `entity_gen` bump every tenancy mutation gets).
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant) -> Result<(), RepositoryError>;
    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Organization>>, RepositoryError>;
    /// ORDER BY created_at, id (rule 9).
    async fn list(&self, limit: u64, offset: u64) -> Result<Vec<NodeView<Organization>>, RepositoryError>;
    /// In-txn guard: NotFound if missing; Precondition(NodeArchived) if own status archived.
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError>;
    /// Sets own status (D10). Idempotent: no-op (updated_at untouched) when already `status`.
    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError>;
}

/// Persistence port for teams.
#[async_trait]
pub trait TeamRepository: Send + Sync {
    /// In-txn guards (D8): org row locked FOR SHARE; NotFound if org missing;
    /// Precondition(ParentArchived) if org effectively archived.
    async fn create(&self, team: &Team) -> Result<(), RepositoryError>;
    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Team>>, RepositoryError>;
    async fn list_by_org(&self, org: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Team>>, RepositoryError>;
    /// Guard: Precondition(NodeArchived) if team is EFFECTIVELY archived (own or org).
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError>;
    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError>;
}

/// Persistence port for projects.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// In-txn guards: team+org locked FOR SHARE; NotFound if team missing; Precondition(ParentArchived)
    /// if team effectively archived; Backend if project.org != team.org (belt-and-braces, composite FK).
    async fn create(&self, project: &Project) -> Result<(), RepositoryError>;
    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Project>>, RepositoryError>;
    async fn list_by_team(&self, team: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Project>>, RepositoryError>;
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError>;
    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError>;
}

/// Persistence port for memberships.
#[async_trait]
pub trait MembershipRepository: Send + Sync {
    /// One transaction, all guards in-txn with FOR SHARE locks (D8, rule 8):
    /// principal exists + stored-prn byte-match (else NotFound / PrnMismatch);
    /// node exists + stored-prn byte-match; node effectively active (else Precondition(NodeArchived));
    /// team/project targets: org membership exists, locked (else Precondition(MissingOrgMembership));
    /// duplicate -> Conflict(DuplicateMembership).
    async fn attach(&self, membership: &Membership) -> Result<MembershipRecord, RepositoryError>;
    async fn find(&self, id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError>;
    /// NotFound if missing. Org memberships cascade: also deletes the principal's
    /// team/project memberships in that org, one transaction (rule 5).
    async fn detach(&self, id: Uuid) -> Result<(), RepositoryError>;
    async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
    /// Resolves node by uuid; PrnMismatch if the supplied ref's canonical != stored prn; NotFound if absent.
    async fn list_by_node(&self, node: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
}

/// Mints new identities (UUIDv7 + PRN). Impure (clock + entropy) — hence a port.
pub trait IdGenerator: Send + Sync {
    fn new_principal_id(&self) -> PrincipalId;
    fn new_organization_id(&self) -> OrganizationId;
    fn new_team_id(&self, org: Uuid) -> TeamId;
    fn new_project_id(&self, org: Uuid) -> ProjectId;
    fn new_membership_id(&self) -> Uuid;
    fn new_external_identity_id(&self) -> Uuid;
}

/// A source of the current time, truncated to microseconds so values round-trip through
/// Postgres `TIMESTAMPTZ` (µs resolution) bit-for-bit.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Verifies a presented bearer token and extracts its claims.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// The pluggable port (ADR-0015). OIDC validator is the v1 impl.
    async fn authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof the repository ports are object-safe (injected as trait objects).
    #[allow(dead_code)]
    fn assert_object_safe(
        _: &dyn PrincipalRepository,
        _: &dyn OrganizationRepository,
        _: &dyn TeamRepository,
        _: &dyn ProjectRepository,
        _: &dyn MembershipRepository,
        _: &dyn ExternalIdentityRepository,
        _: &dyn Authenticator,
    ) {
    }

    #[test]
    fn repository_error_wraps_a_source_error() {
        let e: RepositoryError = Box::<dyn std::error::Error + Send + Sync>::from("boom").into();
        assert!(matches!(e, RepositoryError::Backend(_)));
    }
}
