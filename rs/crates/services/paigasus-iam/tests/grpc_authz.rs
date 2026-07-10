// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `AuthorizationService` (SMA-444 Task 19): `IsAuthorized`'s
//! self-query default-deny and its self/admin exposure rule's `PermissionDenied` (nothing
//! leaked) on an unauthorized non-self query; the role-grant grant -> list -> revoke
//! lifecycle; `PutPolicy` forbidden for a non-admin; and, for a seeded platform_admin actor,
//! the `PutPolicy` -> `ListPolicies` -> `DeletePolicy` round trip for a non-system policy
//! (asserting `convert::to_proto_policy`'s field mapping) plus `DeletePolicy`'s
//! `FailedPrecondition` on a `system == true` starter policy. Mirrors `tests/http_authz.rs`'s
//! scenarios (same shared `Authorize::decide_gated` rule, SMA-444 Task 19 brief — the two
//! transports must never diverge) and `tests/grpc_tenancy.rs`'s harness: the real
//! `grpc::router(AppState::new(db, &cfg), ..)` over an ephemeral `TcpListener`, against an
//! ephemeral Postgres (Docker) + the HTTPS mock IdP.
//!
//! Every `AuthorizationService` RPC is bearer-enforced (Task 12, D14) — each request carries
//! an `authorization: Bearer <token>` metadata entry via the [`authed`] wrapper. Getting a
//! principal's own PRN (`self_principal_prn`) and seeding the bootstrap `platform_admin`
//! grant delegate to `support::provision`/`support::seed_platform_admin` (SMA-444 Task 20):
//! `provision` resolves directly through `state.authn` rather than by driving
//! `TenancyService.ListOrganizations` (that RPC is itself enforced now, so its own status
//! would depend on a grant that doesn't exist yet at that point).

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::roles::FORBID_ARCHIVED_WRITES_ID;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::{DeletePolicyRequest, GrantRoleRequest, IsAuthorizedRequest, ListPoliciesRequest, ListRoleGrantsRequest, Policy, PutPolicyRequest, RevokeRoleRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;

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

#[tokio::test]
async fn is_authorized_self_query_over_grpc_returns_a_default_deny_decision_for_an_ungranted_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-authz-alice", Some("grpc-authz-alice@example.com"), "paigasus", 3600);
    let principal_prn = support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

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
    let actor_token = idp.bearer("grpc-authz-bob", Some("grpc-authz-bob@example.com"), "paigasus", 3600);
    let other_token = idp.bearer("grpc-authz-carol", Some("grpc-authz-carol@example.com"), "paigasus", 3600);
    support::provision(&state, &actor_token).await;
    let other_prn = support::provision(&state, &other_token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

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
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;

    let member_token = idp.bearer("grpc-authz-member", Some("grpc-authz-member@example.com"), "paigasus", 3600);
    let member_prn = support::provision(&state, &member_token).await;

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
    let token = idp.bearer("grpc-authz-nonadmin", Some("grpc-authz-nonadmin@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

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

#[tokio::test]
async fn put_list_delete_policy_lifecycle_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let ch = channel(addr).await;

    let admin_token = idp.bearer("grpc-authz-policy-admin", Some("grpc-authz-policy-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;

    let mut authz = AuthorizationServiceClient::new(ch);
    let source = r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#.to_string();

    // PutPolicy: a seeded platform_admin can author a non-system policy; the returned proto
    // `Policy` round-trips every field `convert::to_proto_policy` maps.
    let put = authz
        .put_policy(authed(
            PutPolicyRequest {
                policy: Some(Policy {
                    policy_id: "grpc-authz-lifecycle-policy".to_string(),
                    kind: "static".to_string(),
                    source: source.clone(),
                    description: "grpc lifecycle test policy".to_string(),
                    system: false,
                }),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .policy
        .expect("policy");
    assert_eq!(put.policy_id, "grpc-authz-lifecycle-policy");
    assert_eq!(put.kind, "static");
    assert_eq!(put.source, source);
    assert_eq!(put.description, "grpc lifecycle test policy");
    assert!(!put.system);

    // ListPolicies: the just-put policy is present alongside the reconcile-seeded starter
    // policies (`bootstrap::reconcile_starter` seeds `authz::roles::starter_policies()` at
    // `AppState::new`, all `system == true`) — assert `to_proto_policy`'s field mapping on
    // the returned entry too.
    let listed = authz
        .list_policies(authed(ListPoliciesRequest { limit: 0, offset: 0 }, &admin_token))
        .await
        .unwrap()
        .into_inner()
        .policies;
    let found = listed.iter().find(|p| p.policy_id == "grpc-authz-lifecycle-policy").expect("just-put policy listed");
    assert_eq!(found.kind, "static");
    assert_eq!(found.source, source);
    assert_eq!(found.description, "grpc lifecycle test policy");
    assert!(!found.system);
    assert!(listed.iter().any(|p| p.system), "the reconcile-seeded starter policies should also be listed");

    // DeletePolicy: delete the just-put policy, then confirm `ListPolicies` no longer
    // returns it.
    authz
        .delete_policy(authed(
            DeletePolicyRequest {
                policy_id: "grpc-authz-lifecycle-policy".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap();

    let listed_after = authz
        .list_policies(authed(ListPoliciesRequest { limit: 0, offset: 0 }, &admin_token))
        .await
        .unwrap()
        .into_inner()
        .policies;
    assert!(!listed_after.iter().any(|p| p.policy_id == "grpc-authz-lifecycle-policy"));

    server.abort();
}

#[tokio::test]
async fn delete_policy_over_grpc_is_failed_precondition_for_a_system_policy() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let ch = channel(addr).await;

    let admin_token = idp.bearer("grpc-authz-policy-sys-admin", Some("grpc-authz-policy-sys-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;

    let mut authz = AuthorizationServiceClient::new(ch);

    // `PgPolicyStore::delete` rejects mutating an existing `system == true` row with
    // `AuthzError::SystemImmutable`, which `TenancyError::class()` maps to
    // `ErrorClass::Precondition` -> `Code::FailedPrecondition` (`convert::status_to_grpc`) —
    // NOT `PermissionDenied`: the actor IS authorized for `DeletePolicy@Root` (seeded
    // platform_admin); it's the store itself that refuses to touch a system row.
    let err = authz
        .delete_policy(authed(
            DeletePolicyRequest {
                policy_id: FORBID_ARCHIVED_WRITES_ID.to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::FailedPrecondition, "{err:?}");

    server.abort();
}
