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
use support::{app_with_state, provision, provision_platform_admin, send};

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
