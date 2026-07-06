// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for the `/v1` tenancy API (organizations/teams/projects):
//! full org lifecycle, nested team/project creation (`org_prn`/`team_prn` + effective-status
//! folding across the hierarchy), and pagination. Drives the real `router(AppState::new(db))`
//! via `tower::ServiceExt::oneshot` — no listening socket — against an ephemeral Postgres
//! (Docker; see `tests/support/mod.rs`).

mod support;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::http::{AppState, router};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn app(db: DatabaseConnection) -> Router {
    router(AppState::new(db))
}

/// Drives one request through the router and returns `(status, json body)`. `Value::Null`
/// stands in for an empty body (the archive/restore/health endpoints don't all have one, but
/// every endpoint under test here does).
async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&b).unwrap())
        }
        None => Body::empty(),
    };
    let request = builder.body(body).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}

#[tokio::test]
async fn org_lifecycle_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    // Create: 201, body has `organization` + `default_team` with parseable PRNs.
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Acme Corp."}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn");
    assert!(org_prn.starts_with("prn:pgs:iam:::organization/"), "unexpected org prn: {org_prn}");
    let team_prn = created["default_team"]["prn"].as_str().expect("default_team.prn");
    assert!(team_prn.contains(":team/"), "unexpected team prn: {team_prn}");
    assert_eq!(created["default_team"]["org_prn"], org_prn);
    let org_id = org_prn.rsplit('/').next().unwrap();

    // Get: 200.
    let (status, got) = send(&app, "GET", &format!("/v1/organizations/{org_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["slug"], "acme");
    assert_eq!(got["prn"], org_prn);

    // Duplicate slug -> 409 `slug-conflict`.
    let (status, err) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Dup"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "slug-conflict");

    // PATCH rename -> 200.
    let (status, renamed) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({"name": "Acme Renamed"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Acme Renamed");
    assert_eq!(renamed["slug"], "acme");

    // PATCH with an empty body -> 400 `nothing-to-rename`.
    let (status, err) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "nothing-to-rename");

    // Archive/restore -> 200, own + effective status flip.
    let (status, archived) = send(&app, "POST", &format!("/v1/organizations/{org_id}/archive"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["status"], "archived");
    assert_eq!(archived["effective_status"], "archived");

    let (status, restored) = send(&app, "POST", &format!("/v1/organizations/{org_id}/restore"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["status"], "active");
    assert_eq!(restored["effective_status"], "active");

    // Unknown id -> 404 `not-found`.
    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{}", Uuid::nil()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not-found");
}

#[tokio::test]
async fn nested_team_and_project_creation_folds_effective_status() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "beta", "name": "Beta"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();

    // A (non-default) team nested under the org.
    let (status, team_body) = send(&app, "POST", &format!("/v1/organizations/{org_id}/teams"), Some(json!({"slug": "eng", "name": "Engineering"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(team_body["org_prn"], org_prn);
    let team_prn = team_body["prn"].as_str().unwrap().to_string();
    let team_id = team_prn.rsplit('/').next().unwrap().to_string();

    // A project nested under the team.
    let (status, project_body) = send(&app, "POST", &format!("/v1/teams/{team_id}/projects"), Some(json!({"slug": "web", "name": "Web"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(project_body["team_prn"], team_prn);
    assert_eq!(project_body["org_prn"], org_prn);
    assert_eq!(project_body["effective_status"], "active");
    let project_id = project_body["prn"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    // List teams (default + eng) / projects (web) under their parents.
    let (status, teams) = send(&app, "GET", &format!("/v1/organizations/{org_id}/teams"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(teams.as_array().unwrap().len(), 2);

    let (status, projects) = send(&app, "GET", &format!("/v1/teams/{team_id}/projects"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects.as_array().unwrap().len(), 1);

    // Archiving the org flips the team's and project's *effective* status without touching
    // their own `status` flag (D1/D10).
    let (status, _) = send(&app, "POST", &format!("/v1/organizations/{org_id}/archive"), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, team_after) = send(&app, "GET", &format!("/v1/teams/{team_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(team_after["status"], "active");
    assert_eq!(team_after["effective_status"], "archived");

    let (status, project_after) = send(&app, "GET", &format!("/v1/projects/{project_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project_after["status"], "active");
    assert_eq!(project_after["effective_status"], "archived");

    // Renaming a project that is only *effectively* archived (via the still-archived org)
    // is rejected too — the guard folds ancestors, not just the project's own status.
    let (status, err) = send(&app, "PATCH", &format!("/v1/projects/{project_id}"), Some(json!({"slug": "web2"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "node-archived");

    // Restore the org, unblocking the project again; rename/archive/restore then work
    // through the project's own routes.
    let (status, _) = send(&app, "POST", &format!("/v1/organizations/{org_id}/restore"), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, renamed) = send(&app, "PATCH", &format!("/v1/projects/{project_id}"), Some(json!({"slug": "web2"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["slug"], "web2");
    assert_eq!(renamed["effective_status"], "active");

    let (status, archived) = send(&app, "POST", &format!("/v1/projects/{project_id}/archive"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["status"], "archived");
    assert_eq!(archived["effective_status"], "archived");

    let (status, restored) = send(&app, "POST", &format!("/v1/projects/{project_id}/restore"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["status"], "active");
    assert_eq!(restored["effective_status"], "active");
}

#[tokio::test]
async fn list_pagination_and_invalid_limit() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    for slug in ["alpha", "bravo", "charlie"] {
        let (status, _) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": slug, "name": slug}))).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, page) = send(&app, "GET", "/v1/organizations?limit=2&offset=0", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.as_array().unwrap().len(), 2);

    let (status, rest) = send(&app, "GET", "/v1/organizations?limit=2&offset=2", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rest.as_array().unwrap().len(), 1);

    // `limit=0` -> 400 `invalid-pagination`.
    let (status, err) = send(&app, "GET", "/v1/organizations?limit=0", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "invalid-pagination");
}
