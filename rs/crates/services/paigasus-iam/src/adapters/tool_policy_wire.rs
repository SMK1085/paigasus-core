// SPDX-License-Identifier: Apache-2.0

//! Conversions that the gRPC and the HTTP tool policy adapters both use (SMA-774): the stored
//! version as the wire message, and the wire answer of a read with no live set.

use paigasus_iam_core::tool_policy::{HeldReport, HeldResult, HeldStatus, HeldVersion, NameDiff, ToolPolicySetKey, ToolPolicyVersion, VersionOrigin, VersionStatus};
use paigasus_proto::paigasus::iam::v1 as pb;
use prost::Message;

use crate::adapters::grpc::convert;
use crate::application::error::TenancyError;
use crate::application::tool_policies::{RecompileOutcome, RecompileRun, SetRecompile};

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
            VersionOrigin::Recompile => pb::ToolPolicyVersionOrigin::Recompile,
            VersionOrigin::Accept => pb::ToolPolicyVersionOrigin::Accept,
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

fn name_diff_to_pb(d: &NameDiff) -> pb::NameDiff {
    pb::NameDiff {
        added: d.added.clone(),
        removed: d.removed.clone(),
        changed: d.changed.clone(),
    }
}

fn report_to_pb(r: &HeldReport) -> pb::HeldToolPolicyReport {
    pb::HeldToolPolicyReport {
        result: match r.result {
            HeldResult::Different => pb::HeldToolPolicyResult::Different,
            HeldResult::Failed => pb::HeldToolPolicyResult::Failed,
        } as i32,
        policies: Some(name_diff_to_pb(&r.policies)),
        conditions: Some(name_diff_to_pb(&r.conditions)),
        schema: Some(name_diff_to_pb(&r.schema)),
        rejections: Some(pb::ToolPolicyRejections {
            rejections: r
                .rejections
                .iter()
                .map(|x| pb::ToolPolicyRejection {
                    policy_id: x.policy_id.clone().unwrap_or_default(),
                    line: x.line.unwrap_or(0),
                    column: x.column.unwrap_or(0),
                    reason: x.code.clone(),
                    message: x.message.clone(),
                })
                .collect(),
        }),
    }
}

/// The held row as the wire message. `compiled` is decoded from the stored bytes, as
/// `version_to_pb` does, and is absent for a failed compile. Bytes that do not decode are a
/// storage defect.
pub(crate) fn held_to_pb(h: &HeldVersion) -> Result<pb::HeldToolPolicyVersion, TenancyError> {
    let compiled = h.compiled.as_deref().map(pb::CompiledToolPolicySet::decode).transpose().map_err(|error| {
        tracing::error!(error = %error, version = h.version, "the stored held tool policy set does not decode");
        TenancyError::Internal
    })?;
    let (kind, org_id) = match h.key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    Ok(pb::HeldToolPolicyVersion {
        kind: kind as i32,
        org_id,
        version: h.version,
        base_version: h.base_version,
        source: h.source.clone(),
        status: match h.status {
            HeldStatus::Held => pb::ToolPolicyVersionStatus::Held,
            HeldStatus::Discarded => pb::ToolPolicyVersionStatus::Discarded,
        } as i32,
        compiler_generation: h.compiler_generation,
        schema_revision: h.schema_revision,
        catalog_revision: h.catalog_revision.unwrap_or(0),
        report: Some(report_to_pb(&h.report)),
        created_by: h.created_by.clone(),
        created_at: Some(convert::ts(h.created_at)),
        compiled,
    })
}

fn set_recompile_to_pb(r: &SetRecompile) -> pb::RecompileSetResult {
    let (kind, org_id) = match r.key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    let (outcome, version) = match r.outcome {
        RecompileOutcome::Identical { version } => (pb::RecompileOutcome::Identical, version),
        RecompileOutcome::HeldDifferent { version } => (pb::RecompileOutcome::HeldDifferent, version),
        RecompileOutcome::HeldFailed { version } => (pb::RecompileOutcome::HeldFailed, version),
        RecompileOutcome::AlreadyProposed => (pb::RecompileOutcome::AlreadyProposed, 0),
        RecompileOutcome::Conflict => (pb::RecompileOutcome::Conflict, 0),
        RecompileOutcome::Error => (pb::RecompileOutcome::Error, 0),
    };
    pb::RecompileSetResult {
        kind: kind as i32,
        org_id,
        base_version: r.base_version,
        outcome: outcome as i32,
        version,
    }
}

/// The run as the wire answer. `AlreadyRunning` and `OlderReplica` have no results.
pub(crate) fn run_to_pb(run: &RecompileRun) -> pb::RunToolPolicyRecompileResponse {
    match run {
        RecompileRun::Ran(results) => pb::RunToolPolicyRecompileResponse {
            status: pb::RecompileRunStatus::Ran as i32,
            results: results.iter().map(set_recompile_to_pb).collect(),
        },
        RecompileRun::AlreadyRunning => pb::RunToolPolicyRecompileResponse {
            status: pb::RecompileRunStatus::AlreadyRunning as i32,
            results: Vec::new(),
        },
        RecompileRun::OlderReplica { .. } => pb::RunToolPolicyRecompileResponse {
            status: pb::RecompileRunStatus::OlderReplica as i32,
            results: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::tool_policy::{ReportRejection, failed_report};

    /// The source location of a rejection in a held report reaches the wire message. A location
    /// that is not known is 0, as the proto comment says.
    #[test]
    fn a_held_report_rejection_keeps_its_line_and_column() {
        let rejection = |line, column| ReportRejection {
            policy_id: Some("p".into()),
            line,
            column,
            code: "c".into(),
            message: "m".into(),
        };
        let held = HeldVersion {
            key: ToolPolicySetKey::Floor,
            version: 2,
            base_version: 1,
            source: String::new(),
            compiled: None,
            compiler_generation: 1,
            schema_revision: 1,
            catalog_revision: None,
            status: HeldStatus::Held,
            report: failed_report(vec![rejection(Some(3), Some(9)), rejection(None, None)]),
            created_by: "job".into(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        };
        let wire = held_to_pb(&held).unwrap().report.unwrap().rejections.unwrap().rejections;
        assert_eq!(wire.iter().map(|r| (r.line, r.column)).collect::<Vec<_>>(), vec![(3, 9), (0, 0)]);
    }
}
