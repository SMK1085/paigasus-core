// SPDX-License-Identifier: Apache-2.0

//! The deferred-router slot (SMA-571).
//!
//! `paigasus-iam` binds its sockets BEFORE it migrates, so for the whole migration window — and,
//! since SMA-559, for a lock-race loser's `migration.lock_wait_secs` wait — there is a live
//! listener with no `AppState` behind it. This module is what it serves: `/healthz` 200,
//! `/readyz` 503 `{"status":"migrating"}`, and — for every other path — a 503 carrying the house
//! error envelope, `{"error":{"code":"service-migrating",…}}`, swapped atomically for the real
//! routers once `AppState::new` returns. gRPC's deferred fallback carries the SAME registered
//! reason as `ErrorInfo` (via [`convert::iam_status`]) rather than a bare `UNAVAILABLE` — see
//! [`deferred_grpc_fallback`] — so a client cannot see two different machine-readable pictures of
//! the identical condition depending on which transport it used.
//!
//! **Why the two bodies differ.** The probes are not part of the API surface: they sit outside
//! `CorrelationLayer` and the metrics layer, and `/readyz`'s three `status` values
//! (`migrating`/`unready`/`ready`) ARE SMA-571 AC 1's "a body distinguishing migrating from a
//! failed database ping". Everything else is a `/v1/*` route, where SMA-587 made the registered
//! error envelope the single shape every error takes — so the deferred 503 must speak it too.
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
use paigasus_observability::Retryable;
use serde_json::json;
use tonic::Code;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;
use tower::{Layer, ServiceExt};

use crate::adapters::grpc::convert;
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
    grpc: crate::adapters::grpc::authn::AuthEnforce<tonic::service::Routes>,
    state: AppState,
}

impl Serving {
    /// Derives the full HTTP and gRPC routers from `state`. Neither carries the boot-time
    /// probes/health: `/healthz`, `/readyz` and `/metrics` live on [`boot_http_router`]
    /// permanently, and gRPC health lives on [`boot_grpc_routes`] — both outlive `Serving` and
    /// keep answering across a future re-migration, which `Serving` itself is not built to
    /// survive.
    pub async fn new(state: AppState, request_timeout: Duration) -> Self {
        let http = crate::adapters::http::traced_app_routes(state.clone(), request_timeout);
        // `AuthLayer` moves HERE from the tonic `Server`'s layer stack (see `grpc::router`),
        // because it needs `AppState` and the boot-time server has none. Behaviour is
        // unchanged: health and the two introspect RPCs are `:path`-exempt
        // (`grpc::authn::is_exempt`) and every non-exempt RPC lives inside these routes.
        let grpc = crate::adapters::grpc::authn::AuthLayer::new(state.clone()).layer(crate::adapters::grpc::routes(state.clone()).await);
        Self { http, grpc, state }
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

/// The registered reason this module's deferred-phase failures carry, and its static,
/// caller-safe message — shared by the HTTP app-route 503 ([`migrating_response`]) AND the gRPC
/// fallback ([`deferred_grpc_fallback`]), so the two transports cannot drift onto different wire
/// strings for the identical condition. Kept next to the renderer rather than inline so the
/// membership test below can assert the code against the canonical registry without restating
/// the literal.
///
/// The code lives here as a literal, not as `ErrorReason::ServiceMigrating.as_wire_reason()`,
/// for the same reason `http/json.rs` carries literals: this module is the ONLY thing serving
/// during the deferred phase, so the response must be renderable with no fallible step in it.
/// `as_wire_reason` returns `Option`, and the only honest handling of a `None` here would be a
/// panic inside the one router that is supposed to survive a broken boot. The membership test is
/// what keeps the literal honest, and `repo:error-code-single-site` is what keeps the membership
/// test from being deleted.
const MIGRATING: (&str, &str) = ("service-migrating", "this replica is still applying its boot migration");

/// The empty-slot 503 body as a `tower::Service` fn, so [`deferred_fallback`]'s `None` arm can
/// run it through exactly one `CorrelationLayer` application via `oneshot` — see that function's
/// doc.
///
/// **Why the error envelope and not `{"status":"migrating"}`** (SMA-587 follow-up). This answers
/// every `/v1/*` path, so it is part of the API surface, and SMA-587 made
/// `{"error":{"code","message"}}` with a REGISTERED reason the one shape every error on those
/// routes takes. A bespoke `status` body here would be the only response on `/v1/organizations` a
/// client could not parse with its normal error decoder — and the deferred phase is precisely
/// when a client most needs to read the code and decide to retry.
///
/// The probes are deliberately NOT changed with it: `/healthz` and `/readyz` sit outside
/// `CorrelationLayer` and the metrics layer, are not part of the API surface, and `/readyz`'s
/// `migrating` vs `unready` vs `ready` bodies ARE SMA-571 AC 1. See [`readyz`].
///
/// No `paigasus-retryable` header is stamped here on purpose: `CorrelationLayer` fills one in
/// for any error response that lacks it (`correlation.rs`'s `Retryable::from_status`), and a 503
/// maps to `"true"` — which is exactly right and is pinned by `tests/boot_deferred.rs`. Stamping
/// it here would only move the decision without changing it.
async fn migrating_response(_req: axum::extract::Request) -> Result<axum::response::Response, std::convert::Infallible> {
    let (code, message) = MIGRATING;
    Ok((StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "error": { "code": code, "message": message } }))).into_response())
}

/// The gRPC routes bound BEFORE the migration: the health service (reporting whatever
/// `slot`'s reporter says — `NOT_SERVING` until [`BootSlot::install`] flips it) plus a catch-all
/// that answers a well-formed `UNAVAILABLE` while the slot is empty and delegates to the real,
/// `AuthLayer`-wrapped routes afterwards.
///
/// **Why a nested `Router<BootSlot>`, not `.fallback()` chained straight onto
/// `into_axum_router()`.** `Routes::into_axum_router` hands back an `axum::Router<()>` — its
/// state is fixed at `()`. [`deferred_grpc_fallback`] needs `State<BootSlot>`, which only
/// type-checks against a router whose state IS `BootSlot`. So the fallback is built as its own
/// small `Router<BootSlot>`, `with_state`'d down to `Router<()>` (exactly the
/// `deferred`/`app` split [`boot_http_router`] already uses for the HTTP side), and attached via
/// `fallback_service` rather than `fallback`.
///
/// **Why the override must exist at all.** `Routes::default()`'s own fallback answers
/// `UNIMPLEMENTED` — and `paigasus-gateway`'s readiness probe
/// (`gateway/src/adapters/http/mod.rs:146-150`) reads `UNIMPLEMENTED` as READY. Left in place, a
/// migrating replica would look ready to the gateway: AC 2 broken, while looking like a fix (see
/// `the_grpc_fallback_is_a_wellformed_unavailable_not_unimplemented`).
pub fn boot_grpc_routes<H>(slot: BootSlot, health: tonic_health::pb::health_server::HealthServer<H>) -> tonic::service::Routes
where
    H: tonic_health::pb::health_server::Health,
{
    // Health is mounted HERE, on the boot routes, so it answers during the deferred phase and
    // reports whatever `BootSlot`'s reporter says. Note `grpc::routes(state)` mounts a health
    // service of its own with its own (dropped, statically SERVING) reporter — in production
    // that one is unreachable, because a health request matches THIS route and never falls
    // through to the fallback. `router()` still needs its own, which is why `routes()` keeps it.
    let routes = tonic::service::Routes::new(health);
    let deferred = Router::new().fallback(deferred_grpc_fallback).with_state(slot);
    let inner = routes.into_axum_router().fallback_service(deferred);
    tonic::service::Routes::from(inner)
}

/// Every gRPC RPC that isn't the boot-time health service, while the slot is empty vs. once it
/// is full. The gRPC counterpart of [`deferred_fallback`].
async fn deferred_grpc_fallback(State(slot): State<BootSlot>, req: axum::extract::Request) -> axum::response::Response {
    match slot.get() {
        // HTTP 200 + `content-type: application/grpc` + `grpc-status: 14`, via
        // `Status::into_http` — a bare 503 is not a gRPC status and no client can interpret it.
        // Mirrors `grpc::authn::reject`.
        //
        // Built through `convert::iam_status` — the same single construction point every OTHER
        // IAM gRPC error goes through (`grpc/convert.rs`'s module doc: "no site can forget the
        // details") — carrying the SAME registered `MIGRATING` reason the HTTP fallback puts in
        // its error envelope, so the two transports report one machine-readable picture of the
        // same condition rather than an HTTP client getting a decodable code and a gRPC client
        // getting nothing. `Code::Unavailable` is non-negotiable: `paigasus-gateway`'s readiness
        // probe (`gateway/src/adapters/http/mod.rs:146-150`) classifies `Unavailable` as NOT
        // ready and everything else — `Unimplemented` included — as ready, and `ErrorDetails`
        // rides in a trailer/header that classification never inspects, so attaching it cannot
        // change which arm the gateway takes.
        None => {
            let (reason, message) = MIGRATING;
            convert::iam_status(Code::Unavailable, reason, message, Retryable::Yes, &[]).into_http::<axum::body::Body>()
        }
        Some(serving) => {
            // `AuthEnforce` is a `Service<Request<tonic::body::Body>>` while axum hands us
            // `Request<axum::body::Body>` — two distinct body types. Mirrors
            // `Routes::add_service`'s own `map_request` (tonic-0.14.6/src/service/router.rs:91).
            let req = req.map(tonic::body::Body::new);
            serving.grpc.clone().oneshot(req).await.expect("AuthEnforce is Infallible").map(axum::body::Body::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `repo:error-code-single-site`'s required membership test for this file: every code this
    /// module can put on the wire must be declared in
    /// `contracts/proto/paigasus/common/v1/error.proto`, so a typo in [`MIGRATING`] fails here
    /// rather than shipping a code no consumer can resolve.
    ///
    /// Driven off the constant rather than a restated literal — the SMA-507 E3 lesson that
    /// `http/json.rs` records: a hand-copied list lets an edit escape both this test and the
    /// gate. There is exactly one code here, so no iterator is needed, but the assertion still
    /// reads the same value the renderer does.
    #[test]
    fn the_deferred_fallback_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let (code, _message) = MIGRATING;
        assert_eq!(
            ErrorReason::from_wire_reason(code),
            Some(ErrorReason::ServiceMigrating),
            "{code} is not declared in common/v1/error.proto"
        );
    }

    /// The renderer itself, so the envelope SHAPE is pinned in the crate that owns it rather
    /// than only in the integration suites. `/readyz`'s `{"status":…}` body is deliberately NOT
    /// this shape — see [`readyz`] and SMA-571 AC 1 — so an edit that "unified" the two would
    /// have to delete an assertion here to pass.
    #[tokio::test]
    async fn the_app_route_fallback_renders_the_house_error_envelope() {
        let req = axum::extract::Request::builder().uri("/v1/organizations").body(axum::body::Body::empty()).unwrap();
        let resp = migrating_response(req).await.expect("Infallible");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let (code, message) = MIGRATING;
        assert_eq!(body["error"]["code"], code);
        assert_eq!(body["error"]["message"], message);
        let keys: std::collections::BTreeSet<&str> = body["error"].as_object().expect("an object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["code", "message"].into_iter().collect::<std::collections::BTreeSet<_>>(),
            "the error object's key set must match every other IAM error body"
        );
    }
}
