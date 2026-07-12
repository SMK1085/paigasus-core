// SPDX-License-Identifier: Apache-2.0

//! `RoleService`: grant/revoke/list role-grant management use cases (SMA-444 Task 17,
//! ADR-0013). `grant` enforces the anti-escalation invariant — only an actor who may already
//! `GrantRole` AT the target scope itself (Root, or a tenancy node) may grant a role there,
//! so a principal can never bootstrap authority it doesn't already hold. `list`'s exposure
//! rule is a deliberate M3 simplification: self is always visible, anyone else's grants
//! require platform-level (`Root`-scoped) `ListRoleGrants` — see [`RoleService::list`]'s doc.
//!
//! **SMA-444 cross-tenant-escalation fix (defense-in-depth):** `parse_grant_scope` builds a
//! `TenancyNodeRef` straight from the caller's raw `scope_prn` string, whose org slot
//! `TenancyNodeRef::from_prn` only checks is PRESENT, never that it's CORRECT
//! (`tenancy::check`) — a team/project scope can name a real node's uuid paired with an
//! arbitrary org uuid. The root-cause fix lives in `PgEntitySliceLoader` (the `Team` branch
//! now parents on the node's REAL stored org, never the caller's PRN), which alone closes the
//! escalation for every decision path. `grant` additionally calls [`RoleService::resolve_scope`]
//! before persisting: it re-resolves the scope node against the DB and rejects a caller PRN
//! that doesn't byte-match the node's real canonical PRN — mirroring the forged-org-slot
//! defense `PgMembershipRepository::attach`/`InMemoryMemberships::attach` already apply to a
//! membership's `node_prn` (`RepositoryError::PrnMismatch`). Without this, a grant made by an
//! actor who legitimately holds authority over the node's REAL parent could still persist a
//! `RoleGrant` whose `scope.canonical_prn()` misrepresents that parent — a data-integrity gap
//! this closes, on top of (not instead of) the entity-slice loader's own fix.
//!
//! **SMA-446 Slice B — the Unit-of-Work reference pattern (Task B4, copied verbatim by
//! B5–B7):** once `grant`/`revoke`'s existing authorize/resolve checks pass, the mutation, its
//! [`DomainEvent`], and its [`AuditEntry`] all share ONE freshly-minted `correlation_id` and
//! commit together on ONE [`UnitOfWork`]-scoped transaction (`grants.grant_in`/`revoke_in`,
//! `outbox.enqueue`, `audit.record`, then `tx.commit()`) — so a mid-txn failure leaves NONE of
//! the three behind, never a partial write. Only once that commit succeeds does the service
//! run its one post-commit side effect: an AWAITED `gen_bumper.bump()` (best-effort/swallowed
//! by the [`PolicyGenBumper`] impl itself) — awaited so the bump is guaranteed to have
//! happened by the time `grant`/`revoke` returns (preserves AC1: the very next `is_authorized`
//! call sees the change), and never run before the commit, so a rolled-back mutation can never
//! bump the generation counter for a change that was never actually persisted. `revoke` of an
//! already-gone grant (`revoke_in` returns `false` — an idempotent race, not an error) emits
//! NOTHING: no event, no audit entry, no bump — a no-op stays a true no-op. The application
//! layer never imports `crate::adapters::authz::Generations` directly; it depends only on the
//! [`PolicyGenBumper`] port (ADR-0005).

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::roles as authz_roles;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, GrantScope, IdGenerator, OrganizationRepository, Outbox, PolicyGenBumper, PrincipalId, ProjectRepository, RoleGrant,
    RoleGrantStore, TeamRepository, TenancyNodeRef, UnitOfWork,
};
use paigasus_kernel::Prn;
use std::sync::Arc;
use uuid::Uuid;

/// Parses a raw principal PRN string: must be syntactically valid (else `InvalidPrn` with the
/// kernel's stable error-kind token), and must be service `"iam"`, resource type
/// `"principal"` (else `InvalidPrn` with the PRN's canonical form). Mirrors
/// `application::memberships::parse_principal_prn` — duplicated rather than shared across
/// modules (a five-line pure parse, not worth a visibility change on an unrelated file).
fn parse_principal_prn(raw: &str) -> Result<PrincipalId, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if prn.service() != "iam" || prn.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(prn.canonical()));
    }
    Ok(PrincipalId::from_prn(prn))
}

/// Parses a raw grant-scope PRN string into a [`GrantScope`]: the synthetic Root sentinel's
/// own canonical PRN maps to `GrantScope::Root`; anything else must parse as a tenancy-node
/// PRN (`TenancyNodeRef::from_prn`, whose `DomainError` auto-converts to `TenancyError::
/// InvalidPrn`) and becomes `GrantScope::Node`.
fn parse_grant_scope(raw: &str) -> Result<GrantScope, TenancyError> {
    if raw == root_prn().canonical() {
        return Ok(GrantScope::Root);
    }
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(GrantScope::Node(TenancyNodeRef::from_prn(prn)?))
}

/// The `Prn` a [`GrantScope`] represents as an authorization *resource* — `root_prn()` for
/// `Root`, or the tenancy node's own PRN. This is what `grant`/`revoke` authorize the actor
/// against: the scope node itself, not the grant's target principal (the anti-escalation
/// invariant — see module docs).
fn scope_resource_prn(scope: &GrantScope) -> Prn {
    match scope {
        GrantScope::Root => root_prn(),
        GrantScope::Node(TenancyNodeRef::Organization(id)) => id.prn().clone(),
        GrantScope::Node(TenancyNodeRef::Team(id)) => id.prn().clone(),
        GrantScope::Node(TenancyNodeRef::Project(id)) => id.prn().clone(),
    }
}

/// Role-grant lifecycle use cases. `grants` is `Arc<dyn RoleGrantStore>` (not generic-DI) —
/// it's the same shared handle `AppState` composes into `PolicySnapshot`, so a later task's
/// wiring clones one `Arc` rather than standing up a second store instance. `orgs`/`teams`/
/// `projects` are likewise `Arc<dyn ...>` trait objects (SMA-444 cross-tenant-escalation fix,
/// module docs) — [`RoleService::resolve_scope`]'s own DB-lookup defense, independent of
/// `grants`. `uow`/`outbox`/`audit`/`gen_bumper` are SMA-446 Slice B's Unit-of-Work reference
/// pattern (module docs): `grant`/`revoke` drive the mutation + its outbox event + its audit
/// entry through `uow` atomically, then run `gen_bumper`'s awaited, best-effort post-commit
/// bump. `ids`/`clock` stay generic-DI, mirroring `MembershipService`.
#[derive(Clone)]
pub struct RoleService<I, C> {
    grants: Arc<dyn RoleGrantStore>,
    orgs: Arc<dyn OrganizationRepository>,
    teams: Arc<dyn TeamRepository>,
    projects: Arc<dyn ProjectRepository>,
    authorize: Authorize,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn PolicyGenBumper>,
    ids: I,
    clock: C,
}

/// Named-field constructor params for [`RoleService::new`] (SMA-446 Slice B Task B4) — the
/// DI-params idiom sibling services (B5–B7) should copy verbatim rather than growing another
/// long positional-argument constructor: one field per dependency, built with struct syntax at
/// the call site so each argument is self-labeling and reordering/inserting a field can't
/// silently swap two same-typed dependencies past the compiler.
pub struct RoleServiceDeps<I, C> {
    pub grants: Arc<dyn RoleGrantStore>,
    pub orgs: Arc<dyn OrganizationRepository>,
    pub teams: Arc<dyn TeamRepository>,
    pub projects: Arc<dyn ProjectRepository>,
    pub authorize: Authorize,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}

impl<I, C> RoleService<I, C>
where
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: RoleServiceDeps<I, C>) -> Self {
        Self {
            grants: deps.grants,
            orgs: deps.orgs,
            teams: deps.teams,
            projects: deps.projects,
            authorize: deps.authorize,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Resolves `scope`'s tenancy node against the DB and confirms the caller-supplied PRN
    /// byte-matches its REAL, stored canonical PRN — `GrantScope::Root` has no forgeable slot
    /// and is a no-op. `NotFound` if the node doesn't exist; `PrnMismatch` if it does but the
    /// caller's org slot doesn't match the stored one (module docs).
    async fn resolve_scope(&self, scope: &GrantScope) -> Result<(), TenancyError> {
        let (claimed, stored) = match scope {
            GrantScope::Root => return Ok(()),
            GrantScope::Node(TenancyNodeRef::Organization(id)) => {
                let view = self.orgs.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?;
                (id.canonical(), view.node.id.canonical())
            }
            GrantScope::Node(TenancyNodeRef::Team(id)) => {
                let view = self.teams.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?;
                (id.canonical(), view.node.id.canonical())
            }
            GrantScope::Node(TenancyNodeRef::Project(id)) => {
                let view = self.projects.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?;
                (id.canonical(), view.node.id.canonical())
            }
        };
        if claimed != stored {
            return Err(TenancyError::PrnMismatch);
        }
        Ok(())
    }

    /// Grants `role_key` to `principal_prn` at `scope_prn`. Order of checks: (1) the
    /// principal PRN parses; (2) `role_key` names a known system role (else `UnknownRole`);
    /// (3) `scope_prn` parses into a `GrantScope`; (4) the scope's `NodeKind` is one the role
    /// allows (else `InvalidScope` — e.g. granting an `Organization`-scoped role at a
    /// `Team`); (5) **the anti-escalation check**: `actor` must itself be authorized for
    /// `Action::GrantRole` AT the scope (the scope node is the resource, not the target
    /// principal) — only someone who already has authority there may hand more of it out; the
    /// scope's Cedar identity is derived from its REAL tenancy ancestry regardless of a forged
    /// PRN (the entity-slice loader's own root-cause fix, SMA-444 cross-tenant-escalation
    /// fix), so a forged-org-slot escalation attempt is denied HERE, `Forbidden`; (6)
    /// [`RoleService::resolve_scope`] re-resolves the scope node against the DB and rejects a
    /// scope PRN that doesn't byte-match the node's real canonical form (else
    /// `NotFound`/`PrnMismatch` — defense-in-depth against persisting a grant whose stored
    /// scope misrepresents its real tenancy, module docs), run only once `actor` is already
    /// authorized so the anti-escalation check above is what a forged-but-unauthorized attempt
    /// actually trips. Only after all six succeed is the grant minted; it is then committed
    /// atomically with its `DomainEvent`/`AuditEntry` (module docs, the UoW reference
    /// pattern), and only once that commit succeeds does the awaited `gen_bumper.bump()` run.
    pub async fn grant(&self, actor: &Prn, principal_prn: &str, role_key: &str, scope_prn: &str) -> Result<RoleGrant, TenancyError> {
        let principal = parse_principal_prn(principal_prn)?;
        let role = authz_roles::role(role_key).ok_or_else(|| TenancyError::UnknownRole(role_key.to_string()))?;
        let scope = parse_grant_scope(scope_prn)?;
        if !role.scope_kinds.contains(&scope.kind()) {
            return Err(TenancyError::InvalidScope(scope_prn.to_string()));
        }

        self.authorize.check(actor, Action::GrantRole, &scope_resource_prn(&scope)).await?;
        self.resolve_scope(&scope).await?;

        let id = self.ids.new_membership_id();
        let now = self.clock.now();
        let grant = RoleGrant {
            id,
            principal,
            role_key: role.key,
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: now,
        };

        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::RoleGranted,
            schema_version: 1,
            aggregate_prn: grant.principal.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"grant_id": grant.id, "role_key": grant.role_key, "scope": grant.scope.canonical_prn()}),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "GrantRole".into(),
            resource_prn: Some(scope_resource_prn(&grant.scope).canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"role_key": grant.role_key, "scope": grant.scope.canonical_prn()}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        self.grants.grant_in(&*tx, &grant).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // Post-commit, awaited (module docs): guarantees the bump has happened by the time
        // `grant` returns (AC1), and can only ever run for a mutation that actually committed.
        self.gen_bumper.bump().await;
        Ok(grant)
    }

    /// Revokes a grant by id. `NotFound` if it doesn't exist (or was already revoked) at the
    /// initial lookup. Authorizes `actor` for `Action::RevokeRole` against the EXISTING
    /// grant's own scope — mirrors `grant`'s anti-escalation posture: revoking requires the
    /// same authority granting there would. The delete, its `DomainEvent`, and its
    /// `AuditEntry` then commit atomically (module docs). `grants.revoke_in` returning `false`
    /// (a benign TOCTOU race: the grant vanished between the lookup above and this txn — e.g.
    /// a concurrent revoke of the same id) is treated as an idempotent no-op: nothing is
    /// enqueued or recorded, and the post-commit bump never runs, since nothing was actually
    /// revoked by THIS call.
    pub async fn revoke(&self, actor: &Prn, id: Uuid) -> Result<(), TenancyError> {
        let grant = self.grants.find(id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::RevokeRole, &scope_resource_prn(&grant.scope)).await?;

        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::RoleRevoked,
            schema_version: 1,
            aggregate_prn: grant.principal.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"grant_id": grant.id, "role_key": grant.role_key, "scope": grant.scope.canonical_prn()}),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "RevokeRole".into(),
            resource_prn: Some(scope_resource_prn(&grant.scope).canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"role_key": grant.role_key, "scope": grant.scope.canonical_prn()}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        let existed = self.grants.revoke_in(&*tx, id).await?;
        if existed {
            self.outbox.enqueue(&*tx, &event).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;

        if existed {
            self.gen_bumper.bump().await;
        }
        Ok(())
    }

    /// Lists every grant held by `principal_prn`. Exposure rule (M3 simplification — a full
    /// per-scope visibility model is out of scope here): an actor may always list their OWN
    /// grants, no policy check needed; listing anyone ELSE's requires `Action::ListRoleGrants`
    /// authorized against `root_prn()` — under Cedar's `resource in ?resource` semantics only
    /// a `Root`-scoped grant (`platform_admin`) satisfies a `Root`-resource check, so this is
    /// effectively "self, or a platform admin."
    pub async fn list(&self, actor: &Prn, principal_prn: &str) -> Result<Vec<RoleGrant>, TenancyError> {
        let principal = parse_principal_prn(principal_prn)?;
        if actor.canonical() != principal.canonical() {
            self.authorize.check(actor, Action::ListRoleGrants, &root_prn()).await?;
        }
        Ok(self.grants.list_by_principal(&principal).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{
        FakeAuditLog, FakeAuthorizer, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork, FixedClock, InMemoryOrgs, InMemoryProjects, InMemoryRoleGrants, InMemoryTeams, SeqIds, TenancyStore,
    };
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{AuthzError, Organization, OrganizationId, Slug, Team, TeamId, Transaction};
    use uuid::Uuid;

    fn principal_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn org_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "organization", Uuid::from_u128(n)).unwrap()
    }

    /// Builds a `RoleService` over an EMPTY tenancy store: fine for every scenario that only
    /// ever reaches `resolve_scope` with `GrantScope::Root` (a no-op) or gets rejected before
    /// `resolve_scope` even runs (e.g. `grant_rejects_a_disallowed_scope_kind`'s scope-kind
    /// check, which — by construction — happens BEFORE `resolve_scope` in `grant`'s check
    /// order) — see `new_service_with_store` for scenarios that need real seeded nodes.
    fn new_service(fake: FakeAuthorizer) -> RoleService<SeqIds, FixedClock> {
        new_service_with_store(fake, TenancyStore::default())
    }

    /// Like `new_service`, but over a caller-supplied `TenancyStore` — for scenarios (SMA-444
    /// cross-tenant-escalation fix) that need `resolve_scope`'s DB lookup to see real seeded
    /// org/team/project rows. The SMA-446 Slice B ports (`uow`/`outbox`/`audit`/`gen_bumper`)
    /// are wired to fresh, unshared fakes — fine for every scenario here that doesn't itself
    /// assert on what got emitted (see `new_service_with_fakes` for those).
    fn new_service_with_store(fake: FakeAuthorizer, store: TenancyStore) -> RoleService<SeqIds, FixedClock> {
        new_service_with_fakes(fake, Arc::new(InMemoryRoleGrants::default()), store).svc
    }

    /// Bundles a `RoleService` together with the SMA-446 Slice B fakes it was built over, so
    /// a test can assert on exactly what `grant`/`revoke` emitted through them (the reference
    /// pattern's `outbox`/`audit`/`gen_bumper` — B5-B7 will copy this test shape too).
    struct ServiceWithFakes {
        svc: RoleService<SeqIds, FixedClock>,
        outbox: FakeOutbox,
        audit: FakeAuditLog,
        bumper: FakePolicyGenBumper,
    }

    /// Like `new_service_with_store`, but over a caller-supplied `grants` store (so a test can
    /// inject one that errors mid-txn) and returning the outbox/audit/gen-bumper fakes
    /// alongside the service for direct assertion.
    fn new_service_with_fakes(fake: FakeAuthorizer, grants: Arc<dyn RoleGrantStore>, store: TenancyStore) -> ServiceWithFakes {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let bumper = FakePolicyGenBumper::default();
        let svc = RoleService::new(RoleServiceDeps {
            grants,
            orgs: Arc::new(InMemoryOrgs(store.clone())),
            teams: Arc::new(InMemoryTeams(store.clone())),
            projects: Arc::new(InMemoryProjects(store)),
            authorize: Authorize::new(Arc::new(fake)),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        ServiceWithFakes { svc, outbox, audit, bumper }
    }

    /// A `RoleGrantStore` whose `grant_in` always fails — simulates a store error mid-txn
    /// (guard D2): `RoleService::grant` must roll back before ever touching the outbox/audit
    /// log, and its post-commit bump must never run for a mutation that never committed.
    #[derive(Default)]
    struct FailingGrantStore;

    #[async_trait]
    impl RoleGrantStore for FailingGrantStore {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }

        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }

        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }

        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }

        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }

        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }

        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Ok(None)
        }
    }

    /// A `RoleGrantStore` whose `find` reports a grant that has already vanished by the time
    /// `revoke_in` runs — simulates a benign TOCTOU race (e.g. a concurrent revoke of the
    /// same id winning first): `RoleService::revoke` must treat `revoke_in`'s `false` as an
    /// idempotent no-op, emitting nothing.
    struct VanishesBeforeRevoke {
        grant: RoleGrant,
    }

    #[async_trait]
    impl RoleGrantStore for VanishesBeforeRevoke {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises find/revoke_in")
        }

        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises find/revoke_in")
        }

        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises find/revoke_in")
        }

        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            // The grant `find` just reported is already gone by the time the txn runs.
            Ok(false)
        }

        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(vec![self.grant.clone()])
        }

        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(vec![self.grant.clone()])
        }

        async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Ok(if id == self.grant.id { Some(self.grant.clone()) } else { None })
        }
    }

    #[tokio::test]
    async fn grant_rejects_an_unknown_role() {
        let svc = new_service(FakeAuthorizer::default());
        let actor = principal_prn(1);
        let err = svc.grant(&actor, &principal_prn(2).canonical(), "no-such-role", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::UnknownRole("no-such-role".to_string()));
    }

    #[tokio::test]
    async fn grant_rejects_a_disallowed_scope_kind() {
        // `platform_admin` only allows `NodeKind::Root` — an organization scope must be
        // rejected even before the authorizer is ever consulted.
        let svc = new_service(FakeAuthorizer::default());
        let actor = principal_prn(1);
        let err = svc.grant(&actor, &principal_prn(2).canonical(), "platform_admin", &org_prn(100).canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::InvalidScope(org_prn(100).canonical()));
    }

    #[tokio::test]
    async fn grant_denies_an_unauthorized_actor() {
        // The authorizer never allows `GrantRole` at `Root` — the actor lacks the authority
        // to grant there, so the grant must be denied before ever touching the store.
        let svc = new_service(FakeAuthorizer::default());
        let actor = principal_prn(1);
        let err = svc.grant(&actor, &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn grant_succeeds_for_an_authorized_actor() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let svc = new_service(fake);
        let actor = principal_prn(1);
        let target = principal_prn(2);

        let grant = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();
        assert_eq!(grant.role_key, "platform_admin");
        assert_eq!(grant.scope, GrantScope::Root);
        assert_eq!(grant.linked_policy_id, format!("grant:{}", grant.id));

        let listed = svc.list(&target, &target.canonical()).await.unwrap();
        assert_eq!(listed, vec![grant]);
    }

    /// SMA-446 Slice B — the UoW reference pattern's core contract: `grant` enqueues exactly
    /// one `DomainEvent` and records exactly one `AuditEntry`, the two sharing ONE
    /// correlation id, and its post-commit `PolicyGenBumper::bump()` has already run — is
    /// AWAITED, not fire-and-forget — by the time `grant` returns (AC1).
    #[tokio::test]
    async fn grant_emits_one_event_and_one_audit_entry_sharing_a_correlation_id_and_awaits_the_bump() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(InMemoryRoleGrants::default()), TenancyStore::default());
        let actor = principal_prn(1);
        let target = principal_prn(2);

        let grant = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "grant must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::RoleGranted);
        assert_eq!(events[0].aggregate_prn, target.canonical());
        assert_eq!(events[0].actor_prn, Some(actor.canonical()));
        assert_eq!(events[0].payload["grant_id"], serde_json::json!(grant.id));

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "grant must record exactly one audit entry");
        assert_eq!(entries[0].action, "GrantRole");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);
        assert_eq!(entries[0].actor_prn, Some(actor.canonical()));

        assert!(events[0].correlation_id.is_some());
        assert_eq!(events[0].correlation_id, entries[0].correlation_id, "the event and the audit entry must share one correlation id");

        assert_eq!(bumper.calls(), 1, "the post-commit gen bump must have been awaited exactly once by the time grant returns");
    }

    /// Guard D2 (SMA-446 Slice B): a store error mid-txn must roll the whole unit of work
    /// back — `grant` must never enqueue an event, record an audit entry, or run its
    /// post-commit bump for a mutation that never actually committed.
    #[tokio::test]
    async fn a_store_error_mid_txn_rolls_back_and_never_emits_or_bumps_guard_d2() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(FailingGrantStore), TenancyStore::default());
        let actor = principal_prn(1);
        let target = principal_prn(2);

        let err = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal, "AuthzError::Backend from a mid-txn store failure maps to Internal");

        assert!(outbox.0.lock().unwrap().is_empty(), "a rolled-back grant must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "a rolled-back grant must not record an audit entry");
        assert_eq!(bumper.calls(), 0, "a rolled-back grant must never bump policy_gen (guard D2)");
    }

    /// SMA-444 cross-tenant-escalation fix (FIX 2, defense-in-depth): `resolve_scope` must
    /// reject a team-scope PRN whose org slot doesn't match the team's REAL stored org, even
    /// though `TenancyNodeRef::from_prn` happily parses it (it only checks an org slot is
    /// PRESENT, never that it's correct — `tenancy::check`) and even when the authorizer would
    /// otherwise ALLOW the grant. Without this check, `grant` would return `Forbidden` (the
    /// always-deny-by-default fake never having been told to allow anything) rather than
    /// `PrnMismatch` — so asserting `PrnMismatch` specifically here proves `resolve_scope` ran
    /// and rejected the forgery BEFORE the authorizer was ever consulted, not merely that SOME
    /// check happened to deny it. This test fails (`Forbidden`, not `PrnMismatch`) without FIX
    /// 2's `resolve_scope` call in `grant`.
    #[tokio::test]
    async fn grant_rejects_a_team_scope_with_a_forged_org_slot_even_when_the_authorizer_would_allow() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(0, 0).unwrap();
        let real_org = Uuid::from_u128(500);
        let wrong_org = Uuid::from_u128(501);
        let team_uuid = Uuid::from_u128(502);
        store
            .orgs
            .lock()
            .unwrap()
            .insert(real_org, Organization::new(OrganizationId::from_uuid(real_org), Slug::parse("acme").unwrap(), "Acme", now).unwrap());
        store
            .teams
            .lock()
            .unwrap()
            .insert(team_uuid, Team::new(TeamId::from_parts(real_org, team_uuid), Slug::parse("eng").unwrap(), "Eng", now).unwrap());

        // The forged scope PRN the caller submits: `team_uuid`'s REAL uuid, but `wrong_org`'s
        // uuid in the org slot (the team really lives under `real_org`).
        let forged_team = TeamId::from_parts(wrong_org, team_uuid);

        let fake = FakeAuthorizer::default();
        // Even an authorizer that WOULD allow `GrantRole` against the forged resource must
        // never be reached — `resolve_scope` rejects the forgery first.
        fake.allow(Action::GrantRole, forged_team.prn());

        let svc = new_service_with_store(fake, store);
        let actor = principal_prn(1);
        let target = principal_prn(2);

        let err = svc.grant(&actor, &target.canonical(), "team_admin", &forged_team.canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::PrnMismatch);

        // No grant was persisted for the forged scope.
        assert!(svc.list(&target, &target.canonical()).await.unwrap().is_empty());
    }

    /// `resolve_scope` must also reject a scope PRN naming a node that doesn't exist at all
    /// (as opposed to one that exists but under the wrong org) — `NotFound`, not a silent pass.
    #[tokio::test]
    async fn grant_rejects_a_scope_naming_a_nonexistent_team() {
        let fake = FakeAuthorizer::default();
        let team = TeamId::from_parts(Uuid::from_u128(600), Uuid::from_u128(601));
        fake.allow(Action::GrantRole, team.prn());

        let svc = new_service(fake);
        let actor = principal_prn(1);
        let target = principal_prn(2);
        let err = svc.grant(&actor, &target.canonical(), "team_admin", &team.canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
    }

    #[tokio::test]
    async fn revoke_missing_grant_is_not_found() {
        let svc = new_service(FakeAuthorizer::default());
        let actor = principal_prn(1);
        assert_eq!(svc.revoke(&actor, Uuid::from_u128(999)).await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn revoke_denies_an_unauthorized_actor_then_succeeds_once_authorized() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let svc = new_service(fake.clone());
        let actor = principal_prn(1);
        let target = principal_prn(2);
        let grant = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();

        // `RevokeRole` was never allowed — only `GrantRole` was — so revoke must deny.
        assert_eq!(svc.revoke(&actor, grant.id).await.unwrap_err(), TenancyError::Forbidden);

        fake.allow(Action::RevokeRole, &root_prn());
        svc.revoke(&actor, grant.id).await.unwrap();
        assert!(svc.list(&target, &target.canonical()).await.unwrap().is_empty());
    }

    /// SMA-446 Slice B: `revoke_in` returning `false` (the grant vanished between `find` and
    /// the txn — a benign TOCTOU race) is an idempotent no-op — `revoke` must succeed WITHOUT
    /// enqueuing an event, recording an audit entry, or running its post-commit bump, since
    /// nothing was actually revoked by this call.
    #[tokio::test]
    async fn revoke_of_a_grant_that_vanished_before_revoke_in_is_an_idempotent_no_op() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::RevokeRole, &root_prn());
        let grant = RoleGrant {
            id: Uuid::from_u128(42),
            principal: PrincipalId::from_prn(principal_prn(2)),
            role_key: "platform_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: "grant:42".to_string(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        };
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(VanishesBeforeRevoke { grant: grant.clone() }), TenancyStore::default());
        let actor = principal_prn(1);

        svc.revoke(&actor, grant.id).await.unwrap();

        assert!(outbox.0.lock().unwrap().is_empty(), "an idempotent no-op revoke must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "an idempotent no-op revoke must not record an audit entry");
        assert_eq!(bumper.calls(), 0, "an idempotent no-op revoke must never bump policy_gen");
    }

    #[tokio::test]
    async fn list_allows_self_without_authorization_but_denies_listing_another_principal() {
        let svc = new_service(FakeAuthorizer::default());
        let actor = principal_prn(1);

        // Self-listing never consults the (always-deny-by-default) fake authorizer.
        assert!(svc.list(&actor, &actor.canonical()).await.unwrap().is_empty());

        // Listing someone else's grants requires platform-level ListRoleGrants at Root.
        let other = principal_prn(2);
        assert_eq!(svc.list(&actor, &other.canonical()).await.unwrap_err(), TenancyError::Forbidden);
    }
}
