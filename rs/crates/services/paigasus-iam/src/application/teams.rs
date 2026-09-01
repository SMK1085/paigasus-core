// SPDX-License-Identifier: Apache-2.0

//! `TeamService`: team lifecycle (create, get, list, rename, archive, restore) scoped to
//! an organization (SMA-442, ADR-0014).
//!
//! **SMA-606 D1/D2/D7 — the UoW reference pattern, applied to teams:** same shape as
//! `OrganizationService` (see its module docs for the full rationale), minus the three-event
//! `create` and the `policy_gen` bumper — `TeamService` writes no policy row, so it bumps only
//! `entity_gen`. `create` builds its event/entry BEFORE `uow.begin()` (D2): it constructs the
//! team itself, so it already holds its PRN. `rename`/`archive`/`restore` instead call their
//! `_in` repository method first and build from the returned `Mutated<NodeView<Team>>::value`:
//! they receive a bare `Uuid`, not a PRN, so they cannot construct one until the repository
//! hands it back. `Mutated::changed == false` (the SMA-440 D5 no-op case) means no event and no
//! audit entry are written, but the post-commit `entity_gen` bump still runs unconditionally.

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EntityGenBumper, EventType, IdGenerator, NodeStatus, NodeView, Outbox, PrincipalId, Slug, Stamp, Team, TeamRepository, UnitOfWork,
};
use std::sync::Arc;
use uuid::Uuid;

/// Named-field constructor params for [`TeamService::new`] (SMA-606 D1), mirroring
/// `OrganizationServiceDeps` — one field per dependency, built with struct syntax at the call
/// site so each argument is self-labeling. `gen_bumper` bumps `entity_gen` post-commit on every
/// mutating call; unlike `OrganizationServiceDeps`, there is no `policy_gen_bumper` — `create`
/// writes only the team row, never a policy-changing grant.
pub struct TeamServiceDeps<R, I, C> {
    pub repo: R,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn EntityGenBumper>,
    pub ids: I,
    pub clock: C,
}

/// Team lifecycle use cases, scoped to an organization. Generic-DI-by-value
/// (`R`epository, `I`d generator, `C`lock) — no `Arc<dyn>`, mirroring `OrganizationService`
/// (design doc §5); `uow`/`outbox`/`audit`/`gen_bumper` are the shared `Arc<dyn ...>` port
/// handles (SMA-606 D1).
#[derive(Clone)]
pub struct TeamService<R, I, C> {
    repo: R,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn EntityGenBumper>,
    ids: I,
    clock: C,
}

impl<R, I, C> TeamService<R, I, C>
where
    R: TeamRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: TeamServiceDeps<R, I, C>) -> Self {
        Self {
            repo: deps.repo,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Builds the `DomainEvent` every team-lifecycle method emits, sharing ONE construction
    /// site (SMA-606, mirrors `OrganizationService::org_event`) — `create` (over a
    /// freshly-built `NodeView`, D2) and `rename`/`archive`/`restore` (over `Mutated::value`)
    /// all funnel through this.
    fn team_event(&self, event_type: EventType, view: &NodeView<Team>, stamp: &Stamp, corr: Uuid) -> DomainEvent {
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

    /// The `AuditEntry` twin of [`Self::team_event`] — same construction-site sharing,
    /// `detail` supplied by the caller since it's the one thing that legitimately varies per
    /// call site (SMA-606 D5).
    fn team_entry(&self, action: Action, view: &NodeView<Team>, stamp: &Stamp, corr: Uuid, detail: serde_json::Value) -> AuditEntry {
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

    /// Creates a team under `org`. `NotFound` if the org is missing; `ParentArchived` if
    /// the org is effectively archived (repo-enforced in-txn guard, D8).
    ///
    /// SMA-606 D2: builds its event/entry BEFORE `uow.begin()` — it constructs the team
    /// itself, so it already holds the PRN it needs. `effective_status` equals `status` here
    /// specifically because `create_in`'s own in-txn guard rejects an effectively-archived
    /// parent org (`ParentArchived`) BEFORE this row is written — a successfully created
    /// team's parent is therefore active. `rename`/`archive`/`restore` get no such guarantee
    /// (a team's own status and its effective one CAN legitimately differ once its org is
    /// later archived) and must read `effective_status` from the repository's own returned
    /// `Mutated::value` instead of assuming equality.
    pub async fn create(&self, org: Uuid, slug: &str, name: &str, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let slug = Slug::parse(slug)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let id = self.ids.new_team_id(org);
        let team = Team::new(id, slug, name, &stamp)?;

        let corr = self.ids.new_correlation_id();
        let view = NodeView {
            node: team.clone(),
            effective_status: team.status,
        };
        let ev = self.team_event(EventType::TeamCreated, &view, &stamp, corr);
        let entry = self.team_entry(Action::CreateTeam, &view, &stamp, corr, serde_json::json!({}));

        let tx = self.uow.begin().await?;
        self.repo.create_in(&*tx, &team, &stamp).await?;
        self.outbox.enqueue(&*tx, &ev).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // POST-COMMIT (D7): only reachable once the commit above succeeded.
        self.gen_bumper.bump().await;

        self.repo.find(team.id.uuid()).await?.ok_or(TenancyError::Internal)
    }

    /// Fetches a team by id. `NotFound` if absent.
    pub async fn get(&self, id: Uuid) -> Result<NodeView<Team>, TenancyError> {
        self.repo.find(id).await?.ok_or(TenancyError::NotFound)
    }

    /// Lists teams under `org`, `ORDER BY created_at, id` (design doc §5.1 rule 9).
    pub async fn list_by_org(&self, org: Uuid, page: Page) -> Result<Vec<NodeView<Team>>, TenancyError> {
        Ok(self.repo.list_by_org(org, page.limit, page.offset).await?)
    }

    /// Renames the slug and/or display name. Requires at least one field
    /// (`NothingToRename` otherwise); rejected on an EFFECTIVELY archived team — own status
    /// or ancestor org (`NodeArchived`).
    ///
    /// SMA-606 D2: builds its event/entry AFTER `rename_in`, from `Mutated::value` — it
    /// receives a bare `Uuid`, not a PRN, so it cannot construct one until the repository hands
    /// back the (possibly renamed) node, and the payload must carry the POST-change
    /// slug/name. `Mutated::changed == false` emits neither; the post-commit `entity_gen` bump
    /// still runs unconditionally (D7).
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.rename_in(&*tx, id, slug.as_ref(), new_name, &stamp).await?;
        if out.changed {
            let ev = self.team_event(EventType::TeamRenamed, &out.value, &stamp, corr);
            // SMA-606 D5: the detail must carry the same payload shape as the event, not an
            // empty object.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "slug": out.value.node.slug.as_str(),
                "name": out.value.node.name,
            });
            let entry = self.team_entry(Action::RenameTeam, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the team's own status to `Archived`. Always permitted (D10) — a team may be
    /// archived directly even while its org is active, or while already effectively
    /// archived via the org. Idempotent: a no-op leaves `updated_at` untouched. SMA-606
    /// D2/D7: mirrors `rename` — builds from `Mutated::value` after `set_status_in`, emits
    /// only when `changed`, and bumps `entity_gen` unconditionally post-commit.
    pub async fn archive(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.set_status_in(&*tx, id, NodeStatus::Archived, &stamp).await?;
        if out.changed {
            let ev = self.team_event(EventType::TeamArchived, &out.value, &stamp, corr);
            // SMA-606 D5: same payload shape as the event — status and effective_status are
            // two genuinely distinct fields, read from `Mutated::value` rather than assumed
            // equal (a team's own status and its effective one can differ once its org is
            // archived).
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.team_entry(Action::ArchiveTeam, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the team's own status to `Active`. Idempotent, mirroring `archive` — including
    /// SMA-606 D2/D7's event/audit/bump posture. Note the team may still be *effectively*
    /// archived afterward if its org remains archived.
    pub async fn restore(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.set_status_in(&*tx, id, NodeStatus::Active, &stamp).await?;
        if out.changed {
            let ev = self.team_event(EventType::TeamRestored, &out.value, &stamp, corr);
            // SMA-606 D5: same payload shape as the event.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.team_entry(Action::RestoreTeam, &out.value, &stamp, corr, detail);
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
    use crate::application::fakes::{CountingGenBumper, FakeAuditLog, FakeOutbox, FakeUnitOfWork, FixedClock, InMemoryOrgs, InMemoryTeams, SeqIds, TenancyStore, test_stamp};
    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use paigasus_iam_core::{Organization, OrganizationId, OrganizationRepository};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Builds a `TeamService` over `store` with fresh, unobserved fakes for every dependency
    /// this SMA-606 conversion added — the tests that only care about lifecycle behaviour (not
    /// what got emitted) use this so they don't have to thread outbox/audit/uow handles through.
    fn new_service_with_clock(store: TenancyStore, clock: FixedClock) -> TeamService<InMemoryTeams, SeqIds, FixedClock> {
        TeamService::new(TeamServiceDeps {
            repo: InMemoryTeams(store),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(CountingGenBumper::default()),
            ids: SeqIds::default(),
            clock,
        })
    }

    fn new_service(store: TenancyStore) -> TeamService<InMemoryTeams, SeqIds, FixedClock> {
        new_service_with_clock(store, FixedClock::default())
    }

    /// Bundles a `TeamService` together with every fake it was built over AND a freshly seeded
    /// org (SMA-606, mirrors `organizations.rs`'s `service_with_fakes`/`service_with_fakes_and_
    /// clock`) — a team needs a parent org to create under, unlike `OrganizationService`'s own
    /// tests. The `FakeUnitOfWork` handle is returned (not just consumed) so a test can assert
    /// `commits()` directly — `fakes.rs:1082-1088` documents that every other fake mutates its
    /// state regardless of whether `commit` is ever called, so a deleted `tx.commit().await?`
    /// would otherwise pass every test unnoticed.
    fn service_with_fakes() -> (TeamService<InMemoryTeams, SeqIds, FixedClock>, FakeOutbox, FakeAuditLog, CountingGenBumper, FakeUnitOfWork, Uuid) {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9700, "acme", &test_stamp(Utc::now(), 1));
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let gen_bumper = CountingGenBumper::default();
        let uow = FakeUnitOfWork::default();
        let svc = TeamService::new(TeamServiceDeps {
            repo: InMemoryTeams(store),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(gen_bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        (svc, outbox, audit, gen_bumper, uow, org)
    }

    /// An `EntityGenBumper` that snapshots a SHARED `FakeUnitOfWork`'s own commit counter the
    /// instant `bump()` runs (SMA-606 D7) — mirrors `organizations.rs`'s `BumpSnapshotBumper`
    /// verbatim (see its doc for why an unconditional-bump-count assertion alone cannot prove
    /// the bump runs strictly AFTER the commit: moving `self.gen_bumper.bump().await` above
    /// `tx.commit().await?` would leave a plain call-count assertion green).
    #[derive(Clone)]
    struct BumpSnapshotBumper {
        uow: FakeUnitOfWork,
        snapshot_at_bump: Arc<Mutex<Option<usize>>>,
        calls: Arc<AtomicUsize>,
    }

    impl BumpSnapshotBumper {
        fn new(uow: FakeUnitOfWork) -> Self {
            BumpSnapshotBumper {
                uow,
                snapshot_at_bump: Arc::new(Mutex::new(None)),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn snapshot_at_bump(&self) -> Option<usize> {
            *self.snapshot_at_bump.lock().unwrap()
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EntityGenBumper for BumpSnapshotBumper {
        async fn bump(&self) {
            *self.snapshot_at_bump.lock().unwrap() = Some(self.uow.commits());
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A deterministic `PrincipalId` for service-call `actor` arguments — mirrors
    /// `organizations.rs`'s own test helper of the same name.
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// Seeds an org directly into the shared store (bypassing `InMemoryOrgs::create`, which
    /// would also provision an unrelated "default" team into the shared team map).
    fn seed_org(store: &TenancyStore, uuid: u128, slug: &str, stamp: &Stamp) -> Uuid {
        let id = Uuid::from_u128(uuid);
        let org = Organization::new(OrganizationId::from_uuid(id), Slug::parse(slug).unwrap(), "Org", stamp).unwrap();
        store.orgs.lock().unwrap().insert(id, org);
        id
    }

    #[tokio::test]
    async fn create_team_under_missing_or_archived_org_fails() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());

        // Missing org -> NotFound.
        assert_eq!(svc.create(Uuid::from_u128(1), "eng", "Engineering", &actor(1)).await.unwrap_err(), TenancyError::NotFound);

        // Effectively-archived org -> ParentArchived (fake honors the port's in-txn guard).
        let org = seed_org(&store, 9000, "acme", &test_stamp(Utc::now(), 1));
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        assert_eq!(svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap_err(), TenancyError::ParentArchived);
    }

    #[tokio::test]
    async fn team_effective_status_follows_org() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9001, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let team_id = created.node.id.uuid();
        assert_eq!(created.node.status, NodeStatus::Active);
        assert_eq!(created.effective_status, NodeStatus::Active);

        // Archiving the org (via the shared store, same as an `InMemoryOrgs` handle would)
        // folds into the team's effective status without touching the team's own flag.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        let view = svc.get(team_id).await.unwrap();
        assert_eq!(view.node.status, NodeStatus::Active);
        assert_eq!(view.effective_status, NodeStatus::Archived);

        // D10: archiving the team directly is still permitted even while it is already
        // effectively archived via the org.
        let archived = svc.archive(team_id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.effective_status, NodeStatus::Archived);

        // Restoring the org does not clear the team's own archived flag — it stays
        // effectively archived.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Active, &test_stamp(Utc::now(), 1)).await.unwrap();
        let still_archived = svc.get(team_id).await.unwrap();
        assert_eq!(still_archived.node.status, NodeStatus::Archived);
        assert_eq!(still_archived.effective_status, NodeStatus::Archived);
    }

    #[tokio::test]
    async fn duplicate_slug_is_conflict_scoped_to_org() {
        let store = TenancyStore::default();
        let org1 = seed_org(&store, 9002, "acme", &test_stamp(Utc::now(), 1));
        let org2 = seed_org(&store, 9003, "beta", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        svc.create(org1, "eng", "Engineering", &actor(1)).await.unwrap();
        assert_eq!(svc.create(org1, "eng", "Eng 2", &actor(1)).await.unwrap_err(), TenancyError::SlugConflict);
        // The same slug under a different org is fine — uniqueness is scoped per org.
        svc.create(org2, "eng", "Engineering", &actor(1)).await.unwrap();
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_restore_reverses() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9004, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        assert_eq!(created.node.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived team is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);

        // Restoring an already-active team is a no-op: updated_at does not advance.
        let t4 = t3 + Duration::seconds(10);
        clock.set(t4);
        let restored_again = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored_again.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_effectively_archived_team() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9005, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        assert_eq!(svc.rename(id, None, None, &actor(1)).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
        svc.restore(id, &actor(1)).await.unwrap();

        // Effectively archived via the org (own status untouched) also blocks rename.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
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
        let org = seed_org(&store, 9006, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let a = svc.create(org, "alpha", "Alpha", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(1));
        let b = svc.create(org, "bravo", "Bravo", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(2));
        let c = svc.create(org, "charlie", "Charlie", &actor(1)).await.unwrap();

        let page = svc.list_by_org(org, Page::new(Some(2), Some(0)).unwrap()).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].node.id.uuid(), a.node.id.uuid());
        assert_eq!(page[1].node.id.uuid(), b.node.id.uuid());

        let page2 = svc.list_by_org(org, Page::new(Some(2), Some(2)).unwrap()).await.unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].node.id.uuid(), c.node.id.uuid());
    }

    /// SMA-440 D5: a rename supplying the values the row already holds changes nothing, so it
    /// must advance neither `updated_at` nor `modified_by`.
    #[tokio::test]
    async fn rename_to_identical_values_is_a_no_op() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9007, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("eng"), Some("Engineering"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a no-op rename must not restamp the modifier");
    }

    /// The negative half, and the one that catches an over-broad no-op: a matching slug with a
    /// DIFFERENT name is a real change and must restamp. Without this, a rename that compares
    /// only the slug would pass the test above while silently dropping every rename.
    #[tokio::test]
    async fn rename_with_a_matching_slug_but_a_new_name_still_changes() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9008, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("eng"), Some("Engineering Team"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Engineering Team");
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
        let org = seed_org(&store, 9011, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("eng"), None, &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a slug-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a slug-only no-op rename must not restamp the modifier");
    }

    /// The mirror of the above: supplying ONLY a matching name (slug omitted) must also be a
    /// no-op.
    #[tokio::test]
    async fn rename_to_identical_name_only_is_a_no_op() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9012, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, None, Some("Engineering"), &actor(2)).await.unwrap();
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
        let org = seed_org(&store, 9013, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("eng-2"), Some("Engineering"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.slug.as_str(), "eng-2");
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
        let org = seed_org(&store, 9009, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());
        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("eng"), None, &actor(2)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    /// The `set_status` half of D5: an idempotent archive advances neither field.
    #[tokio::test]
    async fn an_idempotent_archive_does_not_restamp_the_modifier() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9010, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = new_service_with_clock(store.clone(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
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
    async fn each_team_mutation_emits_one_event_and_one_entry() {
        let (svc, outbox, audit, _bumper, uow, org) = service_with_fakes();
        let actor = actor(1);

        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit its one transaction");
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.archive(view.node.id.uuid(), &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "archive must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::TeamArchived);
        assert_eq!(events[0].payload["status"], "archived");
        assert_eq!(
            events[0].payload["effective_status"], "archived",
            "D9: both statuses, since a node's own status and its effective one can differ"
        );

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::ArchiveTeam.as_wire());
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
        let (svc, outbox, audit, _bumper, uow, org) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        let id = view.node.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("eng2"), Some("Engineering 2"), &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "rename must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::TeamRenamed);
        assert_eq!(events[0].payload["slug"], "eng2");
        assert_eq!(events[0].payload["name"], "Engineering 2");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::RenameTeam.as_wire());
        assert_eq!(entries[0].detail["slug"], "eng2");
        assert_eq!(entries[0].detail["name"], "Engineering 2");
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
    }

    /// SMA-606 D5: every emitted action string comes from `Action::as_wire()`. A hand-typed
    /// literal would be a free `String` nothing checks, and `AuditFilter.action` is how
    /// operators query — a typo makes rows permanently unfindable.
    #[tokio::test]
    async fn the_emitted_actions_match_the_action_vocabulary() {
        let (svc, _outbox, audit, _bumper, uow, org) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        let id = view.node.id.uuid();
        svc.rename(id, Some("eng2"), None, &actor).await.unwrap();
        svc.archive(id, &actor).await.unwrap();
        svc.restore(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 4, "all four mutating calls commit their own transaction");

        let actions: Vec<String> = audit.0.lock().unwrap().iter().map(|e| e.action.clone()).collect();
        assert_eq!(
            actions,
            vec![Action::CreateTeam.as_wire(), Action::RenameTeam.as_wire(), Action::ArchiveTeam.as_wire(), Action::RestoreTeam.as_wire(),]
        );
    }

    /// SMA-440 D5 + SMA-606 D2: a rename whose every supplied field already equals the stored
    /// one changes nothing, so it emits nothing — but still commits its transaction. The
    /// negative half (a real rename right after) is the control — without it an over-broad
    /// no-op that swallows real renames would pass.
    #[tokio::test]
    async fn a_no_op_rename_emits_nothing_but_a_real_one_emits() {
        let (svc, outbox, audit, _bumper, uow, org) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit");
        let id = view.node.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("eng"), Some("Engineering"), &actor).await.unwrap();
        assert!(outbox.0.lock().unwrap().is_empty(), "a no-op rename emits no event");
        assert!(audit.0.lock().unwrap().is_empty(), "a no-op rename writes no audit entry");
        assert_eq!(uow.commits(), 2, "a no-op rename still commits its transaction, it just emits nothing on it");

        svc.rename(id, Some("eng"), Some("Engineering Team"), &actor).await.unwrap();
        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "a matching slug with a differing name is a real rename");
        assert_eq!(events[0].event_type, EventType::TeamRenamed);
        assert_eq!(uow.commits(), 3, "the real rename commits its own transaction too");
    }

    /// SMA-606 D7: the bump is unconditional — it still runs for a no-op.
    #[tokio::test]
    async fn the_gen_bump_runs_even_for_a_no_op() {
        let (svc, _outbox, _audit, bumper, _uow, org) = service_with_fakes();
        let actor = actor(1);
        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        let before = bumper.bumps();

        svc.rename(view.node.id.uuid(), Some("eng"), Some("Engineering"), &actor).await.unwrap();

        assert_eq!(bumper.bumps(), before + 1, "a no-op still bumps entity_gen");
    }

    /// SMA-606 D7: proves the bump runs strictly AFTER the commit (not merely that it runs
    /// unconditionally, which `the_gen_bump_runs_even_for_a_no_op` above already covers, but
    /// would stay green even if `bump()` moved above `tx.commit().await?`). `BumpSnapshotBumper`
    /// instead snapshots the `FakeUnitOfWork`'s own commit counter the instant `bump()` fires.
    #[tokio::test]
    async fn the_post_commit_bump_runs_strictly_after_the_transaction_commits() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9702, "acme", &test_stamp(Utc::now(), 1));
        let uow = FakeUnitOfWork::default();
        let bumper = BumpSnapshotBumper::new(uow.clone());
        let svc = TeamService::new(TeamServiceDeps {
            repo: InMemoryTeams(store),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();

        assert_eq!(bumper.calls(), 1);
        assert_eq!(bumper.snapshot_at_bump(), Some(1), "the commit counter must already read 1 (committed) at the instant bump() runs");
    }
}
