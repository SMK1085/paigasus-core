// SPDX-License-Identifier: Apache-2.0

//! End-to-end regression test for the SMA-444 M3 cross-tenant privilege-escalation fix
//! (critical, found by the final whole-branch review).
//!
//! The vulnerability: `TenancyNodeRef::from_prn`/`TeamId::from_prn` only check that a team
//! PRN's org slot is PRESENT, never that it's CORRECT (`paigasus_iam_core::tenancy::check`),
//! so a caller can submit a resource/scope PRN naming a real team's uuid paired with an
//! arbitrary org uuid. `PgEntitySliceLoader`'s `Team` branch used to parent the team on that
//! caller-controlled org slot instead of the team's REAL stored parent (contrast the `Project`
//! branch, which always derived its parent from the loaded `project_view.node.team_id` — the
//! real stored parent, never the caller's PRN).
//!
//! Exploit: an `org_admin` on `ORG_A` grants `team_admin` to a confederate with
//! `scope_prn = prn:pgs:iam::<ORG_A>:team/<team_B>`, where `team_B` really belongs to `ORG_B`.
//! Pre-fix, the slice loader wrongly parented `team_B` under `ORG_A`, so `resource in ORG_A`
//! matched and the `GrantRole` authorize check wrongly ALLOWED — handing the confederate
//! `team_admin` over a team neither it nor the granting `org_admin` has any legitimate
//! authority over. Post-fix (`pg_entity_slice.rs`'s `Team` branch derives the parent org from
//! the loaded `team_view`, mirroring the `Project` branch), `team_B` is correctly parented on
//! `ORG_B`, so `org_admin`@`ORG_A` does NOT match `resource in ORG_A` and the grant attempt is
//! DENIED (403) — no `role_grant` row is ever written, and the confederate gains no authority
//! over `team_B`.
//!
//! Drives the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot`
//! (`support::app_with_state`/`send`, mirroring `tests/http_authz.rs`) against an ephemeral
//! Postgres (Docker). In CI (`CI` env set) a missing Docker daemon is a HARD FAILURE; on a
//! Docker-less laptop the test skips (returns) with a note — same gating pattern as
//! `tests/http_authz.rs`/`tests/authz_entity_slice.rs`.

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgOrganizationRepository, PgTeamRepository};
use paigasus_iam_core::{Clock, GrantScope, IdGenerator, Organization, OrganizationRepository, Slug, Team, TeamId, TeamRepository, TenancyNodeRef};
use sea_orm::DatabaseConnection;
use serde_json::json;
use support::{app_with_state, seed_org_admin, send};

/// Seeds a fresh organization (with its auto-provisioned default team, plus an `org_admin`
/// owner grant, D8) and one further, non-default team under it, via the real M1 repos —
/// mirroring `tests/authz_entity_slice.rs`'s `seed_chain`/`new_org_and_default_team` helpers.
async fn seed_org_with_team(db: &DatabaseConnection, org_slug: &str, team_slug: &str) -> (Organization, Team) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());

    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id.clone(), Slug::parse(org_slug).unwrap(), "Org", clock.now()).unwrap();
    let default_team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse("default").unwrap(), "Default", clock.now()).unwrap();
    let owner = ids.new_principal_id();
    let owner_grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &owner_grant).await.unwrap();

    let team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse(team_slug).unwrap(), "Team", clock.now()).unwrap();
    team_repo.create(&team).await.unwrap();

    (org, team)
}

#[tokio::test]
async fn forged_org_slot_in_a_team_scope_grant_is_denied_not_escalated() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;

    // Two independent orgs, each with its own (non-default) team; `team_b` is the attack's
    // real target — it belongs to `org_b`, never `org_a`.
    let (org_a, _team_a) = seed_org_with_team(&db, "org-a", "team-a").await;
    let (_org_b, team_b) = seed_org_with_team(&db, "org-b", "team-b").await;

    // P: `org_admin` on ORG_A ONLY — no authority anywhere in ORG_B.
    let actor_token = idp.bearer("mallory", Some("mallory@example.com"), "paigasus", 3600);
    let actor_prn = support::provision(&state, &actor_token).await;
    seed_org_admin(&state, &actor_prn, &org_a.id.canonical()).await;

    // The confederate — currently has no authority anywhere.
    let confederate_token = idp.bearer("confederate", Some("confederate@example.com"), "paigasus", 3600);
    let confederate_prn = support::provision(&state, &confederate_token).await;

    // The forged scope: team_b's REAL uuid, but ORG_A's uuid in the PRN's org slot.
    let forged_team_prn = TeamId::from_parts(org_a.id.uuid(), team_b.id.uuid()).canonical();
    assert_ne!(forged_team_prn, team_b.id.canonical(), "sanity: the forged prn must differ from team_b's real canonical prn");

    let (status, body) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({
            "principal_prn": confederate_prn,
            "role_key": "team_admin",
            "scope_prn": forged_team_prn,
        })),
        Some(actor_token.as_str()),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "org_admin@ORG_A must NOT be able to grant team_admin on team_b (really under ORG_B) via a forged org slot: {body}"
    );
    assert_eq!(body["error"]["code"], "forbidden");

    // Negative outcome #1: no role_grant row for team_b was created — the escalation attempt
    // must never persist, not merely be reported as denied.
    let all_grants = state.role_grant_store.list_all().await.unwrap();
    assert!(
        !all_grants
            .iter()
            .any(|g| matches!(&g.scope, GrantScope::Node(TenancyNodeRef::Team(id)) if id.uuid() == team_b.id.uuid())),
        "a denied grant attempt must never persist a role_grant row for team_b, got {all_grants:?}"
    );

    // Negative outcome #2: the confederate gained no authority over team_b — a self-query for
    // an action `team_admin` would have conferred (RenameTeam) still denies (mirrors
    // `tests/http_authz.rs`'s self-query pattern: no authorization needed to query oneself).
    let (status, decision) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({
            "principal_prn": confederate_prn,
            "action": "RenameTeam",
            "resource_prn": team_b.id.canonical(),
        })),
        Some(confederate_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert_eq!(
        decision["allowed"], false,
        "confederate must hold no authority over team_b after the denied escalation attempt: {decision}"
    );
}
