// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `POST /v1/users`' AUTHORIZATION (SMA-584). The sibling of
//! `tests/grpc_users.rs`'s `create_user_requires_platform_admin`: this file exists because
//! `http_memberships.rs` authenticates as `platform_admin` throughout, so it cannot express
//! the denied case and would stay green if authorization were added to gRPC only.
//!
//! Drives the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no
//! listening socket — against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::persistence::entities::principal;
use paigasus_kernel::Prn;
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use support::{app_with_config, app_with_state, provision, provision_platform_admin, send, test_config};

/// The three-outcome pin for `POST /v1/users` (SMA-584 AC-1/AC-2). A mutation that removes the
/// `Action::CreateUser` guard from `adapters::http::users` fails the middle row; a mutation
/// that puts `/v1/users` on an unauthenticated router fails the first.
#[tokio::test]
async fn create_user_requires_platform_admin_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    // Cloned BEFORE `app_with_state` consumes `db`, so the row-count assertion below can query
    // the same database independently (mirrors `tests/grpc_users.rs`'s identical setup).
    let count_db = db.clone();
    let (app, state, idp) = app_with_state(db).await;

    // An ORDINARY principal: JIT-provisioned, no grant of any kind.
    let plain_token = idp.bearer("http-plain-tester", Some("http-plain@example.com"), "paigasus", 3600);
    provision(&state, &plain_token).await;

    // A platform_admin, seeded at Root.
    let admin_token = idp.bearer("http-admin-tester", Some("http-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    // 1. No bearer at all -> 401. `/v1/users` sits on the bearer-gated `protected` sub-router.
    let (status, body) = send(&app, "POST", "/v1/users", Some(json!({"email": "no-bearer@example.com", "display_name": "No Bearer"})), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // Baseline taken AFTER both principals are provisioned, so the only thing that could move
    // it is the denied create below.
    let before = principal::Entity::find().count(&count_db).await.unwrap();

    // 2. An ordinary, non-admin principal -> 403 `forbidden`.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "denied@example.com", "display_name": "Denied"})),
        Some(plain_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden", "{body}");

    // The check must run BEFORE the use case: a denied create must not mint a principal row.
    let after = principal::Entity::find().count(&count_db).await.unwrap();
    assert_eq!(after, before, "a denied create must not mint a principal row");

    // 3. platform_admin -> 201, and the returned PRN parses.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "allowed@example.com", "display_name": "Allowed"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let prn = body["principal_prn"].as_str().expect("principal_prn");
    Prn::parse(prn).unwrap_or_else(|e| panic!("unexpected principal prn {prn}: {e}"));
}

/// **The action-identity pin (SMA-584).** Every other test in this file and in
/// `tests/grpc_users.rs` passes identically if the adapter checks `CreateOrganization` — or any
/// other Root-only action — instead of `CreateUser`, because `platform_admin`'s template has no
/// `action in [...]` clause and no other system role carries `CreateUser`. So no ROLE grant can
/// tell the two apart.
///
/// A narrow STATIC policy can. This seeds one permitting exactly `CreateUser` (the operator
/// remediation the design doc recommends, §4.3 lever 1) and asserts the principal it covers can
/// create a user but NOT an organization. A mutation that wires any other action into
/// `adapters::http::users` fails here.
#[tokio::test]
async fn the_http_guard_is_bound_to_create_user_specifically() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    // Mirrors `tests/authz_acceptance.rs`'s AC3 setup: a short cache TTL so the decision made
    // immediately after PutPolicy reflects the new policy.
    cfg.authz.policy_cache_ttl_secs = 1;
    let (app, state) = app_with_config(db, &cfg).await;

    // The admin who authors the policy (`Action::PutPolicy` is Root-only).
    let admin_token = idp.bearer("http-bind-admin", Some("http-bind-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    // The subject: an ordinary principal with no role grant at all.
    let subject_token = idp.bearer("http-bind-subject", Some("http-bind-subject@example.com"), "paigasus", 3600);
    provision(&state, &subject_token).await;

    // Before the policy: the subject cannot create a user.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "before@example.com", "display_name": "Before"})),
        Some(subject_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Seed a static policy permitting EXACTLY CreateUser, and nothing else.
    let policy_body = json!({
        "policy_id": "sma-584-create-user-only",
        "kind": "static",
        "source": r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#,
        "description": "SMA-584 action-identity pin: CreateUser only",
    });
    let (status, put) = send(&app, "POST", "/v1/authz/policies", Some(policy_body), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{put}");

    // The subject can now create a user...
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "bound@example.com", "display_name": "Bound"})),
        Some(subject_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "the guard must check CreateUser, not some other Root action: {body}");

    // ...but CONTROL: it still cannot create an organization. This is what proves the seeded
    // policy is genuinely narrow and the subject is not a platform admin by another name.
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "bound-org", "name": "Bound Org"})), Some(subject_token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the CreateUser-only policy must not permit CreateOrganization: {body}");
}
