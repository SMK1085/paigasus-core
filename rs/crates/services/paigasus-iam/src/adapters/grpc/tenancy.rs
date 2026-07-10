// SPDX-License-Identifier: Apache-2.0

//! `TenancyGrpc`: the `TenancyService` gRPC server (21 RPCs, task-16 brief). Every method is
//! thin: parse the wire PRN(s) -> call the same `AppState` service the HTTP surface uses ->
//! convert the result; all business logic lives in the application/domain layers.
//!
//! Every Get/Rename/Archive/Restore re-checks the *stored* canonical PRN (`view.node.id
//! .canonical()`) against the request's parsed one after the service call and maps a
//! divergence to `TenancyError::PrnMismatch` — the forged-org-slot defense (brief rule 8,
//! mirroring the HTTP layer's semantics). Creates only resolve the *parent* PRN (there is no
//! "stored" resource yet to compare against); the service call re-validates the parent's
//! existence/status in-txn regardless.
//!
//! **SMA-444 Task 20 enforcement:** every RPC authorizes the bearer-resolved actor
//! ([`actor_context`]) before performing its operation, gated by
//! `adapters::http::ENFORCE_TENANCY` — mirrors `adapters::http::{organizations,teams,
//! projects,memberships}`'s fetch-then-authorize-then-act posture exactly (the same action
//! to resource map, spec §9.4), so the two transports can never diverge. `CreateTeam`/
//! `ListTeams` fetch the parent org first (`orgs.get`); `CreateProject`/`ListProjects`/
//! `AttachMembership`/`ListMemberships`(node-filtered) resolve their parent/target node by
//! uuid through the owning service ([`resolve_node`]) — all rather than trusting the wire
//! PRN's org slot directly (or building an unchecked PRN straight from a path/wire uuid),
//! which would otherwise let a claimed-but-nonexistent parent reach the entity-slice loader
//! and fail closed as an internal error instead of the expected `NotFound`. The existing
//! forged-org-slot defense (this module's own stored-canonical recheck, and
//! `MembershipService::attach`'s own `PrnMismatch` detection) still fires on the actual
//! mutating call; this only keeps the AUTHORIZATION step itself from ever entity-slice-loading
//! a claimed-but-nonexistent org.

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, NodeStatus, NodeView, TenancyNodeRef};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::list_memberships_request;
use paigasus_proto::paigasus::iam::v1::tenancy_service_server::TenancyService;
use paigasus_proto::paigasus::iam::v1::{
    ArchiveOrganizationRequest, ArchiveOrganizationResponse, ArchiveProjectRequest, ArchiveProjectResponse, ArchiveTeamRequest, ArchiveTeamResponse, AttachMembershipRequest, AttachMembershipResponse,
    CreateOrganizationRequest, CreateOrganizationResponse, CreateProjectRequest, CreateProjectResponse, CreateTeamRequest, CreateTeamResponse, DetachMembershipRequest, DetachMembershipResponse,
    GetOrganizationRequest, GetOrganizationResponse, GetProjectRequest, GetProjectResponse, GetTeamRequest, GetTeamResponse, ListMembershipsRequest, ListMembershipsResponse, ListOrganizationsRequest,
    ListOrganizationsResponse, ListProjectsRequest, ListProjectsResponse, ListTeamsRequest, ListTeamsResponse, RenameOrganizationRequest, RenameOrganizationResponse, RenameProjectRequest,
    RenameProjectResponse, RenameTeamRequest, RenameTeamResponse, RestoreOrganizationRequest, RestoreOrganizationResponse, RestoreProjectRequest, RestoreProjectResponse, RestoreTeamRequest,
    RestoreTeamResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::{AppState, ENFORCE_TENANCY};
use crate::application::error::TenancyError;
use crate::application::memberships::MembershipFilter;

/// The `TenancyService` gRPC server — a thin adapter over the same `AppState` services the
/// HTTP surface uses.
pub struct TenancyGrpc {
    state: AppState,
}

impl TenancyGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// `adapters::grpc::authz::actor_context` exactly (duplicated rather than shared across a
/// transport-internal module boundary, the same posture as `parse_prn`/`parse_policy_kind`
/// there). `Status::unauthenticated` rather than a panic: this "shouldn't happen" for a
/// non-exempt RPC (the `AuthLayer` in `grpc::mod` always resolves one first), but a
/// defensive error beats a 500/panic if it ever does.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated("missing authentication context"))
}

/// Parses a raw tenancy-node PRN string into a typed node ref — mirrors
/// `application::memberships::parse_node_prn`/`adapters::http::memberships::parse_node_prn`.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Resolves `node`'s REAL, stored PRN by looking it up (by uuid alone, ignoring whatever org
/// slot the caller's PRN claims) through the owning tenancy service — see the module docs.
async fn resolve_node(state: &AppState, node: &TenancyNodeRef) -> Result<Prn, TenancyError> {
    Ok(match node {
        TenancyNodeRef::Organization(id) => state.orgs.get(id.uuid()).await?.node.id.prn().clone(),
        TenancyNodeRef::Team(id) => state.teams.get(id.uuid()).await?.node.id.prn().clone(),
        TenancyNodeRef::Project(id) => state.projects.get(id.uuid()).await?.node.id.prn().clone(),
    })
}

#[tonic::async_trait]
impl TenancyService for TenancyGrpc {
    // ---- organizations ----

    async fn create_organization(&self, request: Request<CreateOrganizationRequest>) -> Result<Response<CreateOrganizationResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        if ENFORCE_TENANCY {
            self.state.authorize.check(&actor, Action::CreateOrganization, &root_prn()).await.map_err(convert::status_to_grpc)?;
        }
        let req = request.into_inner();
        let out = self.state.orgs.create(&req.slug, &req.name).await.map_err(convert::status_to_grpc)?;
        // `OrganizationService::create` returns the plain (non-`NodeView`) domain values —
        // both are freshly minted `Active`, and an org has no ancestors (D1/D10), so folding
        // the org's own status through as the team's one ancestor computes the correct
        // effective status for both without a repo round-trip (mirrors `http::dto`).
        let org_status = out.organization.status;
        let team_status = out.default_team.status;
        let organization = convert::to_proto_org(&NodeView {
            node: out.organization,
            effective_status: NodeStatus::effective(org_status, &[]),
        });
        let default_team = convert::to_proto_team(&NodeView {
            node: out.default_team,
            effective_status: NodeStatus::effective(team_status, &[org_status]),
        });
        Ok(Response::new(CreateOrganizationResponse {
            organization: Some(organization),
            default_team: Some(default_team),
        }))
    }

    async fn get_organization(&self, request: Request<GetOrganizationRequest>) -> Result<Response<GetOrganizationResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
        let view = self.state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
        if ENFORCE_TENANCY {
            self.state.authorize.check(&actor, Action::GetOrganization, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(GetOrganizationResponse {
            organization: Some(convert::to_proto_org(&view)),
        }))
    }

    async fn list_organizations(&self, request: Request<ListOrganizationsRequest>) -> Result<Response<ListOrganizationsResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        if ENFORCE_TENANCY {
            self.state.authorize.check(&actor, Action::ListOrganizations, &root_prn()).await.map_err(convert::status_to_grpc)?;
        }
        let req = request.into_inner();
        let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
        let views = self.state.orgs.list(page).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(ListOrganizationsResponse {
            organizations: views.iter().map(convert::to_proto_org).collect(),
        }))
    }

    async fn rename_organization(&self, request: Request<RenameOrganizationRequest>) -> Result<Response<RenameOrganizationResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (id, canonical) = convert::node_uuid(&req.prn, "organization")?;
        if ENFORCE_TENANCY {
            let existing = self.state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::RenameOrganization, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.orgs.rename(id, req.new_slug.as_deref(), req.new_name.as_deref()).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RenameOrganizationResponse {
            organization: Some(convert::to_proto_org(&view)),
        }))
    }

    async fn archive_organization(&self, request: Request<ArchiveOrganizationRequest>) -> Result<Response<ArchiveOrganizationResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
        if ENFORCE_TENANCY {
            let existing = self.state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::ArchiveOrganization, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.orgs.archive(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(ArchiveOrganizationResponse {
            organization: Some(convert::to_proto_org(&view)),
        }))
    }

    async fn restore_organization(&self, request: Request<RestoreOrganizationRequest>) -> Result<Response<RestoreOrganizationResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
        if ENFORCE_TENANCY {
            let existing = self.state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::RestoreOrganization, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.orgs.restore(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RestoreOrganizationResponse {
            organization: Some(convert::to_proto_org(&view)),
        }))
    }

    // ---- teams ----

    async fn create_team(&self, request: Request<CreateTeamRequest>) -> Result<Response<CreateTeamResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (org_id, _) = convert::node_uuid(&req.org_prn, "organization")?;
        if ENFORCE_TENANCY {
            // Resolved by uuid through `orgs.get` (not the wire `org_prn` string directly, and
            // not a `OrganizationId::from_uuid` PRN built without confirming existence): a
            // nonexistent org would otherwise reach the entity-slice loader with a dangling id
            // and fail closed as an internal error rather than the expected `NotFound` — mirrors
            // `create_project`/`list_projects`'s `teams.get` resolution below.
            let org_view = self.state.orgs.get(org_id).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::CreateTeam, org_view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        let view = self.state.teams.create(org_id, &req.slug, &req.name).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(CreateTeamResponse {
            team: Some(convert::to_proto_team(&view)),
        }))
    }

    async fn get_team(&self, request: Request<GetTeamRequest>) -> Result<Response<GetTeamResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
        let view = self.state.teams.get(id).await.map_err(convert::status_to_grpc)?;
        if ENFORCE_TENANCY {
            self.state.authorize.check(&actor, Action::GetTeam, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(GetTeamResponse {
            team: Some(convert::to_proto_team(&view)),
        }))
    }

    async fn list_teams(&self, request: Request<ListTeamsRequest>) -> Result<Response<ListTeamsResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (org_id, _) = convert::node_uuid(&req.org_prn, "organization")?;
        if ENFORCE_TENANCY {
            let org_view = self.state.orgs.get(org_id).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::ListTeams, org_view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
        let views = self.state.teams.list_by_org(org_id, page).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(ListTeamsResponse {
            teams: views.iter().map(convert::to_proto_team).collect(),
        }))
    }

    async fn rename_team(&self, request: Request<RenameTeamRequest>) -> Result<Response<RenameTeamResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (id, canonical) = convert::node_uuid(&req.prn, "team")?;
        if ENFORCE_TENANCY {
            let existing = self.state.teams.get(id).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::RenameTeam, existing.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        let view = self.state.teams.rename(id, req.new_slug.as_deref(), req.new_name.as_deref()).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RenameTeamResponse {
            team: Some(convert::to_proto_team(&view)),
        }))
    }

    async fn archive_team(&self, request: Request<ArchiveTeamRequest>) -> Result<Response<ArchiveTeamResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
        if ENFORCE_TENANCY {
            let existing = self.state.teams.get(id).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::ArchiveTeam, existing.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        let view = self.state.teams.archive(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(ArchiveTeamResponse {
            team: Some(convert::to_proto_team(&view)),
        }))
    }

    async fn restore_team(&self, request: Request<RestoreTeamRequest>) -> Result<Response<RestoreTeamResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
        if ENFORCE_TENANCY {
            let existing = self.state.teams.get(id).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::RestoreTeam, existing.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        let view = self.state.teams.restore(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RestoreTeamResponse {
            team: Some(convert::to_proto_team(&view)),
        }))
    }

    // ---- projects ----

    async fn create_project(&self, request: Request<CreateProjectRequest>) -> Result<Response<CreateProjectResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (team_id, _) = convert::node_uuid(&req.team_prn, "team")?;
        if ENFORCE_TENANCY {
            // Resolved by uuid through `teams.get` (not the wire `team_prn` string directly):
            // `ProjectService::create` itself only ever consumes the bare `team_id` uuid, with
            // no stored-canonical recheck of its own (unlike Get/Rename/Archive/Restore) — so
            // authorizing against the REAL team's prn keeps that existing "trust the uuid"
            // posture, and never entity-slice-loads a claimed-but-nonexistent org.
            let team_view = self.state.teams.get(team_id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::CreateProject, team_view.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.projects.create(team_id, &req.slug, &req.name).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(CreateProjectResponse {
            project: Some(convert::to_proto_project(&view)),
        }))
    }

    async fn get_project(&self, request: Request<GetProjectRequest>) -> Result<Response<GetProjectResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
        let view = self.state.projects.get(id).await.map_err(convert::status_to_grpc)?;
        if ENFORCE_TENANCY {
            self.state.authorize.check(&actor, Action::GetProject, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
        }
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(GetProjectResponse {
            project: Some(convert::to_proto_project(&view)),
        }))
    }

    async fn list_projects(&self, request: Request<ListProjectsRequest>) -> Result<Response<ListProjectsResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (team_id, _) = convert::node_uuid(&req.team_prn, "team")?;
        if ENFORCE_TENANCY {
            let team_view = self.state.teams.get(team_id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::ListProjects, team_view.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
        let views = self.state.projects.list_by_team(team_id, page).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(ListProjectsResponse {
            projects: views.iter().map(convert::to_proto_project).collect(),
        }))
    }

    async fn rename_project(&self, request: Request<RenameProjectRequest>) -> Result<Response<RenameProjectResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let (id, canonical) = convert::node_uuid(&req.prn, "project")?;
        if ENFORCE_TENANCY {
            let existing = self.state.projects.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::RenameProject, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self
            .state
            .projects
            .rename(id, req.new_slug.as_deref(), req.new_name.as_deref())
            .await
            .map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RenameProjectResponse {
            project: Some(convert::to_proto_project(&view)),
        }))
    }

    async fn archive_project(&self, request: Request<ArchiveProjectRequest>) -> Result<Response<ArchiveProjectResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
        if ENFORCE_TENANCY {
            let existing = self.state.projects.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::ArchiveProject, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.projects.archive(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(ArchiveProjectResponse {
            project: Some(convert::to_proto_project(&view)),
        }))
    }

    async fn restore_project(&self, request: Request<RestoreProjectRequest>) -> Result<Response<RestoreProjectResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
        if ENFORCE_TENANCY {
            let existing = self.state.projects.get(id).await.map_err(convert::status_to_grpc)?;
            self.state
                .authorize
                .check(&actor, Action::RestoreProject, existing.node.id.prn())
                .await
                .map_err(convert::status_to_grpc)?;
        }
        let view = self.state.projects.restore(id).await.map_err(convert::status_to_grpc)?;
        if view.node.id.canonical() != canonical {
            return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
        }
        Ok(Response::new(RestoreProjectResponse {
            project: Some(convert::to_proto_project(&view)),
        }))
    }

    // ---- memberships ----

    async fn attach_membership(&self, request: Request<AttachMembershipRequest>) -> Result<Response<AttachMembershipResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        if ENFORCE_TENANCY {
            let node = parse_node_prn(&req.node_prn).map_err(convert::status_to_grpc)?;
            let resource = resolve_node(&self.state, &node).await.map_err(convert::status_to_grpc)?;
            self.state.authorize.check(&actor, Action::AttachMembership, &resource).await.map_err(convert::status_to_grpc)?;
        }
        // Unlike the node CRUD RPCs above, `MembershipService::attach` takes the raw wire PRN
        // strings directly — it parses/validates them itself (principal + node), so there is
        // no separate `node_uuid` step here.
        let record = self.state.memberships.attach(&req.principal_prn, &req.node_prn).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(AttachMembershipResponse {
            membership: Some(convert::to_proto_membership(&record)),
        }))
    }

    async fn detach_membership(&self, request: Request<DetachMembershipRequest>) -> Result<Response<DetachMembershipResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        // `id` is a plain UUIDv7, not a PRN (D5) — there is no dedicated error code for "not a
        // UUID", so this reuses `InvalidPrn` with a static string as context (same sentinel
        // the PRN parsers use for "malformed input", just not itself a PRN here). The error
        // payload is server-side context only; InvalidPrn's Display never surfaces it.
        //
        // Detaching an ORG membership cascades: the principal's team/project memberships in
        // that same org are removed in the same transaction (spec §5.1 rule 5). Detaching a
        // team/project membership removes only itself.
        let id = Uuid::parse_str(&req.id).map_err(|_| convert::status_to_grpc(TenancyError::InvalidPrn("membership id must be a uuid".to_string())))?;
        if ENFORCE_TENANCY {
            let record = self.state.memberships.get(id).await.map_err(convert::status_to_grpc)?;
            let node_prn = Prn::parse(&record.node_prn).map_err(|e| convert::status_to_grpc(TenancyError::InvalidPrn(e.kind().to_owned())))?;
            self.state.authorize.check(&actor, Action::DetachMembership, &node_prn).await.map_err(convert::status_to_grpc)?;
        }
        self.state.memberships.detach(id).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(DetachMembershipResponse {}))
    }

    async fn list_memberships(&self, request: Request<ListMembershipsRequest>) -> Result<Response<ListMembershipsResponse>, Status> {
        let actor = actor_context(&request)?.principal_id.prn().clone();
        let req = request.into_inner();
        let filter = match req.filter {
            Some(list_memberships_request::Filter::PrincipalPrn(prn)) => MembershipFilter::Principal(prn),
            Some(list_memberships_request::Filter::NodePrn(prn)) => MembershipFilter::Node(prn),
            None => return Err(convert::status_to_grpc(TenancyError::InvalidPrn("provide exactly one of principal_prn|node_prn".to_string()))),
        };
        if ENFORCE_TENANCY {
            let resource = match &filter {
                MembershipFilter::Principal(_) => root_prn(),
                MembershipFilter::Node(raw) => {
                    let node = parse_node_prn(raw).map_err(convert::status_to_grpc)?;
                    resolve_node(&self.state, &node).await.map_err(convert::status_to_grpc)?
                }
            };
            self.state.authorize.check(&actor, Action::ListMemberships, &resource).await.map_err(convert::status_to_grpc)?;
        }
        let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
        let records = self.state.memberships.list(filter, page).await.map_err(convert::status_to_grpc)?;
        Ok(Response::new(ListMembershipsResponse {
            memberships: records.iter().map(convert::to_proto_membership).collect(),
        }))
    }
}
