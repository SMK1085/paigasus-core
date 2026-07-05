// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness) and `/readyz` (DB-backed readiness).

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

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
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed: database ping error");
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unready" })))
        }
    }
}

/// Serve the HTTP surface on `addr` until `shutdown` resolves.
pub async fn serve_http(addr: SocketAddr, state: AppState, request_timeout: Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    // `TimeoutLayer::new` is deprecated since tower-http 0.6.7 in favor of
    // `with_status_code`; `REQUEST_TIMEOUT` (408) reproduces `new`'s prior default.
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, request_timeout));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}
