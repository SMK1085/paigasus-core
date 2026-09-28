// SPDX-License-Identifier: Apache-2.0

//! `UserGrpc`: the `UserService` gRPC server (SMA-501) — a thin adapter over `AppState.users`:
//! parse the wire request -> `CreateUser::execute` -> project the id, no business logic in this
//! layer (mirrors `grpc::audit`'s posture).
//!
//! **This RPC IS authorized (SMA-584):** it checks `Action::CreateUser` at `root_prn()`, gated
//! by `enforce_tenancy`, exactly as `POST /v1/users` does. `Root` is the top of the Cedar
//! hierarchy and `resource in ?resource` is descendant-or-self, so no `Organization`/`Team`/
//! `Project`-scoped grant can satisfy it: under the starter role set this is `platform_admin`
//! only. The check runs BEFORE `CreateUser::execute`, so a denied caller never reaches email
//! validation or the unit of work.
//!
//! It sits on `UserService` rather than `TenancyService` because a **user is a principal, not
//! a tenancy node** — a different aggregate from `TenancyService`'s org/team/project/membership
//! surface, exactly as `ServiceAccountService` is. `UserService` is the intended home for
//! future user-principal operations (`GetUser`, `ListUsers`, `ArchiveUser`). (Until SMA-584 the
//! stated reason was that this was the one *unauthorized* RPC and parking it among 21
//! authorized ones would camouflage it; that reason is now obsolete.)
//!
//! **Bearer enforcement still applies:** `UserService` is NOT on `AuthLayer`'s `:path`
//! exemption list (`grpc::authn::is_exempt`), so an unauthenticated call never reaches here.
//!
//! **The four identity RPCs (SMA-712)** — `FindUserByEmail`, `LinkExternalIdentity`,
//! `UnlinkExternalIdentity`, `ChangeUserEmail` — authorize INSIDE `UserIdentityService`, with no
//! `enforce_tenancy` gate (spec 5.1). This adapter only parses the wire values and forwards the
//! bearer-resolved actor and credential. A malformed `user_prn` or `external_identity_id` is
//! refused here, before the service runs, the same way the HTTP path extractor refuses a
//! malformed uuid before its handler runs.

use std::time::Instant;

use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::user_service_server::UserService;
use paigasus_proto::paigasus::iam::v1::{
    ChangeUserEmailRequest, ChangeUserEmailResponse, CreateUserRequest, CreateUserResponse, FindUserByEmailRequest, FindUserByEmailResponse, LinkExternalIdentityRequest, LinkExternalIdentityResponse,
    UnlinkExternalIdentityRequest, UnlinkExternalIdentityResponse,
};
use tonic::{Request, Response, Status};

use paigasus_iam_core::Action;
use paigasus_iam_core::PrincipalId;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::create_user::NewUser;
use crate::application::error::TenancyError;

/// The `UserService` gRPC server — a thin adapter over `AppState.users` (module docs).
pub struct UserGrpc {
    state: AppState,
}

impl UserGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// the identical private helper in `grpc::tenancy`/`grpc::authz`/`grpc::audit`/
/// `grpc::service_accounts`/`grpc::dead_letters`.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

/// An empty wire string means "unset" on an optional scalar (the proto's own doc). NOTE this
/// DIVERGES from HTTP, where `CreateUserBody.locale` is an `Option<String>` and `{"locale": ""}`
/// therefore persists `Some("")` rather than `None` (design D11). The proto sentinel is
/// normative for gRPC; HTTP is deliberately left unchanged.
///
/// `pub(crate)`, not private: the HTTP/gRPC twin test for `CreateUser` (design D9.1) lives in
/// `http::users`, not here, because `adapters::http`'s `mod users` is private and this module
/// (`adapters::grpc::users`, `pub mod` in `grpc::mod`) is the side that's reachable from there —
/// mirrors `http::system_retirement`'s identical note about reaching `grpc::convert` rather than
/// the other way around.
pub(crate) fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

/// Parses a wire `user_prn` into the [`PrincipalId`] it names (SMA-712). It must be an `iam`
/// `principal` PRN. The service then answers 404 when that principal is not a user.
fn user_id(raw: &str) -> Result<PrincipalId, TenancyError> {
    let parsed = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if parsed.service() != "iam" || parsed.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(parsed.canonical()));
    }
    Ok(PrincipalId::from_prn(parsed))
}

/// Parses a wire `external_identity_id` (SMA-712).
fn external_identity_id(raw: &str) -> Result<Uuid, TenancyError> {
    Uuid::parse_str(raw).map_err(|_| TenancyError::InvalidUuid("external_identity_id"))
}

#[tonic::async_trait]
impl UserService for UserGrpc {
    /// `CreateUser`: bearer-required AND authorized (`Action::CreateUser`@`Root`) — see this
    /// module's doc.
    /// An invalid email is rejected before an id is minted or a transaction opened
    /// (`CreateUser::execute`), and a duplicate email rolls the whole unit of work back before
    /// the `iam.principal.created` event is ever enqueued.
    async fn create_user(&self, request: Request<CreateUserRequest>) -> Result<Response<CreateUserResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateUserResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::CreateUser, &root_prn()).await.map_err(convert::status_to_grpc)?;
            }
            let req = request.into_inner();
            let cmd = NewUser {
                email: req.email,
                display_name: req.display_name,
                locale: opt_string(req.locale),
                timezone: opt_string(req.timezone),
            };
            // `TenancyError::from` spelled out rather than a bare `.into()`: `status_to_grpc`
            // takes a `TenancyError`, and inference through two conversions is fragile here.
            let id = self.state.users.execute(cmd).await.map_err(|e| convert::status_to_grpc(TenancyError::from(e)))?;
            Ok(Response::new(CreateUserResponse { principal_prn: id.canonical() }))
        }
        .await;
        record_grpc("User", "CreateUser", started, &result);
        result
    }

    /// `FindUserByEmail` (SMA-712): `GetUser` at Root, checked in `UserIdentityService`.
    async fn find_user_by_email(&self, request: Request<FindUserByEmailRequest>) -> Result<Response<FindUserByEmailResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<FindUserByEmailResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let view = self.state.user_identities.find_by_email(&actor, &req.email).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(FindUserByEmailResponse {
                user: Some(convert::to_proto_user(&view)),
            }))
        }
        .await;
        record_grpc("User", "FindUserByEmail", started, &result);
        result
    }

    /// `LinkExternalIdentity` (SMA-712). A same-user retry returns the stored identity.
    async fn link_external_identity(&self, request: Request<LinkExternalIdentityRequest>) -> Result<Response<LinkExternalIdentityResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<LinkExternalIdentityResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let out = self
                .state
                .user_identities
                .link(&actor, &user, &req.issuer, &req.subject, &req.reason)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(LinkExternalIdentityResponse {
                external_identity: Some(convert::to_proto_external_identity(&out.value)),
            }))
        }
        .await;
        record_grpc("User", "LinkExternalIdentity", started, &result);
        result
    }

    /// `UnlinkExternalIdentity` (SMA-712). The caller's own credential goes to the service, which
    /// refuses to unlink the identity that authenticated this request.
    async fn unlink_external_identity(&self, request: Request<UnlinkExternalIdentityRequest>) -> Result<Response<UnlinkExternalIdentityResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<UnlinkExternalIdentityResponse>, Status> = async {
            let ctx = actor_context(&request)?;
            let actor = ctx.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let identity = external_identity_id(&req.external_identity_id).map_err(convert::status_to_grpc)?;
            self.state
                .user_identities
                .unlink(&actor, &ctx.credential, &user, identity, &req.reason)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(UnlinkExternalIdentityResponse {}))
        }
        .await;
        record_grpc("User", "UnlinkExternalIdentity", started, &result);
        result
    }

    /// `ChangeUserEmail` (SMA-712).
    async fn change_user_email(&self, request: Request<ChangeUserEmailRequest>) -> Result<Response<ChangeUserEmailResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ChangeUserEmailResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let user = user_id(&req.user_prn).map_err(convert::status_to_grpc)?;
            let view = self.state.user_identities.change_email(&actor, &user, &req.email, &req.reason).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ChangeUserEmailResponse {
                user: Some(convert::to_proto_user(&view)),
            }))
        }
        .await;
        record_grpc("User", "ChangeUserEmail", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_optional_scalar_becomes_none() {
        assert_eq!(opt_string(String::new()), None);
        assert_eq!(opt_string("de-DE".to_string()), Some("de-DE".to_string()));
    }

    // The HTTP/gRPC twin test for `CreateUser` (design D9.1) lives in `http::users`'s test
    // module, not here: `adapters::http`'s `mod users` is private, so a test living in THIS
    // module cannot name `http::users::to_command` to run the real HTTP-side projection. This
    // module is `pub`, so the twin test reaches `opt_string` (above) from the other side instead
    // — see `http::users` for the assertion.

    /// SMA-712 spec 4.1: gRPC names a user by its full principal PRN. Anything else is
    /// `invalid-prn` (the recorded transport divergence: HTTP answers `invalid-uuid`).
    #[test]
    fn a_user_prn_must_name_an_iam_principal() {
        let uuid = uuid::Uuid::from_u128(7);
        assert_eq!(user_id(&format!("prn:pgs:iam:::principal/{uuid}")).unwrap().uuid(), uuid);
        assert!(matches!(user_id(""), Err(TenancyError::InvalidPrn(_))));
        assert!(matches!(user_id("not a prn"), Err(TenancyError::InvalidPrn(_))));
        let org = paigasus_iam_core::OrganizationId::from_uuid(uuid);
        assert!(matches!(user_id(&org.canonical()), Err(TenancyError::InvalidPrn(_))));
    }

    #[test]
    fn an_external_identity_id_must_be_a_uuid() {
        let id = uuid::Uuid::from_u128(3);
        assert_eq!(external_identity_id(&id.to_string()).unwrap(), id);
        assert_eq!(external_identity_id("x").unwrap_err(), TenancyError::InvalidUuid("external_identity_id"));
    }
}
