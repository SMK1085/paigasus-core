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
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam::application::create_user::NewUser;
// `Action`/`EventType` are for Tasks 2-5 (audit/outbox assertions); unused at this commit.
#[allow(unused_imports)]
use paigasus_iam_core::{Action, EventType};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    AttachMembershipRequest, CreateOrganizationRequest, CreateProjectRequest, CreateTeamRequest, GetOrganizationRequest, GetTeamRequest, Organization as ProtoOrganization, Project as ProtoProject,
    RenameOrganizationRequest, RenameProjectRequest, RenameTeamRequest, Team as ProtoTeam,
};
// The Archive*/Restore* requests are for Tasks 2-5's archive/restore forged-prn tests; unused
// at this commit.
#[allow(unused_imports)]
use paigasus_proto::paigasus::iam::v1::{ArchiveOrganizationRequest, ArchiveProjectRequest, ArchiveTeamRequest, RestoreOrganizationRequest, RestoreProjectRequest, RestoreTeamRequest};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
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

/// Builds the two `AppState`s this file's forged-PRN tests need on ONE database: `enforced`
/// keeps the default `enforce_tenancy = true`, `unenforced` turns it off.
///
/// Two states on one database work (`tests/authz_acceptance.rs:453-454` does the same):
/// `AppState::new` runs no migration and takes no advisory lock, and `reconcile_starter`
/// converges to the code. They do NOT share generation counters, though — `test_config` uses
/// the memory authz cache, so each state builds its own `Generations::memory()`
/// (`http/mod.rs:339-340`). Therefore every node in a case is created, changed and read
/// through the SAME state, and the `platform_admin` grant is seeded through the enforced one.
#[allow(dead_code)]
async fn two_states(db: &DatabaseConnection, idp: &support::MockIdp) -> (AppState, AppState) {
    let enforced = AppState::new(db.clone(), &support::test_config(idp)).await.unwrap();
    let mut cfg = support::test_config(idp);
    cfg.authz.enforce_tenancy = false;
    let unenforced = AppState::new(db.clone(), &cfg).await.unwrap();
    (enforced, unenforced)
}

/// Counts the `audit_log` rows for one action against one resource PRN. The queries below use
/// the SeaORM entities directly, exactly as `tests/mutation_audit_e2e.rs:101-116` does.
#[allow(dead_code)]
async fn audit_count(db: &DatabaseConnection, action: &str, resource_prn: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(action))
        .filter(audit_log::Column::ResourcePrn.eq(resource_prn))
        .count(db)
        .await
        .expect("count audit_log")
}

/// Counts the `event_outbox` rows for one event type against one aggregate PRN.
#[allow(dead_code)]
async fn outbox_count(db: &DatabaseConnection, event_type: &str, aggregate_prn: &str) -> u64 {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(event_type))
        .filter(event_outbox::Column::AggregatePrn.eq(aggregate_prn))
        .count(db)
        .await
        .expect("count event_outbox")
}

/// Splits a canonical PRN into its six fields: `prn`, `pgs`, service, region, org, `type/uuid`.
fn prn_fields(prn: &str) -> Vec<&str> {
    let fields: Vec<&str> = prn.splitn(6, ':').collect();
    assert_eq!(fields.len(), 6, "a canonical prn has six fields: {prn}");
    fields
}

/// Replaces the organization slot. `""` removes it.
#[allow(dead_code)]
fn with_org(prn: &str, org: &str) -> String {
    let f = prn_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], f[3], org, f[5])
}

/// Replaces the region slot.
#[allow(dead_code)]
fn with_region(prn: &str, region: &str) -> String {
    let f = prn_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], region, f[4], f[5])
}

/// Upper-cases ONLY the resource uuid. A correct PRN written this way must still succeed:
/// `Prn::canonical()` lower-cases it, so a fix that compares the raw request string breaks.
fn upper_uuid(prn: &str) -> String {
    let f = prn_fields(prn);
    let (kind, uuid) = f[5].split_once('/').expect("the last prn field is type/uuid");
    format!("prn:pgs:{}:{}:{}:{}/{}", f[2], f[3], f[4], kind, uuid.to_uppercase())
}

/// Records one assertion. Every case collects its failures instead of panicking, so ONE test
/// run shows every failing case — the unfixed run must show all of them, not only the first
/// (spec § 5.1).
#[allow(dead_code)]
fn check(failures: &mut Vec<String>, label: &str, ok: bool, detail: String) {
    if !ok {
        failures.push(format!("{label}: {detail}"));
    }
}

async fn create_org(client: &mut TenancyServiceClient<Channel>, token: &str, slug: &str, name: &str) -> ProtoOrganization {
    client
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: slug.to_string(),
                name: name.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create org {slug}: {e}"))
        .into_inner()
        .organization
        .expect("organization")
}

async fn create_team(client: &mut TenancyServiceClient<Channel>, token: &str, org_prn: &str, slug: &str) -> ProtoTeam {
    client
        .create_team(authed(
            CreateTeamRequest {
                org_prn: org_prn.to_string(),
                slug: slug.to_string(),
                name: slug.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create team {slug}: {e}"))
        .into_inner()
        .team
        .expect("team")
}

async fn create_project(client: &mut TenancyServiceClient<Channel>, token: &str, team_prn: &str, slug: &str) -> ProtoProject {
    client
        .create_project(authed(
            CreateProjectRequest {
                team_prn: team_prn.to_string(),
                slug: slug.to_string(),
                name: slug.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create project {slug}: {e}"))
        .into_inner()
        .project
        .expect("project")
}

/// Reads `ErrorInfo.reason` off a `tonic::Status`. Every IAM status carries one (SMA-504).
#[allow(dead_code)]
fn reason(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

#[tokio::test]
async fn organization_lifecycle_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-org-tester", Some("grpc-org-tester@example.com"), "paigasus", 3600);
    // SMA-444 Task 20: every `TenancyService` RPC below is now enforced — seed the acting
    // principal a `platform_admin` grant before spawning the server (the same `AppState`,
    // just cloned into the server task, so the seeded grant is visible to it).
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

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

    // Duplicate slug -> AlreadyExists, `ErrorInfo.reason` carries the stable `slug-conflict` code.
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
    // SMA-504: the code is no longer in the message. Read it from ErrorInfo instead — asserting
    // the reason, not a prefix, is what the wire change actually moved.
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.reason, "slug-conflict", "unexpected reason: {info:?}");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);

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
    // `CreateUser` RPC (user creation now has its own gRPC surface on the separate
    // `UserService`, SMA-501); this test still calls the application service directly because
    // it only needs a principal to exist, not coverage of that RPC. `AppState.users` is the
    // same service the HTTP `/v1/users` handler and `UserGrpc::create_user` both call.
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

    // The bearer used to authenticate the RPCs below JIT-provisions its OWN principal on the
    // way in (a separate identity from `alice` above); the membership assertions target
    // `alice`'s `principal_prn`, so that extra principal is inert here.
    let token = idp.bearer("grpc-team-tester", Some("grpc-team-tester@example.com"), "paigasus", 3600);
    // SMA-444 Task 20: seed the ACTOR (`grpc-team-tester`, not `alice`) a `platform_admin`
    // grant — every RPC below authorizes the caller, not the membership's target principal.
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

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
    // `ErrorInfo.reason` carries `missing-org-membership` (the org-membership invariant).
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
    // SMA-504: the code is no longer in the message. Read it from ErrorInfo instead — asserting
    // the reason, not a prefix, is what the wire change actually moved.
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.reason, "missing-org-membership", "unexpected reason: {info:?}");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);

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
    // `ErrorInfo.reason` carries `prn-mismatch` (the forged-org-slot defense, brief rule 8).
    let team_uuid = team.prn.rsplit('/').next().unwrap();
    let wrong_org = Uuid::from_u128(9_999);
    let forged_prn = format!("prn:pgs:iam::{wrong_org}:team/{team_uuid}");
    let err = client.get_team(authed(GetTeamRequest { prn: forged_prn }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    // SMA-504: the code is no longer in the message. Read it from ErrorInfo instead — asserting
    // the reason, not a prefix, is what the wire change actually moved.
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.reason, "prn-mismatch", "unexpected reason: {info:?}");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);

    server.abort();
}

/// T2 (spec § 5.2): a CORRECT prn whose uuid is upper-case must still succeed, for all three
/// node kinds. `Prn::canonical()` lower-cases the uuid, so a comparison against the raw request
/// string would refuse this request. This test passes before the SMA-643 fix as well — it
/// guards the fix's shape, and mutation m4 must break it.
#[tokio::test]
async fn an_upper_case_uuid_in_a_correct_prn_still_renames() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("upper-uuid", Some("upper-uuid@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

    let org = create_org(&mut client, &token, "upper-org", "Upper Org").await;
    let team = create_team(&mut client, &token, &org.prn, "upper-team").await;
    let project = create_project(&mut client, &token, &team.prn, "upper-project").await;

    let renamed = client
        .rename_organization(authed(
            RenameOrganizationRequest {
                prn: upper_uuid(&org.prn),
                new_slug: Some("upper-org-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the organization")
        .into_inner()
        .organization
        .expect("organization");
    assert_eq!(renamed.prn, org.prn, "the answer must carry the stored, canonical prn");
    assert_eq!(renamed.slug, "upper-org-2");

    client
        .rename_team(authed(
            RenameTeamRequest {
                prn: upper_uuid(&team.prn),
                new_slug: Some("upper-team-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the team");

    client
        .rename_project(authed(
            RenameProjectRequest {
                prn: upper_uuid(&project.prn),
                new_slug: Some("upper-project-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the project");

    server.abort();
}
