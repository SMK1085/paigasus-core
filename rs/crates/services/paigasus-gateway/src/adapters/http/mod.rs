// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness, no dependency checks) and `/readyz` (a
//! placeholder here — G8 makes it check IAM + upstream reachability) stay public; the protected
//! `/v1/chat/completions` proxy ([`chat`]) is fronted by the auth middleware ([`auth`]) plus a
//! request-body size limit and renders failures through the OpenAI-compatible error envelope
//! ([`error`]). The inbound chat-completion request DTO ([`dto`]) is parsed only to read
//! `model`/`stream`.

pub mod auth;
pub mod chat;
pub mod dto;
pub mod error;

pub use auth::require_iam_auth;
pub use dto::ChatCompletionRequest;
pub use error::GatewayError;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get, routing::post};
use serde_json::json;

use crate::adapters::iam::Iam;
use crate::adapters::openai::OpenAiClient;

/// Shared handler state: the IAM port (an `Arc<dyn Iam>` so tests inject a fake and the binary
/// injects the real `IamClient`), the OpenAI egress client, and the inbound body-size cap. `Clone`
/// is cheap (all fields are `Arc`/`Copy`), as axum requires for `State`.
#[derive(Clone)]
pub struct AppState {
    /// The IAM port the auth middleware queries (introspect + self-query authorize).
    pub iam: Arc<dyn Iam>,
    /// The outbound OpenAI egress client (holds the real key; forwards raw request bytes).
    pub openai: Arc<OpenAiClient>,
    /// Max inbound request-body size in bytes; an over-limit body is rejected with `413`.
    pub max_request_bytes: usize,
}

/// The gateway's HTTP surface. `/healthz` + `/readyz` are public (no auth, no body limit); the
/// `/v1/chat/completions` proxy is protected by the G5 auth middleware and a
/// [`DefaultBodyLimit`] cap. Both are applied via [`Router::route_layer`], which runs the layers
/// ONLY for the matched protected route — so the health probes stay outside auth AND the body
/// limit, and an unmatched path still 404s without first being challenged for a credential.
pub fn router(state: AppState) -> Router {
    // The auth middleware's state is the IAM port alone (`Arc<dyn Iam>`), independent of the
    // handler's `AppState` — so this clone is just the port, not the whole state.
    let auth = axum::middleware::from_fn_with_state(state.iam.clone(), require_iam_auth);
    // The body-size limit: an over-limit body fails the handler's `Bytes` extractor with `413`.
    // Note (M0): auth runs BEFORE the 413 (the limit is enforced at body extraction, after the
    // middleware) — acceptable here since auth reads only headers, never the body.
    let body_limit = DefaultBodyLimit::max(state.max_request_bytes);

    let protected = Router::new()
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route_layer(auth)
        .route_layer(body_limit)
        .with_state(state);

    Router::new().route("/healthz", get(healthz)).route("/readyz", get(readyz)).merge(protected)
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
    use crate::adapters::iam::IamError;
    use crate::config::OpenAiConfig;
    use axum::body::Body;
    use axum::http::Request;
    use paigasus_proto::paigasus::iam::v1::IntrospectApiKeyResponse;
    use secrecy::SecretString;
    use std::time::Duration;
    use tower::ServiceExt; // for `oneshot`

    /// A never-invoked `Iam` for the health-route tests (they hit only the public, unauthenticated
    /// routes, so these methods are unreachable — the full auth-path table lives in G5's `auth.rs`
    /// unit tests and G7's `tests/chat_proxy.rs`).
    struct UnusedIam;

    #[async_trait::async_trait]
    impl Iam for UnusedIam {
        async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
            unreachable!("health routes never call IAM")
        }
        async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
            unreachable!("health routes never call IAM")
        }
    }

    /// A test `AppState` whose OpenAI client points nowhere in particular (it is never called by the
    /// health routes) and whose IAM port is the never-invoked fake.
    fn test_state() -> AppState {
        let cfg = OpenAiConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: SecretString::from("sk-unused".to_string()),
        };
        let openai = OpenAiClient::new(&cfg, Duration::from_secs(1), Duration::from_secs(1), Duration::from_secs(1)).expect("client builds");
        AppState {
            iam: Arc::new(UnusedIam),
            openai: Arc::new(openai),
            max_request_bytes: 1_048_576,
        }
    }

    #[tokio::test]
    async fn healthz_returns_200_with_status_ok_body() {
        let app = router(test_state());
        let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn readyz_returns_200_placeholder() {
        let app = router(test_state());
        let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn chat_route_requires_auth_missing_bearer_is_401() {
        // The protected route is behind the auth middleware; with no bearer it 401s before any
        // IAM/upstream call (proves the layer is wired, without needing a live upstream).
        let app = router(test_state());
        let req = Request::builder().method("POST").uri("/v1/chat/completions").body(Body::from("{}")).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
