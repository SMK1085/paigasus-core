// SPDX-License-Identifier: Apache-2.0

//! tonic gRPC surface: the well-known `grpc.health.v1.Health` service (via tonic-health), the
//! IAM `TenancyService` (task-16 brief, SMA-442), the `AuthnService` + bearer-enforcement
//! layer (SMA-443 Task 12), the `AuthorizationService` (SMA-444 Task 19), the
//! `ServiceAccountService` (SMA-445 Task 21), the `ServiceInfoService` (SMA-505, always
//! mounted), the `UserService` (SMA-501, always mounted — its one RPC authorizes
//! `Action::CreateUser` at `Root`, SMA-584, see `users` module doc), the `OutboxService`
//! (SMA-501, ALWAYS mounted — see `dead_letters` module doc for why this break-glass
//! surface is not capability-gated,
//! unlike its `AuditService` neighbour), and — when `iam.audit` is enabled — the
//! `AuditService` (SMA-446 Task A10).

pub mod audit;
pub mod authn;
pub mod authz;
pub mod convert;
pub mod dead_letters;
pub mod service_accounts;
pub mod service_info;
pub mod tenancy;
pub mod users;

use tonic::transport::Server;
use tonic::transport::server::Router as TonicRouter;
use tonic_health::ServingStatus;
use tower::layer::util::{Identity, Stack};

use crate::adapters::http::AppState;
use audit::AuditGrpc;
use authn::{AuthLayer, AuthnGrpc};
use authz::AuthzGrpc;
use dead_letters::OutboxGrpc;
use paigasus_observability::CorrelationLayer;
use paigasus_proto::paigasus::common::v1::service_info_service_server::ServiceInfoServiceServer;
use paigasus_proto::paigasus::iam::v1::audit_service_server::AuditServiceServer;
use paigasus_proto::paigasus::iam::v1::authn_service_server::AuthnServiceServer;
use paigasus_proto::paigasus::iam::v1::authorization_service_server::AuthorizationServiceServer;
use paigasus_proto::paigasus::iam::v1::outbox_service_server::OutboxServiceServer;
use paigasus_proto::paigasus::iam::v1::service_account_service_server::ServiceAccountServiceServer;
use paigasus_proto::paigasus::iam::v1::tenancy_service_server::TenancyServiceServer;
use paigasus_proto::paigasus::iam::v1::user_service_server::UserServiceServer;
use service_accounts::ServiceAccountGrpc;
use service_info::ServiceInfoGrpc;
use tenancy::TenancyGrpc;
use users::UserGrpc;

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

/// The single service-registration site (SMA-571 D8). Both [`router`] — which the Docker-gated
/// integration suites drive — and production's deferred path (`adapters::boot::Serving`) build
/// from THIS function, because tonic's `Router` keeps its `Routes` private and cannot be
/// decomposed. Adding a service here mounts it on both; adding it anywhere else mounts it on
/// exactly one, which `service_registration_lives_at_one_site` exists to prevent.
///
/// Mounts the health service, the `TenancyService` (Task 16), the `AuthnService` (Task 12), the
/// `AuthorizationService` (SMA-444 Task 19), the `ServiceAccountService` (SMA-445 Task 21), the
/// `ServiceInfoService` (SMA-505, always mounted), the `UserService` (SMA-501, always mounted),
/// the `OutboxService` (SMA-501, ALWAYS mounted — a break-glass surface must not be
/// disable-able, unlike the read-only `AuditService` below it; see `dead_letters` module doc),
/// and — when `iam.audit` is enabled — the `AuditService` (SMA-446 Task A10), serving a static
/// `SERVING` health status (see `health_service`).
///
/// Carries no layers: `CorrelationLayer` and `AuthLayer` are applied by the caller, because
/// production applies them at two different levels (`CorrelationLayer` on the tonic `Server`,
/// `AuthLayer` on these routes) while `router` applies both on the `Server`.
pub async fn routes(state: AppState) -> tonic::service::Routes {
    let (_reporter, health) = health_service().await;
    let audit_enabled = state.capabilities.audit_query;
    let mut routes = tonic::service::Routes::default()
        .add_service(health)
        .add_service(TenancyServiceServer::new(TenancyGrpc::new(state.clone())))
        .add_service(AuthnServiceServer::new(AuthnGrpc::new(state.clone())))
        .add_service(AuthorizationServiceServer::new(AuthzGrpc::new(state.clone())))
        .add_service(ServiceAccountServiceServer::new(ServiceAccountGrpc::new(state.clone())))
        // SMA-505: always served — the descriptor is how a client learns what the rest of this
        // server offers, so it can never itself be capability-gated.
        .add_service(ServiceInfoServiceServer::new(ServiceInfoGrpc::new(state.clone())))
        // SMA-501: always served, mirroring HTTP's unconditional `/v1/users` mount — its one
        // RPC authorizes `Action::CreateUser` at `Root` (SMA-584, see `users` module doc).
        .add_service(UserServiceServer::new(UserGrpc::new(state.clone())))
        // SMA-501: always served, UNCONDITIONALLY — deliberately unlike `AuditService` below,
        // which is dropped entirely when `iam.audit` is off. `iam.audit` gates a READ-ONLY
        // surface; `OutboxService` permanently discards events and bulk-replays up to 10 000
        // rows — a break-glass surface must not be disable-able, because the moment you need it
        // is the moment a config flag is hardest to change. HTTP mounts `dead_letters::router()`
        // ungated too (see its module doc), so gating gRPC alone would itself be a divergence.
        .add_service(OutboxServiceServer::new(OutboxGrpc::new(state.clone())));
    // `AuditService` is WHOLLY within `iam.audit`, so it is not registered at all when the
    // capability is off — a client then gets `UNIMPLEMENTED`, exactly as it would from a build
    // predating the service.
    if audit_enabled {
        routes = routes.add_service(AuditServiceServer::new(AuditGrpc::new(state)));
    }
    routes
}

/// A tonic `Server` router built from [`routes`] (SMA-571 D8; see that function's doc for the
/// full service inventory), with [`CorrelationLayer`] (SMA-504) and `AuthLayer` both wrapping
/// the whole server, `CorrelationLayer` applied FIRST so it is outermost among our two — a
/// bearer rejection still carries request/correlation ids. It is NOT outermost overall: tonic
/// wraps the whole user stack in its own
/// `RecoverError`/`LoadShed`/`ConcurrencyLimit`/`GrpcTimeout`, so a `Server::timeout` `Status`
/// is produced outside `CorrelationLayer` and carries no ids — an accepted gap (closing it
/// would mean reimplementing tonic's timeout). Health and `AuthnService.Introspect`/
/// `IntrospectApiKey` are `:path`-exempt from bearer enforcement, every
/// `TenancyService`/`AuthorizationService`/`ServiceAccountService`/`ServiceInfoService`/
/// `UserService`/`OutboxService`/`AuditService` RPC is not (spec §7.4, D14) —
/// `UserService.CreateUser` is bearer-required AND authorizes `Action::CreateUser` at `Root`,
/// mirroring `POST /v1/users` (SMA-584, see `users` module doc).
///
/// **Test-only since SMA-571.** `main` no longer calls this at all: production binds
/// `adapters::boot::boot_grpc_routes` — which consumes [`routes`] directly and applies
/// `AuthLayer` inside `boot::Serving` instead of on the `Server` — so the only remaining callers
/// are the Docker-gated suites. The reporter is dropped here; production's dynamic readiness
/// comes from the one `health_service` reporter `main` hands to `BootSlot`.
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>> {
    let routes = routes(state.clone()).await;
    let mut server = Server::builder()
        .timeout(timeout)
        // SMA-504: applied BEFORE `AuthLayer`, so it is outermost among OUR layers and a bearer
        // rejection still carries ids. It is NOT outermost overall: tonic wraps the whole user
        // stack in RecoverError/LoadShed/ConcurrencyLimit/GrpcTimeout, so a `Server::timeout`
        // Status is produced outside this layer and carries no ids and no ErrorInfo. Accepted
        // gap — closing it would mean reimplementing tonic's timeout.
        .layer(CorrelationLayer)
        .layer(AuthLayer::new(state));
    server.add_routes(routes)
}

#[cfg(test)]
mod tests {
    /// SMA-571 D8: service registration must live at exactly ONE site. tonic's `Router` keeps its
    /// `Routes` private, so production's deferred path (`adapters::boot`) cannot reuse `router()` —
    /// it consumes `routes()` instead. If a future service is added to `router()` directly, it
    /// mounts for the eleven Docker-gated suites that drive `router()` and is ABSENT in production,
    /// with CI green. `include_str!` rather than a `repo:*` gate for the same reason
    /// `migration_lock.rs`'s composition-root guard is: one call site does not justify a `T`-array
    /// entry plus an `:affected-smoke` re-baseline.
    #[test]
    fn service_registration_lives_at_one_site() {
        const ME: &str = include_str!("mod.rs");
        // This test module itself quotes `.add_service(`/`.add_routes(` as string literals
        // (the match patterns and assert messages below), so counting over the WHOLE file
        // would self-match and corrupt every count. Slice them off first.
        let production = ME.split("\n#[cfg(test)]").next().expect("module must have a #[cfg(test)] block");
        let registrations = production.matches(".add_service(").count();
        let in_routes = production
            .split("async fn routes(")
            .nth(1)
            .expect("an `async fn routes(` must exist")
            .split("\npub ")
            .next()
            .expect("routes() must be followed by another item or EOF")
            .matches(".add_service(")
            .count();
        assert_eq!(
            registrations, in_routes,
            "every .add_service( must be inside `routes()` — found {registrations} total but only {in_routes} in routes()"
        );
        assert!(production.contains(".add_routes("), "router() must build from routes() via Server::add_routes");
    }
}
