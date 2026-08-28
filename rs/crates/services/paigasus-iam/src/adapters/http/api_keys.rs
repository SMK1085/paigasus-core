// SPDX-License-Identifier: Apache-2.0

//! `/v1/service-accounts/{sa}/api-keys` management handlers plus the unauthenticated
//! `POST /v1/authn/api-keys/introspect` endpoint (SMA-445 Task 20).
//!
//! **Management routes** (`issue`/`list`/`revoke`) mirror `service_accounts.rs`'s shape —
//! `State` + `Extension<AuthContext>` + `Json`/`Path` extraction, `Result<_, ApiError>` — and
//! sit on the SAME bearer-gated `protected` sub-router (merged in `mod.rs`). Every call
//! authorizes through `ApiKeyService` (Task 17), which enforces the D15 anti-escalation
//! invariant BEFORE minting anything (`application/api_keys.rs`'s module docs): `issue`
//! returns `201` with the plaintext token shown exactly once; `list` never carries a secret
//! (`ApiKey` structurally has none); `revoke` is `204`.
//!
//! **Introspection** (`POST /v1/authn/api-keys/introspect`) is the API-key analog of
//! `http/authn.rs`'s `POST /v1/authn/introspect` — unauthenticated (the token travels in the
//! body, not a bearer header) and merged OUTSIDE the bearer-enforcement layer, same
//! `AuthnApiError`/`EnvelopeJson` funnel and route-level body-size cap (spec H1). Exposed via
//! [`introspect_router`], a SEPARATE `Router` from [`router`]'s management routes so `mod.rs`
//! can merge each into the correct half of the HTTP surface.

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use paigasus_iam_core::{Action, ApiKeyId, PrincipalId, TenancyNodeRef};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::authn::AuthnApiError;
use super::dto::{ApiKeyDto, IntrospectApiKeyRequestBody, IntrospectApiKeyResponseDto, IssueApiKeyBody, IssueApiKeyResponseDto, PageQuery};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{ApiKeyId as ApiKeyIdField, ServiceAccountId, UuidPath, UuidPathPair};
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;

/// The bearer-gated management routes — merged into `mod.rs`'s `protected` sub-router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/service-accounts/{sa}/api-keys", post(issue).get(list))
        .route("/v1/service-accounts/{sa}/api-keys/{id}", delete(revoke))
}

/// The unauthenticated introspection route — merged OUTSIDE the bearer layer (mirrors
/// `http/authn.rs::router`'s identical `DefaultBodyLimit` posture for the token-introspect
/// route, spec H1: the only legitimate payload is `{"token":"<= max_token_bytes>"}`, so
/// anything past `body_limit` is rejected before JSON parsing).
pub fn introspect_router(body_limit: usize) -> Router<AppState> {
    Router::new().route("/v1/authn/api-keys/introspect", post(introspect)).route_layer(DefaultBodyLimit::max(body_limit))
}

/// The acting principal's canonical `Prn` — mirrors `adapters::http::authz::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// Parses a caller-supplied PRN string into a `TenancyNodeRef` — duplicated from
/// `service_accounts.rs`'s identical helper rather than made `pub(crate)` across an unrelated
/// module, mirroring `application/api_keys.rs::owner_resource_prn`'s own "not worth a
/// visibility change" rationale for a five-line pure helper.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    TenancyNodeRef::from_prn(prn).map_err(TenancyError::from)
}

/// Builds the `PrincipalId` a `{sa}` path segment names — duplicated from
/// `service_accounts.rs`'s identical helper for the same reason as [`parse_node_prn`].
fn service_account_id(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).expect("static principal prn parts are valid"))
}

/// `POST /v1/service-accounts/{sa}/api-keys`: issues a new key, `201` with the one-time
/// plaintext `token` (spec §10.2's `IssueApiKeyResponse`, shown-once, D2). `scope_actions`
/// entries that don't name a known `Action` fail `400 invalid-action` (mirrors
/// `http/authz.rs::is_authorized`'s identical `Action::parse` funnel) before anything is
/// minted.
async fn issue(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<ServiceAccountId>,
    EnvelopeJson(body): EnvelopeJson<IssueApiKeyBody>,
) -> Result<(StatusCode, Json<IssueApiKeyResponseDto>), ApiError> {
    let actor = actor_prn(&ctx);
    let sa_id = service_account_id(path.id);
    let scope_prn = body.scope_prn.filter(|s| !s.trim().is_empty()).ok_or(TenancyError::MissingRequiredField("scope_prn"))?;
    let scope = parse_node_prn(&scope_prn)?;
    let scope_actions = body
        .scope_actions
        .iter()
        .map(|a| Action::parse(a).ok_or_else(|| TenancyError::InvalidAction(a.clone())))
        .collect::<Result<Vec<_>, _>>()?;
    let new_key = s.api_keys.issue(&actor, &sa_id, scope, body.expires_at, scope_actions, body.scope_roles).await?;
    Ok((StatusCode::CREATED, Json(new_key.into())))
}

/// `GET /v1/service-accounts/{sa}/api-keys`: lists the SA's keys — NEVER a secret/hash
/// (`ApiKeyDto`'s own doc; `ApiKey` structurally has neither field).
async fn list(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<ServiceAccountId>, Query(q): Query<PageQuery>) -> Result<Json<Vec<ApiKeyDto>>, ApiError> {
    let actor = actor_prn(&ctx);
    let sa_id = service_account_id(path.id);
    let page = Page::new(q.limit, q.offset)?;
    let keys = s.api_keys.list(&actor, &sa_id, page).await?;
    Ok(Json(keys.into_iter().map(ApiKeyDto::from).collect()))
}

/// `DELETE /v1/service-accounts/{sa}/api-keys/{id}`: revokes the key and evicts its cached
/// validation (`ApiKeyService::revoke`'s own module docs — the security-critical step). `204
/// No Content`, mirroring `adapters::http::authz::revoke_role_grant`'s shape. `{sa}` is not
/// re-checked against the key's actual owner here — `ApiKeyService::revoke` looks the key up
/// by `{id}` alone and authorizes against ITS OWN service account's owner node, exactly like
/// `RevokeApiKeyRequest` (spec §10.1: `string id = 1`, no service-account field at all); the
/// path segment exists purely for REST nesting. It IS still validated as a uuid, and a
/// malformed `{sa}` reports `service_account_id` — one marker per segment (SMA-586 fix round
/// 2); the single-marker form this replaced named `api_key_id` for both.
async fn revoke(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPathPair<ServiceAccountId, ApiKeyIdField>) -> Result<StatusCode, ApiError> {
    let actor = actor_prn(&ctx);
    // `first` (the `{sa}` segment) is intentionally unused — see the doc comment above: the
    // path segment exists purely for REST nesting, not for re-checking key ownership here.
    let _sa = path.first;
    let id = path.second;
    s.api_keys.revoke(&actor, ApiKeyId::from_uuid(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/authn/api-keys/introspect` (spec §10.2): the full `PrincipalContext` for a
/// presented API-key token. Unauthenticated by design (the credential travels in the body) —
/// mirrors `http/authn.rs::introspect` field-for-field, funneling through the SAME
/// `AuthnApiError` (401 `invalid-token`, 403 `principal-inactive`, 503 `authn-unavailable`) and
/// `EnvelopeJson` (oversized/malformed body -> the same `{"error":{code,message}}` envelope,
/// never axum's default plain-text rejection). Never logs the token.
async fn introspect(State(state): State<AppState>, EnvelopeJson(body): EnvelopeJson<IntrospectApiKeyRequestBody>) -> Result<Json<IntrospectApiKeyResponseDto>, AuthnApiError> {
    let ctx = state.api_key_auth.introspect(&body.token).await?;
    Ok(Json(ctx.into()))
}
