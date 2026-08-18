// SPDX-License-Identifier: Apache-2.0

//! Domain <-> proto conversions and the shared gRPC helpers (`status_to_grpc`, `node_uuid`,
//! `to_page`) every `TenancyGrpc` method uses: parse -> service call -> convert, no business
//! logic in this layer (task-16 brief).

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::{
    ApiKey, ApiKeyStatus, AuthnError, Credential, MembershipRecord, NewApiKey, NodeStatus, NodeView, Organization, OrganizationId, PolicyDocument, PrincipalContext, Project, RoleGrant, RoleGrantRef,
    ServiceAccountRecord, Team,
};
use paigasus_kernel::Prn;
use paigasus_observability::{Retryable, current_ids};
use paigasus_proto::error::IAM_DOMAIN;
use paigasus_proto::paigasus::common::v1::{AuditMetadata, ErrorReason};
use paigasus_proto::paigasus::iam::v1::{
    ApiKey as ProtoApiKey, ApiKeyStatus as ProtoApiKeyStatus, IntrospectApiKeyResponse, IntrospectResponse, IssueApiKeyResponse, Membership, NodeStatus as ProtoNodeStatus,
    Organization as ProtoOrganization, Policy as ProtoPolicy, Project as ProtoProject, RoleGrant as ProtoRoleGrant, RoleGrantRef as ProtoRoleGrantRef, ServiceAccount as ProtoServiceAccount,
    Team as ProtoTeam,
};
use tonic::{Code, Status};
use tonic_types::{ErrorDetails, StatusExt};
use uuid::Uuid;

use crate::adapters::retryable::{authn_retryable, tenancy_retryable};
use crate::application::error::{ErrorClass, TenancyError};
use crate::application::pagination::Page;

/// Derived from the registry, not written as a literal, and hoisted because `as_wire_reason`
/// allocates on every call (ADR-0019 D8).
static MISSING_AUTH_CONTEXT: LazyLock<String> = LazyLock::new(|| ErrorReason::MissingAuthContext.as_wire_reason().expect("a declared reason is never the sentinel"));
static CAPABILITY_DISABLED: LazyLock<String> = LazyLock::new(|| ErrorReason::CapabilityDisabled.as_wire_reason().expect("a declared reason is never the sentinel"));

/// `authn_status`'s six reasons, same registry-derived pattern as the two statics above
/// (D8): every one of these already exists in the registry (spec §6.3), so — per the human's
/// ruling on review finding #2 — they are derived, not hardcoded, even though the brief handed
/// them to us as string literals.
static INVALID_TOKEN: LazyLock<String> = LazyLock::new(|| ErrorReason::InvalidToken.as_wire_reason().expect("a declared reason is never the sentinel"));
static IDENTITY_NOT_PROVISIONED: LazyLock<String> = LazyLock::new(|| ErrorReason::IdentityNotProvisioned.as_wire_reason().expect("a declared reason is never the sentinel"));
static PROVISIONING_FAILED: LazyLock<String> = LazyLock::new(|| ErrorReason::ProvisioningFailed.as_wire_reason().expect("a declared reason is never the sentinel"));
static PRINCIPAL_INACTIVE: LazyLock<String> = LazyLock::new(|| ErrorReason::PrincipalInactive.as_wire_reason().expect("a declared reason is never the sentinel"));
static AUTHN_UNAVAILABLE: LazyLock<String> = LazyLock::new(|| ErrorReason::AuthnUnavailable.as_wire_reason().expect("a declared reason is never the sentinel"));
static AUTHN_INTERNAL: LazyLock<String> = LazyLock::new(|| ErrorReason::Internal.as_wire_reason().expect("a declared reason is never the sentinel"));

/// The `ErrorInfo.metadata` every IAM gRPC error carries.
///
/// The id keys are OMITTED when there is no request scope (§4.3 — unit tests, background tasks
/// and response-body streaming) rather than filled with a nil UUID that would read as a real id.
fn error_metadata(retryable: Retryable, extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert("retryable".to_owned(), retryable.as_wire().to_owned());
    if let Some(ids) = current_ids() {
        metadata.insert("correlation_id".to_owned(), ids.correlation_id.to_string());
        metadata.insert("request_id".to_owned(), ids.request_id.to_string());
    }
    for (k, v) in extra {
        metadata.insert((*k).to_owned(), (*v).to_owned());
    }
    metadata
}

/// Builds a `Status` carrying `google.rpc.ErrorInfo` in the `grpc-status-details-bin` trailer.
/// The single construction point for every IAM gRPC error, so no site can forget the details.
pub fn iam_status(code: Code, reason: &str, message: impl Into<String>, retryable: Retryable, extra: &[(&str, &str)]) -> Status {
    let details = ErrorDetails::with_error_info(reason, &*IAM_DOMAIN, error_metadata(retryable, extra));
    Status::with_error_details(code, message, details)
}

/// The enforcement layer admitted a request without attaching an authenticated context — an
/// internal invariant violation, surfaced as a distinct diagnostic rather than a bare
/// unauthenticated with no machine code at all. `Retryable::No`, not `Unknown` (review finding
/// #4): D4's `Unknown` is for a source ERASED at conversion (a Postgres blip and a logic bug
/// arriving as the same `Internal` variant, indistinguishable to the caller) — that reasoning
/// doesn't apply here. We know exactly what happened (the layer never attached a context), and
/// retrying the identical request cannot resolve it.
pub fn missing_auth_context() -> Status {
    iam_status(Code::Unauthenticated, &MISSING_AUTH_CONTEXT, "missing authentication context", Retryable::No, &[])
}

/// An RPC belonging to a capability this deployment has switched off. The capability NAME rides
/// in metadata rather than in the reason, so a new capability needs no new registry value.
pub fn capability_disabled(capability: &str) -> Status {
    iam_status(
        Code::Unimplemented,
        &CAPABILITY_DISABLED,
        format!("capability {capability} is not enabled on this service"),
        Retryable::No,
        &[("capability", capability)],
    )
}

/// Maps a `TenancyError` to a `tonic::Status`: the gRPC code follows `ErrorClass`, the message is
/// purely human-readable, and the machine-readable `(domain, reason)` rides in `ErrorInfo` in the
/// `grpc-status-details-bin` trailer (ADR-0019 decision 4). The old `"{code}: {display}"` prefix
/// is GONE — clients read `ErrorInfo.reason`, never the message. `Internal`'s `Display` never
/// carries interpolated data (D7), so this never leaks backend detail either.
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
    // `e.code()` IS the canonical wire string — the registry is the validation (see the
    // `every_tenancy_code_is_declared_in_the_canonical_registry` test), not the transform.
    iam_status(code, e.code(), e.to_string(), tenancy_retryable(e.class()), &[])
}

/// Maps an `AuthnError` to a `tonic::Status` for the gRPC authn surface (spec §6.3, D12).
/// Deliberately SEPARATE from the tenancy `status_to_grpc`: authn needs `Unauthenticated`,
/// `PermissionDenied`, `Unavailable` and `Internal`, none of which `ErrorClass` expresses. Every
/// message is STATIC per code and unchanged by SMA-504 — no token, claim or upstream error text
/// ever reaches the wire. What IS new is the machine-readable reason: the gateway previously had
/// to accept a bare `PermissionDenied`, which collapsed three variants (ADR-0020 D4's tripwire).
pub fn authn_status(err: &AuthnError) -> Status {
    let (code, reason, message) = match err {
        AuthnError::InvalidToken(_) => (Code::Unauthenticated, INVALID_TOKEN.as_str(), "invalid bearer token"),
        AuthnError::IdentityNotProvisioned => (Code::PermissionDenied, IDENTITY_NOT_PROVISIONED.as_str(), "identity not provisioned"),
        AuthnError::ProvisioningFailed(_) => (Code::PermissionDenied, PROVISIONING_FAILED.as_str(), "provisioning failed"),
        AuthnError::PrincipalInactive => (Code::PermissionDenied, PRINCIPAL_INACTIVE.as_str(), "principal inactive"),
        AuthnError::Unavailable => (Code::Unavailable, AUTHN_UNAVAILABLE.as_str(), "authentication backend unavailable"),
        AuthnError::Backend(_) => {
            // `Debug` carries the boxed repository/infra source (never token or claim
            // material, by `AuthnError`'s own contract) — logged here, never surfaced.
            tracing::error!(error = ?err, "internal error handling a gRPC authn request");
            (Code::Internal, AUTHN_INTERNAL.as_str(), "internal error")
        }
    };
    iam_status(code, reason, message, authn_retryable(err), &[])
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
    fn forbidden_maps_to_permission_denied_with_structured_detail() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::Forbidden);
        assert_eq!(status.code(), Code::PermissionDenied);
        // The wire change itself: the message is PURELY human-readable now. `Forbidden`'s Display
        // is static (SMA-444 task-16 brief), so no denying-policy detail reaches the wire either.
        assert_eq!(status.message(), "access denied");
        assert!(!status.message().starts_with("forbidden:"), "the in-band code prefix is gone (ADR-0019 decision 4)");

        let details = status.get_error_details();
        let info = details.error_info().expect("every IAM status carries ErrorInfo");
        assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);
        assert_eq!(info.reason, "forbidden");
        assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("false"));
    }

    #[test]
    fn not_found_maps_to_grpc_not_found() {
        let status = status_to_grpc(TenancyError::NotFound);
        assert_eq!(status.code(), Code::NotFound);
    }

    /// §4.3: outside a request scope the id keys are OMITTED, never filled with a nil UUID that
    /// would read as a real id in a support ticket.
    #[test]
    fn the_id_metadata_keys_are_absent_outside_a_request_scope() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::NotFound);
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert!(!info.metadata.contains_key("correlation_id"));
        assert!(!info.metadata.contains_key("request_id"));
    }

    /// The positive half of the contract the test above only pins the negative of (review
    /// finding #1): INSIDE a request scope, both id keys are present and equal exactly the
    /// scope's own ids — not just "present", which `contains_key` alone couldn't distinguish
    /// from a scope leaking someone else's ids. `scope_for_test` (Task 2/3) enters the same
    /// task-local `error_metadata`'s `current_ids()` reads.
    #[tokio::test]
    async fn the_id_metadata_keys_match_the_request_scope_when_present() {
        use paigasus_observability::correlation::{RequestIds, scope_for_test};
        use tonic_types::StatusExt;

        let ids = RequestIds {
            request_id: Uuid::parse_str("0198f2c1-7777-7000-8000-000000000042").unwrap(),
            correlation_id: Uuid::parse_str("0198f2c1-8888-7000-8000-000000000042").unwrap(),
        };
        let status = scope_for_test(ids, async { status_to_grpc(TenancyError::NotFound) }).await;
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(info.metadata.get("correlation_id"), Some(&ids.correlation_id.to_string()));
        assert_eq!(info.metadata.get("request_id"), Some(&ids.request_id.to_string()));
    }

    /// AC 4: an internal error's gRPC message is the static generic one, and nothing in the
    /// metadata carries backend text.
    #[test]
    fn internal_carries_a_generic_message_and_an_unknown_retryable() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::Internal);
        assert_eq!(status.code(), Code::Internal);
        assert_eq!(status.message(), "internal server error");
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(info.reason, "internal");
        assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("unknown"));
    }

    /// AC 6 for the authn funnel: five codes, all registry-resolvable, messages unchanged.
    #[test]
    fn every_authn_status_carries_a_registered_reason_and_its_original_message() {
        use paigasus_iam_core::{ProvisioningDefect, TokenDefect};
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use tonic_types::StatusExt;

        let cases = [
            (AuthnError::InvalidToken(TokenDefect::Malformed), Code::Unauthenticated, "invalid-token", "invalid bearer token"),
            (AuthnError::IdentityNotProvisioned, Code::PermissionDenied, "identity-not-provisioned", "identity not provisioned"),
            (
                AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail),
                Code::PermissionDenied,
                "provisioning-failed",
                "provisioning failed",
            ),
            (AuthnError::PrincipalInactive, Code::PermissionDenied, "principal-inactive", "principal inactive"),
            (AuthnError::Unavailable, Code::Unavailable, "authn-unavailable", "authentication backend unavailable"),
            (AuthnError::Backend("secret db detail".into()), Code::Internal, "internal", "internal error"),
        ];
        for (err, code, reason, message) in cases {
            let status = authn_status(&err);
            assert_eq!(status.code(), code, "{reason}");
            assert_eq!(status.message(), message, "authn messages are static and unchanged (D12)");
            assert!(ErrorReason::from_wire_reason(reason).is_some(), "{reason} must be in the registry");
            let details = status.get_error_details();
            let info = details.error_info().expect("ErrorInfo");
            assert_eq!(info.reason, reason);
            assert!(!format!("{:?}", info.metadata).contains("secret db detail"), "metadata must never carry backend text");
        }
    }

    /// AC 6 for the six sites that build a bare `Status` — the gap SMA-498's HTTP-only sweep
    /// missed. Both capability gates are here because they are exactly what an SDK branches on.
    #[test]
    fn the_bare_status_sites_carry_registered_reasons() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use tonic_types::StatusExt;

        let missing = missing_auth_context();
        assert_eq!(missing.code(), Code::Unauthenticated);
        let details = missing.get_error_details();
        let missing_info = details.error_info().expect("ErrorInfo");
        assert_eq!(missing_info.reason, "missing-auth-context");
        // Review finding #4: `No`, not `Unknown` — the cause is known (the layer never attached
        // a context) and retrying the identical request cannot resolve it.
        assert_eq!(missing_info.metadata.get("retryable").map(String::as_str), Some("false"));
        assert!(ErrorReason::from_wire_reason("missing-auth-context").is_some());

        let disabled = capability_disabled("iam.apikeys");
        assert_eq!(disabled.code(), Code::Unimplemented);
        let details = disabled.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(info.reason, "capability-disabled");
        assert_eq!(info.metadata.get("capability").map(String::as_str), Some("iam.apikeys"));
        assert!(ErrorReason::from_wire_reason("capability-disabled").is_some());
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
    /// Coverage is checked by enumerating every `TenancyError` variant via `strum::EnumIter`
    /// (`#[cfg_attr(test, derive(strum::EnumIter))]` on the enum) rather than a hand-maintained
    /// list: `TenancyError::iter()` yields one instance per variant straight from the type
    /// itself, so a new variant is included automatically — there is no second list that can be
    /// left un-extended. Assertions run through `code()` rather than string literals, so an
    /// unregistered rename fails too.
    #[test]
    fn every_tenancy_code_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        for err in TenancyError::iter() {
            let code = err.code();
            assert!(
                ErrorReason::from_wire_reason(code).is_some(),
                "TenancyError::{err:?} emits {code:?}, which is not declared in common/v1/error.proto"
            );
        }
    }
}
