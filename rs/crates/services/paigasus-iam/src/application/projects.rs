// SPDX-License-Identifier: Apache-2.0

//! `ProjectService`: project lifecycle (create, get, list, rename, archive, restore)
//! scoped to a team (SMA-442, ADR-0014).
//!
//! **SMA-606 D1/D2/D7 — the UoW reference pattern, applied to projects:** same shape as
//! `TeamService`/`OrganizationService` — minus the three-event `create` and the `policy_gen`
//! bumper, `ProjectService` bumps only `entity_gen`. `create` builds its event/entry BEFORE
//! `uow.begin()` (D2): it constructs the project itself, so it already holds its PRN.
//! `rename`/`archive`/`restore` instead call their `_in` repository method first and build
//! from the returned `Mutated<NodeView<Project>>::value`: they receive a bare `Uuid`, not a
//! PRN, so they cannot construct one until the repository hands it back. `Mutated::changed ==
//! false` (the SMA-440 D5 no-op case) means no event and no audit entry are written, but the
//! post-commit `entity_gen` bump still runs unconditionally.

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EntityGenBumper, EventType, IdGenerator, NodeStatus, NodeView, Outbox, PrincipalId, Project, ProjectRepository, Slug, Stamp,
    TeamRepository, UnitOfWork,
};
use std::sync::Arc;
use uuid::Uuid;

/// Named-field constructor params for [`ProjectService::new`] (SMA-606 D1), mirroring
/// `TeamServiceDeps`/`OrganizationServiceDeps` — one field per dependency, built with struct
/// syntax at the call site. `gen_bumper` bumps `entity_gen` post-commit on every mutating
/// call; there is no `policy_gen_bumper` — `create` writes only the project row, never a
/// policy-changing grant.
pub struct ProjectServiceDeps<PR, TR, I, C> {
    pub projects: PR,
    pub teams: TR,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn EntityGenBumper>,
    pub ids: I,
    pub clock: C,
}

/// Project lifecycle use cases, scoped to a team. Generic-DI-by-value
/// (`P`roject `R`epository, `T`eam `R`epository, `I`d generator, `C`lock) — no `Arc<dyn>`,
/// mirroring `TeamService`/`OrganizationService` (design doc §5); `uow`/`outbox`/`audit`/
/// `gen_bumper` are the shared `Arc<dyn ...>` port handles (SMA-606 D1).
#[derive(Clone)]
pub struct ProjectService<PR, TR, I, C> {
    projects: PR,
    teams: TR,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn EntityGenBumper>,
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
    pub fn new(deps: ProjectServiceDeps<PR, TR, I, C>) -> Self {
        Self {
            projects: deps.projects,
            teams: deps.teams,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Builds the `DomainEvent` every project-lifecycle method emits, sharing ONE
    /// construction site (SMA-606, mirrors `OrganizationService::org_event`/
    /// `TeamService::team_event`) — `create` (over a freshly-built `NodeView`, D2) and
    /// `rename`/`archive`/`restore` (over `Mutated::value`) all funnel through this.
    fn project_event(&self, event_type: EventType, view: &NodeView<Project>, stamp: &Stamp, corr: Uuid) -> DomainEvent {
        DomainEvent {
            id: self.ids.new_event_id(),
            event_type,
            schema_version: 1,
            aggregate_prn: view.node.id.prn().canonical(),
            actor_prn: Some(stamp.by.canonical()),
            occurred_at: stamp.at,
            payload: serde_json::json!({
                "node_prn": view.node.id.prn().canonical(),
                "slug": view.node.slug.as_str(),
                "name": view.node.name,
                "status": view.node.status.as_str(),
                "effective_status": view.effective_status.as_str(),
            }),
            correlation_id: Some(corr),
        }
    }

    /// The `AuditEntry` twin of [`Self::project_event`] — same construction-site sharing,
    /// `detail` supplied by the caller since it's the one thing that legitimately varies per
    /// call site (SMA-606 D5).
    fn project_entry(&self, action: Action, view: &NodeView<Project>, stamp: &Stamp, corr: Uuid, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: stamp.at,
            actor_prn: Some(stamp.by.canonical()),
            action: action.as_wire().to_string(),
            resource_prn: Some(view.node.id.prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail,
            correlation_id: Some(corr),
        }
    }

    /// Resolves the team first (its org uuid is an immutable fact baked into the `TeamId`
    /// prn, so this read is race-free), mints the `ProjectId` with the team's org, then
    /// delegates; the repo re-guards in-txn (D8). `NotFound` if the team is missing;
    /// `ParentArchived` if the team is effectively archived — a cheap early exit, since the
    /// repo re-checks the same guard under lock. This pre-read stays OUTSIDE `begin()` (SMA-606):
    /// the repository re-guards under `FOR SHARE` regardless, so moving it in would only
    /// lengthen the transaction.
    ///
    /// SMA-606 D2: builds its event/entry BEFORE `uow.begin()` — it constructs the project
    /// itself, so it already holds the PRN it needs. `effective_status` equals `status` here
    /// specifically because `create_in`'s own in-txn guard re-checks the team's EFFECTIVE
    /// status (own OR ancestor org) and rejects with `ParentArchived` BEFORE this row is
    /// written — a successfully created project's whole ancestor chain is therefore active.
    /// `rename`/`archive`/`restore` get no such guarantee (the chain can be archived later)
    /// and must read `effective_status` from the repository's own returned `Mutated::value`
    /// instead of assuming equality.
    pub async fn create(&self, team: Uuid, slug: &str, name: &str, actor: &PrincipalId) -> Result<NodeView<Project>, TenancyError> {
        let team_view = self.teams.find(team).await?.ok_or(TenancyError::NotFound)?;
        if team_view.effective_status == NodeStatus::Archived {
            return Err(TenancyError::ParentArchived);
        }

        let slug = Slug::parse(slug)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let org = team_view.node.id.org_uuid();
        let id = self.ids.new_project_id(org);
        let project = Project::new(id, team_view.node.id.clone(), slug, name, &stamp)?;

        let corr = self.ids.new_correlation_id();
        // See the `effective_status` rationale in this method's doc comment above.
        let view = NodeView {
            node: project.clone(),
            effective_status: project.status,
        };
        let ev = self.project_event(EventType::ProjectCreated, &view, &stamp, corr);
        // SMA-606 Task 7 fix-round-1 finding 1: the detail must carry the event's payload shape, not
        // an empty object — slug and name are the entire content of a create.
        let detail = serde_json::json!({
            "node_prn": view.node.id.prn().canonical(),
            "slug": view.node.slug.as_str(),
            "name": view.node.name,
        });
        let entry = self.project_entry(Action::CreateProject, &view, &stamp, corr, detail);

        let tx = self.uow.begin().await?;
        self.projects.create_in(&*tx, &project, &stamp).await?;
        self.outbox.enqueue(&*tx, &ev).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // POST-COMMIT (D7): only reachable once the commit above succeeded.
        self.gen_bumper.bump().await;

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
    ///
    /// SMA-606 D2: builds its event/entry AFTER `rename_in`, from `Mutated::value` — it
    /// receives a bare `Uuid`, not a PRN, so it cannot construct one until the repository
    /// hands back the (possibly renamed) node, and the payload must carry the POST-change
    /// slug/name. `Mutated::changed == false` emits neither; the post-commit `entity_gen`
    /// bump still runs unconditionally (D7).
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Project>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.projects.rename_in(&*tx, id, slug.as_ref(), new_name, &stamp).await?;
        if out.changed {
            let ev = self.project_event(EventType::ProjectRenamed, &out.value, &stamp, corr);
            // SMA-606 D5: the detail must carry the same payload shape as the event, not an
            // empty object.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "slug": out.value.node.slug.as_str(),
                "name": out.value.node.name,
            });
            let entry = self.project_entry(Action::RenameProject, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the project's own status to `Archived`. Always permitted (D10). Idempotent: a
    /// no-op leaves `updated_at` untouched. SMA-606 D2/D7: mirrors `rename` — builds from
    /// `Mutated::value` after `set_status_in`, emits only when `changed`, and bumps
    /// `entity_gen` unconditionally post-commit.
    pub async fn archive(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Project>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.projects.set_status_in(&*tx, id, NodeStatus::Archived, &stamp).await?;
        if out.changed {
            let ev = self.project_event(EventType::ProjectArchived, &out.value, &stamp, corr);
            // SMA-606 D5: same payload shape as the event — status and effective_status are
            // two genuinely distinct fields, read from `Mutated::value` rather than assumed
            // equal (a project's own status and its effective one can differ once its team or
            // org is archived).
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.project_entry(Action::ArchiveProject, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the project's own status to `Active`. Idempotent, mirroring `archive` —
    /// including SMA-606 D2/D7's event/audit/bump posture.
    pub async fn restore(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Project>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.projects.set_status_in(&*tx, id, NodeStatus::Active, &stamp).await?;
        if out.changed {
            let ev = self.project_event(EventType::ProjectRestored, &out.value, &stamp, corr);
            // SMA-606 D5: same payload shape as the event.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.project_entry(Action::RestoreProject, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{BumpSnapshotBumper, CountingGenBumper, FakeAuditLog, FakeOutbox, FakeUnitOfWork, FixedClock, InMemoryProjects, InMemoryTeams, SeqIds, TenancyStore, test_stamp};
    use chrono::{Duration, TimeZone, Utc};
    use paigasus_iam_core::{Organization, OrganizationId, Team, TeamId};

    /// Builds a `ProjectService` over `store` with fresh, unobserved fakes for every dependency
    /// this SMA-606 conversion added — mirrors `teams.rs`'s own `new_service_with_clock`.
    fn new_service_with_clock(store: TenancyStore, clock: FixedClock) -> ProjectService<InMemoryProjects, InMemoryTeams, SeqIds, FixedClock> {
        ProjectService::new(ProjectServiceDeps {
            projects: InMemoryProjects(store.clone()),
            teams: InMemoryTeams(store),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(CountingGenBumper::default()),
            ids: SeqIds::default(),
            clock,
        })
    }

    fn new_service(store: TenancyStore) -> ProjectService<InMemoryProjects, InMemoryTeams, SeqIds, FixedClock> {
        new_service_with_clock(store, FixedClock::default())
    }

    /// Bundles a `ProjectService` together with every fake it was built over AND a freshly
    /// seeded org+team (SMA-606, mirrors `teams.rs`'s own `service_with_fakes`) — a project
    /// needs a parent team to create under. The `FakeUnitOfWork` handle is returned so a test
    /// can assert `commits()` directly (fix-round-1 finding 2, ported from `organizations.rs`).
    fn service_with_fakes() -> (
        ProjectService<InMemoryProjects, InMemoryTeams, SeqIds, FixedClock>,
        FakeOutbox,
        FakeAuditLog,
        CountingGenBumper,
        FakeUnitOfWork,
        Uuid,
    ) {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9700, 9701, &test_stamp(Utc::now(), 1));
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let gen_bumper = CountingGenBumper::default();
        let uow = FakeUnitOfWork::default();
        let svc = ProjectService::new(ProjectServiceDeps {
            projects: InMemoryProjects(store.clone()),
            teams: InMemoryTeams(store),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(gen_bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        (svc, outbox, audit, gen_bumper, uow, team)
    }

    /// A deterministic `PrincipalId` for service-call `actor` arguments — mirrors
    /// `organizations.rs`'s own test helper of the same name.
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// Seeds an org + a team under it directly into the shared store (no need to route
    /// through the services under test).
    fn seed_org_and_team(store: &TenancyStore, org_n: u128, team_n: u128, stamp: &Stamp) -> (Uuid, Uuid) {
        let org_id = Uuid::from_u128(org_n);
        let org = Organization::new(OrganizationId::from_uuid(org_id), Slug::parse("acme").unwrap(), "Acme", stamp).unwrap();
        store.orgs.lock().unwrap().insert(org_id, org);

        let team_id = Uuid::from_u128(team_n);
        let team = Team::new(TeamId::from_parts(org_id, team_id), Slug::parse("eng").unwrap(), "Engineering", stamp).unwrap();
        store.teams.lock().unwrap().insert(team_id, team);

        (org_id, team_id)
    }

    #[tokio::test]
    async fn create_project_resolves_org_from_team_and_guards() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());

        // Missing team -> NotFound.
        assert_eq!(svc.create(Uuid::from_u128(1), "web", "Web", &actor(1)).await.unwrap_err(), TenancyError::NotFound);

        let (org, team) = seed_org_and_team(&store, 9100, 9101, &test_stamp(Utc::now(), 1));
        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        assert_eq!(created.node.id.org_uuid(), org);
        assert_eq!(created.node.team_id.uuid(), team);
        assert_eq!(created.effective_status, NodeStatus::Active);

        // Archived team -> ParentArchived.
        InMemoryTeams(store.clone()).set_status(team, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        assert_eq!(svc.create(team, "mobile", "Mobile", &actor(1)).await.unwrap_err(), TenancyError::ParentArchived);
    }

    #[tokio::test]
    async fn duplicate_slug_is_conflict_scoped_to_team() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());
        let (_org1, team1) = seed_org_and_team(&store, 9200, 9201, &test_stamp(Utc::now(), 1));
        let (_org2, team2) = seed_org_and_team(&store, 9202, 9203, &test_stamp(Utc::now(), 1));

        svc.create(team1, "web", "Web", &actor(1)).await.unwrap();
        assert_eq!(svc.create(team1, "web", "Web 2", &actor(1)).await.unwrap_err(), TenancyError::SlugConflict);
        // The same slug under a different team is fine — uniqueness is scoped per team.
        svc.create(team2, "web", "Web", &actor(1)).await.unwrap();
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_restore_reverses() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9300, 9301, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        assert_eq!(created.node.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived project is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_effectively_archived_project() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9400, 9401, &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        assert_eq!(svc.rename(id, None, None, &actor(1)).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
        svc.restore(id, &actor(1)).await.unwrap();

        // Effectively archived via the team (own status untouched) also blocks rename.
        InMemoryTeams(store.clone()).set_status(team, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
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
        let (_org, team) = seed_org_and_team(&store, 9500, 9501, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let a = svc.create(team, "alpha", "Alpha", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(1));
        let b = svc.create(team, "bravo", "Bravo", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(2));
        let c = svc.create(team, "charlie", "Charlie", &actor(1)).await.unwrap();

        let page = svc.list_by_team(team, Page::new(Some(2), Some(0)).unwrap()).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].node.id.uuid(), a.node.id.uuid());
        assert_eq!(page[1].node.id.uuid(), b.node.id.uuid());

        let page2 = svc.list_by_team(team, Page::new(Some(2), Some(2)).unwrap()).await.unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].node.id.uuid(), c.node.id.uuid());
    }

    /// SMA-440 D5: a rename supplying the values the row already holds changes nothing, so it
    /// must advance neither `updated_at` nor `modified_by`.
    #[tokio::test]
    async fn rename_to_identical_values_is_a_no_op() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9600, 9601, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("web"), Some("Web"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a no-op rename must not restamp the modifier");
    }

    /// The negative half, and the one that catches an over-broad no-op: a matching slug with a
    /// DIFFERENT name is a real change and must restamp. Without this, a rename that compares
    /// only the slug would pass the test above while silently dropping every rename.
    #[tokio::test]
    async fn rename_with_a_matching_slug_but_a_new_name_still_changes() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9602, 9603, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("web"), Some("Web App"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Web App");
        assert_eq!(renamed.node.updated_at, t1);
        assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor(2)));
        // Spec Testing case 2: an update moves the MODIFIER and leaves the CREATOR alone. An
        // implementation that stamps both on every write passes every other assertion here.
        assert_eq!(renamed.node.created_by.as_ref(), Some(&actor(1)), "an update must not rewrite created_by");
        assert_eq!(renamed.node.created_at, t0, "an update must not rewrite created_at");
    }

    /// SMA-440 D5, single-field no-op: supplying ONLY a matching slug (name omitted) must
    /// still be treated as a no-op. `new_slug.is_some_and(...)` instead of `is_none_or(...)`
    /// would treat the omitted `new_name` as "differs" and wrongly restamp here.
    #[tokio::test]
    async fn rename_to_identical_slug_only_is_a_no_op() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9608, 9609, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("web"), None, &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a slug-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a slug-only no-op rename must not restamp the modifier");
    }

    /// The mirror of the above: supplying ONLY a matching name (slug omitted) must also be a
    /// no-op.
    #[tokio::test]
    async fn rename_to_identical_name_only_is_a_no_op() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9610, 9611, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, None, Some("Web"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a name-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a name-only no-op rename must not restamp the modifier");
    }

    /// Spec case 4: a DIFFERENT slug paired with the SAME name is still a real change and
    /// must restamp both fields. Complements
    /// `rename_with_a_matching_slug_but_a_new_name_still_changes`, which covers the mirror
    /// case (same slug, different name).
    #[tokio::test]
    async fn rename_with_a_new_slug_but_matching_name_still_changes() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9612, 9613, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("web-2"), Some("Web"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.slug.as_str(), "web-2");
        assert_eq!(renamed.node.updated_at, t1, "a new-slug rename must advance updated_at even with a matching name");
        assert_eq!(
            renamed.node.modified_by.as_ref(),
            Some(&actor(2)),
            "a new-slug rename must restamp the modifier even with a matching name"
        );
    }

    /// Guard order: the archived precondition runs BEFORE the no-op test, so renaming an
    /// archived node to its own slug is still an error and not a silent Ok.
    #[tokio::test]
    async fn a_no_op_rename_on_an_archived_node_is_still_rejected() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9604, 9605, &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());
        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("web"), None, &actor(2)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    /// The `set_status` half of D5: an idempotent archive advances neither field.
    #[tokio::test]
    async fn an_idempotent_archive_does_not_restamp_the_modifier() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9606, 9607, &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(team, "web", "Web", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        svc.archive(id, &actor(2)).await.unwrap();

        clock.set(t0 + Duration::seconds(20));
        let again = svc.archive(id, &actor(3)).await.unwrap();
        assert_eq!(again.node.updated_at, t0 + Duration::seconds(10));
        assert_eq!(again.node.modified_by.as_ref(), Some(&actor(2)), "a no-op archive must not restamp");
    }

    /// SMA-606 D1/D2: one event and one entry per mutation, sharing one correlation id, with
    /// the action taken from `Action::as_wire()` rather than a hand-typed literal. Also asserts
    /// `commits()` directly (fix-round-1 finding 2, ported from `organizations.rs`): every
    /// other fake mutates its own state regardless of whether `tx.commit().await?` is ever
    /// called, so without this a deleted commit call would pass unnoticed.
    #[tokio::test]
    async fn each_project_mutation_emits_one_event_and_one_entry() {
        let (svc, outbox, audit, _bumper, uow, team) = service_with_fakes();
        let actor = actor(1);

        let view = svc.create(team, "web", "Web", &actor).await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit its one transaction");

        // SMA-606 Task 7 fix-round-1 finding 1: create's own detail must carry the event's
        // payload shape too, not an empty object. Task 7 finding 3: `effective_status` is the
        // one value in `create`'s event that is SYNTHESISED rather than read back from the
        // repository (see the doc comment on `create` above), so it is the only one that can
        // be wrong without a repository bug — assert it here, before the buffers are cleared
        // below.
        let create_events = outbox.0.lock().unwrap().clone();
        assert_eq!(create_events.len(), 1);
        assert_eq!(create_events[0].event_type, EventType::ProjectCreated);
        assert_eq!(create_events[0].payload["status"], "active");
        assert_eq!(create_events[0].payload["effective_status"], "active");
        let create_entries = audit.0.lock().unwrap().clone();
        assert_eq!(create_entries.len(), 1);
        assert_eq!(create_entries[0].action, Action::CreateProject.as_wire());
        assert_eq!(create_entries[0].detail["slug"], "web");
        assert_eq!(create_entries[0].detail["name"], "Web");

        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.archive(view.node.id.uuid(), &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "archive must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ProjectArchived);
        assert_eq!(events[0].payload["status"], "archived");
        assert_eq!(
            events[0].payload["effective_status"], "archived",
            "D9: both statuses, since a node's own status and its effective one can differ"
        );

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::ArchiveProject.as_wire());
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
        // SMA-606 D5: the audit detail carries the event's payload shape (two genuinely
        // distinct fields), not an empty object.
        assert_eq!(entries[0].detail["node_prn"], events[0].payload["node_prn"]);
        assert_eq!(entries[0].detail["status"], "archived");
        assert_eq!(entries[0].detail["effective_status"], "archived");
    }

    /// SMA-606 D5: the rename detail/event payload carry the POST-change slug and name, not an
    /// empty object — the two genuinely distinct fields `{"node_prn", "slug", "name"}`.
    #[tokio::test]
    async fn rename_detail_and_event_carry_the_post_change_slug_and_name() {
        let (svc, outbox, audit, _bumper, uow, team) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(team, "web", "Web", &actor).await.unwrap();
        let id = view.node.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("web2"), Some("Web App"), &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "rename must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ProjectRenamed);
        assert_eq!(events[0].payload["slug"], "web2");
        assert_eq!(events[0].payload["name"], "Web App");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::RenameProject.as_wire());
        assert_eq!(entries[0].detail["slug"], "web2");
        assert_eq!(entries[0].detail["name"], "Web App");
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
    }

    /// SMA-606 D5: every emitted action string comes from `Action::as_wire()`. A hand-typed
    /// literal would be a free `String` nothing checks, and `AuditFilter.action` is how
    /// operators query — a typo makes rows permanently unfindable.
    #[tokio::test]
    async fn the_emitted_actions_match_the_action_vocabulary() {
        let (svc, _outbox, audit, _bumper, uow, team) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(team, "web", "Web", &actor).await.unwrap();
        let id = view.node.id.uuid();
        svc.rename(id, Some("web2"), None, &actor).await.unwrap();
        svc.archive(id, &actor).await.unwrap();
        svc.restore(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 4, "all four mutating calls commit their own transaction");

        let actions: Vec<String> = audit.0.lock().unwrap().iter().map(|e| e.action.clone()).collect();
        assert_eq!(
            actions,
            vec![
                Action::CreateProject.as_wire(),
                Action::RenameProject.as_wire(),
                Action::ArchiveProject.as_wire(),
                Action::RestoreProject.as_wire(),
            ]
        );
    }

    /// SMA-440 D5 + SMA-606 D2: a rename whose every supplied field already equals the stored
    /// one changes nothing, so it emits nothing — but still commits its transaction. The
    /// negative half (a real rename right after) is the control — without it an over-broad
    /// no-op that swallows real renames would pass.
    #[tokio::test]
    async fn a_no_op_rename_emits_nothing_but_a_real_one_emits() {
        let (svc, outbox, audit, _bumper, uow, team) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(team, "web", "Web", &actor).await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit");
        let id = view.node.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("web"), Some("Web"), &actor).await.unwrap();
        assert!(outbox.0.lock().unwrap().is_empty(), "a no-op rename emits no event");
        assert!(audit.0.lock().unwrap().is_empty(), "a no-op rename writes no audit entry");
        assert_eq!(uow.commits(), 2, "a no-op rename still commits its transaction, it just emits nothing on it");

        svc.rename(id, Some("web"), Some("Web App"), &actor).await.unwrap();
        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "a matching slug with a differing name is a real rename");
        assert_eq!(events[0].event_type, EventType::ProjectRenamed);
        assert_eq!(uow.commits(), 3, "the real rename commits its own transaction too");
    }

    /// SMA-606 D7: the bump is unconditional — it still runs for a no-op.
    #[tokio::test]
    async fn the_gen_bump_runs_even_for_a_no_op() {
        let (svc, _outbox, _audit, bumper, _uow, team) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(team, "web", "Web", &actor).await.unwrap();
        let before = bumper.bumps();

        svc.rename(view.node.id.uuid(), Some("web"), Some("Web"), &actor).await.unwrap();

        assert_eq!(bumper.bumps(), before + 1, "a no-op still bumps entity_gen");
    }

    /// SMA-606 D7: proves the bump runs strictly AFTER the commit (not merely that it runs
    /// unconditionally, which `the_gen_bump_runs_even_for_a_no_op` above already covers, but
    /// would stay green even if `bump()` moved above `tx.commit().await?`). `BumpSnapshotBumper`
    /// instead snapshots the `FakeUnitOfWork`'s own commit counter the instant `bump()` fires.
    #[tokio::test]
    async fn the_post_commit_bump_runs_strictly_after_the_transaction_commits() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9702, 9703, &test_stamp(Utc::now(), 1));
        let uow = FakeUnitOfWork::default();
        let bumper = BumpSnapshotBumper::new(uow.clone());
        let svc = ProjectService::new(ProjectServiceDeps {
            projects: InMemoryProjects(store.clone()),
            teams: InMemoryTeams(store),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        svc.create(team, "web", "Web", &actor(1)).await.unwrap();

        assert_eq!(bumper.calls(), 1);
        assert_eq!(bumper.snapshot_at_bump(), Some(1), "the commit counter must already read 1 (committed) at the instant bump() runs");
    }
}
