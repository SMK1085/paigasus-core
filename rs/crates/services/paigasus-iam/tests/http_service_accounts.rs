// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/service-accounts/*` + `/v1/authn/api-keys/introspect`
//! (SMA-445 Task 20): the create -> issue -> list lifecycle (proving the plaintext token is
//! shown exactly once and never leaks into a list response), revoke's `204` plus the
//! immediate authentication denial it causes, the unauthenticated introspection endpoint
//! (valid key -> `200` principal context; garbage -> the same token-introspect-style `401`),
//! and the create-requires-authorization `403` gate. Drives the real
//! `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no listening socket —
//! against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).
//!
//! `create_get_list_archive_lifecycle_over_http` also covers the CodeRabbit finding on the
//! SMA-445 PR: `ServiceAccountDto.status` is `"active"` right after create/get/list, then
//! `"disabled"` on both a subsequent GET and the list entry once the account is archived
//! (`DELETE`) — proving the HTTP surface reads the underlying `Principal`'s status live (D16),
//! not a stale value cached at create time.
//!
//! Every scenario needs its acting principal provisioned and, for the authorized scenarios, a
//! `platform_admin` grant — `support::provision_platform_admin` (SMA-444 Task 20), bypassing
//! `RoleService::grant`'s anti-escalation check exactly like `tests/http_authz.rs`. The owning
//! tenancy node is seeded directly via `support::seed_org_ref` (raw SQL, no `CreateOrganization`
//! ceremony needed) — `CreateServiceAccount`/`IssueApiKey`/etc. all authorize against a
//! resource of kind `[Root, Organization, Team, Project]` (spec §8), and `platform_admin`@`Root`
//! covers every action at every resource in that hierarchy regardless of how the org row itself
//! was created.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use support::{app_with_state, provision, provision_platform_admin, seed_org_ref, send, send_raw};

#[tokio::test]
async fn issue_then_list_hides_secret() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("alice", Some("alice@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let owner = seed_org_ref(&state.db).await;

    let (status, created) = send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({ "owner_prn": owner.canonical(), "name": "ci-bot" })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["owner_prn"], owner.canonical());
    assert_eq!(created["name"], "ci-bot");
    let sa_prn = created["prn"].as_str().expect("prn").to_string();
    let sa_id = sa_prn.rsplit('/').next().expect("prn has a trailing id segment").to_string();

    let (status, issued) = send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({ "scope_prn": owner.canonical() })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let plaintext = issued["token"].as_str().expect("token").to_string();
    assert!(plaintext.starts_with("pgs_sk_"), "{issued}");
    assert_eq!(issued["api_key"]["service_account_prn"], sa_prn);
    assert_eq!(issued["api_key"]["status"], "active");
    let key_id = issued["api_key"]["id"].as_str().expect("api_key.id").to_string();
    // The issue response's `api_key` projection carries no secret material either.
    assert!(issued["api_key"].get("token").is_none(), "{issued}");
    assert!(issued["api_key"].get("hash").is_none(), "{issued}");
    assert!(issued["api_key"].get("key_hash").is_none(), "{issued}");

    let (status, listed) = send(&app, "GET", &format!("/v1/service-accounts/{sa_id}/api-keys"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed.as_array().expect("list is a json array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], key_id);
    assert!(items[0]["prefix"].as_str().expect("prefix").starts_with("pgs_sk_"), "{listed}");
    // The shown-once token, and any secret/hash material, must never appear in a list entry.
    assert!(items[0].get("token").is_none(), "list must never carry the plaintext token: {listed}");
    assert!(items[0].get("hash").is_none(), "{listed}");
    assert!(items[0].get("key_hash").is_none(), "{listed}");
    assert!(items[0].get("secret").is_none(), "{listed}");
}

#[tokio::test]
async fn revoke_returns_204_and_denies() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("bob", Some("bob@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let owner = seed_org_ref(&state.db).await;

    let (status, created) = send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({ "owner_prn": owner.canonical(), "name": "ci-bot" })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let sa_id = created["prn"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    let (status, issued) = send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({ "scope_prn": owner.canonical() })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let plaintext = issued["token"].as_str().unwrap().to_string();
    let key_id = issued["api_key"]["id"].as_str().unwrap().to_string();

    let (status, body) = send(&app, "DELETE", &format!("/v1/service-accounts/{sa_id}/api-keys/{key_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // A revoked key must no longer authenticate at all -- present it as a bearer credential on
    // an ordinary protected route and it must be denied exactly like any other invalid token.
    let response = send_raw(&app, "GET", &format!("/v1/service-accounts/{sa_id}"), None, Some(plaintext.as_str())).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "a revoked key must not authenticate");

    // The introspection endpoint agrees: the same revoked token is rejected, not resolved.
    let (status, err) = send(&app, "POST", "/v1/authn/api-keys/introspect", Some(json!({ "token": plaintext })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");
    assert_eq!(err["error"]["code"], "invalid-token");
}

#[tokio::test]
async fn introspect_endpoint_validates_a_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("carol", Some("carol@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let owner = seed_org_ref(&state.db).await;

    let (status, created) = send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({ "owner_prn": owner.canonical(), "name": "ci-bot" })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let sa_prn = created["prn"].as_str().unwrap().to_string();
    let sa_id = sa_prn.rsplit('/').next().unwrap().to_string();

    let (status, issued) = send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({ "scope_prn": owner.canonical() })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let plaintext = issued["token"].as_str().unwrap().to_string();

    // A valid key introspects to the SA's own principal context -- unauthenticated (no bearer
    // header at all), mirroring the token-introspect route's calling convention.
    let (status, body) = send(&app, "POST", "/v1/authn/api-keys/introspect", Some(json!({ "token": plaintext })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal_prn"], sa_prn);
    assert_eq!(body["status"], "active");
    assert!(body["key_id"].as_str().is_some_and(|s| !s.is_empty()), "{body}");
    assert!(body["memberships"].as_array().expect("memberships array").is_empty());
    assert!(body["role_grants"].as_array().expect("role_grants array").is_empty());

    // Garbage input gets the same token-introspect-style rejection: 401 `invalid-token`, no
    // request-body echo.
    let (status, err) = send(&app, "POST", "/v1/authn/api-keys/introspect", Some(json!({ "token": "not-a-key-at-all" })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");
    assert_eq!(err["error"]["code"], "invalid-token");
    assert!(!err["error"]["message"].as_str().unwrap().contains("not-a-key-at-all"), "{err}");
}

#[tokio::test]
async fn create_requires_authorization() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("dave", Some("dave@example.com"), "paigasus", 3600);
    // Provisioned, but deliberately never granted any role -- `CreateServiceAccount` must deny.
    provision(&state, &token).await;
    let owner = seed_org_ref(&state.db).await;

    let (status, body) = send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({ "owner_prn": owner.canonical(), "name": "ci-bot" })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
}

#[tokio::test]
async fn create_get_list_archive_lifecycle_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("erin", Some("erin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let owner = seed_org_ref(&state.db).await;

    let (status, created) = send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({ "owner_prn": owner.canonical(), "name": "ci-bot" })),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["status"], "active", "a freshly created SA is active (D16)");
    let sa_id = created["prn"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    let (status, got) = send(&app, "GET", &format!("/v1/service-accounts/{sa_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["prn"], created["prn"]);
    assert_eq!(got["status"], "active", "{got}");

    let (status, listed) = send(&app, "GET", &format!("/v1/service-accounts?owner_prn={}", owner.canonical()), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["prn"], created["prn"]);
    assert_eq!(items[0]["status"], "active", "{listed}");

    let (status, body) = send(&app, "DELETE", &format!("/v1/service-accounts/{sa_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Archiving sets the underlying principal's status to `disabled` (D16) — a subsequent GET
    // (and the list entry alongside it) must reflect it immediately, not a stale `active`.
    let (status, after_get) = send(&app, "GET", &format!("/v1/service-accounts/{sa_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{after_get}");
    assert_eq!(after_get["status"], "disabled", "{after_get}");

    let (status, after_list) = send(&app, "GET", &format!("/v1/service-accounts?owner_prn={}", owner.canonical()), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{after_list}");
    let after_items = after_list.as_array().unwrap();
    assert_eq!(after_items.len(), 1);
    assert_eq!(after_items[0]["status"], "disabled", "{after_list}");
}

/// SMA-586 fix round 2 (Fix 4), end-to-end on the ONE real two-segment route: each segment of
/// `/v1/service-accounts/{sa}/api-keys/{id}` reports its OWN field.
///
/// Before the fix, `revoke` took a single-marker `UuidPathPair<ApiKeyId>`, so a malformed `{sa}`
/// answered `api_key_id must be a uuid` — a guess presented as fact, and one no synthetic
/// extractor test could see because the marker choice lives on the handler, not the extractor.
#[tokio::test]
async fn each_segment_of_the_api_key_route_names_its_own_field() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("path-sa-user", Some("path-sa@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // Any well-formed uuid: these requests never reach the handler, so it need not exist.
    let valid = "0192f1c0-0000-7000-8000-000000000001";
    let cases = [
        ("DELETE", format!("/v1/service-accounts/not-a-uuid/api-keys/{valid}"), "service_account_id"),
        ("DELETE", format!("/v1/service-accounts/{valid}/api-keys/not-a-uuid"), "api_key_id"),
        // The single-segment sibling routes on the same prefix, for the same marker.
        ("GET", "/v1/service-accounts/not-a-uuid/api-keys".to_string(), "service_account_id"),
        ("GET", "/v1/service-accounts/not-a-uuid".to_string(), "service_account_id"),
    ];
    for (method, uri, field) in cases {
        let (status, err) = send(&app, method, &uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-uuid", "{method} {uri}: {err}");
        assert_eq!(err["error"]["message"], format!("{field} must be a uuid"), "{method} {uri}: {err}");
    }
}
