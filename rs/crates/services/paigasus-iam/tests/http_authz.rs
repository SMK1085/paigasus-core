// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/authz/*` (SMA-444 Task 18): `is-authorized`'s
//! self-query default-deny and its self/admin exposure rule's 403 on an unauthorized
//! non-self query; `POST /v1/authz/policies` forbidden for a non-admin, then succeeding once
//! the actor holds `platform_admin`; and the role-grant grant -> list -> revoke lifecycle.
//! Drives the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no
//! listening socket — against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).
//!
//! Every scenario needs the acting principal provisioned and, for the admin-only scenarios,
//! a `platform_admin` grant — `support::provision`/`support::seed_platform_admin` (SMA-444
//! Task 20), bypassing `RoleService::grant`'s anti-escalation check (there is no prior
//! authority to authorize the very first grant against). `self_principal_prn` below resolves
//! the caller directly through `state.authn` rather than by driving a tenancy route (the
//! pre-Task-20 `GET /v1/organizations` trigger is itself enforced now, so its OWN status
//! would depend on a grant that doesn't exist yet at that point).

mod support;

use axum::Router;
use axum::http::StatusCode;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam_core::OrganizationId;
use paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER;
use paigasus_iam_core::authz::model::root_prn;
use serde_json::json;
use support::{app_with_state, seed_platform_admin, send};
use uuid::Uuid;

/// Resolves `token`'s principal directly through `state.authn` (SMA-444 Task 20's
/// `support::provision`, inlined here since this file already threads `app`/`state`
/// separately in most scenarios) and reads it back via `POST /v1/authn/introspect`
/// (unauthenticated, but `Provisioning::Disabled` — D10 — so it only succeeds once the
/// principal already exists, which the direct `resolve` call just ensured).
async fn self_principal_prn(app: &Router, state: &AppState, token: &str) -> String {
    support::provision(state, token).await;
    let (status, body) = send(app, "POST", "/v1/authn/introspect", Some(json!({"token": token})), None).await;
    assert_eq!(status, StatusCode::OK, "introspect failed: {body}");
    body["principal_prn"].as_str().expect("principal_prn").to_string()
}

#[tokio::test]
async fn is_authorized_self_query_returns_a_default_deny_decision_for_an_ungranted_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("alice", Some("alice@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &state, &token).await;

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
    let (app, state, idp) = app_with_state(db).await;
    let actor_token = idp.bearer("bob", Some("bob@example.com"), "paigasus", 3600);
    let other_token = idp.bearer("carol", Some("carol@example.com"), "paigasus", 3600);
    self_principal_prn(&app, &state, &actor_token).await;
    let other_prn = self_principal_prn(&app, &state, &other_token).await;

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
    let principal_prn = self_principal_prn(&app, &state, &token).await;

    let policy_body = json!({
        "policy_id": "http-authz-test-policy",
        "kind": "static",
        "source": r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#,
        "description": "test policy",
    });

    let (status, body) = send(&app, "POST", "/v1/authz/policies", Some(policy_body.clone()), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    seed_platform_admin(&state, &principal_prn).await;

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
    let admin_prn = self_principal_prn(&app, &state, &admin_token).await;
    seed_platform_admin(&state, &admin_prn).await;

    let member_token = idp.bearer("frank", Some("frank@example.com"), "paigasus", 3600);
    let member_prn = self_principal_prn(&app, &state, &member_token).await;

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
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("grace", Some("grace@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &state, &token).await;

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

/// SMA-444 review fix: unlike the enforced tenancy routes (which fetch-first and 404 on a
/// nonexistent resource before authz is ever consulted), the direct
/// `POST /v1/authz/is-authorized` API is reachable with an arbitrary, well-formed but
/// nonexistent `resource_prn`. A missing tenancy node must fail CLOSED as a `Deny`, never a
/// 500 — and never distinguishable from an ordinary access denial (no existence oracle).
#[tokio::test]
async fn is_authorized_self_query_against_a_nonexistent_resource_denies_not_500() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("henry", Some("henry@example.com"), "paigasus", 3600);
    let principal_prn = self_principal_prn(&app, &state, &token).await;

    // A well-formed organization PRN that was never created.
    let bogus_org = OrganizationId::from_uuid(Uuid::from_u128(0xDEAD_BEEF));

    let (status, body) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": principal_prn,
            "action": "GetOrganization",
            "resource_prn": bogus_org.prn().canonical(),
        })),
        Some(token.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "a missing resource must deny, never 500: {body}");
    assert_eq!(body["allowed"], false);
    assert_eq!(body["reason"], "denied");
    assert_eq!(body["determining_policies"], json!(["resource-not-found"]));
}
