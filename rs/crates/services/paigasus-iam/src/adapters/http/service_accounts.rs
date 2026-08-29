// SPDX-License-Identifier: Apache-2.0

//! `/v1/service-accounts` handlers (SMA-445 Task 20): service-account lifecycle CRUD over
//! HTTP. Mirrors `adapters::http::authz.rs`'s create/delete shape — `State` + `Extension
//! <AuthContext>` + `Json` DTOs, returning `Result<_, ApiError>` — layered on
//! `ServiceAccountService` (Task 16), which itself authorizes every call against the SA's
//! OWNER tenancy node before mutating/reading (never the SA's own principal PRN,
//! `application/service_accounts.rs`'s module docs). Sits on the bearer-gated `protected`
//! sub-router (merged in `mod.rs`), exactly like every other tenancy/authz route.
//!
//! **Path-id convention:** `{sa}` is the service account's PRINCIPAL uuid (the bare uuid
//! inside its `PrincipalId`'s PRN, `resource_type = "principal"`) — mirrors
//! `organizations.rs`'s convention for `{id}` (a tenancy node's bare uuid, extracted through
//! `path::UuidPath` since SMA-586) rather than a raw PRN string in the path (which would need
//! percent-encoding around its embedded `/`). `create`'s response DTO still carries the full canonical `prn` like every other
//! `*Dto`, so a client threads the SAME id forward by taking the PRN's trailing `/`-segment —
//! exactly the convention `tests/http_tenancy.rs` already uses for `ProjectDto`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::{PrincipalId, TenancyNodeRef};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{CreateServiceAccountBody, ServiceAccountDto, ServiceAccountQuery};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{ServiceAccountId, UuidPath};
use super::query::EnvelopeQuery;
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/service-accounts", post(create).get(list))
        .route("/v1/service-accounts/{sa}", get(get_one).delete(archive))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `adapters::http::authz::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// Parses a caller-supplied PRN string into a `TenancyNodeRef` — mirrors
/// `adapters::http::authz::parse_prn`, additionally funneling a well-formed-but-wrong-kind PRN
/// (e.g. a principal PRN where an org/team/project PRN was expected) through the SAME
/// `TenancyError::InvalidPrn` variant a malformed PRN would produce.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    TenancyNodeRef::from_prn(prn).map_err(TenancyError::from)
}

/// Builds the `PrincipalId` a `{sa}` path segment names. `Prn::build` with the fixed, always-
/// valid literal `service`/`resource_type` strings used here can never fail (mirrors
/// `OrganizationId::from_uuid`'s identical `.expect`).
fn service_account_id(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).expect("static principal prn parts are valid"))
}

async fn create(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    EnvelopeJson(body): EnvelopeJson<CreateServiceAccountBody>,
) -> Result<(StatusCode, Json<ServiceAccountDto>), ApiError> {
    let actor = actor_prn(&ctx);
    let owner = parse_node_prn(&body.owner_prn)?;
    let sa = s.service_accounts.create(&actor, owner, &body.name).await?;
    Ok((StatusCode::CREATED, Json(sa.into())))
}

async fn list(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeQuery(q): EnvelopeQuery<ServiceAccountQuery>) -> Result<Json<Vec<ServiceAccountDto>>, ApiError> {
    let actor = actor_prn(&ctx);
    let owner_prn = q.owner_prn.filter(|s| !s.trim().is_empty()).ok_or(TenancyError::MissingRequiredField("owner_prn"))?;
    let owner = parse_node_prn(&owner_prn)?;
    let page = Page::new(q.limit, q.offset)?;
    let accounts = s.service_accounts.list(&actor, &owner, page).await?;
    Ok(Json(accounts.into_iter().map(ServiceAccountDto::from).collect()))
}

async fn get_one(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ServiceAccountId>) -> Result<Json<ServiceAccountDto>, ApiError> {
    let actor = actor_prn(&ctx);
    let id = service_account_id(path.id);
    let account = s.service_accounts.get(&actor, &id).await?;
    Ok(Json(account.into()))
}

/// `DELETE /v1/service-accounts/{sa}`: archives (disables) the service account and evicts
/// every one of its cached API-key validations (`ServiceAccountService::archive`'s own
/// module docs — the security-critical step). `204 No Content`, mirroring
/// `adapters::http::authz::revoke_role_grant`'s shape.
async fn archive(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ServiceAccountId>) -> Result<StatusCode, ApiError> {
    let actor = actor_prn(&ctx);
    let id = service_account_id(path.id);
    s.service_accounts.archive(&actor, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
