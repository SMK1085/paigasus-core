// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness) and `/readyz` (DB-backed readiness).

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
}

/// Liveness only — stateless, so it is testable without a database.
pub fn health_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

/// Full HTTP surface: liveness + DB-backed readiness.
pub fn router(state: AppState) -> Router {
    health_router().merge(Router::new().route("/readyz", get(readyz)).with_state(state))
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ping = state.db.execute(Statement::from_string(state.db.get_database_backend(), "SELECT 1")).await;
    match ping {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unready" }))),
    }
}
