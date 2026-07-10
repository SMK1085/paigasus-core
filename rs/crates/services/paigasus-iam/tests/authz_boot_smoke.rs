// SPDX-License-Identifier: Apache-2.0

//! Boot smoke test (SMA-444 Task 15b): proves `AppState::new` composes the ENTIRE authorizer
//! stack — the shared `Generations`, the `PgPolicyStore`/`PgRoleGrantStore`-backed
//! `PolicySnapshot`, `PgEntitySliceLoader`, `MemoryDecisionCache`, and `TracingAuditSink` —
//! into a working `CedarAuthorizer` against a REAL Postgres, and that the resulting
//! `AppState.authz` actually runs `is_authorized` end-to-end (snapshot load -> entity-slice
//! load -> Cedar evaluation -> audit) without erroring.
//!
//! `AppState::new` seeds the starter Cedar policy set + system role catalog at boot
//! (`bootstrap::reconcile_starter`, SMA-444 Task 17), but seeding the TEMPLATES grants no
//! permission by itself — a template only materializes into a live permission once a
//! `RoleGrant` links it, and this test's principal has none. So the request below still
//! falls through to Cedar's implicit-deny default: `Effect::Deny` is proof the whole chain
//! wires up and runs (including the boot-time seed), not a claim that authorization logic
//! itself is under test (that's `cedar_authorizer.rs`'s/`authz::roles`'s unit suites).
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/health.rs`.

mod support;

use paigasus_iam::adapters::http::AppState;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{AccessRequest, Action, Authorizer, Effect, RequestContext};
use paigasus_kernel::Prn;
use uuid::Uuid;

#[tokio::test]
async fn app_state_composes_a_working_authorizer_that_default_denies_when_unseeded() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    // `AppState::new` needs a valid `IamConfig` (its authn half requires at least a
    // reachable-shaped issuer set); the mock IdP gives it one cheaply — this test never
    // authenticates anything, it only needs `AppState::new` to succeed and hand back a
    // composed `authz`.
    let idp = support::start_mock_idp().await;
    let cfg = support::test_config(&idp);
    let state = AppState::new(db, &cfg).await.expect("AppState::new must compose the authorizer over a real, migrated Postgres");

    // The principal need not exist as a `principal` row (the entity-slice loader falls back
    // to an "active" status for an unprovisioned principal — see
    // `authz_entity_slice.rs::authz_slice_principal_without_a_row_falls_back_to_active_status`)
    // and `root_prn()` always resolves (the synthetic Root sentinel, never a real tenancy
    // row) — so this request exercises the full authorizer without needing to seed any
    // tenancy chain first.
    let principal = Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).expect("static test prn parts are valid");
    let req = AccessRequest {
        principal,
        action: Action::ListOrganizations,
        resource: root_prn(),
        context: RequestContext::empty(),
    };

    let decision = state.authz.is_authorized(&req).await.expect("is_authorized must run end-to-end against real Postgres without erroring");
    assert_eq!(
        decision.effect,
        Effect::Deny,
        "the boot-seeded starter templates grant nothing without a RoleGrant linking one — Cedar's implicit-deny default"
    );
}
