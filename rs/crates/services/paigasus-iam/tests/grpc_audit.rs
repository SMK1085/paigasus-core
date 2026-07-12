// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `AuditService` (SMA-446 Task A10): `ListAuditEntries` is
//! Root-only (mirrors `PolicyService::list`'s restriction, see
//! `application::audit::AuditQueryService`'s module doc) — a non-admin caller gets
//! `PermissionDenied` with nothing leaked; a seeded `platform_admin` caller gets back rows
//! seeded directly into `PgAuditLog` (the simplest, self-contained way to have a denial row
//! to query — SMA-446 A10 brief). Mirrors `tests/grpc_authz.rs`'s harness: the real
//! `grpc::router(AppState::new(db, &cfg), ..)` over an ephemeral `TcpListener`, against an
//! ephemeral Postgres (Docker) + the HTTPS mock IdP.
//!
//! Every `AuditService` RPC is bearer-enforced (Task 12, D14) — each request carries an
//! `authorization: Bearer <token>` metadata entry via the [`authed`] wrapper.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::PgAuditLog;
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome};
use paigasus_proto::paigasus::iam::v1::ListAuditEntriesRequest;
use paigasus_proto::paigasus::iam::v1::audit_service_client::AuditServiceClient;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

/// Spawns the full `grpc::router` (health, tenancy, authn, authorization, service-account, and
/// audit, all wrapped by the bearer layer) on an ephemeral port; `abort()` the returned handle
/// when the test finishes.
async fn spawn_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn channel(addr: SocketAddr) -> Channel {
    tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// Builds a `tonic::Request` carrying an `authorization: Bearer <token>` metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

/// A default (unfiltered) `ListAuditEntriesRequest`: every scalar filter empty/zero (the
/// wire's "unfiltered" sentinel, mirrors the proto doc on `ListAuditEntriesRequest`).
fn default_request() -> ListAuditEntriesRequest {
    ListAuditEntriesRequest {
        actor_prn: String::new(),
        resource_prn: String::new(),
        action: String::new(),
        outcome: String::new(),
        from: None,
        to: None,
        cursor: String::new(),
        limit: 0,
    }
}

/// A denied `AuditEntry` for `actor`, seeded directly through `PgAuditLog` — bypassing HTTP/
/// gRPC entirely, the simplest self-contained way to have a queryable row (SMA-446 A10 brief).
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
async fn list_audit_entries_over_grpc_is_permission_denied_for_a_non_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-audit-nonadmin", Some("grpc-audit-nonadmin@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut audit = AuditServiceClient::new(ch);
    let err = audit.list_audit_entries(authed(default_request(), &token)).await.unwrap_err();

    // A trailers-only `PermissionDenied` — nothing about the audit log's contents ever
    // reached the wire (there is no response message on an error status at all).
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(err.message().starts_with("forbidden:"), "unexpected message: {}", err.message());

    server.abort();
}

#[tokio::test]
async fn list_audit_entries_over_grpc_returns_seeded_rows_for_a_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();

    // Seed a denial row directly through `PgAuditLog`, independent of `AppState`'s own
    // wiring — the same pattern `tests/audit_log_pg.rs` uses.
    let sink = PgAuditLog::new(db);
    let entry_id = Uuid::from_u128(1);
    sink.record_out_of_band(&denial(entry_id, "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa"))
        .await
        .expect("seed denial row");

    let admin_token = idp.bearer("grpc-audit-admin", Some("grpc-audit-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut audit = AuditServiceClient::new(ch);
    let resp = audit.list_audit_entries(authed(default_request(), &admin_token)).await.unwrap().into_inner();

    assert_eq!(resp.entries.len(), 1, "the seeded denial row must be returned; admin_prn={admin_prn}");
    let wire_entry = &resp.entries[0];
    assert_eq!(wire_entry.id, entry_id.to_string());
    assert_eq!(wire_entry.action, "GetProject");
    assert_eq!(wire_entry.outcome, "denied");
    assert_eq!(wire_entry.determining_policies, vec!["policy-forbid-1".to_string()]);
    assert_eq!(wire_entry.detail_json, serde_json::json!({"reason": "no matching allow"}).to_string());
    assert!(wire_entry.correlation_id.is_empty(), "no correlation id was seeded");
    assert!(wire_entry.occurred_at.is_some());
    // A single row under the default limit is not a full page, so there is no next cursor.
    assert!(resp.next_cursor.is_empty());

    server.abort();
}

#[tokio::test]
async fn list_audit_entries_over_grpc_rejects_a_malformed_cursor() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let admin_token = idp.bearer("grpc-audit-badcursor", Some("grpc-audit-badcursor@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut audit = AuditServiceClient::new(ch);
    let err = audit
        .list_audit_entries(authed(
            ListAuditEntriesRequest {
                cursor: "not-a-uuid".to_string(),
                ..default_request()
            },
            &admin_token,
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::InvalidArgument, "{err:?}");

    server.abort();
}
