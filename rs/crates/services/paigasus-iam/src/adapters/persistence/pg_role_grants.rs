// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `RoleGrantStore` (SeaORM). `grant` persists exactly the caller-built
//! `RoleGrant` — including its `linked_policy_id` (the Cedar template-linked policy itself
//! is materialized from grant rows at snapshot-compile time, Task 12, so this store never
//! touches `policy`/`role` rows) — then bumps `policy_gen` via the shared `Generations`
//! handle (spec §7/D11: "bumped on any policy CRUD or role grant/revoke"). `revoke` mirrors
//! `PgPolicyStore::delete`'s idempotent-DELETE posture: a missing id is a no-op success (no
//! `AuthzError::NotFound` variant exists — see `authz::model::AuthzError`) and only a row
//! that actually existed bumps the generation.

use super::entities::role_grant;
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use paigasus_iam_core::{AuthzError, GrantScope, PrincipalId, RoleGrant, RoleGrantStore, TenancyNodeRef};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set};
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
}

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
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

#[async_trait]
impl RoleGrantStore for PgRoleGrantStore {
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError> {
        // A `uq_role_grant_principal_role_scope` (duplicate principal+role+scope) or
        // `uq_role_grant_linked_policy` (duplicate linked_policy_id) violation surfaces here
        // as `AuthzError::Backend` wrapping the SeaORM/Postgres error — never silently
        // swallowed, and no row is written.
        grant_to_model(g).insert(&self.db).await.map_err(map_err)?;
        self.gens.bump_policy_gen().await?;
        Ok(())
    }

    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError> {
        let result = role_grant::Entity::delete_by_id(id).exec(&self.db).await.map_err(map_err)?;
        // Idempotent: revoking an id that was never granted (or already revoked) is a no-op
        // success, mirroring `PgPolicyStore::delete`'s posture (no `NotFound` variant exists
        // on `AuthzError`) — and only a row that actually existed bumps the generation.
        if result.rows_affected > 0 {
            self.gens.bump_policy_gen().await?;
        }
        Ok(())
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
