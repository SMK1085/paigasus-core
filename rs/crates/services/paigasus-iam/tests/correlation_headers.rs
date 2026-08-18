// SPDX-License-Identifier: Apache-2.0

//! SMA-504 D10: the correlation layer attaches exactly where `http_metrics_layer` attaches —
//! around `app_routes` — so the `oneshot` harness exercises it and the operational endpoints
//! stay outside it.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const REQUEST_ID: &str = "paigasus-request-id";
const CORRELATION_ID: &str = "paigasus-correlation-id";

#[tokio::test]
async fn an_auth_rejected_request_still_carries_both_ids() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    // No Authorization header: the bearer layer rejects with 401 BEFORE any handler runs. If the
    // correlation layer were inside that middleware rather than outside it, this response would
    // carry no ids — which is the whole point of the assertion.
    let resp = app.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(REQUEST_ID), "a rejected request must still be attributable");
    assert!(resp.headers().contains_key(CORRELATION_ID));
}

/// D10: `/healthz` and `/readyz` are operational endpoints, deliberately outside every layer
/// (`readyz_router` is merged at the top level). Pinned so the narrowing is a decision rather
/// than an accident someone later "fixes". `/readyz` is the more interesting of the two — it is
/// merged in via its OWN `readyz_router`, a separate function from `/healthz`'s, so this is not
/// just re-testing the same code path under a different path string.
#[tokio::test]
async fn the_operational_endpoints_carry_no_ids() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let resp = app.clone().oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!resp.headers().contains_key(REQUEST_ID), "/healthz is outside the API surface (D10)");
    assert!(!resp.headers().contains_key(CORRELATION_ID), "/healthz is outside the API surface (D10)");

    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "readyz_router's DB ping must succeed against the migrated test database");
    assert!(!resp.headers().contains_key(REQUEST_ID), "/readyz is merged via its own readyz_router, outside the API surface (D10)");
    assert!(
        !resp.headers().contains_key(CORRELATION_ID),
        "/readyz is merged via its own readyz_router, outside the API surface (D10)"
    );
}
