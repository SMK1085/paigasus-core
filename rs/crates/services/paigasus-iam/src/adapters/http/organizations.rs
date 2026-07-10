// SPDX-License-Identifier: Apache-2.0

//! `/v1/organizations` handlers, plus the org-scoped `POST`/`GET .../teams` nested routes
//! (creating/listing a team is an organization-scoped operation). Every handler is a thin
//! extract -> service call -> map; all fallible work lives in `OrganizationService`/
//! `TeamService` and the `ApiError` mapping.
//!
//! **SMA-444 Task 20 enforcement:** every handler authorizes the caller (the bearer-resolved
//! [`AuthContext`]) before performing its operation, gated by [`ENFORCE_TENANCY`] — see the
//! spec §9.4 action->resource table. `Get`/`Rename`/`Archive`/`Restore` fetch the node FIRST
//! (the pre-existing 404-on-unknown-id behavior, e.g. `org_lifecycle_over_http`) and
//! authorize against its confirmed, stored PRN — never a caller-suppliable one — so an
//! unauthorized caller never learns whether a forged id would otherwise 404 vs 403 before the
//! authorization check runs against real data. `CreateTeam`/`ListTeams` authorize against the
//! parent org's PRN, built directly from the path's `org_id` (no extra fetch needed — an
//! `OrganizationId` PRN carries no other node's identity).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;
use paigasus_iam_core::OrganizationId;
use paigasus_iam_core::authz::model::root_prn;
use uuid::Uuid;

use super::AppState;
use super::ENFORCE_TENANCY;
use super::dto::{CreateNodeBody, CreateOrgResponse, OrgDto, PageQuery, RenameBody, TeamDto};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/organizations", post(create_org).get(list_orgs))
        .route("/v1/organizations/{id}", get(get_org).patch(rename_org))
        .route("/v1/organizations/{id}/archive", post(archive_org))
        .route("/v1/organizations/{id}/restore", post(restore_org))
        .route("/v1/organizations/{id}/teams", post(create_team).get(list_teams))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `adapters::http::authz::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> paigasus_kernel::Prn {
    ctx.principal_id.prn().clone()
}

async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<CreateOrgResponse>), ApiError> {
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::CreateOrganization, &root_prn()).await?;
    }
    let out = s.orgs.create(&b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(out.into())))
}

async fn list_orgs(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<PageQuery>) -> Result<Json<Vec<OrgDto>>, ApiError> {
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::ListOrganizations, &root_prn()).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let orgs = s.orgs.list(page).await?;
    Ok(Json(orgs.into_iter().map(OrgDto::from).collect()))
}

async fn get_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    let view = s.orgs.get(id).await?;
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::GetOrganization, view.node.id.prn()).await?;
    }
    Ok(Json(view.into()))
}

async fn rename_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RenameOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ArchiveOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<OrgDto>, ApiError> {
    if ENFORCE_TENANCY {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RestoreOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.restore(id).await?;
    Ok(Json(view.into()))
}

async fn create_team(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(org_id): Path<Uuid>, Json(b): Json<CreateNodeBody>) -> Result<(StatusCode, Json<TeamDto>), ApiError> {
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::CreateTeam, OrganizationId::from_uuid(org_id).prn()).await?;
    }
    let view = s.teams.create(org_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn list_teams(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(org_id): Path<Uuid>, Query(q): Query<PageQuery>) -> Result<Json<Vec<TeamDto>>, ApiError> {
    if ENFORCE_TENANCY {
        s.authorize.check(&actor_prn(&ctx), Action::ListTeams, OrganizationId::from_uuid(org_id).prn()).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let teams = s.teams.list_by_org(org_id, page).await?;
    Ok(Json(teams.into_iter().map(TeamDto::from).collect()))
}
