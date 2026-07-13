// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness, no dependency checks) and `/readyz` (a
//! placeholder here — G8 makes it check IAM + upstream reachability); plus the auth middleware
//! ([`auth`]) that fronts every protected route, the OpenAI-compatible error envelope
//! ([`error`]) it renders failures through, and the inbound chat-completion request DTO
//! ([`dto`]) G7 parses to read `model`/`stream`.

pub mod auth;
pub mod dto;
pub mod error;

pub use auth::require_iam_auth;
pub use dto::ChatCompletionRequest;
pub use error::GatewayError;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use serde_json::json;

/// The gateway's HTTP surface. G3 has no `AppState` yet (no IAM/OpenAI clients wired), so
/// this takes no arguments; G7 introduces the real `AppState` once the chat handler needs
/// one, at which point this becomes `router(state: AppState) -> Router` (mirrors
/// `paigasus-iam::adapters::http::router`'s shape) and `main.rs`'s call site grows one
/// argument.
pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz))
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// Placeholder readiness check — always reports ready. TODO(G8): check IAM gRPC
/// reachability and OpenAI upstream reachability before reporting `200`.
async fn readyz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn healthz_returns_200_with_status_ok_body() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn readyz_returns_200_placeholder() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
