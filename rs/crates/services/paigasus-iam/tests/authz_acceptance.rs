// SPDX-License-Identifier: Apache-2.0

//! Explicit, AC-labeled acceptance tests for the SMA-444 (M3 authorization/Cedar) issue's
//! three acceptance criteria, plus the Redis cross-replica + fail-open cases (spec D11/D12).
//! Every other authz integration test file (`tests/http_authz.rs`, `tests/grpc_authz.rs`,
//! `tests/authz_bootstrap.rs`, `tests/authz_cache_redis.rs`, `tests/cedar_authorizer.rs` unit
//! tests, ...) already exercises these mechanisms incidentally; this file names each
//! acceptance criterion explicitly, end-to-end, over the real HTTP/gRPC surfaces, against a
//! real Postgres (Docker) and — for the cross-replica/fail-open cases — a real Redis (Docker).
//!
//! **AC1** (a role grant changes effective access, immediately, on the same replica): granted
//! via the real `/v1/authz/role-grants` (HTTP) / `GrantRole` (gRPC) API by a seeded
//! `platform_admin` actor, observed via the real `/v1/authz/is-authorized` (HTTP) /
//! `IsAuthorized` (gRPC) API — deny before, allow after, same test, no sleep. This works
//! because `CedarAuthorizer::is_authorized` calls `PolicySnapshot::reload_if_stale`
//! *synchronously* before every decision (`src/adapters/authz/cedar_authorizer.rs`), not only
//! on a background poll interval.
//!
//! **AC2** (a denial/allow names its determining policy): a default-deny names
//! `DEFAULT_DENY_MARKER`; a grant-backed allow names `grant:<uuid>`; a `forbid` (the starter
//! `forbid-archived-writes` policy) names its own policy id and wins over a permitting grant.
//! Every assertion here is a SELF-query (the caller asks about their own access), so
//! `Authorize::decide_gated`'s self/admin exposure rule never redacts the response.
//!
//! **AC3** (a policy change takes effect within the cache-TTL bound): built against an
//! `AppState` whose `authz.policy_cache_ttl_secs` is 1 — the SAME synchronous
//! `reload_if_stale` AC1 exercises makes a `PutPolicy` change visible on the very next
//! decision, trivially satisfying any positive TTL bound. `policy_cache_ttl_secs` is actually
//! the BACKSTOP for a replica that never observes a `policy_gen` bump on its own (e.g. two
//! processes on the `memory` backend, or a `redis` replica between poll ticks) — the section
//! below exercises that cross-replica case directly.
//!
//! **Redis cross-replica + fail-open** (spec D11/D12): two independently-constructed
//! `AppState`s share one Postgres AND one Redis `authz.cache` backend. A grant made through
//! replica A's API is visible to replica B's very next decision — not because of the TTL
//! backstop, but because `Generations::Redis`'s `policy_gen` counter lives IN Redis itself
//! (`INCR`/`GET` on a well-known key), so `reload_if_stale`'s synchronous check on ANY
//! replica's next call observes ANY other replica's bump. A second case stops the Redis
//! container mid-flight and asserts a request still gets a real (default-deny) decision —
//! never a hung or errored request — evaluated against the last-known-good in-memory snapshot
//! plus a live Postgres entity-slice load.
//!
//! **SMA-470 revocation during a Redis outage**: the last case in this file covers what the
//! two above do not — every fail-open assertion here is about a READ taken while Redis is
//! down, never about an INVALIDATION issued while it is. See that test's own doc comment for
//! the mechanism.

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::config::{AuthzCacheBackend, AuthzCacheConfig};
use paigasus_iam_core::authz::engine::DEFAULT_DENY_MARKER;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::roles::FORBID_ARCHIVED_WRITES_ID;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use paigasus_proto::paigasus::iam::v1::{CreateOrganizationRequest, GrantRoleRequest, IsAuthorizedRequest};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use support::{app_with_config, app_with_state, provision, provision_platform_admin, send};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::transport::Channel;

/// Starts an ephemeral Redis container, returning its connection URL. The skip-versus-fail
/// decision lives once, in `support/docker.rs` (SMA-538).
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    support::docker::start_redis_or_skip("authz_acceptance").await
}

/// Spawns the full `grpc::router` (health + tenancy + authn + authorization, all wrapped by
/// the bearer layer) on an ephemeral port, mirroring `tests/grpc_authz.rs`/
/// `tests/grpc_tenancy.rs`; `abort()` the returned handle when the test finishes.
async fn spawn_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn channel(addr: SocketAddr) -> Channel {
    tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// Builds a `tonic::Request` carrying an `authorization: Bearer <token>` metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

// --- AC1: a role grant changes effective access, immediately, same replica ------------------

#[tokio::test]
async fn ac1_role_grant_changes_effective_access_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    let admin_token = idp.bearer("ac1-http-admin", Some("ac1-http-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let principal_token = idp.bearer("ac1-http-principal", Some("ac1-http-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    // Set up an org (+ its default team) as the admin. `CreateProject` authorizes against
    // the *parent team* (`src/adapters/http/teams.rs`), so the team is the resource under
    // test — a tenancy node genuinely under org O, per the AC.
    let (status, org_body) = send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "ac1-http-org", "name": "AC1 Http Org"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let team_prn = org_body["default_team"]["prn"].as_str().expect("default_team.prn").to_string();

    // Before the grant: P is default-denied CreateProject on the team.
    let (status, before) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "CreateProject", "resource_prn": team_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["allowed"], false, "{before}");
    assert_eq!(before["determining_policies"], json!([DEFAULT_DENY_MARKER]));

    // GrantRole(org_admin, P, O) — via the API, as the platform_admin actor.
    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": principal_prn, "role_key": "org_admin", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    let grant_id = granted["id"].as_str().expect("id").to_string();

    // AC1: the SAME is-authorized call, right after the grant, in the same test, on the same
    // replica — no sleep, no poll wait — now allows.
    let (status, after) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "CreateProject", "resource_prn": team_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["allowed"], true, "{after}");
    assert_eq!(after["determining_policies"], json!([format!("grant:{grant_id}")]));
}

#[tokio::test]
async fn ac1_role_grant_changes_effective_access_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();

    let admin_token = idp.bearer("ac1-grpc-admin", Some("ac1-grpc-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let principal_token = idp.bearer("ac1-grpc-principal", Some("ac1-grpc-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut tenancy = TenancyServiceClient::new(ch.clone());
    let created = tenancy
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: "ac1-grpc-org".to_string(),
                name: "AC1 Grpc Org".to_string(),
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner();
    let org_prn = created.organization.expect("organization").prn;
    let team_prn = created.default_team.expect("default_team").prn;

    let mut authz = AuthorizationServiceClient::new(ch);

    // Before the grant: P is default-denied CreateProject on the team.
    let before = authz
        .is_authorized(authed(
            IsAuthorizedRequest {
                principal_prn: principal_prn.clone(),
                action: "CreateProject".to_string(),
                resource_prn: team_prn.clone(),
                context: Default::default(),
            },
            &principal_token,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!before.allowed);
    assert_eq!(before.determining_policies, vec![DEFAULT_DENY_MARKER.to_string()]);

    // GrantRole(org_admin, P, O) — via the API, as the platform_admin actor.
    let granted = authz
        .grant_role(authed(
            GrantRoleRequest {
                principal_prn: principal_prn.clone(),
                role_key: "org_admin".to_string(),
                scope_prn: org_prn,
            },
            &admin_token,
        ))
        .await
        .unwrap()
        .into_inner()
        .grant
        .expect("grant");

    // AC1: the SAME IsAuthorized call, right after the grant, in the same test, on the same
    // replica — no sleep, no poll wait — now allows.
    let after = authz
        .is_authorized(authed(
            IsAuthorizedRequest {
                principal_prn,
                action: "CreateProject".to_string(),
                resource_prn: team_prn,
                context: Default::default(),
            },
            &principal_token,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(after.allowed);
    assert_eq!(after.determining_policies, vec![format!("grant:{}", granted.id)]);

    server.abort();
}

// --- AC2: a denial/allow names its determining policy ----------------------------------------

/// One self-querying principal, over HTTP, walked through all three `determining_policies`
/// shapes AC2 requires: (a) an ungranted default-deny names [`DEFAULT_DENY_MARKER`]; (b) an
/// allow backed by a role grant names `grant:<uuid>`; (c) a write on a now-archived resource
/// is denied naming the starter `forbid-archived-writes` policy id — even though the SAME
/// grant from (b) would otherwise permit it (a Cedar `forbid` always wins over a `permit`).
/// Every query below is a SELF-query (`principal_prn` == the bearer token's own principal), so
/// `Authorize::decide_gated`'s self/admin exposure rule never redacts the response — the raw
/// `determining_policies` is exactly what a non-self, unauthorized caller would NOT see
/// (`tests/http_authz.rs::is_authorized_non_self_query_by_an_unauthorized_actor_is_forbidden`
/// already covers that redaction).
#[tokio::test]
async fn ac2_denial_and_allow_decisions_name_their_determining_policy_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    let admin_token = idp.bearer("ac2-admin", Some("ac2-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let principal_token = idp.bearer("ac2-principal", Some("ac2-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    // Org -> default team -> project hierarchy, created by the admin.
    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "ac2-org", "name": "AC2 Org"})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let org_id = org_prn.rsplit('/').next().unwrap().to_string();
    let team_prn = org_body["default_team"]["prn"].as_str().expect("default_team.prn").to_string();
    let team_id = team_prn.rsplit('/').next().unwrap().to_string();

    let (status, project_body) = send(
        &app,
        "POST",
        &format!("/v1/teams/{team_id}/projects"),
        Some(json!({"slug": "ac2-proj", "name": "AC2 Project"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_body}");
    let project_prn = project_body["prn"].as_str().expect("project.prn").to_string();

    // (a) Default-deny names DEFAULT_DENY_MARKER.
    let (status, deny) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "GetProject", "resource_prn": project_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deny}");
    assert_eq!(deny["allowed"], false, "{deny}");
    assert_eq!(deny["determining_policies"], json!([DEFAULT_DENY_MARKER]));

    // (b) An allow, once granted, names the linked `grant:<uuid>` policy.
    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": principal_prn, "role_key": "org_admin", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    let grant_id = granted["id"].as_str().expect("id").to_string();

    let (status, allow) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "GetProject", "resource_prn": project_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allow}");
    assert_eq!(allow["allowed"], true, "{allow}");
    assert_eq!(allow["determining_policies"], json!([format!("grant:{grant_id}")]));

    // (c) Archiving the org folds the project's effective_status to "archived"; a subsequent
    // WRITE (RenameProject) is denied naming `forbid-archived-writes` — the org_admin grant
    // from (b) still exists and would otherwise permit RenameProject, but the starter forbid
    // policy takes precedence (Cedar semantics: a matching forbid always wins).
    let (status, archived) = send(&app, "POST", &format!("/v1/organizations/{org_id}/archive"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{archived}");

    let (status, forbidden) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "RenameProject", "resource_prn": project_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{forbidden}");
    assert_eq!(forbidden["allowed"], false, "{forbidden}");
    assert_eq!(forbidden["determining_policies"], json!([FORBID_ARCHIVED_WRITES_ID]));
}

// --- AC3: a policy change takes effect within the cache-TTL bound ---------------------------

/// Builds an `AppState` with a deliberately short `authz.policy_cache_ttl_secs` (1s), then
/// `PutPolicy`s a brand-new static permit and asserts the very next decision reflects it — no
/// sleep, no poll wait needed, since `CedarAuthorizer::is_authorized` calls
/// `PolicySnapshot::reload_if_stale` synchronously before every decision (the same mechanism
/// AC1 exercises). That makes "within the TTL bound" trivially true here: the change is
/// visible immediately, which is stronger than the TTL requires. `policy_cache_ttl_secs`
/// itself is the bound for a replica that does NOT observe a `policy_gen` bump synchronously
/// (a cross-replica change under the `memory` backend, or a `redis` replica between poll
/// ticks) — the Redis cross-replica case below exercises that scenario directly, over two
/// independently-constructed `AppState`s.
#[tokio::test]
async fn ac3_policy_change_takes_effect_within_the_cache_ttl_bound_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let (app, state) = app_with_config(db, &cfg).await;

    let admin_token = idp.bearer("ac3-admin", Some("ac3-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let principal_token = idp.bearer("ac3-principal", Some("ac3-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    // Before PutPolicy: nothing permits P to ListOrganizations at Root.
    let (status, before) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["allowed"], false, "{before}");
    assert_eq!(before["determining_policies"], json!([DEFAULT_DENY_MARKER]));

    // PutPolicy: a broad static permit for ListOrganizations, authored by the seeded
    // platform_admin (Action::PutPolicy is Root-only, per `PolicyService::put`).
    let policy_body = json!({
        "policy_id": "ac3-ttl-policy",
        "kind": "static",
        "source": r#"permit(principal, action == Pgs::Iam::Action::"ListOrganizations", resource);"#,
        "description": "AC3 short-TTL test policy",
    });
    let (status, put) = send(&app, "POST", "/v1/authz/policies", Some(policy_body), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{put}");

    // AC3: the very next decision, immediately after PutPolicy — trivially within the
    // `policy_cache_ttl_secs = 1` bound — reflects the new policy.
    let (status, after) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["allowed"], true, "{after}");
    assert_eq!(after["determining_policies"], json!(["ac3-ttl-policy"]));
}

// --- Redis cross-replica + fail-open (spec D11/D12) ------------------------------------------

/// Two independently-constructed `AppState`s (A and B), sharing the SAME Postgres AND the
/// SAME Redis `authz.cache` backend: a grant made through A's API is visible to B's very next
/// decision. This is NOT the AC1/AC3 synchronous-reload mechanism operating twice by
/// coincidence — A and B are genuinely separate `CedarAuthorizer`/`PolicySnapshot`/
/// `Generations::Redis` instances (separate `RedisHandle`s, even), so the only way B can
/// observe A's grant is the shared Redis `policy_gen` counter (`INCR`/`GET` on a well-known
/// key, `src/adapters/authz/generation.rs`) — the actual cross-replica premise `authz.
/// policy_cache_ttl_secs`/`refresh_interval_secs` exist to bound.
#[tokio::test]
async fn redis_cross_replica_grant_via_one_appstate_is_visible_to_another_sharing_redis() {
    let Some((_pg_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((_redis_node, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;

    let mut cfg = support::test_config(&idp);
    cfg.authz.cache = AuthzCacheConfig {
        backend: AuthzCacheBackend::Redis,
        redis_url: Some(redis_url.into()),
    };

    let (app_a, state_a) = app_with_config(db.clone(), &cfg).await;
    let (app_b, _state_b) = app_with_config(db, &cfg).await;

    let admin_token = idp.bearer("redis-xr-admin", Some("redis-xr-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state_a, &admin_token).await;

    let principal_token = idp.bearer("redis-xr-principal", Some("redis-xr-principal@example.com"), "paigasus", 3600);
    // Provisioned through A; shared Postgres means B's bearer middleware resolves the same
    // principal row without JIT-provisioning it a second time.
    let principal_prn = provision(&state_a, &principal_token).await;

    let (status, org_body) = send(
        &app_a,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "redis-xr-org", "name": "Redis XR Org"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();

    // Before the grant: B (a wholly separate CedarAuthorizer/PolicySnapshot instance) denies
    // P too — sanity, proving B independently evaluates rather than trivially agreeing.
    let (status, before) = send(
        &app_b,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "GetOrganization", "resource_prn": org_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["allowed"], false, "{before}");

    // Grant, made through replica A only.
    let (status, granted) = send(
        &app_a,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": principal_prn, "role_key": "org_admin", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    let grant_id = granted["id"].as_str().expect("id").to_string();

    // Cross-replica visibility: B's very next decision (no sleep — `reload_if_stale` reads
    // Redis's shared `policy_gen` synchronously before every decision) reflects A's grant.
    let (status, after) = send(
        &app_b,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "GetOrganization", "resource_prn": org_prn})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["allowed"], true, "{after}");
    assert_eq!(after["determining_policies"], json!([format!("grant:{grant_id}")]));
}

/// D11/D12's fail-open contract, end-to-end: with `authz.cache.backend = redis`, stopping the
/// Redis container mid-flight must never fail (or hang) an `is-authorized` request.
/// `PolicySnapshot::reload_if_stale`'s `policy_gen` read fails and is logged+swallowed
/// (`cedar_authorizer.rs` step 1 — never propagated); `GenerationsReader::entity_gen()` also
/// fails, so the decision cache is bypassed entirely rather than consulted under a
/// partial/guessed key; and `SliceCache` falls through to its inner Postgres-backed loader on
/// a Redis miss (`tests/authz_cache_redis.rs` covers each of these in isolation at the
/// component level — this asserts the composed, end-to-end HTTP behavior). The request still
/// gets a real, correctly-evaluated decision: default-deny for the still-ungranted principal.
#[tokio::test]
async fn redis_cache_backend_fails_open_when_redis_becomes_unavailable_mid_flight() {
    let Some((_pg_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((redis_node, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;

    let mut cfg = support::test_config(&idp);
    cfg.authz.cache = AuthzCacheConfig {
        backend: AuthzCacheBackend::Redis,
        redis_url: Some(redis_url.into()),
    };
    let (app, state) = app_with_config(db, &cfg).await;

    let principal_token = idp.bearer("redis-failopen-principal", Some("redis-failopen-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    // While Redis is up: a normal decision (sanity — not the point of this test, but proves
    // the redis-backed wiring itself works before we pull it out from under the request).
    let (status, up) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{up}");
    assert_eq!(up["allowed"], false, "{up}");

    redis_node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    // With Redis gone: still a clean 200 with a correctly-evaluated (default-deny) decision —
    // never an error, never a hang.
    let (status, down) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": principal_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
        Some(principal_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{down}: a Redis outage must never fail the request (D11/D12 fail-open)");
    assert_eq!(down["allowed"], false, "{down}");
    assert_eq!(down["determining_policies"], json!([DEFAULT_DENY_MARKER]));
}

// --- SMA-470: revocation during a Redis outage ------------------------------------------------

/// The case SMA-470 was filed for, and the gap in everything above: the fail-open tests all
/// cover a READ taken while Redis is down, never an INVALIDATION issued while it is.
///
/// A role grant is made and proven to take effect. Redis then goes away, so (a) the
/// post-commit `policy_gen` bump `RoleService::revoke` issues is SWALLOWED
/// (`GenerationsPolicyGenBumper::bump` logs and returns — the revoke must still succeed, D1
/// fail-open) and (b) every request-driven `PolicySnapshot::reload_if_stale` errors on its own
/// `policy_gen` read, and once the first backstop load carries a PROVISIONAL stamp forward,
/// request-driven reloads are suppressed outright. The revoke itself still commits to
/// Postgres. So the ONLY mechanism that can flip the decision back to DENY is the snapshot's
/// unconditional TTL backstop (`spawn_reload`'s `ttl_elapsed` branch) recompiling from
/// Postgres and INSTALLING the result at an unchanged generation.
///
/// That last word is the whole defect (D-B): `install_if_fresher` used to order installs on
/// `compiled.r#gen`, so a same-generation recompile — which is every recompile during an
/// outage, since the counter cannot be read, let alone advance — was rejected forever. The
/// revoked grant stayed ALLOWed for as long as Redis was down. Installs are now ordered on
/// `load_seq`, a process-local per-load counter, so the backstop converges regardless of what
/// the generation counter does.
///
/// Driven through the REAL `spawn_reload` loop at the CONFIGURED
/// `authz.policy_cache_ttl_secs`/`refresh_interval_secs` (1s/1s, wired exactly as `main.rs`
/// wires them), not a hand-rolled fast interval, so the mechanism under test is the shipped one
/// rather than a test-only fast path. What this asserts is CONVERGENCE, not a numeric bound: the
/// install budget below is deliberately far wider than `ttl + poll` — but NOT because the outage
/// stretches the loop's cadence. SMA-473 capped the reconnect retry budget at one retry, so a
/// failed `policy_gen` read costs ~100-200ms rather than whole retry cycles, and the RUNBOOK's
/// "Revocation freshness is TTL-bounded" bound now holds during an outage too. The budget is wide
/// purely as a failure DEADLINE against a slow CI runner; pinning the real bound here would only
/// buy flakiness, and the bound itself is a documented property, not this test's claim. The
/// acceptance harness never calls `IamConfig::validate`, so honouring its bounds (both non-zero,
/// refresh <= ttl) is this test's own responsibility.
#[tokio::test]
async fn sma470_revoke_during_a_redis_outage_denies_once_the_snapshot_ttl_backstop_reloads() {
    let Some((_pg_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((redis_node, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;

    let mut cfg = support::test_config(&idp);
    cfg.authz.cache = AuthzCacheConfig {
        backend: AuthzCacheBackend::Redis,
        redis_url: Some(redis_url.into()),
    };
    cfg.authz.policy_cache_ttl_secs = 1;
    cfg.authz.refresh_interval_secs = 1;

    let (app, state) = app_with_config(db, &cfg).await;

    let admin_token = idp.bearer("sma470-admin", Some("sma470-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let principal_token = idp.bearer("sma470-principal", Some("sma470-principal@example.com"), "paigasus", 3600);
    let principal_prn = provision(&state, &principal_token).await;

    let (status, org_body) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "sma470-org", "name": "SMA470 Org"})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();

    // Every `is-authorized` below is P asking about P (a SELF-query), so `decide_gated`'s
    // self/admin exposure rule never redacts the answer.
    let decision_body = json!({"principal_prn": principal_prn, "action": "GetOrganization", "resource_prn": org_prn});

    // Before the grant: default-deny — so the ALLOW below is genuinely the grant's doing, not a
    // pre-existing permission that a revoke could never have taken away.
    let (status, before) = send(&app, "POST", "/v1/authz/is-authorized", Some(decision_body.clone()), Some(principal_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["allowed"], false, "{before}");

    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": principal_prn, "role_key": "org_admin", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    let grant_id = granted["id"].as_str().expect("id").to_string();

    // The grant takes effect while Redis is up (AC1's synchronous `reload_if_stale`), which is
    // also what leaves the snapshot holding an AUTHORITATIVE generation stamp going into the
    // outage — the starting state the defect needs.
    let (status, allowed) = send(&app, "POST", "/v1/authz/is-authorized", Some(decision_body.clone()), Some(principal_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    assert_eq!(allowed["allowed"], true, "the grant must take effect before the outage: {allowed}");
    assert_eq!(allowed["determining_policies"], json!([format!("grant:{grant_id}")]), "{allowed}");

    // The compiled set the outage starts from. `content_hash` is a pure function of the
    // documents + grants compiled (SMA-470 D4), so "the backstop installed a recompile that
    // saw the revoke" is exactly "this string changed" — with no dependency on `r#gen`, the
    // counter the outage makes unreadable.
    let hash_before_revoke = state.snapshot().current().await.content_hash.clone();

    // Redis goes away. From here `policy_gen`/`entity_gen` both error: the decision cache is
    // bypassed rather than consulted under a partial key, and the revoke's bump is swallowed.
    redis_node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let (status, revoked) = send(&app, "DELETE", &format!("/v1/authz/role-grants/{grant_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "a revoke must still commit with Redis down (fail-open): {revoked}");

    // Only the TTL backstop can recover the decision now. Spawn the real loop with the real
    // config values, exactly as `main.rs` wires them.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let reload = state
        .snapshot()
        .spawn_reload(Duration::from_secs(cfg.authz.policy_cache_ttl_secs), Duration::from_secs(cfg.authz.refresh_interval_secs), async move {
            let _ = shutdown_rx.await;
        });

    // Wait for the backstop to INSTALL a recompile — the single step D-B made impossible. The
    // wait watches the in-process snapshot rather than polling `/v1/authz/is-authorized`,
    // because the snapshot's `content_hash` IS the property under test ("a recompile that saw
    // the revoke was installed"), while an HTTP probe only lets it be inferred from a decision
    // — and the decision itself is still asserted over the real HTTP surface, once, below.
    // With Redis gone each probe would also pay `ConnectionManager`'s reconnect-retry budget
    // on every counter read a decision takes; that is now bounded (SMA-473 caps it at ONE
    // retry, ~100-200ms per failed read, ~0.2-0.6s per decision — the same cap that took
    // `adapters::api_keys::cache`'s unreachable-backend test from 28.4s to 0.47s), so it is no
    // longer the reason for watching in-process, merely no longer an argument against it.
    //
    // 90s stays deliberately, even though the honest expectation is now `ttl + poll` — ~2s with
    // this test's `policy_cache_ttl_secs = 1` / `refresh_interval_secs = 1` — plus the reload's
    // own duration, rather than that plus tens of seconds of retry cycles. The budget is a
    // failure DEADLINE against a slow runner (testcontainers, the Postgres `list_all`s and the
    // Cedar compile the backstop pays before installing), NOT an assertion of the `ttl + poll`
    // bound: it only has to be wide enough that a failure means the backstop never converges at
    // all, so a regression fails on the mechanism, never on a slow CI runner. The loop below
    // breaks the moment the recompile is observed, so the extra headroom costs nothing on the
    // happy path.
    let install_budget = Duration::from_secs(90);
    let started = std::time::Instant::now();
    let mut installed = false;
    while started.elapsed() < install_budget {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if state.snapshot().current().await.content_hash != hash_before_revoke {
            installed = true;
            break;
        }
    }
    let install_took = started.elapsed();

    // The property that actually matters, over the real API: the revoked grant no longer
    // ALLOWs, and the request still succeeds despite Redis being gone.
    let (status, decision) = send(&app, "POST", "/v1/authz/is-authorized", Some(decision_body), Some(principal_token.as_str())).await;

    // Own the loop's lifetime: real shutdown signal, joined before this test returns, so it can
    // never outlive the testcontainers below. Asserted only afterwards — a panic before the
    // join would abandon the task over a torn-down Postgres.
    let _ = shutdown_tx.send(());
    reload.await.expect("the reload loop exits cleanly");

    assert!(
        installed,
        "the policy snapshot never converged: no recompile installed within the {install_budget:?} liveness budget \
         (ttl = {}s, poll = {}s — the budget is a generous convergence check, not the documented `ttl + poll` bound). \
         A revoke committed during a Redis outage can only be picked up by the TTL backstop, which must install its \
         recompile even though `policy_gen` never moved (SMA-470 D-B) — last decision: {decision}",
        cfg.authz.policy_cache_ttl_secs, cfg.authz.refresh_interval_secs
    );
    assert_eq!(status, StatusCode::OK, "a Redis outage must never fail the request (D11/D12 fail-open): {decision}");
    assert_eq!(
        decision["allowed"], false,
        "a grant revoked during a Redis outage must stop ALLOWing (backstop installed after {install_took:?}): {decision}"
    );
    assert_eq!(
        decision["determining_policies"],
        json!([DEFAULT_DENY_MARKER]),
        "the revoked grant must be gone from the compiled set entirely: {decision}"
    );
}
