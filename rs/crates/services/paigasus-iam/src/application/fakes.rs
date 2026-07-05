// SPDX-License-Identifier: Apache-2.0

//! Shared in-memory fakes for application-service tests (`#[cfg(test)]`-only, never
//! shipped). `TenancyStore` holds the tenancy state behind `Arc<Mutex<HashMap>>`s so the
//! per-port fakes — `InMemoryOrgs`, `InMemoryTeams`, `InMemoryProjects` here, plus
//! `InMemoryMemberships` added in a later task — can each clone a handle onto the *same*
//! backing data: a team fake needs to see an org archived via the org fake to compute
//! effective status (D10), and `InMemoryOrgs::create` populates the shared team map with
//! the auto-provisioned default team (ADR-0014).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{
    Clock, ConflictKind, IdGenerator, NodeStatus, NodeView, Organization, OrganizationId, OrganizationRepository, PreconditionKind, PrincipalId, Project, ProjectId, ProjectRepository,
    RepositoryError, Slug, Team, TeamId, TeamRepository,
};
use paigasus_kernel::Prn;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Shared backing store for all tenancy in-memory fakes. Cloning is cheap (shares the
/// `Arc` innards), so e.g. an `InMemoryTeams` fake sees the same org rows an
/// `InMemoryOrgs` fake mutates.
#[derive(Clone, Default)]
pub struct TenancyStore {
    pub orgs: Arc<Mutex<HashMap<Uuid, Organization>>>,
    pub teams: Arc<Mutex<HashMap<Uuid, Team>>>,
    pub projects: Arc<Mutex<HashMap<Uuid, Project>>>,
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
