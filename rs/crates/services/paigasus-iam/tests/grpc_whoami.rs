// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `AuthnService.WhoAmI` (SMA-632).
//!
//! `WhoAmI` is bearer-enforced BY OMISSION: it is deliberately absent from
//! `adapters::grpc::authn::is_exempt`, so `AuthEnforce` resolves its bearer with
//! `Provisioning::Enabled` and seeds the bootstrap `platform_admin` grant BEFORE the handler
//! runs. The handler itself never sees a token — it reads the `AuthContext` the middleware
//! inserted (`adapters::grpc::authn::actor_context`).
//!
//! `who_am_i_provisions_a_new_identity` below is the primary control this file exists to add:
//! before SMA-632, the consoles got a principal provisioned only as a side effect of calling
//! `ServiceInfoService.GetServiceInfo`, and no IAM test failed if that side effect broke. Step 4
//! of the task brief proves this test (plus the reason-asserting bearer test and the
//! `who_am_i_is_not_exempt` unit test in `adapters::grpc::authn`) actually catches a regression:
//! temporarily adding `WhoAmI` to `is_exempt` fails all three independently.
//!
//! Docker-gated exactly like `tests/grpc_service_info.rs`; see `tests/support/mod.rs`'s module
//! doc for the CI/local skip gating every test file here shares.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::{AppState, router as http_router};
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::BootstrapAdmin;
use paigasus_iam_core::{AuthnError, GrantScope};
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::service_account_service_client::ServiceAccountServiceClient;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{AttachMembershipRequest, CreateOrganizationRequest, CreateServiceAccountRequest, IntrospectRequest, IssueApiKeyRequest, WhoAmIRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;

/// Spawns the full `grpc::router` on an ephemeral port; `abort()` the returned handle when the
/// test finishes. Mirrors `tests/grpc_service_info.rs::spawn_server` exactly.
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

/// Reads `ErrorInfo.reason` off a `tonic::Status` — decodes the `google.rpc.ErrorInfo` from the
/// `grpc-status-details-bin` trailer. `tests/support/` has no shared helper for this yet;
/// mirrors the per-file `reason`/`reason_of` shape already used by `tests/grpc_tenancy.rs` and
/// `tests/grpc_authz.rs`.
fn reason_of(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

/// The bearer test that MUST assert the reason, not just the code. `missing-auth-context` (what
/// the handler answers when the `AuthContext` extension is absent) and enforcement's own
/// rejection are BOTH `Unauthenticated`, so a code-only assertion would still pass with `WhoAmI`
/// added to `is_exempt` — exactly the regression this file exists to catch.
#[tokio::test]
async fn who_am_i_without_a_bearer_is_rejected_by_enforcement() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);

    // No `authorization` metadata at all.
    let status = client.who_am_i(tonic::Request::new(WhoAmIRequest {})).await.unwrap_err();

    assert_eq!(status.code(), Code::Unauthenticated, "{status:?}");
    assert_eq!(
        reason_of(&status),
        "invalid-token",
        "expected ENFORCEMENT's rejection, not the handler's missing-auth-context: {status:?}"
    );

    server.abort();
}

/// SMA-632's whole point, and the primary control for the feature. A token for an identity IAM
/// has never seen: (1) `Introspect` refuses it — the exempt, non-provisioning path; (2) ONE
/// `WhoAmI` call succeeds and names a principal; (3) `Introspect` now succeeds, because step 2
/// provisioned. No `GetServiceInfo` call anywhere — that dependency is what this feature removes.
#[tokio::test]
async fn who_am_i_provisions_a_new_identity() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-whoami-new", Some("grpc-whoami-new@example.com"), "paigasus", 3600);
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);

    // 1. Introspect refuses an identity IAM has never seen — read-only, never provisions (D10).
    let err = client.introspect(IntrospectRequest { token: token.clone() }).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert_eq!(reason_of(&err), "identity-not-provisioned", "{err:?}");

    // 2. ONE WhoAmI call succeeds and names a principal. If WhoAmI were ever added to
    //    `is_exempt`, `AuthEnforce` would forward without an `AuthContext` and the handler
    //    would answer `missing-auth-context` here instead.
    let who = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap().into_inner();
    assert!(who.principal_prn.starts_with("prn:pgs:iam:::principal/"), "{}", who.principal_prn);
    assert_eq!(who.issuer, idp.issuer);
    assert_eq!(who.subject, "grpc-whoami-new");

    // 3. Introspect now succeeds, because step 2 provisioned — same principal.
    let ctx = client.introspect(IntrospectRequest { token }).await.unwrap().into_inner();
    assert_eq!(ctx.principal_prn, who.principal_prn);

    server.abort();
}

/// Bearer enforcement is necessary but not sufficient. `AuthenticateToken::resolve` still checks
/// the issuer's JIT flag under `Provisioning::Enabled` (the `!self.jit.allows` branch of
/// `AuthenticateToken::resolve`, which calls `jit_disabled`), so an issuer with JIT off gets the
/// same `identity-not-provisioned` Introspect gives — the console keeps a branch for it. SMA-707
/// T2: the refusal also writes one `info` line that names the issuer, and not the subject or the
/// email. The capture sees the spawned server, as
/// `who_am_i_with_a_token_without_email_logs_the_provisioning_failure` proves.
#[tokio::test]
async fn who_am_i_obeys_the_jit_policy() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = paigasus_logging::test_support::capture_logs();
    let idp = support::start_mock_idp().await;
    let cfg = support::test_config_with(&[(&idp, false)], 30);
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("t2-jd-subject-9c1e", Some("t2-jd-mail-9c1e@example.com"), "paigasus", 3600);
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);

    let err = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert_eq!(reason_of(&err), "identity-not-provisioned", "{err:?}");

    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT-disabled line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", idp.issuer)), "the line names the issuer: {line}");
    assert!(!text.contains("t2-jd-subject-9c1e"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t2-jd-mail-9c1e"), "the log must not contain the email:\n{text}");

    server.abort();
}

/// The memberships a console renders from. Attach one, then read it back through WhoAmI.
#[tokio::test]
async fn who_am_i_returns_memberships() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-whoami-member", Some("grpc-whoami-member@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut tenancy = TenancyServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

    let org = tenancy
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "acme-whoami".to_string(),
                name: "Acme WhoAmI".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .organization
        .expect("organization");

    let who = authn.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap().into_inner();
    let principal_prn = who.principal_prn.clone();
    assert!(who.memberships.is_empty(), "no memberships yet: {who:?}");

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

    let who = authn.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap().into_inner();
    assert_eq!(who.memberships.len(), 1, "{:?}", who.memberships);
    assert_eq!(who.memberships[0].principal_prn, principal_prn);
    assert_eq!(who.memberships[0].node_prn, org.prn);

    server.abort();
}

/// One WhoAmI call is enough for a configured bootstrap identity to hold platform_admin — the
/// second side effect the consoles used to get from GetServiceInfo. Asserted through
/// `state.role_grant_store.list_by_principal(..)`, following `tests/authz_bootstrap_admin.rs`.
#[tokio::test]
async fn who_am_i_seeds_the_bootstrap_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "grpc-whoami-bootstrap".to_string(),
    }];
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("grpc-whoami-bootstrap", Some("grpc-whoami-bootstrap@example.com"), "paigasus", 3600);
    let (addr, server) = spawn_server(state.clone()).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);

    // Before-state (SMA-666). The principal does not exist yet, so a query by principal ID is not
    // possible. A grant needs a principal, so "not provisioned" proves "no grant" before the call.
    // `Provisioning::Disabled` writes nothing. It only fills the JWKS cache.
    let before = state.authn.resolve(&token, Provisioning::Disabled).await;
    assert!(
        matches!(before, Err(AuthnError::IdentityNotProvisioned)),
        "the bootstrap identity must not exist before the WhoAmI call, or its grant proves nothing: {before:?}",
    );

    let who = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap().into_inner();

    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("already provisioned by the WhoAmI call above");
    assert_eq!(who.principal_prn, principal.principal_id.canonical());
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    let platform_admin_grants: Vec<_> = grants.iter().filter(|g| g.role_key == "platform_admin" && g.scope == GrantScope::Root).collect();
    assert_eq!(platform_admin_grants.len(), 1, "one WhoAmI call must be enough to seed the bootstrap grant: {grants:?}");

    server.abort();
}

/// D4: WhoAmI serves both credential kinds. A service account's key names its principal, and
/// issuer/subject are empty — it has no (issuer, subject) pair. Follows
/// `tests/api_keys_grpc.rs::grpc_issue_and_introspect_parity` for creating a service account and
/// issuing a key.
#[tokio::test]
async fn who_am_i_serves_an_api_key_bearer() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-whoami-sa-owner", Some("grpc-whoami-sa-owner@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let owner = support::seed_org_ref(&state.db).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut sa_client = ServiceAccountServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

    let created = sa_client
        .create_service_account(authed(
            CreateServiceAccountRequest {
                owner_prn: owner.canonical(),
                name: "whoami-bot".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    let sa_prn = created.prn.clone();

    let issued = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: sa_prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: None,
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();

    let who = authn.who_am_i(authed(WhoAmIRequest {}, &issued.token)).await.unwrap().into_inner();
    assert_eq!(who.principal_prn, sa_prn);
    assert_eq!(who.issuer, "", "a service account has no (issuer, subject) pair: {who:?}");
    assert_eq!(who.subject, "", "a service account has no (issuer, subject) pair: {who:?}");

    server.abort();
}

/// The two transports describe the same principal, per the service's parity convention. POST,
/// not GET (D7) — the call creates a principal row and, for a configured bootstrap identity,
/// seeds a grant, so it must not be a safe/idempotent-by-convention method.
#[tokio::test]
async fn who_am_i_grpc_http_parity() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    // ONE AppState, served over both transports — the whole point of this test.
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-whoami-parity", Some("grpc-whoami-parity@example.com"), "paigasus", 3600);

    let http = http_router(state.clone());
    let (status, http_body) = support::send(&http, "POST", "/v1/authn/whoami", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{http_body}");

    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);
    let grpc_who = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap().into_inner();

    assert_eq!(http_body["principal_prn"], grpc_who.principal_prn, "principal_prn must agree across transports");
    assert_eq!(http_body["status"], grpc_who.status, "status must agree across transports");
    assert_eq!(http_body["issuer"], grpc_who.issuer, "issuer must agree across transports");
    assert_eq!(http_body["subject"], grpc_who.subject, "subject must agree across transports");

    server.abort();
}

/// SMA-698 T2: the same token on `WhoAmI` is `PermissionDenied` with reason `provisioning-failed`,
/// and IAM writes one `warn` line. The tests here run on the default current-thread runtime, so
/// the spawned tonic server runs on the test thread and the thread-local subscriber sees it.
#[tokio::test]
async fn who_am_i_with_a_token_without_email_logs_the_provisioning_failure() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = paigasus_logging::test_support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("t2-no-email-subject", None, "paigasus", 3600);
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthnServiceClient::new(channel(addr).await);

    let status = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();

    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");
    assert_eq!(reason_of(&status), "provisioning-failed", "{status:?}");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT failure line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("WARN"), "the line is at warn: {line}");
    assert!(line.contains("missing_email"), "the line names the defect: {line}");
    assert!(line.contains(&idp.issuer), "the line names the issuer: {line}");
    assert!(!text.contains("t2-no-email-subject"), "the log must not contain the subject:\n{text}");

    server.abort();
}

/// SMA-707 T4 (Review Focus R3): production serves HTTP and gRPC from ONE `AppState`, so the two
/// transports share one limiter. A refusal on HTTP and then one on gRPC, for the same
/// JIT-disabled issuer inside the 10 s window, write one line together.
#[tokio::test]
async fn jit_disabled_refusals_on_http_and_grpc_share_one_line() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = paigasus_logging::test_support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config_with(&[(&idp, false)], 30)).await.unwrap();
    let token = idp.bearer("t4-jd-subject-3a7f", Some("t4-jd-mail-3a7f@example.com"), "paigasus", 3600);
    let http = http_router(state.clone());
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthnServiceClient::new(channel(addr).await);

    let (status, body) = support::send(&http, "GET", "/v1/organizations", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity-not-provisioned");
    let err = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert_eq!(reason_of(&err), "identity-not-provisioned", "{err:?}");

    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "both transports share one limiter:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", idp.issuer)), "the line names the issuer: {line}");
    assert!(!text.contains("t4-jd-subject-3a7f"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t4-jd-mail-3a7f"), "the log must not contain the email:\n{text}");

    server.abort();
}
