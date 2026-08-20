// SPDX-License-Identifier: Apache-2.0

//! tonic gRPC surface: the well-known `grpc.health.v1.Health` service (via tonic-health), the
//! IAM `TenancyService` (task-16 brief, SMA-442), the `AuthnService` + bearer-enforcement
//! layer (SMA-443 Task 12), the `AuthorizationService` (SMA-444 Task 19), the
//! `ServiceAccountService` (SMA-445 Task 21), the `ServiceInfoService` (SMA-505, always
//! mounted), and — when `iam.audit` is enabled — the `AuditService` (SMA-446 Task A10).

pub mod audit;
pub mod authn;
pub mod authz;
pub mod convert;
pub mod service_accounts;
pub mod service_info;
pub mod tenancy;

use std::net::SocketAddr;
use tonic::transport::Server;
use tonic::transport::server::Router as TonicRouter;
use tonic_health::ServingStatus;
use tower::layer::util::{Identity, Stack};

use crate::adapters::http::AppState;
use audit::AuditGrpc;
use authn::{AuthLayer, AuthnGrpc};
use authz::AuthzGrpc;
use paigasus_observability::CorrelationLayer;
use paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoServiceServer;
use paigasus_proto::paigasus::iam::v1::audit_service_server::AuditServiceServer;
use paigasus_proto::paigasus::iam::v1::authn_service_server::AuthnServiceServer;
use paigasus_proto::paigasus::iam::v1::authorization_service_server::AuthorizationServiceServer;
use paigasus_proto::paigasus::iam::v1::service_account_service_server::ServiceAccountServiceServer;
use paigasus_proto::paigasus::iam::v1::tenancy_service_server::TenancyServiceServer;
use service_accounts::ServiceAccountGrpc;
use service_info::ServiceInfoGrpc;
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
/// `AuthnService` (Task 12), the `AuthorizationService` (SMA-444 Task 19), the
/// `ServiceAccountService` (SMA-445 Task 21), the `ServiceInfoService` (SMA-505, always
/// mounted), and — when `iam.audit` is enabled — the `AuditService` (SMA-446 Task A10)
/// mounted, serving a static `SERVING` health status (see `health_service`). [`CorrelationLayer`]
/// (SMA-504) and `AuthLayer` both wrap the whole server, `CorrelationLayer` applied FIRST so it
/// is outermost among our two — a bearer rejection still carries request/correlation ids. It is
/// NOT outermost overall: tonic wraps the whole user stack in its own
/// `RecoverError`/`LoadShed`/`ConcurrencyLimit`/`GrpcTimeout`, so a `Server::timeout` `Status` is
/// produced outside `CorrelationLayer` and carries no ids — an accepted gap (closing it would
/// mean reimplementing tonic's timeout). Health and `AuthnService.Introspect`/`IntrospectApiKey`
/// are `:path`-exempt from bearer enforcement, every `TenancyService`/`AuthorizationService`/
/// `ServiceAccountService`/`ServiceInfoService`/`AuditService` RPC is not (spec §7.4, D14).
/// `main` calls `.serve_with_shutdown`. The reporter is dropped here — dynamic readiness is
/// deferred to M1.
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>> {
    let (_reporter, health) = health_service().await;
    let audit_enabled = state.capabilities.audit_query;
    let mut router = Server::builder()
        .timeout(timeout)
        // SMA-504: applied BEFORE `AuthLayer`, so it is outermost among OUR layers and a bearer
        // rejection still carries ids. It is NOT outermost overall: tonic wraps the whole user
        // stack in RecoverError/LoadShed/ConcurrencyLimit/GrpcTimeout, so a `Server::timeout`
        // Status is produced outside this layer and carries no ids and no ErrorInfo. Accepted
        // gap — closing it would mean reimplementing tonic's timeout.
        .layer(CorrelationLayer)
        .layer(AuthLayer::new(state.clone()))
        .add_service(health)
        .add_service(TenancyServiceServer::new(TenancyGrpc::new(state.clone())))
        .add_service(AuthnServiceServer::new(AuthnGrpc::new(state.clone())))
        .add_service(AuthorizationServiceServer::new(AuthzGrpc::new(state.clone())))
        .add_service(ServiceAccountServiceServer::new(ServiceAccountGrpc::new(state.clone())))
        // SMA-505: always served — the descriptor is how a client learns what the rest of this
        // server offers, so it can never itself be capability-gated.
        .add_service(ServiceInfoServiceServer::new(ServiceInfoGrpc::new(state.clone())));
    // `AuditService` is WHOLLY within `iam.audit`, so it is not registered at all when the
    // capability is off — a client then gets `UNIMPLEMENTED`, exactly as it would from a build
    // predating the service. `add_service` returns `Self`, so this does not disturb the
    // concrete `TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>>` return type.
    if audit_enabled {
        router = router.add_service(AuditServiceServer::new(AuditGrpc::new(state)));
    }
    router
}

/// Serve gRPC on `addr` until `shutdown` resolves. gRPC health is a static `SERVING` for M0
/// (see `health_service`); dynamic readiness is a deferred M1 concern.
pub async fn serve(addr: SocketAddr, state: AppState, timeout: std::time::Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> Result<(), tonic::transport::Error> {
    router(state, timeout).await.serve_with_shutdown(addr, shutdown).await
}
