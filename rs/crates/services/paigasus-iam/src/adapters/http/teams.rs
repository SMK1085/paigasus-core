// SPDX-License-Identifier: Apache-2.0

//! `/v1/teams/{id}` handlers, plus the team-scoped `POST`/`GET .../projects` nested routes
//! (creating/listing a project is a team-scoped operation). Thin extract -> service call ->
//! map, mirroring `organizations.rs`.
//!
//! **SMA-444 Task 20 enforcement:** mirrors `organizations.rs`'s posture. `CreateProject`/
//! `ListProjects` authorize against the PARENT team's PRN — unlike `CreateTeam`'s parent org
//! (buildable straight from the path's bare `org_id`), a team's own PRN carries its org uuid
//! (`prn:pgs:iam::{org}:team/{id}`), which the HTTP path's bare `team_id` doesn't — so these
//! two fetch the team first (`s.teams.get`), which doubles as the pre-existing NotFound-on-
//! missing-team behavior every service call already had.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;
use uuid::Uuid;

use super::AppState;
use super::ENFORCE_TENANCY;
use super::dto::{CreateNodeBody, PageQuery, ProjectDto, RenameBody, TeamDto};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/teams/{id}", get(get_team).patch(rename_team))
        .route("/v1/teams/{id}/archive", post(archive_team))
        .route("/v1/teams/{id}/restore", post(restore_team))
        .route("/v1/teams/{id}/projects", post(create_project).get(list_projects))
}

fn actor_prn(ctx: &AuthContext) -> paigasus_kernel::Prn {
    ctx.principal_id.prn().clone()
}

async fn get_team(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    let view = s.teams.get(id).await?;
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::GetTeam, view.node.id.prn()).await?;
    }
    Ok(Json(view.into()))
}

async fn rename_team(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, Json(b): Json<RenameBody>) -> Result<Json<TeamDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.teams.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RenameTeam, view.node.id.prn()).await?;
    }
    let view = s.teams.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_team(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.teams.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ArchiveTeam, view.node.id.prn()).await?;
    }
    let view = s.teams.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_team(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.teams.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RestoreTeam, view.node.id.prn()).await?;
    }
    let view = s.teams.restore(id).await?;
    Ok(Json(view.into()))
}

async fn create_project(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(team_id): Path<Uuid>,
    Json(b): Json<CreateNodeBody>,
) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
    if ENFORCE_TENANCY {
        let team_view = s.teams.get(team_id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::CreateProject, team_view.node.id.prn()).await?;
    }
    let view = s.projects.create(team_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn list_projects(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(team_id): Path<Uuid>, Query(q): Query<PageQuery>) -> Result<Json<Vec<ProjectDto>>, ApiError> {
    if ENFORCE_TENANCY {
        let team_view = s.teams.get(team_id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ListProjects, team_view.node.id.prn()).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let projects = s.projects.list_by_team(team_id, page).await?;
    Ok(Json(projects.into_iter().map(ProjectDto::from).collect()))
}
