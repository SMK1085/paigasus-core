// SPDX-License-Identifier: Apache-2.0

//! Cold-start bootstrap-admin seeding, end-to-end (SMA-444 Task 21b, spec D9/challenge M4):
//! `authz.bootstrap_admins` configures a set of `(issuer, subject)` OIDC identities that
//! must be JIT-granted `platform_admin`@`Root` the first time they authenticate — a fresh
//! deployment otherwise has nobody who can create organizations or grant roles. Drives the
//! real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` against the mock
//! IdP (mirrors `tests/http_authn.rs`/`tests/http_tenancy.rs`), so the seeding is exercised
//! through the ACTUAL HTTP bearer middleware call site
//! (`adapters::http::auth_middleware::require_bearer`), not a direct
//! `BootstrapAdminSeeder::ensure_platform_admin` call (that's `application::bootstrap_admin`'s
//! own fast, DB-free unit suite).
//!
//! Runs against an ephemeral Postgres in Docker; see `tests/support/mod.rs`'s doc comment
//! for the CI/local skip gating every test file here shares.

mod support;

use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::BootstrapAdmin;
use paigasus_iam_core::GrantScope;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{AccessRequest, Action, Authorizer, Effect, RequestContext};
use serde_json::json;
use support::{app_with_config, send, test_config_with};

#[tokio::test]
async fn bootstrap_identity_is_seeded_platform_admin_on_first_authentication_and_can_create_an_organization() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "bootstrap-sub".to_string(),
    }];
    let (app, state) = app_with_config(db, &cfg).await;

    // A fresh identity, never seen before: the middleware's `resolve(.., Enabled)` JIT-
    // provisions the principal AND (this task) seeds it `platform_admin`@`Root` — both
    // before the `POST /v1/organizations` handler ever runs, so the very first authenticated
    // request from this identity already succeeds where an ordinary (non-bootstrap) identity
    // would get a 403 (every other http_*.rs integration test seeds a grant up front for
    // exactly this reason — `seed_platform_admin`/`provision_platform_admin` in
    // `tests/support/mod.rs`).
    let token = idp.bearer("bootstrap-sub", Some("bootstrap-admin@example.com"), "paigasus", 3600);
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::CREATED,
        "a configured bootstrap-admin identity's first authenticated request must already succeed: {created}"
    );

    // Resolve the same identity again (a read-only, side-effect-free lookup once already
    // provisioned) to get its principal id for direct assertions against the store/authorizer.
    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("already provisioned");

    // `POST /v1/organizations` also seeds its creator an `org_admin` owner grant on the new
    // org (D8, unrelated to this task) — so filter for the platform_admin@Root grant
    // specifically rather than asserting on the grant list's raw length.
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    let platform_admin_grants: Vec<_> = grants.iter().filter(|g| g.role_key == "platform_admin" && g.scope == GrantScope::Root).collect();
    assert_eq!(platform_admin_grants.len(), 1, "exactly one platform_admin grant must have been seeded: {grants:?}");

    let decision = state
        .authz
        .is_authorized(&AccessRequest {
            principal: principal.principal_id.prn().clone(),
            action: Action::CreateOrganization,
            resource: root_prn(),
            context: RequestContext::empty(),
        })
        .await
        .expect("is_authorized");
    assert_eq!(
        decision.effect,
        Effect::Allow,
        "the seeded platform_admin grant must authorize CreateOrganization at Root: {decision:?}"
    );
}

#[tokio::test]
async fn a_second_authentication_by_the_same_bootstrap_identity_does_not_duplicate_the_grant() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "bootstrap-sub".to_string(),
    }];
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("bootstrap-sub", Some("bootstrap-admin@example.com"), "paigasus", 3600);

    // First authenticated request: seeds the grant (proven above).
    let (status, _) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);

    // A second, independent authenticated request by the SAME identity — the middleware runs
    // `ensure_platform_admin` again; it must find the existing grant and no-op.
    let (status, _) = send(&app, "GET", "/v1/organizations", None, Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("already provisioned");
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    let platform_admin_grants: Vec<_> = grants.iter().filter(|g| g.role_key == "platform_admin" && g.scope == GrantScope::Root).collect();
    assert_eq!(platform_admin_grants.len(), 1, "a second authentication must not create a duplicate platform_admin grant: {grants:?}");
}

#[tokio::test]
async fn a_non_bootstrap_identity_gets_no_platform_admin_grant_and_stays_forbidden() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "bootstrap-sub".to_string(),
    }];
    let (app, state) = app_with_config(db, &cfg).await;

    // Same issuer, but a DIFFERENT subject — not in the configured bootstrap set.
    let token = idp.bearer("ordinary-user", Some("ordinary@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "an ordinary (non-bootstrap) identity must stay unauthorized: {body}");

    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("JIT-provisioned by the request above despite the 403");
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    assert!(grants.is_empty(), "an ordinary (non-bootstrap) identity must get no role grant at all: {grants:?}");
}

/// The gRPC enforcement layer (`adapters::grpc::authn::AuthEnforce`) calls the exact same
/// `BootstrapAdminSeeder::ensure_platform_admin` the HTTP middleware does — proves the second
/// call site is actually wired, not just the HTTP one.
#[tokio::test]
async fn bootstrap_identity_is_seeded_over_grpc_too() {
    use paigasus_iam::adapters::grpc;
    use paigasus_proto::paigasus::iam::v1::CreateOrganizationRequest;
    use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
    use tokio::net::TcpListener;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "grpc-bootstrap-sub".to_string(),
    }];
    let state = paigasus_iam::adapters::http::AppState::new(db, &cfg).await.expect("AppState::new");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state.clone(), std::time::Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });

    let channel = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut tenancy = TenancyServiceClient::new(channel);

    let token = idp.bearer("grpc-bootstrap-sub", Some("grpc-bootstrap@example.com"), "paigasus", 3600);
    let mut req = tonic::Request::new(CreateOrganizationRequest {
        slug: "acme-grpc".into(),
        name: "Acme gRPC".into(),
    });
    support::grpc_bearer(&mut req, &token);
    let response = tenancy.create_organization(req).await;
    assert!(response.is_ok(), "a bootstrap-admin identity's first gRPC request must succeed: {response:?}");

    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("already provisioned");
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    assert!(
        grants.iter().any(|g| g.role_key == "platform_admin" && g.scope == GrantScope::Root),
        "the gRPC enforcement layer must have seeded the same platform_admin grant: {grants:?}"
    );

    server.abort();
}
