// SPDX-License-Identifier: Apache-2.0

//! `AuthzGrpc`: the `AuthorizationService` gRPC server (7 RPCs from SMA-444 Task 19, plus
//! `RetireSystemPolicy` from SMA-501) — a thin
//! adapter over the same `AppState.authorize`/`policies`/`roles` use cases the HTTP
//! `/v1/authz/*` surface (`adapters::http::authz`) drives: parse the wire request -> call the
//! application service with the bearer-resolved actor -> convert the result, no business
//! logic in this layer (mirrors `grpc::tenancy`/`grpc::authn`'s posture).
//!
//! **Actor extraction:** every RPC here is bearer-enforced by `AuthLayer`/`AuthEnforce`
//! (`grpc::authn`) — `AuthorizationService` is NOT on the `:path` exemption list (see
//! `grpc::router`), so `AuthEnforce` always resolves the caller and inserts an
//! [`AuthContext`] into the request's extensions before this service ever runs. Tonic
//! preserves `http::Request` extensions through `Request::from_http_parts` when it decodes
//! the wire message, so [`actor_context`] reading `request.extensions().get::<AuthContext>()`
//! sees exactly what the layer inserted. Its absence would mean the layer didn't run — it
//! shouldn't happen for a non-exempt RPC, but is handled defensively as
//! `convert::missing_auth_context()` rather than a panic.
//!
//! **`IsAuthorized` self/admin exposure rule (spec §9.2):** enforced by
//! [`Authorize::decide_gated`](crate::application::authorize::Authorize::decide_gated) —
//! the SAME function `adapters::http::authz::is_authorized` calls, so the two transports
//! can never diverge (SMA-444 Task 19 brief). See that function's doc for the rule itself.

use std::time::Instant;

use chrono::Utc;
use paigasus_iam_core::authz::model::{ContextValue, PolicyKind};
use paigasus_iam_core::{AccessRequest, Action, Effect, PolicyDocument, RequestContext};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::authorization_service_server::AuthorizationService;
use paigasus_proto::paigasus::iam::v1::{
    DeletePolicyRequest, DeletePolicyResponse, GrantRoleRequest, GrantRoleResponse, IsAuthorizedRequest, IsAuthorizedResponse, ListPoliciesRequest, ListPoliciesResponse, ListRoleGrantsRequest,
    ListRoleGrantsResponse, PutPolicyRequest, PutPolicyResponse, RetireSystemPolicyRequest, RetireSystemPolicyResponse, RevokeRoleRequest, RevokeRoleResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::error::TenancyError;

/// The `AuthorizationService` gRPC server — a thin adapter over the same `AppState` use
/// cases the HTTP surface uses.
pub struct AuthzGrpc {
    state: AppState,
}

impl AuthzGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions (see the
/// module docs). `convert::missing_auth_context()` rather than a panic — this "shouldn't
/// happen" for a non-exempt RPC, but a defensive error beats a 500/panic if it ever does.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

/// Mirrors `adapters::http::authz::parse_prn` (duplicated rather than shared across a
/// transport boundary — a five-line pure parse, same posture as `RoleService`'s duplicated
/// `parse_principal_prn`).
fn parse_prn(raw: &str) -> Result<Prn, TenancyError> {
    Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))
}

/// Mirrors `adapters::http::authz::parse_policy_kind`.
fn parse_policy_kind(raw: &str) -> Result<PolicyKind, TenancyError> {
    match raw {
        "static" => Ok(PolicyKind::Static),
        "template" => Ok(PolicyKind::Template),
        other => Err(TenancyError::PolicyInvalid(format!("unknown policy kind: {other}"))),
    }
}

/// SMA-505: policy/role-grant ADMINISTRATION is gated by `authz.admin_enabled`. `IsAuthorized`
/// is deliberately not — it is the gateway's per-request primitive, and no capability toggle
/// may break it. `UNIMPLEMENTED` is what a client would get from a server that never registered
/// the RPC, so a disabled capability is indistinguishable from a build that never had it.
fn require_authz_admin(state: &AppState) -> Result<(), Status> {
    if state.capabilities.authz_admin {
        Ok(())
    } else {
        Err(convert::capability_disabled("iam.authz.cedar"))
    }
}

#[tonic::async_trait]
impl AuthorizationService for AuthzGrpc {
    /// `IsAuthorized`: see the module docs for the self/admin exposure rule.
    async fn is_authorized(&self, request: Request<IsAuthorizedRequest>) -> Result<Response<IsAuthorizedResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<IsAuthorizedResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();

            let action = Action::parse(&req.action).ok_or_else(|| convert::status_to_grpc(TenancyError::InvalidAction(req.action.clone())))?;
            let principal = parse_prn(&req.principal_prn).map_err(convert::status_to_grpc)?;
            let resource = parse_prn(&req.resource_prn).map_err(convert::status_to_grpc)?;
            let context = RequestContext(req.context.into_iter().map(|(k, v)| (k, ContextValue::Str(v))).collect());
            let access_req = AccessRequest { principal, action, resource, context };

            // Self/admin exposure rule — `Authorize::decide_gated` is the single shared
            // implementation the HTTP surface also calls; see the module docs.
            let decision = self.state.authorize.decide_gated(&actor, &access_req).await.map_err(convert::status_to_grpc)?;

            Ok(Response::new(IsAuthorizedResponse {
                allowed: decision.effect == Effect::Allow,
                determining_policies: decision.determining_policies,
                reason: match decision.effect {
                    Effect::Allow => "allowed".to_string(),
                    Effect::Deny => "denied".to_string(),
                },
            }))
        }
        .await;
        record_grpc("Authorization", "IsAuthorized", started, &result);
        result
    }

    /// `PutPolicy`: upsert. `system` is always `false` for a client-authored document (mirrors
    /// `adapters::http::authz::put_policy`) — the store itself separately rejects mutating an
    /// already-persisted system row.
    async fn put_policy(&self, request: Request<PutPolicyRequest>) -> Result<Response<PutPolicyResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<PutPolicyResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let policy = req.policy.ok_or_else(|| convert::status_to_grpc(TenancyError::PolicyInvalid("policy is required".to_string())))?;
            let kind = parse_policy_kind(&policy.kind).map_err(convert::status_to_grpc)?;
            let now = Utc::now();
            let doc = PolicyDocument {
                policy_id: policy.policy_id,
                kind,
                source: policy.source,
                description: policy.description,
                system: false,
                created_at: now,
                updated_at: now,
            };
            self.state.policies.put(&actor, doc.clone()).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(PutPolicyResponse {
                policy: Some(convert::to_proto_policy(&doc)),
            }))
        }
        .await;
        record_grpc("Authorization", "PutPolicy", started, &result);
        result
    }

    async fn delete_policy(&self, request: Request<DeletePolicyRequest>) -> Result<Response<DeletePolicyResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<DeletePolicyResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            self.state.policies.delete(&actor, &req.policy_id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(DeletePolicyResponse {}))
        }
        .await;
        record_grpc("Authorization", "DeletePolicy", started, &result);
        result
    }

    async fn list_policies(&self, request: Request<ListPoliciesRequest>) -> Result<Response<ListPoliciesResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<ListPoliciesResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let docs = self.state.policies.list(&actor, page.limit, page.offset).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListPoliciesResponse {
                policies: docs.iter().map(convert::to_proto_policy).collect(),
            }))
        }
        .await;
        record_grpc("Authorization", "ListPolicies", started, &result);
        result
    }

    async fn grant_role(&self, request: Request<GrantRoleRequest>) -> Result<Response<GrantRoleResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<GrantRoleResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let grant = self
                .state
                .roles
                .grant(&actor, &req.principal_prn, &req.role_key, &req.scope_prn)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(GrantRoleResponse {
                grant: Some(convert::to_proto_role_grant(&grant)),
            }))
        }
        .await;
        record_grpc("Authorization", "GrantRole", started, &result);
        result
    }

    async fn revoke_role(&self, request: Request<RevokeRoleRequest>) -> Result<Response<RevokeRoleResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<RevokeRoleResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            // `id` is a plain UUID (a role grant's own id), not a PRN — mirrors
            // `TenancyGrpc::detach_membership`'s `InvalidPrn`-as-sentinel posture for a
            // non-PRN-shaped wire id: there is no dedicated error code for "not a uuid".
            let id = Uuid::parse_str(&req.id).map_err(|_| convert::status_to_grpc(TenancyError::InvalidPrn("role grant id must be a uuid".to_string())))?;
            self.state.roles.revoke(&actor, id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(RevokeRoleResponse {}))
        }
        .await;
        record_grpc("Authorization", "RevokeRole", started, &result);
        result
    }

    async fn list_role_grants(&self, request: Request<ListRoleGrantsRequest>) -> Result<Response<ListRoleGrantsResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<ListRoleGrantsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            // `RoleService::list` has no pagination of its own (an M3 simplification —
            // `adapters::http::authz`'s `RoleGrantQuery` doesn't expose `limit`/`offset` at all
            // either); the wire fields exist for proto-shape parity with `ListPoliciesRequest`
            // but aren't enforced here.
            let grants = self.state.roles.list(&actor, &req.principal_prn).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListRoleGrantsResponse {
                grants: grants.iter().map(convert::to_proto_role_grant).collect(),
            }))
        }
        .await;
        record_grpc("Authorization", "ListRoleGrants", started, &result);
        result
    }

    /// `RetireSystemPolicy`: Root-only (enforced inside `SystemRetirementService::retire`), and
    /// additionally gated on `iam.authz.cedar` — mirroring HTTP, where
    /// `system_retirement::router()` is merged only under `caps.authz_admin`.
    ///
    /// **All three outcomes return `OK`**, discriminated by the response `oneof`: the two
    /// refusals are outcomes that are not `Retired`, not server errors (design D3, and the same
    /// argument `http::system_retirement`'s module doc makes). This DIVERGES from HTTP, which
    /// answers both refusals with a 409 carrying a registry error code — the payload fields are
    /// identical, the status is not. A consequence worth knowing: `record_grpc` labels a
    /// refusal `grpc_status="ok"`, so refusals do not feed the gRPC error-rate alert.
    ///
    /// The outcome -> response mapping lives in `convert::to_proto_retire_response`, a free
    /// function over an owned `RetireOutcome`, so every variant stays testable without an
    /// `AppState` — see its doc for the regression that made that necessary.
    async fn retire_system_policy(&self, request: Request<RetireSystemPolicyRequest>) -> Result<Response<RetireSystemPolicyResponse>, Status> {
        require_authz_admin(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<RetireSystemPolicyResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let outcome = self
                .state
                .retirement
                .retire(&actor, &req.policy_id, req.acknowledge_decision_change)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(convert::to_proto_retire_response(outcome)))
        }
        .await;
        record_grpc("Authorization", "RetireSystemPolicy", started, &result);
        result
    }
}
