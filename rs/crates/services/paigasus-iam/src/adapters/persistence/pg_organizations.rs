// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `OrganizationRepository` (SeaORM). Maps domain <-> entity models and
//! backend errors into the core's `RepositoryError`. Every successful `create`/`rename`/
//! `set_status` bumps `entity_gen` via the shared `Generations` handle (spec §7/D11: "bumped
//! on any tenancy mutation that can change a slice or `effective_status`") — best-effort: a
//! bump failure is logged and swallowed, never surfaced, since the tenancy write already
//! committed and must not be rolled back or reported as failed over a cache-invalidation
//! hiccup (SMA-444 Task 15). `create` ALSO inserts the caller-built `owner_grant`'s
//! `role_grant` row in the SAME transaction as the org + default team (ADR-0014, spec D8:
//! the creating principal becomes the new org's `org_admin` owner, atomically) and — since a
//! grant is a policy change — bumps `policy_gen` too, on top of the usual `entity_gen` bump
//! (SMA-444 Task 20b).

use super::entities::{organization, role_grant, team};
use super::map_err;
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{GrantScope, NodeStatus, NodeView, Organization, OrganizationId, OrganizationRepository, PreconditionKind, RepositoryError, RoleGrant, Slug, Team, TenancyNodeRef};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryOrder, QuerySelect, Set, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root (`http::AppState::new`) hold a repo handle inside a
// `#[derive(Clone)] OrganizationService` (generic-DI-by-value, mirroring `CreateUser`'s
// Task 6 precedent) — cheap, since `DatabaseConnection` clones an `Arc`-backed pool handle,
// not a connection, and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgOrganizationRepository {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgOrganizationRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgOrganizationRepository { db, gens }
    }

    /// Best-effort `entity_gen` bump (spec §7/D11): logged and swallowed on error — a failed
    /// cache-invalidation bump must never fail an already-committed tenancy write, it just
    /// means the decision/slice caches self-heal on their next TTL expiry instead of
    /// immediately.
    async fn bump_entity_gen(&self) {
        if let Err(err) = self.gens.bump_entity_gen().await {
            tracing::warn!(error = %err, "pg_organizations: entity_gen bump failed after a committed write — authz caches may serve stale data until TTL");
        }
    }

    /// Best-effort `policy_gen` bump (spec §7/D11): `create`'s owner grant is a policy
    /// change, so the decision cache must invalidate on it too — same swallow-and-log
    /// posture as [`Self::bump_entity_gen`], for the same reason (the write already
    /// committed; a cache-invalidation hiccup must never be reported as a failed create).
    async fn bump_policy_gen(&self) {
        if let Err(err) = self.gens.bump_policy_gen().await {
            tracing::warn!(error = %err, "pg_organizations: policy_gen bump failed after a committed write — authz caches may serve stale data until TTL");
        }
    }
}

/// Builds the insertable `organization` row from a domain `Organization`.
fn org_to_model(org: &Organization) -> organization::ActiveModel {
    organization::ActiveModel {
        id: Set(org.id.uuid()),
        prn: Set(org.id.canonical()),
        slug: Set(org.slug.as_str().to_string()),
        name: Set(org.name.clone()),
        status: Set(org.status.as_str().to_string()),
        created_at: Set(org.created_at),
        updated_at: Set(org.updated_at),
    }
}

/// Builds the insertable `team` row for the org's auto-provisioned default team (ADR-0014).
fn team_to_model(team: &Team) -> team::ActiveModel {
    team::ActiveModel {
        id: Set(team.id.uuid()),
        org_id: Set(team.id.org_uuid()),
        prn: Set(team.id.canonical()),
        slug: Set(team.slug.as_str().to_string()),
        name: Set(team.name.clone()),
        status: Set(team.status.as_str().to_string()),
        created_at: Set(team.created_at),
        updated_at: Set(team.updated_at),
    }
}

/// Splits a [`GrantScope`] into the row's `scope_kind` string and its (at most one non-null)
/// `scope_org_id`/`scope_team_id`/`scope_project_id` columns — mirrors
/// `pg_role_grants.rs::scope_columns` exactly (duplicated rather than shared: that helper is
/// private to its own module, and `create`'s owner grant is always `GrantScope::Node
/// (TenancyNodeRef::Organization(..))` in practice, so this stays a thin, self-contained
/// mirror rather than a cross-module visibility change).
fn owner_grant_scope_columns(scope: &GrantScope) -> (&'static str, Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match scope {
        GrantScope::Root => ("root", None, None, None),
        GrantScope::Node(TenancyNodeRef::Organization(id)) => ("organization", Some(id.uuid()), None, None),
        GrantScope::Node(TenancyNodeRef::Team(id)) => ("team", None, Some(id.uuid()), None),
        GrantScope::Node(TenancyNodeRef::Project(id)) => ("project", None, None, Some(id.uuid())),
    }
}

/// Builds the insertable `role_grant` row for `create`'s `owner_grant` argument — every field
/// taken as-is from the caller, mirroring `pg_role_grants.rs::grant_to_model`.
fn owner_grant_to_model(g: &RoleGrant) -> role_grant::ActiveModel {
    let (scope_kind, scope_org_id, scope_team_id, scope_project_id) = owner_grant_scope_columns(&g.scope);
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

/// Re-parses a stored `organization` row back into the pure-core `Organization`, mirroring
/// M0's `find_user`: a parse failure on stored data becomes a `Backend` error (never a
/// silent default) — the row was written by this same adapter, so a failure here means the
/// data is corrupt or the domain's parsing rules changed underneath it.
fn model_to_org(model: organization::Model) -> Result<Organization, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));

    let prn = Prn::parse(&model.prn).map_err(|e| backend(e.to_string()))?;
    let id = OrganizationId::from_prn(prn).map_err(|e| backend(e.to_string()))?;
    let slug = Slug::parse(&model.slug).map_err(|e| backend(e.to_string()))?;
    let status = NodeStatus::parse(&model.status).ok_or_else(|| backend(format!("bad node status: {}", model.status)))?;

    Ok(Organization {
        id,
        slug,
        name: model.name,
        status,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

/// Orgs have no ancestors: effective status is own status alone (D1/D10), but still routed
/// through the shared `NodeStatus::effective` rule rather than hand-rolled.
fn org_view(org: Organization) -> NodeView<Organization> {
    let effective_status = NodeStatus::effective(org.status, &[]);
    NodeView { node: org, effective_status }
}

#[async_trait]
impl OrganizationRepository for PgOrganizationRepository {
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant) -> Result<(), RepositoryError> {
        // Org + its auto-provisioned default team + the creating principal's owner grant
        // must commit-or-rollback together (ADR-0014, spec D8): a lone org row without its
        // default team would violate the tenancy invariant that every org has at least one
        // team, and an org without its owner grant would leave nobody authorized to manage
        // what they just created.
        let txn = self.db.begin().await.map_err(map_err)?;

        org_to_model(org).insert(&txn).await.map_err(map_err)?;
        team_to_model(default_team).insert(&txn).await.map_err(map_err)?;
        owner_grant_to_model(owner_grant).insert(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        // `entity_gen` for the tenancy write (org + default team), `policy_gen` for the
        // owner grant (a policy change) — both counters move, spec §7/D11.
        self.bump_entity_gen().await;
        self.bump_policy_gen().await;
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Organization>>, RepositoryError> {
        let Some(model) = organization::Entity::find_by_id(id).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        Ok(Some(org_view(model_to_org(model)?)))
    }

    async fn list(&self, limit: u64, offset: u64) -> Result<Vec<NodeView<Organization>>, RepositoryError> {
        let models = organization::Entity::find()
            .order_by_asc(organization::Column::CreatedAt)
            .order_by_asc(organization::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        models.into_iter().map(|m| model_to_org(m).map(org_view)).collect()
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = organization::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        if model.status == NodeStatus::Archived.as_str() {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }

        let mut active = model.into_active_model();
        if let Some(slug) = new_slug {
            active.slug = Set(slug.as_str().to_string());
        }
        if let Some(name) = new_name {
            active.name = Set(name.to_owned());
        }
        active.updated_at = Set(now);
        let updated = active.update(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        self.bump_entity_gen().await;
        Ok(org_view(model_to_org(updated)?))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Organization>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = organization::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        let final_model = if model.status == status.as_str() {
            // Idempotent: already at the target status — no-op, `updated_at` untouched.
            model
        } else {
            let mut active = model.into_active_model();
            active.status = Set(status.as_str().to_owned());
            active.updated_at = Set(now);
            active.update(&txn).await.map_err(map_err)?
        };

        txn.commit().await.map_err(map_err)?;
        self.bump_entity_gen().await;
        Ok(org_view(model_to_org(final_model)?))
    }
}
