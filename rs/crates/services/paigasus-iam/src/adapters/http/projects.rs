// SPDX-License-Identifier: Apache-2.0

//! `/v1/projects/{id}` handlers — thin extract -> service call -> map, mirroring
//! `organizations.rs`/`teams.rs`. Projects have no further nested collection (memberships
//! attach to any tenancy node via `/v1/memberships`, landing in Task 15).

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use super::AppState;
use super::dto::{ProjectDto, RenameBody};
use super::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/projects/{id}", get(get_project).patch(rename_project))
        .route("/v1/projects/{id}/archive", post(archive_project))
        .route("/v1/projects/{id}/restore", post(restore_project))
}

async fn get_project(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<ProjectDto>, ApiError> {
    let view = s.projects.get(id).await?;
    Ok(Json(view.into()))
}

async fn rename_project(State(s): State<AppState>, Path(id): Path<Uuid>, Json(b): Json<RenameBody>) -> Result<Json<ProjectDto>, ApiError> {
    let view = s.projects.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_project(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<ProjectDto>, ApiError> {
    let view = s.projects.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_project(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<ProjectDto>, ApiError> {
    let view = s.projects.restore(id).await?;
    Ok(Json(view.into()))
}
