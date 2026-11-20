// SPDX-License-Identifier: Apache-2.0

//! The HTTP surface: `/healthz`, `/readyz` and the 404 envelope (spec §6.7).

pub mod error;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use sea_orm::DatabaseConnection;
use serde_json::json;

/// The state of the routes. It holds the database connection for the readiness probe.
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
}

/// The routes of the service.
///
/// `/healthz` and `/readyz` stay outside the metrics layer and the correlation layer. Only the
/// rest of the routes, the 404 fallback included, has both. `/metrics` joins last, so a scrape
/// does not count in its own request metrics. Pass `None` when the boot router owns `/metrics`.
pub fn app_router(state: AppState, metrics: Option<Router>) -> Router {
    let probes = Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).with_state(state);
    let api = Router::new()
        .fallback(error::not_found)
        .layer(paigasus_observability::http_metrics_layer("agent_host"))
        .layer(paigasus_observability::CorrelationLayer);
    let mut app = probes.merge(api);
    if let Some(metrics) = metrics {
        app = app.merge(metrics);
    }
    app
}

/// Liveness. The process runs, and that is all this probe states.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({ "status": "ok" })))
}

/// Readiness. The database must answer a ping.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(()) => (StatusCode::OK, axum::Json(json!({ "status": "ok" }))),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed: database ping error");
            (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "unready" })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn router() -> Router {
        app_router(AppState { db: DatabaseConnection::default() }, None)
    }

    async fn get_path(path: &str) -> (StatusCode, serde_json::Value) {
        let response = router().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn healthz_is_200_always() {
        let (status, body) = get_path("/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn readyz_is_503_when_the_database_ping_fails() {
        let (status, body) = get_path("/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, json!({ "status": "unready" }));
    }

    #[tokio::test]
    async fn an_unknown_path_is_404_in_the_error_envelope() {
        let (status, body) = get_path("/nope").await;
        let (code, _message) = error::NOT_FOUND;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], code);
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
        assert_eq!(body.as_object().unwrap().len(), 1);
    }
}
