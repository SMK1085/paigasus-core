// SPDX-License-Identifier: Apache-2.0

//! `/v1/organizations` handlers, plus the org-scoped `POST`/`GET .../teams` nested routes
//! (creating/listing a team is an organization-scoped operation). Every handler is a thin
//! extract -> service call -> map; all fallible work lives in `OrganizationService`/
//! `TeamService` and the `ApiError` mapping.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use super::AppState;
use super::dto::{CreateNodeBody, CreateOrgResponse, OrgDto, PageQuery, RenameBody, TeamDto};
use super::error::ApiError;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/organizations", post(create_org).get(list_orgs))
        .route("/v1/organizations/{id}", get(get_org).patch(rename_org))
        .route("/v1/organizations/{id}/archive", post(archive_org))
        .route("/v1/organizations/{id}/restore", post(restore_org))
        .route("/v1/organizations/{id}/teams", post(create_team).get(list_teams))
}

async fn create_org(State(s): State<AppState>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<CreateOrgResponse>), ApiError> {
    let out = s.orgs.create(&b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(out.into())))
}

async fn list_orgs(State(s): State<AppState>, Query(q): Query<PageQuery>) -> Result<Json<Vec<OrgDto>>, ApiError> {
    let page = Page::new(q.limit, q.offset)?;
    let orgs = s.orgs.list(page).await?;
    Ok(Json(orgs.into_iter().map(OrgDto::from).collect()))
}

async fn get_org(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    let view = s.orgs.get(id).await?;
    Ok(Json(view.into()))
}

async fn rename_org(State(s): State<AppState>, Path(id): Path<Uuid>, Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {
    let view = s.orgs.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_org(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    let view = s.orgs.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_org(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    let view = s.orgs.restore(id).await?;
    Ok(Json(view.into()))
}

async fn create_team(State(s): State<AppState>, Path(org_id): Path<Uuid>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<TeamDto>), ApiError> {
    let view = s.teams.create(org_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn list_teams(State(s): State<AppState>, Path(org_id): Path<Uuid>, Query(q): Query<PageQuery>) -> Result<Json<Vec<TeamDto>>, ApiError> {
    let page = Page::new(q.limit, q.offset)?;
    let teams = s.teams.list_by_org(org_id, page).await?;
    Ok(Json(teams.into_iter().map(TeamDto::from).collect()))
}
