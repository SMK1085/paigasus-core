// SPDX-License-Identifier: Apache-2.0

//! The REST routes of the agent API (SMA-852 spec §6.5). Each handler calls `AgentAdmin`, which
//! owns the authorization and the rules. This module authenticates the caller (the code of
//! ingress, not a copy) and maps the request and the answer.
//!
//! The order of a call: authentication (a middleware, before any registry read), the path
//! segment, the body, then `AgentAdmin`.

pub mod dto;
pub mod error;

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use paigasus_agent_host_core::ids::PrincipalId;
use serde_json::Value;

use self::dto::{BindBody, CreateBody, JsonBody, ListQuery, PageQuery, UpdateBody, agent_json, inbox_json, include_archived, page_json, page_size, parse_org_prn};
use self::error::AgentApiError;
use super::auth::{authenticate, bearer_token};
use super::path::{AgentIdField, InboxIdField, UuidPath};
use crate::application::agents::{AdminError, AgentAdmin, ResourceKey};
use crate::application::auth::Caller;

/// The largest body of the agent API. The instructions are at most 32 KiB, and JSON escaping can
/// grow them.
const MAX_BODY_BYTES: usize = 256 * 1024;

type Admin = State<Arc<AgentAdmin>>;
type Answer = Result<(StatusCode, Json<Value>), AgentApiError>;

/// The routes of the agent API. `admin.iam()` authenticates the caller.
pub fn agent_routes(admin: Arc<AgentAdmin>) -> Router {
    Router::new()
        .route("/v1/agents", post(create).get(list))
        .route("/v1/agents/{agent}", get(get_agent).patch(update).delete(archive))
        .route("/v1/agents/{agent}/inboxes", post(bind).get(list_inboxes))
        .route("/v1/inboxes/{inbox}", get(get_inbox))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(admin.clone())
        .route_layer(from_fn_with_state(admin, authenticate_middleware))
}

/// Authenticate through IAM and put the [`Caller`] in the request extensions. It runs before any
/// extractor of the handler, so an unauthenticated call reads nothing.
async fn authenticate_middleware(State(admin): Admin, mut req: Request, next: Next) -> Response {
    let token = match bearer_token(req.headers()) {
        Ok(token) => token,
        Err(e) => return e.into_response(),
    };
    match authenticate(admin.iam().as_ref(), &token).await {
        Ok(caller) => {
            req.extensions_mut().insert(caller);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

/// A row that the path names. A REST path holds a bare uuid, so the key has no org.
fn key(id: uuid::Uuid) -> ResourceKey {
    ResourceKey { id, org: None }
}

async fn create(State(admin): Admin, Extension(caller): Extension<Caller>, JsonBody(body): JsonBody<CreateBody>) -> Answer {
    let org = parse_org_prn(&body.org)?;
    let record = admin.create(&caller, org, body.fields()).await.map_err(AgentApiError)?;
    Ok((StatusCode::CREATED, Json(agent_json(&record))))
}

async fn list(State(admin): Admin, Extension(caller): Extension<Caller>, query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>) -> Answer {
    let Query(q) = query.map_err(|_| AgentApiError(AdminError::InvalidRequestSchema))?;
    let org = parse_org_prn(q.org.as_deref().unwrap_or_default())?;
    let archived = include_archived(q.include_archived.as_deref())?;
    let size = page_size(q.page_size.as_deref())?;
    let (items, next) = admin.list(&caller, org, archived, size, q.page_token.as_deref().unwrap_or_default()).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(page_json(items.iter().map(agent_json).collect(), next))))
}

async fn get_agent(State(admin): Admin, Extension(caller): Extension<Caller>, UuidPath { id, .. }: UuidPath<AgentIdField>) -> Answer {
    let record = admin.get(&caller, key(id)).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(agent_json(&record))))
}

async fn update(State(admin): Admin, Extension(caller): Extension<Caller>, UuidPath { id, .. }: UuidPath<AgentIdField>, JsonBody(body): JsonBody<UpdateBody>) -> Answer {
    let record = admin.update(&caller, key(id), body.version, body.fields()).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(agent_json(&record))))
}

async fn archive(State(admin): Admin, Extension(caller): Extension<Caller>, UuidPath { id, .. }: UuidPath<AgentIdField>) -> Answer {
    let record = admin.archive(&caller, key(id)).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(agent_json(&record))))
}

async fn bind(State(admin): Admin, Extension(caller): Extension<Caller>, UuidPath { id, .. }: UuidPath<AgentIdField>, JsonBody(body): JsonBody<BindBody>) -> Answer {
    let principal = PrincipalId::parse(&body.principal).map_err(|_| AgentApiError(AdminError::InvalidField("principal")))?;
    let record = admin.create_inbox(&caller, key(id), principal).await.map_err(AgentApiError)?;
    Ok((StatusCode::CREATED, Json(inbox_json(&record))))
}

async fn list_inboxes(
    State(admin): Admin,
    Extension(caller): Extension<Caller>,
    UuidPath { id, .. }: UuidPath<AgentIdField>,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Answer {
    let Query(q) = query.map_err(|_| AgentApiError(AdminError::InvalidRequestSchema))?;
    let size = page_size(q.page_size.as_deref())?;
    let (items, next) = admin.list_inboxes(&caller, key(id), size, q.page_token.as_deref().unwrap_or_default()).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(page_json(items.iter().map(inbox_json).collect(), next))))
}

async fn get_inbox(State(admin): Admin, Extension(caller): Extension<Caller>, UuidPath { id, .. }: UuidPath<InboxIdField>) -> Answer {
    let record = admin.get_inbox(&caller, key(id)).await.map_err(AgentApiError)?;
    Ok((StatusCode::OK, Json(inbox_json(&record))))
}
