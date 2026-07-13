// SPDX-License-Identifier: Apache-2.0

//! Shared observability plumbing for Paigasus services: a global `metrics`-facade Prometheus
//! recorder, a `GET /metrics` router, an axum request-metrics layer, a gRPC handler helper, and
//! the canonical metric-name registry. Mirrors `paigasus-logging`'s role for tracing.

pub mod grpc;
pub mod http;

pub use grpc::record_grpc;
pub use http::http_metrics_layer;

use std::sync::OnceLock;

use axum::{Router, http::header, response::IntoResponse, routing::get};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Default latency histogram buckets (seconds) for every `*_seconds` family.
const LATENCY_BUCKETS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder for `service` (once per process) and return the render
/// handle. A second in-process call returns a clone of the cached first handle — never a freshly
/// built, disconnected one (`install_recorder` succeeds at most once per process). `service` is
/// used only for the startup log line; the Prometheus scrape `job` label identifies the service.
pub fn init(service: &str) -> PrometheusHandle {
    HANDLE
        .get_or_init(|| {
            let handle = PrometheusBuilder::new()
                .set_buckets(LATENCY_BUCKETS)
                .expect("static non-empty buckets")
                .install_recorder()
                .expect("global metrics recorder installs once");
            tracing::info!(service, "metrics recorder installed");
            handle
        })
        .clone()
}

/// An axum router serving `GET /metrics` as Prometheus text exposition.
pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let body = handle.render();
            std::future::ready(([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response())
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics::counter;

    #[test]
    fn init_installs_recorder_and_second_call_returns_working_handle() {
        let h1 = init("test-svc");
        counter!("obs_test_init_counter").increment(1);
        assert!(h1.render().contains("obs_test_init_counter"), "first handle renders the metric");
        // Second call must NOT return a disconnected, empty handle (the install_recorder foot-gun).
        let h2 = init("test-svc");
        assert!(h2.render().contains("obs_test_init_counter"), "second handle still reflects the global recorder");
    }

    #[tokio::test]
    async fn metrics_router_returns_exposition() {
        use tower::ServiceExt;
        let handle = init("test-svc");
        counter!("obs_test_router_counter").increment(1);
        let app = metrics_router(handle);
        let resp = app.oneshot(axum::http::Request::builder().uri("/metrics").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()[axum::http::header::CONTENT_TYPE], "text/plain; version=0.0.4");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("obs_test_router_counter"));
    }
}
