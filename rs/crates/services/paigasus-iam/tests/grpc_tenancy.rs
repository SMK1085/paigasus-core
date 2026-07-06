// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `TenancyService`: organization create/get/duplicate-slug/
//! not-found, and a team + membership flow covering the org-membership invariant and the
//! forged-org-slot (`prn-mismatch`) defense. Drives the real `grpc::router(AppState::new(db, &cfg),
//! ..)` over an ephemeral `TcpListener` (mirrors `tests/grpc_health.rs`) against an ephemeral
//! Postgres (Docker; see `tests/support/mod.rs`).
//!
//! Every `TenancyService` RPC is bearer-enforced (Task 12): each request carries a valid
//! `authorization: Bearer <token>` metadata entry (minted from the mock IdP, JIT-provisioned
//! by the enforcement layer on the way in) via the [`authed`] wrapper.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::application::create_user::NewUser;
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{AttachMembershipRequest, CreateOrganizationRequest, CreateTeamRequest, GetOrganizationRequest, GetTeamRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

/// Spawns the real `grpc::router` (health + tenancy + authn, bearer-enforced, Task 12) on an
/// ephemeral port and returns its address plus the server task's handle (`abort()` it when the
/// test is done).
async fn spawn_tenancy_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn connect(addr: SocketAddr) -> TenancyServiceClient<Channel> {
    let channel = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    TenancyServiceClient::new(channel)
}

/// Wraps a request message in a `tonic::Request` carrying an `authorization: Bearer <token>`
/// metadata entry — the credential the Task 12 enforcement layer requires on every
/// `TenancyService` RPC.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

#[tokio::test]
async fn organization_lifecycle_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;
    let token = idp.bearer("grpc-org-tester", Some("grpc-org-tester@example.com"), "paigasus", 3600);

    // Create: response has `organization` + `default_team`, both with PRNs parseable by the
    // kernel; the team's `org_prn` matches the org's own `prn`.
    let created = client
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme".to_string(),
                name: "Acme Corp.".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    let org = created.organization.expect("organization");
    let default_team = created.default_team.expect("default_team");
    Prn::parse(&org.prn).unwrap_or_else(|e| panic!("unexpected org prn {}: {e}", org.prn));
    Prn::parse(&default_team.prn).unwrap_or_else(|e| panic!("unexpected team prn {}: {e}", default_team.prn));
    assert_eq!(default_team.org_prn, org.prn);
    assert_eq!(default_team.slug, "default");

    // GetOrganization roundtrip.
    let got = client.get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token)).await.unwrap().into_inner();
    let got_org = got.organization.expect("organization");
    assert_eq!(got_org.prn, org.prn);
    assert_eq!(got_org.slug, "acme");

    // Duplicate slug -> AlreadyExists, message starts with the stable `slug-conflict:` code.
    let err = client
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme".to_string(),
                name: "Dup".to_string(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::AlreadyExists);
    assert!(err.message().starts_with("slug-conflict:"), "unexpected message: {}", err.message());

    // Unknown org -> NotFound (well-formed PRN, but never created).
    let unknown_prn = Prn::build("iam", "", None, "organization", Uuid::from_u128(999_999)).unwrap().canonical();
    let err = client.get_organization(authed(GetOrganizationRequest { prn: unknown_prn }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound);

    server.abort();
}

#[tokio::test]
async fn team_membership_flow_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();

    // Mint a principal via the application service directly — `TenancyService` has no
    // `CreateUser` RPC (users stay HTTP-only per Task 15); `AppState.users` is the same
    // service the HTTP `/v1/users` handler calls.
    let principal_prn = state
        .users
        .execute(NewUser {
            email: "alice@example.com".to_string(),
            display_name: "Alice".to_string(),
            locale: None,
            timezone: None,
        })
        .await
        .unwrap()
        .canonical();

    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;
    // The bearer used to authenticate the RPCs below JIT-provisions its OWN principal on the
    // way in (a separate identity from `alice` above); the membership assertions target
    // `alice`'s `principal_prn`, so that extra principal is inert here.
    let token = idp.bearer("grpc-team-tester", Some("grpc-team-tester@example.com"), "paigasus", 3600);

    let created = client
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme".to_string(),
                name: "Acme".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    let org_prn = created.organization.expect("organization").prn;

    let team = client
        .create_team(authed(
            CreateTeamRequest {
                org_prn: org_prn.clone(),
                slug: "eng".to_string(),
                name: "Engineering".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .team
        .expect("team");
    assert_eq!(team.org_prn, org_prn);

    // Attaching to the team before the org membership exists -> FailedPrecondition,
    // `missing-org-membership:` prefix (the org-membership invariant).
    let err = client
        .attach_membership(authed(
            AttachMembershipRequest {
                principal_prn: principal_prn.clone(),
                node_prn: team.prn.clone(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);
    assert!(err.message().starts_with("missing-org-membership:"), "unexpected message: {}", err.message());

    // Attach to the org first, satisfying the invariant; the team attach then succeeds.
    client
        .attach_membership(authed(
            AttachMembershipRequest {
                principal_prn: principal_prn.clone(),
                node_prn: org_prn.clone(),
            },
            &token,
        ))
        .await
        .unwrap();
    let membership = client
        .attach_membership(authed(
            AttachMembershipRequest {
                principal_prn: principal_prn.clone(),
                node_prn: team.prn.clone(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .membership
        .expect("membership");
    assert_eq!(membership.node_prn, team.prn);
    assert_eq!(membership.principal_prn, principal_prn);

    // GetTeam with a forged org slot (correct team uuid, wrong org uuid) -> InvalidArgument,
    // `prn-mismatch:` prefix (the forged-org-slot defense, brief rule 8).
    let team_uuid = team.prn.rsplit('/').next().unwrap();
    let wrong_org = Uuid::from_u128(9_999);
    let forged_prn = format!("prn:pgs:iam::{wrong_org}:team/{team_uuid}");
    let err = client.get_team(authed(GetTeamRequest { prn: forged_prn }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(err.message().starts_with("prn-mismatch:"), "unexpected message: {}", err.message());

    server.abort();
}
