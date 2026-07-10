// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/authz/*` (SMA-444 Task 18): `is-authorized`'s
//! self-query default-deny and its self/admin exposure rule's 403 on an unauthorized
//! non-self query; `POST /v1/authz/policies` forbidden for a non-admin, then succeeding once
//! the actor holds `platform_admin`; and the role-grant grant -> list -> revoke lifecycle.
//! Drives the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no
//! listening socket — against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).
//!
//! Every scenario needs the acting principal provisioned (a first authenticated call
//! JIT-provisions it) and, for the admin-only scenarios, a `platform_admin` grant seeded
//! directly through `AppState.role_grant_store` — `RoleService::grant`'s own anti-escalation
//! check has no prior authority to authorize the very first grant against (mirrors
//! `tests/authz_bootstrap.rs`'s bootstrap-grant pattern).

mod support;

use axum::Router;
use axum::http::StatusCode;
use chrono::Utc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{GrantScope, PrincipalId, RoleGrant};
use paigasus_kernel::Prn;
use serde_json::json;
use support::{app_with_state, send};
use uuid::Uuid;

/// Triggers JIT-provisioning of `token`'s own principal (ANY protected call does this
/// regardless of the route's business logic — `require_bearer` resolves the caller before
/// the handler ever runs) and then reads back its `principal_prn` via `POST
/// /v1/authn/introspect` (unauthenticated, but `Provisioning::Disabled` — D10 — so it only
/// succeeds once the principal already exists, which the prior call just ensured).
async fn self_principal_prn(app: &Router, token: &str) -> String {
    let (status, body) = send(app, "GET", "/v1/organizations", None, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "JIT-provisioning call failed: {body}");

    let (status, body) = send(app, "POST", "/v1/authn/introspect", Some(json!({"token": token})), None).await;
    assert_eq!(status, StatusCode::OK, "introspect failed: {body}");
    body["principal_prn"].as_str().expect("principal_prn").to_string()
}

/// Seeds a `platform_admin`-at-`Root` grant for `principal_prn` directly through
/// `state.role_grant_store` — bypassing `RoleService::grant`'s anti-escalation authorize
/// check (there is no prior authority to authorize the very first grant against). Sharing
/// this exact store (rather than a freshly constructed one) matters: it bumps the same
/// `Generations` counter `AppState`'s `CedarAuthorizer` reloads against, so the grant is
/// visible to the very next decision (AC1) with no extra wait.
async fn seed_platform_admin(state: &AppState, grant_id: Uuid, principal_prn: &str) {
    let principal = PrincipalId::from_prn(Prn::parse(principal_prn).expect("valid principal prn"));
    let grant = RoleGrant {
        id: grant_id,
        principal,
        role_key: "platform_admin".to_string(),
        scope: GrantScope::Root,
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    };
    state.role_grant_store.grant(&grant).await.expect("seed platform_admin grant");
}

#[tokio::test]
async fn is_authorized_self_query_returns_a_default_deny_decision_for_an_ungranted_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _state, idp) = app_with_state(db).await;
    let token = idp.bearer("alice", Some("alice@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &token).await;

    let (status, body) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": principal_prn,
            "action": "ListOrganizations",
            "resource_prn": root_prn().canonical(),
        })),
        Some(token.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["allowed"], false);
    assert_eq!(body["reason"], "denied");
    assert_eq!(body["determining_policies"], json!([DEFAULT_DENY_MARKER]));
}

#[tokio::test]
async fn is_authorized_non_self_query_by_an_unauthorized_actor_is_forbidden() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _state, idp) = app_with_state(db).await;
    let actor_token = idp.bearer("bob", Some("bob@example.com"), "paigasus", 3600);
    let other_token = idp.bearer("carol", Some("carol@example.com"), "paigasus", 3600);
    self_principal_prn(&app, &actor_token).await;
    let other_prn = self_principal_prn(&app, &other_token).await;

    let (status, body) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": other_prn,
            "action": "ListOrganizations",
            "resource_prn": root_prn().canonical(),
        })),
        Some(actor_token.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
    // The 403 path never computes (let alone leaks) a decision for the probed principal.
    assert!(body.get("allowed").is_none());
    assert!(body.get("determining_policies").is_none());
}

#[tokio::test]
async fn put_policy_is_forbidden_for_a_non_admin_then_succeeds_once_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("dave", Some("dave@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &token).await;

    let policy_body = json!({
        "policy_id": "http-authz-test-policy",
        "kind": "static",
        "source": r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#,
        "description": "test policy",
    });

    let (status, body) = send(&app, "POST", "/v1/authz/policies", Some(policy_body.clone()), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    seed_platform_admin(&state, Uuid::from_u128(1_001), &principal_prn).await;

    let (status, body) = send(&app, "POST", "/v1/authz/policies", Some(policy_body), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["policy_id"], "http-authz-test-policy");
    assert_eq!(body["kind"], "static");
    assert_eq!(body["system"], false);

    let (status, listed) = send(&app, "GET", "/v1/authz/policies", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.as_array().unwrap().iter().any(|p| p["policy_id"] == "http-authz-test-policy"));

    let (status, _) = send(&app, "DELETE", "/v1/authz/policies/http-authz-test-policy", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn role_grant_lifecycle_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    let admin_token = idp.bearer("erin", Some("erin@example.com"), "paigasus", 3600);
    let admin_prn = self_principal_prn(&app, &admin_token).await;
    seed_platform_admin(&state, Uuid::from_u128(1_002), &admin_prn).await;

    let member_token = idp.bearer("frank", Some("frank@example.com"), "paigasus", 3600);
    let member_prn = self_principal_prn(&app, &member_token).await;

    // Grant: platform_admin at Root can grant anywhere, including Root itself.
    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({
            "principal_prn": member_prn,
            "role_key": "platform_admin",
            "scope_prn": root_prn().canonical(),
        })),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    assert_eq!(granted["principal_prn"], member_prn);
    assert_eq!(granted["role_key"], "platform_admin");
    assert_eq!(granted["scope_prn"], root_prn().canonical());
    let grant_id = granted["id"].as_str().expect("id").to_string();

    // List: the member's own grant is visible to the admin querying it.
    let (status, listed) = send(&app, "GET", &format!("/v1/authz/role-grants?principal_prn={member_prn}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], grant_id);

    // Revoke.
    let (status, body) = send(&app, "DELETE", &format!("/v1/authz/role-grants/{grant_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, listed_after) = send(&app, "GET", &format!("/v1/authz/role-grants?principal_prn={member_prn}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed_after}");
    assert!(listed_after.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn is_authorized_rejects_an_unknown_action_and_a_malformed_prn() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _state, idp) = app_with_state(db).await;
    let token = idp.bearer("grace", Some("grace@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &token).await;

    let (status, err) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": principal_prn,
            "action": "NotARealAction",
            "resource_prn": root_prn().canonical(),
        })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-action");

    let (status, err) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": "not-a-prn",
            "action": "ListOrganizations",
            "resource_prn": root_prn().canonical(),
        })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-prn");
}
