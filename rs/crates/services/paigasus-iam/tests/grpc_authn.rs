// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for Task 12: the `AuthnService.Introspect` RPC and the
//! bearer-enforcement tower layer wrapping `grpc::router` (spec §7.3/§7.4, D12/D14). Proves
//! the exemptions (`Introspect` + the well-known health service are reachable with NO
//! bearer), that a protected `TenancyService` RPC is rejected with a proper trailers-only
//! `Unauthenticated` status when the bearer is absent or invalid (never a bare HTTP 401 —
//! which the tonic client couldn't interpret) and is accepted (JIT-provisioning on the way
//! in) when it is valid, and that introspect round-trips the resolved `PrincipalContext`,
//! memberships included. Drives the real `grpc::router(AppState::new(db, &cfg), ..)` over an
//! ephemeral `TcpListener` (mirrors `grpc_tenancy.rs`) against an ephemeral Postgres +
//! the HTTPS mock IdP.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{AttachMembershipRequest, CreateOrganizationRequest, IntrospectRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

/// Spawns the full `grpc::router` (health + tenancy + authn, all wrapped by the bearer
/// layer) on an ephemeral port; `abort()` the returned handle when the test finishes.
async fn spawn_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn channel(addr: SocketAddr) -> Channel {
    tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// Builds a `tonic::Request` carrying an `authorization: Bearer <token>` metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

#[tokio::test]
async fn introspect_over_grpc_round_trips_a_jit_provisioned_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut tenancy = TenancyServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

    // A valid token for a brand-new (issuer, subject). The authenticated CreateOrganization
    // JIT-provisions the token's principal on the way through the enforcement layer (D5) and
    // succeeds — proof that a valid bearer is accepted.
    let token = idp.bearer("grpc-alice", Some("grpc-alice@example.com"), "paigasus", 3600);
    let org = tenancy
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme".into(),
                name: "Acme".into(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .organization
        .expect("organization");

    // Introspect over gRPC WITHOUT auth metadata: proves the `Introspect` exemption. A
    // resolved context here is only possible because the write above already JIT-provisioned
    // this identity — introspect itself never provisions (D10).
    let ctx = authn.introspect(IntrospectRequest { token: token.clone() }).await.unwrap().into_inner();
    assert!(ctx.principal_prn.starts_with("prn:pgs:iam:::principal/"), "{}", ctx.principal_prn);
    assert_eq!(ctx.status, "active");
    assert_eq!(ctx.issuer, idp.issuer);
    assert_eq!(ctx.subject, "grpc-alice");
    assert!(ctx.expires_at.is_some(), "expires_at is set");
    assert!(ctx.memberships.is_empty(), "no memberships yet");
    assert!(ctx.role_group_prns.is_empty(), "role groups empty until M3");
    let principal_prn = ctx.principal_prn.clone();

    // Attach an org membership to the resolved principal, then re-introspect: the membership
    // is reflected (the convert reuses the tenancy `Membership` mapping).
    tenancy
        .attach_membership(authed(
            AttachMembershipRequest {
                principal_prn: principal_prn.clone(),
                node_prn: org.prn.clone(),
            },
            &token,
        ))
        .await
        .unwrap();
    let ctx = authn.introspect(IntrospectRequest { token }).await.unwrap().into_inner();
    assert_eq!(ctx.memberships.len(), 1);
    assert_eq!(ctx.memberships[0].principal_prn, principal_prn);
    assert_eq!(ctx.memberships[0].node_prn, org.prn);

    server.abort();
}

#[tokio::test]
async fn tenancy_rpc_without_bearer_is_unauthenticated() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut tenancy = TenancyServiceClient::new(channel(addr).await);

    // No `authorization` metadata on a protected `TenancyService` RPC: the layer rejects it
    // with a trailers-only gRPC response (HTTP 200 + grpc-status 16), which the tonic client
    // surfaces as `Code::Unauthenticated` — never a transport error from a bare HTTP 401.
    let err = tenancy
        .create_organization(CreateOrganizationRequest {
            slug: "acme".into(),
            name: "Acme".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    server.abort();
}

#[tokio::test]
async fn tenancy_rpc_with_invalid_bearer_is_unauthenticated() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut tenancy = TenancyServiceClient::new(channel(addr).await);

    // A syntactically-broken bearer token: the validator rejects it, the layer maps
    // InvalidToken -> Unauthenticated, before any handler runs.
    let err = tenancy
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme".into(),
                name: "Acme".into(),
            },
            "not-a-jwt",
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    server.abort();
}

#[tokio::test]
async fn grpc_health_serves_without_bearer_through_the_layered_router() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;

    // Health is wrapped by the same layer but stays exempt: reachable with NO bearer.
    let mut health = HealthClient::new(channel(addr).await);
    let resp = health.check(HealthCheckRequest { service: String::new() }).await.unwrap().into_inner();
    assert_eq!(resp.status, ServingStatus::Serving as i32);

    server.abort();
}
