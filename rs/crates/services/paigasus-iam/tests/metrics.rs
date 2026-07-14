// SPDX-License-Identifier: Apache-2.0

//! Integration coverage for IAM's `/metrics` wiring (SMA-446 Unit 3) — mirrors
//! `paigasus-gateway`'s identical `tests/metrics.rs` (Unit 2): assembles the app the same way
//! `main.rs` does for the same-port merge (`[metrics] enabled = true`, `addr` unset):
//! `router(state).merge(paigasus_observability::metrics_router(handle))`. `router(state)` never
//! reads `IamConfig.metrics` itself (that decision lives entirely in `main.rs`, mirroring the
//! gateway's identical posture) — the disabled case is therefore `router(state)` alone, with
//! nothing merged in, exactly like `main.rs`'s `!config.metrics.enabled` branch.
//!
//! `paigasus_observability::init` installs a process-global recorder (`OnceLock`), so every test
//! in THIS binary shares one Prometheus registry — fine here since assertions only check that a
//! metric/label SUBSTRING appears (or is absent) in the rendered exposition, never an exact
//! count (each integration-test file in `tests/` is its own separate process/binary, so this is
//! isolated from every other suite).

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

#[tokio::test]
async fn metrics_route_returns_200_when_mounted() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let handle = paigasus_observability::init("test-iam-metrics-route");
    let app: Router = app.merge(paigasus_observability::metrics_router(handle));
    let resp = app.oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_route_is_404_when_metrics_disabled() {
    // Mirrors `main.rs`'s `!config.metrics.enabled` branch: `router(state)` alone, with no
    // `metrics_router` merged in — `/metrics` must not exist (and no recorder need be installed).
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let resp = app.oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn app_route_request_without_a_bearer_is_recorded_in_iam_http_requests_total() {
    // Bonus (brief: "driving a request that then shows iam_http_requests_total is a bonus if
    // cheap"): proves `http_metrics_layer("iam")` actually records an app-route request — the
    // 401 the bearer-enforcement middleware returns still flows back UP through the outer layer
    // (`app_routes`'s `.layer(http_metrics_layer(..))` is the LAST call, wrapping the whole
    // merged subtree including `protected`'s inner `route_layer(auth)`).
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let handle = paigasus_observability::init("test-iam-app-route-metrics");
    let app: Router = app.merge(paigasus_observability::metrics_router(handle.clone()));

    let resp = app.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no bearer supplied");

    let out = handle.render();
    assert!(out.contains("iam_http_requests_total"), "expected the http layer to record /v1/organizations:\n{out}");
    assert!(out.contains(r#"route="/v1/organizations""#), "route label should be the matched path:\n{out}");
    assert!(out.contains(r#"status_class="4xx""#), "expected the 401 to be recorded as 4xx:\n{out}");
}

#[tokio::test]
async fn healthz_and_readyz_are_excluded_from_iam_http_requests_total() {
    // The Unit 3 scrape-exclusion contract itself: `/healthz`/`/readyz` sit OUTSIDE
    // `app_routes`'s `http_metrics_layer` by construction (`router()` merges them in as their
    // own, separately-built `Router`s — see `health_router`/`readyz_router`'s docs) — so neither
    // gains a `route` label in `iam_http_requests_total`, mirroring the same exclusion
    // `serve_http` applies to `TraceLayer`/`TimeoutLayer` (a 15s Prometheus scrape or a
    // liveness/readiness poll must not spam a trace log, or inflate RED metrics, every tick).
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let handle = paigasus_observability::init("test-iam-health-excluded-from-metrics");
    let app: Router = app.merge(paigasus_observability::metrics_router(handle.clone()));

    for uri in ["/healthz", "/readyz"] {
        let resp = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    }

    let out = handle.render();
    assert!(!out.contains(r#"route="/healthz""#), "expected /healthz to be excluded from iam_http_requests_total:\n{out}");
    assert!(!out.contains(r#"route="/readyz""#), "expected /readyz to be excluded from iam_http_requests_total:\n{out}");
}
