// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/memberships` + `/v1/users` (SMA-442 AC-1): the full
//! attach/list/detach lifecycle across a user, an organization, and a team — including the
//! org-membership invariant, the forged-node-prn defense, cascade-on-detach, and the
//! membership-filter/user-creation validation errors. Drives the real
//! `router(AppState::new(db))` via `tower::ServiceExt::oneshot` — no listening socket —
//! against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).

mod support;

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};
use support::{app, send};
use uuid::Uuid;

/// Creates a user via `POST /v1/users` and returns its `principal_prn`.
async fn create_user(app: &Router, email: &str) -> String {
    let (status, body) = send(app, "POST", "/v1/users", Some(json!({"email": email, "display_name": "Test User"}))).await;
    assert_eq!(status, StatusCode::CREATED, "create_user({email}) failed: {body}");
    body["principal_prn"].as_str().expect("principal_prn").to_string()
}

/// The full AC-1 end-to-end scenario, in order: create a user, create an org, attach the
/// principal to the org, create a team under the org, attach the principal to the team, list
/// by principal (both, ordered), forge a node prn (correct team uuid, wrong org uuid) and
/// confirm `prn-mismatch`, attach a second (org-membership-less) principal to the team and
/// confirm `missing-org-membership`, detach the org membership, confirm the cascade empties
/// the list, and detach the same id again to confirm `not-found`.
#[tokio::test]
async fn ac1_membership_lifecycle_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    // 1. Create the principal.
    let user_prn = create_user(&app, "alice@example.com").await;

    // 2. Create the organization.
    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Acme Corp."}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();

    // 3. Attach principal -> org: 201.
    let (status, org_membership) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": org_prn}))).await;
    assert_eq!(status, StatusCode::CREATED, "{org_membership}");
    assert_eq!(org_membership["principal_prn"], user_prn);
    assert_eq!(org_membership["node_prn"], org_prn);
    let org_membership_id = org_membership["id"].as_str().unwrap().to_string();

    // 4. Create a team under the org.
    let (status, team_body) = send(&app, "POST", &format!("/v1/organizations/{org_id}/teams"), Some(json!({"slug": "eng", "name": "Engineering"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let team_prn = team_body["prn"].as_str().unwrap().to_string();
    let team_id = team_prn.rsplit('/').next().unwrap().to_string();

    // 5. Attach principal -> team: 201 (the org membership from step 3 satisfies the
    // org-membership invariant).
    let (status, team_membership) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": team_prn}))).await;
    assert_eq!(status, StatusCode::CREATED, "{team_membership}");
    assert_eq!(team_membership["node_prn"], team_prn);

    // 6. List by principal: both, ordered (org attached first, so it comes first).
    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["node_prn"], org_prn);
    assert_eq!(listed[1]["node_prn"], team_prn);

    // 7. Forge a node prn: the correct team uuid, but a different org uuid in the org slot.
    // A fixed low-value uuid never collides with a real (UUIDv7, clock-derived) org id.
    let wrong_org = Uuid::from_u128(9_999);
    let forged_team_prn = format!("prn:pgs:iam::{wrong_org}:team/{team_id}");
    let (status, err) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": forged_team_prn}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "prn-mismatch");

    // 8. A second principal, with no org membership, attaching to the team: 409
    // `missing-org-membership`.
    let second_user_prn = create_user(&app, "bob@example.com").await;
    let (status, err) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": second_user_prn, "node_prn": team_prn}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "missing-org-membership");

    // 9. Detach the org membership: 204.
    let (status, body) = send(&app, "DELETE", &format!("/v1/memberships/{org_membership_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    // 10. Listing by principal is now empty — detaching the org membership cascades onto the
    // same principal's team membership in that org (rule 5).
    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.as_array().unwrap().is_empty());

    // 11. Detaching the same id again: 404 `not-found`.
    let (status, err) = send(&app, "DELETE", &format!("/v1/memberships/{org_membership_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not-found");
}

#[tokio::test]
async fn list_memberships_requires_exactly_one_filter() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    // Neither `principal` nor `node` set: 400 `invalid-prn`. (`TenancyError::InvalidPrn`'s
    // `Display` is a fixed, generic message across every construction site — the same
    // convention as `parse_principal_prn`/`parse_node_prn` in `application::memberships` — so
    // only the stable `code` is asserted here, matching `http_tenancy.rs`'s convention.)
    let (status, err) = send(&app, "GET", "/v1/memberships", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-prn");

    // Both set: 400 `invalid-prn`.
    let user_prn = create_user(&app, "carol@example.com").await;
    let (status, err) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}&node={user_prn}"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-prn");
}

#[tokio::test]
async fn create_user_rejects_duplicate_email() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    let _ = create_user(&app, "dupe@example.com").await;

    let (status, err) = send(&app, "POST", "/v1/users", Some(json!({"email": "dupe@example.com", "display_name": "Second"}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "email-conflict");
}

#[tokio::test]
async fn create_user_rejects_invalid_email() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let app = app(db);

    let (status, err) = send(&app, "POST", "/v1/users", Some(json!({"email": "not-an-email", "display_name": "Nope"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-email");
}
