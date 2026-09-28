// SPDX-License-Identifier: Apache-2.0

//! `TenancyNodes`: the stored-PRN check for a caller-supplied tenancy node (SMA-646).
//!
//! `ServiceAccountService::create`/`list` (the owner) and `ApiKeyService::issue` (the key's
//! scope) take a `TenancyNodeRef` that the transport parsed from the caller's PRN.
//! `TenancyNodeRef::from_prn` checks the shape only, not the org-slot VALUE and not the region.
//! So a caller can name a real node's uuid with a forged org slot or region. Before SMA-646,
//! these services echoed that forged PRN in their answers and in the `iam.api_key.issued`
//! payload, while a later read showed the stored PRN.
//!
//! [`TenancyNodes::resolve_and_authorize`] is the one place of the rule, in this order:
//!
//! 1. Load the node by uuid (`find`). An unknown uuid is `TenancyError::NotFound`.
//! 2. Authorize `action` against the STORED PRN, not the caller's. So the decision audit, the
//!    trace and the decision-cache key never carry a forged PRN.
//! 3. Compare the caller's canonical PRN with the stored canonical PRN. A difference writes one
//!    `tracing::warn!` and answers `TenancyError::PrnMismatch`.
//! 4. Return the STORED node, so the write and the answer use what the load read.
//!
//! **Authorize before compare is load-bearing** (SMA-645 §3): if the compare ran first, the
//! difference between a mismatch and `Forbidden` would tell an ungranted caller which
//! organization owns a node. **Load before authorize** costs one existence oracle (`NotFound`
//! against `Forbidden` for an ungranted caller). The tenancy surfaces accept the same cost
//! (`adapters/grpc/tenancy.rs`, `load_org_checked`), and Sven accepted it here (spec A4).
//!
//! **The compare is sound outside the write transaction** because of the F1 invariant (SMA-645
//! §3.1): a node's `prn` column is written once, from `canonical()`, with an empty region, and no
//! code moves a node to another parent. A future "move a node" feature breaks this and must
//! revisit this helper.
//!
//! **The warning line.** A refused request writes no row, no outbox event and no denial row, so
//! the log line is the only trace of a probe with a forged PRN (SMA-643 D4). Its fields are the
//! fields of `warn_prn_mismatch` in `adapters/grpc/tenancy.rs`. SMA-649 chose NO log line for its
//! PRINCIPAL guards, to match the node guard of the same RPC. Here the guarded value IS a tenancy
//! node, so the node-RPC rule applies.
//!
//! `RoleService::resolve_scope` is a similar lookup with the other order (authorize the caller's
//! PRN, then compare) and no log line. It stays unchanged (spec §2.1 D, §7 follow-up).

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::{Action, OrganizationRepository, ProjectRepository, TeamRepository, TenancyNodeRef};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// Read access to the three tenancy repositories, for the stored-PRN check. Only `find` is
/// used. No narrower read-only port exists, and `RoleService` takes the same three.
#[derive(Clone)]
pub struct TenancyNodes {
    pub orgs: Arc<dyn OrganizationRepository>,
    pub teams: Arc<dyn TeamRepository>,
    pub projects: Arc<dyn ProjectRepository>,
}

/// The PRN a tenancy node represents as an authorization resource.
fn node_prn(node: &TenancyNodeRef) -> &Prn {
    match node {
        TenancyNodeRef::Organization(id) => id.prn(),
        TenancyNodeRef::Team(id) => id.prn(),
        TenancyNodeRef::Project(id) => id.prn(),
    }
}

impl TenancyNodes {
    /// Loads `claimed`'s node, authorizes `action` for `actor` against the STORED PRN, refuses a
    /// caller PRN whose canonical form differs from the stored one, and returns the STORED node.
    /// `operation` is the RPC name for the warning line. Module docs give the order and why.
    // SMA-646 Task 1: no service calls this yet. Task 2 removes this attribute.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn resolve_and_authorize(&self, authorize: &Authorize, actor: &Prn, action: Action, claimed: &TenancyNodeRef, operation: &'static str) -> Result<TenancyNodeRef, TenancyError> {
        let stored = match claimed {
            TenancyNodeRef::Organization(id) => TenancyNodeRef::Organization(self.orgs.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
            TenancyNodeRef::Team(id) => TenancyNodeRef::Team(self.teams.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
            TenancyNodeRef::Project(id) => TenancyNodeRef::Project(self.projects.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
        };
        authorize.check(actor, action, node_prn(&stored)).await?;
        let requested = claimed.canonical();
        let stored_canonical = stored.canonical();
        if requested != stored_canonical {
            tracing::warn!(rpc = %operation, actor = %actor.canonical(), requested_prn = %requested, stored_prn = %stored_canonical, "refused a request: the request prn does not match the stored tenancy node");
            return Err(TenancyError::PrnMismatch);
        }
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuthorizer, TenancyStore, seed_org, seed_team, tenancy_nodes};
    use paigasus_iam_core::{NodeStatus, TeamId};
    use uuid::Uuid;

    fn actor() -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
    }

    /// The correct PRN resolves to the STORED node, authorized against the stored PRN.
    #[tokio::test]
    async fn a_correct_prn_returns_the_stored_node() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 10);
        let team = seed_team(&store, &org, 11);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, team.prn());
        let authorize = Authorize::new(Arc::new(fake));

        let got = tenancy_nodes(&store)
            .resolve_and_authorize(&authorize, &actor(), Action::CreateServiceAccount, &TenancyNodeRef::Team(team.clone()), "CreateServiceAccount")
            .await
            .unwrap();
        assert_eq!(got, TenancyNodeRef::Team(team));
    }

    /// Review Focus 1: the helper adds no status gate. An archived node resolves like an active
    /// one; the service keeps its own behaviour for it.
    #[tokio::test]
    async fn an_archived_node_still_resolves() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 20);
        store.orgs.lock().unwrap().get_mut(&org.uuid()).unwrap().status = NodeStatus::Archived;
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListServiceAccounts, org.prn());
        let authorize = Authorize::new(Arc::new(fake));

        let got = tenancy_nodes(&store)
            .resolve_and_authorize(&authorize, &actor(), Action::ListServiceAccounts, &TenancyNodeRef::Organization(org.clone()), "ListServiceAccounts")
            .await
            .unwrap();
        assert_eq!(got, TenancyNodeRef::Organization(org));
    }

    /// An unknown uuid is `NotFound` before any authorization (a default-deny authorizer).
    #[tokio::test]
    async fn an_unknown_node_is_not_found() {
        let store = TenancyStore::default();
        let authorize = Authorize::new(Arc::new(FakeAuthorizer::default()));
        let unknown = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(30), Uuid::from_u128(31)));

        let err = tenancy_nodes(&store)
            .resolve_and_authorize(&authorize, &actor(), Action::CreateServiceAccount, &unknown, "CreateServiceAccount")
            .await
            .unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
    }
}
