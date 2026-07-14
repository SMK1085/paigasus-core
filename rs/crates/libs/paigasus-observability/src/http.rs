// SPDX-License-Identifier: Apache-2.0
//! Shared axum request-metrics middleware.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use metrics::{counter, gauge, histogram};

/// The boxed future the middleware closure returns — factored out to keep the `http_metrics_layer`
/// return type below `clippy::type_complexity`'s threshold.
type BoxFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;

/// RAII guard that decrements an in-flight gauge on drop — including during a panic unwind — so a
/// handler (or inner layer) that panics can never leak the gauge permanently.
struct InflightGuard(metrics::Gauge);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.decrement(1.0);
    }
}

/// Maps an HTTP method to a bounded label value. HTTP permits arbitrary extension methods, so
/// anything outside the standard verb set collapses to `"OTHER"` to keep the label's cardinality
/// bounded.
fn method_label(method: &axum::http::Method) -> &'static str {
    use axum::http::Method;
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        Method::TRACE => "TRACE",
        Method::CONNECT => "CONNECT",
        _ => "OTHER",
    }
}

/// axum middleware recording request count (by route/method/status_class), duration, and an
/// in-flight gauge. `route` is the MatchedPath template (bounded); an unmatched request collapses
/// to `<unmatched>`. `prefix` is the service metric prefix (e.g. `"gateway"`, `"iam"`).
///
/// Attach downstream with `router.layer(http_metrics_layer("gateway"))`.
pub fn http_metrics_layer(prefix: &'static str) -> middleware::FromFnLayer<impl Fn(Request, Next) -> BoxFuture + Clone, (), (Request,)> {
    // The three metric NAMES are fixed once `prefix` is chosen at layer-construction time — build
    // them here, once, rather than re-running `format!` on every single request (the hot path).
    // The closure below is `Fn` (invoked once per request), so each invocation still needs its own
    // owned `String`s to move into the `async move` block; cloning three small strings per request
    // is far cheaper than the `format!` machinery (allocation + fmt machinery) it replaces.
    let inflight_name = format!("{prefix}_http_inflight_requests");
    let requests_total_name = format!("{prefix}_http_requests_total");
    let duration_name = format!("{prefix}_http_request_duration_seconds");
    middleware::from_fn(move |req: Request, next: Next| {
        let inflight_name = inflight_name.clone();
        let requests_total_name = requests_total_name.clone();
        let duration_name = duration_name.clone();
        Box::pin(async move {
            let route = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_owned()).unwrap_or_else(|| "<unmatched>".to_owned());
            let method = method_label(req.method()).to_owned();
            let inflight = gauge!(inflight_name);
            inflight.increment(1.0);
            let _inflight_guard = InflightGuard(inflight);
            let started = Instant::now();
            let resp = next.run(req).await;
            let elapsed = started.elapsed().as_secs_f64();
            let status_class = format!("{}xx", resp.status().as_u16() / 100);
            counter!(
                requests_total_name,
                "route" => route.clone(),
                "method" => method.clone(),
                "status_class" => status_class
            )
            .increment(1);
            histogram!(
                duration_name,
                "route" => route,
                "method" => method
            )
            .record(elapsed);
            resp
        }) as BoxFuture
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{init, metrics_router};
    use axum::{Router, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn records_request_with_matched_route_and_status_class() {
        let handle = init("test-svc");
        let app: Router = Router::new()
            .route("/v1/thing/{id}", get(|| async { "ok" }))
            .layer(http_metrics_layer("gwtest"))
            .merge(metrics_router(handle.clone()));
        // Drive a request through the templated route.
        let _ = app
            .clone()
            .oneshot(axum::http::Request::builder().uri("/v1/thing/42").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let out = handle.render();
        assert!(out.contains("gwtest_http_requests_total"), "counter emitted:\n{out}");
        assert!(out.contains("route=\"/v1/thing/{id}\""), "uses the MatchedPath template, not /v1/thing/42");
        assert!(out.contains("status_class=\"2xx\""));
        assert!(out.contains("gwtest_http_request_duration_seconds"));
    }

    async fn boom_handler() -> &'static str {
        panic!("boom")
    }

    #[tokio::test]
    async fn inflight_gauge_is_decremented_even_when_the_handler_panics() {
        let handle = init("test-svc");
        let app: Router = Router::new()
            .route("/boom", get(boom_handler))
            .layer(http_metrics_layer("panictest"))
            .merge(metrics_router(handle.clone()));
        let req = axum::http::Request::builder().uri("/boom").body(axum::body::Body::empty()).unwrap();
        // Run through a spawned task so the handler's panic surfaces as a `JoinError` instead of
        // unwinding this test — axum does not catch handler panics on its own (that's what
        // `tower_http::catch_panic::CatchPanicLayer` is for), so this exercises the same unwind
        // path a real panicking handler would take in production.
        let join = tokio::spawn(app.oneshot(req));
        let result = join.await;
        assert!(result.is_err(), "expected the handler panic to propagate as a task panic");

        let out = handle.render();
        assert!(out.contains("panictest_http_inflight_requests"), "gauge emitted:\n{out}");
        // The InflightGuard's Drop impl must have run during the unwind, so increment(1.0) is
        // exactly offset by decrement(1.0) — the gauge must not be left stuck above zero.
        let stuck_positive = out.lines().any(|l| l.starts_with("panictest_http_inflight_requests") && !l.trim_end().ends_with(" 0"));
        assert!(!stuck_positive, "inflight gauge leaked after a panicking handler:\n{out}");
    }
}
