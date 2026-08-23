// SPDX-License-Identifier: Apache-2.0

//! Domain <-> proto conversions and the shared gRPC helpers (`status_to_grpc`, `node_uuid`,
//! `to_page`) every `TenancyGrpc` method uses: parse -> service call -> convert, no business
//! logic in this layer (task-16 brief).
//!
//! `iam_status` (SMA-504) is the single construction point every IAM gRPC error must go
//! through: it is the only place that builds `ErrorDetails::with_error_info`, so no call site
//! can forget the machine-readable `(domain, reason, metadata)` triple. `status_to_grpc` and
//! `authn_status` are themselves thin `iam_status` callers, not independent constructors.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::authz::reconcile::policy_kind_str;
use paigasus_iam_core::{
    ApiKey, ApiKeyStatus, AuthnError, Credential, MembershipRecord, NewApiKey, NodeStatus, NodeView, Organization, OrganizationId, PolicyDocument, PrincipalContext, Project, RetireOutcome, RoleGrant,
    RoleGrantRef, ServiceAccountRecord, Team,
};
use paigasus_kernel::Prn;
use paigasus_observability::{Retryable, current_ids};
use paigasus_proto::error::IAM_DOMAIN;
use paigasus_proto::paigasus::common::v1::{AuditMetadata, ErrorReason};
use paigasus_proto::paigasus::iam::v1::{
    ApiKey as ProtoApiKey, ApiKeyStatus as ProtoApiKeyStatus, DeadLetterEntry as ProtoDeadLetterEntry, IntrospectApiKeyResponse, IntrospectResponse, IssueApiKeyResponse, Membership,
    NodeStatus as ProtoNodeStatus, Organization as ProtoOrganization, Policy as ProtoPolicy, Project as ProtoProject, RetireSystemPolicyResponse, RetiredPolicy, RetirementBlocked,
    RetirementNeedsAcknowledgement, RoleGrant as ProtoRoleGrant, RoleGrantRef as ProtoRoleGrantRef, ServiceAccount as ProtoServiceAccount, SurvivingGrant, Team as ProtoTeam,
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
    // `extra` inserts FIRST: the canonical keys below (`retryable`/`correlation_id`/
    // `request_id`) are authoritative and must win a collision, never be silently overwritten by
    // a future caller that happens to pass one of those names in `extra` (review finding #9 —
    // only `("capability", ...)` is passed today, so this ordering is free to fix now).
    for (k, v) in extra {
        metadata.insert((*k).to_owned(), (*v).to_owned());
    }
    metadata.insert("retryable".to_owned(), retryable.as_wire().to_owned());
    if let Some(ids) = current_ids() {
        metadata.insert("correlation_id".to_owned(), ids.correlation_id.to_string());
        metadata.insert("request_id".to_owned(), ids.request_id.to_string());
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
    // The field name (SMA-586) rides in metadata alongside it, so a client can act on WHICH
    // field failed without parsing the message — which SMA-508 AC2 forbids.
    let field = e.field();
    let extra_owned: Vec<(&str, &str)> = field.map(|f| ("field", f)).into_iter().collect();
    iam_status(code, e.code(), e.to_string(), tenancy_retryable(e.class()), &extra_owned)
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
///
/// **Module-private on purpose (SMA-583).** On a filter field `None` means UNFILTERED, so an
/// `and_then(from_ts)` call site silently widens the query instead of rejecting a malformed
/// bound. Callers outside this module must use [`parse_opt_ts`], which keeps the three cases
/// distinct. Note this closes *that shape* only — `parse_opt_ts(..).ok().flatten()` would
/// reintroduce the same bug, and no grep gate would catch that either.
fn from_ts(t: prost_types::Timestamp) -> Option<DateTime<Utc>> {
    let nanos = u32::try_from(t.nanos).ok()?;
    DateTime::<Utc>::from_timestamp(t.seconds, nanos)
}

/// Parses an optional wire timestamp with the three cases kept DISTINCT: absent means
/// unfiltered, a valid value converts, and a **present but unrepresentable** value is a client
/// error.
///
/// That third case is why this exists. `from_ts` returns `None` for a negative `nanos` or an
/// out-of-`chrono`-range `seconds`, and on a filter field `None` means UNFILTERED — so a
/// `req.field.and_then(from_ts)` shape silently DROPS a malformed bound instead of rejecting
/// it. On `BulkReplayDeadLetters` that turned a narrowly-scoped replay into "replay everything
/// up to `max_rows`"; on `ListAuditEntries` it widened the result set (SMA-583). Both surfaces
/// now use this helper, and `from_ts` is module-private so the shape cannot recur outside this
/// file. The HTTP twin rejects the equivalent with a 400 (`http::dead_letters::parse_ts`), so
/// this also restores parity.
///
/// `InvalidTimestamp` carries the field name, which reaches both the message and
/// `ErrorInfo.metadata["field"]` (SMA-586 — this used to be `InvalidPrn`-as-sentinel, whose
/// static Display threw the field name away).
pub(crate) fn parse_opt_ts(t: Option<prost_types::Timestamp>, field: &'static str) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match t {
        None => Ok(None),
        Some(raw) => from_ts(raw).map(Some).ok_or(TenancyError::InvalidTimestamp(field)),
    }
}

/// Rejects an absent required wire field before it reaches a parser that would mis-describe
/// the failure (SMA-586 D5.2).
///
/// proto3 has no absence for a plain `string`, so an unset field arrives as `""` — which
/// `Prn::parse` would report as a malformed PRN rather than as a missing one, diverging from
/// the HTTP twin where the param is genuinely `Option`. Whitespace counts as empty: a
/// `?owner_prn=%20` is not a PRN anyone meant to send.
pub(crate) fn require_present<'a>(raw: &'a str, field: &'static str) -> Result<&'a str, TenancyError> {
    if raw.trim().is_empty() {
        return Err(TenancyError::MissingRequiredField(field));
    }
    Ok(raw)
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

/// Projects a domain [`DeadLetterEntry`] into its wire message. `id`/`correlation_id` become
/// canonical uuid strings, `actor_prn`/`correlation_id`/`last_error` collapse `None` to the
/// empty-string sentinel the proto documents, and an unparked row carries an ABSENT
/// `parked_at` rather than an epoch timestamp. Mirrors `http::dto::DeadLetterEntryDto`'s
/// `From` impl field-for-field — the two are pinned together by
/// `dead_letter_entry_projects_identically_for_http_and_grpc`.
pub fn to_proto_dead_letter_entry(e: &paigasus_iam_core::DeadLetterEntry) -> ProtoDeadLetterEntry {
    ProtoDeadLetterEntry {
        id: e.id.to_string(),
        occurred_at: Some(ts(e.occurred_at)),
        event_type: e.event_type.clone(),
        schema_version: e.schema_version,
        aggregate_prn: e.aggregate_prn.clone(),
        actor_prn: e.actor_prn.clone().unwrap_or_default(),
        payload: e.payload.clone(),
        correlation_id: e.correlation_id.map(|id| id.to_string()).unwrap_or_default(),
        attempts: e.attempts,
        parked_at: e.parked_at.map(ts),
        last_error: e.last_error.clone().unwrap_or_default(),
    }
}

/// Maps a [`RetireOutcome`] onto its wire response. All three variants are gRPC `OK`: the two
/// refusals are outcomes that are not `Retired`, never server errors (design D3, and the same
/// argument `http::system_retirement`'s module doc makes for routing them around `ApiError`).
///
/// A free function over an OWNED outcome, deliberately — that is what lets every variant be
/// constructed in a test with no `AppState`, database, or request, which is exactly the gap
/// that let an earlier `200`->`204` regression in the HTTP twin pass a green suite.
pub fn to_proto_retire_response(outcome: RetireOutcome) -> RetireSystemPolicyResponse {
    use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

    let variant = match outcome {
        RetireOutcome::Retired { policy_id, kind, role_deleted } => Outcome::Retired(RetiredPolicy {
            policy_id,
            kind: policy_kind_str(kind).to_string(),
            role_deleted,
        }),
        RetireOutcome::Blocked { role_key, grants, total, truncated } => Outcome::Blocked(RetirementBlocked {
            role_key,
            grants: grants
                .iter()
                .map(|g| SurvivingGrant {
                    id: g.id.clone(),
                    principal_prn: g.principal_prn.clone(),
                    scope_prn: g.scope_prn.clone(),
                })
                .collect(),
            total_surviving: total,
            truncated,
        }),
        RetireOutcome::NeedsAcknowledgement { policy_id, kind, source, description } => Outcome::NeedsAcknowledgement(RetirementNeedsAcknowledgement {
            policy_id,
            kind: policy_kind_str(kind).to_string(),
            source,
            description,
        }),
    };
    RetireSystemPolicyResponse { outcome: Some(variant) }
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

    /// SMA-586: the field name is also machine-readable on gRPC. `Display` alone is not enough —
    /// SMA-508 AC2 forbids branching on message text, so a field reachable only through the
    /// message is reachable only by humans. Uses the same open metadata map as `capability`.
    #[test]
    fn status_to_grpc_puts_the_field_name_in_error_info_metadata() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::InvalidTimestamp("parked_to"));
        let details = status.get_error_details();
        let info = details.error_info().expect("every IAM status carries ErrorInfo");
        assert_eq!(info.metadata.get("field").map(String::as_str), Some("parked_to"));
        assert_eq!(info.reason, "invalid-timestamp");
        // The canonical keys are untouched by the new one.
        assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("false"));
    }

    /// A variant with no field name adds no key at all — an absent key, never an empty string,
    /// so a consumer can distinguish "no field" from "a field named nothing".
    #[test]
    fn status_to_grpc_omits_the_field_key_when_there_is_no_field() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::NotFound);
        let details = status.get_error_details();
        let info = details.error_info().expect("every IAM status carries ErrorInfo");
        assert!(!info.metadata.contains_key("field"), "metadata: {:?}", info.metadata);
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

    /// Review finding #9: `extra` is inserted BEFORE the canonical `retryable`/`correlation_id`/
    /// `request_id` keys, so a caller that (accidentally or otherwise) passes one of those names
    /// in `extra` can never silently overwrite the authoritative value.
    #[test]
    fn extra_metadata_can_never_override_the_canonical_retryable_key() {
        use tonic_types::StatusExt;

        let status = iam_status(Code::Internal, "internal", "internal error", Retryable::Unknown, &[("retryable", "true")]);
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(
            info.metadata.get("retryable").map(String::as_str),
            Some("unknown"),
            "the canonical retryable value must win over a same-named extra entry"
        );
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

    #[test]
    fn parse_opt_ts_treats_an_absent_timestamp_as_unfiltered() {
        assert_eq!(parse_opt_ts(None, "parked_from").unwrap(), None);
    }

    #[test]
    fn parse_opt_ts_returns_the_exact_instant_for_a_valid_timestamp() {
        let expected = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let got = parse_opt_ts(
            Some(prost_types::Timestamp {
                seconds: expected.timestamp(),
                nanos: 0,
            }),
            "parked_from",
        )
        .unwrap();
        assert_eq!(got, Some(expected));
    }

    /// The whole reason this helper exists. `from_ts` alone returns `None` here, and `None` means
    /// UNFILTERED — so an `and_then(from_ts)` call site would silently drop the caller's time
    /// bound and widen a bulk replay to every parked row. A present field must never be
    /// reinterpreted as an absent one.
    #[test]
    fn parse_opt_ts_rejects_a_present_but_unrepresentable_timestamp_instead_of_unfiltering() {
        for (label, t) in [
            ("negative nanos", prost_types::Timestamp { seconds: 0, nanos: -1 }),
            ("out-of-range seconds", prost_types::Timestamp { seconds: i64::MAX, nanos: 0 }),
        ] {
            let err = parse_opt_ts(Some(t), "parked_from").expect_err(label);
            assert!(matches!(err, TenancyError::InvalidTimestamp(_)), "{label} must be a client error, not None");
        }
        // Sanity: the underlying primitive really does collapse these to None, which is what makes
        // this helper load-bearing rather than decorative.
        assert_eq!(from_ts(prost_types::Timestamp { seconds: 0, nanos: -1 }), None);
    }

    /// The inverse of the pre-SMA-586 behaviour this replaces. `parse_opt_ts` took a field name
    /// and then threw it away, because `InvalidPrn`'s Display is static — pinned by the test that
    /// used to live here. `InvalidTimestamp` interpolates it, so the caller learns WHICH bound
    /// failed, and `status_to_grpc` also puts it in `ErrorInfo.metadata["field"]`.
    #[test]
    fn parse_opt_ts_surfaces_the_field_name_in_its_display() {
        let err = parse_opt_ts(Some(prost_types::Timestamp { seconds: 0, nanos: -1 }), "parked_to").unwrap_err();
        assert_eq!(err, TenancyError::InvalidTimestamp("parked_to"));
        assert!(err.to_string().contains("parked_to"), "got {err}");
        assert_eq!(err.code(), "invalid-timestamp");
    }

    /// The HTTP/gRPC drift guard for the dead-letter surface (design D9.1), paired with
    /// `http::dto`'s own projection. Both transports project the SAME domain value, so feeding one
    /// `DeadLetterEntry` through both and comparing field-for-field is the only cheap, deterministic
    /// way to catch one of them drifting. Deliberately exercises the `None` half of every optional
    /// field, since the empty-string / absent-timestamp sentinel mapping is where the two shapes
    /// differ in TYPE and so is where they are most likely to diverge in MEANING.
    #[test]
    fn dead_letter_entry_projects_identically_for_http_and_grpc() {
        use crate::adapters::http::dto::DeadLetterEntryDto;

        let occurred = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z").unwrap().with_timezone(&Utc);
        let parked = DateTime::parse_from_rfc3339("2026-08-01T11:00:00Z").unwrap().with_timezone(&Utc);
        let domain = paigasus_iam_core::DeadLetterEntry {
            id: Uuid::from_u128(7),
            occurred_at: occurred,
            event_type: "iam.principal.created".to_string(),
            schema_version: 3,
            aggregate_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000042".to_string(),
            actor_prn: None,
            payload: r#"{"principal_id":"x"}"#.to_string(),
            correlation_id: None,
            attempts: 5,
            parked_at: Some(parked),
            last_error: None,
        };

        let http = DeadLetterEntryDto::from(domain.clone());
        let grpc = to_proto_dead_letter_entry(&domain);

        assert_eq!(grpc.id, http.id.to_string());
        assert_eq!(grpc.occurred_at, Some(ts(http.occurred_at)));
        assert_eq!(grpc.event_type, http.event_type);
        assert_eq!(grpc.schema_version, http.schema_version);
        assert_eq!(grpc.aggregate_prn, http.aggregate_prn);
        assert_eq!(grpc.attempts, http.attempts);
        assert_eq!(grpc.parked_at, http.parked_at.map(ts));
        // The sentinel half: HTTP keeps `None`, the wire uses "".
        assert_eq!(http.actor_prn, None);
        assert_eq!(grpc.actor_prn, "");
        assert_eq!(http.correlation_id, None);
        assert_eq!(grpc.correlation_id, "");
        assert_eq!(http.last_error, None);
        assert_eq!(grpc.last_error, "");
        assert_eq!(grpc.payload, http.payload);
    }

    /// The `Some` half, asserted separately so a projection that hardcoded the empty-string
    /// sentinel — passing the test above — still fails here.
    #[test]
    fn dead_letter_entry_forwards_present_optional_fields_verbatim() {
        let occurred = DateTime::parse_from_rfc3339("2026-08-01T10:00:00Z").unwrap().with_timezone(&Utc);
        let correlation = Uuid::from_u128(99);
        let domain = paigasus_iam_core::DeadLetterEntry {
            id: Uuid::from_u128(7),
            occurred_at: occurred,
            event_type: "iam.principal.created".to_string(),
            schema_version: 3,
            aggregate_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000042".to_string(),
            actor_prn: Some("prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001".to_string()),
            payload: "{}".to_string(),
            correlation_id: Some(correlation),
            attempts: 1,
            parked_at: None,
            last_error: Some("connection refused".to_string()),
        };

        let grpc = to_proto_dead_letter_entry(&domain);
        assert_eq!(grpc.actor_prn, domain.actor_prn.clone().unwrap());
        assert_eq!(grpc.correlation_id, correlation.to_string());
        assert_eq!(grpc.last_error, "connection refused");
        assert_eq!(grpc.parked_at, None, "an unparked row must carry an ABSENT timestamp, not epoch");
    }

    /// Every variant constructed directly — no `AppState`, database, or request needed. That is the
    /// whole point of this being a free function over an owned `RetireOutcome` (design D8): an
    /// earlier revision of the HTTP twin changed `Retired`'s status code and the entire crate's
    /// suite stayed green, because nothing exercised the mapping against a real outcome value.
    #[test]
    fn retire_response_maps_each_outcome_to_its_own_variant() {
        use paigasus_iam_core::GrantRef;
        use paigasus_iam_core::authz::model::PolicyKind;
        use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;

        let retired = to_proto_retire_response(RetireOutcome::Retired {
            policy_id: "legacy_auditor".to_string(),
            kind: PolicyKind::Template,
            role_deleted: true,
        });
        match retired.outcome.expect("outcome must be set") {
            Outcome::Retired(r) => {
                assert_eq!(r.policy_id, "legacy_auditor");
                assert_eq!(r.kind, "template");
                assert!(r.role_deleted);
            }
            other => panic!("expected Retired, got {other:?}"),
        }

        let blocked = to_proto_retire_response(RetireOutcome::Blocked {
            role_key: "legacy_auditor".to_string(),
            grants: vec![GrantRef {
                id: "0192f1c0-0000-7000-8000-000000000001".to_string(),
                principal_prn: "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000002".to_string(),
                scope_prn: "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000003".to_string(),
            }],
            total: 42,
            truncated: true,
        });
        match blocked.outcome.expect("outcome must be set") {
            Outcome::Blocked(b) => {
                assert_eq!(b.role_key, "legacy_auditor");
                assert_eq!(b.total_surviving, 42, "the TRUE total, not the truncated page length");
                assert!(b.truncated);
                assert_eq!(b.grants.len(), 1);
                assert_eq!(b.grants[0].id, "0192f1c0-0000-7000-8000-000000000001");
                assert_eq!(b.grants[0].principal_prn, "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000002");
                assert_eq!(b.grants[0].scope_prn, "prn:pgs:iam:::organization/0192f1c0-0000-7000-8000-000000000003");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }

        let needs = to_proto_retire_response(RetireOutcome::NeedsAcknowledgement {
            policy_id: "legacy_forbid".to_string(),
            kind: PolicyKind::Static,
            source: "permit(principal, action, resource);".to_string(),
            description: "an orphaned starter policy".to_string(),
        });
        match needs.outcome.expect("outcome must be set") {
            Outcome::NeedsAcknowledgement(n) => {
                assert_eq!(n.policy_id, "legacy_forbid");
                assert_eq!(n.kind, "static");
                assert_eq!(n.source, "permit(principal, action, resource);");
                assert_eq!(n.description, "an orphaned starter policy");
            }
            other => panic!("expected NeedsAcknowledgement, got {other:?}"),
        }
    }

    /// AC-3: HTTP and gRPC agree on the reason for the same logical failure.
    ///
    /// Both transports derive `reason` from the SAME function (`TenancyError::code()`), so for a
    /// GIVEN variant they cannot disagree. Parity can only break where the two sides CONSTRUCT
    /// DIFFERENT VARIANTS — so this drives each transport's request-conversion entry point
    /// (`to_filter`) rather than comparing two calls to `code()`, which would be tautological.
    /// Driving `to_filter` also proves each helper is still WIRED IN, which is the failure SMA-583
    /// actually hit: a helper that exists and is correct but no longer reached.
    ///
    /// One row calls a helper directly, because that surface has no filter-shaped entry point:
    /// `membership_filter` on both transports. The dead-letter id row used to live here too, as
    /// a hardcoded `TenancyError::InvalidUuid("dead_letter_id")` on the HTTP side — but that
    /// construction never touches HTTP code at all, so it was tautologically equal to the
    /// expected reason (`InvalidUuid(_)::code()` is unconditionally `"invalid-uuid"`) and proved
    /// nothing about HTTP (SMA-586 fix round 1, Finding 1). It now lives in
    /// [`http_dead_letter_id_agrees_with_grpc_on_invalid_uuid`] below, which drives the real
    /// `UuidPath<DeadLetterId>` extractor through an actual request/response round trip instead.
    #[test]
    fn http_and_grpc_agree_on_the_reason_for_the_same_failure() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        use crate::adapters::grpc::{audit as gaudit, dead_letters as gdl, tenancy as gtenancy};
        use crate::adapters::http::dto::{AuditQuery, DeadLetterQuery};
        use crate::adapters::http::{audit as haudit, dead_letters as hdl, memberships as hmem};
        use paigasus_proto::paigasus::iam::v1::{ListAuditEntriesRequest, ListDeadLettersRequest};

        // A `nanos` of -1 is unrepresentable in chrono, which is how a gRPC timestamp fails — it
        // cannot fail to PARSE (it is already a struct), only to CONVERT.
        let bad_ts = prost_types::Timestamp { seconds: 0, nanos: -1 };

        fn audit_req() -> ListAuditEntriesRequest {
            ListAuditEntriesRequest {
                actor_prn: String::new(),
                resource_prn: String::new(),
                action: String::new(),
                outcome: String::new(),
                from: None,
                to: None,
                cursor: String::new(),
                limit: 0,
            }
        }
        fn audit_query() -> AuditQuery {
            AuditQuery {
                actor: None,
                resource: None,
                action: None,
                outcome: None,
                from: None,
                to: None,
                cursor: None,
                limit: None,
            }
        }
        fn dl_req() -> ListDeadLettersRequest {
            ListDeadLettersRequest {
                event_type: String::new(),
                parked_from: None,
                parked_to: None,
                cursor: String::new(),
                limit: 0,
            }
        }
        fn dl_query() -> DeadLetterQuery {
            DeadLetterQuery {
                event_type: None,
                parked_from: None,
                parked_to: None,
                cursor: None,
                limit: None,
            }
        }

        let cases: Vec<(&str, TenancyError, TenancyError, ErrorReason)> = vec![
            (
                "audit from-bound",
                haudit::to_filter(AuditQuery {
                    from: Some("not-a-timestamp".into()),
                    ..audit_query()
                })
                .unwrap_err(),
                gaudit::to_filter(ListAuditEntriesRequest { from: Some(bad_ts), ..audit_req() }).unwrap_err(),
                ErrorReason::InvalidTimestamp,
            ),
            (
                "audit cursor",
                haudit::to_filter(AuditQuery {
                    cursor: Some("not-a-uuid".into()),
                    ..audit_query()
                })
                .unwrap_err(),
                gaudit::to_filter(ListAuditEntriesRequest {
                    cursor: "not-a-uuid".into(),
                    ..audit_req()
                })
                .unwrap_err(),
                ErrorReason::InvalidCursor,
            ),
            (
                "audit outcome",
                haudit::to_filter(AuditQuery {
                    outcome: Some("not-a-real-outcome".into()),
                    ..audit_query()
                })
                .unwrap_err(),
                gaudit::to_filter(ListAuditEntriesRequest {
                    outcome: "not-a-real-outcome".into(),
                    ..audit_req()
                })
                .unwrap_err(),
                ErrorReason::InvalidAuditOutcome,
            ),
            (
                "dead-letter parked_from bound",
                hdl::to_filter(DeadLetterQuery {
                    parked_from: Some("not-a-timestamp".into()),
                    ..dl_query()
                })
                .unwrap_err(),
                gdl::to_filter(ListDeadLettersRequest {
                    parked_from: Some(bad_ts),
                    ..dl_req()
                })
                .unwrap_err(),
                ErrorReason::InvalidTimestamp,
            ),
            (
                "dead-letter cursor",
                hdl::to_filter(DeadLetterQuery {
                    cursor: Some("not-a-uuid".into()),
                    ..dl_query()
                })
                .unwrap_err(),
                gdl::to_filter(ListDeadLettersRequest {
                    cursor: "not-a-uuid".into(),
                    ..dl_req()
                })
                .unwrap_err(),
                ErrorReason::InvalidCursor,
            ),
            (
                "membership filter, neither set",
                hmem::membership_filter(None, None).unwrap_err(),
                gtenancy::membership_filter(None).unwrap_err(),
                ErrorReason::MissingRequiredField,
            ),
        ];

        for (label, http_err, grpc_err, expected) in cases {
            let wire = expected.as_wire_reason().expect("not the Unspecified sentinel");
            assert_eq!(http_err.code(), wire, "{label}: HTTP reason");
            assert_eq!(grpc_err.code(), wire, "{label}: gRPC reason");
        }
    }

    /// The dead-letter-id row of the agreement table above, lifted out into its own test because
    /// it needs an async request/response round trip rather than a plain function call (SMA-586
    /// fix round 1, Finding 1).
    ///
    /// The HTTP side used to be a hardcoded `TenancyError::InvalidUuid("dead_letter_id")`
    /// constructed inline in the test — never touching HTTP code, so it was tautologically equal
    /// to the expected reason (`InvalidUuid(_)::code()` is unconditionally `"invalid-uuid"`).
    /// This drives the REAL `UuidPath<DeadLetterId>` extractor `http::dead_letters`'s own
    /// `replay_one`/`discard_one` routes use, through an actual `Router`/`oneshot` round trip —
    /// the identical pattern `http::path`'s own tests use to prove the same extractor for
    /// `UuidPath<MembershipId>`. No `AppState` is needed: extractor rejection happens before the
    /// handler (and therefore before any state access), so a stateless one-route router reaches
    /// the same rejection a real state-carrying route would produce.
    #[tokio::test]
    async fn http_dead_letter_id_agrees_with_grpc_on_invalid_uuid() {
        use axum::Router;
        use axum::body::to_bytes;
        use axum::http::StatusCode;
        use axum::routing::get;
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use tower::ServiceExt;

        use crate::adapters::grpc::dead_letters as gdl;
        use crate::adapters::http::path::{DeadLetterId, UuidPath};

        async fn ok(_path: UuidPath<DeadLetterId>) -> StatusCode {
            StatusCode::OK
        }

        let wire = ErrorReason::InvalidUuid.as_wire_reason().expect("not the Unspecified sentinel");

        let app = Router::new().route("/x/{id}", get(ok));
        let resp = app.oneshot(axum::http::Request::builder().uri("/x/not-a-uuid").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], wire, "HTTP reason");

        assert_eq!(gdl::parse_id("not-a-uuid").unwrap_err().code(), wire, "gRPC reason");
    }

    /// The counterpart to the agreement table: the two places the transports DELIBERATELY differ.
    /// Recorded as assertions so a change breaks this test rather than slipping through as an
    /// omission — the failure mode the SMA-586 spec review caught in its own first draft.
    ///
    /// Divergence 1 asserts only its gRPC half — see the comment on that assertion for why
    /// (SMA-586 fix round 1, Finding 2: the HTTP half of the original claim turned out false).
    #[test]
    fn the_accepted_transport_divergences_are_exactly_these_two() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        // 1. `IssueApiKey.expires_at`, gRPC half: `prost_types::Timestamp` fails to CONVERT (it
        //    cannot fail to parse — it's already a struct), and gRPC classifies that itself via
        //    `parse_opt_ts`, asserted below.
        //
        //    The HTTP half is NOT asserted here. The original claim was that HTTP's typed
        //    `DateTime<Utc>` field fails inside serde and yields `invalid-request-body` — verified
        //    FALSE against the real route: `issue`'s `Json<IssueApiKeyBody>` parameter
        //    (`http/api_keys.rs`) is axum's plain `Json`, not `http::authn::EnvelopeJson`, so a
        //    malformed body never reaches the IAM error envelope or the registry at all. Driving
        //    the real extractor with `{"expires_at":"not-a-timestamp",...}` produces axum's own
        //    non-JSON 422 rejection text, with no `error.code` field to compare against
        //    `ErrorReason::InvalidRequestBody` in the first place — there is nothing for this test
        //    to assert without either changing production code (out of scope for SMA-586 Task 8,
        //    which makes test/visibility changes only) or hardcoding today's accidental
        //    non-contract response shape as though it were the intended one, which would be worse
        //    than asserting nothing. Flagged for a follow-up ticket rather than fixed here.
        let wire = |r: ErrorReason| r.as_wire_reason().expect("not the Unspecified sentinel");
        assert_eq!(
            parse_opt_ts(Some(prost_types::Timestamp { seconds: 0, nanos: -1 }), "expires_at").unwrap_err().code(),
            wire(ErrorReason::InvalidTimestamp),
        );

        // 2. `mutually-exclusive-fields` is HTTP-only and STRUCTURALLY so: the gRPC surface models
        //    the same choice as a proto3 `oneof`, which cannot carry two values. Its only failure
        //    is "neither set", asserted as `missing-required-field` in the table above.
        use crate::adapters::http::memberships::membership_filter;
        assert_eq!(membership_filter(Some("a".into()), Some("b".into())).unwrap_err().code(), wire(ErrorReason::MutuallyExclusiveFields));
        // There is no gRPC expression of "both set" to compare against — that is the point.
    }
}
