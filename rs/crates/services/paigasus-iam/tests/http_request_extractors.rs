// SPDX-License-Identifier: Apache-2.0

//! SMA-588, end-to-end on REAL routes: a refused query string or path segment answers inside
//! the `{"error":{code,message}}` envelope with a registered reason.
//!
//! Driven against the merged `router(...)` rather than a synthetic one, for the reason SMA-586
//! learned expensively: a synthetic route proves the EXTRACTOR, never the handler wiring, and
//! that is exactly how a mis-named `{sa}` path segment survived its whole suite. Each row here
//! pins one live route's extractor choice.
//!
//! No tenancy fixtures are seeded, except in the second test: its three nested list routes take
//! a uuid path segment before their query string, so it creates an organization (and, for the
//! api-keys route, a service account) to get real uuids to nest under. Both extractors refuse
//! BEFORE the handler runs, so every other row is reachable with nothing but a valid bearer —
//! and each block ends with a well-formed request on the same route, so a row cannot pass merely
//! because the route is broken.
//!
//! All three capability flags default to `true` in `support::test_config`, so every route below
//! is mounted. A disabled capability would 404 and the rows would pass for the wrong reason.

mod support;

use axum::http::StatusCode;
use support::{app_with_state, provision_platform_admin, send};

/// Resolves a registry wire string through the enum rather than restating a kebab literal.
fn wire(reason: paigasus_proto::paigasus::common::v1::ErrorReason) -> String {
    reason.as_wire_reason().expect("not the Unspecified sentinel")
}

#[tokio::test]
async fn a_refused_query_string_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("query-user", Some("query@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    // (uri with a refused query, uri with a well-formed one).
    //
    // Eight routes carry a numeric field and use `?limit=abc`. `list_role_grants` carries NO
    // numeric field — `RoleGrantQuery` is a lone `Option<String>` — so `?limit=abc` there is an
    // ignored UNKNOWN key and answers 200. It uses a REPEATED key instead, which reaches every
    // field on every route. Getting this wrong is how a row ends up asserting nothing.
    let cases: Vec<(&str, &str)> = vec![
        ("/v1/organizations?limit=abc", "/v1/organizations?limit=1"),
        ("/v1/authz/policies?limit=abc", "/v1/authz/policies?limit=1"),
        ("/v1/memberships?limit=abc&principal=x", "/v1/memberships?limit=1&principal=x"),
        ("/v1/service-accounts?limit=abc&owner_prn=x", "/v1/service-accounts?limit=1&owner_prn=x"),
        ("/v1/audit?limit=abc", "/v1/audit?limit=1"),
        ("/v1/outbox/dead-letters?limit=abc", "/v1/outbox/dead-letters?limit=1"),
        ("/v1/authz/role-grants?principal_prn=a&principal_prn=b", "/v1/authz/role-grants?principal_prn=a"),
    ];

    for (bad, good) in &cases {
        let (status, err) = send(&app, "GET", bad, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {bad}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "GET {bad}: {err}");
        assert_eq!(err["error"]["message"], "invalid query parameter", "GET {bad}: {err}");

        // The same route with a well-formed query reaches the handler. Any non-4xx-extractor
        // outcome proves the row above was about the QUERY, not about a broken route.
        let (status, err) = send(&app, "GET", good, None, Some(token.as_str())).await;
        assert_ne!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "GET {good} must reach the handler, got {status}: {err}");
    }

    // A repeated key reaches a route whose failing field is NUMERIC too — the same class, the
    // other field type, so neither is assumed from the other.
    let (status, err) = send(&app, "GET", "/v1/organizations?limit=1&limit=2", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "{err}");
}

/// The three nested list routes, which take a uuid path segment BEFORE their query string — so a
/// refused query on them proves the two extractors compose in one signature.
#[tokio::test]
async fn a_refused_query_on_a_nested_list_route_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use serde_json::json;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("nested-user", Some("nested@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let (_, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "nested", "name": "Nested"})), Some(token.as_str())).await;
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();
    let team_id = created["default_team"]["prn"].as_str().expect("default_team.prn").rsplit('/').next().unwrap().to_string();

    // `api_keys.rs`'s list route also nests under a uuid path segment — a service account's,
    // not an organization's or a team's — so it needs its own fixture: a real service account,
    // owned by the organization created above.
    let (_, sa_created) = send(&app, "POST", "/v1/service-accounts", Some(json!({"owner_prn": org_prn, "name": "nested-sa"})), Some(token.as_str())).await;
    let sa_id = sa_created["prn"].as_str().expect("service account prn").rsplit('/').next().unwrap().to_string();

    for uri in [
        format!("/v1/organizations/{org_id}/teams?limit=abc"),
        format!("/v1/teams/{team_id}/projects?limit=abc"),
        format!("/v1/service-accounts/{sa_id}/api-keys?limit=abc"),
    ] {
        let (status, err) = send(&app, "GET", &uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {uri}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidQueryParameter), "GET {uri}: {err}");
    }

    // Well-formed on the same routes reaches the handler.
    let (status, err) = send(&app, "GET", &format!("/v1/organizations/{org_id}/teams?limit=1"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    let (status, err) = send(&app, "GET", &format!("/v1/teams/{team_id}/projects?limit=1"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    let (status, err) = send(&app, "GET", &format!("/v1/service-accounts/{sa_id}/api-keys?limit=1"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{err}");
}

/// The two `Path<String>` routes. `%FF` is not a valid UTF-8 percent-encoding, so axum refuses
/// the segment before the handler; the extractor names the field it stands for.
#[tokio::test]
async fn an_undecodable_path_segment_answers_in_the_error_envelope() {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("path-user", Some("segment@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let cases = [("DELETE", "/v1/authz/policies/%FF"), ("POST", "/v1/authz/system-policies/%FF/retire")];
    for (method, uri) in cases {
        let (status, err) = send(&app, method, uri, None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
        assert_eq!(err["error"]["code"], wire(ErrorReason::InvalidPathSegment), "{method} {uri}: {err}");
        assert_eq!(err["error"]["message"], "policy_id is not a valid path segment", "{method} {uri}: {err}");
    }

    // An ORDINARY, decodable segment reaches the handler — it is not a uuid, which is exactly
    // why these two routes need `StringPath` rather than `UuidPath`. `PolicyService::delete` is
    // idempotent (deleting an id that never existed is `Ok(())`), so this proves the segment
    // reached `delete_policy`'s own logic and completed: `204 NO_CONTENT`, empirically observed.
    // An `assert_ne!` on the error code here would pass just as well on a 401/403/404/500 — it
    // would prove only "not the extractor's refusal", never that the handler ran.
    let (status, _) = send(&app, "DELETE", "/v1/authz/policies/allow-root-read", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "a decodable segment must reach the handler, which deletes idempotently");

    // Same for `retire`: a decodable, non-uuid, but unknown policy id reaches the handler,
    // which reports `NotFound` on its own terms (empirically observed: 404,
    // `{"error":{"code":"not-found","message":"resource not found"}}`). `retire` and
    // `delete_policy` share the `PolicyId` marker and identical `InvalidPathSegment` message
    // text above, so only a control that pins `retire`'s OWN outcome distinguishes a correctly
    // wired `retire` from one silently mis-wired to `delete_policy`'s handler.
    let (status, err) = send(&app, "POST", "/v1/authz/system-policies/some-policy-id/retire", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], wire(ErrorReason::NotFound), "{err}");
    assert_eq!(err["error"]["message"], "resource not found", "{err}");
}
