// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `GET /v1/audit` (SMA-446 Task A11): `ListAuditLog` is
//! Root-only (mirrors `tests/grpc_audit.rs`'s restriction, see
//! `application::audit::AuditQueryService`'s module doc) — a non-admin caller gets `403
//! Forbidden` with nothing leaked; a seeded `platform_admin` caller gets back rows seeded
//! directly into `PgAuditLog` (the simplest, self-contained way to have a denial row to
//! query — SMA-446 A10/A11 briefs), with `detail` arriving as a real JSON object rather than
//! a stringified blob; a `limit`/`cursor` round-trip paginates via the standard keyset
//! "under-full page has no next_cursor" convention. Drives the real `router(AppState::new(db,
//! &cfg))` via `tower::ServiceExt::oneshot` — no listening socket — against an ephemeral
//! Postgres (Docker; see `tests/support/mod.rs`).

mod support;

use axum::http::StatusCode;
use chrono::Utc;
use paigasus_iam::adapters::persistence::PgAuditLog;
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome};
use serde_json::json;
use support::{app_with_state, send};
use uuid::Uuid;

/// A denied `AuditEntry` for `actor`, seeded directly through `PgAuditLog` — bypassing
/// HTTP/gRPC entirely, the simplest self-contained way to have a queryable row (mirrors
/// `tests/grpc_audit.rs::denial`, duplicated here since each `tests/*.rs` binary compiles its
/// own copy of `mod support` and its fixtures).
fn denial(id: Uuid, actor: &str) -> AuditEntry {
    AuditEntry {
        id,
        occurred_at: Utc::now(),
        actor_prn: Some(actor.to_string()),
        action: "GetProject".to_string(),
        resource_prn: None,
        outcome: AuditOutcome::Denied,
        determining_policies: vec!["policy-forbid-1".to_string()],
        detail: serde_json::json!({"reason": "no matching allow"}),
        correlation_id: None,
    }
}

#[tokio::test]
async fn get_audit_is_forbidden_for_a_non_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("http-audit-nonadmin", Some("http-audit-nonadmin@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;

    let (status, body) = send(&app, "GET", "/v1/audit", None, Some(token.as_str())).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
    // The 403 path never leaks the audit log's contents.
    assert!(body.get("entries").is_none());
}

#[tokio::test]
async fn get_audit_returns_seeded_rows_for_a_platform_admin_with_detail_as_a_json_object() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;

    // Seed a denial row directly through `PgAuditLog`, independent of `AppState`'s own
    // wiring — the same pattern `tests/grpc_audit.rs`/`tests/audit_log_pg.rs` use.
    let sink = PgAuditLog::new(db);
    let entry_id = Uuid::from_u128(1);
    sink.record_out_of_band(&denial(entry_id, "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa"))
        .await
        .expect("seed denial row");

    let admin_token = idp.bearer("http-audit-admin", Some("http-audit-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "GET", "/v1/audit", None, Some(admin_token.as_str())).await;

    assert_eq!(status, StatusCode::OK, "admin_prn={admin_prn}: {body}");
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["id"], entry_id.to_string());
    assert_eq!(entry["action"], "GetProject");
    assert_eq!(entry["outcome"], "denied");
    assert_eq!(entry["determining_policies"], json!(["policy-forbid-1"]));
    // `detail` is a real JSON object on the wire, not a stringified blob (unlike the gRPC
    // surface's `detail_json` string field, SMA-446 A11 brief).
    assert!(entry["detail"].is_object(), "detail must be a JSON object, got: {}", entry["detail"]);
    assert_eq!(entry["detail"], json!({"reason": "no matching allow"}));
    assert!(entry["correlation_id"].is_null());
    assert!(entry["occurred_at"].is_string());
    // A single row under the default limit is not a full page, so there is no next cursor.
    assert!(body["next_cursor"].is_null());
}

#[tokio::test]
async fn get_audit_rejects_a_malformed_cursor() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin_token = idp.bearer("http-audit-badcursor", Some("http-audit-badcursor@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, body) = send(&app, "GET", "/v1/audit?cursor=not-a-uuid", None, Some(admin_token.as_str())).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid-prn");
}

#[tokio::test]
async fn get_audit_paginates_with_limit_and_cursor() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;

    // Seed three denial rows for the same actor. `PgAuditLog::query` orders `id DESC`
    // (`pg_audit_log.rs`'s doc), and `Uuid::from_u128` places its argument in big-endian byte
    // order, so `from_u128(3) > from_u128(2) > from_u128(1)` as UUIDs — the seeded ids sort
    // 3, 2, 1, deterministically, without depending on wall-clock ordering.
    let sink = PgAuditLog::new(db);
    for n in 1..=3u128 {
        sink.record_out_of_band(&denial(Uuid::from_u128(n), "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa"))
            .await
            .expect("seed denial row");
    }

    let admin_token = idp.bearer("http-audit-page-admin", Some("http-audit-page-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, page1) = send(&app, "GET", "/v1/audit?limit=2", None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    let entries1 = page1["entries"].as_array().expect("entries array");
    assert_eq!(entries1.len(), 2);
    assert_eq!(entries1[0]["id"], Uuid::from_u128(3).to_string());
    assert_eq!(entries1[1]["id"], Uuid::from_u128(2).to_string());
    let next_cursor = page1["next_cursor"].as_str().expect("a full page carries a next_cursor").to_string();
    assert_eq!(next_cursor, Uuid::from_u128(2).to_string());

    let (status, page2) = send(&app, "GET", &format!("/v1/audit?limit=2&cursor={next_cursor}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    let entries2 = page2["entries"].as_array().expect("entries array");
    assert_eq!(entries2.len(), 1);
    assert_eq!(entries2[0]["id"], Uuid::from_u128(1).to_string());
    assert!(page2["next_cursor"].is_null(), "an under-full page must not carry a next_cursor");
}
