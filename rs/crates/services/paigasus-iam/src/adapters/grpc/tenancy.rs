// SPDX-License-Identifier: Apache-2.0

//! `TenancyGrpc`: the `TenancyService` gRPC server (21 RPCs, task-16 brief). Every method is
//! thin: parse the wire PRN(s) -> call the same `AppState` service the HTTP surface uses ->
//! convert the result; all business logic lives in the application/domain layers.
//!
//! Every Get/Rename/Archive/Restore compares the *stored* canonical PRN (`view.node.id
//! .canonical()`) with the request's parsed one — the forged-org-slot defense (brief rule 8,
//! mirroring the HTTP layer's semantics). A Get compares after its read. A Rename/Archive/
//! Restore compares BEFORE its write, in `load_{org,team,project}_checked` (SMA-643): the
//! comparison used to run after the service call, so a forged organization slot committed the
//! write, the audit row and the outbox event and still answered `prn-mismatch`. The comparison
//! is sound outside the write transaction because a node's stored PRN never changes (the `prn`
//! column is written once, at insert, and nothing moves a node to a different parent).
//!
//! Creates and Lists **that take a parent PRN** do NOT compare it: they take the parent's uuid
//! and discard the rest, so a forged parent organization slot is accepted without an error (the
//! write still goes to the real parent). That is SMA-645, not a property of this design.
//!
//! **SMA-444 Task 20/21 enforcement:** every RPC authorizes the bearer-resolved actor
//! ([`actor_context`]) before performing its operation, gated by
//! `AppState.enforce_tenancy` (config-driven, `authz.enforce_tenancy`, Task 21) — mirrors
//! `adapters::http::{organizations,teams,projects,memberships}`'s
//! fetch-then-authorize-then-act posture (the same action to resource map, spec §9.4). Under
//! the default `enforce_tenancy = true` the two transports answer alike. They differ only in
//! the test-only `enforce_tenancy = false` setting, where gRPC still loads the node (SMA-643)
//! and HTTP does not, so gRPC answers `not-found` for an unknown uuid where HTTP answers
//! `nothing-to-rename` or `invalid-slug` first. `CreateTeam`/
//! `ListTeams` fetch the parent org first (`orgs.get`); `CreateProject`/`ListProjects`/
//! `AttachMembership`/`ListMemberships`(node-filtered) resolve their parent/target node by
//! uuid through the owning service ([`resolve_node`]) — all rather than trusting the wire
//! PRN's org slot directly (or building an unchecked PRN straight from a path/wire uuid),
//! which would otherwise let a claimed-but-nonexistent parent reach the entity-slice loader
//! and fail closed as an internal error instead of the expected `NotFound`. The existing
//! forged-org-slot defense (this module's own stored-canonical check, and
//! `MembershipService::attach`'s own `PrnMismatch` detection) fires BEFORE the actual mutating
//! call; this only keeps the AUTHORIZATION step itself from ever entity-slice-loading
//! a claimed-but-nonexistent org.

use std::time::Instant;

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, NodeStatus, NodeView, TenancyNodeRef};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
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
use crate::adapters::http::AppState;
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
/// there). `convert::missing_auth_context()` rather than a panic: this "shouldn't happen" for
/// a non-exempt RPC (the `AuthLayer` in `grpc::mod` always resolves one first), but a
/// defensive error beats a 500/panic if it ever does.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

/// Parses a raw tenancy-node PRN string into a typed node ref — mirrors
/// `application::memberships::parse_node_prn`/`adapters::http::memberships::parse_node_prn`.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Maps the `ListMembershipsRequest.filter` oneof to a `MembershipFilter`.
///
/// A proto3 `oneof` cannot carry two values, so `None` means NEITHER field is set — which is
/// `missing-required-field`, not a conflict (SMA-586 D6). The HTTP twin, whose two query
/// params CAN both be present, is the only surface that can produce
/// `MutuallyExclusiveFields`.
///
/// An explicitly-set-but-EMPTY arm is normalised to absent, exactly as
/// `http::memberships::membership_filter` normalises `?principal=` — so both transports answer
/// `missing-required-field` for it (SMA-586 fix round 2). D7's rationale for NOT applying
/// `require_present` here ("proto3's empty string IS the unset sentinel") holds for a plain
/// field, but not for a `oneof`, where the ARM's presence is the presence signal and an empty
/// string inside a present arm is a real, distinguishable value. Left unnormalised it reached
/// `application::memberships` and came back `invalid-prn` while HTTP said
/// `missing-required-field` — a cross-transport divergence D7 exists to prevent.
pub(crate) fn membership_filter(filter: Option<list_memberships_request::Filter>) -> Result<MembershipFilter, TenancyError> {
    let filter = filter.filter(|f| {
        let raw = match f {
            list_memberships_request::Filter::PrincipalPrn(prn) | list_memberships_request::Filter::NodePrn(prn) => prn,
        };
        !raw.trim().is_empty()
    });
    match filter {
        Some(list_memberships_request::Filter::PrincipalPrn(prn)) => Ok(MembershipFilter::Principal(prn)),
        Some(list_memberships_request::Filter::NodePrn(prn)) => Ok(MembershipFilter::Node(prn)),
        None => Err(TenancyError::MissingRequiredField("principal_prn|node_prn")),
    }
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

/// Loads the stored organization, authorizes against its OWN prn, and refuses a request prn
/// that does not match the stored canonical one — all BEFORE the caller writes (SMA-643).
///
/// Order matters twice over. The load comes first because the request prn's organization slot
/// is caller input; authorizing against the stored prn is what keeps a forged slot from
/// choosing the resource. The comparison comes AFTER the authorization, so a caller with no
/// grant gets `permission-denied` for a forged prn and for the correct prn alike — otherwise
/// the difference between the two answers would tell the caller which organization owns the
/// node.
///
/// The comparison is sound outside the write transaction because a node's stored prn never
/// changes: the `prn` column is written once, at insert, and no repository method, service or
/// migration moves a node to a different parent. A future "move" feature breaks that invariant
/// and must revisit this helper.
async fn load_org_checked(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// The team twin of [`load_org_checked`] — same order, same reasons.
async fn load_team_checked(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.teams.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// The project twin of [`load_org_checked`] — same order, same reasons.
async fn load_project_checked(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.projects.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// One warning line per refused write (SMA-643 D4). After the check moved before the write, a
/// refused attempt leaves NO audit row and no denial row, and the attempt is a tampering
/// signal, so the log line is the only trace.
fn warn_prn_mismatch(actor: &Prn, requested: &str, stored: &str, rpc: &str) {
    tracing::warn!(rpc = %rpc, actor = %actor.canonical(), requested_prn = %requested, stored_prn = %stored, "refused a tenancy request: the request prn does not match the stored node");
}

#[tonic::async_trait]
impl TenancyService for TenancyGrpc {
    // ---- organizations ----

    async fn create_organization(&self, request: Request<CreateOrganizationRequest>) -> Result<Response<CreateOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateOrganizationResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            if self.state.enforce_tenancy {
                self.state
                    .authorize
                    .check(actor_principal.prn(), Action::CreateOrganization, &root_prn())
                    .await
                    .map_err(convert::status_to_grpc)?;
            }
            let req = request.into_inner();
            // The creating principal becomes the new org's `org_admin` owner (spec D8) — seeded
            // atomically with the org + default team, regardless of `enforce_tenancy`.
            let out = self.state.orgs.create(&actor_principal, &req.slug, &req.name).await.map_err(convert::status_to_grpc)?;
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
        .await;
        record_grpc("Tenancy", "CreateOrganization", started, &result);
        result
    }

    async fn get_organization(&self, request: Request<GetOrganizationRequest>) -> Result<Response<GetOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<GetOrganizationResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
            let view = self.state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::GetOrganization, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
            }
            if view.node.id.canonical() != canonical {
                return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
            }
            Ok(Response::new(GetOrganizationResponse {
                organization: Some(convert::to_proto_org(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "GetOrganization", started, &result);
        result
    }

    async fn list_organizations(&self, request: Request<ListOrganizationsRequest>) -> Result<Response<ListOrganizationsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListOrganizationsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::ListOrganizations, &root_prn()).await.map_err(convert::status_to_grpc)?;
            }
            let req = request.into_inner();
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let views = self.state.orgs.list(page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListOrganizationsResponse {
                organizations: views.iter().map(convert::to_proto_org).collect(),
            }))
        }
        .await;
        record_grpc("Tenancy", "ListOrganizations", started, &result);
        result
    }

    async fn rename_organization(&self, request: Request<RenameOrganizationRequest>) -> Result<Response<RenameOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RenameOrganizationResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (id, canonical) = convert::node_uuid(&req.prn, "organization")?;
            load_org_checked(&self.state, &actor, Action::RenameOrganization, id, &canonical, "RenameOrganization").await?;
            let view = self
                .state
                .orgs
                .rename(id, req.new_slug.as_deref(), req.new_name.as_deref(), &actor_principal)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(RenameOrganizationResponse {
                organization: Some(convert::to_proto_org(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RenameOrganization", started, &result);
        result
    }

    async fn archive_organization(&self, request: Request<ArchiveOrganizationRequest>) -> Result<Response<ArchiveOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ArchiveOrganizationResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
            load_org_checked(&self.state, &actor, Action::ArchiveOrganization, id, &canonical, "ArchiveOrganization").await?;
            let view = self.state.orgs.archive(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ArchiveOrganizationResponse {
                organization: Some(convert::to_proto_org(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "ArchiveOrganization", started, &result);
        result
    }

    async fn restore_organization(&self, request: Request<RestoreOrganizationRequest>) -> Result<Response<RestoreOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RestoreOrganizationResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "organization")?;
            load_org_checked(&self.state, &actor, Action::RestoreOrganization, id, &canonical, "RestoreOrganization").await?;
            let view = self.state.orgs.restore(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(RestoreOrganizationResponse {
                organization: Some(convert::to_proto_org(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RestoreOrganization", started, &result);
        result
    }

    // ---- teams ----

    async fn create_team(&self, request: Request<CreateTeamRequest>) -> Result<Response<CreateTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateTeamResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (org_id, _) = convert::node_uuid(&req.org_prn, "organization")?;
            if self.state.enforce_tenancy {
                // Resolved by uuid through `orgs.get` (not the wire `org_prn` string directly, and
                // not a `OrganizationId::from_uuid` PRN built without confirming existence): a
                // nonexistent org would otherwise reach the entity-slice loader with a dangling id
                // and fail closed as an internal error rather than the expected `NotFound` — mirrors
                // `create_project`/`list_projects`'s `teams.get` resolution below.
                let org_view = self.state.orgs.get(org_id).await.map_err(convert::status_to_grpc)?;
                self.state.authorize.check(&actor, Action::CreateTeam, org_view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
            }
            let view = self.state.teams.create(org_id, &req.slug, &req.name, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(CreateTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "CreateTeam", started, &result);
        result
    }

    async fn get_team(&self, request: Request<GetTeamRequest>) -> Result<Response<GetTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<GetTeamResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
            let view = self.state.teams.get(id).await.map_err(convert::status_to_grpc)?;
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::GetTeam, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
            }
            if view.node.id.canonical() != canonical {
                return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
            }
            Ok(Response::new(GetTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "GetTeam", started, &result);
        result
    }

    async fn list_teams(&self, request: Request<ListTeamsRequest>) -> Result<Response<ListTeamsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListTeamsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let (org_id, _) = convert::node_uuid(&req.org_prn, "organization")?;
            if self.state.enforce_tenancy {
                let org_view = self.state.orgs.get(org_id).await.map_err(convert::status_to_grpc)?;
                self.state.authorize.check(&actor, Action::ListTeams, org_view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
            }
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let views = self.state.teams.list_by_org(org_id, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListTeamsResponse {
                teams: views.iter().map(convert::to_proto_team).collect(),
            }))
        }
        .await;
        record_grpc("Tenancy", "ListTeams", started, &result);
        result
    }

    async fn rename_team(&self, request: Request<RenameTeamRequest>) -> Result<Response<RenameTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RenameTeamResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (id, canonical) = convert::node_uuid(&req.prn, "team")?;
            load_team_checked(&self.state, &actor, Action::RenameTeam, id, &canonical, "RenameTeam").await?;
            let view = self
                .state
                .teams
                .rename(id, req.new_slug.as_deref(), req.new_name.as_deref(), &actor_principal)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(RenameTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RenameTeam", started, &result);
        result
    }

    async fn archive_team(&self, request: Request<ArchiveTeamRequest>) -> Result<Response<ArchiveTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ArchiveTeamResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
            load_team_checked(&self.state, &actor, Action::ArchiveTeam, id, &canonical, "ArchiveTeam").await?;
            let view = self.state.teams.archive(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ArchiveTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "ArchiveTeam", started, &result);
        result
    }

    async fn restore_team(&self, request: Request<RestoreTeamRequest>) -> Result<Response<RestoreTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RestoreTeamResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "team")?;
            load_team_checked(&self.state, &actor, Action::RestoreTeam, id, &canonical, "RestoreTeam").await?;
            let view = self.state.teams.restore(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(RestoreTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RestoreTeam", started, &result);
        result
    }

    // ---- projects ----

    async fn create_project(&self, request: Request<CreateProjectRequest>) -> Result<Response<CreateProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateProjectResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (team_id, _) = convert::node_uuid(&req.team_prn, "team")?;
            if self.state.enforce_tenancy {
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
            let view = self.state.projects.create(team_id, &req.slug, &req.name, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(CreateProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "CreateProject", started, &result);
        result
    }

    async fn get_project(&self, request: Request<GetProjectRequest>) -> Result<Response<GetProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<GetProjectResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
            let view = self.state.projects.get(id).await.map_err(convert::status_to_grpc)?;
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::GetProject, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
            }
            if view.node.id.canonical() != canonical {
                return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
            }
            Ok(Response::new(GetProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "GetProject", started, &result);
        result
    }

    async fn list_projects(&self, request: Request<ListProjectsRequest>) -> Result<Response<ListProjectsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListProjectsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let (team_id, _) = convert::node_uuid(&req.team_prn, "team")?;
            if self.state.enforce_tenancy {
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
        .await;
        record_grpc("Tenancy", "ListProjects", started, &result);
        result
    }

    async fn rename_project(&self, request: Request<RenameProjectRequest>) -> Result<Response<RenameProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RenameProjectResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (id, canonical) = convert::node_uuid(&req.prn, "project")?;
            load_project_checked(&self.state, &actor, Action::RenameProject, id, &canonical, "RenameProject").await?;
            let view = self
                .state
                .projects
                .rename(id, req.new_slug.as_deref(), req.new_name.as_deref(), &actor_principal)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(RenameProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RenameProject", started, &result);
        result
    }

    async fn archive_project(&self, request: Request<ArchiveProjectRequest>) -> Result<Response<ArchiveProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ArchiveProjectResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
            load_project_checked(&self.state, &actor, Action::ArchiveProject, id, &canonical, "ArchiveProject").await?;
            let view = self.state.projects.archive(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ArchiveProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "ArchiveProject", started, &result);
        result
    }

    async fn restore_project(&self, request: Request<RestoreProjectRequest>) -> Result<Response<RestoreProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RestoreProjectResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let (id, canonical) = convert::node_uuid(&request.get_ref().prn, "project")?;
            load_project_checked(&self.state, &actor, Action::RestoreProject, id, &canonical, "RestoreProject").await?;
            let view = self.state.projects.restore(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(RestoreProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RestoreProject", started, &result);
        result
    }

    // ---- memberships ----

    async fn attach_membership(&self, request: Request<AttachMembershipRequest>) -> Result<Response<AttachMembershipResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<AttachMembershipResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            if self.state.enforce_tenancy {
                let node = parse_node_prn(&req.node_prn).map_err(convert::status_to_grpc)?;
                let resource = resolve_node(&self.state, &node).await.map_err(convert::status_to_grpc)?;
                self.state.authorize.check(&actor, Action::AttachMembership, &resource).await.map_err(convert::status_to_grpc)?;
            }
            // Unlike the node CRUD RPCs above, `MembershipService::attach` takes the raw wire PRN
            // strings directly — it parses/validates them itself (principal + node), so there is
            // no separate `node_uuid` step here.
            let record = self
                .state
                .memberships
                .attach(&req.principal_prn, &req.node_prn, &actor_principal)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(AttachMembershipResponse {
                membership: Some(convert::to_proto_membership(&record)),
            }))
        }
        .await;
        record_grpc("Tenancy", "AttachMembership", started, &result);
        result
    }

    async fn detach_membership(&self, request: Request<DetachMembershipRequest>) -> Result<Response<DetachMembershipResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<DetachMembershipResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            // `DetachMembershipRequest.id` is a bare uuid, not a PRN, so a malformed value is
            // `InvalidUuid` naming the segment (SMA-586). The field name reaches the client in
            // both the message and `ErrorInfo.metadata["field"]`.
            //
            // Detaching an ORG membership cascades: the principal's team/project memberships in
            // that same org are removed in the same transaction (spec §5.1 rule 5). Detaching a
            // team/project membership removes only itself.
            let id = Uuid::parse_str(&req.id).map_err(|_| convert::status_to_grpc(TenancyError::InvalidUuid("membership_id")))?;
            if self.state.enforce_tenancy {
                let record = self.state.memberships.get(id).await.map_err(convert::status_to_grpc)?;
                let node_prn = Prn::parse(&record.node_prn).map_err(|e| convert::status_to_grpc(TenancyError::InvalidPrn(e.kind().to_owned())))?;
                self.state.authorize.check(&actor, Action::DetachMembership, &node_prn).await.map_err(convert::status_to_grpc)?;
            }
            self.state.memberships.detach(id, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(DetachMembershipResponse {}))
        }
        .await;
        record_grpc("Tenancy", "DetachMembership", started, &result);
        result
    }

    async fn list_memberships(&self, request: Request<ListMembershipsRequest>) -> Result<Response<ListMembershipsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListMembershipsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let filter = membership_filter(req.filter).map_err(convert::status_to_grpc)?;
            if self.state.enforce_tenancy {
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
        .await;
        record_grpc("Tenancy", "ListMemberships", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use paigasus_proto::paigasus::common::v1::ErrorReason;

    use super::*;

    /// SMA-586 D6: the `ListMembershipsRequest.filter` oneof cannot carry two values, so its
    /// `None` arm means NEITHER field is set — which is `missing-required-field`. The old message
    /// ("provide exactly one of …") described a failure the wire format makes impossible.
    ///
    /// The expected code is routed through the `ErrorReason` registry rather than spelled as a
    /// bare kebab literal (review finding on SMA-586): this way the assertion fails not only on
    /// a renamed wire string but also if `MissingRequiredField` is ever dropped from the
    /// registry, and it never gives `repo:error-code-single-site` a new literal to guard.
    #[test]
    fn an_absent_membership_filter_oneof_is_a_missing_required_field() {
        let err = membership_filter(None).unwrap_err();
        assert_eq!(err, TenancyError::MissingRequiredField("principal_prn|node_prn"));
        assert_eq!(err.code(), ErrorReason::MissingRequiredField.as_wire_reason().expect("not the Unspecified sentinel"));
    }
}
