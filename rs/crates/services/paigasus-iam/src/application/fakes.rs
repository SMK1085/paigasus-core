// SPDX-License-Identifier: Apache-2.0

//! Shared in-memory fakes for application-service tests (`#[cfg(test)]`-only, never
//! shipped). `TenancyStore` holds the tenancy state behind `Arc<Mutex<HashMap>>`s so the
//! per-port fakes — `InMemoryOrgs` here, plus `InMemoryTeams`/`InMemoryProjects`/
//! `InMemoryMemberships` added in later tasks — can each clone a handle onto the *same*
//! backing data: a team fake needs to see an org archived via the org fake to compute
//! effective status (D10), and `InMemoryOrgs::create` populates the shared team map with
//! the auto-provisioned default team (ADR-0014).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{
    Clock, ConflictKind, IdGenerator, NodeStatus, NodeView, Organization, OrganizationId, OrganizationRepository, PreconditionKind, PrincipalId, ProjectId, RepositoryError, Slug, Team, TeamId,
};
use paigasus_kernel::Prn;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Shared backing store for all tenancy in-memory fakes. Cloning is cheap (shares the
/// `Arc` innards), so e.g. a later `InMemoryTeams` fake sees the same org rows an
/// `InMemoryOrgs` fake mutates.
#[derive(Clone, Default)]
pub struct TenancyStore {
    pub orgs: Arc<Mutex<HashMap<Uuid, Organization>>>,
    pub teams: Arc<Mutex<HashMap<Uuid, Team>>>,
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
}
