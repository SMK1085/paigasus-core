// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness), `/readyz` (DB-backed readiness), the
//! `/v1` tenancy API (organizations/teams/projects/memberships/users, ADR-0014), and the
//! authn introspection endpoint (`/v1/authn/introspect`, SMA-443).

pub mod auth_middleware;
pub mod authn;
pub mod dto;
pub mod error;
mod memberships;
mod organizations;
mod projects;
mod teams;
mod users;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use paigasus_iam_core::{Authenticator, AuthnError, Issuer, ValidatedClaims};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::adapters::clock::SystemClock;
use crate::adapters::id::KernelIdGenerator;
use crate::adapters::oidc::jwks::{HttpJwksFetcher, InMemoryJwksCache, JwksProvider};
use crate::adapters::oidc::redis_cache::RedisJwksCache;
use crate::adapters::oidc::validator::OidcAuthenticator;
use crate::adapters::persistence::{PgExternalIdentityRepository, PgMembershipRepository, PgOrganizationRepository, PgPrincipalRepository, PgProjectRepository, PgTeamRepository};
use crate::application::authenticate_token::{AuthenticateToken, JitPolicy};
use crate::application::create_user::CreateUser;
use crate::application::memberships::MembershipService;
use crate::application::organizations::OrganizationService;
use crate::application::projects::ProjectService;
use crate::application::teams::TeamService;
use crate::config::{IamConfig, JwksCacheBackend};

pub type OrgSvc = OrganizationService<PgOrganizationRepository, KernelIdGenerator, SystemClock>;
pub type TeamSvc = TeamService<PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type ProjectSvc = ProjectService<PgProjectRepository, PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type MembershipSvc = MembershipService<PgMembershipRepository, KernelIdGenerator, SystemClock>;
pub type UserSvc = CreateUser<PgPrincipalRepository, KernelIdGenerator, SystemClock>;

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
    /// Route-level body cap for `POST /v1/authn/introspect` (H1): `max_token_bytes` +
    /// [`INTROSPECT_BODY_OVERHEAD_BYTES`], computed once at wiring time.
    pub introspect_body_limit: usize,
}

impl AppState {
    /// Builds every tenancy service from `db` (each wired to its own Postgres repository —
    /// a cheap clone of the same connection pool handle — `KernelIdGenerator`, and
    /// `SystemClock`) plus the wired `AuthnSvc` from `cfg.authn`: the OIDC authenticator
    /// over the configured JWKS cache backend, the Pg authn repositories, and the
    /// per-issuer `JitPolicy`. Fails when the Redis JWKS cache is configured but
    /// unreachable (`RedisJwksCache::connect` is the async part) — `IamConfig::validate`
    /// has already guaranteed `redis_url` is present and every issuer parses.
    pub async fn new(db: DatabaseConnection, cfg: &IamConfig) -> Result<AppState, AuthnError> {
        let orgs = OrganizationService::new(PgOrganizationRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let teams = TeamService::new(PgTeamRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let projects = ProjectService::new(PgProjectRepository::new(db.clone()), PgTeamRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let memberships = MembershipService::new(PgMembershipRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let users = CreateUser::new(PgPrincipalRepository::new(db.clone()), KernelIdGenerator, SystemClock);

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
            introspect_body_limit: cfg.authn.max_token_bytes + INTROSPECT_BODY_OVERHEAD_BYTES,
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
