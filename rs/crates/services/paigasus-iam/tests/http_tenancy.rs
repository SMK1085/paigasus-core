// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for the `/v1` tenancy API (organizations/teams/projects):
//! full org lifecycle, nested team/project creation (`org_prn`/`team_prn` + effective-status
//! folding across the hierarchy), and pagination. Drives the real `router(AppState::new(db, &cfg))`
//! via `tower::ServiceExt::oneshot` — no listening socket — against an ephemeral Postgres
//! (Docker; see `tests/support/mod.rs`).

mod support;

use axum::http::StatusCode;
use paigasus_iam_core::PrincipalId;
use paigasus_kernel::Prn;
use serde_json::json;
use support::{app_with_state, provision_platform_admin, send};
use uuid::Uuid;

#[tokio::test]
async fn org_lifecycle_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-444 Task 20: every tenancy route below is now enforced — seed the acting
    // principal a `platform_admin` grant so this pre-authorization test's assertions keep
    // testing what they always tested (the tenancy lifecycle), not authorization itself.
    provision_platform_admin(&state, &token).await;

    // Create: 201, body has `organization` + `default_team` with parseable PRNs.
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Acme Corp."})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn");
    assert!(org_prn.starts_with("prn:pgs:iam:::organization/"), "unexpected org prn: {org_prn}");
    let team_prn = created["default_team"]["prn"].as_str().expect("default_team.prn");
    assert!(team_prn.contains(":team/"), "unexpected team prn: {team_prn}");
    assert_eq!(created["default_team"]["org_prn"], org_prn);
    let org_id = org_prn.rsplit('/').next().unwrap();

    // Get: 200.
    let (status, got) = send(&app, "GET", &format!("/v1/organizations/{org_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["slug"], "acme");
    assert_eq!(got["prn"], org_prn);

    // Duplicate slug -> 409 `slug-conflict`.
    let (status, err) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Dup"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "slug-conflict");

    // PATCH rename -> 200.
    let (status, renamed) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({"name": "Acme Renamed"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Acme Renamed");
    assert_eq!(renamed["slug"], "acme");

    // PATCH with an empty body -> 400 `nothing-to-rename`.
    let (status, err) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "nothing-to-rename");

    // Archive/restore -> 200, own + effective status flip.
    let (status, archived) = send(&app, "POST", &format!("/v1/organizations/{org_id}/archive"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["status"], "archived");
    assert_eq!(archived["effective_status"], "archived");

    // Restoring an ARCHIVED resource succeeds: `RestoreOrganization` is deliberately exempt
    // from the `forbid-archived-writes` starter policy's action list (`Action::is_write()`
    // filtered further by `!Action::is_restore()`) — restoring is the one legitimate write on
    // an archived node (its whole purpose), so the forbid must not fire on it. SMA-444 Task 20
    // wires this policy onto the real route for the first time — 200, own + effective status
    // flip back to active, not a 403 `forbidden`.
    let (status, restored) = send(&app, "POST", &format!("/v1/organizations/{org_id}/restore"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["status"], "active");
    assert_eq!(restored["effective_status"], "active");

    // Unknown id -> 404 `not-found`.
    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{}", Uuid::nil()), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not-found");
}

#[tokio::test]
async fn nested_team_and_project_creation_folds_effective_status() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "beta", "name": "Beta"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();

    // A (non-default) team nested under the org.
    let (status, team_body) = send(
        &app,
        "POST",
        &format!("/v1/organizations/{org_id}/teams"),
        Some(json!({"slug": "eng", "name": "Engineering"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(team_body["org_prn"], org_prn);
    let team_prn = team_body["prn"].as_str().unwrap().to_string();
    let team_id = team_prn.rsplit('/').next().unwrap().to_string();

    // A project nested under the team.
    let (status, project_body) = send(
        &app,
        "POST",
        &format!("/v1/teams/{team_id}/projects"),
        Some(json!({"slug": "web", "name": "Web"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(project_body["team_prn"], team_prn);
    assert_eq!(project_body["org_prn"], org_prn);
    assert_eq!(project_body["effective_status"], "active");
    let project_id = project_body["prn"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    // List teams (default + eng) / projects (web) under their parents.
    let (status, teams) = send(&app, "GET", &format!("/v1/organizations/{org_id}/teams"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(teams.as_array().unwrap().len(), 2);

    let (status, projects) = send(&app, "GET", &format!("/v1/teams/{team_id}/projects"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects.as_array().unwrap().len(), 1);

    // Archiving the org flips the team's and project's *effective* status without touching
    // their own `status` flag (D1/D10).
    let (status, _) = send(&app, "POST", &format!("/v1/organizations/{org_id}/archive"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);

    let (status, team_after) = send(&app, "GET", &format!("/v1/teams/{team_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(team_after["status"], "active");
    assert_eq!(team_after["effective_status"], "archived");

    let (status, project_after) = send(&app, "GET", &format!("/v1/projects/{project_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project_after["status"], "active");
    assert_eq!(project_after["effective_status"], "archived");

    // Renaming a project that is only *effectively* archived (via the still-archived org)
    // is rejected too — the guard folds ancestors, not just the project's own status.
    // SMA-444 Task 20: the `forbid-archived-writes` starter policy now enforces this on the
    // real route (spec §3.2, belt-and-braces over M1's own `node-archived` guard) and,
    // being a Cedar `forbid`, takes precedence — 403 `forbidden`, not the pre-enforcement
    // 409 `node-archived` (`org_lifecycle_over_http` covers the same policy on `Restore*`).
    let (status, err) = send(&app, "PATCH", &format!("/v1/projects/{project_id}"), Some(json!({"slug": "web2"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{err}");
    assert_eq!(err["error"]["code"], "forbidden");

    // Restoring the archived org succeeds (same exemption as `org_lifecycle_over_http`'s own
    // scenario: `RestoreOrganization` is deliberately carved out of `forbid-archived-writes`)
    // — 200, own + effective status flip back to active.
    let (status, restored) = send(&app, "POST", &format!("/v1/organizations/{org_id}/restore"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["status"], "active");
    assert_eq!(restored["effective_status"], "active");
}

#[tokio::test]
async fn list_pagination_and_invalid_limit() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    for slug in ["alpha", "bravo", "charlie"] {
        let (status, _) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": slug, "name": slug})), Some(token.as_str())).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, page) = send(&app, "GET", "/v1/organizations?limit=2&offset=0", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.as_array().unwrap().len(), 2);

    let (status, rest) = send(&app, "GET", "/v1/organizations?limit=2&offset=2", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rest.as_array().unwrap().len(), 1);

    // `limit=0` -> 400 `invalid-pagination`.
    let (status, err) = send(&app, "GET", "/v1/organizations?limit=0", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "invalid-pagination");
}

#[tokio::test]
async fn create_team_and_list_teams_404_on_a_nonexistent_org() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // `CreateTeam`/`ListTeams` authorize against the PARENT org's PRN. Before the fix, that
    // PRN was built straight from the path's `org_id` uuid without first confirming the org
    // exists, so a nonexistent org made the authorize call's entity-slice loader error out ->
    // 500 (CreateTeam) / a bare empty 200 pre-enforcement (ListTeams). The handlers now fetch
    // the org first (mirroring `CreateProject`/`ListProjects`'s team fetch), so a nonexistent
    // org 404s BEFORE authorization ever runs — for even a seeded platform_admin, who is
    // authorized for every action everywhere and so isn't itself the reason for the 404.
    let unknown_org_id = Uuid::from_u128(0xdead_beef);

    let (status, err) = send(
        &app,
        "POST",
        &format!("/v1/organizations/{unknown_org_id}/teams"),
        Some(json!({"slug": "eng", "name": "Engineering"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not-found");

    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{unknown_org_id}/teams"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not-found");
}

/// SMA-444 Task 20b (spec D8): `CreateOrganization` atomically seeds an `org_admin` owner
/// grant for the CREATING principal, scoped to the new org — the creator becomes the owner
/// of what it creates. `provision_platform_admin` seeds P as `platform_admin` first purely
/// so the `CreateOrganization`@`Root` authorize check passes (enforcement is Task 20's own
/// concern, not this test's); the owner grant asserted below is a SEPARATE, org-scoped
/// `org_admin` grant the create call itself seeds, not the platform_admin one. Atomicity
/// (org + default team + owner grant all present after the one create call, all in the SAME
/// transaction, ADR-0014) and the `policy_gen` bump are proven at the repository layer by
/// `tests/tenancy_orgs.rs`/`tests/authz_entity_gen_bumps.rs` (same `Generations`-handle idiom
/// every other generation-bump test in this suite uses) — this test proves the actor
/// threads correctly end to end through the real, wired HTTP path.
#[tokio::test]
async fn create_organization_seeds_an_org_admin_owner_grant_for_the_creating_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("org-creator", Some("creator@example.com"), "paigasus", 3600);
    let principal_prn = provision_platform_admin(&state, &token).await;
    let principal = PrincipalId::from_prn(Prn::parse(&principal_prn).expect("valid principal prn"));

    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "owned-co", "name": "Owned Co."})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let team_prn = created["default_team"]["prn"].as_str().expect("default_team.prn");
    assert!(team_prn.contains(":team/"), "unexpected team prn: {team_prn}");

    // The owner grant: `org_admin`, scoped to exactly the new org, held by the creator.
    let grants = state.role_grant_store.list_by_principal(&principal).await.expect("list_by_principal");
    let owner_grant = grants
        .iter()
        .find(|g| g.role_key == "org_admin" && g.scope.canonical_prn() == org_prn)
        .unwrap_or_else(|| panic!("expected an org_admin owner grant scoped to {org_prn}, got {grants:?}"));
    assert_eq!(owner_grant.principal, principal);
    assert_eq!(owner_grant.linked_policy_id, format!("grant:{}", owner_grant.id));
}

/// SMA-586 D5.1, end-to-end on REAL routes: a malformed uuid path segment answers inside the
/// `{"error":{code,message}}` envelope, naming the field the segment actually stands for.
///
/// The extractor's own unit tests build a synthetic `Router::new().route("/x/{id}", …)`, which
/// proves the extractor but NOT the handler-to-marker wiring — a handler carrying the wrong
/// marker is invisible to a synthetic route. That is exactly how the `{sa}`-segment mis-naming
/// fixed in round 2 survived the whole suite. These cases drive the real merged `router(...)`,
/// so each assertion pins one live route's marker choice.
#[tokio::test]
async fn a_malformed_uuid_path_segment_answers_in_the_error_envelope() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("path-user", Some("path@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // (route, expected field) — one per distinct `PathField` marker reachable on this router.
    let cases = [
        ("GET", "/v1/organizations/not-a-uuid", "organization_id"),
        ("GET", "/v1/organizations/not-a-uuid/teams", "organization_id"),
        ("GET", "/v1/teams/not-a-uuid", "team_id"),
        ("GET", "/v1/projects/not-a-uuid", "project_id"),
        ("DELETE", "/v1/memberships/not-a-uuid", "membership_id"),
    ];
    for (method, uri, field) in cases {
        let (status, err) = send(&app, method, uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-uuid", "{method} {uri}: {err}");
        assert_eq!(err["error"]["message"], format!("{field} must be a uuid"), "{method} {uri}: {err}");
    }

    // A well-formed uuid on the same route still reaches the handler (a 404, not a 400) — so
    // the assertions above are about the SEGMENT's shape, not about the route being broken.
    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{}", Uuid::nil()), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
}

/// SMA-587, end-to-end on REAL routes: a refused request body answers inside the
/// `{"error":{code,message}}` envelope with a kind-specific reason.
///
/// Driven against the merged `router(...)` rather than a synthetic one for the reason SMA-586
/// learned expensively: a synthetic route proves the EXTRACTOR, never the handler wiring, and
/// that is exactly how a mis-named `{sa}` path segment survived its whole suite. Each row here
/// pins one live route's extractor choice.
#[tokio::test]
async fn a_refused_body_answers_in_the_error_envelope() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("body-user", Some("body@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // Seed a real org/team/project so the rename routes reach their extractor rather than 404.
    let (_, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "envelope", "name": "Envelope"})), Some(token.as_str())).await;
    let org_id = created["organization"]["prn"].as_str().expect("organization.prn").rsplit('/').next().unwrap().to_string();
    let team_id = created["default_team"]["prn"].as_str().expect("default_team.prn").rsplit('/').next().unwrap().to_string();
    let (_, project) = send(&app, "POST", &format!("/v1/teams/{team_id}/projects"), Some(json!({"slug": "p1", "name": "P1"})), Some(token.as_str())).await;
    let project_id = project["prn"].as_str().expect("project.prn").rsplit('/').next().unwrap().to_string();

    // (method, uri) for every tenancy route that takes a body.
    let routes: Vec<(&str, String)> = vec![
        ("POST", "/v1/organizations".to_string()),
        ("PATCH", format!("/v1/organizations/{org_id}")),
        ("POST", format!("/v1/organizations/{org_id}/teams")),
        ("PATCH", format!("/v1/teams/{team_id}")),
        ("POST", format!("/v1/teams/{team_id}/projects")),
        ("PATCH", format!("/v1/projects/{project_id}")),
    ];

    for (method, uri) in &routes {
        // 400: not JSON at all.
        let (status, err) = support::send_bytes(&app, method, uri, Some("application/json"), b"{not json", Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-request-body", "{method} {uri}: {err}");

        // 422: valid JSON, wrong shape. `RenameBody`'s fields are all `Option<String>` with no
        // `deny_unknown_fields`, so `{}` would DESERIALIZE and reach the handler — a type
        // mismatch is what actually reaches `JsonDataError` on the PATCH routes.
        let (status, err) = support::send_bytes(&app, method, uri, Some("application/json"), br#"{"slug": 1, "name": 2}"#, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], "invalid-request-schema", "{method} {uri}: {err}");
    }

    // 415 is an extractor-level fact, refused before any handler-specific code runs, so it is
    // asserted ONCE here rather than per route (the `json.rs` unit test covers the extractor;
    // this proves it is reachable on a real route at all).
    let (status, err) = support::send_bytes(&app, "POST", "/v1/organizations", Some("text/plain"), b"{}", Some(token.as_str())).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{err}");
    assert_eq!(err["error"]["code"], "unsupported-content-type", "{err}");

    // A well-formed body on the same route still reaches the handler — so every row above is an
    // assertion about the BODY's shape, not about the route being broken.
    let (status, _) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "still-works", "name": "Still Works"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
}
