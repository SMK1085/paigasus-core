// SPDX-License-Identifier: Apache-2.0

//! The deferred-router slot (SMA-571).
//!
//! `paigasus-iam` binds its sockets BEFORE it migrates, so for the whole migration window — and,
//! since SMA-559, for a lock-race loser's `migration.lock_wait_secs` wait — there is a live
//! listener with no `AppState` behind it. This module is what it serves: `/healthz` 200,
//! `/readyz` 503 `migrating`, and 503 `migrating` for every other path, swapped atomically for
//! the real routers once `AppState::new` returns.
//!
//! **Why one struct.** [`Serving`] carries the HTTP router, the gRPC routes AND the `AppState`
//! they were derived from, behind a single constructor. There is therefore no API that installs
//! a router without its state, which is how SMA-571 AC 4's "no window exists where the real
//! router is live but `AppState` is not" is satisfied — by the type, not by an ordering a future
//! edit could get wrong.
//!
//! **Why `OnceLock`, not `ArcSwapOption`.** The slot is written exactly once. `OnceLock` needs no
//! dependency, loads just as cheaply per request, and turns a double-install into a visible
//! `Err` rather than a silent replace.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::json;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;
use tower::{Layer, ServiceExt};

use crate::adapters::http::AppState;

/// Returned by [`BootSlot::install`] when the slot was already filled. A wiring defect rather
/// than an operator error: `install` has exactly one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyInstalled;

impl std::fmt::Display for AlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the boot slot was already installed")
    }
}

impl std::error::Error for AlreadyInstalled {}

/// Everything that only exists once the migration and `AppState::new` have completed.
///
/// Fields are private and [`Serving::new`] is the only constructor, so `http` and `state` are
/// necessarily derived from the SAME `AppState` — see the module doc.
pub struct Serving {
    http: Router,
    state: AppState,
}

impl Serving {
    /// Derives the full HTTP router from `state`. The router deliberately carries NO
    /// `/healthz`, `/readyz` or `/metrics` route: those live on [`boot_http_router`] permanently,
    /// outside `TraceLayer`/`TimeoutLayer`/`http_metrics_layer`, exactly as `serve_http` has
    /// always arranged them.
    /// `async` from the start even though this body awaits nothing yet: Task 3 adds the gRPC
    /// field, and `grpc::routes` is `async`. Declaring it now keeps the signature stable across
    /// tasks. An `async fn` with no `await` is not a warning.
    pub async fn new(state: AppState, request_timeout: Duration) -> Self {
        let http = crate::adapters::http::traced_app_routes(state.clone(), request_timeout);
        Self { http, state }
    }
}

/// The slot itself, plus the gRPC health reporter it flips.
///
/// Cheap to clone: an `Arc` and tonic-health's own `Arc`-backed handle.
#[derive(Clone)]
pub struct BootSlot {
    serving: Arc<OnceLock<Serving>>,
    reporter: HealthReporter,
}

impl BootSlot {
    pub fn new(reporter: HealthReporter) -> Self {
        Self {
            serving: Arc::new(OnceLock::new()),
            reporter,
        }
    }

    /// The ONLY way to become ready.
    ///
    /// `async` and reporter-owning deliberately: `HealthReporter::set_service_status` is
    /// `async`, so leaving the flip to a second call from `main.rs` would reintroduce exactly the
    /// two-step window the module doc says does not exist — and forgetting it would leave gRPC
    /// health `NOT_SERVING` after a SUCCESSFUL boot while `/readyz` answered 200, so nothing
    /// would notice.
    ///
    /// Ordered slot-first: a request arriving between the two sees a working service whose health
    /// has not yet flipped, which is safe. The reverse is not.
    pub async fn install(&self, serving: Serving) -> Result<(), AlreadyInstalled> {
        self.serving.set(serving).map_err(|_| AlreadyInstalled)?;
        self.reporter.set_service_status("", ServingStatus::Serving).await;
        Ok(())
    }

    pub(crate) fn get(&self) -> Option<&Serving> {
        self.serving.get()
    }
}

/// The router bound BEFORE the migration. Owns `/healthz`, `/readyz` and `/metrics` permanently;
/// everything else falls through to the slot.
///
/// `metrics` is the same same-port `/metrics` router `main.rs` used to hand `serve_http` — `None`
/// when metrics are disabled or served on their own `metrics.addr`.
pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router {
    // NOT layered with `CorrelationLayer` here (SMA-571 review round 1): the delegated arm of
    // `deferred_fallback` hands off to `serving.http`, which already carries its OWN
    // `CorrelationLayer` inside `app_routes` (`http/mod.rs`). Layering it again here would
    // double-apply it on every non-probe request for the process's entire post-migration
    // lifetime — see `deferred_fallback`'s doc for why that is a real bug, not a harmless
    // duplicate. The empty-slot 503 gets its OWN single application, scoped to just that arm,
    // inside `deferred_fallback`.
    let deferred = Router::new().fallback(deferred_fallback).with_state(slot.clone());
    let mut app = Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).with_state(slot).fallback_service(deferred);
    if let Some(metrics) = metrics {
        app = app.merge(metrics);
    }
    app
}

/// Liveness. Unconditional: the process is running, which is all liveness ever claimed.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({ "status": "ok" })))
}

/// Readiness. Three distinct bodies, which is SMA-571 AC 1: `migrating` (no schema yet),
/// `unready` (schema present, database ping failed), `ready`.
async fn readyz(State(slot): State<BootSlot>) -> impl IntoResponse {
    let Some(serving) = slot.get() else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "migrating" })));
    };
    crate::adapters::http::ping_readiness(&serving.state).await
}

/// Every non-probe path while the slot is empty; delegation once it is full.
///
/// **Exactly one `CorrelationLayer` application per arm, deliberately asymmetric** (SMA-571
/// review round 1): the `None` arm has no layer of its own anywhere else, so it gets one HERE,
/// scoped to just the bare 503 renderer via a one-off `tower::service_fn` run through `oneshot`.
/// The `Some` arm hands off to `serving.http` — `traced_app_routes` → `app_routes` — which
/// already applies `CorrelationLayer` inside `app_routes` (`http/mod.rs`); wrapping THIS
/// function's whole body in a second layer (the original, wrong shape) would have double-applied
/// it on that arm: `CorrelationLayer::call` mints a FRESH `request_id`/`correlation_id` and enters
/// a nested `tokio::task_local!` scope every time it runs, and the two nested applications
/// disagree — the handler/logs/audit trail observe the INNER (here: `app_routes`'s) ids, since
/// nested `task_local!` scopes shadow, but the outer layer unconditionally overwrites the
/// response headers with ITS OWN ids on the way out. Net effect: the id a caller receives on the
/// response would never match the id that was actually logged, which is exactly the
/// cross-service traceability guarantee (SMA-504) this header exists to provide.
async fn deferred_fallback(State(slot): State<BootSlot>, req: axum::extract::Request) -> axum::response::Response {
    match slot.get() {
        None => {
            let svc = paigasus_observability::CorrelationLayer.layer(tower::service_fn(migrating_response));
            svc.oneshot(req).await.expect("migrating_response is Infallible")
        }
        // `OnceLock::get` is taken ONCE here, at dispatch — so a request in flight across an
        // `install` completes against the value it started with (AC 4's third clause).
        // Unlayered: `serving.http` already carries its own `CorrelationLayer` — see doc above.
        Some(serving) => serving.http.clone().oneshot(req).await.into_response(),
    }
}

/// The bare empty-slot 503 body as a `tower::Service` fn, so [`deferred_fallback`]'s `None` arm
/// can run it through exactly one `CorrelationLayer` application via `oneshot` — see that
/// function's doc.
async fn migrating_response(_req: axum::extract::Request) -> Result<axum::response::Response, std::convert::Infallible> {
    Ok((StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "migrating" }))).into_response())
}
