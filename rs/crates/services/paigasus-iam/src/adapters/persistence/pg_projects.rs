// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `ProjectRepository` (SeaORM). Maps domain <-> entity models and backend
//! errors into the core's `RepositoryError`. Mirrors `pg_teams.rs` one level deeper: every
//! guard/view computation folds the project's two ancestors (team, then org) through the
//! shared `NodeStatus::effective` rule (D1/D10) rather than hand-rolling the combination.
//! Every successful `create`/`rename`/`set_status` bumps `entity_gen` via the shared
//! `Generations` handle (spec §7/D11), best-effort — see `pg_organizations.rs`'s doc comment
//! for why a bump failure is logged and swallowed rather than surfaced (SMA-444 Task 15).

use super::entities::{organization, project, team};
use super::map_err;
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{NodeStatus, NodeView, PreconditionKind, Project, ProjectId, ProjectRepository, RepositoryError, Slug, TeamId};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root (`http::AppState::new`) hold a repo handle inside a
// `#[derive(Clone)] ProjectService` — cheap, `DatabaseConnection` clones an `Arc`-backed
// pool handle, not a connection, and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgProjectRepository {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgProjectRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgProjectRepository { db, gens }
    }

    /// Best-effort `entity_gen` bump — see `pg_organizations.rs::bump_entity_gen`'s doc
    /// comment for the fail-open rationale.
    async fn bump_entity_gen(&self) {
        if let Err(err) = self.gens.bump_entity_gen().await {
            tracing::warn!(error = %err, "pg_projects: entity_gen bump failed after a committed write — authz caches may serve stale data until TTL");
        }
    }
}

/// Builds the insertable `project` row from a domain `Project`.
fn project_to_model(project: &Project) -> project::ActiveModel {
    project::ActiveModel {
        id: Set(project.id.uuid()),
        team_id: Set(project.team_id.uuid()),
        org_id: Set(project.id.org_uuid()),
        prn: Set(project.id.canonical()),
        slug: Set(project.slug.as_str().to_string()),
        name: Set(project.name.clone()),
        status: Set(project.status.as_str().to_string()),
        created_at: Set(project.created_at),
        updated_at: Set(project.updated_at),
    }
}

/// Re-parses a stored `project` row back into the pure-core `Project` (mirrors
/// `model_to_team`): a parse failure on stored data becomes a `Backend` error, never a
/// silent default.
fn model_to_project(model: project::Model) -> Result<Project, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));

    let prn = Prn::parse(&model.prn).map_err(|e| backend(e.to_string()))?;
    let id = ProjectId::from_prn(prn).map_err(|e| backend(e.to_string()))?;
    let team_id = TeamId::from_parts(model.org_id, model.team_id);
    let slug = Slug::parse(&model.slug).map_err(|e| backend(e.to_string()))?;
    let status = parse_status(&model.status)?;

    Ok(Project {
        id,
        team_id,
        slug,
        name: model.name,
        status,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

/// Parses a stored `status` column value. Used for a project's own status and for both
/// ancestors (team, org) — a corrupt value anywhere is a `Backend` error, never a default.
fn parse_status(raw: &str) -> Result<NodeStatus, RepositoryError> {
    NodeStatus::parse(raw).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other(format!("bad node status: {raw}")))))
}

/// Folds the project's own status with its two ancestors (team, org) through the shared rule.
fn project_view(project: Project, team_status: NodeStatus, org_status: NodeStatus) -> NodeView<Project> {
    let effective_status = NodeStatus::effective(project.status, &[team_status, org_status]);
    NodeView { node: project, effective_status }
}

/// A missing ancestor row for an existing child is a data-integrity break (the FK should
/// prevent it) — surfaced as `Backend`, never as a silent `NotFound` on the child's own read.
fn missing_ancestor(kind: &str, ancestor_id: Uuid, child_kind: &str, child_id: Uuid) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("{kind} {ancestor_id} missing for {child_kind} {child_id}"))))
}

#[async_trait]
impl ProjectRepository for PgProjectRepository {
    async fn create(&self, project: &Project) -> Result<(), RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        // D8: the team row is locked FOR SHARE for the duration of this txn so a concurrent
        // archive/delete can't race the guard below.
        let Some(team) = team::Entity::find_by_id(project.team_id.uuid()).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        // Invariant breach (`Project::new` already prevents constructing a cross-org
        // project/team pair) — belt-and-braces, never expected to trip in practice.
        if team.org_id != project.id.org_uuid() {
            return Err(RepositoryError::Backend(Box::new(std::io::Error::other("project org does not match team org"))));
        }

        let Some(org) = organization::Entity::find_by_id(team.org_id).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", team.org_id, "team", team.id));
        };

        let team_status = parse_status(&team.status)?;
        let org_status = parse_status(&org.status)?;
        if NodeStatus::effective(team_status, &[org_status]) == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::ParentArchived));
        }

        project_to_model(project).insert(&txn).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        self.bump_entity_gen().await;
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Project>>, RepositoryError> {
        let Some(model) = project::Entity::find_by_id(id).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        let team_id = model.team_id;
        let org_id = model.org_id;
        let project = model_to_project(model)?;

        // Read path: no lock, ancestor statuses fetched by two further selects.
        let Some(team) = team::Entity::find_by_id(team_id).one(&self.db).await.map_err(map_err)? else {
            return Err(missing_ancestor("team", team_id, "project", id));
        };
        let Some(org) = organization::Entity::find_by_id(org_id).one(&self.db).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", org_id, "project", id));
        };

        Ok(Some(project_view(project, parse_status(&team.status)?, parse_status(&org.status)?)))
    }

    async fn list_by_team(&self, team: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Project>>, RepositoryError> {
        let models = project::Entity::find()
            .filter(project::Column::TeamId.eq(team))
            .order_by_asc(project::Column::CreatedAt)
            .order_by_asc(project::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        if models.is_empty() {
            return Ok(Vec::new());
        }

        // Both ancestors are fetched once and reused for every row (all rows share the same
        // team_id, hence the same org_id too).
        let Some(team_model) = team::Entity::find_by_id(team).one(&self.db).await.map_err(map_err)? else {
            return Err(RepositoryError::Backend(Box::new(std::io::Error::other(format!("team {team} missing for its own projects")))));
        };
        let Some(org_model) = organization::Entity::find_by_id(team_model.org_id).one(&self.db).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", team_model.org_id, "team", team));
        };
        let team_status = parse_status(&team_model.status)?;
        let org_status = parse_status(&org_model.status)?;

        models.into_iter().map(|m| model_to_project(m).map(|p| project_view(p, team_status, org_status))).collect()
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = project::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        let team_id = model.team_id;
        let org_id = model.org_id;

        let Some(team) = team::Entity::find_by_id(team_id).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("team", team_id, "project", id));
        };
        let Some(org) = organization::Entity::find_by_id(org_id).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", org_id, "project", id));
        };

        let team_status = parse_status(&team.status)?;
        let org_status = parse_status(&org.status)?;
        let project_status = parse_status(&model.status)?;

        // Guard on EFFECTIVE status: own status, team's, or org's archives a rename attempt.
        if NodeStatus::effective(project_status, &[team_status, org_status]) == NodeStatus::Archived {
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
        Ok(project_view(model_to_project(updated)?, team_status, org_status))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Project>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = project::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        // D10: no ancestor guard — setting a project's own status is always permitted.
        let final_model = if model.status == status.as_str() {
            // Idempotent: already at the target status — no-op, `updated_at` untouched.
            model
        } else {
            let mut active = model.into_active_model();
            active.status = Set(status.as_str().to_owned());
            active.updated_at = Set(now);
            active.update(&txn).await.map_err(map_err)?
        };

        // Ancestors fetched after the update (no lock) purely to compute the returned view.
        let Some(team) = team::Entity::find_by_id(final_model.team_id).one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("team", final_model.team_id, "project", id));
        };
        let Some(org) = organization::Entity::find_by_id(final_model.org_id).one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", final_model.org_id, "project", id));
        };
        let team_status = parse_status(&team.status)?;
        let org_status = parse_status(&org.status)?;

        txn.commit().await.map_err(map_err)?;
        self.bump_entity_gen().await;
        Ok(project_view(model_to_project(final_model)?, team_status, org_status))
    }
}
