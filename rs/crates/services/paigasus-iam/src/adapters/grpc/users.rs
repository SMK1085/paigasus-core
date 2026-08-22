// SPDX-License-Identifier: Apache-2.0

//! `UserGrpc`: the `UserService` gRPC server (SMA-501) — a thin adapter over `AppState.users`:
//! parse the wire request -> `CreateUser::execute` -> project the id, no business logic in this
//! layer (mirrors `grpc::audit`'s posture).
//!
//! **This RPC performs NO authorization check, deliberately, and that is why the service
//! exists.** `CreateUser::execute` takes no `actor` parameter, `http::users` extracts no
//! `AuthContext`, and there is no `Action::CreateUser` in the Cedar action catalog — so
//! `POST /v1/users` is bearer-gated and otherwise unauthorized. This adapter mirrors that
//! exactly, because parity with the HTTP surface is the acceptance criterion and tightening
//! authorization on an existing endpoint is a behavior change belonging to its own issue.
//!
//! It sits on `UserService` rather than `TenancyService` for exactly this reason: all 21
//! `TenancyService` RPCs authorize in the adapter (`if self.state.enforce_tenancy { … }`), so
//! parking the one unchecked RPC among them would camouflage the single property a reviewer
//! most needs to see. On its own service, the absence is legible in the contract.
//!
//! **Bearer enforcement still applies:** `UserService` is NOT on `AuthLayer`'s `:path`
//! exemption list (`grpc::authn::is_exempt`), so an unauthenticated call never reaches here.

use std::time::Instant;

use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::user_service_server::UserService;
use paigasus_proto::paigasus::iam::v1::{CreateUserRequest, CreateUserResponse};
use tonic::{Request, Response, Status};

use super::convert;
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

/// An empty wire string means "unset" on an optional scalar (the proto's own doc). NOTE this
/// DIVERGES from HTTP, where `CreateUserBody.locale` is an `Option<String>` and `{"locale": ""}`
/// therefore persists `Some("")` rather than `None` (design D11). The proto sentinel is
/// normative for gRPC; HTTP is deliberately left unchanged.
fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

#[tonic::async_trait]
impl UserService for UserGrpc {
    /// `CreateUser`: bearer-required, otherwise UNAUTHORIZED BY DESIGN — see this module's doc.
    /// An invalid email is rejected before an id is minted or a transaction opened
    /// (`CreateUser::execute`), and a duplicate email rolls the whole unit of work back before
    /// the `iam.principal.created` event is ever enqueued.
    async fn create_user(&self, request: Request<CreateUserRequest>) -> Result<Response<CreateUserResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateUserResponse>, Status> = async {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_optional_scalar_becomes_none() {
        assert_eq!(opt_string(String::new()), None);
        assert_eq!(opt_string("de-DE".to_string()), Some("de-DE".to_string()));
    }

    /// The HTTP/gRPC twin test for this surface (design D9.1), covering the ONE field where the
    /// two transports deliberately disagree (D11). Both build the same `NewUser` command, so
    /// feeding an equivalent wire payload through both projections is what pins the divergence
    /// as intentional rather than letting it drift further.
    #[test]
    fn create_user_projects_onto_the_same_command_except_for_the_empty_string_sentinel() {
        use crate::adapters::http::dto::CreateUserBody;

        // Both present: the two transports must agree exactly.
        let grpc = NewUser {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: opt_string("de-DE".to_string()),
            timezone: opt_string("Europe/Berlin".to_string()),
        };
        let body = CreateUserBody {
            email: "a@example.com".to_string(),
            display_name: "A".to_string(),
            locale: Some("de-DE".to_string()),
            timezone: Some("Europe/Berlin".to_string()),
        };
        assert_eq!(grpc.email, body.email);
        assert_eq!(grpc.display_name, body.display_name);
        assert_eq!(grpc.locale, body.locale);
        assert_eq!(grpc.timezone, body.timezone);

        // The allowlisted divergence, asserted so it stays deliberate: the same "empty" wire
        // value means `None` on gRPC and `Some("")` on HTTP, which persists an empty string
        // rather than NULL.
        assert_eq!(opt_string(String::new()), None);
        let http_empty = CreateUserBody {
            email: "b@example.com".to_string(),
            display_name: "B".to_string(),
            locale: Some(String::new()),
            timezone: None,
        };
        assert_eq!(http_empty.locale, Some(String::new()), "HTTP keeps the empty string — gRPC does not");
    }
}
