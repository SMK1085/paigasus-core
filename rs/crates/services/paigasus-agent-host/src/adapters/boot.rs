// SPDX-License-Identifier: Apache-2.0

//! The boot router and its slot.
//!
//! The service binds its listener after it connects to the database and before it migrates. For
//! the whole migration window, and for the lock wait of a replica that lost the lock race, a live
//! listener has no application behind it. This module is what that listener serves: `/healthz`
//! answers 200, `/readyz` answers 503 `{"status":"migrating"}`, and every other path answers 503
//! in the error envelope with the registered code `service-migrating`.
//!
//! [`BootSlot::install`] fills the slot one time with the real router. The slot is an
//! `OnceLock`: the second install returns an error and does not replace the router.

use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use tower::{Layer, ServiceExt};

/// Returned by [`BootSlot::install`] when the slot already holds a router. This is a wiring
/// defect: `install` has one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyInstalled;

impl std::fmt::Display for AlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the boot slot was already installed")
    }
}

impl std::error::Error for AlreadyInstalled {}

/// The slot that holds the application router after the migration. A clone shares the slot.
#[derive(Clone, Default)]
pub struct BootSlot {
    app: Arc<OnceLock<Router>>,
}

impl BootSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills the slot. This is the only way to become ready.
    pub fn install(&self, app: Router) -> Result<(), AlreadyInstalled> {
        self.app.set(app).map_err(|_| AlreadyInstalled)
    }

    fn get(&self) -> Option<&Router> {
        self.app.get()
    }
}

/// The router that the listener serves for the life of the process.
///
/// It owns `/healthz` and, while the slot is empty, `/readyz`. Every other request goes to the
/// installed router, or gets the 503 envelope while the slot is empty. `metrics` is the same-port
/// `/metrics` router, or `None` when metrics are off or have their own listener.
pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router {
    let mut app = Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).fallback(deferred).with_state(slot);
    if let Some(metrics) = metrics {
        app = app.merge(metrics);
    }
    app
}

/// Liveness. The process runs, and that is all this probe states.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({ "status": "ok" })))
}

/// While the slot is empty, 503 `migrating`. After the install, the answer of the installed router.
async fn readyz(State(slot): State<BootSlot>, req: Request) -> Response {
    match slot.get() {
        None => (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "migrating" }))).into_response(),
        Some(app) => app.clone().oneshot(req).await.into_response(),
    }
}

/// Every path except the probes. The installed router has its own correlation layer, so only the
/// empty-slot answer gets one here.
async fn deferred(State(slot): State<BootSlot>, req: Request) -> Response {
    match slot.get() {
        None => {
            let svc = paigasus_observability::CorrelationLayer.layer(tower::service_fn(migrating_response));
            svc.oneshot(req).await.expect("migrating_response is Infallible")
        }
        Some(app) => app.clone().oneshot(req).await.into_response(),
    }
}

/// The registered code of the 503 while migrations run, and its static message. The membership
/// test below checks the code against the registry. The code is a literal because this router is
/// all that serves during the migration, so no fallible step may stand in its way.
const MIGRATING: (&str, &str) = ("service-migrating", "this replica is still applying its boot migration");

async fn migrating_response(_req: Request) -> Result<Response, std::convert::Infallible> {
    let (code, message) = MIGRATING;
    Ok((StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "error": { "code": code, "message": message } }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::http::{AppState, app_router, error};
    use axum::body::Body;
    use sea_orm::DatabaseConnection;

    async fn call(router: &Router, path: &str) -> (StatusCode, serde_json::Value) {
        let response = router.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn while_migrating_healthz_is_200_and_readyz_503() {
        let router = boot_http_router(BootSlot::new(), None);
        assert_eq!(call(&router, "/healthz").await.0, StatusCode::OK);
        let (status, body) = call(&router, "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, json!({ "status": "migrating" }));
    }

    #[tokio::test]
    async fn while_migrating_another_path_is_503_service_migrating() {
        let router = boot_http_router(BootSlot::new(), None);
        let (status, body) = call(&router, "/anything").await;
        let (code, _message) = MIGRATING;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], code);
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn after_install_the_app_router_answers() {
        let slot = BootSlot::new();
        let router = boot_http_router(slot.clone(), None);
        slot.install(app_router(AppState { db: DatabaseConnection::default() }, None)).unwrap();
        let (status, body) = call(&router, "/anything").await;
        let (code, _message) = error::NOT_FOUND;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], code);
        // The installed router answers `/readyz` too. The database is not connected here.
        assert_eq!(call(&router, "/readyz").await.1, json!({ "status": "unready" }));
    }

    #[test]
    fn a_second_install_is_rejected() {
        let slot = BootSlot::new();
        slot.install(Router::new()).unwrap();
        assert_eq!(slot.install(Router::new()), Err(AlreadyInstalled));
    }

    #[test]
    fn the_migrating_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let (code, _message) = MIGRATING;
        assert_eq!(
            ErrorReason::from_wire_reason(code),
            Some(ErrorReason::ServiceMigrating),
            "{code} is not declared in common/v1/error.proto"
        );
    }
}
