// SPDX-License-Identifier: Apache-2.0

//! Domain <-> proto conversions and the shared gRPC helpers (`status_to_grpc`, `node_uuid`,
//! `to_page`) every `TenancyGrpc` method uses: parse -> service call -> convert, no business
//! logic in this layer (task-16 brief).

use chrono::{DateTime, Utc};
use paigasus_iam_core::{AuthnError, MembershipRecord, NodeStatus, NodeView, Organization, OrganizationId, PrincipalContext, Project, RoleGrantRef, Team};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::common::v1::AuditMetadata;
use paigasus_proto::paigasus::iam::v1::{
    IntrospectResponse, Membership, NodeStatus as ProtoNodeStatus, Organization as ProtoOrganization, Project as ProtoProject, RoleGrantRef as ProtoRoleGrantRef, Team as ProtoTeam,
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

/// Projects a `PrincipalContext` into the wire `IntrospectResponse` (spec §7.2/§7.3): PRN
/// strings, principal status as its stable `as_str`, `expires_at` as a prost `Timestamp`,
/// memberships via the shared tenancy `Membership` mapping, and `role_grants` from the
/// core's structured role-grant refs — always empty until a later M3 task populates it (D4).
pub fn to_introspect_response(ctx: &PrincipalContext) -> IntrospectResponse {
    IntrospectResponse {
        principal_prn: ctx.principal.principal_id.canonical(),
        status: ctx.principal.status.as_str().to_string(),
        issuer: ctx.principal.issuer.as_str().to_string(),
        subject: ctx.principal.subject.clone(),
        expires_at: Some(ts(ctx.principal.expires_at)),
        memberships: ctx.memberships.iter().map(to_proto_membership).collect(),
        role_grants: ctx.role_grants.iter().map(to_proto_role_grant_ref).collect(),
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
}
