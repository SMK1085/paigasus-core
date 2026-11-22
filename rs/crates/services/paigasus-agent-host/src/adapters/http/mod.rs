// SPDX-License-Identifier: Apache-2.0

//! The HTTP surface: `/healthz`, `/readyz`, the 404 envelope (spec §6.7), and, when IAM is
//! configured, the event ingress and `GET /v1/service-info` (spec §5).

pub mod auth;
pub mod bytes;
pub mod error;
pub mod ingress;
pub mod path;
pub mod service_info;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use paigasus_agent_host_core::ports::InboxRepository;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use sea_orm::DatabaseConnection;
use serde_json::json;

/// The IAM link and the limits of the ingress. It exists only when `[iam]` is configured.
pub struct Ingress {
    pub iam: Arc<dyn crate::adapters::iam::Iam>,
    pub max_body_bytes: usize,
}

/// The state of the routes. `db` serves the readiness probe. `ingress` is `None` when no `[iam]`
/// section is set: the two ingress routes are then not mounted, and they answer 404.
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub inboxes: Arc<dyn InboxRepository>,
    pub ingress: Option<Arc<Ingress>>,
    /// How the memory changes of a run commit. Chosen at boot from `memory.backend`.
    pub memory: crate::adapters::memory_commit::MemoryCommit,
}

/// The events route and the service-info route.
fn ingress_routes(inboxes: Arc<dyn InboxRepository>, ingress: Arc<Ingress>) -> Router {
    let state = ingress::IngressState { inboxes, ingress: ingress.clone() };
    // `route_layer`: the last layer added is the outermost. So `precheck` runs first, then the
    // authentication, then the handler under the body limit.
    let events = Router::new()
        .route("/v1/inboxes/{inbox}/events", post(ingress::post_event).layer(DefaultBodyLimit::max(ingress.max_body_bytes)))
        .with_state(state)
        .route_layer(from_fn_with_state(ingress.clone(), ingress::authenticate_middleware))
        .route_layer(from_fn_with_state(ingress.clone(), ingress::precheck));
    let info = Router::new()
        .route(paigasus_service_info::ROUTE, get(service_info::service_info))
        .route_layer(from_fn_with_state(ingress, service_info::authenticate_middleware));
    events.merge(info)
}

/// The routes of the service.
///
/// `/healthz` and `/readyz` stay outside the metrics layer and the correlation layer. Only the
/// rest of the routes, the 404 fallback included, has both. `/metrics` joins last, so a scrape
/// does not count in its own request metrics. Pass `None` when the boot router owns `/metrics`.
pub fn app_router(state: AppState, metrics: Option<Router>) -> Router {
    let mut api = Router::new();
    if let Some(ingress) = &state.ingress {
        api = api.merge(ingress_routes(state.inboxes.clone(), ingress.clone()));
    }
    let probes = Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).with_state(state);
    let api = api
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
    use crate::adapters::persistence::inbox_repository::PgInboxRepository;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn router() -> Router {
        app_router(
            AppState {
                db: DatabaseConnection::default(),
                inboxes: Arc::new(PgInboxRepository::new(DatabaseConnection::default())),
                ingress: None,
                memory: crate::adapters::memory_commit::MemoryCommit::InTransaction(crate::adapters::persistence::memory::PgMemoryStore::new(DatabaseConnection::default())),
            },
            None,
        )
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
