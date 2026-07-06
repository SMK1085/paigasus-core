// SPDX-License-Identifier: Apache-2.0

//! `ProjectService`: project lifecycle (create, get, list, rename, archive, restore)
//! scoped to a team (SMA-442, ADR-0014).

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{Clock, IdGenerator, NodeStatus, NodeView, Project, ProjectRepository, Slug, TeamRepository};
use uuid::Uuid;

/// Project lifecycle use cases, scoped to a team. Generic-DI-by-value
/// (`P`roject `R`epository, `T`eam `R`epository, `I`d generator, `C`lock) — no `Arc<dyn>`,
/// mirroring `TeamService`/`OrganizationService` (design doc §5).
#[derive(Clone)]
pub struct ProjectService<PR, TR, I, C> {
    projects: PR,
    teams: TR,
    ids: I,
    clock: C,
}

impl<PR, TR, I, C> ProjectService<PR, TR, I, C>
where
    PR: ProjectRepository,
    TR: TeamRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(projects: PR, teams: TR, ids: I, clock: C) -> Self {
        Self { projects, teams, ids, clock }
    }

    /// Resolves the team first (its org uuid is an immutable fact baked into the `TeamId`
    /// prn, so this read is race-free), mints the `ProjectId` with the team's org, then
    /// delegates; the repo re-guards in-txn (D8). `NotFound` if the team is missing;
    /// `ParentArchived` if the team is effectively archived — a cheap early exit, since the
    /// repo re-checks the same guard under lock.
    pub async fn create(&self, team: Uuid, slug: &str, name: &str) -> Result<NodeView<Project>, TenancyError> {
        let team_view = self.teams.find(team).await?.ok_or(TenancyError::NotFound)?;
        if team_view.effective_status == NodeStatus::Archived {
            return Err(TenancyError::ParentArchived);
        }

        let slug = Slug::parse(slug)?;
        let now = self.clock.now();
        let org = team_view.node.id.org_uuid();
        let id = self.ids.new_project_id(org);
        let project = Project::new(id, team_view.node.id.clone(), slug, name, now)?;

        self.projects.create(&project).await?;
        self.projects.find(project.id.uuid()).await?.ok_or(TenancyError::Internal)
    }

    /// Fetches a project by id. `NotFound` if absent.
    pub async fn get(&self, id: Uuid) -> Result<NodeView<Project>, TenancyError> {
        self.projects.find(id).await?.ok_or(TenancyError::NotFound)
    }

    /// Lists projects under `team`, `ORDER BY created_at, id` (design doc §5.1 rule 9).
    pub async fn list_by_team(&self, team: Uuid, page: Page) -> Result<Vec<NodeView<Project>>, TenancyError> {
        Ok(self.projects.list_by_team(team, page.limit, page.offset).await?)
    }

    /// Renames the slug and/or display name. Requires at least one field
    /// (`NothingToRename` otherwise); rejected on an EFFECTIVELY archived project — own
    /// status, ancestor team, or ancestor org (`NodeArchived`).
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>) -> Result<NodeView<Project>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let now = self.clock.now();
        Ok(self.projects.rename(id, slug.as_ref(), new_name, now).await?)
    }

    /// Sets the project's own status to `Archived`. Always permitted (D10). Idempotent: a
    /// no-op leaves `updated_at` untouched.
    pub async fn archive(&self, id: Uuid) -> Result<NodeView<Project>, TenancyError> {
        let now = self.clock.now();
        Ok(self.projects.set_status(id, NodeStatus::Archived, now).await?)
    }

    /// Sets the project's own status to `Active`. Idempotent, mirroring `archive`.
    pub async fn restore(&self, id: Uuid) -> Result<NodeView<Project>, TenancyError> {
        let now = self.clock.now();
        Ok(self.projects.set_status(id, NodeStatus::Active, now).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryProjects, InMemoryTeams, SeqIds, TenancyStore};
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use paigasus_iam_core::{Organization, OrganizationId, Team, TeamId};

    fn new_service(store: TenancyStore) -> ProjectService<InMemoryProjects, InMemoryTeams, SeqIds, FixedClock> {
        ProjectService::new(InMemoryProjects(store.clone()), InMemoryTeams(store), SeqIds::default(), FixedClock::default())
    }

    /// Seeds an org + a team under it directly into the shared store (no need to route
    /// through the services under test).
    fn seed_org_and_team(store: &TenancyStore, org_n: u128, team_n: u128, now: DateTime<Utc>) -> (Uuid, Uuid) {
        let org_id = Uuid::from_u128(org_n);
        let org = Organization::new(OrganizationId::from_uuid(org_id), Slug::parse("acme").unwrap(), "Acme", now).unwrap();
        store.orgs.lock().unwrap().insert(org_id, org);

        let team_id = Uuid::from_u128(team_n);
        let team = Team::new(TeamId::from_parts(org_id, team_id), Slug::parse("eng").unwrap(), "Engineering", now).unwrap();
        store.teams.lock().unwrap().insert(team_id, team);

        (org_id, team_id)
    }

    #[tokio::test]
    async fn create_project_resolves_org_from_team_and_guards() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());

        // Missing team -> NotFound.
        assert_eq!(svc.create(Uuid::from_u128(1), "web", "Web").await.unwrap_err(), TenancyError::NotFound);

        let (org, team) = seed_org_and_team(&store, 9100, 9101, Utc::now());
        let created = svc.create(team, "web", "Web").await.unwrap();
        assert_eq!(created.node.id.org_uuid(), org);
        assert_eq!(created.node.team_id.uuid(), team);
        assert_eq!(created.effective_status, NodeStatus::Active);

        // Archived team -> ParentArchived.
        InMemoryTeams(store.clone()).set_status(team, NodeStatus::Archived, Utc::now()).await.unwrap();
        assert_eq!(svc.create(team, "mobile", "Mobile").await.unwrap_err(), TenancyError::ParentArchived);
    }

    #[tokio::test]
    async fn duplicate_slug_is_conflict_scoped_to_team() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());
        let (_org1, team1) = seed_org_and_team(&store, 9200, 9201, Utc::now());
        let (_org2, team2) = seed_org_and_team(&store, 9202, 9203, Utc::now());

        svc.create(team1, "web", "Web").await.unwrap();
        assert_eq!(svc.create(team1, "web", "Web 2").await.unwrap_err(), TenancyError::SlugConflict);
        // The same slug under a different team is fine — uniqueness is scoped per team.
        svc.create(team2, "web", "Web").await.unwrap();
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_restore_reverses() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9300, 9301, Utc::now());
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = ProjectService::new(InMemoryProjects(store.clone()), InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(team, "web", "Web").await.unwrap();
        let id = created.node.id.uuid();
        assert_eq!(created.node.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived project is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id).await.unwrap();
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_effectively_archived_project() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9400, 9401, Utc::now());
        let svc = new_service(store.clone());

        let created = svc.create(team, "web", "Web").await.unwrap();
        let id = created.node.id.uuid();

        assert_eq!(svc.rename(id, None, None).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None).await.unwrap_err(), TenancyError::NodeArchived);
        svc.restore(id).await.unwrap();

        // Effectively archived via the team (own status untouched) also blocks rename.
        InMemoryTeams(store.clone()).set_status(team, NodeStatus::Archived, Utc::now()).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let store = TenancyStore::default();
        let svc = new_service(store);
        assert_eq!(svc.get(Uuid::from_u128(999)).await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn lists_are_ordered_and_paginated() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9500, 9501, Utc::now());
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = ProjectService::new(InMemoryProjects(store.clone()), InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let a = svc.create(team, "alpha", "Alpha").await.unwrap();
        clock.set(t0 + Duration::seconds(1));
        let b = svc.create(team, "bravo", "Bravo").await.unwrap();
        clock.set(t0 + Duration::seconds(2));
        let c = svc.create(team, "charlie", "Charlie").await.unwrap();

        let page = svc.list_by_team(team, Page::new(Some(2), Some(0)).unwrap()).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].node.id.uuid(), a.node.id.uuid());
        assert_eq!(page[1].node.id.uuid(), b.node.id.uuid());

        let page2 = svc.list_by_team(team, Page::new(Some(2), Some(2)).unwrap()).await.unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].node.id.uuid(), c.node.id.uuid());
    }
}
