// SPDX-License-Identifier: Apache-2.0

//! `/v1/organizations` handlers, plus the org-scoped `POST`/`GET .../teams` nested routes
//! (creating/listing a team is an organization-scoped operation). Every handler is a thin
//! extract -> service call -> map; all fallible work lives in `OrganizationService`/
//! `TeamService` and the `ApiError` mapping.
//!
//! **SMA-444 Task 20 enforcement:** every handler authorizes the caller (the bearer-resolved
//! [`AuthContext`]) before performing its operation, gated by `AppState.enforce_tenancy`
//! (config-driven, `authz.enforce_tenancy`, SMA-444 Task 21) — see the
//! spec §9.4 action->resource table. `Get`/`Rename`/`Archive`/`Restore` fetch the node FIRST
//! (the pre-existing 404-on-unknown-id behavior, e.g. `org_lifecycle_over_http`) and
//! authorize against its confirmed, stored PRN — never a caller-suppliable one — so an
//! unauthorized caller never learns whether a forged id would otherwise 404 vs 403 before the
//! authorization check runs against real data. `CreateTeam`/`ListTeams` mirror that same
//! fetch-first pattern (mirroring `teams.rs`'s `CreateProject`/`ListProjects`): they fetch the
//! *parent org* first (`s.orgs.get`) and authorize against its confirmed PRN — a PRN built
//! directly from the path's bare `org_id` would resolve fine even for a nonexistent org (an
//! `OrganizationId` PRN carries no other node's identity to validate), so the authorize call
//! would reach the entity-slice loader with a dangling id and fail closed as an internal error
//! rather than the expected `NotFound`/404.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;
use paigasus_iam_core::authz::model::root_prn;

use super::AppState;
use super::dto::{CreateNodeBody, CreateOrgResponse, OrgDto, PageQuery, RenameBody, TeamDto};
use super::error::ApiError;
use super::path::{OrganizationId, UuidPath};
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
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::CreateOrganization, &root_prn()).await?;
    }
    // The creating principal becomes the new org's `org_admin` owner (spec D8) — seeded
    // atomically with the org + default team, regardless of `enforce_tenancy`.
    let out = s.orgs.create(&ctx.principal_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(out.into())))
}

async fn list_orgs(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<PageQuery>) -> Result<Json<Vec<OrgDto>>, ApiError> {
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::ListOrganizations, &root_prn()).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let orgs = s.orgs.list(page).await?;
    Ok(Json(orgs.into_iter().map(OrgDto::from).collect()))
}

async fn get_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>) -> Result<Json<OrgDto>, ApiError> {
    let id = path.id;
    let view = s.orgs.get(id).await?;
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::GetOrganization, view.node.id.prn()).await?;
    }
    Ok(Json(view.into()))
}

async fn rename_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>, Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RenameOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.rename(id, b.slug.as_deref(), b.name.as_deref()).await?;
    Ok(Json(view.into()))
}

async fn archive_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>) -> Result<Json<OrgDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ArchiveOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.archive(id).await?;
    Ok(Json(view.into()))
}

async fn restore_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>) -> Result<Json<OrgDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.orgs.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RestoreOrganization, view.node.id.prn()).await?;
    }
    let view = s.orgs.restore(id).await?;
    Ok(Json(view.into()))
}

async fn create_team(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    Json(b): Json<CreateNodeBody>,
) -> Result<(StatusCode, Json<TeamDto>), ApiError> {
    let org_id = path.id;
    if s.enforce_tenancy {
        let org_view = s.orgs.get(org_id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::CreateTeam, org_view.node.id.prn()).await?;
    }
    let view = s.teams.create(org_id, &b.slug, &b.name).await?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

async fn list_teams(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>, Query(q): Query<PageQuery>) -> Result<Json<Vec<TeamDto>>, ApiError> {
    let org_id = path.id;
    if s.enforce_tenancy {
        let org_view = s.orgs.get(org_id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ListTeams, org_view.node.id.prn()).await?;
    }
    let page = Page::new(q.limit, q.offset)?;
    let teams = s.teams.list_by_org(org_id, page).await?;
    Ok(Json(teams.into_iter().map(TeamDto::from).collect()))
}
