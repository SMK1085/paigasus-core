// SPDX-License-Identifier: Apache-2.0

//! Conversions that the gRPC and the HTTP tool policy adapters both use (SMA-774): the stored
//! version as the wire message, and the wire answer of a read with no live set.

use paigasus_iam_core::tool_policy::{ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus};
use paigasus_proto::paigasus::iam::v1 as pb;
use prost::Message;

use crate::adapters::grpc::convert;
use crate::application::error::TenancyError;

/// The stored version as the wire message. Bytes that do not decode are a storage defect.
pub(crate) fn version_to_pb(v: &ToolPolicyVersion) -> Result<pb::ToolPolicySetVersion, TenancyError> {
    let compiled = pb::CompiledToolPolicySet::decode(v.compiled.as_slice()).map_err(|error| {
        tracing::error!(error = %error, version = v.version, "the stored compiled tool policy set does not decode");
        TenancyError::Internal
    })?;
    let (kind, org_id) = match v.key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    Ok(pb::ToolPolicySetVersion {
        kind: kind as i32,
        org_id,
        version: v.version,
        source: v.source.clone(),
        status: match v.status {
            VersionStatus::Live => pb::ToolPolicyVersionStatus::Live,
            VersionStatus::Superseded => pb::ToolPolicyVersionStatus::Superseded,
        } as i32,
        origin: match v.origin {
            VersionOrigin::Save => pb::ToolPolicyVersionOrigin::Save,
            VersionOrigin::Starter => pb::ToolPolicyVersionOrigin::Starter,
            VersionOrigin::CatalogChange => pb::ToolPolicyVersionOrigin::CatalogChange,
        } as i32,
        compiler_generation: v.compiler_generation,
        schema_revision: v.schema_revision,
        catalog_revision: v.catalog_revision.unwrap_or(0),
        created_by: v.created_by.clone(),
        created_at: Some(convert::ts(v.created_at)),
        compiled: Some(compiled),
    })
}

/// The wire answer of a read with no live set: version 0, empty source, no compiled message.
pub(crate) fn empty_set(key: ToolPolicySetKey) -> pb::ToolPolicySetVersion {
    let (kind, org_id) = match key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    pb::ToolPolicySetVersion {
        kind: kind as i32,
        org_id,
        ..Default::default()
    }
}

/// The compiled message of a stored version, or the wire answer of a read with no version: the
/// kind set, `version` 0, an empty `org_id` and nothing else.
pub(crate) fn compiled_or_empty(found: Option<&ToolPolicyVersion>, key: ToolPolicySetKey) -> Result<pb::CompiledToolPolicySet, TenancyError> {
    match found {
        Some(v) => version_to_pb(v)?.compiled.ok_or_else(|| {
            tracing::error!(key = ?v.key, version = v.version, "the stored tool policy version has no compiled message");
            TenancyError::Internal
        }),
        None => Ok(pb::CompiledToolPolicySet {
            kind: match key {
                ToolPolicySetKey::Tenant(_) => pb::ToolPolicySetKind::Tenant,
                ToolPolicySetKey::Floor => pb::ToolPolicySetKind::Floor,
            } as i32,
            ..Default::default()
        }),
    }
}
