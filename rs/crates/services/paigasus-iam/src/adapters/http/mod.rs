// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness), `/readyz` (DB-backed readiness), the
//! `/v1` tenancy API (organizations/teams/projects/memberships/users, ADR-0014), the
//! authn introspection endpoint (`/v1/authn/introspect`, SMA-443), and the `/v1/authz`
//! authorization API (`is-authorized`/policies/role-grants, SMA-444 Task 18).

pub mod auth_middleware;
pub mod authn;
mod authz;
pub mod authz_middleware;
pub mod dto;
pub mod error;
mod memberships;
mod organizations;
mod projects;
mod teams;
mod users;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use paigasus_iam_core::{Authenticator, AuthnError, Authorizer, Issuer, ValidatedClaims};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::adapters::authz::{CedarAuthorizer, Generations, GenerationsReader, MemoryDecisionCache, PolicySnapshot, TracingAuditSink};
use crate::adapters::clock::SystemClock;
use crate::adapters::id::KernelIdGenerator;
use crate::adapters::oidc::jwks::{HttpJwksFetcher, InMemoryJwksCache, JwksProvider};
use crate::adapters::oidc::redis_cache::RedisJwksCache;
use crate::adapters::oidc::validator::OidcAuthenticator;
use crate::adapters::persistence::{
    PgEntitySliceLoader, PgExternalIdentityRepository, PgMembershipRepository, PgOrganizationRepository, PgPolicyStore, PgPrincipalRepository, PgProjectRepository, PgRoleGrantStore, PgTeamRepository,
};
use crate::application::authenticate_token::{AuthenticateToken, JitPolicy};
use crate::application::authorize::Authorize;
use crate::application::bootstrap;
use crate::application::create_user::CreateUser;
use crate::application::memberships::MembershipService;
use crate::application::organizations::OrganizationService;
use crate::application::policies::PolicyService;
use crate::application::projects::ProjectService;
use crate::application::roles::RoleService;
use crate::application::teams::TeamService;
use crate::config::{IamConfig, JwksCacheBackend};
use paigasus_iam_core::{AuditSink, DecisionCache, EntitySliceLoader, PolicyStore, RoleGrantStore};

pub type OrgSvc = OrganizationService<PgOrganizationRepository, KernelIdGenerator, SystemClock>;
pub type TeamSvc = TeamService<PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type ProjectSvc = ProjectService<PgProjectRepository, PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type MembershipSvc = MembershipService<PgMembershipRepository, KernelIdGenerator, SystemClock>;
pub type UserSvc = CreateUser<PgPrincipalRepository, KernelIdGenerator, SystemClock>;
/// The `RoleGrant` CRUD use case (SMA-444 Task 18), wired over the same `Arc<dyn
/// RoleGrantStore>` `AppState.role_grant_store` holds — mirrors every other `*Svc` alias's
/// `KernelIdGenerator`/`SystemClock` DI posture.
pub type RoleSvc = RoleService<KernelIdGenerator, SystemClock>;

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

/// Max staleness bound for the in-process [`PolicySnapshot`] (spec §7/D11 AC3): `main.rs`
/// spawns [`PolicySnapshot::spawn_reload`] with this as its `ttl` — a forced, unconditional
/// reload once this much time has passed since the last successful (re)load, even if
/// `policy_gen` never visibly advanced on this replica (the `memory` backend under a change
/// made through a different process). Hardcoded for now; a later task makes this
/// config-driven (`authz.cache.*`, mirroring `authn.jwks_cache`).
pub const AUTHZ_POLICY_SNAPSHOT_TTL: Duration = Duration::from_secs(30);

/// Poll cadence for the same background reload loop: how often it checks whether
/// `policy_gen` has advanced past the compiled snapshot's own generation. A `CedarAuthorizer`
/// decision also reloads synchronously before deciding (AC1), so this interval only bounds
/// cross-replica/background staleness, never same-replica visibility of a fresh grant.
/// Hardcoded for now; see [`AUTHZ_POLICY_SNAPSHOT_TTL`].
pub const AUTHZ_POLICY_RELOAD_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Gates the SMA-444 Task 20 tenancy-retrofit enforcement (`organizations.rs`/`teams.rs`/
/// `projects.rs`/`memberships.rs`, and their gRPC mirrors in `adapters::grpc::tenancy`): when
/// `true`, every handler calls `AppState.authorize.check` before performing its operation and
/// maps a deny to `TenancyError::Forbidden` (403 / `PermissionDenied`). Hardcoded `true` for
/// now — a later task swaps this for the config-driven `authz.enforce_tenancy` (spec §11).
pub const ENFORCE_TENANCY: bool = true;

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
    /// **Authorizer wiring (SMA-444 Task 15):** ONE shared [`Generations`] handle (the
    /// `memory` backend — config-driven backend/TTL selection, mirroring `authn.jwks_cache`,
    /// is a later task) feeds the authz Postgres stores (`PgPolicyStore`, `PgRoleGrantStore`,
    /// `PgEntitySliceLoader`) AND the three tenancy repositories below, so a policy/grant
    /// change bumps `policy_gen` and a tenancy structure/status change bumps `entity_gen` —
    /// both observed by the SAME counters `CedarAuthorizer`'s decision cache keys off. The
    /// initial [`PolicySnapshot`] build reads whatever the policy store currently holds —
    /// [`bootstrap::reconcile_starter`] (SMA-444 Task 17) runs first and seeds the starter
    /// Cedar policy set + the system role catalog on a fresh/unseeded database, so the
    /// initial snapshot always compiles at least that starter set, never an empty one.
    pub async fn new(db: DatabaseConnection, cfg: &IamConfig) -> Result<AppState, AuthnError> {
        let gens = Generations::memory();

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
        let slices: Arc<dyn EntitySliceLoader> = Arc::new(PgEntitySliceLoader::new(db.clone(), gens.clone()));
        let decisions: Arc<dyn DecisionCache> = Arc::new(MemoryDecisionCache::new());
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
        let roles = RoleService::new(role_grant_store.clone(), authorize.clone(), KernelIdGenerator, SystemClock);
        let policies = PolicyService::new(policy_store.clone(), authorize.clone());

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
        })
    }
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
        // `route_layer` (not `layer`): the enforcement covers exactly the routes defined
        // above and never the merged-in `/healthz`/`/readyz`/introspect or the 404 fallback.
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware::require_bearer))
        .with_state(state.clone());
    let public = Router::new().route("/readyz", get(readyz)).with_state(state.clone());
    let authn_api = authn::router(state.introspect_body_limit).with_state(state);
    health_router().merge(protected).merge(public).merge(authn_api)
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
