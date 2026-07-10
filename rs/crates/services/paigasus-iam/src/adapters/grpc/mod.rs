// SPDX-License-Identifier: Apache-2.0

//! tonic gRPC surface: the well-known `grpc.health.v1.Health` service (via tonic-health), the
//! IAM `TenancyService` (task-16 brief, SMA-442), the `AuthnService` + bearer-enforcement
//! layer (SMA-443 Task 12), and the `AuthorizationService` (SMA-444 Task 19).

pub mod authn;
pub mod authz;
pub mod convert;
pub mod tenancy;

use std::net::SocketAddr;
use tonic::transport::Server;
use tonic::transport::server::Router as TonicRouter;
use tonic_health::ServingStatus;
use tower::layer::util::{Identity, Stack};

use crate::adapters::http::AppState;
use authn::{AuthLayer, AuthnGrpc};
use authz::AuthzGrpc;
use paigasus_proto::paigasus::iam::v1::authn_service_server::AuthnServiceServer;
use paigasus_proto::paigasus::iam::v1::authorization_service_server::AuthorizationServiceServer;
use paigasus_proto::paigasus::iam::v1::tenancy_service_server::TenancyServiceServer;
use tenancy::TenancyGrpc;

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

/// A tonic `Server` router with the health service, the `TenancyService` (Task 16), the
/// `AuthnService` (Task 12), and the `AuthorizationService` (SMA-444 Task 19) mounted,
/// serving a static `SERVING` health status (see `health_service`). The `AuthLayer` wraps the
/// whole server — health and `AuthnService.Introspect` are `:path`-exempt from bearer
/// enforcement, every `TenancyService`/`AuthorizationService` RPC is not (spec §7.4, D14).
/// `main` calls `.serve_with_shutdown`. The reporter is dropped here — dynamic readiness is
/// deferred to M1.
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Identity>> {
    let (_reporter, health) = health_service().await;
    Server::builder()
        .timeout(timeout)
        .layer(AuthLayer::new(state.clone()))
        .add_service(health)
        .add_service(TenancyServiceServer::new(TenancyGrpc::new(state.clone())))
        .add_service(AuthnServiceServer::new(AuthnGrpc::new(state.clone())))
        .add_service(AuthorizationServiceServer::new(AuthzGrpc::new(state)))
}

/// Serve gRPC on `addr` until `shutdown` resolves. gRPC health is a static `SERVING` for M0
/// (see `health_service`); dynamic readiness is a deferred M1 concern.
pub async fn serve(addr: SocketAddr, state: AppState, timeout: std::time::Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> Result<(), tonic::transport::Error> {
    router(state, timeout).await.serve_with_shutdown(addr, shutdown).await
}
