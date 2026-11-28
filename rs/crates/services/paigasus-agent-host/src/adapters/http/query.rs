// SPDX-License-Identifier: Apache-2.0

//! The house query-string extractor of the agent host. `EnvelopeQuery<T>` answers a refused query
//! string in the error envelope, with the registered reason `invalid-query-parameter`, and not
//! with the plain-text rejection of axum. It is the counterpart of `EnvelopeQuery` in
//! `paigasus-iam`. It holds no code literal: it renders through [`ApiError`].
//!
//! Two failures reach it: a value that does not parse into its type, and a key that the query
//! gives twice. The message is fixed and names no parameter.

use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use super::error::ApiError;

/// Did the caller cause this rejection? Only a 400 is the caller's fault. Anything else goes back
/// to axum unchanged.
fn is_client_error(rejection: &QueryRejection) -> bool {
    rejection.status() == StatusCode::BAD_REQUEST
}

/// `Query<T>` with the error envelope on rejection. A handler must not take a bare
/// `axum::extract::Query` (`repo:http-extractor-envelope`).
#[derive(Debug)]
pub struct EnvelopeQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for EnvelopeQuery<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(EnvelopeQuery(value)),
            Err(rejection) if is_client_error(&rejection) => Err(ApiError::InvalidQueryParameter.into_response()),
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}
