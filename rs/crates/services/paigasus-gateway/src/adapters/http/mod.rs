// SPDX-License-Identifier: Apache-2.0

//! axum HTTP surface: `/healthz` (liveness, no dependency checks) and `/readyz` (a real
//! readiness probe — IAM reachability via a sentinel introspect; the upstream is validated by
//! config-presence at boot) stay public; the protected `/v1/chat/completions` proxy ([`chat`]) is
//! fronted by the auth middleware ([`auth`]) plus a request-body size limit and renders failures
//! through the OpenAI-compatible error envelope ([`error`]). The inbound chat-completion request
//! DTO ([`dto`]) is parsed only to read `model`/`stream`. `GET /v1/service-info` ([`service_info`],
//! SMA-505) is a THIRD, separately-protected group: it authenticates via [`auth::require_authenticated`]
//! but never authorizes, and carries no body limit.

pub mod auth;
pub mod chat;
pub mod dto;
pub mod error;
pub mod service_info;

pub use auth::{require_authenticated, require_iam_auth};
pub use dto::ChatCompletionRequest;
pub use error::GatewayError;

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, State};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, http::StatusCode, routing::get, routing::post};
use serde_json::json;

use crate::adapters::iam::{Iam, IamError};
use crate::adapters::openai::OpenAiClient;

/// Deliberately-INVALID sentinel token for the `/readyz` IAM-reachability probe. It has no
/// `pgs_sk_` prefix, so a REACHABLE IAM parses and rejects it (an application-level `Status`, e.g.
/// `Unauthenticated`) while an UNREACHABLE IAM fails at the transport. It authenticates nothing —
/// the introspect is a cheap, unaudited parse-reject, never a real credential.
const READYZ_PROBE_TOKEN: &str = "readyz-probe";

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
    /// SMA-505: whether streamed completions are served. `false` makes `chat` reject
    /// `stream: true` with `400` and withdraws `gateway.chat.stream` from the descriptor.
    pub stream_enabled: bool,
}

/// The gateway's HTTP surface. `/healthz` + `/readyz` are public (no auth, no body limit); the
/// `/v1/chat/completions` proxy is protected by the G5 auth middleware and a
/// [`DefaultBodyLimit`] cap; `GET /v1/service-info` (SMA-505) is protected by
/// [`require_authenticated`] alone, in its OWN `route_layer` group. Each protected group's
/// middleware is applied via [`Router::route_layer`], which runs the layers ONLY for the matched
/// route in THAT group — so the health probes stay outside auth AND the body limit, the
/// descriptor never inherits the chat group's authorization check or body limit, and an
/// unmatched path still 404s without first being challenged for a credential.
///
/// The discovery route deliberately does NOT join `protected`: `require_iam_auth` runs a D9
/// self-query authorization on top of authentication, and discovery must authenticate but never
/// authorize (a caller who legitimately cannot invoke the model must still be able to learn that
/// streaming exists). It also needs no [`DefaultBodyLimit`] — it is a GET with no body. Merging
/// it into `protected` would silently saddle it with both.
///
/// The shared [`AppState`] is applied ONCE, at the end, over the whole tree so the stateful
/// `readyz` (it probes IAM), the protected chat handler, and the descriptor handler all read the
/// same state. `healthz` takes no state — a stateless handler is still valid inside a stateful
/// router.
///
/// [`paigasus_observability::http_metrics_layer`] is applied over the whole tree (health routes
/// included — they get a bounded `route` label, which is acceptable). `/metrics` itself is
/// deliberately NOT part of this router: it is merged in by `main` (or served on its own
/// listener) AFTER this function returns, so a scrape never inflates its own request metrics.
pub fn router(state: AppState) -> Router {
    // The auth middleware's state is the IAM port alone (`Arc<dyn Iam>`), captured here BEFORE the
    // final `with_state`, and independent of the handler's `AppState` — so this clone is just the
    // port, not the whole state.
    let auth = axum::middleware::from_fn_with_state(state.iam.clone(), require_iam_auth);
    // The body-size limit: an over-limit body fails the handler's `Bytes` extractor with `413`.
    // Note (M0): auth runs BEFORE the 413 (the limit is enforced at body extraction, after the
    // middleware) — acceptable here since auth reads only headers, never the body.
    let body_limit = DefaultBodyLimit::max(state.max_request_bytes);

    let protected = Router::new().route("/v1/chat/completions", post(chat::chat_completions)).route_layer(auth).route_layer(body_limit);

    // SMA-505: its own group, because discovery authenticates but does not authorize, and needs
    // no body limit (it is a GET). `route_layer` keeps the middleware off unmatched paths, so a
    // 404 is still a 404 rather than a credential challenge.
    let discovery_auth = axum::middleware::from_fn_with_state(state.iam.clone(), require_authenticated);
    let discovery = Router::new().route(paigasus_service_info::ROUTE, get(service_info::get_service_info)).route_layer(discovery_auth);

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(protected)
        .merge(discovery)
        .layer(paigasus_observability::http_metrics_layer("gateway"))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// Readiness probe: `200 {"status":"ready"}` only when the gateway can actually serve. For M0 the
/// meaningful hard dependency is IAM (every request introspects + authorizes against it), so
/// readiness turns on IAM reachability; a transient upstream blip must NOT flap readiness, so the
/// OpenAI upstream is checked by config-presence only (see below), never a per-poll network probe.
///
/// ## IAM reachability via a sentinel introspect (a dedicated gRPC health RPC is a follow-up)
/// The M0 [`Iam`] port exposes no health RPC, so reachability is inferred from a cheap sentinel
/// `IntrospectApiKey` with the deliberately-invalid [`READYZ_PROBE_TOKEN`]: a REACHABLE IAM parses
/// and rejects it, returning an application-level `Status` (e.g. `Unauthenticated`); an UNREACHABLE
/// IAM fails at the transport ([`IamError::Connect`], or an `Rpc` `Status` of
/// `Unavailable`/`DeadlineExceeded`/`Internal`). The probe is a plain parse-reject — cheap and NOT
/// audited (it authenticates nothing). A dedicated IAM gRPC health RPC is the documented follow-up.
///
/// Classification: transport/backend outcomes (`Connect` / `Unavailable` / `DeadlineExceeded` /
/// `Internal`) → NOT ready (`503`) — kept consistent with `auth::introspect_error`'s request-path
/// mapping so the probe reports not-ready during the same IAM failures that break real requests;
/// ANY other outcome (an application-level `Status` such as `Unauthenticated`, or even an `Ok`)
/// proves IAM answered → ready (`200`).
///
/// ## Upstream — config-presence only for M0 (a live probe is a follow-up)
/// `upstream.openai.{base_url,api_key}` are validated non-empty at boot by
/// `GatewayConfig::validate`, so there is nothing further to check here without a live network
/// probe on every poll (flaky + rate-costly). A live upstream-reachability probe is a documented
/// follow-up; deliberately NOT added here.
async fn readyz(State(state): State<AppState>) -> Response {
    match state.iam.introspect_api_key(READYZ_PROBE_TOKEN).await {
        // Transport/backend failures → IAM unreachable or unhealthy → not ready. `Internal` is
        // included so this stays consistent with `auth::introspect_error`, which maps `Internal` to
        // `IamUnavailable` (503) on the request path: a persistent `Internal` from IAM would
        // otherwise make `/readyz` report ready while every real chat request fails 503 — exactly
        // the outage the probe exists to catch.
        Err(IamError::Connect(_)) => iam_not_ready(),
        Err(IamError::Rpc(status)) if matches!(status.code(), tonic::Code::Unavailable | tonic::Code::DeadlineExceeded | tonic::Code::Internal) => iam_not_ready(),
        // Any other application-level `Status` (e.g. `Unauthenticated`) or an `Ok` proves IAM answered.
        Err(IamError::Rpc(_)) | Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response(),
    }
}

/// The `503` readiness response when the IAM dependency is unreachable. Static, dependency-scoped,
/// and carries no upstream detail.
fn iam_not_ready() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "not_ready", "dependency": "iam" }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OpenAiConfig;
    use axum::body::Body;
    use axum::http::Request;
    use paigasus_proto::paigasus::iam::v1::IntrospectApiKeyResponse;
    use secrecy::SecretString;
    use std::time::Duration;
    use tonic::Status;
    use tower::ServiceExt; // for `oneshot`

    /// A never-invoked `Iam` for the routes that must NOT touch IAM (`/healthz`, and the unauth
    /// `/v1/chat/completions` row that 401s before any IAM call). Its methods panic, so a test
    /// using it that DID reach IAM would fail loudly — which is exactly how `/healthz` proves it
    /// has no IAM dependency. The full auth-path table lives in G5's `auth.rs` unit tests and G7's
    /// `tests/chat_proxy.rs`.
    struct UnusedIam;

    #[async_trait::async_trait]
    impl Iam for UnusedIam {
        async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
            unreachable!("this route must never call IAM")
        }
        async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
            unreachable!("this route must never call IAM")
        }
        async fn introspect_token(&self, _token: &str) -> Result<paigasus_proto::paigasus::iam::v1::IntrospectResponse, IamError> {
            unreachable!("this route must never call IAM")
        }
    }

    /// The introspect outcome a [`ProbeIam`] returns for the `/readyz` sentinel probe — either a
    /// REACHABLE IAM (it answered: an `Ok`, or an application-level `Unauthenticated`) or an
    /// UNREACHABLE/UNHEALTHY one (a transport/backend `Unavailable`/`DeadlineExceeded`/`Internal`
    /// `Rpc`, or `Connect`).
    #[derive(Clone, Copy)]
    enum Probe {
        ReachableOk,
        ReachableUnauthenticated,
        UnreachableUnavailable,
        UnreachableDeadline,
        UnreachableInternal,
        UnreachableConnect,
    }

    /// A configurable `Iam` for the readiness tests: `introspect_api_key` yields the selected
    /// [`Probe`] outcome so `/readyz`'s reachable/unreachable classification is exercised.
    struct ProbeIam(Probe);

    #[async_trait::async_trait]
    impl Iam for ProbeIam {
        async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
            match self.0 {
                Probe::ReachableOk => Ok(IntrospectApiKeyResponse::default()),
                Probe::ReachableUnauthenticated => Err(IamError::Rpc(Status::unauthenticated("invalid key"))),
                Probe::UnreachableUnavailable => Err(IamError::Rpc(Status::unavailable("iam is down"))),
                Probe::UnreachableDeadline => Err(IamError::Rpc(Status::deadline_exceeded("iam timed out"))),
                Probe::UnreachableInternal => Err(IamError::Rpc(Status::internal("iam internal error"))),
                Probe::UnreachableConnect => Err(IamError::Connect("channel build failed".to_string())),
            }
        }
        async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
            unreachable!("the readiness probe never authorizes")
        }
        async fn introspect_token(&self, _token: &str) -> Result<paigasus_proto::paigasus::iam::v1::IntrospectResponse, IamError> {
            unreachable!("the readiness probe never introspects a token")
        }
    }

    /// An OpenAI client that points nowhere in particular — the health/readiness routes never call
    /// the upstream, so it is only there to satisfy [`AppState`].
    fn unused_openai() -> OpenAiClient {
        let cfg = OpenAiConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: SecretString::from("sk-unused".to_string()),
        };
        OpenAiClient::new(&cfg, Duration::from_secs(1), Duration::from_secs(1), Duration::from_secs(1)).expect("client builds")
    }

    /// A test `AppState` over the given IAM port (the OpenAI client is never called by these routes).
    fn state_with_iam(iam: Arc<dyn Iam>) -> AppState {
        AppState {
            iam,
            openai: Arc::new(unused_openai()),
            max_request_bytes: 1_048_576,
            stream_enabled: true,
        }
    }

    async fn get_status(app: Router, uri: &str) -> StatusCode {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn healthz_returns_200_with_status_ok_body_and_no_iam_dependency() {
        // `UnusedIam` panics if called — so a 200 here also proves `/healthz` never touches IAM.
        let app = router(state_with_iam(Arc::new(UnusedIam)));
        let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn healthz_stays_200_even_when_iam_is_unreachable() {
        // Liveness must not flap on a dependency outage: `/healthz` is 200 regardless of IAM.
        let app = router(state_with_iam(Arc::new(ProbeIam(Probe::UnreachableUnavailable))));
        assert_eq!(get_status(app, "/healthz").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_is_200_when_iam_is_reachable() {
        // Both a reachable-Ok and a reachable-but-rejecting (`Unauthenticated`) IAM prove IAM
        // answered → ready.
        for probe in [Probe::ReachableOk, Probe::ReachableUnauthenticated] {
            let app = router(state_with_iam(Arc::new(ProbeIam(probe))));
            assert_eq!(get_status(app, "/readyz").await, StatusCode::OK, "reachable IAM must be ready");
        }
    }

    #[tokio::test]
    async fn readyz_is_503_when_iam_is_unreachable() {
        // A transport/backend outcome (Connect, or an Unavailable/DeadlineExceeded/Internal Rpc) →
        // not ready. `Internal` is included to match `introspect_error`'s request-path mapping.
        for probe in [Probe::UnreachableUnavailable, Probe::UnreachableDeadline, Probe::UnreachableInternal, Probe::UnreachableConnect] {
            let app = router(state_with_iam(Arc::new(ProbeIam(probe))));
            assert_eq!(get_status(app, "/readyz").await, StatusCode::SERVICE_UNAVAILABLE, "unreachable IAM must be not-ready");
        }
    }

    #[tokio::test]
    async fn chat_route_requires_auth_missing_bearer_is_401() {
        // The protected route is behind the auth middleware; with no bearer it 401s before any
        // IAM/upstream call (proves the layer is wired, without needing a live upstream). `UnusedIam`
        // panics if reached, so the 401 also proves no IAM call happens on the missing-bearer path.
        let app = router(state_with_iam(Arc::new(UnusedIam)));
        let req = Request::builder().method("POST").uri("/v1/chat/completions").body(Body::from("{}")).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
