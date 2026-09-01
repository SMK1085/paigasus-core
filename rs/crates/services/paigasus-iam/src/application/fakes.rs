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
    AccessRequest, Action, ApiKey, ApiKeyId, ApiKeyRepository, ApiKeyStatus, AuditEntry, AuditFilter, AuditLog, Authorizer, AuthzError, BulkReplayRequest, Clock, ConflictKind, DeadLetterEntry,
    DeadLetterFilter, DeadLetters, Decision, DomainEvent, Effect, IdGenerator, KeyEntropy, Membership, MembershipRecord, MembershipRepository, Mutated, NodeStatus, NodeView, Organization,
    OrganizationId, OrganizationRepository, Outbox, PolicyDocument, PolicyGenBumper, PolicyStore, PreconditionKind, Principal, PrincipalId, PrincipalStatus, Project, ProjectId, ProjectRepository,
    PutOutcome, RepositoryError, RoleGrant, RoleGrantStore, Savepoint, SecretHasher, ServiceAccount, ServiceAccountRecord, ServiceAccountRepository, Slug, Stamp, Team, TeamId, TeamRepository,
    TenancyNodeRef, Transaction, UnitOfWork,
};
use paigasus_kernel::Prn;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Builds a deterministic `Stamp` for tests: `at` from the clock, `by` from a u128 seed.
#[must_use]
pub fn test_stamp(at: DateTime<Utc>, actor: u128) -> Stamp {
    Stamp::new(at, PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(actor)).unwrap()))
}

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
    /// `org_admin` owner grants seeded by [`InMemoryOrgs::create`] (SMA-444 Task 20b, spec
    /// D8) — a separate map from `MembershipRepository`'s own fakes above, mirroring the
    /// real schema's `role_grant` table being wholly separate from `membership`.
    pub role_grants: Arc<Mutex<HashMap<Uuid, RoleGrant>>>,
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
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        self.create_in(&*tx, org, default_team, owner_grant, stamp).await
    }

    async fn create_in(&self, _tx: &dyn Transaction, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, _stamp: &Stamp) -> Result<(), RepositoryError> {
        let mut orgs = self.0.orgs.lock().unwrap();
        if orgs.values().any(|existing| existing.slug == org.slug) {
            return Err(RepositoryError::Conflict(ConflictKind::SlugTaken));
        }
        orgs.insert(org.id.uuid(), org.clone());
        drop(orgs);
        self.0.teams.lock().unwrap().insert(default_team.id.uuid(), default_team.clone());
        self.0.role_grants.lock().unwrap().insert(owner_grant.id, owner_grant.clone());
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

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.rename_in(&*tx, id, new_slug, new_name, stamp).await?;
        Ok(out.value)
    }

    async fn rename_in(&self, _tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError> {
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

        // SMA-440 D5: every SUPPLIED field already equal to the stored one means this write
        // changes nothing, so it stamps nothing. The mandated order (see pg_organizations.rs's
        // `rename`) is archived -> no-op -> conflict; this no-op check instead runs AFTER the
        // conflict guard above, not before it. That is equivalent only because the conflict scan
        // just above excludes self by id, so a rename that resubmits the org's own current slug
        // can never read as a conflict with itself. If that self-exclusion is ever removed, this
        // fake would start rejecting a true no-op instead of silently matching production.
        let slug_same = new_slug.is_none_or(|s| &org.slug == s);
        let name_same = new_name.is_none_or(|n| org.name == n);
        if slug_same && name_same {
            return Ok(Mutated { value: org_view(org), changed: false });
        }

        if let Some(slug) = new_slug {
            org.slug = slug.clone();
        }
        if let Some(name) = new_name {
            org.name = name.to_owned();
        }
        org.updated_at = stamp.at;
        org.modified_by = Some(stamp.by.clone());
        Ok(Mutated { value: org_view(org), changed: true })
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.set_status_in(&*tx, id, status, stamp).await?;
        Ok(out.value)
    }

    async fn set_status_in(&self, _tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError> {
        let mut orgs = self.0.orgs.lock().unwrap();
        let org = orgs.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        let changed = org.status != status;
        if changed {
            org.status = status;
            org.updated_at = stamp.at;
            org.modified_by = Some(stamp.by.clone());
        }
        Ok(Mutated { value: org_view(org), changed })
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
    async fn create(&self, team: &Team, stamp: &Stamp) -> Result<(), RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        self.create_in(&*tx, team, stamp).await
    }

    async fn create_in(&self, _tx: &dyn Transaction, team: &Team, _stamp: &Stamp) -> Result<(), RepositoryError> {
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

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Team>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.rename_in(&*tx, id, new_slug, new_name, stamp).await?;
        Ok(out.value)
    }

    async fn rename_in(&self, _tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Team>>, RepositoryError> {
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

        // SMA-440 D5: every SUPPLIED field already equal to the stored one means this write
        // changes nothing, so it stamps nothing. The mandated order (see pg_teams.rs's
        // `rename`) is archived -> no-op -> conflict; this no-op check instead runs AFTER the
        // conflict guard above, not before it. That is equivalent only because the conflict scan
        // just above excludes self by id, so a rename that resubmits the team's own current slug
        // can never read as a conflict with itself. If that self-exclusion is ever removed, this
        // fake would start rejecting a true no-op instead of silently matching production.
        let slug_same = new_slug.is_none_or(|s| &team.slug == s);
        let name_same = new_name.is_none_or(|n| team.name == n);
        if slug_same && name_same {
            return Ok(Mutated {
                value: team_view(&self.0, team),
                changed: false,
            });
        }

        if let Some(slug) = new_slug {
            team.slug = slug.clone();
        }
        if let Some(name) = new_name {
            team.name = name.to_owned();
        }
        team.updated_at = stamp.at;
        team.modified_by = Some(stamp.by.clone());
        Ok(Mutated {
            value: team_view(&self.0, team),
            changed: true,
        })
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<NodeView<Team>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.set_status_in(&*tx, id, status, stamp).await?;
        Ok(out.value)
    }

    async fn set_status_in(&self, _tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Team>>, RepositoryError> {
        let mut teams = self.0.teams.lock().unwrap();
        let team = teams.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        let changed = team.status != status;
        if changed {
            team.status = status;
            team.updated_at = stamp.at;
            team.modified_by = Some(stamp.by.clone());
        }
        Ok(Mutated {
            value: team_view(&self.0, team),
            changed,
        })
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
    async fn create(&self, project: &Project, stamp: &Stamp) -> Result<(), RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        self.create_in(&*tx, project, stamp).await
    }

    async fn create_in(&self, _tx: &dyn Transaction, project: &Project, _stamp: &Stamp) -> Result<(), RepositoryError> {
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

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Project>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.rename_in(&*tx, id, new_slug, new_name, stamp).await?;
        Ok(out.value)
    }

    async fn rename_in(&self, _tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Project>>, RepositoryError> {
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

        // SMA-440 D5: every SUPPLIED field already equal to the stored one means this write
        // changes nothing, so it stamps nothing. The mandated order (see pg_projects.rs's
        // `rename`) is archived -> no-op -> conflict; this no-op check instead runs AFTER the
        // conflict guard above, not before it. That is equivalent only because the conflict scan
        // just above excludes self by id, so a rename that resubmits the project's own current
        // slug can never read as a conflict with itself. If that self-exclusion is ever removed,
        // this fake would start rejecting a true no-op instead of silently matching production.
        let slug_same = new_slug.is_none_or(|s| &project.slug == s);
        let name_same = new_name.is_none_or(|n| project.name == n);
        if slug_same && name_same {
            return Ok(Mutated {
                value: project_view(&self.0, project),
                changed: false,
            });
        }

        if let Some(slug) = new_slug {
            project.slug = slug.clone();
        }
        if let Some(name) = new_name {
            project.name = name.to_owned();
        }
        project.updated_at = stamp.at;
        project.modified_by = Some(stamp.by.clone());
        Ok(Mutated {
            value: project_view(&self.0, project),
            changed: true,
        })
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<NodeView<Project>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.set_status_in(&*tx, id, status, stamp).await?;
        Ok(out.value)
    }

    async fn set_status_in(&self, _tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Project>>, RepositoryError> {
        let mut projects = self.0.projects.lock().unwrap();
        let project = projects.get_mut(&id).ok_or(RepositoryError::NotFound)?;
        let changed = project.status != status;
        if changed {
            project.status = status;
            project.updated_at = stamp.at;
            project.modified_by = Some(stamp.by.clone());
        }
        Ok(Mutated {
            value: project_view(&self.0, project),
            changed,
        })
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
        created_by: m.created_by.clone(),
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
    async fn attach(&self, membership: &Membership, _stamp: &Stamp) -> Result<MembershipRecord, RepositoryError> {
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
            created_by: membership.created_by.clone(),
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

    fn new_service_account_id(&self) -> PrincipalId {
        let prn = Prn::build("iam", "", None, "principal", self.next()).expect("valid principal prn");
        PrincipalId::from_prn(prn)
    }

    fn new_api_key_id(&self) -> ApiKeyId {
        ApiKeyId::from_uuid(self.next())
    }

    fn new_audit_id(&self) -> Uuid {
        self.next()
    }

    fn new_event_id(&self) -> Uuid {
        self.next()
    }

    fn new_correlation_id(&self) -> Uuid {
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

    // Txn-scoped twins (SMA-446, Slice B — the `RoleService::grant`/`revoke` reference
    // pattern B5-B7 copy): this fake has no real backing transaction, so `tx` is ignored and
    // the mutation applies immediately — mirrors `grant`/`revoke` above.
    async fn grant_in(&self, _tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError> {
        self.0.lock().unwrap().insert(g.id, g.clone());
        Ok(())
    }

    async fn revoke_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<bool, AuthzError> {
        Ok(self.0.lock().unwrap().remove(&id).is_some())
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

    // Txn-scoped twins (SMA-446, Slice B Task B5 — the `PolicyService::put`/`delete`
    // reference pattern copies `RoleService::grant`/`revoke`'s posture): this fake has no
    // real backing transaction, so `tx` is ignored and the mutation applies immediately —
    // mirrors `InMemoryRoleGrants::grant_in`/`revoke_in`. There is no concurrent racer against
    // a single in-memory `Mutex`-guarded map, so `put_in` never has anything to absorb — it
    // only ever reports `Inserted`/`Updated`, mirroring `put`'s own insert-or-update posture
    // (`AbsorbedIdempotent` is exercised by the real-Postgres savepoint tests instead).
    async fn put_in(&self, _tx: &dyn Transaction, doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
        let mut docs = self.docs.lock().unwrap();
        if docs.get(&doc.policy_id).is_some_and(|existing| existing.system) {
            return Err(AuthzError::SystemImmutable(doc.policy_id.clone()));
        }
        let outcome = if docs.contains_key(&doc.policy_id) { PutOutcome::Updated } else { PutOutcome::Inserted };
        docs.insert(doc.policy_id.clone(), doc.clone());
        Ok(outcome)
    }

    async fn delete_in(&self, _tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError> {
        let mut docs = self.docs.lock().unwrap();
        if docs.get(policy_id).is_some_and(|existing| existing.system) {
            return Err(AuthzError::SystemImmutable(policy_id.to_string()));
        }
        Ok(docs.remove(policy_id).is_some())
    }

    async fn policy_gen(&self) -> Result<u64, AuthzError> {
        Ok(self.gen_counter.load(Ordering::SeqCst))
    }

    async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        Ok(self.gen_counter.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

/// In-memory `ServiceAccountRepository` fake for `service_accounts.rs` unit tests (SMA-445
/// Task 16), faithful to the port's doc contract: duplicate name PER OWNER ->
/// `Conflict(ServiceAccountNameTaken)` (D7); `set_principal_status` mirrors D16 (status lives
/// on the `Principal`, never on `ServiceAccount` itself) via a SEPARATE `statuses` map, keyed
/// the same way `PgServiceAccountRepository` splits `principal`/`service_account` across two
/// tables — missing id -> `NotFound`.
#[derive(Clone, Default)]
pub struct InMemoryServiceAccounts {
    pub accounts: Arc<Mutex<HashMap<Uuid, ServiceAccount>>>,
    pub statuses: Arc<Mutex<HashMap<Uuid, PrincipalStatus>>>,
}

#[async_trait]
impl ServiceAccountRepository for InMemoryServiceAccounts {
    async fn create(&self, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
        let mut accounts = self.accounts.lock().unwrap();
        if accounts.values().any(|existing| existing.owner == sa.owner && existing.name == sa.name) {
            return Err(RepositoryError::Conflict(ConflictKind::ServiceAccountNameTaken));
        }
        accounts.insert(sa.principal_id.uuid(), sa.clone());
        drop(accounts);
        self.statuses.lock().unwrap().insert(principal.id.uuid(), principal.status);
        Ok(())
    }

    // Txn-scoped twin (SMA-446, Slice B Task B7 — the `ServiceAccountService::create`
    // reference pattern): this fake has no real backing transaction, so `tx` is ignored and
    // the mutation applies immediately — mirrors `InMemoryRoleGrants::grant`/`grant_in`.
    async fn create_in(&self, _tx: &dyn Transaction, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
        let mut accounts = self.accounts.lock().unwrap();
        if accounts.values().any(|existing| existing.owner == sa.owner && existing.name == sa.name) {
            return Err(RepositoryError::Conflict(ConflictKind::ServiceAccountNameTaken));
        }
        accounts.insert(sa.principal_id.uuid(), sa.clone());
        drop(accounts);
        self.statuses.lock().unwrap().insert(principal.id.uuid(), principal.status);
        Ok(())
    }

    async fn find(&self, id: &PrincipalId) -> Result<Option<ServiceAccountRecord>, RepositoryError> {
        let Some(account) = self.accounts.lock().unwrap().get(&id.uuid()).cloned() else {
            return Ok(None);
        };
        // Mirrors `PgServiceAccountRepository::find`'s posture: the status comes from the
        // SEPARATE `statuses` map (D16 — never from `ServiceAccount` itself).
        let status = self.statuses.lock().unwrap().get(&id.uuid()).copied().unwrap_or(PrincipalStatus::Active);
        Ok(Some(ServiceAccountRecord { account, status }))
    }

    async fn list_by_owner(&self, owner: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<ServiceAccountRecord>, RepositoryError> {
        let accounts = self.accounts.lock().unwrap();
        let mut items: Vec<ServiceAccount> = accounts.values().filter(|sa| &sa.owner == owner).cloned().collect();
        drop(accounts);
        items.sort_by_key(|sa| (sa.created_at, sa.principal_id.uuid()));

        let statuses = self.statuses.lock().unwrap();
        Ok(items
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|account| {
                let status = statuses.get(&account.principal_id.uuid()).copied().unwrap_or(PrincipalStatus::Active);
                ServiceAccountRecord { account, status }
            })
            .collect())
    }

    async fn set_principal_status(&self, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError> {
        let mut statuses = self.statuses.lock().unwrap();
        if !statuses.contains_key(&id.uuid()) {
            return Err(RepositoryError::NotFound);
        }
        statuses.insert(id.uuid(), status);
        Ok(())
    }

    // Txn-scoped twin (SMA-446, Slice B Task B7 — the `ServiceAccountService::archive`
    // reference pattern): `tx` is ignored, mirroring `create_in`/`InMemoryRoleGrants::
    // revoke_in` above.
    async fn set_principal_status_in(&self, _tx: &dyn Transaction, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError> {
        let mut statuses = self.statuses.lock().unwrap();
        if !statuses.contains_key(&id.uuid()) {
            return Err(RepositoryError::NotFound);
        }
        statuses.insert(id.uuid(), status);
        Ok(())
    }
}

/// The key plus its stored hash, exactly what `ApiKeyRepository::find_by_id` returns — a
/// named alias so `InMemoryApiKeys`'s backing map doesn't trip clippy's `type_complexity`.
type StoredKey = (ApiKey, Vec<u8>);

/// In-memory `ApiKeyRepository` fake for `service_accounts.rs`/a future `ApiKeyService`'s
/// unit tests (SMA-445 Task 16): duplicate `key_hash` -> `Conflict(ApiKeyHashCollision)` (D7),
/// mirroring `PgApiKeyRepository`; `list_ids_by_service_account` is the one method archive-
/// evict actually needs, but the full port surface is implemented so the fake stays reusable
/// for Task 17's own tests without another round of fake-writing.
#[derive(Clone, Default)]
pub struct InMemoryApiKeys(pub Arc<Mutex<HashMap<ApiKeyId, StoredKey>>>);

#[async_trait]
impl ApiKeyRepository for InMemoryApiKeys {
    async fn issue(&self, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
        let mut keys = self.0.lock().unwrap();
        if keys.values().any(|(_, h)| h.as_slice() == key_hash) {
            return Err(RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision));
        }
        keys.insert(key.id, (key.clone(), key_hash.to_vec()));
        Ok(())
    }

    // Txn-scoped twin (SMA-446, Slice B Task B6 — the `ApiKeyService::issue` reference
    // pattern): this fake has no real backing transaction, so `tx` is ignored and the
    // mutation applies immediately — mirrors `InMemoryRoleGrants::grant`/`grant_in`.
    async fn issue_in(&self, _tx: &dyn Transaction, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
        let mut keys = self.0.lock().unwrap();
        if keys.values().any(|(_, h)| h.as_slice() == key_hash) {
            return Err(RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision));
        }
        keys.insert(key.id, (key.clone(), key_hash.to_vec()));
        Ok(())
    }

    async fn find_by_id(&self, id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError> {
        Ok(self.0.lock().unwrap().get(&id).cloned())
    }

    async fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<(), RepositoryError> {
        let mut keys = self.0.lock().unwrap();
        if let Some((key, _)) = keys.get_mut(&id)
            && key.status != ApiKeyStatus::Revoked
        {
            key.status = ApiKeyStatus::Revoked;
            key.revoked_at = Some(now);
        }
        Ok(())
    }

    // Txn-scoped twin, mirroring `issue_in` above: `tx` is ignored, the mutation applies
    // immediately, and the returned bool reports whether THIS call actually performed the
    // Active -> Revoked transition (`false` for an idempotent no-op — already revoked, or the
    // id was never issued at all).
    async fn revoke_in(&self, _tx: &dyn Transaction, id: ApiKeyId, now: DateTime<Utc>) -> Result<bool, RepositoryError> {
        let mut keys = self.0.lock().unwrap();
        let Some((key, _)) = keys.get_mut(&id) else {
            return Ok(false);
        };
        if key.status == ApiKeyStatus::Revoked {
            return Ok(false);
        }
        key.status = ApiKeyStatus::Revoked;
        key.revoked_at = Some(now);
        Ok(true)
    }

    async fn list_by_service_account(&self, sa: &PrincipalId, limit: u64, offset: u64) -> Result<Vec<ApiKey>, RepositoryError> {
        let keys = self.0.lock().unwrap();
        let mut items: Vec<&ApiKey> = keys.values().map(|(k, _)| k).filter(|k| &k.service_account_id == sa).collect();
        items.sort_by_key(|k| (k.created_at, k.id.uuid()));
        Ok(items.into_iter().skip(offset as usize).take(limit as usize).cloned().collect())
    }

    async fn list_ids_by_service_account(&self, sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError> {
        Ok(self.0.lock().unwrap().values().filter(|(k, _)| &k.service_account_id == sa).map(|(k, _)| k.id).collect())
    }

    async fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>, throttle_secs: u64) -> Result<(), RepositoryError> {
        let mut keys = self.0.lock().unwrap();
        if let Some((key, _)) = keys.get_mut(&id) {
            let should_update = key.last_used_at.is_none_or(|t| now - t >= chrono::Duration::seconds(throttle_secs as i64));
            if should_update {
                key.last_used_at = Some(now);
            }
        }
        Ok(())
    }
}

/// Deterministic fake `SecretHasher` for `api_keys.rs` unit tests (SMA-445 Task 17): an
/// identity transform (`hash(secret) == secret`), so a test can assert "the stored hash ==
/// `hasher.hash(secret)`" without pulling in `adapters::api_keys::HmacSecretHasher`'s real
/// HMAC — that adapter needs a real `Pepper`, an adapter-layer concern the application layer's
/// unit tests shouldn't need to construct just to exercise authorization plumbing.
#[derive(Clone, Copy, Default)]
pub struct FakeSecretHasher;

impl SecretHasher for FakeSecretHasher {
    fn hash(&self, secret: &[u8]) -> Vec<u8> {
        secret.to_vec()
    }

    fn verify(&self, secret: &[u8], expected: &[u8]) -> bool {
        secret == expected
    }
}

/// Deterministic fake `KeyEntropy` for `api_keys.rs` unit tests: sequential, never-repeating
/// 32-byte "secrets" (the call counter right-aligned into an otherwise-zero buffer), mirroring
/// `SeqIds`'s own sequential-not-random posture. Real entropy would make `issue`'s "returns
/// plaintext once, persists only the hash" assertions harder to pin down deterministically, and
/// would risk two calls in the same test colliding on `InMemoryApiKeys::issue`'s hash-uniqueness
/// guard purely by bad luck.
#[derive(Default)]
pub struct SeqKeyEntropy(AtomicU64);

impl KeyEntropy for SeqKeyEntropy {
    fn new_secret(&self) -> [u8; 32] {
        let n = self.0.fetch_add(1, Ordering::Relaxed);
        let mut secret = [0u8; 32];
        secret[24..].copy_from_slice(&n.to_be_bytes());
        secret
    }
}

/// The `Transaction` [`FakeUnitOfWork::begin`] hands out: `commit` always succeeds AND
/// increments the issuing [`FakeUnitOfWork`]'s own commit counter (SMA-469 review fix — see
/// [`FakeUnitOfWork::commits`]'s doc); dropping it without calling `commit` — an early
/// `return` on a `NotFound`/validation error, mirroring real rollback-on-drop — never touches
/// the counter, exactly like a real transaction that never commits. `savepoint` is never
/// reached by any current caller (`RoleService`'s reference pattern never opens one), and
/// `as_any` downcasts to itself, mirroring `SeaOrmTransaction`'s own `as_any` contract.
///
/// `pub(crate)` (SMA-481): `system_retirement`'s own `ScriptedRetirer` fake hands this SAME
/// type out of `begin_retirement` — its port returns a `Transaction` directly rather than going
/// through a `UnitOfWork`, but the "was this ever actually committed" guarantee this type gives
/// is exactly what its refusal tests need too. Reused, not re-derived — do not clone this type.
pub(crate) struct CountingTransaction(pub(crate) Arc<AtomicUsize>);

impl CountingTransaction {
    /// A `CountingTransaction` with no shared owner (SMA-606 Task 4): its commit counter is a
    /// throwaway `Arc`, for the node-repository fakes' `create`/`rename`/`set_status` wrappers,
    /// which need SOME `Transaction` to hand their `_in` twin but have no `FakeUnitOfWork`/
    /// caller to share a counter with (unlike `FakeUnitOfWork::begin`, which hands out a
    /// `CountingTransaction` sharing ITS OWN counter so `FakeUnitOfWork::commits` can assert on
    /// it). `pub(crate)` — not private to one impl — so Task 5's membership fake can reuse it
    /// too.
    pub(crate) fn detached() -> Self {
        CountingTransaction(Arc::new(AtomicUsize::new(0)))
    }
}

#[async_trait]
impl Transaction for CountingTransaction {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn savepoint(&mut self) -> Result<Box<dyn Savepoint<'_>>, RepositoryError> {
        unimplemented!("application-layer unit tests never open a savepoint")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// In-memory `UnitOfWork` fake for application-service unit tests (SMA-446 Slice B — the
/// `RoleService::grant`/`revoke` reference pattern B5-B7 copy): `begin` always succeeds with
/// a [`CountingTransaction`] sharing this fake's own counter. There is no real backing store
/// for these fakes to make atomic (`InMemoryRoleGrants::grant_in`/`revoke_in`, [`FakeOutbox::
/// enqueue`], [`FakeAuditLog::record`] all ignore the `&dyn Transaction` they're handed and
/// mutate their own state immediately) — what these unit tests actually exercise is the
/// SERVICE's control flow (does it call the right ports, in the right order, only once every
/// prior step succeeded); real cross-table commit/rollback atomicity is proven by the
/// Postgres integration tests instead.
///
/// `commits()` (SMA-469 review fix) reports how many transactions this fake has ever handed
/// out were actually committed. Every other fake in this module mutates its own state
/// regardless of whether `commit` is ever called — deleting a service's `tx.commit().await?`
/// line entirely would otherwise pass every existing unit test unnoticed. A test that asserts
/// `commits() == 1` after a successful mutating call closes that gap; mirrors how
/// [`FakeDeadLetters::replay_matching_calls`] exposes a call counter for its own "must not be
/// called" guarantee.
#[derive(Clone, Default)]
pub struct FakeUnitOfWork(Arc<AtomicUsize>);

impl FakeUnitOfWork {
    pub fn commits(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl UnitOfWork for FakeUnitOfWork {
    async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError> {
        Ok(Box::new(CountingTransaction(self.0.clone())))
    }
}

/// In-memory `Outbox` fake for `roles.rs` (and later B5-B7) unit tests: records every
/// enqueued event (ignoring `tx`, see [`FakeUnitOfWork`]'s doc) so a test can assert exactly
/// what — and how many — events a use case emitted.
#[derive(Clone, Default)]
pub struct FakeOutbox(pub Arc<Mutex<Vec<DomainEvent>>>);

#[async_trait]
impl Outbox for FakeOutbox {
    async fn enqueue(&self, _tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError> {
        self.0.lock().unwrap().push(ev.clone());
        Ok(())
    }
}

/// In-memory `AuditLog` fake for `roles.rs` (and later B5-B7) unit tests: `record` records
/// every in-txn call (ignoring `tx`, see [`FakeUnitOfWork`]'s doc) so a test can assert what
/// was written. `record_out_of_band` records into the SAME buffer (SMA-477), so a caller that
/// audits outside a transaction is asserted on in one place. `query` is unused by every
/// current caller of this fake and panics if reached — a future caller that needs it should
/// extend this fake rather than silently no-op (mirrors this module's other `InMemory*` fakes'
/// "implement exactly what's exercised" posture).
#[derive(Clone, Default)]
pub struct FakeAuditLog(pub Arc<Mutex<Vec<AuditEntry>>>);

#[async_trait]
impl AuditLog for FakeAuditLog {
    async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError> {
        // SMA-477: `bootstrap::reconcile_policies` holds no `Transaction`, so it records
        // out of band. Same buffer as `record` — assertions read one place.
        self.0.lock().unwrap().push(e.clone());
        Ok(())
    }

    async fn record(&self, _tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError> {
        self.0.lock().unwrap().push(e.clone());
        Ok(())
    }

    async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        unimplemented!("application-layer unit tests never call query")
    }
}

/// An `AuditLog` whose out-of-band writes always fail — proves boot survives a failed audit
/// write (SMA-477 D9). `record`/`query` are unreachable for this fake's only caller.
#[derive(Clone, Default)]
pub struct FailingAuditLog;

#[async_trait]
impl AuditLog for FailingAuditLog {
    async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
        Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
    }

    async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
        unimplemented!("FailingAuditLog is only used for the out-of-band path")
    }

    async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        unimplemented!("FailingAuditLog is only used for the out-of-band path")
    }
}

/// In-memory [`DeadLetters`] for service tests (SMA-469). `replay_in`/`discard_in` REMOVE the
/// entry, so a second call on the same id returns `None` — matching the adapter, whose
/// `AND parked = true` predicate makes a replayed or discarded row unmatchable.
#[derive(Clone, Default)]
pub struct FakeDeadLetters {
    entries: Arc<Mutex<Vec<DeadLetterEntry>>>,
    replay_matching_calls: Arc<AtomicUsize>,
}

impl FakeDeadLetters {
    pub fn seed(&self, e: DeadLetterEntry) {
        self.entries.lock().unwrap().push(e);
    }
    pub fn replay_matching_calls(&self) -> usize {
        self.replay_matching_calls.load(Ordering::SeqCst)
    }
    fn take(&self, id: Uuid) -> Option<DeadLetterEntry> {
        let mut g = self.entries.lock().unwrap();
        let idx = g.iter().position(|e| e.id == id)?;
        Some(g.remove(idx))
    }
}

#[async_trait]
impl DeadLetters for FakeDeadLetters {
    async fn list(&self, _f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError> {
        Ok(self.entries.lock().unwrap().clone())
    }

    async fn replay_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Ok(self.take(id))
    }

    async fn replay_matching_in(&self, _tx: &dyn Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError> {
        self.replay_matching_calls.fetch_add(1, Ordering::SeqCst);
        let mut g = self.entries.lock().unwrap();
        let n = g.len().min(r.capped_max_rows() as usize);
        g.drain(..n);
        Ok(n as u64)
    }

    async fn discard_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Ok(self.take(id))
    }
}

/// A `DeadLetters` whose mutating methods simulate a mid-transaction store failure (SMA-469
/// review fix), mirroring `roles.rs`'s `FailingGrantStore`: every one of `replay_in`/
/// `replay_matching_in`/`discard_in` returns `RepositoryError::Backend`, so a service's error
/// path — surfacing the error, never recording an audit entry, never committing — is proven
/// against a genuine store failure rather than by code inspection alone. `list` is never
/// exercised by any caller of this fake, so it panics rather than silently returning an empty
/// vec (mirrors `FailingGrantStore`'s "implement exactly what's exercised" posture).
#[derive(Clone, Copy, Default)]
pub struct FailingDeadLetters;

fn simulated_backend_failure() -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure")))
}

#[async_trait]
impl DeadLetters for FailingDeadLetters {
    async fn list(&self, _f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError> {
        unimplemented!("this fake only exercises the mutating methods")
    }

    async fn replay_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Err(simulated_backend_failure())
    }

    async fn replay_matching_in(&self, _tx: &dyn Transaction, _r: &BulkReplayRequest) -> Result<u64, RepositoryError> {
        Err(simulated_backend_failure())
    }

    async fn discard_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Err(simulated_backend_failure())
    }
}

/// Counting `PolicyGenBumper` fake for `roles.rs` (and later B5-B7) unit tests: `bump`
/// increments an atomic counter so a test can assert it was — or, for a rolled-back
/// mutation, was NOT — called, and exactly how many times.
#[derive(Clone, Default)]
pub struct FakePolicyGenBumper(pub Arc<AtomicU64>);

impl FakePolicyGenBumper {
    pub fn calls(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PolicyGenBumper for FakePolicyGenBumper {
    async fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn org(uuid: Uuid, slug: &str, stamp: &Stamp) -> Organization {
        Organization::new(OrganizationId::from_uuid(uuid), Slug::parse(slug).unwrap(), "Name", stamp).unwrap()
    }

    fn principal(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// An `org_admin` owner grant for `org`, mirroring `application::organizations::
    /// OrganizationService::create`'s own construction (SMA-444 Task 20b, spec D8).
    fn owner_grant(id: Uuid, owner: &PrincipalId, org: &OrganizationId) -> RoleGrant {
        RoleGrant {
            id,
            principal: owner.clone(),
            role_key: "org_admin".to_string(),
            scope: paigasus_iam_core::GrantScope::Node(TenancyNodeRef::Organization(org.clone())),
            linked_policy_id: format!("grant:{id}"),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn create_populates_the_shared_team_map_and_records_the_owner_grant() {
        let store = TenancyStore::default();
        let repo = InMemoryOrgs(store.clone());
        let now = Utc.timestamp_opt(0, 0).unwrap();
        let stamp = test_stamp(now, 1);
        let organization = org(Uuid::from_u128(1), "acme", &stamp);
        let team = Team::new(TeamId::from_parts(organization.id.uuid(), Uuid::from_u128(2)), Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
        let owner = principal(3);
        let grant = owner_grant(Uuid::from_u128(4), &owner, &organization.id);

        repo.create(&organization, &team, &grant, &stamp).await.unwrap();

        assert!(store.teams.lock().unwrap().contains_key(&team.id.uuid()));
        assert_eq!(store.role_grants.lock().unwrap().get(&grant.id), Some(&grant));
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
        let stamp = test_stamp(now, 1);

        let org_id = Uuid::from_u128(1);
        let team_id = Uuid::from_u128(2);
        let missing_project = Project::new(
            ProjectId::from_parts(org_id, Uuid::from_u128(3)),
            TeamId::from_parts(org_id, team_id),
            Slug::parse("web").unwrap(),
            "Web",
            &stamp,
        )
        .unwrap();
        assert!(matches!(repo.create(&missing_project, &stamp).await.unwrap_err(), RepositoryError::NotFound));

        let team = Team::new(TeamId::from_parts(org_id, team_id), Slug::parse("eng").unwrap(), "Eng", &stamp).unwrap();
        store.teams.lock().unwrap().insert(team_id, team);
        InMemoryTeams(store.clone()).set_status(team_id, NodeStatus::Archived, &stamp).await.unwrap();

        let project = Project::new(
            ProjectId::from_parts(org_id, Uuid::from_u128(4)),
            TeamId::from_parts(org_id, team_id),
            Slug::parse("web").unwrap(),
            "Web",
            &stamp,
        )
        .unwrap();
        assert!(matches!(
            repo.create(&project, &stamp).await.unwrap_err(),
            RepositoryError::Precondition(PreconditionKind::ParentArchived)
        ));
    }
}
