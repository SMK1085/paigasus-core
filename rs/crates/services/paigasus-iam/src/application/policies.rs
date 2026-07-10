// SPDX-License-Identifier: Apache-2.0

//! `PolicyService`: authored Cedar policy/template CRUD use cases (SMA-444 Task 17,
//! ADR-0013). Every operation is platform-scoped — `PutPolicy`/`DeletePolicy`/`ListPolicies`
//! are Root-only actions (design §3.2: only `platform_admin` holds them), so every method
//! authorizes against `root_prn()` before ever touching the store. The store itself
//! (`PolicyStore::put`/`delete`) separately rejects mutation of an already-persisted
//! `system = true` row (`AuthzError::SystemImmutable` -> `TenancyError::SystemImmutable`) —
//! this service does not duplicate that check.

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, PolicyDocument, PolicyStore};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// Policy/template CRUD use cases. `policies` is `Arc<dyn PolicyStore>` — the same shared
/// handle `AppState` composes into `PolicySnapshot`, so a later task's wiring clones one
/// `Arc` rather than standing up a second store instance (mirrors `RoleService`'s posture).
#[derive(Clone)]
pub struct PolicyService {
    policies: Arc<dyn PolicyStore>,
    authorize: Authorize,
}

impl PolicyService {
    #[must_use]
    pub fn new(policies: Arc<dyn PolicyStore>, authorize: Authorize) -> Self {
        Self { policies, authorize }
    }

    /// Authorizes `actor` for `Action::PutPolicy` at `root_prn()`, then persists `doc`. The
    /// store validates the Cedar source and rejects editing an existing system row.
    pub async fn put(&self, actor: &Prn, doc: PolicyDocument) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::PutPolicy, &root_prn()).await?;
        self.policies.put(&doc).await?;
        Ok(())
    }

    /// Authorizes `actor` for `Action::DeletePolicy` at `root_prn()`, then deletes
    /// `policy_id`. The store rejects deleting an existing system row; deleting an id that
    /// never existed is an idempotent no-op (store contract).
    pub async fn delete(&self, actor: &Prn, policy_id: &str) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::DeletePolicy, &root_prn()).await?;
        self.policies.delete(policy_id).await?;
        Ok(())
    }

    /// Authorizes `actor` for `Action::ListPolicies` at `root_prn()`, then lists every
    /// persisted policy/template, paginated in-service (the store has no `limit`/`offset` of
    /// its own — `list_all` always returns the full, consistent snapshot).
    pub async fn list(&self, actor: &Prn, limit: u64, offset: u64) -> Result<Vec<PolicyDocument>, TenancyError> {
        self.authorize.check(actor, Action::ListPolicies, &root_prn()).await?;
        let mut docs = self.policies.list_all().await?;
        docs.sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
        Ok(docs.into_iter().skip(offset as usize).take(limit as usize).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuthorizer, InMemoryPolicies};
    use chrono::Utc;
    use paigasus_iam_core::authz::model::PolicyKind;
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn doc(policy_id: &str) -> PolicyDocument {
        let now = Utc::now();
        PolicyDocument {
            policy_id: policy_id.to_string(),
            kind: PolicyKind::Static,
            source: r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string(),
            description: "test policy".to_string(),
            system: false,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn put_denies_an_unauthorized_actor() {
        let svc = PolicyService::new(Arc::new(InMemoryPolicies::default()), Authorize::new(Arc::new(FakeAuthorizer::default())));
        let err = svc.put(&actor_prn(1), doc("p1")).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn put_succeeds_for_an_authorized_actor_and_list_returns_it() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::PutPolicy, &root_prn());
        fake.allow(Action::ListPolicies, &root_prn());
        let svc = PolicyService::new(Arc::new(InMemoryPolicies::default()), Authorize::new(Arc::new(fake)));
        let actor = actor_prn(1);

        svc.put(&actor, doc("p1")).await.unwrap();
        let listed = svc.list(&actor, 50, 0).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].policy_id, "p1");
    }

    #[tokio::test]
    async fn delete_denies_an_unauthorized_actor() {
        let svc = PolicyService::new(Arc::new(InMemoryPolicies::default()), Authorize::new(Arc::new(FakeAuthorizer::default())));
        assert_eq!(svc.delete(&actor_prn(1), "p1").await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn list_denies_an_unauthorized_actor() {
        let svc = PolicyService::new(Arc::new(InMemoryPolicies::default()), Authorize::new(Arc::new(FakeAuthorizer::default())));
        assert_eq!(svc.list(&actor_prn(1), 50, 0).await.unwrap_err(), TenancyError::Forbidden);
    }
}
