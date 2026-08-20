// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/outbox/dead-letters` (SMA-469): the three Cedar actions
//! are Root-only, so a non-admin bearer gets 403 with nothing about the dead-letter contents
//! in the response. Docker gating mirrors `tests/http_audit.rs`.
//!
//! Also carries two security-gap closures deferred from Task 14's review (not part of the
//! original six-case brief):
//!
//! - Gap A: `tests/http_authn.rs::every_protected_v1_route_requires_bearer` is a
//!   HAND-MAINTAINED route list, not a generic sweep — the four dead-letter routes have been
//!   added there directly (see that file's diff), so this file does not re-duplicate that fix.
//! - Gap B: [`dead_letter_routes_require_bearer_through_the_real_composed_router`] below,
//!   which drives the REAL `router()`/`app_routes()` production composition (via
//!   `support::app`) rather than `src/adapters/http/mod.rs`'s
//!   `protected_router_merge_has_no_path_conflicts` unit test, which rebuilds the merge chain
//!   by hand and would NOT notice `dead_letters::router()` being moved outside the
//!   bearer-enforcing `route_layer`.

mod support;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;
use support::{app_with_state, send};
use uuid::Uuid;

/// Seeds a parked `event_outbox` row directly — the dead-letter state — mirroring
/// `tests/dead_letters_pg.rs::seed_parked_with_details`; duplicated here since each
/// `tests/*.rs` binary compiles its own copy of `mod support` and its fixtures.
async fn seed_parked(db: &DatabaseConnection, id: u128, event_type: &str) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set(event_type.to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb".to_string())),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(Some(Uuid::from_u128(999_999))),
        published_at: Set(None),
        attempts: Set(5),
        parked: Set(true),
        parked_at: Set(Some(Utc::now() - Duration::days(1))),
        last_error: Set(Some("backend error: transport closed".to_string())),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

#[tokio::test]
async fn list_requires_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("http-dlq-nonadmin", Some("http-dlq-nonadmin@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;

    let (status, body) = send(&app, "GET", "/v1/outbox/dead-letters", None, Some(token.as_str())).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
    // The 403 path never leaks anything about the dead-letter contents.
    assert!(body.get("entries").is_none());
}

#[tokio::test]
async fn list_returns_parked_rows_for_a_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    seed_parked(&db, 1, "iam.principal.created").await;

    let admin_token = idp.bearer("http-dlq-list-admin", Some("http-dlq-list-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "GET", "/v1/outbox/dead-letters", None, Some(admin_token.as_str())).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["last_error"], "backend error: transport closed");
    assert_eq!(entry["attempts"], 5);
    // `payload` is the raw stored TEXT, emitted as a JSON STRING — never re-parsed into an
    // object (an invalid-JSON payload is one of the reasons a row parks in the first place).
    assert!(entry["payload"].is_string(), "payload must be a JSON string, got: {}", entry["payload"]);
}

#[tokio::test]
async fn replay_one_returns_the_row_and_a_second_call_is_404() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let id = seed_parked(&db, 1, "iam.principal.created").await;

    let admin_token = idp.bearer("http-dlq-replay-admin", Some("http-dlq-replay-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "POST", &format!("/v1/outbox/dead-letters/{id}/replay"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id.to_string());

    // The documented success-after-timeout signal: a client that timed out on the first call
    // and retries sees exactly this 404, not a spurious error — the row is simply no longer
    // parked.
    let (status, body) = send(&app, "POST", &format!("/v1/outbox/dead-letters/{id}/replay"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not-found");
}

#[tokio::test]
async fn discard_one_removes_the_row() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let id = seed_parked(&db, 1, "iam.principal.created").await;

    let admin_token = idp.bearer("http-dlq-discard-admin", Some("http-dlq-discard-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "POST", &format!("/v1/outbox/dead-letters/{id}/discard"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id.to_string());

    let (status, body) = send(&app, "GET", "/v1/outbox/dead-letters", None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"].as_array().expect("entries array").len(), 0, "the discarded row must be gone");
}

#[tokio::test]
async fn bulk_replay_without_max_rows_is_400_invalid_bulk_replay() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let id = seed_parked(&db, 1, "iam.principal.created").await;

    let admin_token = idp.bearer("http-dlq-bulk-invalid-admin", Some("http-dlq-bulk-invalid-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    // `{}` is valid JSON with `max_rows` absent — NOT a bodyless/malformed request, which
    // would 415/422 via axum's own `Json` rejection instead of our documented error contract.
    let (status, body) = send(&app, "POST", "/v1/outbox/dead-letters/replay", Some(json!({})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid-bulk-replay");

    // Validation happened before any store access — the seeded row must still be parked.
    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert!(row.parked, "an invalid bulk-replay request must never touch the store");
}

#[tokio::test]
async fn bulk_replay_with_max_rows_replays_and_reports_the_count() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    seed_parked(&db, 1, "iam.principal.created").await;
    seed_parked(&db, 2, "iam.principal.created").await;

    let admin_token = idp.bearer("http-dlq-bulk-admin", Some("http-dlq-bulk-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "POST", "/v1/outbox/dead-letters/replay", Some(json!({"max_rows": 10})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["replayed"], 2, "must match the seeded parked count");
}

/// Gap B (SMA-469 Task 15, carried over from Task 14's review): `src/adapters/http/mod.rs`'s
/// `protected_router_merge_has_no_path_conflicts` test re-duplicates `app_routes`'s `protected`
/// merge chain BY HAND rather than calling `app_routes` itself — so if someone moved
/// `.merge(dead_letters::router())` OUTSIDE the bearer-enforcing
/// `.route_layer(...require_bearer)` call, that unit test would keep passing (it never
/// re-checks the layer placement) while the destructive `POST .../discard` endpoint became
/// reachable with NO bearer at all.
///
/// This test closes that gap by driving the REAL, production composition: `support::app`
/// calls `app_with_state`, which calls `router(state.clone())` (`src/adapters/http/mod.rs`'s
/// `pub fn router`) — the exact function `serve_http` wires onto a real listener, itself
/// `health_router().merge(readyz_router(state.clone())).merge(app_routes(state))`. There is no
/// hand-rebuilt chain anywhere in this test's call path: if the merge order in `app_routes`
/// ever changes so `dead_letters::router()` sits outside `route_layer`, these requests would
/// reach the handlers directly (missing the `AuthContext` extension the auth middleware
/// installs) instead of getting intercepted at 401, and this test would fail.
///
/// `POST /v1/outbox/dead-letters/{id}/discard` is the single highest-consequence assertion in
/// this file: reaching that handler without a bearer would permanently destroy event data.
#[tokio::test]
async fn dead_letter_routes_require_bearer_through_the_real_composed_router() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, _idp) = support::app(db).await;
    let id = Uuid::from_u128(1);

    for (method, path) in [
        ("GET", "/v1/outbox/dead-letters".to_string()),
        ("POST", "/v1/outbox/dead-letters/replay".to_string()),
        ("POST", format!("/v1/outbox/dead-letters/{id}/replay")),
        ("POST", format!("/v1/outbox/dead-letters/{id}/discard")),
    ] {
        let (status, body) = send(&app, method, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "route {method} {path} must be 401 without a bearer token: {body}");
        assert_eq!(body["error"]["code"], "invalid-token", "route {method} {path}: unexpected body {body}");
    }
}
