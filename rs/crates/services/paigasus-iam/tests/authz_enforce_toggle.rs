// SPDX-License-Identifier: Apache-2.0

//! SMA-444 Task 21: `authz.enforce_tenancy` is now config-driven (`AppState.enforce_tenancy`,
//! replacing the old hardcoded `ENFORCE_TENANCY` const). This proves the `false` setting
//! actually short-circuits every `if state.enforce_tenancy { .. authorize.check(..) .. }`
//! guard in `organizations.rs`/`teams.rs`/`projects.rs`/`memberships.rs` (and their gRPC
//! mirrors), not just that the field exists on `AppState`: an otherwise-completely-ungranted
//! principal — who every other `is_authorized_*`/`*_is_forbidden` test in this suite proves
//! gets a 403 under the (default) `enforce_tenancy = true` — can still create an organization
//! when the toggle is off.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips — same gating pattern as
//! `tests/authz_boot_smoke.rs`/`tests/roundtrip.rs`.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use support::{app_with_config, provision, send, test_config};

#[tokio::test]
async fn enforce_tenancy_false_lets_an_otherwise_ungranted_principal_create_an_organization() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("no-grants-user", Some("no-grants@example.com"), "paigasus", 3600);
    // JIT-provision the principal but deliberately grant it NOTHING (no `seed_platform_admin`/
    // `seed_org_admin`) — with the default `enforce_tenancy = true` this principal's
    // `CreateOrganization` call would be denied (403 forbidden), mirroring every
    // `is_authorized_*_returns_a_default_deny_*`/`*_is_forbidden` test elsewhere in this
    // suite.
    provision(&state, &token).await;

    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "no-grant-org", "name": "No Grant Org"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "enforce_tenancy = false must bypass the authorization gate entirely: {body}");
}
