// SPDX-License-Identifier: Apache-2.0

//! `/v1/users` handlers: user-principal creation. Thin extract -> use-case call -> map,
//! mirroring `organizations.rs`; `CreateUserError` converts into `TenancyError` (see
//! `application::create_user`) so `?` works against `ApiError` here too.

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

async fn create_user(State(s): State<AppState>, Json(b): Json<CreateUserBody>) -> Result<(StatusCode, Json<CreateUserResponse>), ApiError> {
    let cmd = NewUser {
        email: b.email,
        display_name: b.display_name,
        locale: b.locale,
        timezone: b.timezone,
    };
    let id = s.users.execute(cmd).await?;
    Ok((StatusCode::CREATED, Json(CreateUserResponse { principal_prn: id.canonical() })))
}
