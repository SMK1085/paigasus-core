// SPDX-License-Identifier: Apache-2.0

//! Wire DTOs for the `/v1` tenancy + authn HTTP API. Plain serde structs; the
//! `From<NodeView<_>>`/`From<PrincipalContext>` impls do the only real work — projecting a
//! domain value into the stable JSON shape (status fields as strings via `as_str`,
//! timestamps as RFC3339 via chrono's serde feature, PRNs as canonical strings).

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::{
    ApiKey, AuditEntry, Credential, DeadLetterEntry, MembershipRecord, NewApiKey, NodeStatus, NodeView, Organization, OrganizationId, PolicyDocument, PrincipalContext, Project, RoleGrant,
    RoleGrantRef, ServiceAccountRecord, Team,
};
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
        let principal = ctx.principal;
        // Token introspection only ever validates a JWT (`AuthenticateToken::introspect`
        // always resolves via the OIDC authenticator), so `ApiKey` is unreachable here —
        // Task 19 adds a dedicated api-key introspection path rather than extending this one.
        let (issuer, subject, expires_at) = match principal.credential {
            Credential::Oidc { issuer, subject, expires_at } => (issuer.as_str().to_string(), subject, expires_at),
            Credential::ApiKey { .. } => {
                debug_assert!(false, "token introspection resolved an ApiKey credential; only Oidc is reachable here");
                (String::new(), String::new(), Utc::now())
            }
        };
        IntrospectResponseDto {
            principal_prn: principal.principal_id.canonical(),
            status: principal.status.as_str().to_string(),
            issuer,
            subject,
            expires_at,
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

// --- SMA-445 Task 20: service-account + api-key DTOs ---------------------------------------

/// A `ServiceAccount`-shaped JSON entry (spec §10.2's proto `ServiceAccount` message,
/// field-for-field): `status` is the underlying `Principal`'s lifecycle status (D16: it lives
/// there, never on the `ServiceAccount` row itself) — `"active"`/`"disabled"` via
/// `PrincipalStatus::as_str`, built from the `ServiceAccountRecord` every read path
/// (`ServiceAccountService::create`/`get`/`list`) now hands back rather than a bare
/// `ServiceAccount`.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceAccountDto {
    pub prn: String,
    pub owner_prn: String,
    pub name: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceAccountRecord> for ServiceAccountDto {
    fn from(record: ServiceAccountRecord) -> Self {
        let sa = record.account;
        ServiceAccountDto {
            prn: sa.principal_id.canonical(),
            owner_prn: sa.owner.canonical(),
            name: sa.name,
            status: record.status.as_str().to_string(),
            created_at: sa.created_at,
            updated_at: sa.updated_at,
        }
    }
}

/// Body for `POST /v1/service-accounts` (spec §10.2's `CreateServiceAccountRequest`
/// field-for-field).
#[derive(Debug, Clone, Deserialize)]
pub struct CreateServiceAccountBody {
    pub owner_prn: String,
    pub name: String,
}

/// Query params for `GET /v1/service-accounts`: `owner_prn` is REQUIRED (mirrors
/// `RoleGrantQuery::principal_prn` — kept `Option` so a missing param funnels through the
/// `TenancyError::InvalidPrn` `{"error":{code,message}}` envelope instead of axum's default
/// plain-text query rejection).
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceAccountQuery {
    pub owner_prn: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// An `ApiKey`-shaped JSON entry (spec §10.2's proto `ApiKey` message field-for-field) — NEVER
/// carries a secret or hash: the domain `ApiKey` type structurally has neither field at all
/// (`application/api_keys.rs`'s module docs), so there is nothing for this projection to leak.
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyDto {
    pub id: Uuid,
    pub service_account_prn: String,
    pub scope_prn: String,
    pub prefix: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub scope_actions: Vec<String>,
    pub scope_roles: Vec<String>,
}

impl From<ApiKey> for ApiKeyDto {
    fn from(key: ApiKey) -> Self {
        ApiKeyDto {
            id: key.id.uuid(),
            service_account_prn: key.service_account_id.canonical(),
            scope_prn: key.scope.canonical(),
            prefix: key.prefix,
            status: key.status.as_str().to_string(),
            expires_at: key.expires_at,
            last_used_at: key.last_used_at,
            scope_actions: key.scope_actions.iter().map(|a| a.as_wire().to_string()).collect(),
            scope_roles: key.scope_roles,
        }
    }
}

/// Body for `POST /v1/service-accounts/{sa}/api-keys` (spec §10.2's `IssueApiKeyRequest`,
/// minus the path-carried `service_account_prn`). `scope_prn` is `Option` only so a missing
/// value funnels through `TenancyError::InvalidPrn` (mirrors `ServiceAccountQuery::owner_prn`)
/// rather than axum's default JSON-rejection body — it is semantically REQUIRED, exactly like
/// the proto's plain (non-`optional`) `string scope_prn` field. `expires_at` unset means
/// non-expiring (or the configured `default_expiry_days` fallback, `ApiKeyService::issue`).
#[derive(Debug, Clone, Deserialize)]
pub struct IssueApiKeyBody {
    pub scope_prn: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scope_actions: Vec<String>,
    #[serde(default)]
    pub scope_roles: Vec<String>,
}

/// Response for `POST /v1/service-accounts/{sa}/api-keys` (spec §10.2's `IssueApiKeyResponse`
/// field-for-field): the plaintext `token` is returned ONLY here, exactly once — never again
/// re-derivable, and never present on `ApiKeyDto`/any list entry (shown-once, spec D2).
#[derive(Debug, Clone, Serialize)]
pub struct IssueApiKeyResponseDto {
    pub api_key: ApiKeyDto,
    pub token: String,
}

impl From<NewApiKey> for IssueApiKeyResponseDto {
    fn from(new_key: NewApiKey) -> Self {
        IssueApiKeyResponseDto {
            api_key: new_key.key.into(),
            token: new_key.plaintext,
        }
    }
}

/// Body for `POST /v1/authn/api-keys/introspect` (spec §10.2's `IntrospectApiKeyRequest`) —
/// mirrors `IntrospectBody`; the token IS the credential and must never be logged (mirrors the
/// module docs on `http/authn.rs`'s `introspect`).
#[derive(Clone, Deserialize)]
pub struct IntrospectApiKeyRequestBody {
    pub token: String,
}

/// `IntrospectApiKeyResponse`-shaped JSON (spec §10.2's proto message field-for-field):
/// unlike `IntrospectResponseDto` (OIDC-only: `issuer`/`subject`), an API-key-authenticated
/// principal has no issuer/subject — `key_id` takes their place, mirroring the proto shape
/// exactly (`IntrospectApiKeyResponse.key_id`). `scope_prn` (SMA-446) carries the key's tenancy
/// scope for the gateway to authorize against — threaded off the credential, so a cache HIT
/// supplies it with NO extra DB read (D11).
#[derive(Debug, Clone, Serialize)]
pub struct IntrospectApiKeyResponseDto {
    pub principal_prn: String,
    pub status: String,
    pub key_id: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub memberships: Vec<MembershipDto>,
    pub role_grants: Vec<RoleGrantRefDto>,
    pub scope_prn: String,
}

impl From<PrincipalContext> for IntrospectApiKeyResponseDto {
    fn from(ctx: PrincipalContext) -> Self {
        let principal = ctx.principal;
        // API-key introspection always resolves via `AuthenticateApiKey::introspect`, which
        // only ever produces a `Credential::ApiKey` — mirrors `IntrospectResponseDto`'s own
        // debug_assert for its (opposite) unreachable arm.
        let (key_id, expires_at, scope_prn) = match principal.credential {
            Credential::ApiKey { key_id, expires_at, scope_prn } => (key_id.to_string(), expires_at, scope_prn),
            Credential::Oidc { .. } => {
                debug_assert!(false, "api-key introspection resolved an Oidc credential; only ApiKey is reachable here");
                (String::new(), None, String::new())
            }
        };
        IntrospectApiKeyResponseDto {
            principal_prn: principal.principal_id.canonical(),
            status: principal.status.as_str().to_string(),
            key_id,
            expires_at,
            memberships: ctx.memberships.into_iter().map(MembershipDto::from).collect(),
            role_grants: ctx.role_grants.into_iter().map(RoleGrantRefDto::from).collect(),
            scope_prn,
        }
    }
}

// --- SMA-446 Task A11: `GET /v1/audit` DTOs -------------------------------------------------

/// A domain [`AuditEntry`] projected into its HTTP wire shape (mirrors
/// `grpc::audit::to_proto_entry`'s field-for-field mapping): unlike the gRPC wire's
/// `detail_json` (a stringified blob — proto has no native JSON type), `detail` here stays a
/// real `serde_json::Value` — JSON has no such limitation, so there is no reason to add the
/// string-encoding indirection on this transport.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntryDto {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_prn: Option<String>,
    pub action: String,
    pub resource_prn: Option<String>,
    pub outcome: String,
    pub determining_policies: Vec<String>,
    pub detail: serde_json::Value,
    pub correlation_id: Option<Uuid>,
}

impl From<AuditEntry> for AuditEntryDto {
    fn from(e: AuditEntry) -> Self {
        AuditEntryDto {
            id: e.id,
            occurred_at: e.occurred_at,
            actor_prn: e.actor_prn,
            action: e.action,
            resource_prn: e.resource_prn,
            outcome: e.outcome.as_str().to_string(),
            determining_policies: e.determining_policies,
            detail: e.detail,
            correlation_id: e.correlation_id,
        }
    }
}

/// Response for `GET /v1/audit` (spec-equivalent of the gRPC `ListAuditEntriesResponse`):
/// `next_cursor` is present only when the page came back FULL (`http::audit::list`'s own doc)
/// — HTTP's native `Option`/absent-key sentinel standing in for the gRPC wire's empty-string
/// one.
#[derive(Debug, Clone, Serialize)]
pub struct AuditListResponseDto {
    pub entries: Vec<AuditEntryDto>,
    pub next_cursor: Option<String>,
}

/// Query params for `GET /v1/audit` (SMA-446 Task A11): every field optional. `from`/`to` are
/// RFC3339 timestamp strings. Kept as raw `Option<String>` (rather than e.g. `Option<DateTime
/// <Utc>>`/a typed enum) so a parse failure funnels through `http::audit::to_filter`'s
/// `TenancyError::InvalidPrn` `{"error":{code,message}}` envelope — mirrors
/// `RoleGrantQuery`/`ServiceAccountQuery`'s identical "keep it a string, let the handler
/// validate" posture for their own required fields.
#[derive(Debug, Clone, Deserialize)]
pub struct AuditQuery {
    pub actor: Option<String>,
    pub resource: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

// --- SMA-469: `/v1/outbox/dead-letters` DTOs --------------------------------------------------

/// A parked outbox row over HTTP (SMA-469). `payload` is emitted as a JSON **string** — it is
/// the raw serialized TEXT exactly as stored, deliberately NOT re-parsed into a
/// `serde_json::Value`: invalid payload JSON is one of the reasons a row parks, so a surface
/// that could only render valid JSON could not display the rows it exists to explain.
#[derive(Debug, Clone, Serialize)]
pub struct DeadLetterEntryDto {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,
    pub correlation_id: Option<Uuid>,
    pub attempts: u32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl From<DeadLetterEntry> for DeadLetterEntryDto {
    fn from(e: DeadLetterEntry) -> Self {
        DeadLetterEntryDto {
            id: e.id,
            occurred_at: e.occurred_at,
            event_type: e.event_type,
            schema_version: e.schema_version,
            aggregate_prn: e.aggregate_prn,
            actor_prn: e.actor_prn,
            payload: e.payload,
            correlation_id: e.correlation_id,
            attempts: e.attempts,
            parked_at: e.parked_at,
            last_error: e.last_error,
        }
    }
}

/// `next_cursor` is present only when the page came back FULL, mirroring `AuditListResponseDto`.
#[derive(Debug, Clone, Serialize)]
pub struct DeadLetterListResponseDto {
    pub entries: Vec<DeadLetterEntryDto>,
    pub next_cursor: Option<String>,
}

/// Query params for `GET /v1/outbox/dead-letters`. Timestamps stay raw `Option<String>` so a
/// parse failure funnels through the handler's `{"error":{code,message}}` envelope, mirroring
/// `AuditQuery`'s identical posture.
#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterQuery {
    pub event_type: Option<String>,
    pub parked_from: Option<String>,
    pub parked_to: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// Body for the bulk `POST /v1/outbox/dead-letters/replay`. `max_rows` is `Option` on the wire
/// so an omitted field is distinguishable from an explicit `0` — both are rejected, but the
/// type must be able to represent "absent" to reject it deliberately rather than defaulting.
#[derive(Debug, Clone, Deserialize)]
pub struct BulkReplayBody {
    pub event_type: Option<String>,
    pub parked_from: Option<String>,
    pub parked_to: Option<String>,
    pub max_rows: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkReplayResponseDto {
    pub replayed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::{ApiKeyId, AuthnPrincipal, PrincipalId, PrincipalKind, PrincipalStatus};
    use paigasus_kernel::Prn;

    /// SMA-446: the HTTP `IntrospectApiKeyResponseDto` surfaces the credential's `scope_prn`
    /// (D11), and — paired with `convert.rs`'s twin gRPC test — deterministically guarantees the
    /// two wire shapes agree without PG/Redis. Builds a `PrincipalContext` carrying a known scope
    /// and asserts the `From` projection echoes it.
    #[test]
    fn introspect_api_key_dto_carries_scope_prn() {
        let scope_prn = "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000042".to_string();
        let principal_id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(42)).unwrap());
        let ctx = PrincipalContext {
            principal: AuthnPrincipal {
                principal_id,
                kind: PrincipalKind::ServiceAccount,
                status: PrincipalStatus::Active,
                credential: Credential::ApiKey {
                    key_id: ApiKeyId::from_uuid(Uuid::from_u128(7)),
                    expires_at: None,
                    scope_prn: scope_prn.clone(),
                },
            },
            memberships: Vec::new(),
            role_grants: Vec::new(),
        };

        let dto = IntrospectApiKeyResponseDto::from(ctx);
        assert_eq!(dto.scope_prn, scope_prn, "the HTTP introspect DTO must carry the key's scope_prn (SMA-446, D11)");
    }
}
