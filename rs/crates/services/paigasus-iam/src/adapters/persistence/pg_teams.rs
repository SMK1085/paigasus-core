// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `TeamRepository` (SeaORM). Maps domain <-> entity models and backend
//! errors into the core's `RepositoryError`. Mirrors `pg_organizations.rs`'s shape one level
//! deeper: every guard/view computation folds the team's single ancestor (its org) through
//! the shared `NodeStatus::effective` rule (D1/D10) rather than hand-rolling the combination.

use super::entities::{organization, team};
use super::map_err;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{NodeStatus, NodeView, PreconditionKind, RepositoryError, Slug, Team, TeamId, TeamRepository};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root (`http::AppState::new`) hold a repo handle inside a
// `#[derive(Clone)] TeamService`/`ProjectService` — cheap, `DatabaseConnection` clones an
// `Arc`-backed pool handle, not a connection.
#[derive(Clone)]
pub struct PgTeamRepository {
    db: DatabaseConnection,
}

impl PgTeamRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgTeamRepository { db }
    }
}

/// Builds the insertable `team` row from a domain `Team`.
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

/// Re-parses a stored `team` row back into the pure-core `Team` (mirrors `model_to_org`): a
/// parse failure on stored data becomes a `Backend` error, never a silent default.
fn model_to_team(model: team::Model) -> Result<Team, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));

    let prn = Prn::parse(&model.prn).map_err(|e| backend(e.to_string()))?;
    let id = TeamId::from_prn(prn).map_err(|e| backend(e.to_string()))?;
    let slug = Slug::parse(&model.slug).map_err(|e| backend(e.to_string()))?;
    let status = parse_status(&model.status)?;

    Ok(Team {
        id,
        slug,
        name: model.name,
        status,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

/// Parses a stored `status` column value. Used both for a team's own status and for an
/// ancestor org's status — a corrupt value in either is a `Backend` error, never a default.
fn parse_status(raw: &str) -> Result<NodeStatus, RepositoryError> {
    NodeStatus::parse(raw).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other(format!("bad node status: {raw}")))))
}

/// Folds the team's own status with its single ancestor (org) through the shared rule.
fn team_view(team: Team, org_status: NodeStatus) -> NodeView<Team> {
    let effective_status = NodeStatus::effective(team.status, &[org_status]);
    NodeView { node: team, effective_status }
}

/// A missing ancestor row for an existing child is a data-integrity break (the FK should
/// prevent it) — surfaced as `Backend`, never as a silent `NotFound` on the child's own read.
fn missing_ancestor(kind: &str, ancestor_id: Uuid, child_kind: &str, child_id: Uuid) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("{kind} {ancestor_id} missing for {child_kind} {child_id}"))))
}

#[async_trait]
impl TeamRepository for PgTeamRepository {
    async fn create(&self, team: &Team) -> Result<(), RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        // D8: the org row is locked FOR SHARE for the duration of this txn so a concurrent
        // archive/delete can't race the guard below.
        let Some(org) = organization::Entity::find_by_id(team.id.org_uuid()).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        if parse_status(&org.status)? == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::ParentArchived));
        }

        team_to_model(team).insert(&txn).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Option<NodeView<Team>>, RepositoryError> {
        let Some(model) = team::Entity::find_by_id(id).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        let org_id = model.org_id;
        let team = model_to_team(model)?;

        // Read path: no lock, ancestor status fetched by a second select.
        let Some(org) = organization::Entity::find_by_id(org_id).one(&self.db).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", org_id, "team", id));
        };
        Ok(Some(team_view(team, parse_status(&org.status)?)))
    }

    async fn list_by_org(&self, org: Uuid, limit: u64, offset: u64) -> Result<Vec<NodeView<Team>>, RepositoryError> {
        let models = team::Entity::find()
            .filter(team::Column::OrgId.eq(org))
            .order_by_asc(team::Column::CreatedAt)
            .order_by_asc(team::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        if models.is_empty() {
            return Ok(Vec::new());
        }

        // The org is fetched once and reused for every row (all rows share the same org_id).
        let Some(org_model) = organization::Entity::find_by_id(org).one(&self.db).await.map_err(map_err)? else {
            return Err(RepositoryError::Backend(Box::new(std::io::Error::other(format!("organization {org} missing for its own teams")))));
        };
        let org_status = parse_status(&org_model.status)?;

        models.into_iter().map(|m| model_to_team(m).map(|t| team_view(t, org_status))).collect()
    }

    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = team::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        let org_id = model.org_id;
        let Some(org) = organization::Entity::find_by_id(org_id).lock_shared().one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", org_id, "team", id));
        };
        let org_status = parse_status(&org.status)?;
        let team_status = parse_status(&model.status)?;

        // Guard on EFFECTIVE status: own status or the org's archives a rename attempt.
        if NodeStatus::effective(team_status, &[org_status]) == NodeStatus::Archived {
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
        Ok(team_view(model_to_team(updated)?, org_status))
    }

    async fn set_status(&self, id: Uuid, status: NodeStatus, now: DateTime<Utc>) -> Result<NodeView<Team>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = team::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        // D10: no ancestor guard — setting a team's own status is always permitted.
        let final_model = if model.status == status.as_str() {
            // Idempotent: already at the target status — no-op, `updated_at` untouched.
            model
        } else {
            let mut active = model.into_active_model();
            active.status = Set(status.as_str().to_owned());
            active.updated_at = Set(now);
            active.update(&txn).await.map_err(map_err)?
        };

        // Ancestor fetched after the update (no lock) purely to compute the returned view.
        let Some(org) = organization::Entity::find_by_id(final_model.org_id).one(&txn).await.map_err(map_err)? else {
            return Err(missing_ancestor("organization", final_model.org_id, "team", id));
        };
        let org_status = parse_status(&org.status)?;

        txn.commit().await.map_err(map_err)?;
        Ok(team_view(model_to_team(final_model)?, org_status))
    }
}
