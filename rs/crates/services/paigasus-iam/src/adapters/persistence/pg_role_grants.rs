// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `RoleGrantStore` (SeaORM). `grant`/`revoke` are thin one-shot-`UnitOfWork`
//! wrappers around [`RoleGrantStore::grant_in`]/[`RoleGrantStore::revoke_in`] (SMA-446, Slice
//! B): they open their own `SeaOrmTransaction`, run the txn-scoped insert/delete on it,
//! commit, then best-effort bump `policy_gen` via the shared `Generations` handle (spec
//! §7/D11: "bumped on any policy CRUD or role grant/revoke"). `BootstrapAdminSeeder` was the
//! last production caller of `grant`; since SMA-468 it drives its own `UnitOfWork` instead —
//! its grant, outbox event and audit row commit together on one transaction via `grant_in`,
//! not `grant` — so a seed failure has a single diagnosable path rather than the wrapper's
//! own bump-then-forget one. As of SMA-468, `grant`/`revoke` therefore have **zero production
//! callers**; every remaining call site is test/integration-only (`authz_role_grants.rs`,
//! `authz_bootstrap.rs`, `tests/support/mod.rs`, and `#[cfg(test)]` fixtures in
//! `cedar_authorizer.rs`/`policy_snapshot.rs`). `grant_in`/`revoke_in` are the txn-scoped
//! primitives `RoleService::grant`/`revoke` (the reference pattern) actually drive: they
//! persist exactly the caller-built `RoleGrant` — including its `linked_policy_id` (the
//! Cedar template-linked policy itself is materialized from grant rows at snapshot-compile
//! time, Task 12, so this store never touches `policy`/`role` rows) — on the caller's own
//! transaction, and deliberately never bump `policy_gen` themselves; the bump is the
//! caller's own awaited, post-commit responsibility (`application::roles::RoleService` via
//! `PolicyGenBumper`). The generation bump itself is logged and swallowed on error,
//! mirroring `pg_organizations.rs::bump_entity_gen`: the write already committed, so a
//! Redis-down bump failure must never fail it, it just means the change lands on the
//! snapshot's TTL backstop (`policy_cache_ttl_secs + refresh_interval_secs`) instead of
//! immediately — the decision cache follows for free, since its key's policy component is the
//! compiled set's `content_hash` (SMA-470 D4), which rotates the moment that reload installs.
//! `revoke`/`revoke_in` mirror
//! `PgPolicyStore::delete`'s idempotent-DELETE posture: a missing id is a no-op success (no
//! `AuthzError::NotFound` variant exists — see `authz::model::AuthzError`) and only a row
//! that actually existed bumps the generation (`revoke`) or is reported back to the caller
//! (`revoke_in`'s `bool`).

use super::entities::role_grant;
use super::uow::{SeaOrmTransaction, recover_txn};
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use paigasus_iam_core::{AuthzError, GrantScope, PrincipalId, RepositoryError, RoleGrant, RoleGrantStore, TenancyNodeRef, Transaction};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set, SqlErr, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root hold a store handle inside a `#[derive(Clone)]` service
// (mirrors `PgPolicyStore`'s precedent) — cheap: `DatabaseConnection` clones an `Arc`-backed
// pool handle, and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgRoleGrantStore {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgRoleGrantStore {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgRoleGrantStore { db, gens }
    }

    /// Best-effort `policy_gen` bump (spec §7/D11): logged and swallowed on error — mirrors
    /// `pg_organizations.rs::bump_entity_gen`/`bump_policy_gen` exactly: `grant`/`revoke`'s
    /// mutation already committed, so a Redis-down bump failure must never fail an
    /// already-successful write; it just means the change lands on the policy snapshot's TTL
    /// backstop — `policy_cache_ttl_secs + refresh_interval_secs`, NOT the decision cache's own
    /// TTL — instead of immediately (D11: a swallowed bump degrades to backstop-bounded
    /// staleness). The decision cache needs no expiry of its own to follow: its key's policy
    /// component is the compiled set's `content_hash` (SMA-470 D4), so that reload rotates the
    /// key space for it — which is also why the claim has to be phrased this way, since
    /// `MemoryDecisionCache` has no TTL at all.
    async fn bump_policy_gen_best_effort(&self) {
        if let Err(err) = self.gens.bump_policy_gen().await {
            tracing::warn!(error = %err, "pg_role_grants: policy_gen bump failed after a committed write — authz decisions may be stale until the policy snapshot's TTL backstop reloads");
        }
    }
}

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// `grant_in`'s error mapping. The private [`map_err`] above collapses every `DbErr` into
/// `Backend`, which is right for the reads and deletes around it but wrong here: a violation of
/// `fk_role_grant_role` means the `role` row this grant names does not exist, which is exactly
/// what `RoleService::grant` reports as `UnknownRole` before it ever reaches the database.
///
/// This is not a theoretical branch. SMA-481 D6: a retirement holds the role row `FOR UPDATE`
/// while a concurrent grant from a replica on an OLDER binary — one whose code catalog still
/// defines the retired role — blocks behind it. When the retirement commits with the row
/// deleted, that grant resumes, re-runs its FK check and fails. Without this mapping the caller
/// gets a `500 internal error` for a condition the service understands perfectly well.
fn map_grant_err(e: DbErr, role_key: &str) -> AuthzError {
    match e.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(_)) => AuthzError::UnknownRole(role_key.to_string()),
        _ => map_err(e),
    }
}

/// A stored-error helper for a corrupt/unparseable `scope_node_prn` or `principal_id` — a
/// data-integrity break (the row was written by this same adapter), surfaced as `Backend`,
/// never a silent default (mirrors `pg_memberships.rs::parse_status`'s posture).
fn backend_err(msg: impl std::fmt::Display) -> AuthzError {
    AuthzError::Backend(Box::new(std::io::Error::other(msg.to_string())))
}

/// Splits a [`GrantScope`] into the row's `scope_kind` string and its (at most one non-null)
/// `scope_org_id`/`scope_team_id`/`scope_project_id` columns — the inverse of
/// [`model_to_grant`]'s scope reconstruction. `GrantScope::Root` maps to all three NULL
/// (`ck_role_grant_scope`'s `root` arm).
fn scope_columns(scope: &GrantScope) -> (&'static str, Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match scope {
        GrantScope::Root => ("root", None, None, None),
        GrantScope::Node(TenancyNodeRef::Organization(id)) => ("organization", Some(id.uuid()), None, None),
        GrantScope::Node(TenancyNodeRef::Team(id)) => ("team", None, Some(id.uuid()), None),
        GrantScope::Node(TenancyNodeRef::Project(id)) => ("project", None, None, Some(id.uuid())),
    }
}

/// Builds the insertable `role_grant` row from a domain `RoleGrant` — every field is taken
/// as-is from the caller (the store never recomputes `linked_policy_id`; the use-case layer
/// sets it to `format!("grant:{}", g.id)`, mirroring `authz::engine::link_grant`).
fn grant_to_model(g: &RoleGrant) -> role_grant::ActiveModel {
    let (scope_kind, scope_org_id, scope_team_id, scope_project_id) = scope_columns(&g.scope);
    role_grant::ActiveModel {
        id: Set(g.id),
        principal_id: Set(g.principal.uuid()),
        role_key: Set(g.role_key.clone()),
        scope_kind: Set(scope_kind.to_string()),
        scope_node_prn: Set(g.scope.canonical_prn()),
        scope_org_id: Set(scope_org_id),
        scope_team_id: Set(scope_team_id),
        scope_project_id: Set(scope_project_id),
        linked_policy_id: Set(g.linked_policy_id.clone()),
        created_at: Set(g.created_at),
    }
}

/// Reconstructs the domain `RoleGrant` from a stored row: `scope_kind = 'root'` rebuilds
/// [`GrantScope::Root`] directly (there is no tenancy row for the synthetic Root sentinel);
/// otherwise `scope_node_prn` — the node's own canonical PRN, already carrying its org
/// context — round-trips through `Prn::parse` + `TenancyNodeRef::from_prn`, so the
/// `scope_*_id` FK columns (query/index-only) are never needed for this reconstruction.
/// `principal_id` is a bare uuid with no stored PRN of its own — a principal's PRN shape is
/// fully deterministic (`iam`, no org, `principal`, the uuid), so it's synthesized directly
/// rather than joining back to the `principal` table (mirrors
/// `application/memberships.rs`'s/`authenticate_token.rs`'s precedent).
fn model_to_grant(m: role_grant::Model) -> Result<RoleGrant, AuthzError> {
    let scope = if m.scope_kind == "root" {
        GrantScope::Root
    } else {
        let prn = Prn::parse(&m.scope_node_prn).map_err(backend_err)?;
        let node = TenancyNodeRef::from_prn(prn).map_err(backend_err)?;
        GrantScope::Node(node)
    };
    let principal_prn = Prn::build("iam", "", None, "principal", m.principal_id).map_err(backend_err)?;
    Ok(RoleGrant {
        id: m.id,
        principal: PrincipalId::from_prn(principal_prn),
        role_key: m.role_key,
        scope,
        linked_policy_id: m.linked_policy_id,
        created_at: m.created_at,
    })
}

/// Maps a [`recover_txn`] failure (an opaque `&dyn Transaction` that isn't a
/// `SeaOrmTransaction` — never happens in production, only a misbuilt fake could trigger it)
/// into `AuthzError::Backend`, mirroring [`map_err`]'s posture for row-level failures.
fn map_txn_err(e: RepositoryError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

#[async_trait]
impl RoleGrantStore for PgRoleGrantStore {
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError> {
        // A thin one-shot-`UnitOfWork` wrapper (module docs): open a `SeaOrmTransaction`,
        // insert via `grant_in`, commit, then bump — the exact behavior this method had
        // before Slice B, just re-expressed over the txn-scoped primitive so the insert
        // logic lives in exactly one place.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        // A `uq_role_grant_principal_role_scope` (duplicate principal+role+scope) or
        // `uq_role_grant_linked_policy` (duplicate linked_policy_id) violation surfaces here
        // as `AuthzError::Backend` wrapping the SeaORM/Postgres error — never silently
        // swallowed; dropping `tx` without committing rolls the failed insert back, and no
        // row is written.
        self.grant_in(&*tx, g).await?;
        tx.commit().await.map_err(map_txn_err)?;
        self.bump_policy_gen_best_effort().await;
        Ok(())
    }

    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError> {
        // Thin one-shot-`UnitOfWork` wrapper, mirroring `grant`'s above.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let existed = self.revoke_in(&*tx, id).await?;
        tx.commit().await.map_err(map_txn_err)?;
        // Idempotent: revoking an id that was never granted (or already revoked) is a no-op
        // success, mirroring `PgPolicyStore::delete`'s posture (no `NotFound` variant exists
        // on `AuthzError`) — and only a row that actually existed bumps the generation.
        if existed {
            self.bump_policy_gen_best_effort().await;
        }
        Ok(())
    }

    async fn grant_in(&self, tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        grant_to_model(g).insert(txn).await.map_err(|e| map_grant_err(e, &g.role_key))?;
        Ok(())
    }

    async fn revoke_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<bool, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let result = role_grant::Entity::delete_by_id(id).exec(txn).await.map_err(map_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Entity::find().all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }

    async fn list_by_principal(&self, p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Entity::find().filter(role_grant::Column::PrincipalId.eq(p.uuid())).all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }

    async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
        let Some(model) = role_grant::Entity::find_by_id(id).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        Ok(Some(model_to_grant(model)?))
    }
}
