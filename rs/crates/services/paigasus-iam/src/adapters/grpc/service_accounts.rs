// SPDX-License-Identifier: Apache-2.0

//! `ServiceAccountGrpc`: the `ServiceAccountService` gRPC server (7 RPCs, SMA-445 Task 21) — a
//! thin adapter over the same `AppState.service_accounts`/`api_keys` use cases the HTTP
//! `/v1/service-accounts/*` surface (`adapters::http::service_accounts`/`api_keys`) drives:
//! parse the wire PRN(s) -> call the application service with the bearer-resolved actor ->
//! convert the result, no business logic in this layer (mirrors `grpc::tenancy`'s posture).
//!
//! **Actor extraction:** every RPC here is bearer-enforced by `AuthLayer`/`AuthEnforce`
//! (`grpc::authn`) — `ServiceAccountService` is NOT on the `:path` exemption list (see
//! `grpc::router`/`grpc::authn::is_exempt`), so `AuthEnforce` always resolves the caller (OIDC
//! or, since Task 19's credential router, an API key) and inserts an `AuthContext` into the
//! request's extensions before this service ever runs.
//!
//! **Authorization lives in the application layer here** (unlike `TenancyGrpc`'s own
//! `enforce_tenancy`-gated `authorize.check` calls): `ServiceAccountService`/`ApiKeyService`
//! (Tasks 16/17) bake `Authorize::check` into every method themselves, so this adapter never
//! calls `self.state.authorize` directly — it just forwards the bearer-resolved actor and lets
//! the application service's own check (`Forbidden` -> `PermissionDenied`, via
//! `convert::status_to_grpc`) do the work.
//!
//! **`ArchiveServiceAccount`'s response is EMPTY** (`ArchiveServiceAccountResponse {}`, mirroring
//! `DetachMembershipResponse`): archive is a lifecycle op with no meaningful payload —
//! `ServiceAccountService::archive` itself returns `()` (D16: lifecycle status lives on the
//! underlying `Principal`, not on `ServiceAccount`, so archiving never changes any of
//! `ServiceAccount`'s OWN fields anyway). The handler therefore authorizes ONLY
//! `ArchiveServiceAccount` (no `GetServiceAccount` fetch to populate a body), matching the HTTP
//! `DELETE`'s 204 semantics exactly — an RPC named after one action requires exactly that one
//! action's authority.

use std::time::Instant;

use paigasus_iam_core::{Action, ApiKeyId, PrincipalId, TenancyNodeRef};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::service_account_service_server::ServiceAccountService;
use paigasus_proto::paigasus::iam::v1::{
    ArchiveServiceAccountRequest, ArchiveServiceAccountResponse, CreateServiceAccountRequest, CreateServiceAccountResponse, GetServiceAccountRequest, GetServiceAccountResponse, IssueApiKeyRequest,
    IssueApiKeyResponse, ListApiKeysRequest, ListApiKeysResponse, ListServiceAccountsRequest, ListServiceAccountsResponse, RevokeApiKeyRequest, RevokeApiKeyResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use super::convert::require_present;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::error::TenancyError;

/// The `ServiceAccountService` gRPC server — a thin adapter over the same `AppState` services
/// the HTTP surface uses (module docs).
pub struct ServiceAccountGrpc {
    state: AppState,
}

impl ServiceAccountGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// `grpc::tenancy::actor_context`/`grpc::authz::actor_context` exactly (duplicated rather than
/// shared across a transport-internal module boundary, the same posture as those).
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

/// Parses a caller-supplied PRN string into a `TenancyNodeRef` — mirrors
/// `adapters::http::service_accounts::parse_node_prn`/`grpc::tenancy::parse_node_prn`.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Parses a wire `principal` PRN into the [`PrincipalId`] a service account's own identity
/// uses — the gRPC analog of `adapters::http::service_accounts::service_account_id`, but from
/// a wire PRN string (the `prn`/`service_account_prn` request fields) rather than a path uuid.
/// Unlike `convert::node_uuid`'s tenancy-node parsing, a service account has no "stored
/// canonical" recheck of its own (its PRN IS its whole identity, not a parent-embedding one,
/// so there is no forged-org-slot analog to defend against here) — this stays a plain parse.
fn service_account_id(raw: &str) -> Result<PrincipalId, TenancyError> {
    let parsed = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if parsed.service() != "iam" || parsed.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(parsed.canonical()));
    }
    Ok(PrincipalId::from_prn(parsed))
}

/// SMA-505: API-KEY management (`IssueApiKey`/`RevokeApiKey`/`ListApiKeys`) is gated by
/// `api_keys.management_enabled`. The four service-account lifecycle RPCs
/// (`CreateServiceAccount`/`GetServiceAccount`/`ListServiceAccounts`/`ArchiveServiceAccount`)
/// are deliberately not — they are tenancy management, not an API-key concern, and no capability
/// toggle here may touch them. `UNIMPLEMENTED` is what a client would get from a server that
/// never registered the RPC, so a disabled capability is indistinguishable from a build that
/// never had it.
fn require_apikey_management(state: &AppState) -> Result<(), Status> {
    if state.capabilities.apikeys_management {
        Ok(())
    } else {
        Err(convert::capability_disabled("iam.apikeys"))
    }
}

#[tonic::async_trait]
impl ServiceAccountService for ServiceAccountGrpc {
    async fn create_service_account(&self, request: Request<CreateServiceAccountRequest>) -> Result<Response<CreateServiceAccountResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateServiceAccountResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let owner = parse_node_prn(&req.owner_prn).map_err(convert::status_to_grpc)?;
            let sa = self.state.service_accounts.create(&actor, owner, &req.name).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(CreateServiceAccountResponse {
                service_account: Some(convert::to_proto_service_account(&sa)),
            }))
        }
        .await;
        record_grpc("ServiceAccount", "CreateServiceAccount", started, &result);
        result
    }

    async fn get_service_account(&self, request: Request<GetServiceAccountRequest>) -> Result<Response<GetServiceAccountResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<GetServiceAccountResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let id = service_account_id(&request.get_ref().prn).map_err(convert::status_to_grpc)?;
            let sa = self.state.service_accounts.get(&actor, &id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(GetServiceAccountResponse {
                service_account: Some(convert::to_proto_service_account(&sa)),
            }))
        }
        .await;
        record_grpc("ServiceAccount", "GetServiceAccount", started, &result);
        result
    }

    async fn list_service_accounts(&self, request: Request<ListServiceAccountsRequest>) -> Result<Response<ListServiceAccountsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListServiceAccountsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let owner_prn = require_present(&req.owner_prn, "owner_prn").map_err(convert::status_to_grpc)?;
            let owner = parse_node_prn(owner_prn).map_err(convert::status_to_grpc)?;
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let accounts = self.state.service_accounts.list(&actor, &owner, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListServiceAccountsResponse {
                service_accounts: accounts.iter().map(convert::to_proto_service_account).collect(),
            }))
        }
        .await;
        record_grpc("ServiceAccount", "ListServiceAccounts", started, &result);
        result
    }

    async fn archive_service_account(&self, request: Request<ArchiveServiceAccountRequest>) -> Result<Response<ArchiveServiceAccountResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ArchiveServiceAccountResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let id = service_account_id(&request.get_ref().prn).map_err(convert::status_to_grpc)?;
            // Archive is a lifecycle op with an EMPTY response (module docs) — `ServiceAccountService
            // ::archive` authorizes ONLY `ArchiveServiceAccount`, matching the HTTP `DELETE`'s 204
            // semantics exactly (no `GetServiceAccount` double-authz just to populate a body).
            self.state.service_accounts.archive(&actor, &id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ArchiveServiceAccountResponse {}))
        }
        .await;
        record_grpc("ServiceAccount", "ArchiveServiceAccount", started, &result);
        result
    }

    async fn issue_api_key(&self, request: Request<IssueApiKeyRequest>) -> Result<Response<IssueApiKeyResponse>, Status> {
        require_apikey_management(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<IssueApiKeyResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let sa_id = service_account_id(&req.service_account_prn).map_err(convert::status_to_grpc)?;
            let scope_prn = require_present(&req.scope_prn, "scope_prn").map_err(convert::status_to_grpc)?;
            let scope = parse_node_prn(scope_prn).map_err(convert::status_to_grpc)?;
            // `expires_at` unset means non-expiring (or the configured `default_expiry_days`
            // fallback, `ApiKeyService::issue`) — mirrors `IssueApiKeyBody::expires_at`'s HTTP
            // counterpart. A present-but-out-of-range timestamp is `InvalidTimestamp`
            // (SMA-586). `parse_opt_ts` is that exact absent/valid/unrepresentable split,
            // shared with the filter call sites (SMA-583). NOTE the HTTP twin diverges here
            // and that is deliberate: its `expires_at` is a typed `DateTime<Utc>` in the body,
            // so a malformed value fails inside serde and yields `invalid-request-body`, which
            // is the correct reason for a body that would not deserialize.
            let expires_at = convert::parse_opt_ts(req.expires_at, "expires_at").map_err(convert::status_to_grpc)?;
            let scope_actions = req
                .scope_actions
                .iter()
                .map(|a| Action::parse(a).ok_or_else(|| TenancyError::InvalidAction(a.clone())))
                .collect::<Result<Vec<_>, _>>()
                .map_err(convert::status_to_grpc)?;
            let new_key = self
                .state
                .api_keys
                .issue(&actor, &sa_id, scope, expires_at, scope_actions, req.scope_roles)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(convert::to_proto_issue_api_key_response(&new_key)))
        }
        .await;
        record_grpc("ServiceAccount", "IssueApiKey", started, &result);
        result
    }

    async fn revoke_api_key(&self, request: Request<RevokeApiKeyRequest>) -> Result<Response<RevokeApiKeyResponse>, Status> {
        require_apikey_management(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<RevokeApiKeyResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            // `RevokeApiKeyRequest.id` is a bare uuid, not a PRN, so a malformed value is
            // `InvalidUuid` naming the segment (SMA-586). The field name reaches the client in
            // both the message and `ErrorInfo.metadata["field"]`.
            let id = Uuid::parse_str(&req.id).map_err(|_| convert::status_to_grpc(TenancyError::InvalidUuid("api_key_id")))?;
            self.state.api_keys.revoke(&actor, ApiKeyId::from_uuid(id)).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(RevokeApiKeyResponse {}))
        }
        .await;
        record_grpc("ServiceAccount", "RevokeApiKey", started, &result);
        result
    }

    async fn list_api_keys(&self, request: Request<ListApiKeysRequest>) -> Result<Response<ListApiKeysResponse>, Status> {
        require_apikey_management(&self.state)?;
        let started = Instant::now();
        let result: Result<Response<ListApiKeysResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let sa_id = service_account_id(&req.service_account_prn).map_err(convert::status_to_grpc)?;
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let keys = self.state.api_keys.list(&actor, &sa_id, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListApiKeysResponse {
                api_keys: keys.iter().map(convert::to_proto_api_key).collect(),
            }))
        }
        .await;
        record_grpc("ServiceAccount", "ListApiKeys", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-586 D5.2: an empty required PRN field is `missing-required-field`, not a PRN parse
    /// failure. Before this, an empty `owner_prn` fell through to `parse_node_prn` and answered
    /// `invalid-prn` — while the HTTP twin answered `missing-required-field`, so the two
    /// transports disagreed on the same logical failure.
    #[test]
    fn an_empty_owner_prn_is_a_missing_required_field() {
        assert_eq!(require_present("", "owner_prn").unwrap_err(), TenancyError::MissingRequiredField("owner_prn"));
        assert_eq!(require_present("   ", "owner_prn").unwrap_err(), TenancyError::MissingRequiredField("owner_prn"));
        assert_eq!(require_present("iam::org/x", "owner_prn").unwrap(), "iam::org/x");
    }
}
