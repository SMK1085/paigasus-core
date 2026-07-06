// SPDX-License-Identifier: Apache-2.0

//! Wire DTOs for the `/v1` tenancy HTTP API. Plain serde structs; the `From<NodeView<_>>`
//! impls do the only real work — projecting a domain node + its effective status into the
//! stable JSON shape (status fields as strings via `NodeStatus::as_str`, timestamps as
//! RFC3339 via chrono's serde feature).

use chrono::{DateTime, Utc};
use paigasus_iam_core::{NodeStatus, NodeView, Organization, OrganizationId, Project, Team};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::organizations::CreateOrgOutput;

/// Query params for the `GET .../{collection}` list endpoints.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Body for every `POST .../{collection}` create endpoint (organizations, teams, projects
/// all share the same slug+name shape).
#[derive(Debug, Clone, Deserialize)]
pub struct CreateNodeBody {
    pub slug: String,
    pub name: String,
}

/// Body for every `PATCH /v1/{organizations,teams,projects}/{id}` rename endpoint. Both
/// `None` maps to `TenancyError::NothingToRename` (400) in the application layer.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RenameBody {
    pub slug: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrgDto {
    pub prn: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub effective_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<NodeView<Organization>> for OrgDto {
    fn from(view: NodeView<Organization>) -> Self {
        OrgDto {
            prn: view.node.id.canonical(),
            slug: view.node.slug.as_str().to_string(),
            name: view.node.name,
            status: view.node.status.as_str().to_string(),
            effective_status: view.effective_status.as_str().to_string(),
            created_at: view.node.created_at,
            updated_at: view.node.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamDto {
    pub prn: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub effective_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub org_prn: String,
}

impl From<NodeView<Team>> for TeamDto {
    fn from(view: NodeView<Team>) -> Self {
        TeamDto {
            org_prn: OrganizationId::from_uuid(view.node.id.org_uuid()).canonical(),
            prn: view.node.id.canonical(),
            slug: view.node.slug.as_str().to_string(),
            name: view.node.name,
            status: view.node.status.as_str().to_string(),
            effective_status: view.effective_status.as_str().to_string(),
            created_at: view.node.created_at,
            updated_at: view.node.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDto {
    pub prn: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub effective_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub team_prn: String,
    pub org_prn: String,
}

impl From<NodeView<Project>> for ProjectDto {
    fn from(view: NodeView<Project>) -> Self {
        ProjectDto {
            team_prn: view.node.team_id.canonical(),
            org_prn: OrganizationId::from_uuid(view.node.id.org_uuid()).canonical(),
            prn: view.node.id.canonical(),
            slug: view.node.slug.as_str().to_string(),
            name: view.node.name,
            status: view.node.status.as_str().to_string(),
            effective_status: view.effective_status.as_str().to_string(),
            created_at: view.node.created_at,
            updated_at: view.node.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateOrgResponse {
    pub organization: OrgDto,
    pub default_team: TeamDto,
}

/// `OrganizationService::create` returns the plain (non-`NodeView`) domain values — both are
/// freshly minted `Active` (`Organization::new`/`Team::new`), and an org has no ancestors
/// (D1/D10), so folding the org's own status through as the team's one ancestor computes the
/// correct effective status for both without needing a repo round-trip.
impl From<CreateOrgOutput> for CreateOrgResponse {
    fn from(out: CreateOrgOutput) -> Self {
        let org_status = out.organization.status;
        let team_status = out.default_team.status;
        CreateOrgResponse {
            organization: NodeView {
                node: out.organization,
                effective_status: NodeStatus::effective(org_status, &[]),
            }
            .into(),
            default_team: NodeView {
                node: out.default_team,
                effective_status: NodeStatus::effective(team_status, &[org_status]),
            }
            .into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MembershipDto {
    pub id: Uuid,
    pub principal_prn: String,
    pub node_prn: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMembershipBody {
    pub principal_prn: String,
    pub node_prn: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserBody {
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateUserResponse {
    pub principal_prn: String,
}
