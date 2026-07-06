// SPDX-License-Identifier: Apache-2.0

//! `/v1/memberships` handlers: attach/detach/list principal-to-tenancy-node memberships.
//! Thin extract -> service call -> map, mirroring `organizations.rs`/`teams.rs`, except
//! `list_memberships` also validates the query itself: exactly one of `principal`/`node`
//! must be set, else `TenancyError::InvalidPrn("provide exactly one of principal|node")`
//! (code `invalid-prn`, 400) — mirrors the proto oneof rule (ADR-0014).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use uuid::Uuid;

use super::AppState;
use super::dto::{CreateMembershipBody, MembershipDto, MembershipQuery};
use super::error::ApiError;
use crate::application::error::TenancyError;
use crate::application::memberships::MembershipFilter;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/memberships", post(create_membership).get(list_memberships))
        .route("/v1/memberships/{id}", delete(delete_membership))
}

async fn create_membership(State(s): State<AppState>, Json(b): Json<CreateMembershipBody>) -> Result<(StatusCode, Json<MembershipDto>), ApiError> {
    let record = s.memberships.attach(&b.principal_prn, &b.node_prn).await?;
    Ok((StatusCode::CREATED, Json(record.into())))
}

/// `DELETE /v1/memberships/{id}`. Detaching an ORG membership cascades: the
/// principal's team/project memberships within that same org are removed in
/// the same transaction (spec §5.1 rule 5). Detaching a team/project
/// membership removes only itself. Detaching a nonexistent id is a 404, not
/// an idempotent no-op.
async fn delete_membership(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    s.memberships.detach(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_memberships(State(s): State<AppState>, Query(q): Query<MembershipQuery>) -> Result<Json<Vec<MembershipDto>>, ApiError> {
    let filter = match (q.principal, q.node) {
        (Some(principal), None) => MembershipFilter::Principal(principal),
        (None, Some(node)) => MembershipFilter::Node(node),
        _ => return Err(ApiError(TenancyError::InvalidPrn("provide exactly one of principal|node".to_string()))),
    };
    let page = Page::new(q.limit, q.offset)?;
    let records = s.memberships.list(filter, page).await?;
    Ok(Json(records.into_iter().map(MembershipDto::from).collect()))
}
