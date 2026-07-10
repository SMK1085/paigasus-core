// SPDX-License-Identifier: Apache-2.0

//! Shared in-memory fakes for application-service tests (`#[cfg(test)]`-only, never
//! shipped). `TenancyStore` holds the tenancy state behind `Arc<Mutex<HashMap>>`s so the
//! per-port fakes — `InMemoryOrgs`, `InMemoryTeams`, `InMemoryProjects`, `InMemoryMemberships`
//! — can each clone a handle onto the *same* backing data: a team fake needs to see an org
//! archived via the org fake to compute effective status (D10), and `InMemoryOrgs::create`
//! populates the shared team map with the auto-provisioned default team (ADR-0014).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{
    AccessRequest, Action, Authorizer, AuthzError, Clock, ConflictKind, Decision, Effect, IdGenerator, Membership, MembershipRecord, MembershipRepository, NodeStatus, NodeView, Organization,
    OrganizationId, OrganizationRepository, PolicyDocument, PolicyStore, PreconditionKind, PrincipalId, Project, ProjectId, ProjectRepository, RepositoryError, RoleGrant, RoleGrantStore, Slug, Team,
    TeamId, TeamRepository, TenancyNodeRef,
};
use paigasus_kernel::Prn;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Shared backing store for all tenancy in-memory fakes. Cloning is cheap (shares the
/// `Arc` innards), so e.g. an `InMemoryTeams` fake sees the same org rows an
/// `InMemoryOrgs` fake mutates. `principals` is a minimal stand-in for the principal
/// repository (Task 6's own fake lives in `create_user.rs`, unrelated store): just the
/// uuid -> canonical-prn record `InMemoryMemberships` checks caller prns against.
#[derive(Clone, Default)]
pub struct TenancyStore {
    pub orgs: Arc<Mutex<HashMap<Uuid, Organization>>>,
    pub teams: Arc<Mutex<HashMap<Uuid, Team>>>,
    pub projects: Arc<Mutex<HashMap<Uuid, Project>>>,
    pub memberships: Arc<Mutex<HashMap<Uuid, Membership>>>,
    pub principals: Arc<Mutex<HashMap<Uuid, String>>>,
}

/// In-memory `OrganizationRepository` fake, faithful to the port's doc contracts:
/// duplicate slug (globally, across all orgs) -> `Conflict(SlugTaken)`; missing id ->
/// `NotFound`; rename targeting an own-archived org -> `Precondition(NodeArchived)`;
/// `set_status` is idempotent (a no-op leaves `updated_at` untouched).
#[derive(Clone, Default)]
pub struct InMemoryOrgs(pub TenancyStore);

/// Orgs have no ancestors, but effective status is still computed via the shared rule
/// (D1/D10) rather than hand-rolled as "effective == own".
fn org_view(org: &Organization) -> NodeView<Organization> {
    NodeView {
        node: org.clone(),
        effective_status: NodeStatus::effective(org.status, &[]),
    }
}

#[async_trait]
impl OrganizationRepository for InMemoryOrgs {
    async fn create(&self, org: &Organization, default_team: &Team) -> Result<(), RepositoryError> {
        let mut orgs = self.0.orgs.lock().unwrap();
        if orgs.values().any(|existing| existing.slug == org.slug) {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }
        orgs.insert(org.id.uuid(), org.clone());
        drop(orgs);
        self.0.teams.lock().unwrap().insert(default_team.id.uuid(), default_team.clone());
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Organization>>, RepositoryError> {
        Ok(self.0.orgs.lock().unwrap().get(&id).map(org_view))
    }

    async fn list(&self, limit: u64, offset: u64) -> Result<Vec<NodeView<Organization>>, RepositoryError> {
        let orgs = self.0.orgs.lock().unwrap();
        let mut items: Vec<&Organization> = orgs.values().collect();
        items.sort_by_key(|o| (o.created_at, o.id.uuid()));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).map(org_view).collect())
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError> {
        let mut orgs = self.0.orgs.lock().unwrap();

        let current_status = orgs.get(&id).map(|o| o.status).ok_or(RepositoryError::NotFound)?;
        if current_status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }
        if let Some(slug) = new_slug
            && orgs.values().any(|o| o.id.uuid() != id && &o.slug == slug)
        {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }

        let org = orgs.get_mut(&id).expect("existence checked above");
        if let Some(slug) = new_slug {
            org.slug = slug.clone();
        }
        if let Some(name) = new_name {
            org.name = name.to_owned();
        }
        org.updated_at = now;
        Ok(org_view(org))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError> {
        let mut orgs = self.0.orgs.lock().unwrap();
        let org = orgs.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        if org.status != status {
            org.status = status;
            org.updated_at = now;
        }
        Ok(org_view(org))
    }
}

/// Looks up an org's own status (orgs have no ancestors of their own).
fn org_status(store: &TenancyStore, org: Uuid) -> Option<NodeStatus> {
    store.orgs.lock().unwrap().get(&org).map(|o| o.status)
}

/// In-memory `TeamRepository` fake, faithful to the port's doc contracts: creating under a
/// missing org -> `NotFound`; under an effectively-archived org -> `Precondition
/// (ParentArchived)`; duplicate slug scoped to the org -> `Conflict(SlugTaken)`; rename
/// targeting an EFFECTIVELY archived team (own status or ancestor org) -> `Precondition
/// (NodeArchived)`; `set_status` is idempotent (own status only — D10, "always permitted").
#[derive(Clone, Default)]
pub struct InMemoryTeams(pub TenancyStore);

fn team_view(store: &TenancyStore, team: &Team) -> NodeView<Team> {
    let ancestors: Vec<NodeStatus> = org_status(store, team.id.org_uuid()).into_iter().collect();
    NodeView {
        node: team.clone(),
        effective_status: NodeStatus::effective(team.status, &ancestors),
    }
}

#[async_trait]
impl TeamRepository for InMemoryTeams {
    async fn create(&self, team: &Team) -> Result<(), RepositoryError> {
        let org = team.id.org_uuid();
        let status = org_status(&self.0, org).ok_or(RepositoryError::NotFound)?;
        if status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::ParentArchived));
        }

        let mut teams = self.0.teams.lock().unwrap();
        if teams.values().any(|t| t.id.org_uuid() == org && t.slug == team.slug) {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }
        teams.insert(team.id.uuid(), team.clone());
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Team>>, RepositoryError> {
        Ok(self.0.teams.lock().unwrap().get(&id).map(|t| team_view(&self.0, t)))
    }

    async fn list_by_org(&self, org: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Team>>, RepositoryError> {
        let teams = self.0.teams.lock().unwrap();
        let mut items: Vec<&Team> = teams.values().filter(|t| t.id.org_uuid() == org).collect();
        items.sort_by_key(|t| (t.created_at, t.id.uuid()));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).map(|t| team_view(&self.0, t)).collect())
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError> {
        let mut teams = self.0.teams.lock().unwrap();

        let current = teams.get(&id).cloned().ok_or(RepositoryError::NotFound)?;
        if team_view(&self.0, &current).effective_status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }
        if let Some(slug) = new_slug
            && teams.values().any(|t| t.id.uuid() != id && t.id.org_uuid() == current.id.org_uuid() && &t.slug == slug)
        {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }

        let team = teams.get_mut(&id).expect("existence checked above");
        if let Some(slug) = new_slug {
            team.slug = slug.clone();
        }
        if let Some(name) = new_name {
            team.name = name.to_owned();
        }
        team.updated_at = now;
        Ok(team_view(&self.0, team))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError> {
        let mut teams = self.0.teams.lock().unwrap();
        let team = teams.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        if team.status != status {
            team.status = status;
            team.updated_at = now;
        }
        Ok(team_view(&self.0, team))
    }
}

/// In-memory `ProjectRepository` fake, mirroring `InMemoryTeams` one level deeper: creating
/// under a missing team -> `NotFound`; under an EFFECTIVELY archived team (own status or
/// ancestor org) -> `Precondition(ParentArchived)`; duplicate slug scoped to the team ->
/// `Conflict(SlugTaken)`; rename targeting an effectively archived project (own, team, or
/// org) -> `Precondition(NodeArchived)`; `set_status` is idempotent.
#[derive(Clone, Default)]
pub struct InMemoryProjects(pub TenancyStore);

/// Ancestor statuses for a project: its team's own status, then the team's org's own
/// status (D10 folds any own-archived flag anywhere up the chain via `NodeStatus::effective`).
fn project_ancestors(store: &TenancyStore, project: &Project) -> Vec<NodeStatus> {
    let Some(team) = store.teams.lock().unwrap().get(&project.team_id.uuid()).cloned() else {
        return Vec::new();
    };
    let mut ancestors = vec![team.status];
    ancestors.extend(org_status(store, team.id.org_uuid()));
    ancestors
}

fn project_view(store: &TenancyStore, project: &Project) -> NodeView<Project> {
    NodeView {
        node: project.clone(),
        effective_status: NodeStatus::effective(project.status, &project_ancestors(store, project)),
    }
}

#[async_trait]
impl ProjectRepository for InMemoryProjects {
    async fn create(&self, project: &Project) -> Result<(), RepositoryError> {
        let team_uuid = project.team_id.uuid();
        let team = self.0.teams.lock().unwrap().get(&team_uuid).cloned().ok_or(RepositoryError::NotFound)?;
        if team_view(&self.0, &team).effective_status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::ParentArchived));
        }
        if project.id.org_uuid() != team.id.org_uuid() {
            return Err(RepositoryError::Backend(Box::<dyn std::error::Error + Send + Sync>::from("project org does not match team org")));
        }

        let mut projects = self.0.projects.lock().unwrap();
        if projects.values().any(|p| p.team_id.uuid() == team_uuid && p.slug == project.slug) {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }
        projects.insert(project.id.uuid(), project.clone());
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Project>>, RepositoryError> {
        Ok(self.0.projects.lock().unwrap().get(&id).map(|p| project_view(&self.0, p)))
    }

    async fn list_by_team(&self, team: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Project>>, RepositoryError> {
        let projects = self.0.projects.lock().unwrap();
        let mut items: Vec<&Project> = projects.values().filter(|p| p.team_id.uuid() == team).collect();
        items.sort_by_key(|p| (p.created_at, p.id.uuid()));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).map(|p| project_view(&self.0, p)).collect())
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError> {
        let mut projects = self.0.projects.lock().unwrap();

        let current = projects.get(&id).cloned().ok_or(RepositoryError::NotFound)?;
        if project_view(&self.0, &current).effective_status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }
        if let Some(slug) = new_slug
            && projects.values().any(|p| p.id.uuid() != id && p.team_id.uuid() == current.team_id.uuid() && &p.slug == slug)
        {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }

        let project = projects.get_mut(&id).expect("existence checked above");
        if let Some(slug) = new_slug {
            project.slug = slug.clone();
        }
        if let Some(name) = new_name {
            project.name = name.to_owned();
        }
        project.updated_at = now;
        Ok(project_view(&self.0, project))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError> {
        let mut projects = self.0.projects.lock().unwrap();
        let project = projects.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        if project.status != status {
            project.status = status;
            project.updated_at = now;
        }
        Ok(project_view(&self.0, project))
    }
}

/// Builds a `MembershipRecord` from a stored `Membership`. Safe to read canonicals
/// straight off the entity: by the time a row is in the store, `InMemoryMemberships::attach`
/// has already checked them against the store's stated-canonical prns.
fn to_record(m: &Membership) -> MembershipRecord {
    MembershipRecord {
        id: m.id,
        principal_prn: m.principal_id.canonical(),
        node_prn: m.node.canonical(),
        created_at: m.created_at,
    }
}

/// For team/project nodes, the org uuid that must already have a membership row before a
/// team/project attach is allowed (D8's org-membership invariant). Organization nodes have
/// no such parent scope, hence `None`.
fn parent_org_uuid(node: &TenancyNodeRef) -> Option<Uuid> {
    match node {
        TenancyNodeRef::Organization(_) => None,
        TenancyNodeRef::Team(id) => Some(id.org_uuid()),
        TenancyNodeRef::Project(id) => Some(id.org_uuid()),
    }
}

/// Resolves a node ref against the store: `None` if the node doesn't exist, else its
/// stored canonical prn (to detect a forged/stale caller prn), own status, and ancestor
/// statuses (D10's `NodeStatus::effective`).
fn node_lookup(store: &TenancyStore, node: &TenancyNodeRef) -> Option<(String, NodeStatus, Vec<NodeStatus>)> {
    match node {
        TenancyNodeRef::Organization(id) => {
            let org = store.orgs.lock().unwrap().get(&id.uuid())?.clone();
            Some((org.id.canonical(), org.status, Vec::new()))
        }
        TenancyNodeRef::Team(id) => {
            let team = store.teams.lock().unwrap().get(&id.uuid())?.clone();
            let ancestors = org_status(store, team.id.org_uuid()).into_iter().collect();
            Some((team.id.canonical(), team.status, ancestors))
        }
        TenancyNodeRef::Project(id) => {
            let project = store.projects.lock().unwrap().get(&id.uuid())?.clone();
            Some((project.id.canonical(), project.status, project_ancestors(store, &project)))
        }
    }
}

/// In-memory `MembershipRepository` fake, faithful to the port's doc contract (exact guard
/// order): principal exists in the store's `principals` map + stored-canonical compare ->
/// `NotFound`/`PrnMismatch`; node exists + stored-canonical compare -> `NotFound`/
/// `PrnMismatch`; node effectively active (D1/D10) -> `Precondition(NodeArchived)`;
/// team/project targets require an existing org membership -> `Precondition
/// (MissingOrgMembership)`; duplicate (same principal + same node) ->
/// `Conflict(DuplicateMembership)`. `detach` cascades: removing an org membership also
/// removes that principal's team/project memberships scoped to that org (rule 5).
#[derive(Clone, Default)]
pub struct InMemoryMemberships(pub TenancyStore);

#[async_trait]
impl MembershipRepository for InMemoryMemberships {
    async fn attach(&self, membership: &Membership) -> Result<MembershipRecord, RepositoryError> {
        let store = &self.0;
        let principal_uuid = membership.principal_id.uuid();

        // 1. Principal must exist, and the caller's prn must byte-match the stored one.
        let stored_principal_prn = { store.principals.lock().unwrap().get(&principal_uuid).cloned() }.ok_or(RepositoryError::NotFound)?;
        if stored_principal_prn != membership.principal_id.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }

        // 2. Node must exist, and the caller's prn must byte-match the stored one.
        let (node_canonical, own_status, ancestors) = node_lookup(store, &membership.node).ok_or(RepositoryError::NotFound)?;
        if node_canonical != membership.node.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }

        // 3. The node must be effectively active.
        if NodeStatus::effective(own_status, &ancestors) == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }

        // 4. Team/project targets require an existing org membership.
        if let Some(org_uuid) = parent_org_uuid(&membership.node) {
            let expected_org_node = TenancyNodeRef::Organization(OrganizationId::from_uuid(org_uuid));
            let has_org_membership = store
                .memberships
                .lock()
                .unwrap()
                .values()
                .any(|m| m.principal_id.uuid() == principal_uuid && m.node == expected_org_node);
            if !has_org_membership {
                return Err(RepositoryError::Precondition(PreconditionKind::MissingOrgMembership));
            }
        }

        // 5. No duplicate (same principal, same node).
        let mut memberships = store.memberships.lock().unwrap();
        let duplicate = memberships.values().any(|m| m.principal_id.uuid() == principal_uuid && m.node == membership.node);
        if duplicate {
            return Err(RepositoryError::Conflict(ConflictKind::DuplicateMembership));
        }

        memberships.insert(membership.id, membership.clone());
        Ok(MembershipRecord {
            id: membership.id,
            principal_prn: stored_principal_prn,
            node_prn: node_canonical,
            created_at: membership.created_at,
        })
    }

    async fn find(&self, id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError> {
        Ok(self.0.memberships.lock().unwrap().get(&id).map(to_record))
    }

    async fn detach(&self, id: Uuid) -> Result<(), RepositoryError> {
        let mut memberships = self.0.memberships.lock().unwrap();
        let membership = memberships.get(&id).cloned().ok_or(RepositoryError::NotFound)?;
        memberships.remove(&id);

        if let TenancyNodeRef::Organization(org_id) = &membership.node {
            let org_uuid = org_id.uuid();
            let principal_uuid = membership.principal_id.uuid();
            memberships.retain(|_, m| !(m.principal_id.uuid() == principal_uuid && parent_org_uuid(&m.node) == Some(org_uuid)));
        }
        Ok(())
    }

    async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let memberships = self.0.memberships.lock().unwrap();
        let mut items: Vec<&Membership> = memberships.values().filter(|m| m.principal_id.uuid() == principal).collect();
        items.sort_by_key(|m| (m.created_at, m.id));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).map(to_record).collect())
    }

    async fn list_by_node(&self, node: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let (node_canonical, _own_status, _ancestors) = node_lookup(&self.0, node).ok_or(RepositoryError::NotFound)?;
        if node_canonical != node.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }

        let memberships = self.0.memberships.lock().unwrap();
        let mut items: Vec<&Membership> = memberships.values().filter(|m| m.node == *node).collect();
        items.sort_by_key(|m| (m.created_at, m.id));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).map(to_record).collect())
    }
}

/// Settable fake clock: `FixedClock::default()` starts at the Unix epoch; `set` drives it
/// forward so tests can assert `updated_at` semantics deterministically.
#[derive(Clone, Default)]
pub struct FixedClock(Arc<Mutex<DateTime<Utc>>>);

impl FixedClock {
    pub fn set(&self, t: DateTime<Utc>) {
        *self.0.lock().unwrap() = t;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Deterministic id generator: mints sequential `Uuid::from_u128(n)` values through the
/// typed-id constructors, so tests get stable, human-readable ids without pulling in
/// UUIDv7/entropy.
#[derive(Default)]
pub struct SeqIds(AtomicU64);

impl SeqIds {
    fn next(&self) -> Uuid {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed)))
    }
}

impl IdGenerator for SeqIds {
    fn new_principal_id(&self) -> PrincipalId {
        let prn = Prn::build("iam", "", None, "principal", self.next()).expect("valid principal prn");
        PrincipalId::from_prn(prn)
    }

    fn new_organization_id(&self) -> OrganizationId {
        OrganizationId::from_uuid(self.next())
    }

    fn new_team_id(&self, org: Uuid) -> TeamId {
        TeamId::from_parts(org, self.next())
    }

    fn new_project_id(&self, org: Uuid) -> ProjectId {
        ProjectId::from_parts(org, self.next())
    }

    fn new_membership_id(&self) -> Uuid {
        self.next()
    }

    fn new_external_identity_id(&self) -> Uuid {
        self.next()
    }
}

/// Programmable `Authorizer` fake for application-service unit tests (`authorize.rs`,
/// `roles.rs`, `policies.rs`): `allow(action, resource)` whitelists an exact
/// `(Action, resource-canonical-prn)` pair — every other request denies, mirroring Cedar's
/// own default-deny posture. Keyed on the resource's canonical prn string rather than `Prn`
/// itself (no `Hash`/`Eq` on `Prn`, and canonical-string equality is exactly what this authz
/// layer's identity comparison reduces to).
#[derive(Clone, Default)]
pub struct FakeAuthorizer {
    allowed: Arc<Mutex<HashSet<(Action, String)>>>,
}

impl FakeAuthorizer {
    pub fn allow(&self, action: Action, resource: &Prn) {
        self.allowed.lock().unwrap().insert((action, resource.canonical()));
    }
}

#[async_trait]
impl Authorizer for FakeAuthorizer {
    async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError> {
        let allow = self.allowed.lock().unwrap().contains(&(req.action, req.resource.canonical()));
        Ok(Decision {
            effect: if allow { Effect::Allow } else { Effect::Deny },
            determining_policies: Vec::new(),
        })
    }
}

/// In-memory `RoleGrantStore` fake for `roles.rs` unit tests: a plain
/// `Mutex<HashMap<Uuid, RoleGrant>>`, no generation-counter bookkeeping (that's
/// `PgRoleGrantStore`'s job, exercised by the Docker integration tests).
#[derive(Clone, Default)]
pub struct InMemoryRoleGrants(pub Arc<Mutex<HashMap<Uuid, RoleGrant>>>);

#[async_trait]
impl RoleGrantStore for InMemoryRoleGrants {
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError> {
        self.0.lock().unwrap().insert(g.id, g.clone());
        Ok(())
    }

    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError> {
        self.0.lock().unwrap().remove(&id);
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }

    async fn list_by_principal(&self, p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
        Ok(self.0.lock().unwrap().values().filter(|g| g.principal == *p).cloned().collect())
    }

    async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
        Ok(self.0.lock().unwrap().get(&id).cloned())
    }
}

/// In-memory `PolicyStore` fake for `policies.rs` unit tests: rejects mutation of an
/// already-persisted `system = true` row, mirroring `PgPolicyStore`'s posture, without any
/// Cedar parse/schema validation (that's `authz::schema::validate_policy`'s own unit suite).
#[derive(Clone, Default)]
pub struct InMemoryPolicies {
    docs: Arc<Mutex<HashMap<String, PolicyDocument>>>,
    gen_counter: Arc<AtomicU64>,
}

#[async_trait]
impl PolicyStore for InMemoryPolicies {
    async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
        Ok(self.docs.lock().unwrap().values().cloned().collect())
    }

    async fn put(&self, doc: &PolicyDocument) -> Result<(), AuthzError> {
        let mut docs = self.docs.lock().unwrap();
        if docs.get(&doc.policy_id).is_some_and(|existing| existing.system) {
            return Err(AuthzError::SystemImmutable(doc.policy_id.clone()));
        }
        docs.insert(doc.policy_id.clone(), doc.clone());
        drop(docs);
        self.gen_counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn delete(&self, policy_id: &str) -> Result<(), AuthzError> {
        let mut docs = self.docs.lock().unwrap();
        if docs.get(policy_id).is_some_and(|existing| existing.system) {
            return Err(AuthzError::SystemImmutable(policy_id.to_string()));
        }
        docs.remove(policy_id);
        drop(docs);
        self.gen_counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn policy_gen(&self) -> Result<u64, AuthzError> {
        Ok(self.gen_counter.load(Ordering::SeqCst))
    }

    async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        Ok(self.gen_counter.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn org(uuid: Uuid, slug: &str, now: DateTime<Utc>) -> Organization {
        Organization::new(OrganizationId::from_uuid(uuid), Slug::parse(slug).unwrap(), "Name", now).unwrap()
    }

    #[tokio::test]
    async fn create_populates_the_shared_team_map() {
        let store = TenancyStore::default();
        let repo = InMemoryOrgs(store.clone());
        let now = Utc.timestamp_opt(0, 0).unwrap();
        let organization = org(Uuid::from_u128(1), "acme", now);
        let team = Team::new(TeamId::from_parts(organization.id.uuid(), Uuid::from_u128(2)), Slug::parse("default").unwrap(), "Default", now).unwrap();

        repo.create(&organization, &team).await.unwrap();

        assert!(store.teams.lock().unwrap().contains_key(&team.id.uuid()));
    }

    /// `ProjectService::create` early-exits on an effectively-archived team before ever
    /// calling the repo (see `application::projects` tests), so that path never exercises
    /// `InMemoryProjects::create`'s own guard. Exercise the repo directly to prove it is a
    /// real, independent guard (D8's "in-txn, re-guards" — belt-and-braces), not dead code.
    #[tokio::test]
    async fn project_create_repo_guard_rejects_missing_or_archived_team() {
        let store = TenancyStore::default();
        let repo = InMemoryProjects(store.clone());
        let now = Utc.timestamp_opt(0, 0).unwrap();

        let org_id = Uuid::from_u128(1);
        let team_id = Uuid::from_u128(2);
        let missing_project = Project::new(
            ProjectId::from_parts(org_id, Uuid::from_u128(3)),
            TeamId::from_parts(org_id, team_id),
            Slug::parse("web").unwrap(),
            "Web",
            now,
        )
        .unwrap();
        assert!(matches!(repo.create(&missing_project).await.unwrap_err(), RepositoryError::NotFound));

        let team = Team::new(TeamId::from_parts(org_id, team_id), Slug::parse("eng").unwrap(), "Eng", now).unwrap();
        store.teams.lock().unwrap().insert(team_id, team);
        InMemoryTeams(store.clone()).set_status(team_id, NodeStatus::Archived, now).await.unwrap();

        let project = Project::new(
            ProjectId::from_parts(org_id, Uuid::from_u128(4)),
            TeamId::from_parts(org_id, team_id),
            Slug::parse("web").unwrap(),
            "Web",
            now,
        )
        .unwrap();
        assert!(matches!(repo.create(&project).await.unwrap_err(), RepositoryError::Precondition(PreconditionKind::ParentArchived)));
    }
}
