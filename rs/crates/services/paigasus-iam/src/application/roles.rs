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

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::roles as authz_roles;
use paigasus_iam_core::{Action, Clock, GrantScope, IdGenerator, OrganizationRepository, PrincipalId, ProjectRepository, RoleGrant, RoleGrantStore, TeamRepository, TenancyNodeRef};
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
/// `grants`. `ids`/`clock` stay generic-DI, mirroring `MembershipService`.
#[derive(Clone)]
pub struct RoleService<I, C> {
    grants: Arc<dyn RoleGrantStore>,
    orgs: Arc<dyn OrganizationRepository>,
    teams: Arc<dyn TeamRepository>,
    projects: Arc<dyn ProjectRepository>,
    authorize: Authorize,
    ids: I,
    clock: C,
}

impl<I, C> RoleService<I, C>
where
    I: IdGenerator,
    C: Clock,
{
    pub fn new(
        grants: Arc<dyn RoleGrantStore>,
        orgs: Arc<dyn OrganizationRepository>,
        teams: Arc<dyn TeamRepository>,
        projects: Arc<dyn ProjectRepository>,
        authorize: Authorize,
        ids: I,
        clock: C,
    ) -> Self {
        Self {
            grants,
            orgs,
            teams,
            projects,
            authorize,
            ids,
            clock,
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
    /// actually trips. Only after all six succeed is the grant minted and persisted.
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
        self.grants.grant(&grant).await?;
        Ok(grant)
    }

    /// Revokes a grant by id. `NotFound` if it doesn't exist (or was already revoked).
    /// Authorizes `actor` for `Action::RevokeRole` against the EXISTING grant's own scope —
    /// mirrors `grant`'s anti-escalation posture: revoking requires the same authority
    /// granting there would.
    pub async fn revoke(&self, actor: &Prn, id: Uuid) -> Result<(), TenancyError> {
        let grant = self.grants.find(id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::RevokeRole, &scope_resource_prn(&grant.scope)).await?;
        Ok(self.grants.revoke(id).await?)
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
    use crate::application::fakes::{FakeAuthorizer, FixedClock, InMemoryOrgs, InMemoryProjects, InMemoryRoleGrants, InMemoryTeams, SeqIds, TenancyStore};
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{Organization, OrganizationId, Slug, Team, TeamId};
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
    /// org/team/project rows.
    fn new_service_with_store(fake: FakeAuthorizer, store: TenancyStore) -> RoleService<SeqIds, FixedClock> {
        RoleService::new(
            Arc::new(InMemoryRoleGrants::default()),
            Arc::new(InMemoryOrgs(store.clone())),
            Arc::new(InMemoryTeams(store.clone())),
            Arc::new(InMemoryProjects(store)),
            Authorize::new(Arc::new(fake)),
            SeqIds::default(),
            FixedClock::default(),
        )
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
