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
use paigasus_proto::paigasus::common::v1::ErrorReason;
use paigasus_proto::paigasus::iam::v1::user_service_client::UserServiceClient;
use paigasus_proto::paigasus::iam::v1::{ChangeUserEmailRequest, CreateUserRequest, FindUserByEmailRequest, LinkExternalIdentityRequest, UnlinkExternalIdentityRequest};
use sea_orm::{EntityTrait, PaginatorTrait};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

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
    // Cloned BEFORE `AppState::new` consumes `db`, so the row-count assertion below can query the
    // same database independently — mirrors `tests/http_users.rs`'s identical setup.
    let count_db = db.clone();
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

    // Baseline taken AFTER both principals are provisioned, so the only thing that could move it
    // is the denied create below.
    let before = principal::Entity::find().count(&count_db).await.unwrap();

    // An ordinary, non-admin principal -> PermissionDenied.
    let err = client.create_user(authed(create_user_request("denied@example.com"), &plain_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    // The check must run BEFORE the use case: a denied create must not mint a principal row.
    // Without this the test would still pass if a later change moved the guard AFTER
    // `CreateUser::execute` and merely discarded the result — the HTTP twin asserts the same.
    let after = principal::Entity::find().count(&count_db).await.unwrap();
    assert_eq!(after, before, "a denied create must not mint a principal row");

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

/// SMA-584: the gRPC twin of `tests/authz_enforce_toggle.rs`'s
/// `enforce_tenancy_false_lets_an_otherwise_ungranted_principal_create_a_user`. Without this,
/// `enforce_tenancy` is pinned on the HTTP transport only — deleting the
/// `if self.state.enforce_tenancy` wrapper from `grpc::users` would leave gRPC always-checking
/// while HTTP bypasses, and every test would still pass. That is a transport divergence, which
/// is the one thing this issue exists to prevent, so both halves of the toggle are pinned.
#[tokio::test]
async fn enforce_tenancy_false_lets_an_ungranted_principal_create_a_user_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let state = AppState::new(db, &cfg).await.unwrap();

    let token = idp.bearer("grpc-toggle-tester", Some("grpc-toggle-tester@example.com"), "paigasus", 3600);
    // JIT-provision the principal but grant it NOTHING — under the default `enforce_tenancy =
    // true` this exact call is `PermissionDenied` (`create_user_requires_platform_admin`).
    support::provision(&state, &token).await;

    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let resp = client
        .create_user(authed(create_user_request("grpc-toggle-off@example.com"), &token))
        .await
        .expect("enforce_tenancy = false must bypass the CreateUser gate over gRPC")
        .into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}

// --- SMA-712: the operator identity RPCs ----------------------------------------------------

/// The four identity RPCs, by index, with the Cedar action each one checks.
const OPS: [&str; 4] = ["GetUser", "LinkExternalIdentity", "UnlinkExternalIdentity", "ChangeUserEmail"];

fn maybe_authed<T>(msg: T, token: Option<&str>) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    if let Some(token) = token {
        support::grpc_bearer(&mut req, token);
    }
    req
}

fn reason_of(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

fn missing_user_prn() -> String {
    format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0xdead))
}

/// Calls identity op `op` (an index into `OPS`) with well-formed values against `user_prn`.
async fn call_op(client: &mut UserServiceClient<Channel>, op: usize, token: Option<&str>, user_prn: &str, issuer: &str) -> Result<(), tonic::Status> {
    let user_prn = user_prn.to_string();
    match op {
        0 => client
            .find_user_by_email(maybe_authed(
                FindUserByEmailRequest {
                    email: "nobody-grpc@example.com".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        1 => client
            .link_external_identity(maybe_authed(
                LinkExternalIdentityRequest {
                    user_prn,
                    issuer: issuer.to_string(),
                    subject: "matrix-sub".to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        2 => client
            .unlink_external_identity(maybe_authed(
                UnlinkExternalIdentityRequest {
                    user_prn,
                    external_identity_id: Uuid::from_u128(0xbeef).to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        3 => client
            .change_user_email(maybe_authed(
                ChangeUserEmailRequest {
                    user_prn,
                    email: "matrix-grpc@example.com".to_string(),
                    reason: "matrix".to_string(),
                },
                token,
            ))
            .await
            .map(|_| ()),
        other => panic!("no identity op {other}"),
    }
}

/// SMA-712 spec 11: for each RPC, Unauthenticated without a bearer, PermissionDenied without
/// the action, and success for platform_admin, through a full link, change and unlink cycle.
#[tokio::test]
async fn identity_rpcs_need_a_bearer_and_their_action_and_work_for_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let plain_token = idp.bearer("grpc-id-plain", Some("grpc-id-plain@example.com"), "paigasus", 3600);
    support::provision(&state, &plain_token).await;
    let admin_token = idp.bearer("grpc-id-admin", Some("grpc-id-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let target = client
        .create_user(authed(create_user_request("grpc-id-target@example.com"), &admin_token))
        .await
        .unwrap()
        .into_inner()
        .principal_prn;

    for (op, name) in OPS.iter().enumerate() {
        let err = call_op(&mut client, op, None, &target, &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::Unauthenticated, "{name} without a bearer");
        let err = call_op(&mut client, op, Some(plain_token.as_str()), &target, &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{name} without the action");
    }

    let found = client
        .find_user_by_email(authed(
            FindUserByEmailRequest {
                email: "grpc-id-target@example.com".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .user
        .expect("user");
    assert_eq!(found.prn, target);
    assert_eq!(found.status, "active");
    assert!(found.external_identities.is_empty());

    let link = |subject: &str| LinkExternalIdentityRequest {
        user_prn: target.clone(),
        issuer: idp.issuer.clone(),
        subject: subject.to_string(),
        reason: "INC-1: same person".to_string(),
    };
    let linked = client
        .link_external_identity(authed(link("grpc-linked-sub"), &admin_token))
        .await
        .unwrap()
        .into_inner()
        .external_identity
        .expect("identity");
    assert_eq!(linked.subject, "grpc-linked-sub");
    assert_eq!(linked.issuer, idp.issuer);
    let again = client
        .link_external_identity(authed(link("grpc-linked-sub"), &admin_token))
        .await
        .unwrap()
        .into_inner()
        .external_identity
        .expect("identity");
    assert_eq!(again.id, linked.id, "a same-user link returns the stored identity");

    let err = client
        .link_external_identity(authed(
            LinkExternalIdentityRequest {
                user_prn: target.clone(),
                issuer: String::new(),
                subject: "x".to_string(),
                reason: "r".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert_eq!(
        reason_of(&err),
        ErrorReason::UnknownIssuer.as_wire_reason().unwrap(),
        "an empty issuer answers like a missing HTTP field"
    );

    let changed = client
        .change_user_email(authed(
            ChangeUserEmailRequest {
                user_prn: target.clone(),
                email: "grpc-id-moved@example.com".to_string(),
                reason: "INC-2: moved".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .user
        .expect("user");
    assert_eq!(changed.email, "grpc-id-moved@example.com");
    assert_eq!(changed.external_identities.len(), 1);

    let unlink = || UnlinkExternalIdentityRequest {
        user_prn: target.clone(),
        external_identity_id: linked.id.clone(),
        reason: "INC-3: undo".to_string(),
    };
    client.unlink_external_identity(authed(unlink(), &admin_token)).await.unwrap();
    let err = client.unlink_external_identity(authed(unlink(), &admin_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "a repeated unlink is not found");

    server.abort();
}

/// SMA-712 spec 11, action identity end to end: each subject holds a static policy that permits
/// exactly ONE of the four actions. It passes the check on that RPC (NotFound on a missing user)
/// and fails it on the other three (PermissionDenied). A role grant cannot show this, because
/// `platform_admin` permits all four.
#[tokio::test]
async fn each_identity_rpc_checks_its_own_action() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let state = AppState::new(db, &cfg).await.unwrap();
    let admin_token = idp.bearer("grpc-matrix-admin", Some("grpc-matrix-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;
    let admin = Prn::parse(&admin_prn).unwrap();

    let mut tokens = Vec::new();
    for (index, action) in OPS.iter().enumerate() {
        let token = idp.bearer(&format!("grpc-matrix-{index}"), Some(format!("grpc-matrix-{index}@example.com").as_str()), "paigasus", 3600);
        let prn = support::provision(&state, &token).await;
        let uuid = Prn::parse(&prn).unwrap().resource_id();
        let doc = paigasus_iam_core::PolicyDocument {
            policy_id: format!("sma-712-grpc-matrix-{index}"),
            kind: paigasus_iam_core::authz::model::PolicyKind::Static,
            source: format!(r#"permit(principal == Pgs::Iam::Principal::"{uuid}", action == Pgs::Iam::Action::"{action}", resource);"#),
            description: format!("SMA-712 action-identity pin: {action} only"),
            system: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        state.policies.put(&admin, doc).await.expect("platform_admin may PutPolicy at Root");
        tokens.push(token);
    }

    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);
    for (holder, token) in tokens.iter().enumerate() {
        for (op, name) in OPS.iter().enumerate() {
            let err = call_op(&mut client, op, Some(token.as_str()), &missing_user_prn(), &idp.issuer).await.unwrap_err();
            let want = if holder == op { Code::NotFound } else { Code::PermissionDenied };
            assert_eq!(err.code(), want, "a subject that holds only {} calls {name}", OPS[holder]);
        }
    }
    server.abort();
}

/// SMA-712 spec 5.1 and 11: the check runs before validation, and `enforce_tenancy = false`
/// does not open the calls. A denied caller with bad values gets PermissionDenied.
#[tokio::test]
async fn identity_rpcs_ignore_enforce_tenancy_and_authorize_before_validation() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("grpc-toggle-id", Some("grpc-toggle-id@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    for (op, name) in OPS.iter().enumerate() {
        let err = call_op(&mut client, op, Some(token.as_str()), &missing_user_prn(), &idp.issuer).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{name} must stay closed with enforce_tenancy = false");
    }

    let err = client
        .find_user_by_email(authed(FindUserByEmailRequest { email: "not-an-email".to_string() }, &token))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .link_external_identity(authed(
            LinkExternalIdentityRequest {
                user_prn: missing_user_prn(),
                issuer: "https://unknown.example.com/".to_string(),
                subject: " padded".to_string(),
                reason: String::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .unlink_external_identity(authed(
            UnlinkExternalIdentityRequest {
                user_prn: missing_user_prn(),
                external_identity_id: Uuid::from_u128(1).to_string(),
                reason: "   ".to_string(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    let err = client
        .change_user_email(authed(
            ChangeUserEmailRequest {
                user_prn: missing_user_prn(),
                email: "@".to_string(),
                reason: String::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);

    // Code review F4: the ONE value that is checked before the authorization is the syntax of
    // the id (plan deviation 5, the twin of the HTTP path extractor). A denied caller with a
    // malformed `user_prn` gets `invalid-prn`, not PermissionDenied. It learns only the syntax.
    // If a later change moves the parse behind the check, this assertion shows it.
    let err = client
        .change_user_email(authed(
            ChangeUserEmailRequest {
                user_prn: "not a prn".to_string(),
                email: "ok@example.com".to_string(),
                reason: "r".to_string(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "{err:?}");
    assert_eq!(reason_of(&err), "invalid-prn");

    server.abort();
}

/// Code review F1: SMA-649's stored-PRN rule on the three write RPCs. A `user_prn` with a
/// region or an organization slot names the real user by uuid, but it differs from the stored
/// `principal.prn`, so each call answers `InvalidArgument` / `prn-mismatch` and changes
/// nothing. A wrong resource type stays `invalid-prn`. Control: the canonical PRN works.
#[tokio::test]
async fn a_user_prn_with_a_region_or_an_org_slot_is_a_prn_mismatch() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin_token = idp.bearer("grpc-id-forged-admin", Some("grpc-id-forged-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let target = client
        .create_user(authed(create_user_request("grpc-id-forged@example.com"), &admin_token))
        .await
        .unwrap()
        .into_inner()
        .principal_prn;
    let uuid = Prn::parse(&target).unwrap().resource_id();
    let regional = format!("prn:pgs:iam:eu-west-1::principal/{uuid}");
    let org_slot = Prn::build("iam", "", Some(Uuid::from_u128(99)), "principal", uuid).unwrap().canonical();

    for forged in [&regional, &org_slot] {
        // Skip op 0 (`FindUserByEmail`): it takes no user PRN.
        for (op, name) in OPS.iter().enumerate().skip(1) {
            let err = call_op(&mut client, op, Some(admin_token.as_str()), forged, &idp.issuer).await.unwrap_err();
            assert_eq!(err.code(), Code::InvalidArgument, "{name} with {forged}: {err:?}");
            assert_eq!(reason_of(&err), "prn-mismatch", "{name} with {forged}");
        }
    }
    let wrong_type = format!("prn:pgs:iam:::organization/{uuid}");
    let err = call_op(&mut client, 1, Some(admin_token.as_str()), &wrong_type, &idp.issuer).await.unwrap_err();
    assert_eq!(reason_of(&err), "invalid-prn", "a wrong resource type is still a malformed user PRN");

    assert_eq!(
        principal::Entity::find_by_id(uuid).one(&db).await.unwrap().expect("principal row").prn,
        target,
        "the stored PRN is canonical"
    );
    assert_eq!(
        user::Entity::find_by_id(uuid).one(&db).await.unwrap().unwrap().email,
        "grpc-id-forged@example.com",
        "no forged email change"
    );
    assert_eq!(
        paigasus_iam::adapters::persistence::entities::external_identity::Entity::find().count(&db).await.unwrap(),
        1,
        "only the admin's own JIT identity exists: no forged link"
    );

    call_op(&mut client, 3, Some(admin_token.as_str()), &target, &idp.issuer)
        .await
        .expect("the canonical PRN is the control");
    server.abort();
}

/// Fix round 1 (review finding 2): the handler must forward the CALLER's real credential to the
/// service's own-identity guard, not a placeholder. The admin's bearer JIT-links an identity
/// (issuer `idp.issuer`, subject `grpc-id-own-guard-admin`, mirroring
/// `identity_rpcs_need_a_bearer_and_their_action_and_work_for_platform_admin`'s setup) as a side
/// effect of `support::provision_platform_admin`. Unlinking THAT identity, authenticated by the
/// SAME bearer, must be refused: `FailedPrecondition` with the `cannot-unlink-own-identity`
/// reason. A handler that passed an `ApiKey`/placeholder credential instead of `ctx.credential`
/// would never trip this guard and this test would see `Ok` or `NotFound` instead.
#[tokio::test]
async fn unlink_refuses_the_identity_that_authenticated_the_caller() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let admin_token = idp.bearer("grpc-id-own-guard-admin", Some("grpc-id-own-guard-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    let admin = client
        .find_user_by_email(authed(
            FindUserByEmailRequest {
                email: "grpc-id-own-guard-admin@example.com".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .user
        .expect("user");
    let own_identity = admin.external_identities.first().expect("JIT provisioning links the bearer's own identity").id.clone();

    let err = client
        .unlink_external_identity(authed(
            UnlinkExternalIdentityRequest {
                user_prn: admin.prn.clone(),
                external_identity_id: own_identity,
                reason: "INC-4: attempted self-unlink".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition, "{err:?}");
    assert_eq!(reason_of(&err), "cannot-unlink-own-identity");

    server.abort();
}
