// SPDX-License-Identifier: Apache-2.0

//! SMA-505 AC 1/2/3 over HTTP: the descriptor's shape, its authentication requirement, and the
//! surface half of "flip a flag, the key disappears" — the route is genuinely gone, not merely
//! unadvertised.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop the test skips — the same gating pattern as
//! `tests/authz_enforce_toggle.rs`. The DESCRIPTOR half of AC 3 is proven unconditionally by the
//! pure-predicate unit tests in `src/service_info.rs`, so a skipped daemon never leaves AC 3
//! entirely unproven.

mod support;

use axum::http::StatusCode;
use serde_json::Value;
use support::{app_with_config, provision, send, send_raw, test_config};

/// Reads the descriptor's capability list as a set (the proto declares the list unordered).
fn capability_set(body: &Value) -> std::collections::HashSet<String> {
    body["capabilities"]
        .as_array()
        .expect("capabilities must be an array, never absent")
        .iter()
        .map(|v| v.as_str().expect("capability keys are strings").to_string())
        .collect()
}

#[tokio::test]
async fn the_descriptor_requires_a_bearer_and_reports_every_enabled_capability() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let cfg = test_config(&idp);
    let (app, state) = app_with_config(db, &cfg).await;

    // AC 2: no credential -> 401, never 200 and never 404.
    let (status, _) = send(&app, "GET", "/v1/service-info", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the descriptor must not be an unauthenticated surface");

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;
    let (status, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // AC 1: exact shape.
    assert_eq!(body["service"], "iam");
    assert!(body["version"].as_str().is_some_and(|v| !v.is_empty()), "version must be a non-empty string");
    assert_eq!(
        capability_set(&body),
        std::collections::HashSet::from(["iam.authz.cedar".to_string(), "iam.apikeys".to_string(), "iam.audit".to_string()])
    );
}

/// AC 3, surface half: each flag off removes its own route AND its own key, and leaves the
/// siblings alone. The sibling assertion is what stops this passing against an implementation
/// that returns an empty list unconditionally.
#[tokio::test]
async fn disabling_audit_query_removes_both_the_route_and_the_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.audit.query_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let (status, _) = send(&app, "GET", "/v1/audit", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a disabled capability's route must be unmounted, not merely unadvertised");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.audit"), "the disabled key must be absent: {body}");
    assert!(caps.contains("iam.authz.cedar"), "siblings must survive: {body}");
    assert!(caps.contains("iam.apikeys"), "siblings must survive: {body}");
}

#[tokio::test]
async fn disabling_authz_admin_removes_policy_role_grant_and_retirement_routes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.admin_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    for path in ["/v1/authz/policies", "/v1/authz/role-grants"] {
        let (status, _) = send(&app, "GET", path, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} must be unmounted");
    }
    // The system-policy retirement route is mounted by the SAME `if caps.authz_admin` branch
    // as the two routes above (`adapters/http/mod.rs::app_routes`'s `.merge(system_retirement::
    // router())`), and the spec singles it out as the most privileged route this flag gates —
    // but nothing previously requested it, so a refactor moving that merge out of the branch
    // would leave it mounted with this test's name still claiming otherwise. `SystemRetirementService::retire`
    // authorizes FIRST, before any lookup (root-only) — so a MOUNTED route answers 403 for this
    // non-root token, never 404, which is what makes 404 here proof the route is actually gone
    // rather than an artifact of the made-up id or empty body. `send_raw`, not `send`: the
    // rejection body here is not guaranteed to be the crate's JSON envelope.
    let retire_path = "/v1/authz/system-policies/does-not-exist/retire";
    let response = send_raw(&app, "POST", retire_path, Some(serde_json::json!({})), Some(token.as_str())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{retire_path} must be unmounted");
    // is-authorized is a service-to-service primitive and stays mounted regardless — anything
    // other than 404 proves it is still routed. An empty `{}` body fails `IsAuthorizedBody`'s
    // required-field deserialization, and axum's own `Json` extractor rejection renders as
    // plain text rather than the crate's JSON envelope — so this goes through `send_raw`
    // (status only) rather than `send`, which would panic trying to parse that body as JSON.
    let response = send_raw(&app, "POST", "/v1/authz/is-authorized", Some(serde_json::json!({})), Some(token.as_str())).await;
    assert_ne!(response.status(), StatusCode::NOT_FOUND, "is-authorized must stay mounted so the gateway keeps working");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.authz.cedar"), "{body}");
    assert!(caps.contains("iam.apikeys") && caps.contains("iam.audit"), "siblings must survive: {body}");
}

#[tokio::test]
async fn disabling_apikey_management_removes_management_but_keeps_introspection() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.api_keys.management_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let sa = "00000000-0000-0000-0000-000000000001";
    // A GET here would 404 from `ApiKeyService::list` itself (it looks the service account up
    // BEFORE authorizing, and no such account exists in a freshly migrated DB), so it cannot
    // distinguish "route unmounted" from "no such service account" — a mounted route would 404
    // too, making the assertion vacuous. A POST with an empty body instead reaches the HANDLER's
    // own `scope_prn is required` check (`adapters::http::api_keys::issue`) before any
    // application-layer lookup, which is a 400 whenever the route IS mounted — so 404 here
    // genuinely proves unmounting.
    let (status, _) = send(&app, "POST", &format!("/v1/service-accounts/{sa}/api-keys"), Some(serde_json::json!({})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "key management must be unmounted");

    // Introspection is a service-to-service primitive the gateway calls per request.
    let (status, _) = send(&app, "POST", "/v1/authn/api-keys/introspect", Some(serde_json::json!({"token": "nope"})), None).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "introspection must stay mounted so the gateway keeps working");

    let (_, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    let caps = capability_set(&body);
    assert!(!caps.contains("iam.apikeys"), "{body}");
    assert!(caps.contains("iam.authz.cedar") && caps.contains("iam.audit"), "siblings must survive: {body}");
}

/// The empty-list case SMA-499 § 2.7's MUST-emit-defaults rule exists for, and the multi-flag
/// combination R3 warns about: conditional merging must not panic at router registration.
#[tokio::test]
async fn all_capabilities_disabled_serves_an_empty_array_not_a_missing_field() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.admin_enabled = false;
    cfg.api_keys.management_enabled = false;
    cfg.audit.query_enabled = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("descriptor-reader", Some("reader@example.com"), "paigasus", 3600);
    provision(&state, &token).await;

    let (status, body) = send(&app, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["capabilities"], serde_json::json!([]), "capabilities must be emitted as [], never omitted: {body}");
}
