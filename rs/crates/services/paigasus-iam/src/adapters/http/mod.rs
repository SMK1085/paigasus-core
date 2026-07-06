// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness), `/readyz` (DB-backed readiness), and the
//! `/v1` tenancy API (organizations/teams/projects — memberships/users routes land in
//! Task 15, ADR-0014).

pub mod dto;
pub mod error;
mod organizations;
mod projects;
mod teams;

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::adapters::clock::SystemClock;
use crate::adapters::id::KernelIdGenerator;
use crate::adapters::persistence::{PgMembershipRepository, PgOrganizationRepository, PgPrincipalRepository, PgProjectRepository, PgTeamRepository};
use crate::application::create_user::CreateUser;
use crate::application::memberships::MembershipService;
use crate::application::organizations::OrganizationService;
use crate::application::projects::ProjectService;
use crate::application::teams::TeamService;

pub type OrgSvc = OrganizationService<PgOrganizationRepository, KernelIdGenerator, SystemClock>;
pub type TeamSvc = TeamService<PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type ProjectSvc = ProjectService<PgProjectRepository, PgTeamRepository, KernelIdGenerator, SystemClock>;
pub type MembershipSvc = MembershipService<PgMembershipRepository, KernelIdGenerator, SystemClock>;
pub type UserSvc = CreateUser<PgPrincipalRepository, KernelIdGenerator, SystemClock>;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub orgs: OrgSvc,
    pub teams: TeamSvc,
    pub projects: ProjectSvc,
    // Constructed now so the composition root is stable across Task 14/15; the HTTP routes
    // that call these land in Task 15 (memberships + user-creation endpoints).
    pub memberships: MembershipSvc,
    pub users: UserSvc,
}

impl AppState {
    /// Builds every tenancy service from `db`, each wired to its own Postgres repository
    /// (a cheap clone of the same connection pool handle), `KernelIdGenerator`, and
    /// `SystemClock`.
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        let orgs = OrganizationService::new(PgOrganizationRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let teams = TeamService::new(PgTeamRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let projects = ProjectService::new(PgProjectRepository::new(db.clone()), PgTeamRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let memberships = MembershipService::new(PgMembershipRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        let users = CreateUser::new(PgPrincipalRepository::new(db.clone()), KernelIdGenerator, SystemClock);
        AppState {
            db,
            orgs,
            teams,
            projects,
            memberships,
            users,
        }
    }
}

/// Liveness only — stateless, so it is testable without a database.
pub fn health_router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

/// Full HTTP surface: liveness + DB-backed readiness + the `/v1` tenancy API.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(organizations::router())
        .merge(teams::router())
        .merge(projects::router())
        .route("/readyz", get(readyz))
        .with_state(state);
    health_router().merge(api)
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
