// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/service-info` (spec §5.3). The caller is authenticated and not authorised. A caller
//! that IAM answers with `identity-not-provisioned` is accepted: the console calls this route first.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use paigasus_service_info::{ServiceInfoDto, descriptor};

use super::Ingress;
use super::auth::{authenticate_discovery, bearer_token};

/// The service slug of the agent host.
pub const SERVICE: &str = "agents";

/// The authentication of the discovery route. It has no `Caller` and no authorisation.
pub async fn authenticate_middleware(State(ingress): State<Arc<Ingress>>, req: Request, next: Next) -> Response {
    let token = match bearer_token(req.headers()) {
        Ok(token) => token,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = authenticate_discovery(&*ingress.iam, &token).await {
        return e.into_response();
    }
    next.run(req).await
}

/// The descriptor of this service. The service has no capability yet.
pub async fn service_info() -> Json<ServiceInfoDto> {
    Json(ServiceInfoDto::from(&descriptor(SERVICE, env!("CARGO_PKG_VERSION"), &[])))
}
