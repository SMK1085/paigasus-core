// SPDX-License-Identifier: Apache-2.0

//! `ToolPolicyGrpc`: the `ToolPolicyService` gRPC server (8 RPCs, SMA-774 Task 13, spec sections
//! 6.2, 7 and 9.1). A thin adapter over `AppState.tool_policies`: check the capability, parse the
//! wire values, call the service with the bearer-resolved actor, convert the result.
//!
//! **Capability (ruling D8).** The service is always registered. Each RPC first checks
//! `state.capabilities.tool_policies` and answers `capability-disabled("iam.tool.policies")` when
//! the feature is off, reads included. The check runs before authorization, so a caller learns
//! nothing about the organization of a disabled deployment.
//!
//! **Authorization lives in the application service.** The tenant RPCs name an organization by
//! PRN. The two floor RPCs have no organization field: the service authorizes them at Root and
//! this adapter takes no PRN from the request for them.
//!
//! **A rejection is a successful response** (`oneof result { accepted, rejected }`).

use std::time::Instant;

use paigasus_iam_core::tool_policy::{ImportFormat, ToolKind, ToolPolicySetKey, ToolPolicyVersion};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1 as pb;
use paigasus_proto::paigasus::iam::v1::tool_policy_service_server::ToolPolicyService;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::{AppState, ToolPolicySvc};
use crate::adapters::tool_policy_wire::{empty_set, version_to_pb};
use crate::application::error::TenancyError;
use crate::application::tool_policies::wire::tool_to_pb;
use crate::application::tool_policies::{CatalogAccepted, RejectionReason, ToolPolicyRejection, WriteResult};

/// The capability key in `capability-disabled` metadata.
const CAPABILITY: &str = "iam.tool.policies";

/// The `ToolPolicyService` gRPC server.
pub struct ToolPolicyGrpc {
    state: AppState,
}

impl ToolPolicyGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The service, or `capability-disabled` when the feature is off.
    fn svc(&self) -> Result<&ToolPolicySvc, Status> {
        match (&self.state.tool_policies, self.state.capabilities.tool_policies) {
            (Some(svc), true) => Ok(svc),
            _ => Err(convert::capability_disabled(CAPABILITY)),
        }
    }
}

fn actor<T>(request: &Request<T>) -> Result<Prn, Status> {
    request
        .extensions()
        .get::<AuthContext>()
        .map(|ctx| ctx.principal_id.prn().clone())
        .ok_or_else(convert::missing_auth_context)
}

fn org_uuid(org_prn: &str) -> Result<Uuid, Status> {
    convert::node_uuid(org_prn, "organization").map(|(id, _)| id)
}

/// The wire kind. `UNSPECIFIED` and an unknown number are a missing field.
fn kind_from_wire(raw: i32) -> Result<ToolKind, Status> {
    match pb::ToolKind::try_from(raw) {
        Ok(pb::ToolKind::Function) => Ok(ToolKind::Function),
        Ok(pb::ToolKind::Custom) => Ok(ToolKind::Custom),
        _ => Err(convert::status_to_grpc(TenancyError::MissingRequiredField("kind"))),
    }
}

fn format_from_wire(raw: i32) -> Result<ImportFormat, Status> {
    match pb::ToolImportFormat::try_from(raw) {
        Ok(pb::ToolImportFormat::OpenaiChatCompletions) => Ok(ImportFormat::OpenAiChatCompletions),
        Ok(pb::ToolImportFormat::OpenaiResponses) => Ok(ImportFormat::OpenAiResponses),
        Ok(pb::ToolImportFormat::McpToolsList) => Ok(ImportFormat::McpToolsList),
        _ => Err(convert::status_to_grpc(TenancyError::MissingRequiredField("format"))),
    }
}

/// Text that holds JSON. A text that does not parse is a `tool-catalog-invalid` rejection.
fn parse_json(text: &str, field: &str) -> Result<serde_json::Value, ToolPolicyRejection> {
    serde_json::from_str(text).map_err(|error| ToolPolicyRejection::new(RejectionReason::CatalogInvalid, format!("{field} is not valid JSON: {error}")))
}

pub(crate) fn rejection_to_pb(r: &ToolPolicyRejection) -> pb::ToolPolicyRejection {
    pb::ToolPolicyRejection {
        policy_id: r.policy_id.clone().unwrap_or_default(),
        line: r.line.unwrap_or(0),
        column: r.column.unwrap_or(0),
        reason: r.reason.code().to_string(),
        message: r.message.clone(),
    }
}

pub(crate) fn rejections_to_pb(rejections: &[ToolPolicyRejection]) -> pb::ToolPolicyRejections {
    pb::ToolPolicyRejections {
        rejections: rejections.iter().map(rejection_to_pb).collect(),
    }
}

fn accepted_to_pb(a: &CatalogAccepted) -> pb::CatalogWriteAccepted {
    pb::CatalogWriteAccepted {
        catalog_revision: a.catalog_revision,
        tool_policy_set_version: a.set_version,
        tools: a.tools.iter().map(tool_to_pb).collect(),
        skipped: a
            .skipped
            .iter()
            .map(|(index, ty)| pb::SkippedImportEntry {
                index: u32::try_from(*index).unwrap_or(u32::MAX),
                r#type: ty.clone(),
            })
            .collect(),
    }
}

fn set_result(r: Result<WriteResult<ToolPolicyVersion>, TenancyError>) -> Result<Result<pb::ToolPolicySetVersion, pb::ToolPolicyRejections>, Status> {
    match r.map_err(convert::status_to_grpc)? {
        WriteResult::Accepted(v) => Ok(Ok(version_to_pb(&v).map_err(convert::status_to_grpc)?)),
        WriteResult::Rejected(rejections) => Ok(Err(rejections_to_pb(&rejections))),
    }
}

fn catalog_result(r: Result<WriteResult<CatalogAccepted>, TenancyError>) -> Result<Result<pb::CatalogWriteAccepted, pb::ToolPolicyRejections>, Status> {
    match r.map_err(convert::status_to_grpc)? {
        WriteResult::Accepted(a) => Ok(Ok(accepted_to_pb(&a))),
        WriteResult::Rejected(rejections) => Ok(Err(rejections_to_pb(&rejections))),
    }
}

fn single_rejection(rejection: ToolPolicyRejection) -> pb::ToolPolicyRejections {
    rejections_to_pb(&[rejection])
}

#[tonic::async_trait]
impl ToolPolicyService for ToolPolicyGrpc {
    async fn put_tool(&self, request: Request<pb::PutToolRequest>) -> Result<Response<pb::PutToolResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::PutToolResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let kind = kind_from_wire(req.kind)?;
            let schema = if req.input_schema_json.is_empty() {
                Ok(None)
            } else {
                parse_json(&req.input_schema_json, "input_schema_json").map(Some)
            };
            let outcome = match schema {
                Ok(schema) => catalog_result(svc.put_tool(&actor, org, &req.name, kind, schema, req.expected_catalog_revision).await)?,
                Err(rejection) => Err(single_rejection(rejection)),
            };
            Ok(Response::new(pb::PutToolResponse {
                result: Some(match outcome {
                    Ok(a) => pb::put_tool_response::Result::Accepted(a),
                    Err(r) => pb::put_tool_response::Result::Rejected(r),
                }),
            }))
        }
        .await;
        record_grpc("ToolPolicy", "PutTool", started, &result);
        result
    }

    async fn delete_tool(&self, request: Request<pb::DeleteToolRequest>) -> Result<Response<pb::DeleteToolResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::DeleteToolResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let outcome = catalog_result(svc.delete_tool(&actor, org, &req.name, req.expected_catalog_revision).await)?;
            Ok(Response::new(pb::DeleteToolResponse {
                result: Some(match outcome {
                    Ok(a) => pb::delete_tool_response::Result::Accepted(a),
                    Err(r) => pb::delete_tool_response::Result::Rejected(r),
                }),
            }))
        }
        .await;
        record_grpc("ToolPolicy", "DeleteTool", started, &result);
        result
    }

    async fn list_tools(&self, request: Request<pb::ListToolsRequest>) -> Result<Response<pb::ListToolsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::ListToolsResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let (tools, catalog_revision) = svc.list_tools(&actor, org, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(pb::ListToolsResponse {
                tools: tools.iter().map(tool_to_pb).collect(),
                catalog_revision,
            }))
        }
        .await;
        record_grpc("ToolPolicy", "ListTools", started, &result);
        result
    }

    async fn import_tools(&self, request: Request<pb::ImportToolsRequest>) -> Result<Response<pb::ImportToolsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::ImportToolsResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let format = format_from_wire(req.format)?;
            let namespace = Some(req.namespace.as_str()).filter(|n| !n.is_empty());
            let outcome = match parse_json(&req.document_json, "document_json") {
                Ok(document) => catalog_result(svc.import_tools(&actor, org, format, &document, namespace, req.replace, req.expected_catalog_revision).await)?,
                Err(rejection) => Err(single_rejection(rejection)),
            };
            Ok(Response::new(pb::ImportToolsResponse {
                result: Some(match outcome {
                    Ok(a) => pb::import_tools_response::Result::Accepted(a),
                    Err(r) => pb::import_tools_response::Result::Rejected(r),
                }),
            }))
        }
        .await;
        record_grpc("ToolPolicy", "ImportTools", started, &result);
        result
    }

    async fn put_tool_policy_set(&self, request: Request<pb::PutToolPolicySetRequest>) -> Result<Response<pb::PutToolPolicySetResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::PutToolPolicySetResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let outcome = set_result(svc.put_set(&actor, org, &req.source, req.expected_version).await)?;
            Ok(Response::new(pb::PutToolPolicySetResponse {
                result: Some(match outcome {
                    Ok(v) => pb::put_tool_policy_set_response::Result::Accepted(v),
                    Err(r) => pb::put_tool_policy_set_response::Result::Rejected(r),
                }),
            }))
        }
        .await;
        record_grpc("ToolPolicy", "PutToolPolicySet", started, &result);
        result
    }

    async fn get_tool_policy_set(&self, request: Request<pb::GetToolPolicySetRequest>) -> Result<Response<pb::GetToolPolicySetResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::GetToolPolicySetResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let org = org_uuid(&req.org_prn)?;
            let set = match svc.get_set(&actor, org, req.version).await.map_err(convert::status_to_grpc)? {
                Some(v) => version_to_pb(&v).map_err(convert::status_to_grpc)?,
                None => empty_set(ToolPolicySetKey::Tenant(org)),
            };
            Ok(Response::new(pb::GetToolPolicySetResponse { set: Some(set) }))
        }
        .await;
        record_grpc("ToolPolicy", "GetToolPolicySet", started, &result);
        result
    }

    async fn put_floor_policy_set(&self, request: Request<pb::PutFloorPolicySetRequest>) -> Result<Response<pb::PutFloorPolicySetResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::PutFloorPolicySetResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let outcome = set_result(svc.put_floor(&actor, &req.source, req.expected_version).await)?;
            Ok(Response::new(pb::PutFloorPolicySetResponse {
                result: Some(match outcome {
                    Ok(v) => pb::put_floor_policy_set_response::Result::Accepted(v),
                    Err(r) => pb::put_floor_policy_set_response::Result::Rejected(r),
                }),
            }))
        }
        .await;
        record_grpc("ToolPolicy", "PutFloorPolicySet", started, &result);
        result
    }

    async fn get_floor_policy_set(&self, request: Request<pb::GetFloorPolicySetRequest>) -> Result<Response<pb::GetFloorPolicySetResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<pb::GetFloorPolicySetResponse>, Status> = async {
            let svc = self.svc()?;
            let actor = actor(&request)?;
            let req = request.into_inner();
            let set = match svc.get_floor(&actor, req.version).await.map_err(convert::status_to_grpc)? {
                Some(v) => version_to_pb(&v).map_err(convert::status_to_grpc)?,
                None => empty_set(ToolPolicySetKey::Floor),
            };
            Ok(Response::new(pb::GetFloorPolicySetResponse { set: Some(set) }))
        }
        .await;
        record_grpc("ToolPolicy", "GetFloorPolicySet", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// All eight rejection reasons reach the wire message with their registered code.
    #[test]
    fn every_rejection_reason_converts_with_its_code() {
        let reasons: Vec<RejectionReason> = RejectionReason::iter().collect();
        assert_eq!(reasons.len(), 8);
        for reason in reasons {
            let wire = rejection_to_pb(&ToolPolicyRejection::new(reason, "m"));
            assert_eq!(wire.reason, reason.code());
            assert_eq!((wire.policy_id.as_str(), wire.line, wire.column, wire.message.as_str()), ("", 0, 0, "m"));
        }
    }

    #[test]
    fn a_kind_or_format_that_is_not_set_is_a_missing_field() {
        assert!(kind_from_wire(pb::ToolKind::Unspecified as i32).is_err());
        assert!(kind_from_wire(99).is_err());
        assert!(format_from_wire(0).is_err());
        assert_eq!(kind_from_wire(pb::ToolKind::Custom as i32).unwrap(), ToolKind::Custom);
    }
}
