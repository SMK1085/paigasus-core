// SPDX-License-Identifier: Apache-2.0

//! End-to-end proof of the persistent denial-audit path (SMA-446 Slice A, Task A12): an
//! unauthorized HTTP call produces an authorization denial; the `FanOutAuditSink` wired into
//! `CedarAuthorizer` (`AppState::new`) pushes it into the `DenialAuditBuffer`; the spawned
//! `DenialAuditDrain` persists it to `PgAuditLog` out of band; and `GET /v1/audit` (as a
//! platform admin) then returns that denial row. Unlike `tests/http_audit.rs` (which seeds a
//! row directly through `PgAuditLog`), this exercises the WHOLE buffer -> drain -> table ->
//! query wiring.
//!
//! The oneshot harness has no server lifecycle, so this test spawns the drain itself
//! (mirroring `main.rs`'s `servers.spawn(drain.run(..))`). Because the drain is async, the
//! assertion POLLS `GET /v1/audit` in a bounded retry loop until the denial row appears,
//! rather than asserting once immediately (a single immediate assert would be flaky). Drives
//! the real `router(AppState::new(db, &cfg))` against an ephemeral Postgres (Docker; see
//! `tests/support/mod.rs`).

mod support;

use axum::http::StatusCode;
use paigasus_iam_core::authz::model::root_prn;
use serde_json::json;
use std::time::Duration;
use support::{app_with_state, send};
use tokio::sync::watch;

#[tokio::test]
async fn http_denial_is_buffered_drained_to_postgres_and_queryable() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    // Spawn the denial-audit drain against the SAME `PgAuditLog` sink the read side queries,
    // mirroring `main.rs` — the oneshot harness runs no server, so the test owns the drain's
    // lifecycle. `take_denial_drain` hands it out exactly once.
    let (drain_shutdown_tx, drain_shutdown_rx) = watch::channel(());
    let drain = state.take_denial_drain().expect("the drain must be available exactly once");
    let sink = state.audit_sink();
    let drain_task = tokio::spawn(async move {
        let mut rx = drain_shutdown_rx;
        drain
            .run(sink, async move {
                let _ = rx.changed().await;
            })
            .await;
    });

    // A non-admin principal makes an ENFORCED tenancy call it isn't authorized for
    // (`POST /v1/organizations` -> `CreateOrganization` @ Root, `enforce_tenancy` defaults on).
    // With no grant, the default-deny fires -> `403 Forbidden`, and `CedarAuthorizer` records
    // the denial through the fan-out sink into the buffer.
    let denier_token = idp.bearer("audit-e2e-denier", Some("audit-e2e-denier@example.com"), "paigasus", 3600);
    support::provision(&state, &denier_token).await;
    let (status, deny_body) = send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "audit-e2e-org", "name": "Audit E2E Org"})),
        Some(denier_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the non-admin org create must be denied: {deny_body}");

    // A SEPARATE platform admin to read the audit log (`ListAuditLog` is Root-only).
    let admin_token = idp.bearer("audit-e2e-admin", Some("audit-e2e-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    // The drain persists out of band, so poll (bounded) until the denial row lands rather than
    // asserting once immediately. ~4s budget (80 * 50ms) is generous headroom over the drain's
    // notify-driven wake, without hanging CI if the wiring is broken.
    let root_resource = root_prn().canonical();
    let mut found = None;
    for _ in 0..80 {
        let (status, page) = send(&app, "GET", "/v1/audit?outcome=denied", None, Some(admin_token.as_str())).await;
        assert_eq!(status, StatusCode::OK, "audit query must succeed for the platform admin: {page}");
        if let Some(entry) = page["entries"].as_array().and_then(|entries| entries.iter().find(|e| e["action"] == "CreateOrganization")).cloned() {
            found = Some(entry);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let entry = found.expect("the CreateOrganization denial must reach the audit log within the poll budget");
    assert_eq!(entry["outcome"], "denied", "{entry}");
    assert_eq!(entry["action"], "CreateOrganization", "{entry}");
    assert_eq!(entry["resource_prn"], root_resource, "the denial's resource must be Root: {entry}");
    let policies = entry["determining_policies"].as_array().expect("determining_policies must be an array");
    assert!(!policies.is_empty(), "a denial must carry at least one determining policy: {entry}");

    // Graceful drain shutdown: dropping the sender resolves the drain's shutdown future, which
    // runs one final drain pass and returns.
    drop(drain_shutdown_tx);
    drain_task.await.expect("the drain task must not panic");
}
