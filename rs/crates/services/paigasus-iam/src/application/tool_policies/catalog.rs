// SPDX-License-Identifier: Apache-2.0

//! The pure part of a catalog write (spec section 4.2): what one write upserts and deletes, and
//! the catalog that results. No I/O. The service runs the coupling compile on the result.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use paigasus_iam_core::tool_policy::{CatalogProblem, ImportedTools, StoredTool, ToolEntry, name_key};

use super::rejection::{RejectionReason, ToolPolicyRejection};
use crate::application::error::TenancyError;

/// The changes of one catalog write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogPlan {
    pub upserts: Vec<ToolEntry>,
    /// The `name_key` of each tool to remove.
    pub deletes: Vec<String>,
    /// Import entries of another type: the index in the document and the `type` value.
    pub skipped: Vec<(usize, String)>,
}

/// Why a write does not go on: a rejection (a successful result) or an error.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlanStop {
    Rejected(Vec<ToolPolicyRejection>),
    Error(TenancyError),
}

/// The result of the catalog layer's problems. A name conflict is an error
/// (`tool-catalog-conflict`); any other problem is a `tool-catalog-invalid` rejection.
pub(crate) fn problems_to_stop(problems: Vec<CatalogProblem>) -> PlanStop {
    if problems.iter().any(|p| matches!(p, CatalogProblem::Conflict { .. })) {
        return PlanStop::Error(TenancyError::ToolCatalogConflict);
    }
    let rejections = problems
        .into_iter()
        .map(|p| match p {
            CatalogProblem::Invalid { name: Some(name), message } => ToolPolicyRejection::new(RejectionReason::CatalogInvalid, format!("{name}: {message}")),
            CatalogProblem::Invalid { name: None, message } => ToolPolicyRejection::new(RejectionReason::CatalogInvalid, message),
            CatalogProblem::Conflict { .. } => unreachable!("a conflict returned above"),
        })
        .collect();
    PlanStop::Rejected(rejections)
}

/// A new entry with the `name_key` of a stored entry but another display name is a conflict:
/// the match key is the same, so the two names are one tool (spec section 4.1).
fn check_display_names(current: &[StoredTool], upserts: &[ToolEntry]) -> Result<(), PlanStop> {
    for entry in upserts {
        if current
            .iter()
            .any(|stored| stored.entry.name.name_key == entry.name.name_key && stored.entry.name.display != entry.name.display)
        {
            return Err(PlanStop::Error(TenancyError::ToolCatalogConflict));
        }
    }
    Ok(())
}

pub(crate) fn plan_put(current: &[StoredTool], entry: ToolEntry) -> Result<CatalogPlan, PlanStop> {
    let upserts = vec![entry];
    check_display_names(current, &upserts)?;
    Ok(CatalogPlan {
        upserts,
        deletes: Vec::new(),
        skipped: Vec::new(),
    })
}

/// The tool to delete must exist, else `NotFound`. The name matches by its match key.
pub(crate) fn plan_delete(current: &[StoredTool], name: &str) -> Result<CatalogPlan, PlanStop> {
    let key = name_key(name);
    if !current.iter().any(|stored| stored.entry.name.name_key == key) {
        return Err(PlanStop::Error(TenancyError::NotFound));
    }
    Ok(CatalogPlan {
        upserts: Vec::new(),
        deletes: vec![key],
        skipped: Vec::new(),
    })
}

/// With `replace`, every stored tool that is not in the batch is deleted.
pub(crate) fn plan_import(current: &[StoredTool], imported: ImportedTools, replace: bool) -> Result<CatalogPlan, PlanStop> {
    check_display_names(current, &imported.tools)?;
    let deletes = if replace {
        let kept: BTreeSet<&str> = imported.tools.iter().map(|t| t.name.name_key.as_str()).collect();
        current.iter().map(|s| s.entry.name.name_key.clone()).filter(|k| !kept.contains(k.as_str())).collect()
    } else {
        Vec::new()
    };
    Ok(CatalogPlan {
        upserts: imported.tools,
        deletes,
        skipped: imported.skipped,
    })
}

/// The catalog after the plan, sorted by `name_key`.
pub(crate) fn apply_plan(current: &[StoredTool], plan: &CatalogPlan) -> Vec<ToolEntry> {
    let mut next: Vec<ToolEntry> = current
        .iter()
        .map(|s| s.entry.clone())
        .filter(|e| !plan.deletes.contains(&e.name.name_key) && !plan.upserts.iter().any(|u| u.name.name_key == e.name.name_key))
        .collect();
    next.extend(plan.upserts.iter().cloned());
    next.sort_by(|a, b| a.name.name_key.cmp(&b.name.name_key));
    next
}

/// A catalog over the tool limit is a `tool-catalog-invalid` rejection.
pub(crate) fn check_tool_limit(next: &[ToolEntry], max_tools: usize) -> Result<(), PlanStop> {
    if next.len() > max_tools {
        return Err(PlanStop::Rejected(vec![ToolPolicyRejection::new(
            RejectionReason::CatalogInvalid,
            format!("the catalog would have {} tools; the limit is {max_tools}", next.len()),
        )]));
    }
    Ok(())
}

/// The whole catalog after the plan as stored, sorted by `name_key`. A kept tool keeps its
/// timestamps. An upserted tool gets `now` as in [`stored_upserts`].
pub(crate) fn stored_next(current: &[StoredTool], plan: &CatalogPlan, now: DateTime<Utc>) -> Vec<StoredTool> {
    let mut next: Vec<StoredTool> = current
        .iter()
        .filter(|s| !plan.deletes.contains(&s.entry.name.name_key) && !plan.upserts.iter().any(|u| u.name.name_key == s.entry.name.name_key))
        .cloned()
        .collect();
    next.extend(stored_upserts(current, plan, now));
    next.sort_by(|a, b| a.entry.name.name_key.cmp(&b.entry.name.name_key));
    next
}

/// The write time for the size check: the second of `now` with the largest nanosecond value.
/// The write stores a later `now`. Its second has the same varint length, and no nanosecond value
/// encodes longer than 999 999 999. So the size of the check is an upper bound of the stored size.
pub(crate) fn size_check_time(now: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(now.timestamp(), 999_999_999).unwrap_or(now)
}

/// A catalog whose `ListToolsResponse` encoding (all tools, one response) is over `max_bytes` is a
/// `tool-catalog-invalid` rejection.
pub(crate) fn check_catalog_size(encoded: usize, max_bytes: usize) -> Result<(), PlanStop> {
    if encoded > max_bytes {
        return Err(PlanStop::Rejected(vec![ToolPolicyRejection::new(
            RejectionReason::CatalogInvalid,
            format!("the catalog would encode as a {encoded}-byte ListToolsResponse; the limit is {max_bytes} bytes (tool_policies.max_catalog_response_bytes)"),
        )]));
    }
    Ok(())
}

/// The upserted entries as stored: an entry that replaces a stored one keeps its `created_at`.
pub(crate) fn stored_upserts(current: &[StoredTool], plan: &CatalogPlan, now: DateTime<Utc>) -> Vec<StoredTool> {
    plan.upserts
        .iter()
        .map(|entry| StoredTool {
            entry: entry.clone(),
            created_at: current.iter().find(|s| s.entry.name.name_key == entry.name.name_key).map_or(now, |s| s.created_at),
            updated_at: now,
        })
        .collect()
}
