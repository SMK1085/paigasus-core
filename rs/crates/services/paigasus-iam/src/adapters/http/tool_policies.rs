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
//! **The settings and the compiled-policy fetch (SMA-775).** `PUT .../tool-policy-settings` writes
//! the mode and the fail mode. The first enable to `log_only` also saves the starter set. When
//! two first enables race, the loser gets a version conflict (409). IAM writes nothing for the
//! loser, and the change of the winner is saved. The loser reads the settings and retries if it
//! still needs a change. The retry succeeds. `GET .../compiled-tool-policies` and
//! `GET /v1/compiled-tool-policy-floor` are the reads of the gateway service identity.
//!
//! **The floor routes take no organization.** The service authorizes them at Root.
//! Import has its own path, `/tool-imports`, so a tool named `import` stays reachable at
//! `/tools/import`.

use axum::extract::State;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use paigasus_iam_core::tool_policy::{FailMode, ImportFormat, StoredTool, ToolKind, ToolPolicyMode, ToolPolicySetKey, ToolPolicySettings, ToolSource};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1 as pb;
use serde_json::{Value, json};

use super::dto::{
    DeleteToolQuery, FailModeDto, ImportFormatDto, ImportToolsBody, PageQuery, PolicySetVersionQuery, PutPolicySetBody, PutToolBody, PutToolPolicySettingsBody, ToolKindDto, ToolPolicyModeDto,
};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{HeldVersionSegment, OrganizationId, StringPath, ToolNameSegment, UuidAndSegmentPath, UuidPath};
use super::query::EnvelopeQuery;
use super::{AppState, ToolPolicySvc};
use crate::adapters::auth::AuthContext;
use crate::adapters::tool_policy_wire::{compiled_or_empty, empty_set, held_to_pb, run_to_pb, version_to_pb};
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::application::tool_policies::{CatalogAccepted, HeldTarget, ToolPolicyRejection, WriteResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/organizations/{id}/tools", get(list_tools).post(put_tool))
        .route("/v1/organizations/{id}/tools/{name}", delete(delete_tool))
        .route("/v1/organizations/{id}/tool-imports", post(import_tools))
        .route("/v1/organizations/{id}/tool-policy-set", put(put_set).get(get_set))
        .route("/v1/tool-policy-floor", put(put_floor).get(get_floor))
        .route("/v1/organizations/{id}/tool-policy-settings", put(put_settings).get(get_settings))
        .route("/v1/organizations/{id}/compiled-tool-policies", get(get_compiled))
        .route("/v1/compiled-tool-policy-floor", get(get_compiled_floor))
        .route("/v1/organizations/{id}/held-tool-policy-versions", get(list_held_tenant))
        .route("/v1/held-tool-policy-versions", get(list_held_all))
        .route("/v1/organizations/{id}/held-tool-policy-versions/{version}/accept", post(accept_held_tenant))
        .route("/v1/tool-policy-floor/held-versions/{version}/accept", post(accept_held_floor))
        .route("/v1/organizations/{id}/held-tool-policy-versions/{version}/discard", post(discard_held_tenant))
        .route("/v1/tool-policy-floor/held-versions/{version}/discard", post(discard_held_floor))
        .route("/v1/tool-policy-recompile", post(run_recompile))
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

/// The set as JSON, from the same wire message gRPC serves. The compiled message keeps its proto field names.
fn set_json(v: &pb::ToolPolicySetVersion) -> Result<Value, ApiError> {
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
    let compiled = v.compiled.as_ref().map(compiled_json).transpose()?;
    Ok(json!({
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
    }))
}

fn name_diff_json(d: Option<&pb::NameDiff>) -> Value {
    let d = d.cloned().unwrap_or_default();
    json!({"added": d.added, "removed": d.removed, "changed": d.changed})
}

/// The held row as JSON, from the same wire message gRPC serves.
fn held_json(h: &pb::HeldToolPolicyVersion) -> Result<Value, ApiError> {
    let kind = if h.kind == pb::ToolPolicySetKind::Floor as i32 { "floor" } else { "tenant" };
    let status = match pb::ToolPolicyVersionStatus::try_from(h.status) {
        Ok(pb::ToolPolicyVersionStatus::Held) => Some("held"),
        Ok(pb::ToolPolicyVersionStatus::Discarded) => Some("discarded"),
        _ => None,
    };
    let report = h.report.as_ref().map(|r| {
        json!({
            "result": enum_name::<pb::HeldToolPolicyResult>(r.result, pb::HeldToolPolicyResult::as_str_name),
            "policies": name_diff_json(r.policies.as_ref()),
            "conditions": name_diff_json(r.conditions.as_ref()),
            "schema": name_diff_json(r.schema.as_ref()),
            "rejections": r.rejections.as_ref().map(|x| x.rejections.iter().map(|r| json!({"policy_id": r.policy_id, "line": r.line, "column": r.column, "reason": r.reason, "message": r.message})).collect::<Vec<_>>()).unwrap_or_default(),
        })
    });
    let compiled = h.compiled.as_ref().map(compiled_json).transpose()?;
    Ok(json!({
        "kind": kind,
        "org_id": h.org_id,
        "version": h.version,
        "base_version": h.base_version,
        "source": h.source,
        "status": status,
        "compiler_generation": h.compiler_generation,
        "schema_revision": h.schema_revision,
        "catalog_revision": h.catalog_revision,
        "report": report,
        "created_by": h.created_by,
        "created_at": h.created_at.as_ref().and_then(|t| chrono::DateTime::from_timestamp(t.seconds, u32::try_from(t.nanos).unwrap_or(0))).map(|t| t.to_rfc3339()),
        "compiled": compiled,
    }))
}

/// The held version number of a path segment. A segment that is not a positive number is
/// `invalid-path-segment`.
fn held_version(segment: &str) -> Result<u64, ApiError> {
    segment.parse::<u64>().ok().filter(|v| *v > 0).ok_or(ApiError(TenancyError::InvalidPathSegment("held_version")))
}

/// The name of a proto enum value, or its number when the value is unknown.
fn enum_name<E: TryFrom<i32>>(raw: i32, name: impl Fn(&E) -> &'static str) -> String {
    E::try_from(raw).map_or_else(|_| raw.to_string(), |e| name(&e).to_string())
}

fn predicate_json(p: &pb::HistoryPredicate) -> Value {
    use pb::history_predicate::Node;
    let pair = |pair: &pb::PredicatePair| json!({"left": pair.left.as_deref().map(predicate_json), "right": pair.right.as_deref().map(predicate_json)});
    match &p.node {
        None => json!({}),
        Some(Node::EventKind(k)) => json!({"event_kind": enum_name::<pb::HistoryEventKind>(*k, pb::HistoryEventKind::as_str_name)}),
        Some(Node::Tool(t)) => json!({"tool": {"name_key": t.name_key, "any_catalog_tool": t.any_catalog_tool}}),
        Some(Node::Field(f)) => {
            use pb::literal_value::Value as Lit;
            let value = f.value.as_ref().and_then(|v| v.value.as_ref()).map(|v| match v {
                Lit::LongValue(n) => json!({"long_value": n}),
                Lit::StringValue(x) => json!({"string_value": x}),
                Lit::BoolValue(b) => json!({"bool_value": b}),
                Lit::DecimalValue(d) => json!({"decimal_value": d}),
            });
            json!({"field": {"path": f.path, "op": enum_name::<pb::ComparisonOp>(f.op, pb::ComparisonOp::as_str_name), "value": value}})
        }
        Some(Node::AllOf(x)) => json!({"all_of": pair(x)}),
        Some(Node::AnyOf(x)) => json!({"any_of": pair(x)}),
        Some(Node::Negation(x)) => json!({"negation": predicate_json(x)}),
    }
}

fn condition_json(c: &pb::HistoryCondition) -> Value {
    use pb::history_condition::Shape;
    let op = |raw: i32| enum_name::<pb::ComparisonOp>(raw, pb::ComparisonOp::as_str_name);
    let shape = match &c.shape {
        None => json!({}),
        Some(Shape::Formerly(f)) => json!({"formerly": {"predicate": f.predicate.as_ref().map(predicate_json)}}),
        Some(Shape::CountWithin(x)) => json!({"count_within": {"predicate": x.predicate.as_ref().map(predicate_json), "op": op(x.op), "threshold": x.threshold}}),
        Some(Shape::SumWithin(x)) => {
            json!({"sum_within": {"predicate": x.predicate.as_ref().map(predicate_json), "field_path": x.field_path, "op": op(x.op), "threshold": x.threshold}})
        }
    };
    let mut out = json!({
        "slot": c.slot,
        "policy_id": c.policy_id,
        "scope": enum_name::<pb::HistoryScope>(c.scope, pb::HistoryScope::as_str_name),
        "window_ms": c.window_ms,
    });
    if let (Some(out), Some(shape)) = (out.as_object_mut(), shape.as_object()) {
        out.extend(shape.clone());
    }
    out
}

/// The compiled message as JSON, with the proto field names. A schema text that is not JSON is a
/// storage defect: it is logged and answered as `Internal`, as `version_to_pb` does.
fn compiled_json(c: &pb::CompiledToolPolicySet) -> Result<Value, ApiError> {
    // The empty answer (version 0, nothing stored) has no schema text: it renders as `null`.
    // A stored version starts at 1, so an empty schema text on a stored version stays a defect.
    let cedar_schema = if c.version == 0 && c.cedar_schema.is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(&c.cedar_schema).map_err(|error| {
            tracing::error!(error = %error, version = c.version, "the stored compiled schema is not JSON");
            ApiError(TenancyError::Internal)
        })?
    };
    Ok(json!({
        "kind": enum_name::<pb::ToolPolicySetKind>(c.kind, pb::ToolPolicySetKind::as_str_name),
        "org_id": c.org_id,
        "version": c.version,
        "compiler_generation": c.compiler_generation,
        "schema_revision": c.schema_revision,
        "catalog_revision": c.catalog_revision,
        "cedar_policies": c.cedar_policies,
        "cedar_schema": cedar_schema,
        "conditions": c.conditions.iter().map(condition_json).collect::<Vec<_>>(),
        "max_window_ms": c.max_window_ms,
        "catalog_names": c.catalog_names,
    }))
}

fn set_response(r: WriteResult<paigasus_iam_core::tool_policy::ToolPolicyVersion>) -> Result<Json<Value>, ApiError> {
    match r {
        WriteResult::Accepted(v) => Ok(Json(json!({"result": "accepted", "set": set_json(&version_to_pb(&v)?)?}))),
        WriteResult::Rejected(rejections) => Ok(rejected(&rejections)),
    }
}

fn read_response(found: Option<paigasus_iam_core::tool_policy::ToolPolicyVersion>, key: ToolPolicySetKey) -> Result<Json<Value>, ApiError> {
    let wire = match found {
        Some(v) => version_to_pb(&v)?,
        None => empty_set(key),
    };
    Ok(Json(set_json(&wire)?))
}

fn settings_json(s: &ToolPolicySettings) -> Value {
    json!({
        "mode": s.mode.as_str(),
        "fail_mode": s.fail_mode.as_str(),
        "updated_at": s.updated_at.map(|t| t.to_rfc3339()),
        "updated_by": s.updated_by,
    })
}

async fn get_settings(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidPath<OrganizationId>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    Ok(Json(settings_json(&svc.get_settings(&actor_prn(&ctx), path.id).await?)))
}

/// Writes the settings. A first enable that races with another first enable can answer 409
/// `tool-policy-version-conflict`. IAM writes nothing for that request. Read the settings, and
/// retry if the change is still needed.
async fn put_settings(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    EnvelopeJson(b): EnvelopeJson<PutToolPolicySettingsBody>,
) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let mode = match b.mode {
        ToolPolicyModeDto::Off => ToolPolicyMode::Off,
        ToolPolicyModeDto::LogOnly => ToolPolicyMode::LogOnly,
        ToolPolicyModeDto::Enforce => ToolPolicyMode::Enforce,
    };
    let fail_mode = match b.fail_mode {
        FailModeDto::Open => FailMode::Open,
        FailModeDto::Closed => FailMode::Closed,
    };
    let accepted = svc.put_settings(&actor_prn(&ctx), path.id, mode, fail_mode).await?;
    Ok(Json(json!({"settings": settings_json(&accepted.settings), "starter_version": accepted.starter_version})))
}

async fn get_compiled(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    EnvelopeQuery(q): EnvelopeQuery<PolicySetVersionQuery>,
) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let fetched = svc.get_compiled(&actor_prn(&ctx), path.id, q.version).await?;
    let set = compiled_or_empty(fetched.set.as_ref(), ToolPolicySetKey::Tenant(path.id))?;
    Ok(Json(
        json!({"set": compiled_json(&set)?, "settings": settings_json(&fetched.settings), "floor_version": fetched.floor_version}),
    ))
}

async fn get_compiled_floor(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeQuery(q): EnvelopeQuery<PolicySetVersionQuery>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let floor = svc.get_compiled_floor(&actor_prn(&ctx), q.version).await?;
    let set = compiled_or_empty(floor.as_ref(), ToolPolicySetKey::Floor)?;
    Ok(Json(json!({"set": compiled_json(&set)?})))
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

/// Lists the held rows of one organization. `limit` defaults to 50 and is at most 200, as on
/// every IAM list route.
async fn list_held_tenant(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<OrganizationId>,
    EnvelopeQuery(q): EnvelopeQuery<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    list_held(&s, &ctx, Some(path.id), q).await
}

/// Lists the held rows of every set, the floor included. Needs `ManageToolPolicyRecompile`.
async fn list_held_all(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeQuery(q): EnvelopeQuery<PageQuery>) -> Result<Json<Value>, ApiError> {
    list_held(&s, &ctx, None, q).await
}

async fn list_held(s: &AppState, ctx: &AuthContext, org: Option<uuid::Uuid>, q: PageQuery) -> Result<Json<Value>, ApiError> {
    let svc = svc(s)?;
    let page = Page::new(q.limit, q.offset)?;
    let held = svc.list_held(&actor_prn(ctx), org, page).await?;
    let versions = held.iter().map(|h| held_json(&held_to_pb(h)?)).collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(json!({"versions": versions})))
}

async fn accept_held_tenant(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidAndSegmentPath<OrganizationId, HeldVersionSegment>) -> Result<Json<Value>, ApiError> {
    accept_held(&s, &ctx, HeldTarget::Tenant(path.id), &path.segment).await
}

async fn accept_held_floor(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: StringPath<HeldVersionSegment>) -> Result<Json<Value>, ApiError> {
    accept_held(&s, &ctx, HeldTarget::Floor, &path.value).await
}

async fn accept_held(s: &AppState, ctx: &AuthContext, target: HeldTarget, segment: &str) -> Result<Json<Value>, ApiError> {
    let svc = svc(s)?;
    let version = svc.accept_held(&actor_prn(ctx), target, held_version(segment)?).await?;
    Ok(Json(json!({"accepted": set_json(&version_to_pb(&version)?)?})))
}

async fn discard_held_tenant(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: UuidAndSegmentPath<OrganizationId, HeldVersionSegment>) -> Result<Json<Value>, ApiError> {
    discard_held(&s, &ctx, HeldTarget::Tenant(path.id), &path.segment).await
}

async fn discard_held_floor(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, path: StringPath<HeldVersionSegment>) -> Result<Json<Value>, ApiError> {
    discard_held(&s, &ctx, HeldTarget::Floor, &path.value).await
}

async fn discard_held(s: &AppState, ctx: &AuthContext, target: HeldTarget, segment: &str) -> Result<Json<Value>, ApiError> {
    let svc = svc(s)?;
    let held = svc.discard_held(&actor_prn(ctx), target, held_version(segment)?).await?;
    Ok(Json(json!({"discarded": held_json(&held_to_pb(&held)?)?})))
}

/// The HTTP name of a run status. Each status has its own arm. `run_to_pb` never sets
/// `UNSPECIFIED` or a number outside the enum, so either one is a defect of this release: it is a
/// 500, not a guess such as `older_replica`.
fn run_status_name(status: i32) -> Result<&'static str, ApiError> {
    match pb::RecompileRunStatus::try_from(status) {
        Ok(pb::RecompileRunStatus::Ran) => Ok("ran"),
        Ok(pb::RecompileRunStatus::AlreadyRunning) => Ok("already_running"),
        Ok(pb::RecompileRunStatus::OlderReplica) => Ok("older_replica"),
        Ok(pb::RecompileRunStatus::Unspecified) | Err(_) => {
            tracing::error!(status, "a re-compile run has no known status");
            Err(ApiError(TenancyError::Internal))
        }
    }
}

/// Runs the re-compile job once and waits for it. A run with many stale sets can last longer than
/// the request timeout of the server.
async fn run_recompile(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Value>, ApiError> {
    let svc = svc(&s)?;
    let run = run_to_pb(&svc.run_recompile(&actor_prn(&ctx)).await?);
    let status = run_status_name(run.status)?;
    let results: Vec<Value> = run
        .results
        .iter()
        .map(|r| {
            json!({
                "kind": if r.kind == pb::ToolPolicySetKind::Floor as i32 { "floor" } else { "tenant" },
                "org_id": r.org_id,
                "base_version": r.base_version,
                "outcome": enum_name::<pb::RecompileOutcome>(r.outcome, pb::RecompileOutcome::as_str_name),
                "version": r.version,
            })
        })
        .collect();
    Ok(Json(json!({"status": status, "results": results})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::tool_policies::sets::{CompiledMeta, encode_compiled};
    use paigasus_iam_core::tool_policy::CompiledOutput;
    use prost::Message;

    /// Each run status has its own HTTP name. `UNSPECIFIED` and an unknown number are a 500.
    #[test]
    fn each_run_status_has_its_own_name_and_an_unknown_one_is_internal() {
        assert_eq!(run_status_name(pb::RecompileRunStatus::Ran as i32).ok(), Some("ran"));
        assert_eq!(run_status_name(pb::RecompileRunStatus::AlreadyRunning as i32).ok(), Some("already_running"));
        assert_eq!(run_status_name(pb::RecompileRunStatus::OlderReplica as i32).ok(), Some("older_replica"));
        for unknown in [pb::RecompileRunStatus::Unspecified as i32, 99] {
            let code = run_status_name(unknown).err().map(|e| e.0.code());
            assert_eq!(code, Some(TenancyError::Internal.code()), "status {unknown}");
        }
    }

    /// The compile result fixture that holds two history conditions.
    const TENANT_ACCEPTED: &str = include_str!("../../../../../../tools/paigasus-policy-compiler/protocol/fixtures/result_tenant_accepted.json");

    /// IAM stores no set with a history condition until SMA-753, so the save path cannot reach
    /// this rendering. The test builds the stored bytes directly. SMA-753 may move it back to
    /// `tests/http_tool_policies.rs` as a save-and-read test.
    #[test]
    fn a_stored_set_renders_the_compiled_kind_and_conditions() {
        let fixture: Value = serde_json::from_str(TENANT_ACCEPTED).expect("the fixture parses");
        let out: CompiledOutput = serde_json::from_value(fixture["compiled"].clone()).expect("the compiled output parses");
        let meta = CompiledMeta {
            key: ToolPolicySetKey::Tenant(uuid::Uuid::nil()),
            version: 1,
            compiler_generation: 1,
            schema_revision: 1,
            catalog_revision: Some(0),
            max_window_ms: 3_600_000,
            catalog_names: Vec::new(),
        };
        let compiled = pb::CompiledToolPolicySet::decode(encode_compiled(&meta, &out).as_slice()).expect("decode");
        let compiled = compiled_json(&compiled).unwrap_or_else(|_| panic!("the compiled set renders"));
        assert_eq!(compiled["kind"], "TOOL_POLICY_SET_KIND_TENANT");
        let conditions = compiled["conditions"].as_array().expect("conditions");
        assert_eq!(conditions.len(), 2);
        let formerly = &conditions[0];
        assert_eq!(formerly["slot"], "p_no_hsend_hafter_hsearch_0__temporal_0");
        assert_eq!(formerly["policy_id"], "no-send-after-search");
        assert_eq!((formerly["scope"].as_str(), formerly["window_ms"].as_u64()), (Some("HISTORY_SCOPE_SESSION"), Some(600_000)));
        let all_of = &formerly["formerly"]["predicate"]["all_of"];
        assert_eq!(all_of["left"]["all_of"]["left"]["event_kind"], "HISTORY_EVENT_KIND_RESPONSE");
        assert_eq!(all_of["left"]["all_of"]["right"]["tool"]["name_key"], "search");
        assert_eq!(all_of["right"]["field"]["path"], json!(["q"]));
        assert_eq!(all_of["right"]["field"]["op"], "COMPARISON_OP_EQ");
        assert_eq!(all_of["right"]["field"]["value"]["string_value"], "secret");
        let count = &conditions[1];
        assert_eq!(count["scope"], "HISTORY_SCOPE_PRINCIPAL");
        assert_eq!(
            (count["count_within"]["op"].as_str(), count["count_within"]["threshold"].as_i64()),
            (Some("COMPARISON_OP_GT"), Some(50))
        );
    }
}
