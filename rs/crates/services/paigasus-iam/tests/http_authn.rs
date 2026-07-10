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
use support::{send, send_raw, send_raw_parts, start_mock_idp, test_config, test_config_with};
use uuid::Uuid;

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
    // role_grants stays empty until a later M3 task populates it.
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal_prn"], principal_prn);
    assert_eq!(body["status"], "active");
    assert_eq!(body["issuer"], idp.issuer);
    assert_eq!(body["subject"], "sub-alice");
    assert!(body["expires_at"].is_string(), "expires_at must be an RFC3339 string: {body}");
    assert!(body["role_grants"].as_array().expect("role_grants array").is_empty());
    assert!(body["memberships"].as_array().expect("memberships array").is_empty());

    // Attach an org membership; introspect reflects it (D13: introspect is the one
    // entry point that assembles memberships). These are PROTECTED tenancy routes, so they
    // carry the same bearer token (Task 11 enforcement); introspect itself stays bearer-free.
    let (status, org) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let (status, membership) = send(&app, "POST", "/v1/memberships", Some(json!({ "principal_prn": principal_prn, "node_prn": org_prn })), Some(&token)).await;
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

#[tokio::test]
async fn introspect_oversized_body_is_413_request_too_large() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // Past max_token_bytes (16384) + the 1024-byte envelope headroom: rejected by the
    // route-level body limit BEFORE JSON parsing, in the standard envelope (H1). The
    // 401 band just above max_token_bytes stays covered by
    // introspect_oversized_token_is_401_invalid_token — the two-tier behavior is by design.
    let huge = format!(r#"{{"token":"{}"}}"#, "a".repeat(20_000));
    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("application/json"), Some(huge.into_bytes())).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "request_too_large");
    assert_eq!(body["error"]["message"], "request body too large");
}

#[tokio::test]
async fn introspect_malformed_json_is_enveloped() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // Broken JSON must render the same {"error":{code,message}} envelope as every other
    // authn error — not axum's default plain-text rejection (H1).
    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("application/json"), Some(b"{not json".to_vec())).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(body["error"]["message"], "invalid request body");
}

#[tokio::test]
async fn introspect_wrong_content_type_is_enveloped() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    let response = send_raw_parts(&app, "POST", "/v1/authn/introspect", None, Some("text/plain"), Some(br#"{"token":"x"}"#.to_vec())).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
}

// --- Task 11: bearer enforcement on the protected `/v1` surface (D14, spec §7.4) ---

#[tokio::test]
async fn protected_route_without_token_is_401() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // No `Authorization` header at all on a protected tenancy route: 401 with the same
    // `invalid_token` body as any rejected credential, but a BARE `Bearer` challenge —
    // RFC 6750 §3.1 says a request with no authentication information gets a challenge
    // without an error attribute (H3). Only the header distinguishes the cases.
    let response = send_raw(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
}

#[tokio::test]
async fn present_but_malformed_authorization_keeps_error_challenge() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // A PRESENT-but-unusable header (foreign scheme) is NOT "missing credentials": the
    // client did attempt authentication, so the challenge keeps the error attribute
    // (H3 differentiates only the fully-absent case).
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some("Basic dXNlcjpwdw=="), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer error=\"invalid_token\"");
}

#[tokio::test]
async fn protected_route_with_invalid_token_is_401_with_www_authenticate() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;

    // A syntactically-broken bearer token on a protected GET: the validator rejects it and
    // the middleware funnels that through the same 401 `invalid_token` + challenge path.
    let response = send_raw(&app, "GET", "/v1/organizations", None, Some("not-a-jwt")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers().get("www-authenticate").expect("WWW-Authenticate header").to_str().unwrap();
    assert_eq!(challenge, "Bearer error=\"invalid_token\"");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
}

#[tokio::test]
async fn fused_bearer_scheme_is_401() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // A perfectly VALID token, but the scheme is fused with the credential ("Bearer<jwt>",
    // no space): header parsing requires `<scheme> <credential>`, so this must 401 without
    // the token ever reaching the validator.
    let token = idp.bearer("fused-scheme", Some("fused@example.com"), "paigasus", 3600);
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some(&format!("Bearer{token}")), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "invalid_token");
}

#[tokio::test]
async fn lowercase_bearer_scheme_is_accepted() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // RFC 7235 §2.1: the auth-scheme is case-insensitive, so `bearer <jwt>` must
    // authenticate exactly like `Bearer <jwt>` (and JIT-provision on the way in).
    let token = idp.bearer("lowercase-bearer", Some("lower@example.com"), "paigasus", 3600);
    let response = send_raw_parts(&app, "GET", "/v1/organizations", Some(&format!("bearer {token}")), None, None).await;
    assert_eq!(response.status(), StatusCode::OK, "a lowercase bearer scheme must be accepted");
}

#[tokio::test]
async fn protected_route_with_valid_token_succeeds_and_jit_provisions() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // A valid token for a brand-new (issuer, subject): the middleware's resolve(.., Enabled)
    // JIT-provisions the principal on the way in (AC 2), so the protected write succeeds.
    let token = idp.bearer("jit-newcomer", Some("newcomer@example.com"), "paigasus", 3600);
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Introspect the SAME token (bearer-free — introspect is exempt): the identity now
    // resolves to a real principal. Since introspect never provisions (D10), a 200 here is
    // only possible because the middleware already ran JIT on the write above — the AC 2 proof.
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["subject"], "jit-newcomer");
    assert_eq!(body["issuer"], idp.issuer);
    assert!(body["principal_prn"].as_str().is_some_and(|prn| prn.starts_with("prn:pgs:iam:::principal/")), "{body}");
}

#[tokio::test]
async fn readyz_and_introspect_do_not_require_bearer() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    // `/readyz` is exempt: reachable (200) with no `Authorization` header at all.
    let (status, body) = send(&app, "GET", "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready");

    // `POST /v1/authn/introspect` is exempt too: a bearer-free request reaches the handler.
    // The token (in the body, not the header) is a valid but never-provisioned identity, so
    // the handler's own D10 read-only path answers 403 `identity_not_provisioned` — crucially
    // NOT the middleware's 401 `invalid_token`, the tell-tale of an enforced route.
    let token = idp.bearer("introspect-exempt", Some("exempt@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity_not_provisioned");
}

#[tokio::test]
async fn jit_disabled_unknown_identity_is_403() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let jit_enabled = start_mock_idp().await;
    let jit_disabled = start_mock_idp().await;
    // Two configured issuers; the second has jit_provisioning = false.
    let state = AppState::new(db, &test_config_with(&[(&jit_enabled, true), (&jit_disabled, false)], 30)).await.expect("AppState::new");
    let app = router(state);

    // A valid token from the JIT-disabled issuer for an unknown identity: the middleware
    // verifies the signature but refuses to provision (per-issuer flag, D5), so the request
    // is 403 `identity_not_provisioned` instead of a JIT success.
    let token = jit_disabled.bearer("no-jit-user", Some("nojit@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity_not_provisioned");
}

#[tokio::test]
async fn key_rotation_validates_tokens_signed_with_the_new_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let mut idp = start_mock_idp().await;
    // Cooldown 0 so the kid-miss refetch triggered by the post-swap token isn't suppressed
    // by the cooldown from the first fetch. The default-cooldown suppression path is already
    // unit-tested in the JWKS provider (spec §4.3), so this test targets the swap-then-succeed
    // path only.
    let state = AppState::new(db, &test_config_with(&[(&idp, true)], 0)).await.expect("AppState::new");
    let app = router(state);

    // 1. A token under the ORIGINAL key validates (and warms the per-issuer JWKS cache).
    let before = idp.bearer("rotating-user", Some("rotate@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "pre", "name": "Pre" })), Some(&before)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // 2. Rotate the IdP's signing key: the served JWKS now carries only the NEW kid, while
    // the validator's cache still holds the old one.
    idp.rotate();

    // 3. A token under the NEW key: its unknown kid forces a single refetch (cooldown 0), the
    // IdP serves the rotated JWKS, the signature verifies against the fresh key, and the
    // protected write succeeds.
    let after = idp.bearer("rotating-user", Some("rotate@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "post", "name": "Post" })), Some(&after)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Every `/v1` route the tenancy + authz sub-routers expose (organizations/teams/projects/
/// memberships/users/authz), paired with its method — the enumeration mirrors
/// `src/adapters/http/{organizations,teams,projects,memberships,users,authz}.rs` route
/// tables exactly. When adding a new /v1 route, it must appear here — this test converts
/// default-open HTTP routing into a tested invariant (final-review Important 3).
#[tokio::test]
async fn every_protected_v1_route_requires_bearer() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, idp) = support::app(db).await;

    let id = Uuid::nil();
    let protected_routes: Vec<(&str, String)> = vec![
        // organizations.rs
        ("POST", "/v1/organizations".to_string()),
        ("GET", "/v1/organizations".to_string()),
        ("GET", format!("/v1/organizations/{id}")),
        ("PATCH", format!("/v1/organizations/{id}")),
        ("POST", format!("/v1/organizations/{id}/archive")),
        ("POST", format!("/v1/organizations/{id}/restore")),
        ("POST", format!("/v1/organizations/{id}/teams")),
        ("GET", format!("/v1/organizations/{id}/teams")),
        // teams.rs
        ("GET", format!("/v1/teams/{id}")),
        ("PATCH", format!("/v1/teams/{id}")),
        ("POST", format!("/v1/teams/{id}/archive")),
        ("POST", format!("/v1/teams/{id}/restore")),
        ("POST", format!("/v1/teams/{id}/projects")),
        ("GET", format!("/v1/teams/{id}/projects")),
        // projects.rs
        ("GET", format!("/v1/projects/{id}")),
        ("PATCH", format!("/v1/projects/{id}")),
        ("POST", format!("/v1/projects/{id}/archive")),
        ("POST", format!("/v1/projects/{id}/restore")),
        // memberships.rs
        ("POST", "/v1/memberships".to_string()),
        ("GET", "/v1/memberships".to_string()),
        ("DELETE", format!("/v1/memberships/{id}")),
        // users.rs
        ("POST", "/v1/users".to_string()),
        // authz.rs
        ("POST", "/v1/authz/is-authorized".to_string()),
        ("POST", "/v1/authz/policies".to_string()),
        ("GET", "/v1/authz/policies".to_string()),
        ("DELETE", "/v1/authz/policies/some-policy-id".to_string()),
        ("POST", "/v1/authz/role-grants".to_string()),
        ("GET", "/v1/authz/role-grants".to_string()),
        ("DELETE", format!("/v1/authz/role-grants/{id}")),
    ];

    for (method, path) in &protected_routes {
        let response = send_raw(&app, method, path, None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "route {method} {path} must be 401 without a bearer token");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "invalid_token", "route {method} {path}: unexpected body {body}");
    }

    // Expected-200-without-token: liveness/readiness stay reachable with no bearer at all.
    let (status, body) = send(&app, "GET", "/healthz", None, None).await;
    assert_eq!(status, StatusCode::OK, "/healthz must be reachable without a bearer: {body}");
    let (status, body) = send(&app, "GET", "/readyz", None, None).await;
    assert_eq!(status, StatusCode::OK, "/readyz must be reachable without a bearer: {body}");

    // Exception: POST /v1/authn/introspect is deliberately bearer-free (D10) — the credential
    // travels in the body, not the header. Proof it's NOT enforced: an unprovisioned-but-valid
    // token reaches the handler and gets the handler's OWN 403 (identity_not_provisioned), not
    // the middleware's 401 invalid_token that every route above returns when bearer-free.
    let token = idp.bearer("sweep-exempt", Some("sweep@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "/v1/authn/introspect must stay exempt from bearer enforcement: {body}");
    assert_eq!(body["error"]["code"], "identity_not_provisioned");
}
