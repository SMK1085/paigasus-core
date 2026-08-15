// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/service-info` — IAM's capability descriptor (ADR-0020, SMA-505).
//!
//! Merged INSIDE `app_routes`'s `protected` sub-router, so it inherits
//! `auth_middleware::require_bearer`: an OIDC session and a service-account API key both work,
//! and no authorization action is checked. Discovery must not be gated on a permission — a
//! caller who legitimately cannot use a feature still needs to know it exists.
//!
//! Inheriting `require_bearer` also inherits its `Provisioning::Enabled` JIT provisioning and
//! bootstrap-admin seeding, so this `GET` can create a principal row. That is true of every
//! protected IAM route and is not changed here, but it is a write on a read endpoint and is
//! called out rather than left to be discovered.

use axum::{Json, Router, extract::State, routing::get};
use paigasus_service_info::{ROUTE, ServiceInfoDto};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(ROUTE, get(get_service_info))
}

/// Always `200` for an authenticated caller. The body is the BARE `ServiceInfo` (SMA-499 D3),
/// and `capabilities` is always present — as `[]` when nothing is enabled.
async fn get_service_info(State(state): State<AppState>) -> Json<ServiceInfoDto> {
    Json(ServiceInfoDto::from(&state.capabilities.descriptor()))
}
