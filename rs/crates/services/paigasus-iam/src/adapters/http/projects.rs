// SPDX-License-Identifier: Apache-2.0

//! `/v1/projects/{id}` handlers — thin extract -> service call -> map, mirroring
//! `organizations.rs`/`teams.rs`. Projects have no further nested collection (memberships
//! attach to any tenancy node via `/v1/memberships`, landing in Task 15).
//!
//! **SMA-444 Task 20 enforcement:** mirrors `organizations.rs`/`teams.rs`'s fetch-then-
//! authorize-then-act posture for `Get`/`Rename`/`Archive`/`Restore`.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;

use super::AppState;
use super::dto::{ProjectDto, RenameBody};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{ProjectId, UuidPath};
use crate::adapters::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/projects/{id}", get(get_project).patch(rename_project))
        .route("/v1/projects/{id}/archive", post(archive_project))
        .route("/v1/projects/{id}/restore", post(restore_project))
}

fn actor_prn(ctx: &AuthContext) -> paigasus_kernel::Prn {
    ctx.principal_id.prn().clone()
}

async fn get_project(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ProjectId>) -> Result<Json<ProjectDto>, ApiError> {
    let id = path.id;
    let view = s.projects.get(id).await?;
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::GetProject, view.node.id.prn()).await?;
    }
    Ok(Json(view.into()))
}

async fn rename_project(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ProjectId>, EnvelopeJson(b): EnvelopeJson<RenameBody>) -> Result<Json<ProjectDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.projects.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RenameProject, view.node.id.prn()).await?;
    }
    let view = s.projects.rename(id, b.slug.as_deref(), b.name.as_deref(), &ctx.principal_id).await?;
    Ok(Json(view.into()))
}

async fn archive_project(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ProjectId>) -> Result<Json<ProjectDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.projects.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::ArchiveProject, view.node.id.prn()).await?;
    }
    let view = s.projects.archive(id, &ctx.principal_id).await?;
    Ok(Json(view.into()))
}

async fn restore_project(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ProjectId>) -> Result<Json<ProjectDto>, ApiError> {
    let id = path.id;
    if s.enforce_tenancy {
        let view = s.projects.get(id).await?;
        s.authorize.check(&actor_prn(&ctx), Action::RestoreProject, view.node.id.prn()).await?;
    }
    let view = s.projects.restore(id, &ctx.principal_id).await?;
    Ok(Json(view.into()))
}
