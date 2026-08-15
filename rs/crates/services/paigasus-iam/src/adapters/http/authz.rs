// SPDX-License-Identifier: Apache-2.0

//! `/v1/authz` handlers: the `IsAuthorized` query plus policy/role-grant CRUD (SMA-444
//! Task 18, spec §9.2). Sits on the protected `/v1` sub-router (bearer-enforced, D14) —
//! mirrors `memberships.rs`'s thin extract -> service call -> map shape, except every
//! handler here ALSO extracts the caller's [`AuthContext`] (the acting principal), since
//! everything beneath these routes (`Authorize::decide`, `RoleService`, `PolicyService`)
//! needs an actor to authorize against. Management routes (policies/role-grants) are
//! doubly gated: bearer-enforced (this module sits behind `auth_middleware::require_bearer`,
//! same as every other route merged into `router()`'s `protected` sub-router) AND
//! self-authorized by the application services themselves (`PutPolicy`/`GrantRole` etc.).
//!
//! **`IsAuthorized` self/admin exposure rule (spec §9.2, challenge M6, as scoped for this
//! task):** enforced by [`Authorize::decide_gated`] — a caller may always ask about their
//! OWN access — `request.principal_prn == actor` — and gets back the full [`Decision`]
//! (`allowed` + `determining_policies` + `reason`). Asking about a DIFFERENT principal
//! requires the caller to already hold `Action::ListRoleGrants` on the target resource —
//! i.e., to already administer roles there. If that check denies, the handler returns `403
//! Forbidden` and nothing else: a caller who wasn't permitted to ask the question never sees
//! `determining_policies` (which can carry `grant:<uuid>` ids) or even the `allowed` bit —
//! the 403 path never decides `req` for the probed principal at all. `decide_gated` lives on
//! `Authorize` (not here) so the gRPC `IsAuthorized` surface (SMA-444 Task 19) calls the
//! exact same rule and the two transports can never diverge.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use paigasus_iam_core::authz::model::{ContextValue, PolicyKind};
use paigasus_iam_core::{AccessRequest, Action, Effect, PolicyDocument, RequestContext};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{GrantRoleBody, IsAuthorizedBody, IsAuthorizedResponseDto, PageQuery, PolicyDto, PutPolicyBody, RoleGrantDto, RoleGrantQuery};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;

/// `POST /v1/authz/is-authorized` — the authorization DECISION endpoint. Always mounted: it is
/// the service-to-service primitive the gateway calls per request, not policy administration,
/// so no `authz.admin_enabled` setting removes it (SMA-505 D8).
pub fn decision_router() -> Router<AppState> {
    Router::new().route("/v1/authz/is-authorized", post(is_authorized))
}

/// Policy and role-grant ADMINISTRATION — gated by `authz.admin_enabled`, and the surface the
/// `iam.authz.cedar` capability key describes.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/v1/authz/policies", post(put_policy).get(list_policies))
        .route("/v1/authz/policies/{policy_id}", delete(delete_policy))
        .route("/v1/authz/role-grants", post(create_role_grant).get(list_role_grants))
        .route("/v1/authz/role-grants/{id}", delete(revoke_role_grant))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — every
/// handler in this module authorizes/queries as this principal.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

fn parse_prn(raw: &str) -> Result<Prn, TenancyError> {
    Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))
}

fn parse_policy_kind(raw: &str) -> Result<PolicyKind, TenancyError> {
    match raw {
        "static" => Ok(PolicyKind::Static),
        "template" => Ok(PolicyKind::Template),
        other => Err(TenancyError::PolicyInvalid(format!("unknown policy kind: {other}"))),
    }
}

/// `POST /v1/authz/is-authorized`: see the module docs for the self/admin exposure rule
/// (enforced by [`Authorize::decide_gated`]).
async fn is_authorized(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(body): Json<IsAuthorizedBody>) -> Result<Json<IsAuthorizedResponseDto>, ApiError> {
    let actor = actor_prn(&ctx);
    let action = Action::parse(&body.action).ok_or_else(|| TenancyError::InvalidAction(body.action.clone()))?;
    let principal = parse_prn(&body.principal_prn)?;
    let resource = parse_prn(&body.resource_prn)?;

    let context = RequestContext(body.context.into_iter().map(|(k, v)| (k, ContextValue::Str(v))).collect());
    let req = AccessRequest { principal, action, resource, context };
    let decision = s.authorize.decide_gated(&actor, &req).await?;

    Ok(Json(IsAuthorizedResponseDto {
        allowed: decision.effect == Effect::Allow,
        reason: match decision.effect {
            Effect::Allow => "allowed".to_string(),
            Effect::Deny => "denied".to_string(),
        },
        determining_policies: decision.determining_policies,
    }))
}

/// `POST /v1/authz/policies`: upsert. `system` is always `false` for a client-authored
/// document — the store itself separately rejects mutating an already-persisted system row.
async fn put_policy(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(body): Json<PutPolicyBody>) -> Result<(StatusCode, Json<PolicyDto>), ApiError> {
    let actor = actor_prn(&ctx);
    let kind = parse_policy_kind(&body.kind)?;
    let now = Utc::now();
    let doc = PolicyDocument {
        policy_id: body.policy_id,
        kind,
        source: body.source,
        description: body.description,
        system: false,
        created_at: now,
        updated_at: now,
    };
    s.policies.put(&actor, doc.clone()).await?;
    Ok((StatusCode::OK, Json(doc.into())))
}

async fn list_policies(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<PageQuery>) -> Result<Json<Vec<PolicyDto>>, ApiError> {
    let actor = actor_prn(&ctx);
    let page = Page::new(q.limit, q.offset)?;
    let docs = s.policies.list(&actor, page.limit, page.offset).await?;
    Ok(Json(docs.into_iter().map(PolicyDto::from).collect()))
}

async fn delete_policy(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(policy_id): Path<String>) -> Result<StatusCode, ApiError> {
    let actor = actor_prn(&ctx);
    s.policies.delete(&actor, &policy_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_role_grant(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(body): Json<GrantRoleBody>) -> Result<(StatusCode, Json<RoleGrantDto>), ApiError> {
    let actor = actor_prn(&ctx);
    let grant = s.roles.grant(&actor, &body.principal_prn, &body.role_key, &body.scope_prn).await?;
    Ok((StatusCode::CREATED, Json(grant.into())))
}

async fn list_role_grants(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<RoleGrantQuery>) -> Result<Json<Vec<RoleGrantDto>>, ApiError> {
    let actor = actor_prn(&ctx);
    let principal_prn = q.principal_prn.ok_or_else(|| TenancyError::InvalidPrn("principal_prn query parameter is required".to_string()))?;
    let grants = s.roles.list(&actor, &principal_prn).await?;
    Ok(Json(grants.into_iter().map(RoleGrantDto::from).collect()))
}

async fn revoke_role_grant(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    let actor = actor_prn(&ctx);
    s.roles.revoke(&actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
