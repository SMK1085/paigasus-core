// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `AuthorizationService` (SMA-444 Task 19): `IsAuthorized`'s
//! self-query default-deny and its self/admin exposure rule's `PermissionDenied` (nothing
//! leaked) on an unauthorized non-self query; the role-grant grant -> list -> revoke
//! lifecycle; and `PutPolicy` forbidden for a non-admin. Mirrors `tests/http_authz.rs`'s
//! scenarios (same shared `Authorize::decide_gated` rule, SMA-444 Task 19 brief — the two
//! transports must never diverge) and `tests/grpc_tenancy.rs`'s harness: the real
//! `grpc::router(AppState::new(db, &cfg), ..)` over an ephemeral `TcpListener`, against an
//! ephemeral Postgres (Docker) + the HTTPS mock IdP.
//!
//! Every `AuthorizationService` RPC is bearer-enforced (Task 12, D14) — each request carries
//! an `authorization: Bearer <token>` metadata entry via the [`authed`] wrapper. Getting a
//! principal's own PRN mirrors `tests/http_authz.rs::self_principal_prn`: JIT-provision it
//! with any protected call, then read it back via the unauthenticated `Introspect` RPC.
//! Seeding the bootstrap `platform_admin` grant mirrors `tests/http_authz.rs::
//! seed_platform_admin`: directly through `AppState.role_grant_store`, bypassing
//! `RoleService::grant`'s anti-escalation check (there is no prior authority to authorize the
//! very first grant against).

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{GrantScope, PrincipalId, RoleGrant};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{GrantRoleRequest, IntrospectRequest, IsAuthorizedRequest, ListOrganizationsRequest, ListRoleGrantsRequest, Policy, PutPolicyRequest, RevokeRoleRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

/// Spawns the full `grpc::router` (health + tenancy + authn + authorization, all wrapped by
/// the bearer layer) on an ephemeral port; `abort()` the returned handle when the test
/// finishes.
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

/// Triggers JIT-provisioning of `token`'s own principal (any bearer-enforced RPC does this —
/// `AuthEnforce` resolves the caller before the handler ever runs) via a protected
/// `TenancyService.ListOrganizations` call, then reads its `principal_prn` back via the
/// unauthenticated `AuthnService.Introspect` RPC (mirrors `tests/http_authz.rs::
/// self_principal_prn`).
async fn self_principal_prn(ch: Channel, token: &str) -> String {
    let mut tenancy = TenancyServiceClient::new(ch.clone());
    tenancy.list_organizations(authed(ListOrganizationsRequest { limit: 0, offset: 0 }, token)).await.unwrap();
    let mut authn = AuthnServiceClient::new(ch);
    authn.introspect(IntrospectRequest { token: token.to_string() }).await.unwrap().into_inner().principal_prn
}

/// Seeds a `platform_admin`-at-`Root` grant for `principal_prn` directly through
/// `state.role_grant_store` — mirrors `tests/http_authz.rs::seed_platform_admin` exactly (see
/// its doc for why this bypasses `RoleService::grant`'s anti-escalation check and why sharing
/// this exact store matters for the `CedarAuthorizer`'s generation-counter visibility).
async fn seed_platform_admin(state: &AppState, grant_id: Uuid, principal_prn: &str) {
    let principal = PrincipalId::from_prn(Prn::parse(principal_prn).expect("valid principal prn"));
    let grant = RoleGrant {
        id: grant_id,
        principal,
        role_key: "platform_admin".to_string(),
        scope: GrantScope::Root,
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    };
    state.role_grant_store.grant(&grant).await.expect("seed platform_admin grant");
}

#[tokio::test]
async fn is_authorized_self_query_over_grpc_returns_a_default_deny_decision_for_an_ungranted_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let token = idp.bearer("grpc-authz-alice", Some("grpc-authz-alice@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(ch.clone(), &token).await;

    let mut authz = AuthorizationServiceClient::new(ch);
    let resp = authz
        .is_authorized(authed(
            IsAuthorizedRequest {
                principal_prn: principal_prn.clone(),
                action: "ListOrganizations".to_string(),
                resource_prn: root_prn().canonical(),
                context: Default::default(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();

    assert!(!resp.allowed);
    assert_eq!(resp.reason, "denied");
    assert_eq!(resp.determining_policies, vec![DEFAULT_DENY_MARKER.to_string()]);

    server.abort();
}

#[tokio::test]
async fn is_authorized_non_self_query_by_an_unauthorized_actor_over_grpc_is_permission_denied_with_nothing_leaked() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let actor_token = idp.bearer("grpc-authz-bob", Some("grpc-authz-bob@example.com"), "paigasus", 3600);
    let other_token = idp.bearer("grpc-authz-carol", Some("grpc-authz-carol@example.com"), "paigasus", 3600);
    self_principal_prn(ch.clone(), &actor_token).await;
    let other_prn = self_principal_prn(ch.clone(), &other_token).await;

    let mut authz = AuthorizationServiceClient::new(ch);
    let err = authz
        .is_authorized(authed(
            IsAuthorizedRequest {
                principal_prn: other_prn,
                action: "ListOrganizations".to_string(),
                resource_prn: root_prn().canonical(),
                context: Default::default(),
            },
            &actor_token,
        ))
        .await
        .unwrap_err();

    // A trailers-only `PermissionDenied` — the 403-equivalent gRPC code — with the stable
    // `forbidden:` message prefix and NOTHING else: no `allowed` bit, no
    // `determining_policies` for the probed principal ever reached the wire (there is no
    // response message on an error status at all).
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(err.message().starts_with("forbidden:"), "unexpected message: {}", err.message());

    server.abort();
}

#[tokio::test]
async fn grant_list_revoke_role_grant_lifecycle_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let ch = channel(addr).await;

    let admin_token = idp.bearer("grpc-authz-admin", Some("grpc-authz-admin@example.com"), "paigasus", 3600);
    let admin_prn = self_principal_prn(ch.clone(), &admin_token).await;
    seed_platform_admin(&state, Uuid::from_u128(9_101), &admin_prn).await;

    let member_token = idp.bearer("grpc-authz-member", Some("grpc-authz-member@example.com"), "paigasus", 3600);
    let member_prn = self_principal_prn(ch.clone(), &member_token).await;

    let mut authz = AuthorizationServiceClient::new(ch);

    // Grant: platform_admin at Root can grant anywhere, including Root itself.
    let granted = authz
        .grant_role(authed(
            GrantRoleRequest {
                principal_prn: member_prn.clone(),
                role_key: "platform_admin".to_string(),
                scope_prn: root_prn().canonical(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .grant
        .expect("grant");
    assert_eq!(granted.principal_prn, member_prn);
    assert_eq!(granted.role_key, "platform_admin");
    assert_eq!(granted.scope_prn, root_prn().canonical());

    // List: the member's own grant is visible to the admin querying it.
    let listed = authz
        .list_role_grants(authed(
            ListRoleGrantsRequest {
                principal_prn: member_prn.clone(),
                limit: 0,
                offset: 0,
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .grants;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, granted.id);

    // Revoke, then confirm the list is empty.
    authz.revoke_role(authed(RevokeRoleRequest { id: granted.id.clone() }, &admin_token)).await.unwrap();

    let listed_after = authz
        .list_role_grants(authed(
            ListRoleGrantsRequest {
                principal_prn: member_prn,
                limit: 0,
                offset: 0,
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .grants;
    assert!(listed_after.is_empty());

    server.abort();
}

#[tokio::test]
async fn put_policy_over_grpc_is_permission_denied_for_a_non_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let token = idp.bearer("grpc-authz-nonadmin", Some("grpc-authz-nonadmin@example.com"), "paigasus", 3600);
    self_principal_prn(ch.clone(), &token).await;

    let mut authz = AuthorizationServiceClient::new(ch);
    let err = authz
        .put_policy(authed(
            PutPolicyRequest {
                policy: Some(Policy {
                    policy_id: "grpc-authz-test-policy".to_string(),
                    kind: "static".to_string(),
                    source: r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string(),
                    description: "test policy".to_string(),
                    system: false,
                }),
            },
            &token,
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    server.abort();
}
