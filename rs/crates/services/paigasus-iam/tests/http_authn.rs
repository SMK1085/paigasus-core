// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `POST /v1/authn/introspect` (SMA-443 Task 10): the D10
//! read-only guarantee (unknown identity -> 403 `identity_not_provisioned`, never
//! provisions), the happy path after provisioning through `AuthnSvc::resolve(..,
//! Enabled)` — exercising the REAL discovery + JWKS fetch against the HTTPS mock IdP —
//! and the 401 `invalid_token` surfaces including the `WWW-Authenticate` challenge and
//! the oversized-token cap. Drives the real `router(AppState::new(db, &cfg))` via
//! `tower::ServiceExt::oneshot` against an ephemeral Postgres (Docker; see
//! `tests/support/mod.rs`).

mod support;

use axum::body::to_bytes;
use axum::http::StatusCode;
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::application::authenticate_token::Provisioning;
use serde_json::json;
use support::{send, send_raw, start_mock_idp, test_config};

#[tokio::test]
async fn introspect_unknown_identity_is_403_and_never_provisions() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // A perfectly valid token (trusted issuer, right audience, live signature) whose
    // (issuer, subject) has never been provisioned: introspect must 403, not JIT (D10).
    let token = idp.bearer("sub-unknown", Some("unknown@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity_not_provisioned");

    // Introspect again with the same token: still 403 — definitive evidence the first
    // call had no user-creation side effect (D10 read-only guarantee).
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity_not_provisioned");
}

#[tokio::test]
async fn introspect_resolved_identity_returns_full_context() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = start_mock_idp().await;
    let state = AppState::new(db, &test_config(&idp)).await.expect("AppState::new");
    let app = router(state.clone());

    // Provision through the use case with `Enabled` — the JIT path the Task 11
    // middleware will drive. This also exercises the real discovery + JWKS fetch
    // against the HTTPS mock IdP (self-signed cert, `accept_invalid_tls`).
    let token = idp.bearer("sub-alice", Some("alice@example.com"), "paigasus", 3600);
    let principal = state.authn.resolve(&token, Provisioning::Enabled).await.expect("resolve(Enabled) JIT-provisions");
    let principal_prn = principal.principal_id.canonical();

    // Introspect over HTTP: 200 with the full context; no memberships yet, and
    // role_group_prns stays empty until M3.
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal_prn"], principal_prn);
    assert_eq!(body["status"], "active");
    assert_eq!(body["issuer"], idp.issuer);
    assert_eq!(body["subject"], "sub-alice");
    assert!(body["expires_at"].is_string(), "expires_at must be an RFC3339 string: {body}");
    assert!(body["role_group_prns"].as_array().expect("role_group_prns array").is_empty());
    assert!(body["memberships"].as_array().expect("memberships array").is_empty());

    // Attach an org membership; introspect reflects it (D13: introspect is the one
    // entry point that assembles memberships).
    let (status, org) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let (status, membership) = send(&app, "POST", "/v1/memberships", Some(json!({ "principal_prn": principal_prn, "node_prn": org_prn })), None).await;
    assert_eq!(status, StatusCode::CREATED, "{membership}");

    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let memberships = body["memberships"].as_array().expect("memberships array");
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0]["principal_prn"], principal_prn);
    assert_eq!(memberships[0]["node_prn"], org_prn);
}

#[tokio::test]
async fn introspect_invalid_token_is_401_with_www_authenticate() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    let response = send_raw(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": "not-a-jwt" })), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer error=\"invalid_token\"");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
    // The static message never echoes the presented token (spec §6.3).
    assert!(!body["error"]["message"].as_str().unwrap().contains("not-a-jwt"), "{body}");
}

#[tokio::test]
async fn introspect_oversized_token_is_401_invalid_token() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // One byte over `test_config`'s max_token_bytes (16384): 401 `invalid_token` via the
    // validator's own length cap — the handler must NOT pre-filter (D10 note).
    let oversized = "a".repeat(16_385);
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": oversized })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid_token");
}
