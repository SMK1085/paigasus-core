// SPDX-License-Identifier: Apache-2.0

//! `OrganizationService`: organization lifecycle (create with an auto-provisioned
//! default team, get, list, rename, archive, restore) — ADR-0014.
//!
//! **SMA-606 D1/D2/D7 — the UoW reference pattern, applied to tenancy:** `create` writes the
//! org, its auto-provisioned default team, and the owner grant in one `create_in` call, then
//! enqueues THREE `DomainEvent`s (org/team/grant) and THREE `AuditEntry`s — all sharing one
//! correlation id — on the same `UnitOfWork`-scoped transaction, then commits. `create` builds
//! its events/entries BEFORE `uow.begin()` (D2): it constructs every entity itself, so it
//! already holds every PRN it needs. `rename`/`archive`/`restore` instead call their `_in`
//! repository method FIRST and build from the returned `Mutated<NodeView<Organization>>::value`
//! (D2): they receive a bare `Uuid`, not a PRN, so they cannot construct one until the
//! repository hands back the (possibly renamed) node. `Mutated::changed == false` — the
//! SMA-440 D5 no-op case — means no event and no audit entry are written (D2's extension of
//! D5), but the post-commit `entity_gen` bump still runs unconditionally (D7): cache
//! invalidation is a separate concern from audit correctness. `create` additionally bumps
//! `policy_gen` post-commit, because it also writes the owner grant — a policy change.

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EntityGenBumper, EventType, GrantScope, IdGenerator, NodeStatus, NodeView, Organization, OrganizationRepository, Outbox,
    PolicyGenBumper, PrincipalId, RoleGrant, Slug, Stamp, Team, TenancyNodeRef, UnitOfWork,
};
use std::sync::Arc;
use uuid::Uuid;

/// Output of [`OrganizationService::create`]: the new org plus its auto-provisioned
/// `"default"` team, created together in one repository transaction (ADR-0014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrgOutput {
    pub organization: Organization,
    pub default_team: Team,
}

/// Named-field constructor params for [`OrganizationService::new`] (SMA-606 D1) — copies
/// `application::roles::RoleServiceDeps`/`application::api_keys::ApiKeyServiceDeps`'s DI-params
/// idiom verbatim: one field per dependency, built with struct syntax at the call site so each
/// argument is self-labeling. `gen_bumper` bumps `entity_gen` post-commit on every mutating
/// call; `policy_gen_bumper` bumps `policy_gen` post-commit too, but only from `create` (the
/// owner grant it writes is a policy change — module docs).
pub struct OrganizationServiceDeps<R, I, C> {
    pub repo: R,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn EntityGenBumper>,
    pub policy_gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}

/// Organization lifecycle use cases. Generic-DI-by-value (`R`epository, `I`d generator,
/// `C`lock) — no `Arc<dyn>`, mirroring M0's `CreateUser` (per-aggregate grouping, design
/// doc §5); `uow`/`outbox`/`audit`/`gen_bumper`/`policy_gen_bumper` are the shared `Arc<dyn
/// ...>` port handles (SMA-606 D1), mirroring `RoleService`'s own split between generic-DI
/// repository access and `Arc<dyn ...>` cross-cutting ports.
#[derive(Clone)]
pub struct OrganizationService<R, I, C> {
    repo: R,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn EntityGenBumper>,
    policy_gen_bumper: Arc<dyn PolicyGenBumper>,
    ids: I,
    clock: C,
}

impl<R, I, C> OrganizationService<R, I, C>
where
    R: OrganizationRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: OrganizationServiceDeps<R, I, C>) -> Self {
        Self {
            repo: deps.repo,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            policy_gen_bumper: deps.policy_gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Builds the `DomainEvent` every org-lifecycle method emits, sharing ONE construction
    /// site (SMA-606, the `system_retirement.rs`/`dead_letters.rs` `audit_entry` helper
    /// precedent) — `create` (over a freshly-built `NodeView`, D2) and `rename`/`archive`/
    /// `restore` (over `Mutated::value`) all funnel through this.
    fn org_event(&self, event_type: EventType, view: &NodeView<Organization>, stamp: &Stamp, corr: Uuid) -> DomainEvent {
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

    /// The `AuditEntry` twin of [`Self::org_event`] — same construction-site sharing, `detail`
    /// supplied by the caller since it's the one thing that legitimately varies per call site.
    fn org_entry(&self, action: Action, view: &NodeView<Organization>, stamp: &Stamp, corr: Uuid, detail: serde_json::Value) -> AuditEntry {
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

    /// Creates an organization, its auto-provisioned `"default"` team, and an `org_admin`
    /// owner grant for `actor` scoped to the new org, all in one repository transaction
    /// (ADR-0014, spec D8) — the creating principal becomes the owner of what it creates.
    ///
    /// SMA-606 D4: writes three rows, so it emits three events (`OrganizationCreated`,
    /// `TeamCreated`, `RoleGranted`) and three audit entries, all sharing ONE correlation id.
    /// The team and role events/entries carry `"source": "organization_create"` in their
    /// payload/detail (mirrors `bootstrap_admin.rs`'s own `"source": "bootstrap_admins"`
    /// convention) — a consumer can tell the auto-provisioned team from an explicit
    /// `TeamService::create`, and this grant from a user-requested `RoleService::grant` that
    /// actually passed its own anti-escalation check. The role event/entry's `aggregate_prn`/
    /// `resource_prn` follow `RoleService::grant`/`BootstrapAdminSeeder::seed_grant`'s own
    /// convention (`roles.rs:224`, `bootstrap_admin.rs:146-175`): the PRINCIPAL's prn, not the
    /// node's.
    pub async fn create(&self, actor: &PrincipalId, slug: &str, name: &str) -> Result<CreateOrgOutput, TenancyError> {
        let slug = Slug::parse(slug)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());

        let org_id = self.ids.new_organization_id();
        let organization = Organization::new(org_id, slug, name, &stamp)?;

        let team_id = self.ids.new_team_id(organization.id.uuid());
        let default_slug = Slug::parse("default").expect("\"default\" is a valid slug");
        // The auto-provisioned default team records the ORG's creator (spec D8) — the same
        // Stamp, so the two rows cannot disagree.
        let default_team = Team::new(team_id, default_slug, "Default", &stamp)?;

        let grant_id = self.ids.new_membership_id();
        let owner_grant = RoleGrant {
            id: grant_id,
            principal: actor.clone(),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Node(TenancyNodeRef::Organization(organization.id.clone())),
            linked_policy_id: format!("grant:{grant_id}"),
            created_at: stamp.at,
        };

        // SMA-606 D2: `create` holds every PRN already (it just constructed all three rows
        // above), so it builds every event/entry BEFORE opening the transaction — unlike
        // `rename`/`archive`/`restore`, which build AFTER their `_in` call, from `Mutated::value`.
        let corr = self.ids.new_correlation_id();
        let org_view = NodeView {
            node: organization.clone(),
            effective_status: organization.status,
        };
        let org_event = self.org_event(EventType::OrganizationCreated, &org_view, &stamp, corr);
        // SMA-606 Task 7 fix-round-1 finding 1, extended by the fix wave (D5): the detail must
        // equal the event's payload shape in full, including status/effective_status.
        let org_detail = serde_json::json!({
            "node_prn": org_view.node.id.prn().canonical(),
            "slug": org_view.node.slug.as_str(),
            "name": org_view.node.name,
            "status": org_view.node.status.as_str(),
            "effective_status": org_view.effective_status.as_str(),
        });
        let org_entry = self.org_entry(Action::CreateOrganization, &org_view, &stamp, corr, org_detail);

        let team_event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::TeamCreated,
            schema_version: 1,
            aggregate_prn: default_team.id.prn().canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: stamp.at,
            payload: serde_json::json!({
                "node_prn": default_team.id.prn().canonical(),
                "slug": default_team.slug.as_str(),
                "name": default_team.name,
                "status": default_team.status.as_str(),
                "effective_status": default_team.status.as_str(),
                "source": "organization_create",
            }),
            correlation_id: Some(corr),
        };
        let team_entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: stamp.at,
            actor_prn: Some(actor.canonical()),
            action: Action::CreateTeam.as_wire().to_string(),
            resource_prn: Some(default_team.id.prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            // SMA-606 fix wave (D5): detail must equal the event's payload shape plus the
            // provenance key, not the provenance key alone — this escaped two earlier fix
            // rounds because both inspected the org's own create entry and neither looked at
            // this sibling team entry.
            detail: serde_json::json!({
                "node_prn": default_team.id.prn().canonical(),
                "slug": default_team.slug.as_str(),
                "name": default_team.name,
                "status": default_team.status.as_str(),
                "effective_status": default_team.status.as_str(),
                "source": "organization_create",
            }),
            correlation_id: Some(corr),
        };

        // SMA-606: mirrors `RoleService::grant` (`roles.rs:224`) and `BootstrapAdminSeeder::
        // seed_grant` (`bootstrap_admin.rs:146-175`) — the role event's `aggregate_prn` is the
        // PRINCIPAL's prn, not the node's, exactly like those two other `RoleGranted` emitters.
        let grant_event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::RoleGranted,
            schema_version: 1,
            aggregate_prn: actor.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: stamp.at,
            payload: serde_json::json!({
                "grant_id": owner_grant.id,
                "role_key": owner_grant.role_key,
                "scope": owner_grant.scope.canonical_prn(),
                "source": "organization_create",
            }),
            correlation_id: Some(corr),
        };
        let grant_entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: stamp.at,
            actor_prn: Some(actor.canonical()),
            action: Action::GrantRole.as_wire().to_string(),
            resource_prn: Some(organization.id.prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({
                "grant_id": owner_grant.id,
                "role_key": owner_grant.role_key,
                "scope": owner_grant.scope.canonical_prn(),
                "source": "organization_create",
            }),
            correlation_id: Some(corr),
        };

        let events = vec![org_event, team_event, grant_event];
        let entries = vec![org_entry, team_entry, grant_entry];

        let tx = self.uow.begin().await?;
        self.repo.create_in(&*tx, &organization, &default_team, &owner_grant, &stamp).await?;
        for ev in &events {
            self.outbox.enqueue(&*tx, ev).await?;
        }
        for entry in &entries {
            self.audit.record(&*tx, entry).await?;
        }
        tx.commit().await?;

        // POST-COMMIT (D7): only reachable once the commit above succeeded. Both bumps run,
        // because `create` writes the owner grant — a policy change — as well as three entity
        // rows.
        self.gen_bumper.bump().await;
        self.policy_gen_bumper.bump().await;

        Ok(CreateOrgOutput { organization, default_team })
    }

    /// Fetches an organization by id. `NotFound` if absent.
    pub async fn get(&self, id: Uuid) -> Result<NodeView<Organization>, TenancyError> {
        self.repo.find(id).await?.ok_or(TenancyError::NotFound)
    }

    /// Lists organizations, `ORDER BY created_at, id` (design doc §5.1 rule 9).
    pub async fn list(&self, page: Page) -> Result<Vec<NodeView<Organization>>, TenancyError> {
        Ok(self.repo.list(page.limit, page.offset).await?)
    }

    /// Renames the slug and/or display name. Requires at least one field
    /// (`NothingToRename` otherwise); rejected on an (effectively) archived org
    /// (`NodeArchived`).
    ///
    /// SMA-606 D2: builds its event/audit entry AFTER `rename_in`, from `Mutated::value` — it
    /// receives a bare `Uuid`, not a PRN, so it cannot construct one until the repository hands
    /// back the (possibly renamed) node, and the payload must carry the POST-change slug/name.
    /// `Mutated::changed == false` (SMA-440 D5's no-op) emits neither; the post-commit
    /// `entity_gen` bump still runs unconditionally (D7).
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.rename_in(&*tx, id, slug.as_ref(), new_name, &stamp).await?;
        if out.changed {
            let ev = self.org_event(EventType::OrganizationRenamed, &out.value, &stamp, corr);
            // fix-round-1 finding 3 (spec D5): the detail must carry the same payload shape as
            // the event, not an empty object — else the audit row records THAT a rename
            // happened but not WHAT it changed to, recoverable only by joining the outbox event
            // on `correlation_id`.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "slug": out.value.node.slug.as_str(),
                "name": out.value.node.name,
            });
            let entry = self.org_entry(Action::RenameOrganization, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the org's own status to `Archived`. Idempotent: a no-op leaves `updated_at`
    /// untouched if already archived (D10). SMA-606 D2/D7: mirrors `rename` — builds from
    /// `Mutated::value` after `set_status_in`, emits only when `changed`, and bumps
    /// `entity_gen` unconditionally post-commit.
    pub async fn archive(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.set_status_in(&*tx, id, NodeStatus::Archived, &stamp).await?;
        if out.changed {
            let ev = self.org_event(EventType::OrganizationArchived, &out.value, &stamp, corr);
            // fix-round-1 finding 3 (spec D5): same payload shape as the event.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.org_entry(Action::ArchiveOrganization, &out.value, &stamp, corr, detail);
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }

    /// Sets the org's own status to `Active`. Idempotent, mirroring `archive` — including
    /// SMA-606 D2/D7's event/audit/bump posture.
    pub async fn restore(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.set_status_in(&*tx, id, NodeStatus::Active, &stamp).await?;
        if out.changed {
            let ev = self.org_event(EventType::OrganizationRestored, &out.value, &stamp, corr);
            // fix-round-1 finding 3 (spec D5): same payload shape as the event.
            let detail = serde_json::json!({
                "node_prn": out.value.node.id.prn().canonical(),
                "status": out.value.node.status.as_str(),
                "effective_status": out.value.effective_status.as_str(),
            });
            let entry = self.org_entry(Action::RestoreOrganization, &out.value, &stamp, corr, detail);
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
    use crate::application::fakes::{BumpSnapshotBumper, CountingGenBumper, FailingRenameOrgs, FakeAuditLog, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork, FixedClock, InMemoryOrgs, SeqIds};
    use chrono::{Duration, TimeZone, Utc};

    /// Bundles an `OrganizationService` together with every fake it was built over (SMA-606,
    /// mirrors `api_keys.rs`'s `ServiceWithFakes`/`new_service_with_fakes`, `:393-434`), so a
    /// test can assert on exactly what `create`/`rename`/`archive`/`restore` emitted through the
    /// outbox/audit ports, how many times the post-commit `entity_gen` bump ran, AND (fix-round-1
    /// finding 2) that the transaction actually committed — `fakes.rs:1082-1088`'s own doc warns
    /// that every other fake mutates its state regardless of whether `commit` is ever called, so
    /// a deleted `tx.commit().await?` would otherwise pass unnoticed. The `FakeUnitOfWork`
    /// handle is returned (not just consumed) so a test can assert `commits()` directly, mirrring
    /// `dead_letters.rs:467,489,508`/`system_retirement.rs:781,831,904`.
    fn service_with_fakes_and_clock(clock: FixedClock) -> (OrganizationService<InMemoryOrgs, SeqIds, FixedClock>, FakeOutbox, FakeAuditLog, CountingGenBumper, FakeUnitOfWork) {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let gen_bumper = CountingGenBumper::default();
        let uow = FakeUnitOfWork::default();
        let svc = OrganizationService::new(OrganizationServiceDeps {
            repo: InMemoryOrgs::default(),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(gen_bumper.clone()),
            policy_gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock,
        });
        (svc, outbox, audit, gen_bumper, uow)
    }

    fn service_with_fakes() -> (OrganizationService<InMemoryOrgs, SeqIds, FixedClock>, FakeOutbox, FakeAuditLog, CountingGenBumper, FakeUnitOfWork) {
        service_with_fakes_and_clock(FixedClock::default())
    }

    fn new_service() -> OrganizationService<InMemoryOrgs, SeqIds, FixedClock> {
        service_with_fakes().0
    }

    /// Mirrors `service_with_fakes_and_clock`, but wires `FailingRenameOrgs` in place of
    /// `InMemoryOrgs` (SMA-606 Testing case 6) so a test can prove `rename`'s outbox/audit
    /// writes never survive a `rename_in` failure — the brief for this task specifies only the
    /// fake, not this helper; it is modelled on `service_with_fakes`'s own return shape,
    /// `FakeUnitOfWork` handle included, mirroring `roles.rs`'s `a_store_error_mid_txn_...`
    /// helper wiring in `FailingGrantStore`.
    fn service_with_failing_rename() -> (OrganizationService<FailingRenameOrgs, SeqIds, FixedClock>, FakeOutbox, FakeAuditLog, CountingGenBumper, FakeUnitOfWork) {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let gen_bumper = CountingGenBumper::default();
        let uow = FakeUnitOfWork::default();
        let svc = OrganizationService::new(OrganizationServiceDeps {
            repo: FailingRenameOrgs(InMemoryOrgs::default()),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(gen_bumper.clone()),
            policy_gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        (svc, outbox, audit, gen_bumper, uow)
    }

    /// A deterministic `PrincipalId` for `create`'s `actor` argument — the tests below don't
    /// exercise authorization (that's `adapters::http`/`grpc`'s job), just that `create`
    /// threads whatever actor it's given into the owner grant (see `fakes.rs`'s
    /// `InMemoryOrgs` recording it).
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// A raw `Prn` for a principal — the shape `PrincipalId::from_prn` (SMA-606's new tests)
    /// takes, mirroring `roles.rs::tests::principal_prn`.
    fn principal_prn(n: u128) -> paigasus_kernel::Prn {
        paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    #[tokio::test]
    async fn create_provisions_default_team() {
        let svc = new_service();
        let out = svc.create(&actor(1), "acme", "Acme Corp.").await.unwrap();
        assert_eq!(out.default_team.slug.as_str(), "default");
        assert_eq!(out.default_team.id.org_uuid(), out.organization.id.uuid());
    }

    #[tokio::test]
    async fn duplicate_slug_is_conflict() {
        let svc = new_service();
        svc.create(&actor(1), "acme", "A").await.unwrap();
        assert_eq!(svc.create(&actor(1), "acme", "B").await.unwrap_err(), TenancyError::SlugConflict);
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_restore_reverses() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();
        assert_eq!(created.organization.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.effective_status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived org is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived_again.node.status, NodeStatus::Archived);
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.effective_status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);

        // Restoring an already-active org is a no-op: updated_at does not advance.
        let t4 = t3 + Duration::seconds(10);
        clock.set(t4);
        let restored_again = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored_again.node.status, NodeStatus::Active);
        assert_eq!(restored_again.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_archived_node() {
        let svc = new_service();
        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        assert_eq!(svc.rename(id, None, None, &actor(1)).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let svc = new_service();
        assert_eq!(svc.get(Uuid::from_u128(999)).await.unwrap_err(), TenancyError::NotFound);
    }

    /// SMA-440 D5: a rename supplying the values the row already holds changes nothing, so it
    /// must advance neither `updated_at` nor `modified_by`.
    #[tokio::test]
    async fn rename_to_identical_values_is_a_no_op() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("acme"), Some("Acme"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a no-op rename must not restamp the modifier");
    }

    /// The negative half, and the one that catches an over-broad no-op: a matching slug with a
    /// DIFFERENT name is a real change and must restamp. Without this, a rename that compares
    /// only the slug would pass the test above while silently dropping every rename.
    #[tokio::test]
    async fn rename_with_a_matching_slug_but_a_new_name_still_changes() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("acme"), Some("Acme Corp."), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Acme Corp.");
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
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("acme"), None, &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a slug-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a slug-only no-op rename must not restamp the modifier");
    }

    /// The mirror of the above: supplying ONLY a matching name (slug omitted) must also be a
    /// no-op.
    #[tokio::test]
    async fn rename_to_identical_name_only_is_a_no_op() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, None, Some("Acme"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a name-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a name-only no-op rename must not restamp the modifier");
    }

    /// Spec case 4: a DIFFERENT slug paired with the SAME name is still a real change and
    /// must restamp both fields. Complements
    /// `rename_with_a_matching_slug_but_a_new_name_still_changes`, which covers the mirror
    /// case (same slug, different name).
    #[tokio::test]
    async fn rename_with_a_new_slug_but_matching_name_still_changes() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("acme-2"), Some("Acme"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.slug.as_str(), "acme-2");
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
        let svc = new_service();
        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("acme"), None, &actor(2)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    /// The `set_status` half of D5: an idempotent archive advances neither field.
    #[tokio::test]
    async fn an_idempotent_archive_does_not_restamp_the_modifier() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_with_fakes_and_clock(clock.clone()).0;

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        svc.archive(id, &actor(2)).await.unwrap();

        clock.set(t0 + Duration::seconds(20));
        let again = svc.archive(id, &actor(3)).await.unwrap();
        assert_eq!(again.node.updated_at, t0 + Duration::seconds(10));
        assert_eq!(again.node.modified_by.as_ref(), Some(&actor(2)), "a no-op archive must not restamp");
    }

    /// SMA-606 D4: create writes three rows, so it emits three events on ONE correlation id.
    /// The team and role events carry `"source": "organization_create"` so a consumer can tell
    /// the auto-provisioned team from an explicit one, and this grant from a user-requested
    /// one that actually passed `RoleService::grant`'s anti-escalation check.
    #[tokio::test]
    async fn create_emits_three_events_on_one_correlation_id() {
        let (svc, outbox, audit, _bumper, uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));

        svc.create(&actor, "acme", "Acme").await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit its one transaction (fix-round-1 finding 2)");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3, "org create writes three rows, so it emits three events");
        let corr = events[0].correlation_id.expect("every tenancy event carries a correlation id");
        assert!(events.iter().all(|e| e.correlation_id == Some(corr)), "all three share one correlation id");

        let types: Vec<EventType> = events.iter().map(|e| e.event_type).collect();
        assert_eq!(types, vec![EventType::OrganizationCreated, EventType::TeamCreated, EventType::RoleGranted]);

        let team = &events[1];
        assert_eq!(team.payload["source"], "organization_create");
        let grant = &events[2];
        assert_eq!(grant.payload["source"], "organization_create");
        assert_eq!(
            grant.aggregate_prn,
            actor.canonical(),
            "the role event's aggregate is the principal, matching RoleService and BootstrapAdminSeeder"
        );

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.correlation_id == Some(corr)));
        assert_eq!(entries[0].action, Action::CreateOrganization.as_wire());
        // SMA-606 Task 7 fix-round-2 finding 1: the org create entry's own detail must carry
        // the event's payload shape too, not go unverified — `org_detail` (organizations.rs)
        // could otherwise lose a field or carry a typo and every test would still pass.
        assert_eq!(entries[0].detail["node_prn"], events[0].payload["node_prn"]);
        assert_eq!(entries[0].detail["slug"], "acme");
        assert_eq!(entries[0].detail["name"], "Acme");
        // fix-round-1 finding 3: pin the team/grant entries' own action AND source too, so a
        // wrong `Action` variant on either — or a dropped/renamed "source" key — cannot pass.
        assert_eq!(entries[1].action, Action::CreateTeam.as_wire());
        assert_eq!(entries[1].detail["source"], "organization_create");
        assert_eq!(entries[2].action, Action::GrantRole.as_wire());
        assert_eq!(entries[2].detail["source"], "organization_create");
    }

    /// SMA-606 fix wave finding 4: `OrganizationService` never got the per-mutation emission
    /// tests its two siblings (`TeamService`, `ProjectService`) have —
    /// `each_team_mutation_emits_one_event_and_one_entry` (`teams.rs:635`) is the model. `create`'s
    /// own three-event shape is already covered by `create_emits_three_events_on_one_correlation_id`
    /// above, so this covers archive/restore, the two mutations `EventType::OrganizationArchived`/
    /// `EventType::OrganizationRestored` and `Action::ArchiveOrganization`/
    /// `Action::RestoreOrganization` had NO assertion anywhere in the repository before this.
    #[tokio::test]
    async fn each_org_mutation_emits_one_event_and_one_entry() {
        let (svc, outbox, audit, _bumper, uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit its one transaction");
        let id = out.organization.id.uuid();

        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.archive(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "archive must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::OrganizationArchived);
        assert_eq!(
            events[0].aggregate_prn,
            out.organization.id.prn().canonical(),
            "SMA-606 fix wave: pin aggregate_prn once per service (orgs/teams/projects)"
        );
        assert_eq!(events[0].payload["status"], "archived");
        assert_eq!(
            events[0].payload["effective_status"], "archived",
            "D9: both statuses, since a node's own status and its effective one can differ"
        );

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::ArchiveOrganization.as_wire());
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
        assert_eq!(entries[0].detail["node_prn"], events[0].payload["node_prn"]);
        assert_eq!(entries[0].detail["status"], "archived");
        assert_eq!(entries[0].detail["effective_status"], "archived");

        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.restore(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 3, "restore must commit its own transaction too");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::OrganizationRestored);
        assert_eq!(events[0].payload["status"], "active");
        assert_eq!(events[0].payload["effective_status"], "active");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::RestoreOrganization.as_wire());
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
    }

    /// SMA-606 D5: every emitted action string comes from `Action::as_wire()`. A hand-typed
    /// literal would be a free `String` nothing checks, and `AuditFilter.action` is how
    /// operators query — a typo makes rows permanently unfindable. SMA-606 fix wave finding 4:
    /// also pins the EVENT TYPE for restore — `EventType::OrganizationRestored` was asserted
    /// nowhere in the repository before this — mirroring `TeamService`/`ProjectService`'s own
    /// `the_emitted_actions_match_the_action_vocabulary` (`teams.rs:717`).
    #[tokio::test]
    async fn the_emitted_actions_match_the_action_vocabulary() {
        let (svc, outbox, audit, _bumper, uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        let id = out.organization.id.uuid();
        svc.rename(id, Some("acme2"), None, &actor).await.unwrap();
        svc.archive(id, &actor).await.unwrap();
        svc.restore(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 4, "all four mutating calls commit their own transaction");

        // create emits three entries (org/team/grant); rename/archive/restore emit one each.
        let actions: Vec<String> = audit.0.lock().unwrap().iter().map(|e| e.action.clone()).collect();
        assert_eq!(
            actions,
            vec![
                Action::CreateOrganization.as_wire(),
                Action::CreateTeam.as_wire(),
                Action::GrantRole.as_wire(),
                Action::RenameOrganization.as_wire(),
                Action::ArchiveOrganization.as_wire(),
                Action::RestoreOrganization.as_wire(),
            ]
        );

        let event_types: Vec<EventType> = outbox.0.lock().unwrap().iter().map(|e| e.event_type).collect();
        assert_eq!(
            event_types,
            vec![
                EventType::OrganizationCreated,
                EventType::TeamCreated,
                EventType::RoleGranted,
                EventType::OrganizationRenamed,
                EventType::OrganizationArchived,
                EventType::OrganizationRestored,
            ]
        );
    }

    /// SMA-440 D5 + SMA-606 D2: a rename whose every supplied field already equals the stored
    /// one changes nothing, so it emits nothing. The negative half is the control — without
    /// it an over-broad no-op that swallows real renames passes.
    #[tokio::test]
    async fn a_no_op_rename_emits_nothing_but_a_real_one_emits() {
        let (svc, outbox, audit, _bumper, uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        assert_eq!(uow.commits(), 1, "create must commit (fix-round-1 finding 2)");
        let id = out.organization.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("acme"), Some("Acme"), &actor).await.unwrap();
        assert!(outbox.0.lock().unwrap().is_empty(), "a no-op rename emits no event");
        assert!(audit.0.lock().unwrap().is_empty(), "a no-op rename writes no audit entry");
        assert_eq!(uow.commits(), 2, "a no-op rename still commits its transaction, it just emits nothing on it");

        svc.rename(id, Some("acme"), Some("Acme Inc"), &actor).await.unwrap();
        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "a matching slug with a differing name is a real rename");
        assert_eq!(events[0].event_type, EventType::OrganizationRenamed);
        assert_eq!(events[0].payload["name"], "Acme Inc", "the payload carries the POST-change name");
        assert_eq!(uow.commits(), 3, "the real rename commits its own transaction too");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        // fix-round-1 finding 3: the audit entry must carry the same payload shape as its
        // event, not an empty detail — else the audit row records that something changed but
        // not what it changed to.
        assert_eq!(entries[0].detail["slug"], "acme");
        assert_eq!(entries[0].detail["name"], "Acme Inc", "the audit detail carries the POST-change name too");
    }

    /// SMA-606 fix wave finding 5 (spec Testing case 3): an idempotent `set_status` no-op
    /// emits nothing. Six `if out.changed` sites gate the `set_status` path and no test at
    /// either tier read a `Mutated` from `set_status_in` before this — a `set_status_in` that
    /// wrote nothing yet returned `changed: true` would pass every existing test and emit a
    /// spurious archive event plus a false audit row on every repeat archive. Modelled on
    /// `a_no_op_rename_emits_nothing_but_a_real_one_emits`: archive, clear the buffers, archive
    /// again (no-op — the negative half), then restore (a real transition — the positive half
    /// that stops an over-broad no-op passing).
    #[tokio::test]
    async fn a_no_op_archive_emits_nothing_but_a_real_restore_emits() {
        let (svc, outbox, audit, _bumper, uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        let id = out.organization.id.uuid();
        assert_eq!(uow.commits(), 1, "create must commit");

        svc.archive(id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 2, "archive must commit its own transaction");
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.archive(id, &actor).await.unwrap();
        assert!(outbox.0.lock().unwrap().is_empty(), "an idempotent archive emits no event");
        assert!(audit.0.lock().unwrap().is_empty(), "an idempotent archive writes no audit entry");
        assert_eq!(uow.commits(), 3, "a no-op archive still commits its transaction, it just emits nothing on it");

        svc.restore(id, &actor).await.unwrap();
        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "restoring an archived org is a real transition");
        assert_eq!(events[0].event_type, EventType::OrganizationRestored);
        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1, "the real restore writes a real audit entry");
        assert_eq!(uow.commits(), 4, "the real restore commits its own transaction too");
    }

    /// SMA-606 D7: the bump is unconditional — it still runs for a no-op, preserving SMA-440
    /// D5's deliberate choice to leave cache invalidation alone. This does NOT prove the bump
    /// runs AFTER the commit (fix-round-1 finding 1): `CountingGenBumper`'s call count is
    /// identical whether `gen_bumper.bump().await` sits before or after `tx.commit().await?`.
    /// See `the_post_commit_bump_runs_strictly_after_the_transaction_commits` below for the
    /// ordering proof.
    #[tokio::test]
    async fn the_gen_bump_runs_after_commit_and_even_for_a_no_op() {
        let (svc, _outbox, _audit, bumper, _uow) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1));
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        let before = bumper.bumps();

        svc.rename(out.organization.id.uuid(), Some("acme"), Some("Acme"), &actor).await.unwrap();

        assert_eq!(bumper.bumps(), before + 1, "a no-op still bumps entity_gen");
    }

    /// SMA-606 D7, fix-round-1 finding 1: `the_gen_bump_runs_after_commit_and_even_for_a_no_op`
    /// above proves the bump is UNCONDITIONAL, not that it runs AFTER the commit — moving
    /// `self.gen_bumper.bump().await` above `tx.commit().await?` in `create` would leave that
    /// test, and all the others, green. `BumpSnapshotBumper` instead snapshots the
    /// `FakeUnitOfWork`'s own commit counter the instant `bump()` fires: if the bump ran before
    /// the commit, the snapshot reads the PRE-commit count (0), not the post-commit one (1).
    #[tokio::test]
    async fn the_post_commit_bump_runs_strictly_after_the_transaction_commits() {
        let uow = FakeUnitOfWork::default();
        let bumper = BumpSnapshotBumper::new(uow.clone());
        let svc = OrganizationService::new(OrganizationServiceDeps {
            repo: InMemoryOrgs::default(),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(bumper.clone()),
            policy_gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        svc.create(&PrincipalId::from_prn(principal_prn(1)), "acme", "Acme").await.unwrap();

        assert_eq!(bumper.calls(), 1);
        assert_eq!(bumper.snapshot_at_bump(), Some(1), "the commit counter must already read 1 (committed) at the instant bump() runs");
    }

    /// SMA-606 Risk 1: an event must never outlive a mutation that rolled back. Paired with
    /// `a_no_op_rename_emits_nothing_but_a_real_one_emits`: either test alone passes an
    /// implementation that emits nothing at all, or one that always emits.
    #[tokio::test]
    async fn a_failure_mid_transaction_leaves_no_event_and_no_entry() {
        let (svc, outbox, audit, _bumper, _uow) = service_with_failing_rename();
        let actor = PrincipalId::from_prn(principal_prn(1));

        let err = svc.rename(Uuid::from_u128(1), Some("acme2"), None, &actor).await.unwrap_err();

        assert_eq!(err, TenancyError::Internal, "RepositoryError::Backend from a mid-txn store failure maps to Internal");
        assert!(outbox.0.lock().unwrap().is_empty(), "the event must not survive a failed mutation");
        assert!(audit.0.lock().unwrap().is_empty(), "nor the audit entry");
    }
}
