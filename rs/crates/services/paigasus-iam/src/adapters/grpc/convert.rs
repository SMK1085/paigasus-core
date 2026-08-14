// SPDX-License-Identifier: Apache-2.0

//! Domain <-> proto conversions and the shared gRPC helpers (`status_to_grpc`, `node_uuid`,
//! `to_page`) every `TenancyGrpc` method uses: parse -> service call -> convert, no business
//! logic in this layer (task-16 brief).

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::{
    ApiKey, ApiKeyStatus, AuthnError, Credential, MembershipRecord, NewApiKey, NodeStatus, NodeView, Organization, OrganizationId, PolicyDocument, PrincipalContext, Project, RoleGrant, RoleGrantRef,
    ServiceAccountRecord, Team,
};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::common::v1::AuditMetadata;
use paigasus_proto::paigasus::iam::v1::{
    ApiKey as ProtoApiKey, ApiKeyStatus as ProtoApiKeyStatus, IntrospectApiKeyResponse, IntrospectResponse, IssueApiKeyResponse, Membership, NodeStatus as ProtoNodeStatus,
    Organization as ProtoOrganization, Policy as ProtoPolicy, Project as ProtoProject, RoleGrant as ProtoRoleGrant, RoleGrantRef as ProtoRoleGrantRef, ServiceAccount as ProtoServiceAccount,
    Team as ProtoTeam,
};
use tonic::{Code, Status};
use uuid::Uuid;

use crate::application::error::{ErrorClass, TenancyError};
use crate::application::pagination::Page;

/// Maps a `TenancyError` to a `tonic::Status`: the gRPC code follows `ErrorClass`; the message
/// is `"{code}: {display}"` — the stable kebab-case code (`TenancyError::code`) stays
/// machine-readable in-band, since tonic has no structured-error-detail convention by default
/// (task-16 brief). `Internal`'s `Display` never carries interpolated data (D7), so this never
/// leaks backend detail either.
pub fn status_to_grpc(e: TenancyError) -> Status {
    let code = match e.class() {
        ErrorClass::Validation => Code::InvalidArgument,
        ErrorClass::NotFound => Code::NotFound,
        ErrorClass::Conflict => Code::AlreadyExists,
        ErrorClass::Precondition => Code::FailedPrecondition,
        ErrorClass::Forbidden => Code::PermissionDenied,
        ErrorClass::Internal => {
            tracing::error!(error = %e, code = e.code(), "internal error handling gRPC request");
            Code::Internal
        }
    };
    Status::new(code, format!("{}: {}", e.code(), e))
}

/// Maps an `AuthnError` to a `tonic::Status` for the gRPC authn surface (spec §6.3, D12).
/// Deliberately SEPARATE from the tenancy `status_to_grpc`: authn needs `Unauthenticated`,
/// `PermissionDenied`, `Unavailable`, and `Internal`, none of which the tenancy `ErrorClass`
/// expresses. Every message is STATIC per code — no token, claim, or upstream error text
/// ever reaches the wire (mirrors the HTTP `AuthnApiError` funnel). The enforcement layer
/// renders the returned `Status` as a trailers-only gRPC response via `Status::into_http`;
/// the `Introspect` handler returns it directly.
pub fn authn_status(err: &AuthnError) -> Status {
    let (code, message) = match err {
        AuthnError::InvalidToken(_) => (Code::Unauthenticated, "invalid bearer token"),
        AuthnError::IdentityNotProvisioned => (Code::PermissionDenied, "identity not provisioned"),
        AuthnError::ProvisioningFailed(_) => (Code::PermissionDenied, "provisioning failed"),
        AuthnError::PrincipalInactive => (Code::PermissionDenied, "principal inactive"),
        AuthnError::Unavailable => (Code::Unavailable, "authentication backend unavailable"),
        AuthnError::Backend(_) => {
            // `Debug` carries the boxed repository/infra source (never token or claim
            // material, by `AuthnError`'s own contract) — logged here, never surfaced.
            tracing::error!(error = ?err, "internal error handling a gRPC authn request");
            (Code::Internal, "internal error")
        }
    };
    Status::new(code, message)
}

/// Parses a wire PRN, requiring the `"iam"` service and an `expect`ed resource type. Returns
/// the resource uuid and the PRN's own canonical string. A syntactically invalid PRN maps
/// through the kernel's stable error-kind token; a well-formed PRN of the wrong service/type
/// carries its canonical form instead (mirrors `application::memberships`'s PRN parsing).
///
/// The returned canonical is compared by every Get/Rename/Archive/Restore handler against the
/// service's stored canonical PRN after the call — the forged-org-slot defense (brief rule 8,
/// mirroring the HTTP layer's semantics via stored-PRN comparison).
pub fn node_uuid(prn: &str, expect: &str) -> Result<(Uuid, String), Status> {
    let parsed = Prn::parse(prn).map_err(|e| status_to_grpc(TenancyError::InvalidPrn(e.kind().to_owned())))?;
    if parsed.service() != "iam" || parsed.resource_type() != expect {
        return Err(status_to_grpc(TenancyError::InvalidPrn(parsed.canonical())));
    }
    Ok((parsed.resource_id(), parsed.canonical()))
}

/// Builds a validated `Page` from the wire's `limit`/`offset`: `limit == 0` means "server
/// default" (proto comment: `limit 0 => server default 50`), so it maps to `None`; any other
/// value is an explicit request and is passed through for `Page::new` to bounds-check. Unlike
/// the HTTP query-param surface (where an *absent* limit is `None` and an explicit `0` is
/// rejected), the wire has no way to distinguish "absent" from "zero" — `uint32` — so `0` is
/// read as "unset" here (task-16 brief).
pub fn to_page(limit: u32, offset: u64) -> Result<Page, TenancyError> {
    let limit = if limit == 0 { None } else { Some(i64::from(limit)) };
    Page::new(limit, Some(offset as i64))
}

/// Builds a `prost_types::Timestamp` from a `chrono::DateTime<Utc>`.
pub fn ts(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

/// Builds a `chrono::DateTime<Utc>` from a `prost_types::Timestamp` — the inverse of [`ts`],
/// needed by `IssueApiKeyRequest.expires_at` (SMA-445 Task 21, the first wire request carrying
/// a caller-supplied, rather than server-produced, timestamp). `None` for an out-of-range
/// value (a negative `nanos`, or a `seconds`/`nanos` pair `chrono` can't represent) rather than
/// panicking — callers map that to a client error themselves (mirrors `node_uuid`'s own
/// "caller decides how to surface a parse failure" posture).
pub fn from_ts(t: prost_types::Timestamp) -> Option<DateTime<Utc>> {
    let nanos = u32::try_from(t.nanos).ok()?;
    DateTime::<Utc>::from_timestamp(t.seconds, nanos)
}

/// Builds `AuditMetadata` from created/modified timestamps. `created_by`/`modified_by` stay
/// empty until M2 wires an actor through the request context (task-16 brief).
pub fn audit(created: DateTime<Utc>, updated: DateTime<Utc>) -> AuditMetadata {
    AuditMetadata {
        created_at: Some(ts(created)),
        modified_at: Some(ts(updated)),
        created_by: String::new(),
        modified_by: String::new(),
    }
}

fn to_proto_status(s: NodeStatus) -> i32 {
    match s {
        NodeStatus::Active => ProtoNodeStatus::Active as i32,
        NodeStatus::Archived => ProtoNodeStatus::Archived as i32,
    }
}

/// Projects an organization view into its wire message.
pub fn to_proto_org(v: &NodeView<Organization>) -> ProtoOrganization {
    ProtoOrganization {
        prn: v.node.id.canonical(),
        slug: v.node.slug.as_str().to_owned(),
        name: v.node.name.clone(),
        status: to_proto_status(v.node.status),
        effective_status: to_proto_status(v.effective_status),
        audit: Some(audit(v.node.created_at, v.node.updated_at)),
    }
}

/// Projects a team view into its wire message.
pub fn to_proto_team(v: &NodeView<Team>) -> ProtoTeam {
    ProtoTeam {
        prn: v.node.id.canonical(),
        org_prn: OrganizationId::from_uuid(v.node.id.org_uuid()).canonical(),
        slug: v.node.slug.as_str().to_owned(),
        name: v.node.name.clone(),
        status: to_proto_status(v.node.status),
        effective_status: to_proto_status(v.effective_status),
        audit: Some(audit(v.node.created_at, v.node.updated_at)),
    }
}

/// Projects a project view into its wire message.
pub fn to_proto_project(v: &NodeView<Project>) -> ProtoProject {
    ProtoProject {
        prn: v.node.id.canonical(),
        team_prn: v.node.team_id.canonical(),
        org_prn: OrganizationId::from_uuid(v.node.id.org_uuid()).canonical(),
        slug: v.node.slug.as_str().to_owned(),
        name: v.node.name.clone(),
        status: to_proto_status(v.node.status),
        effective_status: to_proto_status(v.effective_status),
        audit: Some(audit(v.node.created_at, v.node.updated_at)),
    }
}

/// Projects a membership record into its wire message. Memberships are immutable (D5), so
/// `modified_at` mirrors `created_at`.
pub fn to_proto_membership(r: &MembershipRecord) -> Membership {
    Membership {
        id: r.id.to_string(),
        principal_prn: r.principal_prn.clone(),
        node_prn: r.node_prn.clone(),
        audit: Some(audit(r.created_at, r.created_at)),
    }
}

/// Projects a core `RoleGrantRef` into its wire message: a direct field-for-field mapping
/// (both carry `scope_prn`/`role_key` already as plain strings — no PRN parsing needed here).
pub fn to_proto_role_grant_ref(r: &RoleGrantRef) -> ProtoRoleGrantRef {
    ProtoRoleGrantRef {
        scope_prn: r.scope_prn.clone(),
        role_key: r.role_key.clone(),
    }
}

/// Projects an authored `PolicyDocument` into its wire `Policy` message (SMA-444 Task 19):
/// `kind` as its stable lowercase string (mirrors `adapters::http::dto::PolicyDto`'s `From`
/// impl) — `created_at`/`updated_at` have no proto field (the `Policy` message carries none,
/// unlike `RoleGrantDto`'s HTTP-only `created_at`).
pub fn to_proto_policy(doc: &PolicyDocument) -> ProtoPolicy {
    ProtoPolicy {
        policy_id: doc.policy_id.clone(),
        kind: match doc.kind {
            PolicyKind::Static => "static".to_string(),
            PolicyKind::Template => "template".to_string(),
        },
        source: doc.source.clone(),
        description: doc.description.clone(),
        system: doc.system,
    }
}

/// Projects a core `RoleGrant` into its wire `RoleGrant` message (SMA-444 Task 19):
/// `principal_prn`/`scope_prn` as canonical PRN strings — mirrors
/// `adapters::http::dto::RoleGrantDto`'s `From` impl, minus its HTTP-only `created_at` (the
/// proto `RoleGrant` message carries no timestamp).
pub fn to_proto_role_grant(g: &RoleGrant) -> ProtoRoleGrant {
    ProtoRoleGrant {
        id: g.id.to_string(),
        principal_prn: g.principal.canonical(),
        role_key: g.role_key.clone(),
        scope_prn: g.scope.canonical_prn(),
    }
}

/// Projects a `PrincipalContext` into the wire `IntrospectResponse` (spec §7.2/§7.3): PRN
/// strings, principal status as its stable `as_str`, `expires_at` as a prost `Timestamp`,
/// memberships via the shared tenancy `Membership` mapping, and `role_grants` from the
/// core's structured role-grant refs — always empty until a later M3 task populates it (D4).
pub fn to_introspect_response(ctx: &PrincipalContext) -> IntrospectResponse {
    // Token introspection only ever validates a JWT (`AuthenticateToken::introspect` always
    // resolves via the OIDC authenticator), so `ApiKey` is unreachable here — Task 19 adds a
    // dedicated api-key introspection path rather than extending this one.
    let (issuer, subject, expires_at) = match &ctx.principal.credential {
        Credential::Oidc { issuer, subject, expires_at } => (issuer.as_str().to_string(), subject.clone(), *expires_at),
        Credential::ApiKey { .. } => {
            debug_assert!(false, "token introspection resolved an ApiKey credential; only Oidc is reachable here");
            (String::new(), String::new(), Utc::now())
        }
    };
    IntrospectResponse {
        principal_prn: ctx.principal.principal_id.canonical(),
        status: ctx.principal.status.as_str().to_string(),
        issuer,
        subject,
        expires_at: Some(ts(expires_at)),
        memberships: ctx.memberships.iter().map(to_proto_membership).collect(),
        role_grants: ctx.role_grants.iter().map(to_proto_role_grant_ref).collect(),
    }
}

fn to_proto_api_key_status(s: ApiKeyStatus) -> i32 {
    match s {
        ApiKeyStatus::Active => ProtoApiKeyStatus::Active as i32,
        ApiKeyStatus::Revoked => ProtoApiKeyStatus::Revoked as i32,
    }
}

/// Projects a service account into its wire message (SMA-445 Task 21). `status` is the
/// underlying `Principal`'s lifecycle status (D16: it lives there, never on the
/// `ServiceAccount` row itself — mirrors `http::dto::ServiceAccountDto`'s identical doc),
/// built from the `ServiceAccountRecord` every read path (`ServiceAccountService::create`/
/// `get`/`list`) now hands back rather than a bare `ServiceAccount`.
pub fn to_proto_service_account(record: &ServiceAccountRecord) -> ProtoServiceAccount {
    let sa = &record.account;
    ProtoServiceAccount {
        prn: sa.principal_id.canonical(),
        owner_prn: sa.owner.canonical(),
        name: sa.name.clone(),
        status: record.status.as_str().to_string(),
        audit: Some(audit(sa.created_at, sa.updated_at)),
    }
}

/// Projects an API key into its wire message (SMA-445 Task 21). NEVER carries a secret/hash —
/// `ApiKey` structurally has neither field (mirrors `http::dto::ApiKeyDto`'s identical doc).
/// `audit.modified_at` is `revoked_at` when the key has been revoked, else `created_at` — an
/// API key has no generic `updated_at` of its own (unlike a tenancy node), so revocation is the
/// only state transition worth surfacing as "modified" (mirrors `to_proto_membership`'s
/// immutable-record posture, generalized to the one mutation this entity does have).
pub fn to_proto_api_key(key: &ApiKey) -> ProtoApiKey {
    ProtoApiKey {
        id: key.id.uuid().to_string(),
        service_account_prn: key.service_account_id.canonical(),
        scope_prn: key.scope.canonical(),
        prefix: key.prefix.clone(),
        status: to_proto_api_key_status(key.status),
        expires_at: key.expires_at.map(ts),
        last_used_at: key.last_used_at.map(ts),
        scope_actions: key.scope_actions.iter().map(|a| a.as_wire().to_string()).collect(),
        scope_roles: key.scope_roles.clone(),
        audit: Some(audit(key.created_at, key.revoked_at.unwrap_or(key.created_at))),
    }
}

/// Projects a freshly minted `NewApiKey` into its wire `IssueApiKeyResponse` (SMA-445 Task 21,
/// spec §10.1): the plaintext `token` shown exactly once (D2), mirroring
/// `http::dto::IssueApiKeyResponseDto`'s identical `From` impl.
pub fn to_proto_issue_api_key_response(new_key: &NewApiKey) -> IssueApiKeyResponse {
    IssueApiKeyResponse {
        api_key: Some(to_proto_api_key(&new_key.key)),
        token: new_key.plaintext.clone(),
    }
}

/// Projects a `PrincipalContext` resolved via an API key into the wire
/// `IntrospectApiKeyResponse` (SMA-445 Task 21, spec §10.1) — the API-key peer of
/// [`to_introspect_response`]: `key_id` takes the place of `issuer`/`subject` (an
/// API-key-authenticated principal has neither), mirroring
/// `http::dto::IntrospectApiKeyResponseDto`'s identical `From` impl. `scope_prn` (SMA-446)
/// carries the key's tenancy scope straight off the credential — a cache HIT supplies it with
/// NO extra DB read (D11), so the gateway can authorize `InvokeModel` against it.
pub fn to_introspect_api_key_response(ctx: &PrincipalContext) -> IntrospectApiKeyResponse {
    // API-key introspection always resolves via `AuthenticateApiKey::introspect`, which only
    // ever produces a `Credential::ApiKey` — mirrors `to_introspect_response`'s own (opposite)
    // unreachable-arm debug_assert.
    let (key_id, expires_at, scope_prn) = match &ctx.principal.credential {
        Credential::ApiKey { key_id, expires_at, scope_prn } => (key_id.to_string(), *expires_at, scope_prn.clone()),
        Credential::Oidc { .. } => {
            debug_assert!(false, "api-key introspection resolved an Oidc credential; only ApiKey is reachable here");
            (String::new(), None, String::new())
        }
    };
    IntrospectApiKeyResponse {
        principal_prn: ctx.principal.principal_id.canonical(),
        status: ctx.principal.status.as_str().to_string(),
        key_id,
        expires_at: expires_at.map(ts),
        memberships: ctx.memberships.iter().map(to_proto_membership).collect(),
        role_grants: ctx.role_grants.iter().map(to_proto_role_grant_ref).collect(),
        scope_prn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_maps_to_permission_denied() {
        let status = status_to_grpc(TenancyError::Forbidden);
        assert_eq!(status.code(), Code::PermissionDenied);
        // Message stays "{code}: {display}", and `Forbidden`'s Display is static (SMA-444
        // task-16 brief) — no denying-policy detail ever reaches the wire.
        assert_eq!(status.message(), "forbidden: access denied");
    }

    #[test]
    fn not_found_maps_to_grpc_not_found() {
        let status = status_to_grpc(TenancyError::NotFound);
        assert_eq!(status.code(), Code::NotFound);
    }

    /// SMA-446: `to_introspect_api_key_response` surfaces the credential's `scope_prn` on the
    /// wire (D11 — the gateway authorizes `InvokeModel` against it). Deterministic, no PG/Redis:
    /// builds a `PrincipalContext` carrying a known scope and asserts the mapper echoes it —
    /// with `dto.rs`'s twin test, this guarantees the gRPC/HTTP wire shapes agree.
    #[test]
    fn introspect_api_key_response_carries_scope_prn() {
        use paigasus_iam_core::{ApiKeyId, AuthnPrincipal, PrincipalId, PrincipalKind, PrincipalStatus};

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

        let resp = to_introspect_api_key_response(&ctx);
        assert_eq!(resp.scope_prn, scope_prn, "the gRPC introspect response must carry the key's scope_prn (SMA-446, D11)");
    }

    /// AC2: every code `TenancyError::code()` can return is declared in the canonical registry
    /// (`contracts/proto/paigasus/common/v1/error.proto`, SMA-498).
    ///
    /// `assert_variant_is_known` is a wildcard-free match whose only job is to fail COMPILATION when
    /// a variant is added to `TenancyError` without being added to `all` below — and therefore
    /// without being registered. That closes the gap a plain hand-written list leaves open, without
    /// waiting for SMA-507's drift gate. Assertions go through `code()` rather than string literals,
    /// so a rename that is not registered fails too.
    ///
    /// It is CALLED in the loop rather than left as an unused `_`-prefixed helper: the workspace
    /// lints are `warnings = deny`, so an uncalled function risks a hard `dead_code` failure.
    #[test]
    fn every_tenancy_code_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        fn assert_variant_is_known(e: &TenancyError) {
            match e {
                TenancyError::SlugConflict
                | TenancyError::DuplicateMembership
                | TenancyError::EmailConflict
                | TenancyError::ServiceAccountNameConflict
                | TenancyError::InvalidEmail(_)
                | TenancyError::InvalidSlug(_)
                | TenancyError::InvalidName(_)
                | TenancyError::InvalidPrn(_)
                | TenancyError::PrnMismatch
                | TenancyError::InvalidPagination
                | TenancyError::NothingToRename
                | TenancyError::NotFound
                | TenancyError::ParentArchived
                | TenancyError::NodeArchived
                | TenancyError::MissingOrgMembership
                | TenancyError::Forbidden
                | TenancyError::UnknownRole(_)
                | TenancyError::InvalidScope(_)
                | TenancyError::SystemImmutable(_)
                | TenancyError::PolicyInvalid(_)
                | TenancyError::PolicyConflict(_)
                | TenancyError::InvalidAction(_)
                | TenancyError::InvalidBulkReplay
                | TenancyError::NotSystemOwned(_)
                | TenancyError::FleetNotConverged
                | TenancyError::Internal => {}
            }
        }

        let s = || "x".to_string();
        let all = [
            TenancyError::SlugConflict,
            TenancyError::DuplicateMembership,
            TenancyError::EmailConflict,
            TenancyError::ServiceAccountNameConflict,
            TenancyError::InvalidEmail(s()),
            TenancyError::InvalidSlug(s()),
            TenancyError::InvalidName(s()),
            TenancyError::InvalidPrn(s()),
            TenancyError::PrnMismatch,
            TenancyError::InvalidPagination,
            TenancyError::NothingToRename,
            TenancyError::NotFound,
            TenancyError::ParentArchived,
            TenancyError::NodeArchived,
            TenancyError::MissingOrgMembership,
            TenancyError::Forbidden,
            TenancyError::UnknownRole(s()),
            TenancyError::InvalidScope(s()),
            TenancyError::SystemImmutable(s()),
            TenancyError::PolicyInvalid(s()),
            TenancyError::PolicyConflict(s()),
            TenancyError::InvalidAction(s()),
            TenancyError::InvalidBulkReplay,
            TenancyError::NotSystemOwned(s()),
            TenancyError::FleetNotConverged,
            TenancyError::Internal,
        ];
        assert_eq!(all.len(), 26, "TenancyError has 26 variants; update `all` and the match together");

        for err in &all {
            assert_variant_is_known(err);
            let code = err.code();
            assert!(
                ErrorReason::from_wire_reason(code).is_some(),
                "TenancyError::{err:?} emits {code:?}, which is not declared in common/v1/error.proto"
            );
        }
    }
}
