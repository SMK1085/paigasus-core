// SPDX-License-Identifier: Apache-2.0

//! The tool catalog and tool policy set routes (SMA-774 Task 13, spec sections 6.2, 7 and 9.1).
//! Each handler is a thin extract -> service call -> JSON map, the HTTP twin of
//! `grpc::tool_policies`.
//!
//! **Capability (ruling D8).** `app_routes` merges [`router`] only when
//! `capabilities.tool_policies` is true. With the feature off, every route here answers 404, as it
//! would on a build that predates the feature.
//!
//! **A rejection is `200 {"result":"rejected","rejections":[...]}`**, and an accept is
//! `200 {"result":"accepted", ...}`. Bodies use the snake_case field names of the proto messages.
//!
//! **The floor routes take no organization.** The service authorizes them at Root.
//! Import has its own path, `/tool-imports`, so a tool named `import` stays reachable at
//! `/tools/import`.

use axum::extract::State;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use paigasus_iam_core::tool_policy::{ImportFormat, StoredTool, ToolKind, ToolPolicySetKey, ToolSource};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1 as pb;
use serde_json::{Value, json};

use super::dto::{DeleteToolQuery, ImportFormatDto, ImportToolsBody, PageQuery, PolicySetVersionQuery, PutPolicySetBody, PutToolBody, ToolKindDto};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{OrganizationId, ToolNameSegment, UuidAndSegmentPath, UuidPath};
use super::query::EnvelopeQuery;
use super::{AppState, ToolPolicySvc};
use crate::adapters::auth::AuthContext;
use crate::adapters::grpc::tool_policies::{empty_set, version_to_pb};
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::application::tool_policies::{CatalogAccepted, ToolPolicyRejection, WriteResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/organizations/{id}/tools", get(list_tools).post(put_tool))
        .route("/v1/organizations/{id}/tools/{name}", delete(delete_tool))
        .route("/v1/organizations/{id}/tool-imports", post(import_tools))
        .route("/v1/organizations/{id}/tool-policy-set", put(put_set).get(get_set))
        .route("/v1/tool-policy-floor", put(put_floor).get(get_floor))
}

fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// The service. The routes exist only with the capability, so `None` is a wiring defect.
fn svc(s: &AppState) -> Result<&ToolPolicySvc, ApiError> {
    s.tool_policies.as_ref().ok_or_else(|| {
        tracing::error!("a tool policy route ran without ToolPolicyService");
        ApiError(TenancyError::Internal)
    })
}

fn rejected(rejections: &[ToolPolicyRejection]) -> Json<Value> {
    let rejections: Vec<Value> = rejections
        .iter()
        .map(|r| json!({"policy_id": r.policy_id, "line": r.line, "column": r.column, "reason": r.reason.code(), "message": r.message}))
        .collect();
    Json(json!({"result": "rejected", "rejections": rejections}))
}

fn tool_json(tool: &StoredTool) -> Value {
    let entry = &tool.entry;
    json!({
        "name_key": entry.name.name_key,
        "display_name": entry.name.display,
        "kind": match entry.kind { ToolKind::Function => "function", ToolKind::Custom => "custom" },
        "input_schema": entry.input_schema,
        "cedar_input_type": entry.cedar_input_type,
        "dropped_fields": entry.dropped_fields.iter().map(|d| json!({"path": d.path, "reason": d.reason})).collect::<Vec<_>>(),
        "source": match entry.source { ToolSource::Manual => "manual", ToolSource::OpenAi => "openai", ToolSource::Mcp => "mcp" },
        "created_at": tool.created_at.to_rfc3339(),
        "updated_at": tool.updated_at.to_rfc3339(),
    })
}

fn catalog_response(r: WriteResult<CatalogAccepted>) -> Json<Value> {
    match r {
        WriteResult::Accepted(a) => Json(json!({
            "result": "accepted",
            "catalog_revision": a.catalog_revision,
            "tool_policy_set_version": a.set_version,
            "tools": a.tools.iter().map(tool_json).collect::<Vec<_>>(),
            "skipped": a.skipped.iter().map(|(index, ty)| json!({"index": index, "type": ty})).collect::<Vec<_>>(),
        })),
        WriteResult::Rejected(rejections) => rejected(&rejections),
    }
}

/// The set as JSON, from the same wire message gRPC serves. The compiled history conditions are
/// not part of this JSON form.
fn set_json(v: &pb::ToolPolicySetVersion) -> Value {
    let kind = if v.kind == pb::ToolPolicySetKind::Floor as i32 { "floor" } else { "tenant" };
    let status = match pb::ToolPolicyVersionStatus::try_from(v.status) {
        Ok(pb::ToolPolicyVersionStatus::Live) => Some("live"),
        Ok(pb::ToolPolicyVersionStatus::Superseded) => Some("superseded"),
        Ok(pb::ToolPolicyVersionStatus::Held) => Some("held"),
        Ok(pb::ToolPolicyVersionStatus::Discarded) => Some("discarded"),
        _ => None,
    };
    let origin = match pb::ToolPolicyVersionOrigin::try_from(v.origin) {
        Ok(pb::ToolPolicyVersionOrigin::Save) => Some("save"),
        Ok(pb::ToolPolicyVersionOrigin::Starter) => Some("starter"),
        Ok(pb::ToolPolicyVersionOrigin::CatalogChange) => Some("catalog_change"),
        Ok(pb::ToolPolicyVersionOrigin::Recompile) => Some("recompile"),
        Ok(pb::ToolPolicyVersionOrigin::Accept) => Some("accept"),
        _ => None,
    };
    let compiled = v.compiled.as_ref().map(|c| {
        json!({
            "version": c.version,
            "compiler_generation": c.compiler_generation,
            "schema_revision": c.schema_revision,
            "catalog_revision": c.catalog_revision,
            "cedar_policies": c.cedar_policies,
            "cedar_schema": serde_json::from_str::<Value>(&c.cedar_schema).unwrap_or(Value::Null),
            "max_window_ms": c.max_window_ms,
            "catalog_names": c.catalog_names,
        })
    });
    json!({
        "kind": kind,
        "org_id": v.org_id,
        "version": v.version,
        "source": v.source,
        "status": status,
        "origin": origin,
        "compiler_generation": v.compiler_generation,
        "schema_revision": v.schema_revision,
        "catalog_revision": v.catalog_revision,
        "created_by": v.created_by,
        "created_at": v.created_at.as_ref().and_then(|t| chrono::DateTime::from_timestamp(t.seconds, u32::try_from(t.nanos).unwrap_or(0))).map(|t| t.to_rfc3339()),
        "compiled": compiled,
    })
}

fn set_response(r: WriteResult<paigasus_iam_core::tool_policy::ToolPolicyVersion>) -> Result<Json<Value>, ApiError> {
    match r {
        WriteResult::Accepted(v) => Ok(Json(json!({"result": "accepted", "set": set_json(&version_to_pb(&v)?)}))),
        WriteResult::Rejected(rejections) => Ok(rejected(&rejections)),
    }
}

fn read_response(found: Option<paigasus_iam_core::tool_policy::ToolPolicyVersion>, key: ToolPolicySetKey) -> Result<Json<Value>, ApiError> {
    let wire = match found {
        Some(v) => version_to_pb(&v)?,
        None => empty_set(key),
    };
    Ok(Json(set_json(&wire)))
}

async fn list_tools(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>, EnvelopeQuery(q): EnvelopeQuery<PageQuery>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let page = Page::new(q.limit, q.offset)?;
    let (tools, catalog_revision) = svc.list_tools(&actor_prn(&ctx), path.id, page).await?;
    Ok(Json(json!({"tools": tools.iter().map(tool_json).collect::<Vec<_>>(), "catalog_revision": catalog_revision})))
}

async fn put_tool(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>, EnvelopeJson(b): EnvelopeJson<PutToolBody>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let kind = match b.kind {
        ToolKindDto::Function => ToolKind::Function,
        ToolKindDto::Custom => ToolKind::Custom,
    };
    let out = svc.put_tool(&actor_prn(&ctx), path.id, &b.name, kind, b.input_schema, b.expected_catalog_revision).await?;
    Ok(catalog_response(out))
}

async fn delete_tool(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidAndSegmentPath<OrganizationId, ToolNameSegment>,
    EnvelopeQuery(q): EnvelopeQuery<DeleteToolQuery>,
) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let out = svc.delete_tool(&actor_prn(&ctx), path.id, &path.segment, q.expected_catalog_revision).await?;
    Ok(catalog_response(out))
}

async fn import_tools(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    EnvelopeJson(b): EnvelopeJson<ImportToolsBody>,
) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let format = match b.format {
        ImportFormatDto::OpenaiChatCompletions => ImportFormat::OpenAiChatCompletions,
        ImportFormatDto::OpenaiResponses => ImportFormat::OpenAiResponses,
        ImportFormatDto::McpToolsList => ImportFormat::McpToolsList,
    };
    let namespace = b.namespace.as_deref().filter(|n| !n.is_empty());
    let out = svc
        .import_tools(&actor_prn(&ctx), path.id, format, &b.document, namespace, b.replace, b.expected_catalog_revision)
        .await?;
    Ok(catalog_response(out))
}

async fn put_set(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>, EnvelopeJson(b): EnvelopeJson<PutPolicySetBody>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    set_response(svc.put_set(&actor_prn(&ctx), path.id, &b.source, b.expected_version).await?)
}

async fn get_set(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    EnvelopeQuery(q): EnvelopeQuery<PolicySetVersionQuery>,
) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    read_response(svc.get_set(&actor_prn(&ctx), path.id, q.version).await?, ToolPolicySetKey::Tenant(path.id))
}

async fn put_floor(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<PutPolicySetBody>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    set_response(svc.put_floor(&actor_prn(&ctx), &b.source, b.expected_version).await?)
}

async fn get_floor(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeQuery(q): EnvelopeQuery<PolicySetVersionQuery>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    read_response(svc.get_floor(&actor_prn(&ctx), q.version).await?, ToolPolicySetKey::Floor)
}
