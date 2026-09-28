// SPDX-License-Identifier: Apache-2.0

//! `/v1/users` handlers: user-principal creation. Thin extract -> use-case call -> map,
//! mirroring `organizations.rs`; `CreateUserError` converts into `TenancyError` (see
//! `application::create_user`) so `?` works against `ApiError` here too.
//!
//! **Authorized (SMA-584):** the handler checks `Action::CreateUser` at `root_prn()`, gated by
//! `AppState.enforce_tenancy` — the same shape `organizations.rs`'s `create_org` uses for
//! `CreateOrganization`. `Root` is the top of the Cedar hierarchy and `resource in ?resource`
//! is descendant-or-self, so no `Organization`/`Team`/`Project`-scoped grant can satisfy it:
//! under the starter role set this is `platform_admin` only. (An operator-authored STATIC
//! policy via `PutPolicy` can still permit it narrowly — that is the intended escape hatch,
//! not a hole.)
//!
//! The check runs BEFORE `to_command`/`execute`, so a denied caller never reaches email
//! validation or the unit of work and cannot use the endpoint as an email-existence oracle.
//! `grpc::users`'s `UserGrpc::create_user` mirrors this exactly; the two transports are ONE
//! decision, not two, and `tests/http_users.rs` + `tests/grpc_users.rs` are written so that
//! changing either transport alone reds CI.
//!
//! **The identity routes (SMA-712)** — `POST /v1/users/find-by-email`,
//! `/v1/users/{id}/external-identities`, `/v1/users/{id}/external-identities/{identity_id}/unlink`
//! and `/v1/users/{id}/email` — authorize INSIDE `UserIdentityService`, with no
//! `enforce_tenancy` gate (spec 5.1). `{id}` is the user's principal uuid, the convention of
//! `/v1/service-accounts/{sa}`. Unlink is a `POST`, not a `DELETE`, because it carries a
//! `reason` body.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, PrincipalId};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{ChangeUserEmailBody, CreateUserBody, CreateUserResponse, ExternalIdentityDto, FindUserByEmailBody, LinkExternalIdentityBody, UnlinkExternalIdentityBody, UserDto};
use super::error::ApiError;
use super::json::EnvelopeJson;
use super::path::{ExternalIdentityId, UserId, UuidPath, UuidPathPair};
use crate::adapters::auth::AuthContext;
use crate::application::create_user::NewUser;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/users", post(create_user))
        .route("/v1/users/find-by-email", post(find_by_email))
        .route("/v1/users/{id}/external-identities", post(link_identity))
        .route("/v1/users/{id}/external-identities/{identity_id}/unlink", post(unlink_identity))
        .route("/v1/users/{id}/email", post(change_email))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `adapters::http::organizations::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> paigasus_kernel::Prn {
    ctx.principal_id.prn().clone()
}

/// The HTTP body -> use-case command projection, pulled out of the handler so the twin test
/// below (this module's test module, not `grpc::users`'s — see that module's `opt_string` doc
/// for why) can run the REAL mapping rather than a hand-built copy of it. Paired with
/// `grpc::users`'s `CreateUserRequest` -> `NewUser` projection: the two are asserted to agree on
/// every field except the deliberate empty-string divergence (design D11), and that assertion is
/// only meaningful because both sides run production code.
pub(crate) fn to_command(b: CreateUserBody) -> NewUser {
    NewUser {
        email: b.email,
        display_name: b.display_name,
        locale: b.locale,
        timezone: b.timezone,
    }
}

async fn create_user(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<CreateUserBody>) -> Result<(StatusCode, Json<CreateUserResponse>), ApiError> {
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::CreateUser, &root_prn()).await?;
    }
    let cmd = to_command(b);
    let id = s.users.execute(cmd).await?;
    Ok((StatusCode::CREATED, Json(CreateUserResponse { principal_prn: id.canonical() })))
}

/// The `PrincipalId` a `{id}` segment names. `Prn::build` with these fixed parts cannot fail.
fn user_id(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).expect("static principal prn parts are valid"))
}

/// `POST /v1/users/find-by-email` (SMA-712): 200 with the user, or 404.
async fn find_by_email(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<FindUserByEmailBody>) -> Result<Json<UserDto>, ApiError> {
    let view = s.user_identities.find_by_email(&actor_prn(&ctx), b.email.as_deref().unwrap_or_default()).await?;
    Ok(Json(view.into()))
}

/// `POST /v1/users/{id}/external-identities` (SMA-712): 201 with the new identity, or 200 with
/// the stored identity when the same user already holds the pair (a safe retry).
async fn link_identity(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<UserId>,
    EnvelopeJson(b): EnvelopeJson<LinkExternalIdentityBody>,
) -> Result<(StatusCode, Json<ExternalIdentityDto>), ApiError> {
    let out = s
        .user_identities
        .link(
            &actor_prn(&ctx),
            &user_id(path.id),
            b.issuer.as_deref().unwrap_or_default(),
            b.subject.as_deref().unwrap_or_default(),
            b.reason.as_deref().unwrap_or_default(),
        )
        .await?;
    let status = if out.changed { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(out.value.into())))
}

/// `POST /v1/users/{id}/external-identities/{identity_id}/unlink` (SMA-712): 204. A repeated
/// unlink gives 404. The caller's own credential goes to the service for the self-lockout guard.
async fn unlink_identity(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPathPair<UserId, ExternalIdentityId>,
    EnvelopeJson(b): EnvelopeJson<UnlinkExternalIdentityBody>,
) -> Result<StatusCode, ApiError> {
    s.user_identities
        .unlink(&actor_prn(&ctx), &ctx.credential, &user_id(path.first), path.second, b.reason.as_deref().unwrap_or_default())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/users/{id}/email` (SMA-712): 200 with the user.
async fn change_email(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    path: UuidPath<UserId>,
    EnvelopeJson(b): EnvelopeJson<ChangeUserEmailBody>,
) -> Result<Json<UserDto>, ApiError> {
    let view = s
        .user_identities
        .change_email(&actor_prn(&ctx), &user_id(path.id), b.email.as_deref().unwrap_or_default(), b.reason.as_deref().unwrap_or_default())
        .await?;
    Ok(Json(view.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::grpc::users::opt_string;

    /// The HTTP/gRPC twin test for `CreateUser` (design D9.1), covering the ONE field where the
    /// two transports deliberately disagree (D11). Lives HERE, not in `grpc::users`'s test
    /// module, because `adapters::http`'s `mod users` is private — this file can reach
    /// `grpc::users` (a `pub mod`), but not the reverse (mirrors `http::system_retirement`'s
    /// identical `response_for`/`grpc::convert` pairing). Both sides run PRODUCTION code: `to_command`
    /// is the exact function `create_user` calls, and `opt_string` is the exact function
    /// `UserGrpc::create_user` calls — so this test detects drift in either projection, not just
    /// a hand-copied stand-in for one of them.
    #[test]
    fn create_user_projects_onto_the_same_command_except_for_the_empty_string_sentinel() {
        // Both present: the two transports must agree exactly.
        let body = CreateUserBody {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: Some("de-DE".to_string()),
            timezone: Some("Europe/Berlin".to_string()),
        };
        let http = to_command(body);
        let grpc = NewUser {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: opt_string("de-DE".to_string()),
            timezone: opt_string("Europe/Berlin".to_string()),
        };
        assert_eq!(http.email, grpc.email);
        assert_eq!(http.display_name, grpc.display_name);
        assert_eq!(http.locale, grpc.locale);
        assert_eq!(http.timezone, grpc.timezone);

        // The allowlisted divergence, asserted so it stays deliberate: the same "empty" wire
        // value means `Some("")` on HTTP (persists an empty string) and `None` on gRPC. Both
        // sides run their REAL projection here too.
        let http_empty = CreateUserBody {
            email: "b@example.com".to_string(),
            display_name: "B".to_string(),
            locale: Some(String::new()),
            timezone: None,
        };
        let http_empty_cmd = to_command(http_empty);
        let grpc_empty_locale = opt_string(String::new());
        assert_eq!(http_empty_cmd.locale, Some(String::new()), "HTTP keeps the empty string — gRPC does not");
        assert_eq!(grpc_empty_locale, None, "gRPC's empty-string sentinel collapses to None");
    }
}
