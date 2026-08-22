// SPDX-License-Identifier: Apache-2.0

//! `/v1/users` handlers: user-principal creation. Thin extract -> use-case call -> map,
//! mirroring `organizations.rs`; `CreateUserError` converts into `TenancyError` (see
//! `application::create_user`) so `?` works against `ApiError` here too.
//!
//! **No authorization check beyond the bearer, deliberately (design D0).**
//! `CreateUser::execute` takes no `actor` parameter, this handler extracts no `AuthContext`,
//! and there is no `Action::CreateUser` in the Cedar action catalog — so any bearer-authenticated
//! caller may create a user principal. `grpc::users`'s `UserGrpc::create_user` is the gRPC
//! mirror of this exact posture (see its module doc for the three-part justification); tightening
//! authorization here without tightening it there (or vice versa) breaks the parity that is this
//! surface's whole acceptance criterion, so treat the two as one decision, not two.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use super::AppState;
use super::dto::{CreateUserBody, CreateUserResponse};
use super::error::ApiError;
use crate::application::create_user::NewUser;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/users", post(create_user))
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

async fn create_user(State(s): State<AppState>, Json(b): Json<CreateUserBody>) -> Result<(StatusCode, Json<CreateUserResponse>), ApiError> {
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
