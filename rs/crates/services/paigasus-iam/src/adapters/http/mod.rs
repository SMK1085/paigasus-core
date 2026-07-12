// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness), `/readyz` (DB-backed readiness), the
//! `/v1` tenancy API (organizations/teams/projects/memberships/users, ADR-0014), the
//! authn introspection endpoint (`/v1/authn/introspect`, SMA-443), and the `/v1/authz`
//! authorization API (`is-authorized`/policies/role-grants, SMA-444 Task 18).

mod api_keys;
pub mod auth_middleware;
pub mod authn;
mod authz;
pub mod authz_middleware;
pub mod dto;
pub mod error;
mod memberships;
mod organizations;
mod projects;
mod service_accounts;
mod teams;
mod users;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use paigasus_iam_core::{Authenticator, AuthnError, Authorizer, Issuer, ValidatedClaims};
use redis::aio::ConnectionManager;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::adapters::api_keys::{ApiKeyValidationCache, HmacSecretHasher, MemoryApiKeyCache, OsRngKeyEntropy, RedisApiKeyCache};
use crate::adapters::authz::{CedarAuthorizer, Generations, GenerationsReader, MemoryDecisionCache, PolicySnapshot, RedisDecisionCache, SliceCache, TracingAuditSink};
use crate::adapters::clock::SystemClock;
use crate::adapters::id::KernelIdGenerator;
use crate::adapters::oidc::jwks::{HttpJwksFetcher, InMemoryJwksCache, JwksProvider};
use crate::adapters::oidc::redis_cache::RedisJwksCache;
use crate::adapters::oidc::validator::OidcAuthenticator;
use crate::adapters::persistence::{
    PgApiKeyRepository, PgAuditLog, PgEntitySliceLoader, PgExternalIdentityRepository, PgMembershipRepository, PgOrganizationRepository, PgPolicyStore, PgPrincipalRepository, PgProjectRepository,
    PgRoleGrantStore, PgServiceAccountRepository, PgTeamRepository,
};
use crate::application::api_keys::ApiKeyService;
use crate::application::audit::AuditQueryService;
use crate::application::authenticate_api_key::AuthenticateApiKey;
use crate::application::authenticate_token::{AuthenticateToken, JitPolicy};
use crate::application::authorize::Authorize;
use crate::application::bootstrap;
use crate::application::bootstrap_admin::BootstrapAdminSeeder;
use crate::application::create_user::CreateUser;
use crate::application::memberships::MembershipService;
use crate::application::organizations::OrganizationService;
use crate::application::policies::PolicyService;
use crate::application::projects::ProjectService;
use crate::application::roles::RoleService;
use crate::application::service_accounts::ServiceAccountService;
use crate::application::teams::TeamService;
use crate::config::{ApiKeyCacheBackend, AuthzCacheBackend, IamConfig, JwksCacheBackend};
use paigasus_iam_core::{ApiKeyRepository, AuditLog, AuditSink, DecisionCache, EntitySliceLoader, OrganizationRepository, PolicyStore, ProjectRepository, RoleGrantStore, TeamRepository};

pub type OrgSvc = OrganizationService<PgOrganizationRepository, KernelIdGenerator, SystemClock>;
pub type TeamSvc = TeamService<PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type ProjectSvc = ProjectService<PgProjectRepository, PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type MembershipSvc = MembershipService<PgMembershipRepository, KernelIdGenerator, SystemClock>;
pub type UserSvc = CreateUser<PgPrincipalRepository, KernelIdGenerator, SystemClock>;
/// The `RoleGrant` CRUD use case (SMA-444 Task 18), wired over the same `Arc<dyn
/// RoleGrantStore>` `AppState.role_grant_store` holds — mirrors every other `*Svc` alias's
/// `KernelIdGenerator`/`SystemClock` DI posture.
pub type RoleSvc = RoleService<KernelIdGenerator, SystemClock>;
/// The cold-start bootstrap-admin seeder (SMA-444 Task 21b), wired over the same `Arc<dyn
/// RoleGrantStore>` `AppState.role_grant_store` holds — mirrors `RoleSvc`'s DI posture.
pub type BootstrapAdminSvc = BootstrapAdminSeeder<KernelIdGenerator, SystemClock>;

/// The wired API-key bearer authenticator (SMA-445 Task 19) — consumed by BOTH enforcement
/// seams (`auth_middleware::require_bearer`, `grpc::authn::AuthEnforce::call`): a presented
/// bearer token whose prefix matches `AppState.api_key_prefix` routes here instead of to
/// `AuthnSvc::resolve`. Shares the SAME `Arc<dyn ApiKeyValidationCache>` `api_keys`'s
/// issue/revoke evicts through (module docs on `application::authenticate_api_key`).
pub type ApiKeyAuthSvc = AuthenticateApiKey<PgApiKeyRepository, PgPrincipalRepository, HmacSecretHasher, SystemClock, PgMembershipRepository>;
/// The API-key lifecycle use case (issue/revoke/list, SMA-445 Task 17) — the future
/// `/v1/service-accounts/*/api-keys` HTTP routes (Task 20) call through this.
pub type ApiKeySvc = ApiKeyService<PgApiKeyRepository, PgServiceAccountRepository, KernelIdGenerator, SystemClock, HmacSecretHasher, OsRngKeyEntropy>;
/// The service-account lifecycle use case (create/get/list/archive, SMA-445 Task 16) — the
/// future `/v1/service-accounts` HTTP routes (Task 20) call through this.
pub type ServiceAccountSvc = ServiceAccountService<PgServiceAccountRepository, KernelIdGenerator, SystemClock>;

/// The OIDC authenticator over the in-process JWKS cache (the `memory` backend, D2).
pub type Oidc = OidcAuthenticator<HttpJwksFetcher, InMemoryJwksCache, SystemClock>;
/// The OIDC authenticator over the external Redis JWKS cache (the `redis` backend, D15).
pub type OidcRedis = OidcAuthenticator<HttpJwksFetcher, RedisJwksCache, SystemClock>;

/// The one concrete `Authenticator` the composition root wires, chosen by
/// `authn.jwks_cache.backend`. The payloads are `Arc`ed because `AppState` (and therefore
/// `AuthnSvc`) must be `Clone`, while `OidcAuthenticator` deliberately is not — its JWKS
/// provider owns per-issuer single-flight/cooldown state that every `AppState` clone must
/// SHARE, not duplicate (a cloned cache would defeat both the cache and the rate limits).
#[derive(Clone)]
pub enum WiredAuthenticator {
    Memory(Arc<Oidc>),
    Redis(Arc<OidcRedis>),
}

#[async_trait]
impl Authenticator for WiredAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError> {
        match self {
            WiredAuthenticator::Memory(inner) => inner.authenticate(token).await,
            WiredAuthenticator::Redis(inner) => inner.authenticate(token).await,
        }
    }
}

/// The fully wired `AuthenticateToken` use case (Tasks 11–12 consume this via
/// `AppState.authn` for the middleware `resolve` path and the gRPC `Introspect`).
pub type AuthnSvc = AuthenticateToken<WiredAuthenticator, PgExternalIdentityRepository, PgPrincipalRepository, PgMembershipRepository, KernelIdGenerator, SystemClock>;

/// Headroom over `max_token_bytes` for the introspect JSON envelope (`{"token":"…"}` —
/// braces, quotes, key, and any insignificant whitespace): a request larger than
/// `max_token_bytes` + this can never carry a valid token, so the route body limit
/// rejects it before JSON parsing (spec H1).
const INTROSPECT_BODY_OVERHEAD_BYTES: usize = 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub orgs: OrgSvc,
    pub teams: TeamSvc,
    pub projects: ProjectSvc,
    pub memberships: MembershipSvc,
    pub users: UserSvc,
    pub authn: AuthnSvc,
    /// The `Authorizer` port's implementation (ADR-0013, SMA-444 Task 15) — `Arc`-shared
    /// across every `AppState` clone (mirroring `WiredAuthenticator`'s posture) so every
    /// HTTP/gRPC worker decides against the SAME in-process policy snapshot, decision cache,
    /// and background reload task rather than a per-clone duplicate.
    pub authz: Arc<CedarAuthorizer>,
    /// The compiled-policy snapshot `authz` itself evaluates against — kept as its own field
    /// (rather than only reachable through `authz`) so `main.rs` can spawn its background
    /// reload task ([`PolicySnapshot::spawn_reload`]) without `CedarAuthorizer` needing to
    /// expose its private internals.
    snapshot: Arc<PolicySnapshot>,
    /// Route-level body cap for `POST /v1/authn/introspect` (H1): `max_token_bytes` +
    /// [`INTROSPECT_BODY_OVERHEAD_BYTES`], computed once at wiring time.
    pub introspect_body_limit: usize,
    /// Role-grant CRUD use case (SMA-444 Task 18) — the `/v1/authz/role-grants` HTTP routes
    /// call through this, mirroring `orgs`/`teams`/etc.'s posture.
    pub roles: RoleSvc,
    /// Policy/template CRUD use case — the `/v1/authz/policies` HTTP routes call through
    /// this.
    pub policies: PolicyService,
    /// The same `Authorize` wrapper `roles`/`policies` embed internally, exposed directly for
    /// the `POST /v1/authz/is-authorized` handler: it needs `Authorize::check` for the
    /// self/admin exposure rule's authorization side-check AND `Authorize::decide` for the
    /// raw `Decision` (`roles`/`policies` only ever need the collapsed `check`).
    pub authorize: Authorize,
    /// The same `Arc<dyn RoleGrantStore>` `roles` wraps internally, exposed directly so a
    /// caller can seed a grant bypassing `RoleService::grant`'s anti-escalation check — there
    /// is necessarily no prior authority to authorize the very first grant against (mirrors
    /// `tests/authz_bootstrap.rs`'s bootstrap-grant pattern). Sharing THIS exact store (not a
    /// freshly constructed one) matters: `PgRoleGrantStore::grant` bumps `policy_gen` via the
    /// `Generations` handle embedded in `AppState::new`'s `gens` — a grant seeded through a
    /// different store instance would bump a different, unobserved counter and never become
    /// visible to `authz`'s `PolicySnapshot::reload_if_stale` (AC1). Production HTTP/gRPC
    /// code should go through `roles`, never this; today it exists for integration-test
    /// seeding ahead of SMA-444 Task 21's config-driven bootstrap-admin seeding.
    pub role_grant_store: Arc<dyn RoleGrantStore>,
    /// The SMA-444 Task 20 tenancy-retrofit enforcement toggle, config-driven from
    /// `authz.enforce_tenancy` (Task 21 — replaces the old hardcoded `ENFORCE_TENANCY`
    /// const): every tenancy handler (`organizations.rs`/`teams.rs`/`projects.rs`/
    /// `memberships.rs`, and their gRPC mirrors in `adapters::grpc::tenancy`) reads this
    /// field before deciding whether to call `AppState.authorize.check`.
    pub enforce_tenancy: bool,
    /// Cold-start bootstrap-admin seeding (SMA-444 Task 21b, spec D9/challenge M4):
    /// `adapters::http::auth_middleware::require_bearer` and
    /// `adapters::grpc::authn::AuthEnforce` both call
    /// `ensure_platform_admin` right after a successful `authn.resolve(.., Enabled)`, so a
    /// configured `authz.bootstrap_admins` identity is JIT-granted `platform_admin`@`Root`
    /// on its first authentication. Built from `cfg.authz.bootstrap_admins` — empty by
    /// default, in which case every call is a no-op `HashSet` lookup. Deliberately NOT
    /// consulted by the read-only `introspect` path (D10).
    pub bootstrap_seeder: BootstrapAdminSvc,
    /// The wired API-key bearer authenticator (SMA-445 Task 19) — the credential-router branch
    /// in `auth_middleware::require_bearer`/`grpc::authn::AuthEnforce::call` calls
    /// `api_key_auth.resolve` instead of `authn.resolve` when a presented bearer token starts
    /// with `api_key_prefix`.
    pub api_key_auth: ApiKeyAuthSvc,
    /// The service-account lifecycle use case (SMA-445 Task 16) — no HTTP route calls this yet
    /// (the `/v1/service-accounts` routes land in Task 20); wired here so that task only has to
    /// add handlers, not composition-root plumbing.
    pub service_accounts: ServiceAccountSvc,
    /// The API-key lifecycle use case (SMA-445 Task 17) — same "wired ahead of its routes"
    /// posture as `service_accounts` above.
    pub api_keys: ApiKeySvc,
    /// The configured API-key bearer prefix (`cfg.api_keys.key_prefix`, e.g. `pgs_sk_`) — both
    /// enforcement seams need it to decide `api_key_auth.resolve` vs. `authn.resolve` for a
    /// presented bearer token (SMA-445 Task 19); cached here rather than re-read from `cfg` at
    /// request time, mirroring `introspect_body_limit`'s identical rationale.
    pub api_key_prefix: String,
    /// Route-level body cap for `POST /v1/authn/api-keys/introspect` (SMA-445 Task 20, spec
    /// H1) — `cfg.api_keys.max_token_bytes` + [`INTROSPECT_BODY_OVERHEAD_BYTES`], mirroring
    /// `introspect_body_limit`'s identical rationale for the OIDC token-introspect route.
    pub api_key_introspect_body_limit: usize,
    /// The audit-log read-side use case (SMA-446 Task A10) — the gRPC
    /// `AuditService.ListAuditEntries` (`adapters::grpc::audit`) and the future HTTP
    /// `/v1/audit-log` handler (Task A11) both read through this. Wraps the SAME `authorize`
    /// this state also exposes directly (mirrors `roles`/`policies`'s posture); the Root-only
    /// restriction on `list` lives in `AuditQueryService` itself, not the Cedar schema (see
    /// its module doc).
    pub audit_query: AuditQueryService,
}

impl AppState {
    /// The compiled-policy snapshot `self.authz` decides against — `main.rs` spawns its
    /// background reload task (`PolicySnapshot::spawn_reload`) off this handle, exactly once
    /// per process (every `AppState` clone shares the same underlying `Arc`).
    #[must_use]
    pub fn snapshot(&self) -> Arc<PolicySnapshot> {
        self.snapshot.clone()
    }

    /// Builds every tenancy service from `db` (each wired to its own Postgres repository —
    /// a cheap clone of the same connection pool handle — `KernelIdGenerator`, and
    /// `SystemClock`) plus the wired `AuthnSvc` from `cfg.authn`: the OIDC authenticator
    /// over the configured JWKS cache backend, the Pg authn repositories, and the
    /// per-issuer `JitPolicy`. Fails when the Redis JWKS cache is configured but
    /// unreachable (`RedisJwksCache::connect` is the async part) — `IamConfig::validate`
    /// has already guaranteed `redis_url` is present and every issuer parses.
    ///
    /// **Authorizer wiring (SMA-444 Task 15/21):** ONE shared [`Generations`] handle — the
    /// `memory` backend, or a `redis` `ConnectionManager` selected by `authz.cache.backend`
    /// (Task 21) — feeds the authz Postgres stores (`PgPolicyStore`, `PgRoleGrantStore`,
    /// `PgEntitySliceLoader`) AND the three tenancy repositories below, so a policy/grant
    /// change bumps `policy_gen` and a tenancy structure/status change bumps `entity_gen` —
    /// both observed by the SAME counters `CedarAuthorizer`'s decision cache keys off. When
    /// `redis` is configured, the SAME `ConnectionManager` (`redis_conn` below) also backs
    /// the decision cache (`RedisDecisionCache`) and the entity-slice cache (`SliceCache`) —
    /// one shared connection, not three independent ones. Fails fast (mirroring the JWKS
    /// redis cache below) when redis is configured but unreachable — `IamConfig::validate`
    /// has already guaranteed `redis_url` is present. The initial [`PolicySnapshot`] build
    /// reads whatever the policy store currently holds — [`bootstrap::reconcile_starter`]
    /// (SMA-444 Task 17) runs first and seeds the starter Cedar policy set + the system role
    /// catalog on a fresh/unseeded database, so the initial snapshot always compiles at least
    /// that starter set, never an empty one.
    pub async fn new(db: DatabaseConnection, cfg: &IamConfig) -> Result<AppState, AuthnError> {
        let authz_cfg = &cfg.authz;
        let (gens, redis_conn): (Generations, Option<ConnectionManager>) = match authz_cfg.cache.backend {
            AuthzCacheBackend::Memory => (Generations::memory(), None),
            AuthzCacheBackend::Redis => {
                // `IamConfig::validate` rejects a redis backend without a URL at boot; a
                // `None` here is a wiring defect, not an operator error.
                let redis_url = authz_cfg
                    .cache
                    .redis_url
                    .as_deref()
                    .ok_or_else(|| AuthnError::Backend("authz.cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
                let conn = connect_redis(redis_url).await?;
                (Generations::Redis(conn.clone()), Some(conn))
            }
        };

        let orgs = OrganizationService::new(PgOrganizationRepository::new(db.clone(), gens.clone()), KernelIdGenerator, SystemClock);
        let teams = TeamService::new(PgTeamRepository::new(db.clone(), gens.clone()), KernelIdGenerator, SystemClock);
        let projects = ProjectService::new(
            PgProjectRepository::new(db.clone(), gens.clone()),
            PgTeamRepository::new(db.clone(), gens.clone()),
            KernelIdGenerator,
            SystemClock,
        );
        let memberships = MembershipService::new(PgMembershipRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let users = CreateUser::new(PgPrincipalRepository::new(db.clone()), KernelIdGenerator, SystemClock);

        let policy_store: Arc<dyn PolicyStore> = Arc::new(PgPolicyStore::new(db.clone(), gens.clone()));
        let role_grant_store: Arc<dyn RoleGrantStore> = Arc::new(PgRoleGrantStore::new(db.clone(), gens.clone()));
        bootstrap::reconcile_starter(policy_store.as_ref(), &db).await.map_err(|e| AuthnError::Backend(Box::new(e)))?;
        let snapshot = Arc::new(
            PolicySnapshot::new(policy_store.clone(), role_grant_store.clone())
                .await
                .map_err(|e| AuthnError::Backend(Box::new(e)))?,
        );
        // `slices`/`decisions` backend selection mirrors `gens` above: `memory` when no redis
        // connection was opened, `redis` sharing `redis_conn` when one was (never a second,
        // independent connection).
        let slices: Arc<dyn EntitySliceLoader> = {
            let pg_loader: Arc<dyn EntitySliceLoader> = Arc::new(PgEntitySliceLoader::new(db.clone(), gens.clone()));
            match &redis_conn {
                Some(conn) => Arc::new(SliceCache::from_connection(pg_loader, conn.clone(), authz_cfg.slice_cache_ttl_secs)) as Arc<dyn EntitySliceLoader>,
                None => pg_loader,
            }
        };
        let decisions: Arc<dyn DecisionCache> = match &redis_conn {
            Some(conn) => Arc::new(RedisDecisionCache::from_connection(conn.clone(), authz_cfg.decision_cache_ttl_secs)),
            None => Arc::new(MemoryDecisionCache::new()),
        };
        let authz = Arc::new(CedarAuthorizer::new(
            snapshot.clone(),
            slices,
            decisions,
            Arc::new(gens.clone()) as Arc<dyn GenerationsReader>,
            Arc::new(TracingAuditSink) as Arc<dyn AuditSink>,
        ));

        // Application-layer authz use cases (SMA-444 Task 18): all three share the ONE
        // `Arc<CedarAuthorizer>` built above (via `Authorize`), and `roles`/`policies` share
        // the exact `policy_store`/`role_grant_store` handles the snapshot itself reads from
        // — so a grant/policy change made through these use cases bumps the same `gens`
        // counter `authz`'s `PolicySnapshot::reload_if_stale` polls (AC1).
        let authorize = Authorize::new(authz.clone() as Arc<dyn Authorizer>);
        // SMA-444 cross-tenant-escalation fix (FIX 2): `RoleService::resolve_scope`'s own
        // DB-lookup defense needs read access to the tenancy repos, independent of
        // `orgs`/`teams`/`projects` above (those are wrapped in `OrganizationService`/etc.,
        // not exposed as bare repos) — cheap fresh instances, `DatabaseConnection` clones an
        // `Arc`-backed pool handle.
        let role_orgs: Arc<dyn OrganizationRepository> = Arc::new(PgOrganizationRepository::new(db.clone(), gens.clone()));
        let role_teams: Arc<dyn TeamRepository> = Arc::new(PgTeamRepository::new(db.clone(), gens.clone()));
        let role_projects: Arc<dyn ProjectRepository> = Arc::new(PgProjectRepository::new(db.clone(), gens.clone()));
        let roles = RoleService::new(role_grant_store.clone(), role_orgs, role_teams, role_projects, authorize.clone(), KernelIdGenerator, SystemClock);
        let policies = PolicyService::new(policy_store.clone(), authorize.clone());

        // SMA-446 Task A10: the audit-log read-side wiring. `audit_log` is kept as its own
        // `Arc<dyn AuditLog>` handle (rather than folded straight into `audit_query`) so a
        // later task (A12, the drain sink) CAN share it — it's fine if A12 re-constructs its
        // own `PgAuditLog` instead (a cheap `DatabaseConnection` clone either way); this just
        // keeps the option open without over-engineering the sharing now (A10 brief).
        let audit_log: Arc<dyn AuditLog> = Arc::new(PgAuditLog::new(db.clone()));
        let audit_query = AuditQueryService::new(audit_log.clone(), authorize.clone());

        // Shares the SAME `role_grant_store` handle `roles`/`snapshot` do (Task 21b): a
        // bootstrap-admin seed bumps the identical `policy_gen` counter `CedarAuthorizer`
        // polls, exactly like every other role-grant mutation in this composition root.
        let bootstrap_seeder = BootstrapAdminSeeder::new(&authz_cfg.bootstrap_admins, role_grant_store.clone(), KernelIdGenerator, SystemClock);

        // --- SMA-445 Task 19: API-key auth router + service-account/api-key services -------
        // `api_key_hasher`/`api_key_cache` are the SAME shared instances `api_key_auth`
        // verifies/reads through and `api_keys`/`service_accounts` mint/evict through — a
        // revoke/archive's `cache.evict` must be visible to `api_key_auth.resolve`'s
        // `cache.get` without a second cache instance to keep in sync (mirrors
        // `role_grant_store`'s single-shared-`Arc` posture above). `pepper()` surfaces
        // `ApiKeyConfig`'s own decode/length validation — `IamConfig::validate` already
        // guarantees this succeeds for a boot-time config that went through it, but `AppState::
        // new` takes a bare `&IamConfig` (main.rs calls `validate()` separately), so this is a
        // real fallible step here, not a redundant belt-and-braces check.
        let api_key_pepper = cfg.api_keys.pepper().map_err(|e| AuthnError::Backend(Box::new(e)))?;
        let api_key_hasher = HmacSecretHasher::new(api_key_pepper);
        let api_key_cache: Arc<dyn ApiKeyValidationCache> = match cfg.api_keys.introspect_cache.backend {
            ApiKeyCacheBackend::Memory => Arc::new(MemoryApiKeyCache::new(cfg.api_keys.introspect_cache.ttl_secs)),
            ApiKeyCacheBackend::Redis => {
                // Reuse the SHARED `redis_conn` opened above for `authz.cache.backend =
                // "redis"` when one exists (the ordinary single-Redis deployment posture)
                // rather than opening a second, independent connection; only dial a fresh one
                // when authz's own cache is memory-backed but `api_keys.introspect_cache`
                // still wants redis. `IamConfig::validate` guarantees `redis_url` is present
                // whenever this arm needs to open its own connection.
                let conn = match &redis_conn {
                    Some(conn) => conn.clone(),
                    None => {
                        let redis_url = cfg
                            .api_keys
                            .introspect_cache
                            .redis_url
                            .as_deref()
                            .ok_or_else(|| AuthnError::Backend("api_keys.introspect_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
                        connect_redis(redis_url).await?
                    }
                };
                Arc::new(RedisApiKeyCache::from_connection(conn, cfg.api_keys.introspect_cache.ttl_secs))
            }
        };

        let api_key_auth = AuthenticateApiKey::new(
            PgApiKeyRepository::new(db.clone()),
            PgPrincipalRepository::new(db.clone()),
            api_key_hasher.clone(),
            SystemClock,
            api_key_cache.clone(),
            PgMembershipRepository::new(db.clone()),
            cfg.api_keys.clone(),
        );
        let service_accounts = ServiceAccountService::new(
            PgServiceAccountRepository::new(db.clone()),
            Arc::new(PgApiKeyRepository::new(db.clone())) as Arc<dyn ApiKeyRepository>,
            api_key_cache.clone(),
            authorize.clone(),
            KernelIdGenerator,
            SystemClock,
        );
        let api_keys = ApiKeyService::new(
            PgApiKeyRepository::new(db.clone()),
            PgServiceAccountRepository::new(db.clone()),
            role_grant_store.clone(),
            authorize.clone(),
            api_key_hasher,
            OsRngKeyEntropy,
            api_key_cache,
            KernelIdGenerator,
            SystemClock,
            cfg.api_keys.clone(),
        );

        let authn_cfg = &cfg.authn;
        if authn_cfg.accept_invalid_tls {
            tracing::warn!("accept_invalid_tls is enabled: TLS certificate verification for IdP discovery/JWKS fetches is DISABLED — test-only configuration, never use in production");
        }
        let fetcher = HttpJwksFetcher::new(Duration::from_secs(authn_cfg.http_timeout_secs), authn_cfg.accept_invalid_tls)?;
        let ttl = Duration::from_secs(authn_cfg.jwks_ttl_secs);
        let cooldown = Duration::from_secs(authn_cfg.jwks_refresh_cooldown_secs);
        let authenticator = match authn_cfg.jwks_cache.backend {
            JwksCacheBackend::Memory => WiredAuthenticator::Memory(Arc::new(OidcAuthenticator::new(
                authn_cfg.issuers.clone(),
                JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, ttl, cooldown),
                authn_cfg.leeway_secs,
                authn_cfg.max_token_bytes,
            )?)),
            JwksCacheBackend::Redis => {
                // `IamConfig::validate` rejects a redis backend without a URL at boot; a
                // `None` here is a wiring defect, not an operator error.
                let redis_url = authn_cfg
                    .jwks_cache
                    .redis_url
                    .as_deref()
                    .ok_or_else(|| AuthnError::Backend("jwks_cache.backend = \"redis\" without redis_url (IamConfig::validate must run first)".into()))?;
                let cache = RedisJwksCache::connect(redis_url, authn_cfg.jwks_ttl_secs).await?;
                WiredAuthenticator::Redis(Arc::new(OidcAuthenticator::new(
                    authn_cfg.issuers.clone(),
                    JwksProvider::new(fetcher, cache, SystemClock, ttl, cooldown),
                    authn_cfg.leeway_secs,
                    authn_cfg.max_token_bytes,
                )?))
            }
        };

        let jit_flags = authn_cfg
            .issuers
            .iter()
            .map(|issuer_cfg| Issuer::parse(&issuer_cfg.issuer).map(|issuer| (issuer, issuer_cfg.jit_provisioning)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AuthnError::Backend(e.to_string().into()))?;
        let authn = AuthenticateToken::new(
            authenticator,
            PgExternalIdentityRepository::new(db.clone()),
            PgPrincipalRepository::new(db.clone()),
            PgMembershipRepository::new(db.clone()),
            KernelIdGenerator,
            SystemClock,
            JitPolicy::from_issuers(&jit_flags),
        );

        Ok(AppState {
            db,
            orgs,
            teams,
            projects,
            memberships,
            users,
            authn,
            authz,
            snapshot,
            introspect_body_limit: cfg.authn.max_token_bytes + INTROSPECT_BODY_OVERHEAD_BYTES,
            roles,
            policies,
            authorize,
            role_grant_store,
            enforce_tenancy: authz_cfg.enforce_tenancy,
            bootstrap_seeder,
            api_key_auth,
            service_accounts,
            api_keys,
            api_key_prefix: cfg.api_keys.key_prefix.clone(),
            api_key_introspect_body_limit: cfg.api_keys.max_token_bytes + INTROSPECT_BODY_OVERHEAD_BYTES,
            audit_query,
        })
    }
}

/// Opens `redis_url` and wraps it in an auto-reconnecting `ConnectionManager` — shared by every
/// redis-backed cache `AppState::new` wires (the authz `Generations`/`RedisDecisionCache`/
/// `SliceCache` trio, SMA-444 Task 21; the API-key `RedisApiKeyCache`, SMA-445 Task 19, when it
/// can't reuse an already-open `redis_conn`), mirroring `RedisJwksCache::connect`'s connect
/// pattern.
async fn connect_redis(redis_url: &str) -> Result<ConnectionManager, AuthnError> {
    let client = redis::Client::open(redis_url).map_err(|e| AuthnError::Backend(Box::new(e)))?;
    ConnectionManager::new(client).await.map_err(|e| AuthnError::Backend(Box::new(e)))
}

/// Liveness only — stateless, so it is testable without a database.
pub fn health_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

/// Full HTTP surface: liveness + DB-backed readiness + the `/v1` tenancy API + authn
/// introspection. The tenancy routes (organizations/teams/projects/memberships/users) sit
/// on their own sub-router carrying the bearer-enforcement `route_layer` (D14 — attached
/// HERE, inside `router()`, not in `serve_http`, so the `oneshot` test harness exercises
/// it). `/healthz`, `/readyz`, and `POST /v1/authn/introspect` are merged OUTSIDE that
/// layer and stay unauthenticated (spec §7.4).
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .merge(organizations::router())
        .merge(teams::router())
        .merge(projects::router())
        .merge(memberships::router())
        .merge(users::router())
        .merge(authz::router())
        .merge(service_accounts::router())
        .merge(api_keys::router())
        // `route_layer` (not `layer`): the enforcement covers exactly the routes defined
        // above and never the merged-in `/healthz`/`/readyz`/introspect or the 404 fallback.
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware::require_bearer))
        .with_state(state.clone());
    let public = Router::new().route("/readyz", get(readyz)).with_state(state.clone());
    let authn_api = authn::router(state.introspect_body_limit).with_state(state.clone());
    // `POST /v1/authn/api-keys/introspect` (SMA-445 Task 20): unauthenticated like `authn_api`
    // above — merged OUTSIDE `protected`, so the bearer-enforcement `route_layer` never covers
    // it (spec §10.2, mirrors `authn::router`'s own posture exactly).
    let api_key_introspect_api = api_keys::introspect_router(state.api_key_introspect_body_limit).with_state(state);
    health_router().merge(protected).merge(public).merge(authn_api).merge(api_key_introspect_api)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ping = state.db.execute(Statement::from_string(state.db.get_database_backend(), "SELECT 1")).await;
    match ping {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed: database ping error");
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unready" })))
        }
    }
}

/// Serve the HTTP surface on `addr` until `shutdown` resolves.
pub async fn serve_http(addr: SocketAddr, state: AppState, request_timeout: Duration, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    // `TimeoutLayer::new` is deprecated since tower-http 0.6.7 in favor of
    // `with_status_code`; `REQUEST_TIMEOUT` (408) reproduces `new`'s prior default.
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, request_timeout));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}
