// SPDX-License-Identifier: Apache-2.0

//! Integration coverage for the gateway's `/metrics` wiring (Task A6): the same-port merge
//! matches how `main.rs` assembles the app when `[metrics] enabled = true, addr` is unset —
//! `router(state).merge(paigasus_observability::metrics_router(handle))`.
//!
//! `paigasus_observability::init` installs a process-global recorder (`OnceLock`), so every test
//! in THIS binary shares one Prometheus registry — fine here since assertions only check that a
//! metric/label SUBSTRING appears in the rendered exposition, never an exact count (each
//! integration-test file in `tests/` is its own separate process/binary, so this is isolated from
//! `chat_proxy.rs`/`openai_egress.rs`).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use secrecy::SecretString;
use tower::ServiceExt; // for `oneshot`

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{Iam, IamError};
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_proto::paigasus::iam::v1::IntrospectApiKeyResponse;

/// A never-invoked `Iam` — these tests never exercise the protected chat route.
struct UnusedIam;

#[async_trait::async_trait]
impl Iam for UnusedIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        unreachable!("these tests never drive the protected route")
    }
    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        unreachable!("these tests never drive the protected route")
    }
}

/// An `OpenAiClient` pointed nowhere in particular — the health/`/metrics` routes never call it.
fn unused_openai() -> OpenAiClient {
    let cfg = OpenAiConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: SecretString::from("sk-unused".to_string()),
    };
    OpenAiClient::new(&cfg, Duration::from_secs(1), Duration::from_secs(1), Duration::from_secs(1)).expect("client builds")
}

fn unused_state() -> AppState {
    AppState {
        iam: Arc::new(UnusedIam),
        openai: Arc::new(unused_openai()),
        max_request_bytes: 1_048_576,
    }
}

#[tokio::test]
async fn metrics_route_returns_200_when_mounted() {
    let handle = paigasus_observability::init("test-gateway-metrics-route");
    let app: Router = router(unused_state()).merge(paigasus_observability::metrics_router(handle));
    let resp = app.oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_route_is_404_when_metrics_disabled() {
    // Mirrors `main.rs`'s `!config.metrics.enabled` branch: `router(state)` alone, with no
    // `metrics_router` merged in — `/metrics` must not exist (and no recorder need be installed).
    let app = router(unused_state());
    let resp = app.oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn healthz_request_is_recorded_in_http_requests_total() {
    let handle = paigasus_observability::init("test-gateway-healthz-metrics");
    let app: Router = router(unused_state()).merge(paigasus_observability::metrics_router(handle.clone()));
    let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let out = handle.render();
    assert!(out.contains("gateway_http_requests_total"), "expected the http layer to record /healthz:\n{out}");
    assert!(out.contains("route=\"/healthz\""), "route label should be the matched path:\n{out}");
}
