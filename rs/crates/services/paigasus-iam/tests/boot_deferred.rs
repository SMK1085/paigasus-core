// SPDX-License-Identifier: Apache-2.0

//! SMA-571: what the HTTP surface answers while the slot is EMPTY — i.e. for the whole window
//! between the bind and the end of the migration. These need no `AppState` and therefore no
//! Docker: an empty slot has no payload to build. The delegation half (a FULL slot) needs a real
//! `AppState` and lives in `boot_install_pg.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, boot_grpc_routes, boot_http_router};
use tower::ServiceExt; // for `oneshot`

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn empty_slot_router() -> axum::Router {
    let (reporter, _health) = paigasus_iam::adapters::grpc::health_service().await;
    boot_http_router(BootSlot::new(reporter), None)
}

/// AC 1's first half: liveness answers even though nothing is migrated.
#[tokio::test]
async fn healthz_is_200_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// AC 1's second half. `migrating` must be distinguishable from `unready` (the DB-ping failure
/// body, exercised in `boot_install_pg.rs`) — that distinction IS the acceptance criterion.
#[tokio::test]
async fn readyz_is_503_migrating_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(resp).await["status"], "migrating");
}

/// AC 2. The two wrong answers are called out by name because both are plausible bugs: a 404
/// means the fallback was never attached, a 401 means the real router leaked through and the
/// bearer layer answered — and a caller would read either as "this replica is up".
#[tokio::test]
async fn an_app_route_is_503_migrating_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an app route must 503 while migrating — NOT 404 (fallback missing) and NOT 401 (real router leaked)"
    );
    assert_eq!(body_json(resp).await["status"], "migrating");
}

/// SMA-504's cross-service contract: the deferred 503 is exactly the response a caller most wants
/// to retry, so it must carry correlation ids. `/healthz` and `/readyz` must NOT — that is pinned
/// by `tests/correlation_headers.rs:42-52` and this asserts the other side of the same line.
#[tokio::test]
async fn the_deferred_503_carries_correlation_headers_but_the_probes_do_not() {
    let app = empty_slot_router().await;
    let resp = app.clone().oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert!(resp.headers().contains_key("paigasus-request-id"), "the deferred fallback is inside CorrelationLayer");
    // SMA-571 Task 3 review carry-over: `CorrelationLayer` fills in `paigasus-retryable` for any
    // error response no renderer already stamped one on (`correlation.rs`'s `Retryable::from_status`),
    // and nothing pinned it here before. A 503 maps to `"true"` — this IS the response a caller
    // most wants to retry, so getting this wrong would be a silent regression.
    assert_eq!(resp.headers()["paigasus-retryable"], "true", "a 503 is retryable and no renderer here supplies the header itself");

    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert!(!resp.headers().contains_key("paigasus-request-id"), "/readyz stays outside the API surface (D10)");
}

/// SMA-571 D7 — the single most consequential assertion in this change.
///
/// `paigasus-gateway`'s readiness probe classifies IAM's gRPC replies
/// (`gateway/src/adapters/http/mod.rs:146-150`): `Unavailable`/`DeadlineExceeded`/`Internal` mean
/// NOT ready, and **anything else — including `Unimplemented` — means READY**. `Routes::default()`
/// installs an `unimplemented` fallback, so a boot router that merely mounts health would make the
/// gateway report ready against a migrating IAM: AC 2 broken, and it would look like a fix.
///
/// A bare HTTP 503 is equally wrong — no gRPC client can interpret it. So this asserts the wire
/// shape, not a status code.
#[tokio::test]
async fn the_grpc_fallback_is_a_wellformed_unavailable_not_unimplemented() {
    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    let routes = boot_grpc_routes(BootSlot::new(reporter), health);

    let req = Request::builder()
        .method("POST")
        .uri("/paigasus.iam.v1.TenancyService/CreateOrganization")
        .header("content-type", "application/grpc")
        .body(tonic::body::Body::empty())
        .unwrap();
    let resp = routes.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "a gRPC status rides on HTTP 200, never a bare 503");
    assert_eq!(resp.headers()["content-type"], "application/grpc");
    assert_eq!(
        resp.headers()["grpc-status"],
        "14",
        "must be UNAVAILABLE (14). UNIMPLEMENTED (12) is Routes::default()'s own fallback and the \
         gateway reads it as READY — see gateway/src/adapters/http/mod.rs:150"
    );
}

/// Health must be ROUTED to the health service during the deferred phase, not swallowed by the
/// migrating fallback — delete the health mount and this returns `grpc-status: 14` and fails.
///
/// **What this does NOT check** (SMA-571 final review): it asserts only that `grpc-status` is
/// not `"14"`, so it passes identically whether the reporter reports `NOT_SERVING` or `SERVING`.
/// The real status-VALUE decode — proving the response is actually `NOT_SERVING` while migrating
/// and flips to `SERVING` once installed — lives in `boot_lifecycle_pg.rs` against the spawned
/// process, where a genuine migrate -> ready transition exists to observe.
#[tokio::test]
async fn grpc_health_is_routed_to_the_health_service_not_the_migrating_fallback() {
    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await;
    let routes = boot_grpc_routes(BootSlot::new(reporter.clone()), health);

    let req = Request::builder()
        .method("POST")
        .uri("/grpc.health.v1.Health/Check")
        .header("content-type", "application/grpc")
        .body(tonic::body::Body::empty())
        .unwrap();
    let resp = routes.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_ne!(
        resp.headers().get("grpc-status").map(|v| v.to_str().unwrap()),
        Some("14"),
        "health must be served by the boot routes, not swallowed by the migrating fallback"
    );
}
