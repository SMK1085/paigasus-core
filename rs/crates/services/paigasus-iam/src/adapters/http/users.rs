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

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;
use paigasus_iam_core::authz::model::root_prn;

use super::AppState;
use super::dto::{CreateUserBody, CreateUserResponse};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::create_user::NewUser;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/users", post(create_user))
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

async fn create_user(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(b): Json<CreateUserBody>) -> Result<(StatusCode, Json<CreateUserResponse>), ApiError> {
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::CreateUser, &root_prn()).await?;
    }
    let cmd = to_command(b);
    let id = s.users.execute(cmd).await?;
    Ok((StatusCode::CREATED, Json(CreateUserResponse { principal_prn: id.canonical() })))
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
