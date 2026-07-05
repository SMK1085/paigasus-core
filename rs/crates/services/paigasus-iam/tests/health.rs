// SPDX-License-Identifier: Apache-2.0

//! HTTP liveness smoke test — `/healthz` returns 200 without a database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::http::health_router;
use tower::ServiceExt; // for `oneshot`

#[tokio::test]
async fn healthz_returns_200() {
    let app = health_router();
    let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
