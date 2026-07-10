// SPDX-License-Identifier: Apache-2.0

//! `OrganizationService`: organization lifecycle (create with an auto-provisioned
//! default team, get, list, rename, archive, restore) — ADR-0014.

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{Clock, GrantScope, IdGenerator, NodeStatus, NodeView, Organization, OrganizationRepository, PrincipalId, RoleGrant, Slug, Team, TenancyNodeRef};
use uuid::Uuid;

/// Output of [`OrganizationService::create`]: the new org plus its auto-provisioned
/// `"default"` team, created together in one repository transaction (ADR-0014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrgOutput {
    pub organization: Organization,
    pub default_team: Team,
}

/// Organization lifecycle use cases. Generic-DI-by-value (`R`epository, `I`d generator,
/// `C`lock) — no `Arc<dyn>`, mirroring M0's `CreateUser` (per-aggregate grouping, design
/// doc §5).
#[derive(Clone)]
pub struct OrganizationService<R, I, C> {
    repo: R,
    ids: I,
    clock: C,
}

impl<R, I, C> OrganizationService<R, I, C>
where
    R: OrganizationRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(repo: R, ids: I, clock: C) -> Self {
        Self { repo, ids, clock }
    }

    /// Creates an organization, its auto-provisioned `"default"` team, and an `org_admin`
    /// owner grant for `actor` scoped to the new org, all in one repository transaction
    /// (ADR-0014, spec D8) — the creating principal becomes the owner of what it creates.
    pub async fn create(&self, actor: &PrincipalId, slug: &str, name: &str) -> Result<CreateOrgOutput, TenancyError> {
        let slug = Slug::parse(slug)?;
        let now = self.clock.now();

        let org_id = self.ids.new_organization_id();
        let organization = Organization::new(org_id, slug, name, now)?;

        let team_id = self.ids.new_team_id(organization.id.uuid());
        let default_slug = Slug::parse("default").expect("\"default\" is a valid slug");
        let default_team = Team::new(team_id, default_slug, "Default", now)?;

        let grant_id = self.ids.new_membership_id();
        let owner_grant = RoleGrant {
            id: grant_id,
            principal: actor.clone(),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Node(TenancyNodeRef::Organization(organization.id.clone())),
            linked_policy_id: format!("grant:{grant_id}"),
            created_at: now,
        };

        self.repo.create(&organization, &default_team, &owner_grant).await?;
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
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>) -> Result<NodeView<Organization>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let now = self.clock.now();
        Ok(self.repo.rename(id, slug.as_ref(), new_name, now).await?)
    }

    /// Sets the org's own status to `Archived`. Idempotent: a no-op leaves `updated_at`
    /// untouched if already archived (D10).
    pub async fn archive(&self, id: Uuid) -> Result<NodeView<Organization>, TenancyError> {
        let now = self.clock.now();
        Ok(self.repo.set_status(id, NodeStatus::Archived, now).await?)
    }

    /// Sets the org's own status to `Active`. Idempotent, mirroring `archive`.
    pub async fn restore(&self, id: Uuid) -> Result<NodeView<Organization>, TenancyError> {
        let now = self.clock.now();
        Ok(self.repo.set_status(id, NodeStatus::Active, now).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryOrgs, SeqIds};
    use chrono::{Duration, TimeZone, Utc};

    fn new_service() -> OrganizationService<InMemoryOrgs, SeqIds, FixedClock> {
        OrganizationService::new(InMemoryOrgs::default(), SeqIds::default(), FixedClock::default())
    }

    /// A deterministic `PrincipalId` for `create`'s `actor` argument — the tests below don't
    /// exercise authorization (that's `adapters::http`/`grpc`'s job), just that `create`
    /// threads whatever actor it's given into the owner grant (see `fakes.rs`'s
    /// `InMemoryOrgs` recording it).
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
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
        let svc = OrganizationService::new(InMemoryOrgs::default(), SeqIds::default(), clock.clone());

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();
        assert_eq!(created.organization.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.effective_status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived org is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id).await.unwrap();
        assert_eq!(archived_again.node.status, NodeStatus::Archived);
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.effective_status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);

        // Restoring an already-active org is a no-op: updated_at does not advance.
        let t4 = t3 + Duration::seconds(10);
        clock.set(t4);
        let restored_again = svc.restore(id).await.unwrap();
        assert_eq!(restored_again.node.status, NodeStatus::Active);
        assert_eq!(restored_again.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_archived_node() {
        let svc = new_service();
        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        assert_eq!(svc.rename(id, None, None).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let svc = new_service();
        assert_eq!(svc.get(Uuid::from_u128(999)).await.unwrap_err(), TenancyError::NotFound);
    }
}
