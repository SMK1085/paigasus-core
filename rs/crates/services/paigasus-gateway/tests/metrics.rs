// SPDX-License-Identifier: Apache-2.0

//! Integration coverage for the gateway's `/metrics` wiring (Task A6) and the IAM/upstream call
//! metrics recorded at the auth + chat instrumentation sites (Task A7).
//!
//! Assembles the app the same way `main.rs` does when `[metrics] enabled = true, addr` is unset
//! (the same-port merge): `router(state).merge(paigasus_observability::metrics_router(handle))`.
//! `paigasus_observability::init` installs a process-global recorder (`OnceLock`), so every test
//! in THIS binary shares one Prometheus registry — fine here since assertions only check that a
//! metric/label SUBSTRING appears in the rendered exposition, never an exact count (each
//! integration-test file in `tests/` is its own separate process/binary, so this is isolated from
//! `chat_proxy.rs`/`openai_egress.rs`).

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use secrecy::SecretString;
use tower::ServiceExt; // for `oneshot`

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{Iam, IamError};
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_proto::paigasus::iam::v1::IntrospectApiKeyResponse;
use support::MockOpenAi;

const CALLER_KEY: &str = "sk-caller-secret";
const CALLER_SA: &str = "prn:paigasus:iam:default:sa/gw-caller";
const CALLER_SCOPE: &str = "prn:paigasus:iam:default:scope/team-a";
const CALLER_KEY_ID: &str = "key-abc123";

const NON_STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;

/// A never-invoked `Iam` — the `/metrics`/`/healthz` tests never drive the protected route.
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

/// A canned `Iam` that always introspects an active caller and authorizes the self-query — for
/// the A7 proxied-request test, which DOES drive the protected route.
struct AllowedIam;

#[async_trait::async_trait]
impl Iam for AllowedIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        Ok(IntrospectApiKeyResponse {
            principal_prn: CALLER_SA.to_owned(),
            status: "active".to_owned(),
            key_id: CALLER_KEY_ID.to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
            scope_prn: CALLER_SCOPE.to_owned(),
        })
    }

    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(true)
    }
}

/// An `OpenAiClient` pointed nowhere in particular — used only where the upstream is never called.
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

// ---- Task A6: `/metrics` wiring + HTTP request metrics ----------------------------------------

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

// ---- Task A7: IAM-call + upstream instrumentation ----------------------------------------------

#[tokio::test]
async fn successful_proxied_request_records_iam_and_upstream_metrics() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let cfg = OpenAiConfig {
        base_url: mock.base_url.clone(),
        api_key: SecretString::from("sk-real-openai-key".to_string()),
    };
    let openai = OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds");

    let handle = paigasus_observability::init("test-gateway-proxy-metrics");
    let state = AppState {
        iam: Arc::new(AllowedIam),
        openai: Arc::new(openai),
        max_request_bytes: 1_048_576,
    };
    let app: Router = router(state).merge(paigasus_observability::metrics_router(handle.clone()));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(NON_STREAM_BODY))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let out = handle.render();
    assert!(out.contains("gateway_iam_calls_total"), "expected an IAM-call counter:\n{out}");
    assert!(out.contains("operation=\"introspect\""), "expected an introspect-labeled IAM call:\n{out}");
    assert!(out.contains("operation=\"authorize\""), "expected an authorize-labeled IAM call:\n{out}");
    assert!(out.contains("gateway_iam_call_duration_seconds"), "expected the IAM-call duration histogram:\n{out}");
    assert!(out.contains("gateway_upstream_requests_total"), "expected an upstream-call counter:\n{out}");
    assert!(out.contains("gateway_upstream_request_duration_seconds"), "expected the upstream-call duration histogram:\n{out}");
    assert!(out.contains(r#"status_class="2xx""#), "expected the upstream call's status_class to be 2xx:\n{out}");
}
