// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `UserService.CreateUser` (SMA-501): minting a principal, the
//! duplicate-email conflict, a malformed-email rejection (no principal minted), the D11
//! empty-locale-becomes-unset wire sentinel, and the SMA-584 pin that this RPC is
//! bearer-required AND authorized (`Action::CreateUser`@`Root`) — mirroring `POST /v1/users`
//! exactly (see `adapters::grpc::users` module doc; `tests/http_users.rs` is the HTTP twin).
//! Drives the real `grpc::router(AppState::new(db, &cfg), ..)` over an ephemeral `TcpListener`
//! (mirrors `tests/grpc_tenancy.rs`/`tests/grpc_audit.rs`) against an ephemeral Postgres
//! (Docker; see `tests/support/mod.rs`) and the HTTPS mock IdP.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::{principal, user};
use paigasus_iam_core::PrincipalId;
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::CreateUserRequest;
use paigasus_proto::paigasus::iam::v1::user_service_client::UserServiceClient;
use sea_orm::{EntityTrait, PaginatorTrait};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;

/// Spawns the full `grpc::router` (health, tenancy, authn, authz, service-account, service-info,
/// users, outbox — all wrapped by the bearer layer) on an ephemeral port; `abort()` the
/// returned handle when the test finishes.
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

/// Wraps a request message in a `tonic::Request` carrying an `authorization: Bearer <token>`
/// metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

fn create_user_request(email: &str) -> CreateUserRequest {
    CreateUserRequest {
        email: email.to_string(),
        display_name: "Test User".to_string(),
        locale: String::new(),
        timezone: String::new(),
    }
}

/// A mutation that dropped `id.canonical()` from `UserGrpc::create_user`'s response (e.g.
/// returning an empty or malformed string) would fail this test: the wire `principal_prn` must
/// be a PRN the kernel itself can parse back.
#[tokio::test]
async fn create_user_over_grpc_mints_a_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-user-tester", Some("grpc-user-tester@example.com"), "paigasus", 3600);
    // SMA-584: CreateUser now requires `Action::CreateUser`@`Root`. These tests cover minting,
    // conflicts and the D11 wire sentinel — not authorization — so they act as a platform_admin.
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let resp = client.create_user(authed(create_user_request("mint@example.com"), &token)).await.unwrap().into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}

/// A mutation that collapsed `TenancyError::EmailConflict`'s `ErrorClass::Conflict` into
/// `Validation` (or dropped the `uq_user_email` unique-violation mapping, letting the second
/// insert panic instead of erroring cleanly) would fail this test: the second `CreateUser` call
/// for the same email must come back `AlreadyExists`, not any other code or a transport error.
#[tokio::test]
async fn a_duplicate_email_is_already_exists() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dupe-tester", Some("grpc-dupe-tester@example.com"), "paigasus", 3600);
    // SMA-584: CreateUser now requires `Action::CreateUser`@`Root`. These tests cover minting,
    // conflicts and the D11 wire sentinel — not authorization — so they act as a platform_admin.
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    client.create_user(authed(create_user_request("dupe@example.com"), &token)).await.unwrap();
    let err = client.create_user(authed(create_user_request("dupe@example.com"), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::AlreadyExists, "{err:?}");

    server.abort();
}

/// A mutation that moved `Email::parse` to run AFTER an id is minted (or after the UnitOfWork
/// transaction opens) would fail this test's second half: a rejected create must leave the
/// `principal` table's row count untouched, not merely return an error to the caller.
#[tokio::test]
async fn a_malformed_email_is_invalid_argument() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let count_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-badmail-tester", Some("grpc-badmail-tester@example.com"), "paigasus", 3600);
    // Provision the acting principal FIRST, so its JIT-provisioned row is already counted in
    // `before` and the malformed create below is the only thing that could change the count.
    // SMA-584: CreateUser now requires `Action::CreateUser`@`Root`. These tests cover minting,
    // conflicts and the D11 wire sentinel — not authorization — so they act as a platform_admin.
    support::provision_platform_admin(&state, &token).await;
    let before = principal::Entity::find().count(&count_db).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let err = client.create_user(authed(create_user_request("not-an-email"), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "{err:?}");
    let after = principal::Entity::find().count(&count_db).await.unwrap();
    assert_eq!(after, before, "a rejected create must not mint a principal row");

    server.abort();
}

/// A mutation that deleted `users.rs`'s `opt_string` empty-string branch (making the wire's
/// `""` persist as `Some(String::new())` instead of `None`) would fail this test: the D11
/// sentinel says an empty `locale` scalar means "unset" on gRPC, so the persisted `user.locale`
/// column must be `NULL`/`None`, not an empty string.
#[tokio::test]
async fn an_empty_locale_becomes_unset() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let query_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-locale-tester", Some("grpc-locale-tester@example.com"), "paigasus", 3600);
    // SMA-584: CreateUser now requires `Action::CreateUser`@`Root`. These tests cover minting,
    // conflicts and the D11 wire sentinel — not authorization — so they act as a platform_admin.
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let resp = client.create_user(authed(create_user_request("locale@example.com"), &token)).await.unwrap().into_inner();
    let principal_id = PrincipalId::from_prn(Prn::parse(&resp.principal_prn).expect("valid principal prn"));
    let row = user::Entity::find_by_id(principal_id.uuid()).one(&query_db).await.unwrap().expect("user row present");
    assert_eq!(row.locale, None, "an empty wire locale must persist as None (D11), not Some(\"\")");

    server.abort();
}

/// **Design pin (SMA-584).** `UserService.CreateUser` is bearer-required AND authorized: it
/// checks `Action::CreateUser` at `root_prn()`, gated by `enforce_tenancy`, exactly as
/// `POST /v1/users` does (`adapters::grpc::users` module doc). This test pins all three halves
/// so a future maintainer who changes authorization on ONE transport is forced to consider the
/// other: unauthenticated is rejected (proving `UserService` carries no `is_exempt` allowlist
/// entry), an ordinary non-admin principal is DENIED, and a `platform_admin` succeeds.
/// `tests/http_users.rs` is the HTTP-side twin.
#[tokio::test]
async fn create_user_requires_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();

    // An ORDINARY principal: JIT-provisioned, no grant of any kind.
    let plain_token = idp.bearer("grpc-plain-tester", Some("grpc-plain-tester@example.com"), "paigasus", 3600);
    support::provision(&state, &plain_token).await;

    // A platform_admin, seeded at Root.
    let admin_token = idp.bearer("grpc-admin-tester", Some("grpc-admin-tester@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    // No bearer at all -> Unauthenticated: `UserService` is not on `AuthLayer`'s `is_exempt`
    // allowlist (module doc), so this never even reaches the handler.
    let err = client.create_user(create_user_request("no-bearer@example.com")).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    // An ordinary, non-admin principal -> PermissionDenied.
    let err = client.create_user(authed(create_user_request("denied@example.com"), &plain_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    // platform_admin -> Ok.
    let resp = client.create_user(authed(create_user_request("allowed@example.com"), &admin_token)).await.unwrap().into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}

/// **The action-identity pin, gRPC half (SMA-584).** The twin of
/// `tests/http_users.rs::the_http_guard_is_bound_to_create_user_specifically`; see that test's
/// doc for why a role grant cannot distinguish `CreateUser` from any other Root-only action, and
/// for why the policy seeded below is deliberately broad on principal/resource and narrow only
/// on the action — it is NOT the least-privilege remediation an operator should copy (design doc
/// §4.3 lever 1 is). A mutation that wires a different action into `adapters::grpc::users` fails
/// here.
#[tokio::test]
async fn the_grpc_guard_is_bound_to_create_user_specifically() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let state = AppState::new(db, &cfg).await.unwrap();

    let admin_token = idp.bearer("grpc-bind-admin", Some("grpc-bind-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;

    let subject_token = idp.bearer("grpc-bind-subject", Some("grpc-bind-subject@example.com"), "paigasus", 3600);
    support::provision(&state, &subject_token).await;

    let (addr, server) = spawn_server(state.clone()).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    // Before the policy: denied.
    let err = client.create_user(authed(create_user_request("grpc-before@example.com"), &subject_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    // Seed a static policy permitting EXACTLY CreateUser, authored by the platform_admin.
    let doc = paigasus_iam_core::PolicyDocument {
        policy_id: "sma-584-create-user-only-grpc".to_string(),
        kind: paigasus_iam_core::authz::model::PolicyKind::Static,
        source: r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#.to_string(),
        description: "SMA-584 action-identity pin: CreateUser only".to_string(),
        system: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let actor = paigasus_kernel::Prn::parse(&admin_prn).expect("valid principal prn");
    // `PolicyService::put(&self, actor: &Prn, doc: PolicyDocument)` — `doc` is taken BY VALUE.
    // It authorizes `Action::PutPolicy` at Root itself, which the seeded platform_admin holds.
    state.policies.put(&actor, doc).await.expect("platform_admin may PutPolicy at Root");

    // Now permitted...
    let resp = client.create_user(authed(create_user_request("grpc-bound@example.com"), &subject_token)).await.unwrap().into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}
