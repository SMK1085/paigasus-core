// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/memberships` + `/v1/users` (SMA-442 AC-1): the full
//! attach/list/detach lifecycle across a user, an organization, and a team — including the
//! org-membership invariant, the forged-node-prn defense, cascade-on-detach, and the
//! membership-filter/user-creation validation errors. Drives the real
//! `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no listening socket —
//! against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).

mod support;

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};
use support::{app_with_config, app_with_state, provision, provision_platform_admin, send, test_config};
use uuid::Uuid;

/// Creates a user via `POST /v1/users` and returns its `principal_prn`.
async fn create_user(app: &Router, token: &str, email: &str) -> String {
    let (status, body) = send(app, "POST", "/v1/users", Some(json!({"email": email, "display_name": "Test User"})), Some(token)).await;
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
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-444 Task 20: `/v1/organizations`, `/v1/memberships`, and the nested `.../teams`
    // route below are now enforced — seed the acting principal a `platform_admin` grant.
    provision_platform_admin(&state, &token).await;

    // 1. Create the principal.
    let user_prn = create_user(&app, &token, "alice@example.com").await;

    // 2. Create the organization.
    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "acme", "name": "Acme Corp."})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();

    // 3. Attach principal -> org: 201.
    let (status, org_membership) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": org_prn})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{org_membership}");
    assert_eq!(org_membership["principal_prn"], user_prn);
    assert_eq!(org_membership["node_prn"], org_prn);
    let org_membership_id = org_membership["id"].as_str().unwrap().to_string();

    // 4. Create a team under the org.
    let (status, team_body) = send(
        &app,
        "POST",
        &format!("/v1/organizations/{org_id}/teams"),
        Some(json!({"slug": "eng", "name": "Engineering"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let team_prn = team_body["prn"].as_str().unwrap().to_string();
    let team_id = team_prn.rsplit('/').next().unwrap().to_string();

    // 5. Attach principal -> team: 201 (the org membership from step 3 satisfies the
    // org-membership invariant).
    let (status, team_membership) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": team_prn})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{team_membership}");
    assert_eq!(team_membership["node_prn"], team_prn);

    // 6. List by principal: both, ordered (org attached first, so it comes first).
    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["node_prn"], org_prn);
    assert_eq!(listed[1]["node_prn"], team_prn);

    // 6b. SMA-676 D8: `principal_kind=user` keeps both (alice is a `user`); `service_account`
    // keeps neither; an unrecognized value is refused before any repository read.
    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}&principal_kind=user"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 2);

    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}&principal_kind=service_account"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.as_array().unwrap().is_empty());

    let (status, err) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}&principal_kind=bogus"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-principal-kind");

    // 7. Forge a node prn: the correct team uuid, but a different org uuid in the org slot.
    // A fixed low-value uuid never collides with a real (UUIDv7, clock-derived) org id.
    let wrong_org = Uuid::from_u128(9_999);
    let forged_team_prn = format!("prn:pgs:iam::{wrong_org}:team/{team_id}");
    let (status, err) = send(
        &app,
        "POST",
        "/v1/memberships",
        Some(json!({"principal_prn": user_prn, "node_prn": forged_team_prn})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "prn-mismatch");

    // 8. A second principal, with no org membership, attaching to the team: 409
    // `missing-org-membership`.
    let second_user_prn = create_user(&app, &token, "bob@example.com").await;
    let (status, err) = send(
        &app,
        "POST",
        "/v1/memberships",
        Some(json!({"principal_prn": second_user_prn, "node_prn": team_prn})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "missing-org-membership");

    // 9. Detach the org membership: 204.
    let (status, body) = send(&app, "DELETE", &format!("/v1/memberships/{org_membership_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    // 10. Listing by principal is now empty — detaching the org membership cascades onto the
    // same principal's team membership in that org (rule 5).
    let (status, listed) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.as_array().unwrap().is_empty());

    // 11. Detaching the same id again: 404 `not-found`.
    let (status, err) = send(&app, "DELETE", &format!("/v1/memberships/{org_membership_id}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not-found");
}

#[tokio::test]
async fn list_memberships_requires_exactly_one_filter() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-584: `POST /v1/users` now requires `Action::CreateUser`@`Root`. These tests are about
    // the membership filter / duplicate-email / invalid-email behaviour, not authorization, so
    // they authenticate as a platform_admin to get past the gate — `tests/http_users.rs` owns
    // the authorization cases.
    provision_platform_admin(&state, &token).await;

    // Neither `principal` nor `node` set: 400 `missing-required-field`. These two cases used
    // to share one reason (`invalid-prn`); SMA-586 D6 split them, because "you omitted a
    // filter" and "you sent two" are different mistakes with different fixes.
    let (status, err) = send(&app, "GET", "/v1/memberships", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "missing-required-field");

    // Both set: 400 `mutually-exclusive-fields`. HTTP is the only surface that can produce
    // this — the gRPC twin models the choice as a `oneof`, which cannot carry two values.
    let user_prn = create_user(&app, &token, "carol@example.com").await;
    let (status, err) = send(&app, "GET", &format!("/v1/memberships?principal={user_prn}&node={user_prn}"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "mutually-exclusive-fields");
}

#[tokio::test]
async fn create_user_rejects_duplicate_email() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-584: `POST /v1/users` now requires `Action::CreateUser`@`Root`. These tests are about
    // the membership filter / duplicate-email / invalid-email behaviour, not authorization, so
    // they authenticate as a platform_admin to get past the gate — `tests/http_users.rs` owns
    // the authorization cases.
    provision_platform_admin(&state, &token).await;

    let _ = create_user(&app, &token, "dupe@example.com").await;

    let (status, err) = send(&app, "POST", "/v1/users", Some(json!({"email": "dupe@example.com", "display_name": "Second"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "email-conflict");
}

#[tokio::test]
async fn create_user_rejects_invalid_email() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-584: `POST /v1/users` now requires `Action::CreateUser`@`Root`. These tests are about
    // the membership filter / duplicate-email / invalid-email behaviour, not authorization, so
    // they authenticate as a platform_admin to get past the gate — `tests/http_users.rs` owns
    // the authorization cases.
    provision_platform_admin(&state, &token).await;

    let (status, err) = send(&app, "POST", "/v1/users", Some(json!({"email": "not-an-email", "display_name": "Nope"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-email");
}

/// HTTP-body-envelope coverage for `POST /v1/memberships` (SMA-587 Task 5), mirroring
/// `tests/http_tenancy.rs::a_refused_body_answers_in_the_error_envelope`'s shape exactly.
#[tokio::test]
async fn a_refused_body_answers_in_the_error_envelope() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("envelope-membership-user", Some("envelope-membership@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // 400: not JSON at all.
    let (status, err) = support::send_bytes(&app, "POST", "/v1/memberships", Some("application/json"), b"{not json", Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-request-body", "{err}");

    // 422: valid JSON, wrong shape. `CreateMembershipBody`'s `principal_prn`/`node_prn` are
    // both required `String`s, so a number in either slot is a genuine type mismatch.
    let (status, err) = support::send_bytes(
        &app,
        "POST",
        "/v1/memberships",
        Some("application/json"),
        br#"{"principal_prn": 1, "node_prn": 2}"#,
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["error"]["code"], "invalid-request-schema", "{err}");

    // A well-formed body on the same route still reaches the handler — seed a real user +
    // org so the control attach succeeds rather than failing on an unrelated validation error.
    let user_prn = create_user(&app, &token, "envelope-membership-target@example.com").await;
    let (status, org_body) = send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "envelope-membership", "name": "Envelope Membership"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let (status, body) = send(&app, "POST", "/v1/memberships", Some(json!({"principal_prn": user_prn, "node_prn": org_prn})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Percent-encodes the two PRN characters that are reserved in a URL. A colon is legal in a
/// query value, but the test must not depend on that (spec T3).
fn q(prn: &str) -> String {
    prn.replace(':', "%3A").replace('/', "%2F")
}

/// Replaces the region and the organization slot of a canonical principal PRN.
fn principal_with(prn: &str, region: &str, org: &str) -> String {
    let uuid = prn.rsplit('/').next().expect("a principal prn ends in /<uuid>");
    format!("prn:pgs:iam:{region}:{org}:principal/{uuid}")
}

/// SMA-649 T3 (HTTP): `GET /v1/memberships?principal=…` confirms the principal PRN against the
/// stored principal, under both `enforce_tenancy` settings, with `principal_kind` unset and
/// `user`. Forged region or org slot -> 400 `prn-mismatch`; unknown uuid -> 404 `not-found`;
/// unknown uuid + `limit=500` -> 400 `invalid-pagination` (B8). Controls: the canonical PRN
/// and its upper-case-uuid form list the seeded membership.
#[tokio::test]
async fn a_forged_principal_prn_never_lists_memberships_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (on_app, on_state, idp) = app_with_state(db.clone()).await;
    let mut cfg = test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let (off_app, _off_state) = app_with_config(db.clone(), &cfg).await;
    let token = idp.bearer("forged-lm-http", Some("forged-lm-http@example.com"), "paigasus", 3600);
    provision_platform_admin(&on_state, &token).await;

    let alice = create_user(&on_app, &token, "lm-http-alice@example.com").await;
    let (status, org_body) = send(&on_app, "POST", "/v1/organizations", Some(json!({"slug": "lm-http", "name": "LM HTTP"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let real_org = org_prn.rsplit('/').next().unwrap().to_string();
    let (status, body) = send(&on_app, "POST", "/v1/memberships", Some(json!({"principal_prn": alice, "node_prn": org_prn})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let absent_org = Uuid::from_u128(0x0f49).to_string();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));
    let upper = principal_with(&alice, "", "").replace(alice.rsplit('/').next().unwrap(), &alice.rsplit('/').next().unwrap().to_uppercase());
    let mut failures: Vec<String> = Vec::new();

    for (setting, app) in [("enforce=on", &on_app), ("enforce=off", &off_app)] {
        for kind in ["", "&principal_kind=user"] {
            let expect = |failures: &mut Vec<String>, label: String, got: (StatusCode, Value), status: StatusCode, code: &str| {
                if got.0 != status || got.1["error"]["code"] != code {
                    failures.push(format!("{label}: expected {status} {code}, got {} {}", got.0, got.1));
                }
            };
            for (shape, forged) in [
                ("non-empty region", principal_with(&alice, "eu-west-1", "")),
                ("real org uuid in the org slot", principal_with(&alice, "", &real_org)),
                ("absent org uuid in the org slot", principal_with(&alice, "", &absent_org)),
            ] {
                let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&forged)), None, Some(token.as_str())).await;
                expect(&mut failures, format!("{setting} [{kind}] {shape}"), got, StatusCode::BAD_REQUEST, "prn-mismatch");
            }
            let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&unknown)), None, Some(token.as_str())).await;
            expect(&mut failures, format!("{setting} [{kind}] unknown principal"), got, StatusCode::NOT_FOUND, "not-found");
            let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}&limit=500", q(&unknown)), None, Some(token.as_str())).await;
            expect(
                &mut failures,
                format!("{setting} [{kind}] unknown principal + limit 500 (B8)"),
                got,
                StatusCode::BAD_REQUEST,
                "invalid-pagination",
            );

            for (shape, prn) in [("canonical", alice.clone()), ("upper-case uuid", upper.clone())] {
                let (status, body) = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&prn)), None, Some(token.as_str())).await;
                let listed = body.as_array().map(|rows| rows.iter().any(|r| r["node_prn"] == org_prn)).unwrap_or(false);
                if status != StatusCode::OK || !listed {
                    failures.push(format!("{setting} [{kind}] control {shape}: {status} {body}"));
                }
            }
        }
    }

    assert!(failures.is_empty(), "forged principal ListMemberships HTTP cases failed:\n{}", failures.join("\n"));
}

/// SMA-649 T3, B6 / AC5 (HTTP): with `enforce_tenancy` on, a caller without a root grant gets
/// 403 `forbidden` for a forged, a canonical and an unknown principal PRN alike. The handler
/// authorizes at `root_prn()` before `list` (`adapters/http/memberships.rs`); only this test
/// pins that order on HTTP.
#[tokio::test]
async fn an_ungranted_caller_cannot_list_memberships_by_any_principal_prn_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("b6-http-admin", Some("b6-http-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;
    let alice = create_user(&app, &admin, "b6-http-alice@example.com").await;
    let stranger = idp.bearer("b6-http-stranger", Some("b6-http-stranger@example.com"), "paigasus", 3600);
    provision(&state, &stranger).await;

    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));
    for (label, prn) in [
        ("canonical", alice.clone()),
        ("forged region", principal_with(&alice, "eu-west-1", "")),
        ("forged org slot", principal_with(&alice, "", &Uuid::from_u128(0x0f49).to_string())),
        ("unknown", unknown),
    ] {
        let (status, body) = send(&app, "GET", &format!("/v1/memberships?principal={}", q(&prn)), None, Some(stranger.as_str())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        assert_eq!(body["error"]["code"], "forbidden", "{label}");
    }
}
