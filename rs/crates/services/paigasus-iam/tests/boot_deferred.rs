// SPDX-License-Identifier: Apache-2.0

//! SMA-571: what the HTTP surface answers while the slot is EMPTY — i.e. for the whole window
//! between the bind and the end of the migration. These need no `AppState` and therefore no
//! Docker: an empty slot has no payload to build. The delegation half (a FULL slot) needs a real
//! `AppState` and lives in `boot_install_pg.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, boot_http_router};
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

    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert!(!resp.headers().contains_key("paigasus-request-id"), "/readyz stays outside the API surface (D10)");
}
