// SPDX-License-Identifier: Apache-2.0

//! tonic gRPC surface. M0 serves only the well-known `grpc.health.v1.Health` (via
//! tonic-health); IAM RPCs arrive in later milestones.

use std::net::SocketAddr;
use tonic::transport::Server;
use tonic::transport::server::Router as TonicRouter;
use tonic_health::ServingStatus;

/// Build a health service with the overall server marked SERVING, plus its reporter.
/// M0 serves a **static** `SERVING` status — there is no gRPC readiness wiring in scope
/// (`/readyz` is HTTP-only, see `adapters::http`). The reporter is returned so the caller
/// *could* flip the status later, but threading it through `router`/`serve` to make gRPC
/// health dynamic is a deferred M1 concern. The empty service name ("") is the overall
/// status that `grpc_health_probe`/k8s query by default.
pub async fn health_service() -> (
    tonic_health::server::HealthReporter,
    tonic_health::pb::health_server::HealthServer<impl tonic_health::pb::health_server::Health>,
) {
    let (reporter, service) = tonic_health::server::health_reporter();
    reporter.set_service_status("", ServingStatus::Serving).await;
    (reporter, service)
}

/// A tonic `Server` router with the health service mounted, serving a static `SERVING`
/// status (M0; see `health_service`). `main` calls `.serve_with_shutdown`. The reporter is
/// dropped here — dynamic readiness is deferred to M1.
pub async fn router(timeout: std::time::Duration) -> TonicRouter {
    let (_reporter, health) = health_service().await;
    Server::builder().timeout(timeout).add_service(health)
}

/// Serve gRPC on `addr` until `shutdown` resolves. gRPC health is a static `SERVING` for M0
/// (see `health_service`); dynamic readiness is a deferred M1 concern.
pub async fn serve(addr: SocketAddr, timeout: std::time::Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> Result<(), tonic::transport::Error> {
    router(timeout).await.serve_with_shutdown(addr, shutdown).await
}
