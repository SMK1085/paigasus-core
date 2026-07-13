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

/// axum middleware recording request count (by route/method/status_class), duration, and an
/// in-flight gauge. `route` is the MatchedPath template (bounded); an unmatched request collapses
/// to `<unmatched>`. `prefix` is the service metric prefix (e.g. `"gateway"`, `"iam"`).
///
/// Attach downstream with `router.layer(http_metrics_layer("gateway"))`.
pub fn http_metrics_layer(prefix: &'static str) -> middleware::FromFnLayer<impl Fn(Request, Next) -> BoxFuture + Clone, (), (Request,)> {
    middleware::from_fn(move |req: Request, next: Next| {
        Box::pin(async move {
            let route = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_owned()).unwrap_or_else(|| "<unmatched>".to_owned());
            let method = req.method().as_str().to_owned();
            let inflight = gauge!(format!("{prefix}_http_inflight_requests"));
            inflight.increment(1.0);
            let started = Instant::now();
            let resp = next.run(req).await;
            inflight.decrement(1.0);
            let elapsed = started.elapsed().as_secs_f64();
            let status_class = format!("{}xx", resp.status().as_u16() / 100);
            counter!(
                format!("{prefix}_http_requests_total"),
                "route" => route.clone(),
                "method" => method.clone(),
                "status_class" => status_class
            )
            .increment(1);
            histogram!(
                format!("{prefix}_http_request_duration_seconds"),
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
}
