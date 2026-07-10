// SPDX-License-Identifier: Apache-2.0

//! Wire DTOs for the `/v1` tenancy + authn HTTP API. Plain serde structs; the
//! `From<NodeView<_>>`/`From<PrincipalContext>` impls do the only real work — projecting a
//! domain value into the stable JSON shape (status fields as strings via `as_str`,
//! timestamps as RFC3339 via chrono's serde feature, PRNs as canonical strings).

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::{MembershipRecord, NodeStatus, NodeView, Organization, OrganizationId, PolicyDocument, PrincipalContext, Project, RoleGrant, RoleGrantRef, Team};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

impl From<MembershipRecord> for MembershipDto {
    fn from(record: MembershipRecord) -> Self {
        MembershipDto {
            id: record.id,
            principal_prn: record.principal_prn,
            node_prn: record.node_prn,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMembershipBody {
    pub principal_prn: String,
    pub node_prn: String,
}

/// Query params for `GET /v1/memberships`: exactly one of `principal`/`node` must be set
/// (else `TenancyError::InvalidPrn` — mirrors the proto oneof rule).
#[derive(Debug, Clone, Deserialize)]
pub struct MembershipQuery {
    pub principal: Option<String>,
    pub node: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
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

/// Body for `POST /v1/authn/introspect` — mirrors proto `IntrospectRequest` (spec §7.2).
/// The token IS the credential: this body must never be logged (see the handler doc).
#[derive(Clone, Deserialize)]
pub struct IntrospectBody {
    pub token: String,
}

/// A [`RoleGrantRef`]-shaped JSON entry: mirrors the proto `RoleGrantRef` message field-for-
/// field (`scope_prn`, `role_key`).
#[derive(Debug, Clone, Serialize)]
pub struct RoleGrantRefDto {
    pub scope_prn: String,
    pub role_key: String,
}

impl From<RoleGrantRef> for RoleGrantRefDto {
    fn from(r: RoleGrantRef) -> Self {
        RoleGrantRefDto {
            scope_prn: r.scope_prn,
            role_key: r.role_key,
        }
    }
}

/// `IntrospectResponse`-shaped JSON (spec §7.2): mirrors proto
/// `paigasus.iam.v1.IntrospectResponse` field-for-field — snake_case, PRN strings,
/// `expires_at` as RFC3339, `role_grants` empty until a later M3 task populates it.
#[derive(Debug, Clone, Serialize)]
pub struct IntrospectResponseDto {
    pub principal_prn: String,
    pub status: String,
    pub issuer: String,
    pub subject: String,
    pub expires_at: DateTime<Utc>,
    pub memberships: Vec<MembershipDto>,
    pub role_grants: Vec<RoleGrantRefDto>,
}

impl From<PrincipalContext> for IntrospectResponseDto {
    fn from(ctx: PrincipalContext) -> Self {
        IntrospectResponseDto {
            principal_prn: ctx.principal.principal_id.canonical(),
            status: ctx.principal.status.as_str().to_string(),
            issuer: ctx.principal.issuer.as_str().to_string(),
            subject: ctx.principal.subject,
            expires_at: ctx.principal.expires_at,
            memberships: ctx.memberships.into_iter().map(MembershipDto::from).collect(),
            role_grants: ctx.role_grants.into_iter().map(RoleGrantRefDto::from).collect(),
        }
    }
}

/// Body for `POST /v1/authz/is-authorized` (spec §9.1's `IsAuthorizedRequest` field-for-
/// field). `context` defaults to empty when the key is omitted entirely, matching proto3's
/// default-empty-map semantics for `map<string,string> context = 4`.
#[derive(Debug, Clone, Deserialize)]
pub struct IsAuthorizedBody {
    pub principal_prn: String,
    pub action: String,
    pub resource_prn: String,
    #[serde(default)]
    pub context: BTreeMap<String, String>,
}

/// Response for `POST /v1/authz/is-authorized` (spec §9.1's `IsAuthorizedResponse` field-
/// for-field). Only ever populated for a self/admin caller — see `http/authz.rs`'s module
/// docs for the exposure rule; a non-self, non-admin caller never reaches a successful
/// response at all (403 Forbidden, nothing returned).
#[derive(Debug, Clone, Serialize)]
pub struct IsAuthorizedResponseDto {
    pub allowed: bool,
    pub determining_policies: Vec<String>,
    pub reason: String,
}

/// Body for `POST /v1/authz/policies` (spec §9.1's `Policy` message minus the server-
/// computed `system` flag — a client-authored PUT can never mark a policy `system = true`;
/// `http/authz.rs`'s handler always sets `system = false` on the `PolicyDocument` it builds,
/// and the store separately rejects mutating an already-persisted system row).
#[derive(Debug, Clone, Deserialize)]
pub struct PutPolicyBody {
    pub policy_id: String,
    pub kind: String,
    pub source: String,
    pub description: String,
}

/// A `Policy`-shaped JSON entry (spec §9.1's proto `Policy` message field-for-field): both
/// `POST .../policies`'s response and each entry of `GET .../policies`'s list.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyDto {
    pub policy_id: String,
    pub kind: String,
    pub source: String,
    pub description: String,
    pub system: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<PolicyDocument> for PolicyDto {
    fn from(doc: PolicyDocument) -> Self {
        PolicyDto {
            policy_id: doc.policy_id,
            kind: match doc.kind {
                PolicyKind::Static => "static".to_string(),
                PolicyKind::Template => "template".to_string(),
            },
            source: doc.source,
            description: doc.description,
            system: doc.system,
            created_at: doc.created_at,
            updated_at: doc.updated_at,
        }
    }
}

/// Body for `POST /v1/authz/role-grants` (spec §9.1's `GrantRoleRequest` field-for-field).
#[derive(Debug, Clone, Deserialize)]
pub struct GrantRoleBody {
    pub principal_prn: String,
    pub role_key: String,
    pub scope_prn: String,
}

/// A `RoleGrant`-shaped JSON entry (spec §9.1's proto `RoleGrant` message field-for-field,
/// plus `created_at` — audit-useful context the proto message omits but the domain type
/// carries). `linked_policy_id` stays internal Cedar-wiring detail, never on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct RoleGrantDto {
    pub id: Uuid,
    pub principal_prn: String,
    pub role_key: String,
    pub scope_prn: String,
    pub created_at: DateTime<Utc>,
}

impl From<RoleGrant> for RoleGrantDto {
    fn from(g: RoleGrant) -> Self {
        RoleGrantDto {
            id: g.id,
            principal_prn: g.principal.canonical(),
            role_key: g.role_key,
            scope_prn: g.scope.canonical_prn(),
            created_at: g.created_at,
        }
    }
}

/// Query params for `GET /v1/authz/role-grants`: `principal_prn` is REQUIRED (unlike
/// `PageQuery`'s fields) — `RoleService::list` always lists exactly one principal's grants,
/// there is no list-everyone mode over HTTP. Kept `Option` here (rather than a bare
/// `String`) so a missing param maps through `http/authz.rs`'s own `TenancyError::InvalidPrn`
/// funnel — the same `{"error":{code,message}}` envelope every other validation error uses —
/// instead of axum's default plain-text query-rejection.
#[derive(Debug, Clone, Deserialize)]
pub struct RoleGrantQuery {
    pub principal_prn: Option<String>,
}
