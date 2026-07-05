// SPDX-License-Identifier: Apache-2.0

//! `MembershipService`: attach/detach/list principal-to-tenancy-node memberships
//! (SMA-442, ADR-0014).

// Nothing in `main.rs` wires this into a route yet — the composition root (HTTP/gRPC
// handlers) lands in later tasks. Until then it's exercised only via the `#[cfg(test)]`
// fakes in `fakes.rs`; same reasoning as `OrganizationService`/`TeamService`/`ProjectService`.
#![allow(dead_code)]

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{Clock, IdGenerator, Membership, MembershipRecord, MembershipRepository, PrincipalId, TenancyNodeRef};
use paigasus_kernel::Prn;
use uuid::Uuid;

/// Which axis to filter a membership listing by — raw PRN strings from the wire, parsed
/// the same way `attach`'s arguments are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipFilter {
    Principal(String),
    Node(String),
}

/// Parses a raw principal PRN string: must be syntactically valid (else `InvalidPrn` with
/// the kernel's stable error-kind token), and must be service `"iam"`, resource type
/// `"principal"` (else `InvalidPrn` with the PRN's canonical form).
fn parse_principal_prn(raw: &str) -> Result<PrincipalId, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if prn.service() != "iam" || prn.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(prn.canonical()));
    }
    Ok(PrincipalId::from_prn(prn))
}

/// Parses a raw tenancy-node PRN string into a typed node ref (organization/team/project).
/// A `DomainError` from `TenancyNodeRef::from_prn` (wrong resource type, malformed org slot)
/// auto-converts into `TenancyError::InvalidPrn`.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Membership lifecycle use cases: attach a principal to a tenancy node, detach, list.
/// Generic-DI-by-value (`M`embership repository, `I`d generator, `C`lock) — no `Arc<dyn>`,
/// mirroring `OrganizationService`/`TeamService`/`ProjectService` (design doc §5).
#[derive(Clone)]
pub struct MembershipService<M, I, C> {
    repo: M,
    ids: I,
    clock: C,
}

impl<M, I, C> MembershipService<M, I, C>
where
    M: MembershipRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(repo: M, ids: I, clock: C) -> Self {
        Self { repo, ids, clock }
    }

    /// Attaches `principal_prn` to `node_prn`. This only parses/validates the wire PRNs and
    /// mints the membership id/timestamp — every existence/guard check (principal exists,
    /// node exists, prn byte-match, effectively-active, org-membership invariant,
    /// duplicate) happens in-txn in `repo.attach` (D8, port doc contract).
    pub async fn attach(&self, principal_prn: &str, node_prn: &str) -> Result<MembershipRecord, TenancyError> {
        let principal_id = parse_principal_prn(principal_prn)?;
        let node = parse_node_prn(node_prn)?;

        let id = self.ids.new_membership_id();
        let now = self.clock.now();
        let membership = Membership::new(id, principal_id, node, now);

        Ok(self.repo.attach(&membership).await?)
    }

    /// Detaches a membership by id. `NotFound` if missing. Detaching an org membership
    /// cascades: the repo also detaches the principal's team/project memberships scoped to
    /// that org, in one transaction (rule 5).
    pub async fn detach(&self, id: Uuid) -> Result<(), TenancyError> {
        Ok(self.repo.detach(id).await?)
    }

    /// Lists memberships by principal or node, `ORDER BY created_at, id` (design doc §5.1
    /// rule 9).
    pub async fn list(&self, filter: MembershipFilter, page: Page) -> Result<Vec<MembershipRecord>, TenancyError> {
        match filter {
            MembershipFilter::Principal(raw) => {
                let principal_id = parse_principal_prn(&raw)?;
                Ok(self.repo.list_by_principal(principal_id.uuid(), page.limit, page.offset).await?)
            }
            MembershipFilter::Node(raw) => {
                let node = parse_node_prn(&raw)?;
                Ok(self.repo.list_by_node(&node, page.limit, page.offset).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryMemberships, SeqIds, TenancyStore};
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{NodeStatus, Organization, OrganizationId, Project, ProjectId, Slug, Team, TeamId};

    fn new_service(store: TenancyStore) -> MembershipService<InMemoryMemberships, SeqIds, FixedClock> {
        MembershipService::new(InMemoryMemberships(store), SeqIds::default(), FixedClock::default())
    }

    /// Seeds a principal directly into the shared store's `principals` map (the
    /// canonical-prn record `InMemoryMemberships` checks caller prns against).
    fn seed_principal(store: &TenancyStore, uuid: u128) -> PrincipalId {
        let id = Uuid::from_u128(uuid);
        let principal_id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", id).unwrap());
        store.principals.lock().unwrap().insert(id, principal_id.canonical());
        principal_id
    }

    /// Seeds an org + a team under it directly into the shared store.
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
    async fn attach_happy_paths_and_invariant() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 1);
        let (org, team) = seed_org_and_team(&store, 100, 101, now);
        let svc = new_service(store.clone());

        let org_prn = OrganizationId::from_uuid(org).canonical();
        let team_prn = TeamId::from_parts(org, team).canonical();

        // Without an org membership, attaching to a team fails the invariant.
        assert_eq!(svc.attach(&principal.canonical(), &team_prn).await.unwrap_err(), TenancyError::MissingOrgMembership);

        // Attaching to the org itself succeeds and returns the org's canonical prn.
        let org_membership = svc.attach(&principal.canonical(), &org_prn).await.unwrap();
        assert_eq!(org_membership.node_prn, org_prn);
        assert_eq!(org_membership.principal_prn, principal.canonical());

        // Now that the org membership exists, the team attach succeeds.
        svc.attach(&principal.canonical(), &team_prn).await.unwrap();

        // A duplicate org attach is a conflict.
        assert_eq!(svc.attach(&principal.canonical(), &org_prn).await.unwrap_err(), TenancyError::DuplicateMembership);
    }

    #[tokio::test]
    async fn attach_rejects_bad_prns() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 10);
        let (org, team) = seed_org_and_team(&store, 200, 201, now);
        let svc = new_service(store.clone());
        let team_prn = TeamId::from_parts(org, team).canonical();

        // Not a PRN at all.
        assert!(matches!(svc.attach("not-a-prn", &team_prn).await.unwrap_err(), TenancyError::InvalidPrn(_)));

        // Well-formed PRN, but the wrong resource type for a principal.
        let user_prn = Prn::build("iam", "", None, "user", Uuid::from_u128(999)).unwrap().canonical();
        assert!(matches!(svc.attach(&user_prn, &team_prn).await.unwrap_err(), TenancyError::InvalidPrn(_)));

        // Forged node prn: the correct team uuid, but a different org uuid in the org slot.
        let wrong_org = Uuid::from_u128(9_999);
        let forged_team_prn = format!("prn:pgs:iam::{wrong_org}:team/{team}");
        assert_eq!(svc.attach(&principal.canonical(), &forged_team_prn).await.unwrap_err(), TenancyError::PrnMismatch);

        // Unknown principal (well-formed, but never seeded into the store).
        let unknown_principal = Prn::build("iam", "", None, "principal", Uuid::from_u128(12_345)).unwrap().canonical();
        assert_eq!(svc.attach(&unknown_principal, &team_prn).await.unwrap_err(), TenancyError::NotFound);

        // Archived team: satisfy the org-membership invariant first, then archive the team
        // directly in the store and confirm the effective-status guard still fires.
        svc.attach(&principal.canonical(), &OrganizationId::from_uuid(org).canonical()).await.unwrap();
        store.teams.lock().unwrap().get_mut(&team).unwrap().status = NodeStatus::Archived;
        assert_eq!(svc.attach(&principal.canonical(), &team_prn).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn detach_cascades_org_memberships() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 20);
        let (org, team) = seed_org_and_team(&store, 300, 301, now);

        let project_id = Uuid::from_u128(302);
        let project = Project::new(ProjectId::from_parts(org, project_id), TeamId::from_parts(org, team), Slug::parse("web").unwrap(), "Web", now).unwrap();
        store.projects.lock().unwrap().insert(project_id, project);

        let svc = new_service(store.clone());
        let org_prn = OrganizationId::from_uuid(org).canonical();
        let team_prn = TeamId::from_parts(org, team).canonical();
        let project_prn = ProjectId::from_parts(org, project_id).canonical();
        let page = Page::new(None, None).unwrap();

        let org_membership = svc.attach(&principal.canonical(), &org_prn).await.unwrap();
        svc.attach(&principal.canonical(), &team_prn).await.unwrap();
        svc.attach(&principal.canonical(), &project_prn).await.unwrap();
        assert_eq!(svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap().len(), 3);

        // Detaching the org membership cascades: the team and project memberships for the
        // same principal in that org go with it.
        svc.detach(org_membership.id).await.unwrap();
        assert!(svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap().is_empty());

        // Detaching an already-detached membership is `NotFound`.
        assert_eq!(svc.detach(org_membership.id).await.unwrap_err(), TenancyError::NotFound);

        // A team-only detach removes only itself, leaving the org membership intact.
        let org_membership2 = svc.attach(&principal.canonical(), &org_prn).await.unwrap();
        let team_membership2 = svc.attach(&principal.canonical(), &team_prn).await.unwrap();
        svc.detach(team_membership2.id).await.unwrap();
        let remaining = svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, org_membership2.id);
    }
}
