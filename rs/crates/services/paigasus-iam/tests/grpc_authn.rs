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

    // A valid token for a brand-new (issuer, subject). The authenticated CreateOrganization
    // JIT-provisions the token's principal on the way through the enforcement layer (D5) and
    // succeeds — proof that a valid bearer is accepted. SMA-444 Task 20: `CreateOrganization`/
    // `AttachMembership` are now also authorization-enforced, so a `platform_admin` grant is
    // seeded for this (issuer, subject) up front — `provision`'s `state.authn.resolve` is the
    // exact JIT path the layer itself runs, so this doesn't change what's being proven.
    let token = idp.bearer("grpc-alice", Some("grpc-alice@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut tenancy = TenancyServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

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
    assert!(ctx.role_grants.is_empty(), "role grants empty until a later M3 task populates them");
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

/// SMA-504: the trailers-only rejection path must carry ErrorInfo too. `Status::into_http`
/// serializes differently from a handler-returned Status, so the unit tests in `convert.rs`
/// cannot prove this — only a real client/server round trip can.
///
/// A bearer-less call hits `AuthEnforce::call`'s missing-token branch
/// (`grpc/authn.rs::call` -> `reject(&AuthnError::InvalidToken(TokenDefect::Malformed))` ->
/// `convert::authn_status`), NOT `convert::missing_auth_context()` — that helper is for the
/// disjoint case where `AuthContext` extraction fails on an ALREADY-authenticated request
/// (`grpc::{tenancy,authz,service_accounts,audit}::actor_context`). So the expected code is
/// `Unauthenticated` and the expected reason is `invalid-token` (review finding #3).
#[tokio::test]
async fn a_bearer_rejection_carries_error_info_over_the_wire() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut tenancy = TenancyServiceClient::new(channel(addr).await);

    // No `authorization` metadata: AuthLayer rejects before the handler, via `Status::into_http`.
    let err = tenancy
        .create_organization(CreateOrganizationRequest {
            slug: "acme".into(),
            name: "Acme".into(),
        })
        .await
        .expect_err("a bearer-less tenancy call must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("the trailers-only path must carry ErrorInfo");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);
    assert_eq!(info.reason, "invalid-token");
    assert!(
        paigasus_proto::paigasus::common::v1::ErrorReason::from_wire_reason(&info.reason).is_some(),
        "{} is not in the registry",
        info.reason
    );
    // Review finding #1: this call runs inside a real `CorrelationLayer` scope (it wraps
    // `AuthLayer` outermost, `grpc/mod.rs::router`), so — unlike the `convert.rs` unit tests,
    // which run outside any scope — both id keys must be present and survive the
    // `Status::into_http` trailers-only serialization, not just in-process construction.
    let correlation_id = info.metadata.get("correlation_id").expect("correlation_id must survive the trailers-only wire round trip");
    let request_id = info.metadata.get("request_id").expect("request_id must survive the trailers-only wire round trip");
    assert!(uuid::Uuid::parse_str(correlation_id).is_ok(), "correlation_id must be a UUID: {correlation_id}");
    assert!(uuid::Uuid::parse_str(request_id).is_ok(), "request_id must be a UUID: {request_id}");

    // Review finding #2 (spec D2): "A test pins that the two agree; clients may read either."
    // `CorrelationLayer` also sets `paigasus-request-id`/`paigasus-correlation-id` as plain gRPC
    // headers, which on this trailers-only path tonic folds into the SAME `Status`'s metadata
    // alongside `grpc-status-details-bin` — so both must be readable off `err.metadata()` too,
    // and must equal the `ErrorInfo.metadata` ids above, not just independently be UUIDs.
    let header_correlation_id = err
        .metadata()
        .get("paigasus-correlation-id")
        .expect("paigasus-correlation-id must also survive as a plain gRPC header/metadata entry")
        .to_str()
        .expect("ascii");
    let header_request_id = err
        .metadata()
        .get("paigasus-request-id")
        .expect("paigasus-request-id must also survive as a plain gRPC header/metadata entry")
        .to_str()
        .expect("ascii");
    assert_eq!(header_correlation_id, correlation_id, "the header correlation id must equal the ErrorInfo.metadata correlation id (D2)");
    assert_eq!(header_request_id, request_id, "the header request id must equal the ErrorInfo.metadata request id (D2)");

    server.abort();
}

/// SMA-504 spec §4.1: the one documented, accepted gap in "every `Status` this codebase
/// produces is machine-readable". `grpc::router`'s own doc comment: `CorrelationLayer` and
/// `AuthLayer` both wrap the whole server, but tonic wraps THAT ENTIRE stack in its own
/// `RecoverError`/`LoadShed`/`ConcurrencyLimit`/`GrpcTimeout` — so a `Server::timeout`-produced
/// `Status` never touches our code and carries no ids and no `ErrorInfo`. Review finding #1:
/// asserted here rather than left as prose in three places and pinned nowhere, so a tonic
/// upgrade that moved `GrpcTimeout` inside our stack would fail this test rather than silently
/// invalidate the spec's claim.
///
/// Built with `Duration::from_millis(1)` — far shorter than any real RPC through this harness
/// can complete in. A brand-new `(issuer, subject)` forces `AuthLayer` to JIT-provision the
/// principal (a DB WRITE), and this fresh `AppState`'s in-memory JWKS cache starts empty, so
/// validating the token's signature requires a real HTTPS round trip to the mock IdP to fetch
/// it FIRST — either alone comfortably exceeds 1ms; both together make hitting the deadline the
/// deterministic outcome, not a race.
///
/// The code tonic 0.14.6 produces here is `Code::Cancelled`, not `Code::DeadlineExceeded` — read
/// from its own source (`transport/service/grpc_timeout.rs`'s `GrpcTimeout` raises a private
/// `TimeoutExpired` on expiry; `status.rs`'s `find_status_in_source_chain`, which the
/// `RecoverError` layer wrapping `GrpcTimeout` calls to turn that raw error into a `Status`,
/// maps `TimeoutExpired` to `Status::cancelled(..)`, never `Status::deadline_exceeded(..)`) and
/// confirmed empirically against this exact server. That mapping is an implementation detail
/// tonic does not guarantee across versions, so the assertion accepts either code — the gap this
/// test exists to pin is narrower and more durable: no `Status` a
/// `Server::builder().timeout(..)`-triggered expiry produces carries `ErrorInfo` or our ids.
#[tokio::test]
async fn a_server_side_timeout_status_carries_no_error_info_or_ids() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    // The 1ms timeout, applied via the SAME `router(state, timeout)` parameter every other test
    // in this file leaves at its default `Duration::from_secs(5)` — this is the only difference.
    let router = grpc::router(state, Duration::from_millis(1)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    let mut tenancy = TenancyServiceClient::new(channel(addr).await);

    let token = idp.bearer("grpc-timeout-victim", Some("grpc-timeout-victim@example.com"), "paigasus", 3600);
    let err = tenancy
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme-timeout".into(),
                name: "Acme".into(),
            },
            &token,
        ))
        .await
        .expect_err("a 1ms server timeout must fail a real DB-backed, JIT-provisioning RPC");

    // Either code is accepted deliberately: tonic 0.14.6 produces `Cancelled` (see the doc
    // comment), but that mapping is an implementation detail it has changed before and does not
    // guarantee across versions. Pinning it exactly would red this test on a tonic bump that
    // altered nothing we care about. What IS stable — and what the assertions below pin — is that
    // a timeout Status carries no ErrorInfo and no ids.
    assert!(matches!(err.code(), Code::Cancelled | Code::DeadlineExceeded), "{err:?}");
    let details = tonic_types::StatusExt::get_error_details(&err);
    assert!(
        details.error_info().is_none(),
        "a tonic-internal Server::timeout Status must carry no ErrorInfo — the accepted gap (spec §4.1), not something our code produced"
    );
    assert!(err.metadata().get("paigasus-correlation-id").is_none(), "no ids either — the gap is total, not partial");
    assert!(err.metadata().get("paigasus-request-id").is_none());

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
