// SPDX-License-Identifier: Apache-2.0

//! `/v1/teams/{id}` handlers, plus the team-scoped `POST`/`GET .../projects` nested routes
//! (creating/listing a project is a team-scoped operation). Thin extract -> service call ->
//! map, mirroring `organizations.rs`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use super::AppState;
use super::dto::{CreateNodeBody, PageQuery, ProjectDto, RenameBody, TeamDto};
use super::error::ApiError;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/teams/{id}", get(get_team).patch(rename_team))
        .route("/v1/teams/{id}/archive", post(archive_team))
        .route("/v1/teams/{id}/restore", post(restore_team))
        .route("/v1/teams/{id}/projects", post(create_project).get(list_projects))
}

async fn get_team(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    let view = s.teams.get(id).await?;
    Ok(Json(view.into()))
}

async fn rename_team(State(s): State<AppState>, Path(id): Path<Uuid>, Json(b): Json<RenameBody>) -> Result<Json<TeamDto>, ApiError> {
    let view = s.teams.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_team(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    let view = s.teams.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_team(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<TeamDto>, ApiError> {
    let view = s.teams.restore(id).await?;
    Ok(Json(view.into()))
}

async fn create_project(State(s): State<AppState>, Path(team_id): Path<Uuid>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
    let view = s.projects.create(team_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn list_projects(State(s): State<AppState>, Path(team_id): Path<Uuid>, Query(q): Query<PageQuery>) -> Result<Json<Vec<ProjectDto>>, ApiError> {
    let page = Page::new(q.limit, q.offset)?;
    let projects = s.projects.list_by_team(team_id, page).await?;
    Ok(Json(projects.into_iter().map(ProjectDto::from).collect()))
}
