// SPDX-License-Identifier: Apache-2.0

//! Hexagonal ports (traits) for authorization (ADR-0013): the pure core depends on these
//! abstractions, not on the eventual Postgres/cache/audit adapters (ADR-0005). Later M3
//! tasks provide the service-crate implementations.

use super::model::{AccessRequest, AuthzDecisionEvent, AuthzError, Decision, EntitySlice, PolicyDocument, RoleGrant};
use crate::ports::Transaction;
use crate::value::PrincipalId;
use async_trait::async_trait;
use paigasus_kernel::Prn;
use uuid::Uuid;

/// Decides one [`AccessRequest`] and returns its [`Decision`]. The service-facing entry
/// point that wraps the pure `authz::engine::PolicyEngine` with whatever policy/entity
/// loading and caching an adapter needs.
#[async_trait]
pub trait Authorizer: Send + Sync {
    async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError>;
}

/// Persistence port for authored Cedar policies/templates ([`PolicyDocument`]).
#[async_trait]
pub trait PolicyStore: Send + Sync {
    /// One repeatable-read txn (impl detail) — every document is a consistent snapshot.
    async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError>;
    /// Validates; rejects system-owned documents.
    async fn put(&self, doc: &PolicyDocument) -> Result<(), AuthzError>;
    /// Rejects system-owned documents.
    async fn delete(&self, policy_id: &str) -> Result<(), AuthzError>;
    async fn policy_gen(&self) -> Result<u64, AuthzError>;
    async fn bump_policy_gen(&self) -> Result<u64, AuthzError>;
}

/// Persistence port for materialized [`RoleGrant`]s.
#[async_trait]
pub trait RoleGrantStore: Send + Sync {
    /// Inserts the grant row and bumps the policy generation counter.
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError>;
    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError>;
    /// Txn-scoped twin of [`RoleGrantStore::grant`] (SMA-446, Slice B — the
    /// `RoleService::grant` reference pattern): inserts the row on the caller's own `tx`,
    /// deliberately WITHOUT bumping the policy generation counter — the caller bumps it
    /// itself, once, as an awaited POST-COMMIT step via `PolicyGenBumper`, mirroring
    /// `Outbox::enqueue`/`AuditLog::record`'s in-txn posture.
    async fn grant_in(&self, tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError>;
    /// Txn-scoped twin of [`RoleGrantStore::revoke`]: deletes the row on the caller's own
    /// `tx`, returning whether a row actually existed to delete — the caller only
    /// enqueues/records/bumps when this is `true`; `false` is an idempotent no-op (mirrors
    /// `revoke`'s own idempotent-DELETE posture). Deliberately never bumps the generation
    /// counter itself, same reasoning as [`RoleGrantStore::grant_in`].
    async fn revoke_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<bool, AuthzError>;
    async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError>;
    async fn list_by_principal(&self, p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError>;
    /// Looks up a single grant by id — `None` if it was never granted or has since been
    /// revoked. `RoleService::revoke` (SMA-444 Task 17) uses this to resolve the grant's
    /// `GrantScope` before authorizing the revoke itself against it.
    async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError>;
}

/// Loads the minimal [`EntitySlice`] needed to decide one request (principal, resource,
/// ancestor chain, synthetic `Root`).
#[async_trait]
pub trait EntitySliceLoader: Send + Sync {
    async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError>;
    async fn entity_gen(&self) -> Result<u64, AuthzError>;
}

/// Caches previously-computed [`Decision`]s, keyed by the adapter's own cache-key scheme
/// (e.g. request + policy/entity generation).
#[async_trait]
pub trait DecisionCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<Decision>;
    async fn put(&self, key: &str, decision: &Decision);
}

/// Records an [`AuthzDecisionEvent`] for audit.
#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn record(&self, ev: &AuthzDecisionEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof the authz ports are object-safe (injected as trait objects).
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn Authorizer, _: &dyn PolicyStore, _: &dyn RoleGrantStore, _: &dyn EntitySliceLoader, _: &dyn DecisionCache, _: &dyn AuditSink) {}
}
