// SPDX-License-Identifier: Apache-2.0

//! Hexagonal ports (traits) the service's adapters implement. Kept in the pure core so
//! use cases depend on abstractions, not on SeaORM/axum (ADR-0005).

use crate::api_key::{ApiKey, ApiKeyId};
use crate::audit::{AuditEntry, AuditFilter};
use crate::authn::{AuthnError, ExternalIdentity, Issuer, ValidatedClaims};
use crate::authz::model::RoleGrant;
use crate::domain_event::DomainEvent;
use crate::principal::{Principal, PrincipalStatus};
use crate::service_account::{ServiceAccount, ServiceAccountRecord};
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
    ServiceAccountNameTaken,
    ApiKeyHashCollision,
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
    /// Service accounts are kind-agnostic `Principal`s (D16), so this mints the same
    /// `principal` PRN shape as [`IdGenerator::new_principal_id`].
    fn new_service_account_id(&self) -> PrincipalId;
    /// A bare UUIDv7 (API keys are not tenancy/authz resources, so no PRN wrapper).
    fn new_api_key_id(&self) -> ApiKeyId;
    /// A bare, ordered UUIDv7 (audit entries are not tenancy/authz resources, so no PRN
    /// wrapper) — the ordering backs newest-first / id-descending keyset paging in
    /// [`AuditLog::query`].
    fn new_audit_id(&self) -> Uuid;
    /// A bare UUIDv7 identifying an outbox row (SMA-446, Slice B) — mints the same shape as
    /// [`IdGenerator::new_audit_id`], no PRN wrapper (domain events are not tenancy/authz
    /// resources).
    fn new_event_id(&self) -> Uuid;
    /// A bare UUIDv7 correlating a [`DomainEvent`] with the audit-log entry (and any other
    /// artifacts) produced by the same mutation (SMA-446, Slice B).
    fn new_correlation_id(&self) -> Uuid;
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

/// Hashes and verifies API-key secrets. Non-async — a pure keyed-hash computation, not I/O.
/// The pepper is injected into the adapter (never into the core), so the port surface is
/// just the two operations: `hash` for HMAC-SHA-256(pepper, secret) at issuance time, and
/// `verify` for a constant-time comparison against the stored hash at authn time.
pub trait SecretHasher: Send + Sync {
    fn hash(&self, secret: &[u8]) -> Vec<u8>;
    fn verify(&self, secret: &[u8], expected: &[u8]) -> bool;
}

/// A source of API-key secret entropy. Non-async and getrandom-free at this layer (the core
/// stays getrandom-free per ADR-0005) — the adapter supplies the actual RNG.
pub trait KeyEntropy: Send + Sync {
    fn new_secret(&self) -> [u8; 32];
}

/// Persistence port for service accounts (non-human `Principal`s, D16).
#[async_trait]
pub trait ServiceAccountRepository: Send + Sync {
    /// One transaction spanning principal + service_account (mirrors `PrincipalRepository::
    /// create_user`'s D9 pattern).
    async fn create(&self, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError>;
    /// Returns the `ServiceAccount` alongside its owning `Principal`'s lifecycle status (D16:
    /// status lives on the `Principal`, so a read of the account also reads the principal row).
    async fn find(&self, id: &PrincipalId) -> Result<Option<ServiceAccountRecord>, RepositoryError>;
    /// ORDER BY created_at, id (rule 9). Each entry's `status` is its own principal's status
    /// (mirrors `find`'s doc) — not the queried owner's, which has no status of its own here.
    async fn list_by_owner(&self, owner: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<ServiceAccountRecord>, RepositoryError>;
    /// Sets the lifecycle status on the underlying `Principal` row (D16: status lives on
    /// `Principal`, not `ServiceAccount`).
    async fn set_principal_status(&self, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError>;
}

/// Persistence port for API keys. The secret's hash is stored alongside the key metadata but
/// modeled separately from `ApiKey` (the domain entity never carries hash material).
#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    async fn issue(&self, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError>;
    /// Txn-scoped twin of [`ApiKeyRepository::issue`] (SMA-446, Slice B Task B6 — the
    /// `ApiKeyService::issue` reference pattern, copying `RoleGrantStore::grant_in`'s
    /// posture): inserts the row on the caller's own `tx`. Unlike `RoleGrantStore`/
    /// `PolicyStore`'s `_in` twins, there is no generation counter to skip bumping here —
    /// API-key issue/revoke never touch `policy_gen`/`entity_gen` at all (the caller's own
    /// post-commit side effect is the [`crate::adapters::api_keys::ApiKeyValidationCache`]
    /// evict on revoke, not a `PolicyGenBumper` bump).
    async fn issue_in(&self, tx: &dyn Transaction, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError>;
    /// The key plus its stored hash, for the authn adapter to `SecretHasher::verify` against.
    async fn find_by_id(&self, id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError>;
    async fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<(), RepositoryError>;
    /// Txn-scoped twin of [`ApiKeyRepository::revoke`]: flips `id`'s status to `Revoked` on
    /// the caller's own `tx`, returning whether THIS call actually performed the transition.
    /// `false` is an idempotent no-op — the row was already revoked, or never existed at all
    /// (a benign TOCTOU race; the caller already resolved the key via `find_by_id` before
    /// opening this transaction) — mirroring `RoleGrantStore::revoke_in`/`PolicyStore::
    /// delete_in`'s posture: the caller only enqueues/records when this is `true`. The
    /// caller's post-commit cache-evict (SECURITY-CRITICAL, spec §9/D5) runs unconditionally
    /// after every successful commit regardless of this bool — even a no-op revoke call must
    /// still clear any stale cached validation for `id`.
    async fn revoke_in(&self, tx: &dyn Transaction, id: ApiKeyId, now: DateTime<Utc>) -> Result<bool, RepositoryError>;
    /// ORDER BY created_at, id (rule 9).
    async fn list_by_service_account(&self, sa: &PrincipalId, limit: u64, offset: u64) -> Result<Vec<ApiKey>, RepositoryError>;
    /// All key ids owned by a service account, for archive-evict (revoking every key when its
    /// service account is archived) — no pagination, the whole set is needed at once.
    async fn list_ids_by_service_account(&self, sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError>;
    /// Updates `last_used_at` if more than `throttle_secs` has elapsed since the last update,
    /// to bound write amplification from hot keys.
    async fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>, throttle_secs: u64) -> Result<(), RepositoryError>;
}

/// Persistence port for the append-only audit log (SMA-446). `record_out_of_band` is called
/// after the triggering transaction commits (or after a denial), never inside it — the audit
/// log is not part of the domain transaction's atomicity guarantee.
#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError>;
    /// In-txn twin of [`AuditLog::record_out_of_band`] (Slice B): writes `e` on the caller's
    /// own `tx` (an opaque [`Transaction`], recovered by the adapter via its concrete
    /// downcast), so the audit row shares that transaction's atomicity — it becomes visible
    /// only if `tx` itself commits. Used when the audit entry must land alongside the
    /// mutation it describes (and typically that same mutation's [`Outbox::enqueue`]), unlike
    /// `record_out_of_band`'s deliberately-detached, always-durable write.
    async fn record(&self, tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError>;
    /// Results are newest-first by `id` (UUIDv7, so creation-time-ordered); keyset paging via
    /// [`AuditFilter::cursor`] also pages on that same `id`. `occurred_at` is assigned
    /// independently at entry-construction time and does not affect ordering or paging.
    async fn query(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError>;
}

/// Opens one atomic unit of work backed by a single database transaction (SMA-446, Slice B).
/// The application layer uses this to `begin` a [`Transaction`], drive one or more txn-scoped
/// mutations plus [`Outbox::enqueue`]/`AuditLog` writes through it, then commit — so the
/// aggregate mutation, its outbox row, and its audit row become visible atomically.
#[async_trait]
pub trait UnitOfWork: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError>;
}

/// A single in-flight database transaction. The port itself is backend-agnostic (ADR-0005);
/// adapters recover their concrete transaction type via `as_any().downcast_ref` — the entry
/// point later `Pg*` adapters use to run txn-scoped writes against the same connection.
///
/// `Sync` (not just `Send`, SMA-446 Slice B): [`Outbox::enqueue`]/[`AuditLog::record`] take
/// `&dyn Transaction` as an argument, and `#[async_trait]`'s default expansion requires the
/// resulting future to be `Send` — which requires the captured `&dyn Transaction` itself to be
/// `Send`, which in turn requires `dyn Transaction: Sync`. Without this bound, `PgOutbox`/
/// `PgAuditLog`'s txn-scoped methods fail to compile with "future cannot be sent between
/// threads safely".
#[async_trait]
pub trait Transaction: Send + Sync {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;
    /// Opens a nested transaction (a Postgres `SAVEPOINT` in the concrete adapter) so a
    /// conflict-absorbing mutation can roll back just the savepoint without aborting the
    /// outer transaction.
    async fn savepoint(&mut self) -> Result<Box<dyn Savepoint<'_>>, RepositoryError>;
    /// Downcast entry point for adapters (mirrors [`Transaction`]'s own doc).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// A nested transaction opened via [`Transaction::savepoint`], borrowing its parent
/// transaction for `'a`.
#[async_trait]
pub trait Savepoint<'a>: Send {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;
    async fn rollback(self: Box<Self>) -> Result<(), RepositoryError>;
    /// Downcast entry point for adapters (mirrors [`Transaction::as_any`]).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Persistence port for the transactional outbox (SMA-446, Slice B): writes a [`DomainEvent`]
/// row in the same transaction as the mutation that produced it, so the event only becomes
/// visible if that mutation itself commits.
#[async_trait]
pub trait Outbox: Send + Sync {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError>;
}

/// Publishes a relayed [`DomainEvent`] to an external sink (e.g. a message bus). Not part of
/// the domain transaction — invoked by the outbox relay after the event has already
/// committed, so failures here are retried out-of-band rather than rolling anything back.
#[async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError>;
}

/// A use case's injected, best-effort, POST-COMMIT side effect that bumps the authz
/// `policy_gen` counter (SMA-446, Slice B — the `RoleService::grant`/`revoke` reference
/// pattern B5–B7 copy). Kept as a port (rather than a direct `Generations` dependency) so the
/// application layer never imports the adapter-layer `crate::adapters::authz::Generations`
/// type (ADR-0005). Implementations swallow/log their own errors and return nothing — the
/// triggering mutation has already committed by the time `bump` runs, so a bump failure (e.g.
/// Redis unreachable) must never surface as a use-case error; the decision cache instead
/// self-heals on its next TTL expiry (mirrors the pre-Slice-B `PgRoleGrantStore::
/// bump_policy_gen_best_effort` this port's adapter impl is lifted from).
#[async_trait]
pub trait PolicyGenBumper: Send + Sync {
    async fn bump(&self);
}

/// Errors an [`EventPublisher`] can report, source-preserving like [`RepositoryError`] — the
/// core never assumes a specific transport.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("backend error")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
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
    fn new_repos_are_object_safe() {
        #[allow(dead_code)]
        fn _assert(_: &dyn ServiceAccountRepository, _: &dyn ApiKeyRepository, _: &dyn SecretHasher, _: &dyn KeyEntropy) {}
    }

    #[test]
    fn repository_error_wraps_a_source_error() {
        let e: RepositoryError = Box::<dyn std::error::Error + Send + Sync>::from("boom").into();
        assert!(matches!(e, RepositoryError::Backend(_)));
    }

    // Compile-time proof the audit port is object-safe (injected as a trait object).
    #[allow(dead_code)]
    fn audit_log_is_object_safe(_: &dyn AuditLog) {}

    // Compile-time proof the new UoW/outbox/event-publisher/gen-bumper ports are object-safe
    // (SMA-446, Slice B).
    #[allow(dead_code)]
    fn unit_of_work_ports_are_object_safe(_: &dyn UnitOfWork, _: &dyn Outbox, _: &dyn EventPublisher, _: &dyn PolicyGenBumper) {}

    // `Transaction`/`Savepoint` are only ever held as `Box<dyn ...>` (their `commit`/
    // `rollback` methods consume `self: Box<Self>`), so prove object-safety via that receiver
    // rather than `&dyn`.
    #[allow(dead_code)]
    fn transaction_and_savepoint_are_object_safe(_: Box<dyn Transaction>, _: Box<dyn Savepoint<'_>>) {}

    #[test]
    fn publish_error_wraps_a_source_error() {
        let e: PublishError = Box::<dyn std::error::Error + Send + Sync>::from("boom").into();
        assert!(matches!(e, PublishError::Backend(_)));
    }
}
