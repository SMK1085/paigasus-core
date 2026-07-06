// SPDX-License-Identifier: Apache-2.0

//! Domain <-> proto conversions and the shared gRPC helpers (`status_to_grpc`, `node_uuid`,
//! `to_page`) every `TenancyGrpc` method uses: parse -> service call -> convert, no business
//! logic in this layer (task-16 brief).

use chrono::{DateTime, Utc};
use paigasus_iam_core::{MembershipRecord, NodeStatus, NodeView, Organization, OrganizationId, Project, Team};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::common::v1::AuditMetadata;
use paigasus_proto::paigasus::iam::v1::{Membership, NodeStatus as ProtoNodeStatus, Organization as ProtoOrganization, Project as ProtoProject, Team as ProtoTeam};
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
        ErrorClass::Internal => Code::Internal,
    };
    Status::new(code, format!("{}: {}", e.code(), e))
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
