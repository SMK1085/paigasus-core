// SPDX-License-Identifier: Apache-2.0

//! `AuthenticateApiKey` end-to-end coverage against real Postgres (SMA-445 Task 18): the
//! cache-first `resolve` hot path (`application::authenticate_api_key::AuthenticateApiKey`)
//! driven through the REAL `PgApiKeyRepository`/`PgPrincipalRepository`/`PgServiceAccountRepository`
//! adapters and a REAL `HmacSecretHasher` — not the in-memory fakes the unit-test suite in
//! `authenticate_api_key.rs` itself uses.
//!
//! `disabled_sa_denies_live_key` is the carry-forward, security-critical case (Task 17's
//! review): a still-Active key belonging to a `Disabled` service account must be denied on
//! EVERY resolve, not just at issuance — proving the unconditional D16 status check.
//!
//! **SMA-445 Task 22 (end-to-end acceptance + security regressions, the M4 capstone):** three
//! more tests at the bottom of this file drive the WHOLE stack — real Postgres, the real
//! `router(AppState::new(..))`, Cedar authorization (M3) — through `support::app_with_state`,
//! mirroring `tests/authz_acceptance.rs`'s harness (seed a `platform_admin`, create orgs via
//! the real API, grant roles via the real API). `sa_acts_authorized_by_policy` is the issue's
//! headline AC-2 ("a service account can act, authorized by policy"); `issuance_escalation_denied`
//! is D15's end-to-end mirror of `application/api_keys.rs`'s own unit test
//! (`issue_denied_when_actor_cannot_grant_all_sa_roles`), now through the HTTP surface;
//! `cached_key_denied_after_archive` is D16 + cache-eviction end-to-end, mirroring
//! `application/service_accounts.rs`'s `archive_disables_and_evicts_keys` unit test but through
//! a real authenticate-once-then-archive-then-reauthenticate HTTP round trip.

mod support;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{Duration, Utc};
use paigasus_iam::adapters::api_keys::{ApiKeyValidationCache, HmacSecretHasher, MemoryApiKeyCache, OsRngKeyEntropy, Pepper};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgApiKeyRepository, PgMembershipRepository, PgPrincipalRepository, PgServiceAccountRepository};
use paigasus_iam::application::authenticate_api_key::AuthenticateApiKey;
use paigasus_iam::config::ApiKeyConfig;
use paigasus_iam_core::{
    ApiKey, ApiKeyRepository, ApiKeyStatus, AuthnError, Clock, IdGenerator, KeyEntropy, PrincipalStatus, SecretHasher, ServiceAccountRepository, TenancyNodeRef, display_prefix, format_token,
};
use sea_orm::DatabaseConnection;
use serde_json::json;
use std::sync::Arc;

const PREFIX: &str = "pgs_sk_";

/// A >=32-byte pepper, base64-encoded — mirrors `hasher.rs`'s own test helper.
fn test_pepper_b64() -> String {
    STANDARD.encode([0x5au8; 32])
}

fn hasher() -> HmacSecretHasher {
    HmacSecretHasher::new(Pepper::from_config(&test_pepper_b64()).unwrap())
}

/// Issues a REAL API key row (via `PgApiKeyRepository`, hashed with the SAME `HmacSecretHasher`
/// the returned service will verify against) for `sa`, scoped to `scope`. Returns the ONE-TIME
/// plaintext token plus the persisted `ApiKey`.
async fn issue_key(db: &DatabaseConnection, hasher: &HmacSecretHasher, sa: &paigasus_iam_core::PrincipalId, scope: TenancyNodeRef, expires_at: Option<chrono::DateTime<Utc>>) -> (String, ApiKey) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let id = ids.new_api_key_id();
    let secret = OsRngKeyEntropy.new_secret();
    let hash = hasher.hash(&secret);
    let key = ApiKey {
        id,
        service_account_id: sa.clone(),
        scope,
        prefix: display_prefix(PREFIX, id),
        status: ApiKeyStatus::Active,
        expires_at,
        last_used_at: None,
        created_at: clock.now(),
        revoked_at: None,
        scope_actions: Vec::new(),
        scope_roles: Vec::new(),
    };
    PgApiKeyRepository::new(db.clone()).issue(&key, &hash).await.unwrap();
    let plaintext = format_token(PREFIX, id, &secret);
    (plaintext, key)
}

#[allow(clippy::type_complexity)]
fn new_service(
    db: &DatabaseConnection,
    hasher: HmacSecretHasher,
    cache: Arc<dyn ApiKeyValidationCache>,
) -> AuthenticateApiKey<PgApiKeyRepository, PgPrincipalRepository, HmacSecretHasher, SystemClock, PgMembershipRepository> {
    AuthenticateApiKey::new(
        PgApiKeyRepository::new(db.clone()),
        PgPrincipalRepository::new(db.clone()),
        hasher,
        SystemClock,
        cache,
        PgMembershipRepository::new(db.clone()),
        ApiKeyConfig::default(),
    )
}

/// AC — a freshly issued, Active key resolves to an `AuthnPrincipal` naming the ServiceAccount
/// it was issued for.
#[tokio::test]
async fn valid_key_resolves_to_sa_principal() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (plaintext, key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    let svc = new_service(&db, hasher(), Arc::new(MemoryApiKeyCache::new(30)));

    let resolved = svc.resolve(&plaintext).await.unwrap();
    assert_eq!(resolved.principal_id, sa.principal_id);
    assert_eq!(resolved.kind, paigasus_iam_core::PrincipalKind::ServiceAccount);
    assert_eq!(resolved.status, PrincipalStatus::Active);
    match resolved.credential {
        paigasus_iam_core::Credential::ApiKey { key_id, expires_at: None, scope_prn } => {
            assert_eq!(key_id, key.id);
            // SMA-446: the resolved credential carries the key's tenancy scope PRN — exactly the
            // scope the key was issued for (`owner`, == `key.scope`).
            assert_eq!(scope_prn, key.scope.canonical(), "resolved credential must carry the key's scope_prn");
        }
        other => panic!("expected an ApiKey credential with no expiry, got {other:?}"),
    }
}

/// AC — a revoked key is denied, `InvalidToken`.
#[tokio::test]
async fn revoked_key_denied() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (plaintext, key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    PgApiKeyRepository::new(db.clone()).revoke(key.id, SystemClock.now()).await.unwrap();
    let svc = new_service(&db, hasher(), Arc::new(MemoryApiKeyCache::new(30)));

    let err = svc.resolve(&plaintext).await.unwrap_err();
    assert!(matches!(err, AuthnError::InvalidToken(_)));
}

/// AC — a key issued with an already-past `expires_at` is denied, `InvalidToken`.
#[tokio::test]
async fn expired_key_denied() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let past = SystemClock.now() - Duration::seconds(60);
    let (plaintext, _key) = issue_key(&db, &hasher(), &sa.principal_id, owner, Some(past)).await;
    let svc = new_service(&db, hasher(), Arc::new(MemoryApiKeyCache::new(30)));

    let err = svc.resolve(&plaintext).await.unwrap_err();
    assert!(matches!(err, AuthnError::InvalidToken(_)));
}

/// AC — a token whose secret doesn't verify against the stored HMAC hash is denied,
/// `InvalidToken`.
#[tokio::test]
async fn wrong_secret_denied() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (_plaintext, key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    let tampered = format_token(PREFIX, key.id, &[0xABu8; 32]);
    let svc = new_service(&db, hasher(), Arc::new(MemoryApiKeyCache::new(30)));

    let err = svc.resolve(&tampered).await.unwrap_err();
    assert!(matches!(err, AuthnError::InvalidToken(_)));
}

/// THE carry-forward AC (Task 17's review, D16) — a still-Active key whose service account has
/// since been disabled (`set_principal_status(Disabled)`) must be denied `PrincipalInactive` on
/// resolve: this is the SOLE mechanism that stops a disabled SA's key from authenticating.
#[tokio::test]
async fn disabled_sa_denies_live_key() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (plaintext, _key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    sar.set_principal_status(&sa.principal_id, PrincipalStatus::Disabled).await.unwrap();
    let svc = new_service(&db, hasher(), Arc::new(MemoryApiKeyCache::new(30)));

    let err = svc.resolve(&plaintext).await.unwrap_err();
    assert!(matches!(err, AuthnError::PrincipalInactive), "expected PrincipalInactive, got {err:?}");
}

/// AC — a cache hit skips the DB entirely, end-to-end with real infra: resolve once (a miss,
/// which `cache.put`s), then revoke the key DIRECTLY via the repository (bypassing
/// `ApiKeyService::revoke`'s own `cache.evict` step — the only thing that would normally keep
/// the cache honest), then resolve again. The second resolve still succeeds because it never
/// touches the now-revoked Postgres row — proving the cache-first hot path is real, not just
/// unit-tested against fakes.
#[tokio::test]
async fn cache_hit_still_authenticates_a_since_revoked_key_within_ttl() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (plaintext, key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    let cache = Arc::new(MemoryApiKeyCache::new(30));
    let svc = new_service(&db, hasher(), cache.clone());

    let first = svc.resolve(&plaintext).await.unwrap();
    assert_eq!(first.principal_id, sa.principal_id);
    assert!(cache.get(key.id).await.is_some(), "sanity: the first resolve must have populated the cache");

    PgApiKeyRepository::new(db.clone()).revoke(key.id, SystemClock.now()).await.unwrap();

    let second = svc.resolve(&plaintext).await.unwrap();
    assert_eq!(second.principal_id, sa.principal_id, "a cache hit must authenticate without re-reading the now-revoked Postgres row");
}

/// THE regression AC for the cache-hit auth-bypass fix (SMA-445): resolve a valid token once to
/// populate the cache, then present a token with the SAME `key_id` but a DIFFERENT secret. Even
/// though the `key_id` is cached as a positive validation, the secret is the credential — the
/// forged token MUST be denied `InvalidToken`, NOT authenticated. Before the fix the cache-hit
/// path skipped HMAC verification entirely, so any secret paired with a cached `key_id` would
/// have succeeded for up to the cache TTL.
#[tokio::test]
async fn cache_hit_with_wrong_secret_is_denied() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let (plaintext, key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;
    let cache = Arc::new(MemoryApiKeyCache::new(30));
    let svc = new_service(&db, hasher(), cache.clone());

    // First resolve: a miss that populates the positive cache entry (with the stored hash).
    svc.resolve(&plaintext).await.unwrap();
    assert!(cache.get(key.id).await.is_some(), "sanity: the first resolve must have populated the cache");

    // Same key_id, a forged secret — must be denied on the cache hit, not authenticated.
    let forged = format_token(PREFIX, key.id, &[0xABu8; 32]);
    let err = svc.resolve(&forged).await.unwrap_err();
    assert!(matches!(err, AuthnError::InvalidToken(_)), "a wrong secret against a cached key_id must be denied, got {err:?}");

    // The forged attempt must not have knocked the valid entry out of the cache (no DoS lever).
    assert!(cache.get(key.id).await.is_some(), "a wrong-secret guess must not evict the valid cached entry");
}

// --- SMA-445 Task 19: credential router at both enforcement seams -------------------------
//
// The tests above drive `AuthenticateApiKey` directly (application layer, bypassing HTTP
// entirely). The three below instead drive the REAL, fully wired `router(AppState::new(db,
// &cfg))` — `support::app_with_state` — proving the `require_bearer` middleware's
// credential-router branch (SMA-445 Task 19) actually dispatches a `pgs_sk_`-prefixed bearer
// to `state.api_key_auth` and everything else to `state.authn`, exactly like the sibling gRPC
// `AuthEnforce::call` branch (unit-untestable here; covered by `tests/grpc_authn.rs` staying
// green, module docs on `adapters::grpc::authn`). `support::test_config`'s `api_keys.pepper`
// is the SAME fixed test pepper `hasher()` above decodes (`[0x5A; 32]`), so a key issued via
// `issue_key`/`hasher()` verifies against the router's own wired `HmacSecretHasher`.

/// AC (Task 19) — a valid API-key bearer authenticates through the credential router: the
/// freshly created SA holds no role grants, so `GET /v1/organizations` denies with 403
/// (`CedarAuthorizer`, no matching grant) rather than the 200 an authorized caller would get —
/// but critically NOT 401, the one status only a failed AUTHENTICATION (not authorization)
/// step can produce. Either a 403 or a 200 proves the key routed to `api_key_auth.resolve` and
/// authenticated as the SA principal; only 401 would mean the router never dispatched it.
#[tokio::test]
async fn api_key_bearer_authenticates_via_the_router() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, _state, _idp) = support::app_with_state(db.clone()).await;

    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();
    let (plaintext, _key) = issue_key(&db, &hasher(), &sa.principal_id, owner, None).await;

    let (status, body) = support::send(&app, "GET", "/v1/organizations", None, Some(&plaintext)).await;
    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "the api-key bearer must authenticate (a 403 from CedarAuthorizer denying a grant-less SA is fine, 401 would mean the router failed to route it at all): {body}"
    );
}

/// AC (Task 19) — a bearer carrying the configured `pgs_sk_` prefix but structurally garbage
/// content (no valid keyid/secret shape) is denied 401 `invalid-token` through the SAME funnel
/// every other rejected credential renders through (`AuthnApiError`) — and the response body
/// never echoes the presented (bogus) token material.
#[tokio::test]
async fn garbage_api_key_bearer_is_401() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, ..) = support::app_with_state(db).await;

    let garbage = format!("{PREFIX}deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
    let (status, body) = support::send(&app, "GET", "/v1/organizations", None, Some(&garbage)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token");
    assert_eq!(body["error"]["message"], "invalid bearer token");
    assert!(!body.to_string().contains("deadbeef"), "the presented garbage token must never be echoed in the response: {body}");
}

/// AC (Task 19) — adding the credential router must not break the pre-existing OIDC path: a
/// non-`pgs_sk_`-prefixed bearer still resolves through `state.authn` (JIT-provisioning), the
/// SAME as before this task. Mirrors `api_key_bearer_authenticates_via_the_router`'s "not 401
/// proves authentication succeeded" posture — the fresh principal has no role grants either,
/// so `GET /v1/organizations` still denies 403, never 401.
#[tokio::test]
async fn jwt_bearer_still_authenticates() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, _state, idp) = support::app_with_state(db).await;

    let token = idp.bearer("sub-alice", Some("alice@example.com"), "paigasus", 3600);
    let (status, body) = support::send(&app, "GET", "/v1/organizations", None, Some(&token)).await;
    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "the OIDC bearer must still authenticate via AuthnSvc after the credential router was added: {body}"
    );
}

// --- SMA-445 Task 22: full-stack acceptance + security regressions (the M4 capstone) --------
//
// Real Postgres, real `router(AppState::new(..))`, real Cedar authorization -- these three
// tests seed a platform_admin (`support::provision_platform_admin`), create tenancy nodes and
// grant roles through the actual `/v1/organizations`/`/v1/authz/role-grants` API (never a
// bypassed store), exactly like `tests/authz_acceptance.rs`'s harness.

/// AC-2, THE headline acceptance criterion of the whole M4 issue: "a service account can act,
/// authorized by policy (M3)." A freshly issued SA holds an `org_member` grant at its owning
/// org -- a READ-ONLY role (`ORG_MEMBER_ACTIONS`, `authz/roles.rs`: `GetOrganization`,
/// `GetTeam`, `ListTeams`, `GetProject`, `ListProjects` -- no `CreateTeam`, a write). Presenting
/// the SA's issued key as a `Bearer` on `GET /v1/organizations/{id}` (an action the grant
/// PERMITS) is allowed; the SAME key on `POST /v1/organizations/{id}/teams` (`CreateTeam`, an
/// action the grant does NOT permit) is denied 403 -- proving the key authenticates through the
/// credential router (Task 19) straight to the SA's own Cedar-evaluated grant set (M3), not
/// blanket access.
#[tokio::test]
async fn sa_acts_authorized_by_policy() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, state, idp) = support::app_with_state(db).await;

    let admin_token = idp.bearer("t22-ac2-admin", Some("t22-ac2-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, org_body) = support::send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "t22-ac2-org", "name": "T22 AC2 Org"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let org_id = org_prn.rsplit('/').next().expect("org prn has a trailing id segment").to_string();

    let (status, sa_body) = support::send(&app, "POST", "/v1/service-accounts", Some(json!({"owner_prn": org_prn, "name": "ac2-bot"})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{sa_body}");
    let sa_prn = sa_body["prn"].as_str().expect("prn").to_string();
    let sa_id = sa_prn.rsplit('/').next().expect("sa prn has a trailing id segment").to_string();

    // Grant the SA `org_member` (read-only) at the org via the real `/v1/authz/role-grants` API,
    // `principal_prn` = the SA's own principal PRN.
    let (status, granted) = support::send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": sa_prn, "role_key": "org_member", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");

    let (status, issued) = support::send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({"scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let plaintext = issued["token"].as_str().expect("token").to_string();

    // Allowed: `org_member` permits `GetOrganization`.
    let (status, get_body) = support::send(&app, "GET", &format!("/v1/organizations/{org_id}"), None, Some(plaintext.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{get_body}");
    assert_eq!(get_body["prn"], org_prn);

    // Denied: `org_member` does NOT permit `CreateTeam` (a write, excluded from
    // `ORG_MEMBER_ACTIONS`) -- 403, never 401 (the key itself authenticated fine).
    let (status, team_body) = support::send(
        &app,
        "POST",
        &format!("/v1/organizations/{org_id}/teams"),
        Some(json!({"slug": "ac2-denied-team", "name": "AC2 Denied Team"})),
        Some(plaintext.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{team_body}");
    assert_eq!(team_body["error"]["code"], "forbidden");
}

/// D15, end-to-end: the HTTP-surface mirror of `application/api_keys.rs`'s own unit test
/// (`issue_denied_when_actor_cannot_grant_all_sa_roles`). Setup: the SA (owned by org O) holds
/// an `org_admin` grant at a SEPARATE org X, made by the seeded `platform_admin`. A second
/// actor is granted `org_admin` at O ONLY -- which carries `IssueApiKey`@O (so the ordinary
/// "may this actor manage this SA's keys at all" check passes) but NOT `GrantRole`@org_x (org X
/// is a wholly unrelated tenancy subtree, never "in" O). `ApiKeyService::issue`'s D15 check
/// walks EVERY grant the target SA holds and requires the actor to dominate each one -- the
/// actor's own real API call to issue a key for this SA must be denied 403, not just the
/// application-layer unit test.
#[tokio::test]
async fn issuance_escalation_denied() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, state, idp) = support::app_with_state(db).await;

    let admin_token = idp.bearer("t22-d15-admin", Some("t22-d15-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    // Org O -- the SA's own owner.
    let (status, owner_body) = support::send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "t22-d15-owner-org", "name": "T22 D15 Owner Org"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owner_body}");
    let owner_prn = owner_body["organization"]["prn"].as_str().expect("organization.prn").to_string();

    // Org X -- a wholly separate tenancy subtree the SA will hold a grant in.
    let (status, org_x_body) = support::send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "t22-d15-org-x", "name": "T22 D15 Org X"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_x_body}");
    let org_x_prn = org_x_body["organization"]["prn"].as_str().expect("organization.prn").to_string();

    // The SA, owned by O.
    let (status, sa_body) = support::send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({"owner_prn": owner_prn, "name": "escalation-bot"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sa_body}");
    let sa_prn = sa_body["prn"].as_str().expect("prn").to_string();
    let sa_id = sa_prn.rsplit('/').next().expect("sa prn has a trailing id segment").to_string();

    // The SA holds an `org_admin` grant at org X, made by the platform_admin.
    let (status, sa_grant) = support::send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": sa_prn, "role_key": "org_admin", "scope_prn": org_x_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sa_grant}");

    // A second actor: `org_admin` at O ONLY -- `IssueApiKey`@O, but no authority whatsoever at
    // org X.
    let actor_token = idp.bearer("t22-d15-actor", Some("t22-d15-actor@example.com"), "paigasus", 3600);
    let actor_prn = support::provision(&state, &actor_token).await;
    let (status, actor_grant) = support::send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": actor_prn, "role_key": "org_admin", "scope_prn": owner_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{actor_grant}");

    // The actor attempts to issue a key for the SA -- `IssueApiKey`@O passes, but the SA's
    // `org_admin`@org_x grant is one the actor cannot dominate (`GrantRole`@org_x denies) -- the
    // WHOLE issuance is denied, 403, before anything is minted.
    let (status, issue_body) = support::send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({"scope_prn": owner_prn})),
        Some(actor_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{issue_body}");
    assert_eq!(issue_body["error"]["code"], "forbidden");
}

/// D16 + cache eviction, end-to-end: the HTTP-surface mirror of `application/service_accounts.rs`'s
/// `archive_disables_and_evicts_keys` unit test, but through a real authenticate-then-archive-
/// then-reauthenticate HTTP round trip. Issues a key for an SA holding an `org_member` grant (so
/// the FIRST authenticated call is an unambiguous `200`, proving the key authenticated through
/// the credential router AND populated `AppState.api_key_auth`'s shared validation cache, Task
/// 19). Archives the SA via the real `DELETE /v1/service-accounts/{sa}` API -- which evicts
/// every one of the SA's cached validations (`ServiceAccountService::archive`'s security-
/// critical step) AND disables the underlying `Principal` (D16). Presenting the SAME key again
/// is now a genuine cache MISS (the entry was evicted, not merely stale) that reads Postgres
/// fresh and hits the unconditional D16 SA-status check in `AuthenticateApiKey::resolve_uncached`
/// -- denied `403 principal-inactive`, a status/code distinct from an ordinary authorization
/// `403 forbidden`, proving the deny happens at AUTHENTICATION (the credential router), not
/// merely a subsequent authorization check.
#[tokio::test]
async fn cached_key_denied_after_archive() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let (app, state, idp) = support::app_with_state(db).await;

    let admin_token = idp.bearer("t22-cache-evict-admin", Some("t22-cache-evict-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (status, org_body) = support::send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "t22-cache-evict-org", "name": "T22 Cache Evict Org"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().expect("organization.prn").to_string();
    let org_id = org_prn.rsplit('/').next().expect("org prn has a trailing id segment").to_string();

    let (status, sa_body) = support::send(
        &app,
        "POST",
        "/v1/service-accounts",
        Some(json!({"owner_prn": org_prn, "name": "cache-evict-bot"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sa_body}");
    let sa_prn = sa_body["prn"].as_str().expect("prn").to_string();
    let sa_id = sa_prn.rsplit('/').next().expect("sa prn has a trailing id segment").to_string();

    // A read-only grant so the first authenticated call below is a clean, unambiguous 200.
    let (status, granted) = support::send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": sa_prn, "role_key": "org_member", "scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");

    let (status, issued) = support::send(
        &app,
        "POST",
        &format!("/v1/service-accounts/{sa_id}/api-keys"),
        Some(json!({"scope_prn": org_prn})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let plaintext = issued["token"].as_str().expect("token").to_string();

    // Authenticate once through the real transport -- the credential router resolves the key
    // via `state.api_key_auth`, populating its shared validation cache.
    let (status, first) = support::send(&app, "GET", &format!("/v1/organizations/{org_id}"), None, Some(plaintext.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{first}");

    // Archive the SA via the real API.
    let (status, archived) = support::send(&app, "DELETE", &format!("/v1/service-accounts/{sa_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{archived}");

    // The SAME key, presented again: denied `principal-inactive`, never the stale-cache 200 the
    // archive's cache-eviction step exists to prevent.
    let (status, second) = support::send(&app, "GET", &format!("/v1/organizations/{org_id}"), None, Some(plaintext.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{second}");
    assert_eq!(second["error"]["code"], "principal-inactive", "{second}");
}
