// SPDX-License-Identifier: Apache-2.0

//! SMA-676 end to end over HTTP, against the real Cedar policy set and Postgres (Docker):
//! an `org_admin` lists the grants at its own org (D4 b) and at no other org; grants
//! `gateway_user` to a person, twice, and gets one grant (D9); the person may then
//! `InvokeModel` on the org; the admin revokes, and the person may not. Plus the D3, D6 and D7
//! refusals. The fake IAM of the console is not Cedar, so this file is where the decision is
//! proved; the console e2e row R30 proves only the wiring.
//!
//! Runs against an ephemeral Postgres in Docker. In CI a missing daemon is a hard failure;
//! run a filtered local invocation with `PAIGASUS_REQUIRE_DOCKER=1`.

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::PgOrganizationRepository;
use paigasus_iam_core::{Clock, IdGenerator, Organization, OrganizationRepository, Slug, Stamp, Team};
use sea_orm::DatabaseConnection;
use serde_json::json;
use support::{app_with_state, provision, seed_org_admin, send};

/// A fresh org with its default team and an owner `org_admin` grant, through the real repo.
async fn seed_org(db: &DatabaseConnection, slug: &str) -> Organization {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let owner = ids.new_principal_id();
    let stamp = Stamp::new(clock.now(), owner.clone());
    let org = Organization::new(ids.new_organization_id(), Slug::parse(slug).unwrap(), "Org", &stamp).unwrap();
    let default_team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
    let owner_grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    repo.create(&org, &default_team, &owner_grant, &stamp).await.unwrap();
    org
}

#[tokio::test]
async fn an_org_admin_grants_and_revokes_gateway_user_and_invoke_model_follows() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let org = seed_org(&db, "pma-acme").await;
    let org_prn = org.id.canonical();

    let admin_token = idp.bearer("pma-admin", Some("pma-admin@example.com"), "paigasus", 3600);
    let admin_prn = provision(&state, &admin_token).await;
    seed_org_admin(&state, &admin_prn, &org_prn).await;
    let person_token = idp.bearer("pma-person", Some("pma-person@example.com"), "paigasus", 3600);
    let person_prn = provision(&state, &person_token).await;

    let holders = format!("/v1/authz/role-grants?scope_prn={org_prn}&role_key=gateway_user&principal_kind=user&limit=200");
    let (status, listed) = send(&app, "GET", &holders, None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed, json!([]));

    let body = json!({ "principal_prn": person_prn, "role_key": "gateway_user", "scope_prn": org_prn });
    let (status, first) = send(&app, "POST", "/v1/authz/role-grants", Some(body.clone()), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, second) = send(&app, "POST", "/v1/authz/role-grants", Some(body), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "D9: a second grant is a success: {second}");
    assert_eq!(second["id"], first["id"], "D9: the second grant returns the existing grant");

    let (status, listed) = send(&app, "GET", &holders, None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["principal_prn"], json!(person_prn));

    let invoke = json!({ "principal_prn": person_prn, "action": "InvokeModel", "resource_prn": org_prn });
    let (status, decision) = send(&app, "POST", "/v1/authz/is-authorized", Some(invoke.clone()), Some(person_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert_eq!(decision["allowed"], true, "gateway_user at the org allows InvokeModel on the org: {decision}");

    let grant_id = first["id"].as_str().expect("a grant id");
    let (status, body) = send(&app, "DELETE", &format!("/v1/authz/role-grants/{grant_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, decision) = send(&app, "POST", "/v1/authz/is-authorized", Some(invoke), Some(person_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert_eq!(decision["allowed"], false, "after the revoke the person may not InvokeModel: {decision}");
}

/// D4 (b) and Review Focus 5: the same `org_admin` lists at its own org — also when the PRN
/// carries an UPPER-case uuid, which must match the stored lower-case canonical PRN — and is
/// refused at another org.
#[tokio::test]
async fn an_org_admin_may_list_at_its_own_org_with_an_upper_case_uuid_and_not_at_another_org() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let own = seed_org(&db, "pma-own").await;
    let other = seed_org(&db, "pma-other").await;
    let admin_token = idp.bearer("pma-lister", Some("pma-lister@example.com"), "paigasus", 3600);
    let admin_prn = provision(&state, &admin_token).await;
    seed_org_admin(&state, &admin_prn, &own.id.canonical()).await;

    let upper = format!("prn:pgs:iam:::organization/{}", own.id.uuid().to_string().to_uppercase());
    let (status, listed) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={upper}&role_key=org_admin"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(
        listed.as_array().unwrap().iter().any(|g| g["principal_prn"] == json!(admin_prn)),
        "the admin's own org_admin grant is at this org: {listed}"
    );

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={}", other.id.canonical()), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
}

/// D3, D6, D7 and Review Focus 2 over HTTP.
#[tokio::test]
async fn the_http_list_refuses_no_filter_an_unknown_kind_and_a_large_scope_page_but_not_a_large_principal_page() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let org = seed_org(&db, "pma-rules").await;
    let token = idp.bearer("pma-rules", Some("pma-rules@example.com"), "paigasus", 3600);
    let me = provision(&state, &token).await;
    seed_org_admin(&state, &me, &org.id.canonical()).await;
    let scope = org.id.canonical();

    let (status, body) = send(&app, "GET", "/v1/authz/role-grants", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "missing-required-field");

    for kind in ["robot", "", "USER"] {
        let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={scope}&principal_kind={kind}"), None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind:?}: {body}");
        assert_eq!(body["error"]["code"], "invalid-principal-kind", "{kind:?}");
    }

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={scope}&limit=201"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid-pagination");

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?principal_prn={me}&limit=500"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "D6: the bare principal request ignores limit: {body}");
    assert!(!body.as_array().unwrap().is_empty());
}
