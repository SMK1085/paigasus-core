// SPDX-License-Identifier: Apache-2.0

//! SMA-505 AC 1/2/3 for the gateway's descriptor. No database and no Docker — this crate's
//! harness drives the router via `oneshot` against a fake IAM, mirroring `tests/chat_proxy.rs`'s
//! app-builder shape and `tests/metrics.rs`'s recorder pattern.
//!
//! Covered: missing bearer -> 401; a valid API key -> 200 with the full descriptor (capability
//! list compared as a SET, since the proto declares it unordered); a valid OIDC token -> 200
//! against a fake whose `is_authorized_self` panics (proves discovery makes no authorization
//! call); a validated-but-unprovisioned identity -> 200 on discovery but 401 on
//! `/v1/chat/completions` with the SAME credential; `stream_enabled = false` -> 200 with
//! `"capabilities":[]`, never omitted; and IAM unreachable on both legs -> 503, with
//! `gateway_iam_calls_total{operation="introspect_token",result="unavailable"}` increased by
//! exactly 1 — read by PARSING A NUMBER out of the rendered Prometheus exposition, never
//! `contains()` on a `# TYPE` line (that line is emitted whether or not the counter was ever
//! incremented; this exact failure has shipped twice in this repo, see `tests/metrics.rs`'s doc).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use secrecy::SecretString;
use serde_json::Value;
use tonic::Status;
use tonic_types::{ErrorDetails, StatusExt};
use tower::ServiceExt; // for `oneshot`

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{Iam, IamError};
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};

const CALLER_KEY: &str = "sk-caller-secret";
const CALLER_SA: &str = "prn:paigasus:iam:default:sa/gw-caller";
const CALLER_SCOPE: &str = "prn:paigasus:iam:default:scope/team-a";
const CALLER_KEY_ID: &str = "key-abc123";

const CONSOLE_TOKEN: &str = "console-oidc-token";
const CONSOLE_PRINCIPAL: &str = "prn:paigasus:iam:default:user/console-user";

const NON_STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;

// ---- fake IAM -----------------------------------------------------------------------------

/// The `introspect_api_key` outcome a [`FakeIam`] should produce.
enum ApiKeyOutcome {
    Active,
    Unauthenticated,
    /// A channel/connect-time failure — models an unreachable IAM.
    Connect,
    /// The middleware under test must never call `introspect_api_key` at all — a hit is a test
    /// bug (or a real regression), so it panics loudly rather than modeling a scenario.
    Unreachable,
}

/// The `introspect_token` outcome a [`FakeIam`] should produce.
enum TokenOutcome {
    Active,
    /// A VALIDATED token whose identity has no local principal (ADR-0020 D4).
    PermissionDenied,
    /// A channel/connect-time failure — models an unreachable IAM.
    Connect,
    /// The middleware under test must never call `introspect_token` at all — a hit is a test bug
    /// (or a real regression), so it panics loudly rather than modeling a scenario.
    Unreachable,
}

/// Whether `is_authorized_self` may be called. Every discovery test wires `Unreachable` — the
/// point of `require_authenticated` is that discovery performs NO authorization, so a stray
/// authz call must fail the test loudly rather than pass silently.
enum AuthzOutcome {
    Unreachable,
}

/// A canned, no-network `Iam` for the discovery tests.
struct FakeIam {
    api_key: ApiKeyOutcome,
    token: TokenOutcome,
    authz: AuthzOutcome,
}

#[async_trait::async_trait]
impl Iam for FakeIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        match self.api_key {
            ApiKeyOutcome::Active => Ok(IntrospectApiKeyResponse {
                principal_prn: CALLER_SA.to_owned(),
                status: "active".to_owned(),
                key_id: CALLER_KEY_ID.to_owned(),
                expires_at: None,
                memberships: Vec::new(),
                role_grants: Vec::new(),
                scope_prn: CALLER_SCOPE.to_owned(),
            }),
            ApiKeyOutcome::Unauthenticated => Err(IamError::Rpc(Status::unauthenticated("invalid key"))),
            ApiKeyOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
            ApiKeyOutcome::Unreachable => panic!("introspect_api_key must not be called in this scenario"),
        }
    }

    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        match self.authz {
            AuthzOutcome::Unreachable => panic!("discovery must never call is_authorized_self — it performs no authorization"),
        }
    }

    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        match self.token {
            TokenOutcome::Active => Ok(IntrospectResponse {
                principal_prn: CONSOLE_PRINCIPAL.to_owned(),
                status: "active".to_owned(),
                issuer: "https://issuer.example.com".to_owned(),
                subject: "console-user".to_owned(),
                expires_at: None,
                memberships: Vec::new(),
                role_grants: Vec::new(),
            }),
            // SMA-504: `require_authenticated` now narrows to `ErrorInfo`'s
            // `identity-not-provisioned` reason, not the bare code — so the fake must carry the
            // same details IAM's `authn_status` (Task 4) actually attaches, or this relaxation
            // (ADR-0020 D4) would fail closed.
            TokenOutcome::PermissionDenied => Err(IamError::Rpc(Status::with_error_details(
                tonic::Code::PermissionDenied,
                "not provisioned",
                ErrorDetails::with_error_info(
                    paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned.as_wire_reason().expect("a declared reason"),
                    &*paigasus_proto::error::IAM_DOMAIN,
                    std::collections::HashMap::new(),
                ),
            ))),
            TokenOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
            TokenOutcome::Unreachable => panic!("introspect_token must not be called in this scenario"),
        }
    }
}

// ---- app + request builders ----------------------------------------------------------------

/// An `OpenAiClient` pointed nowhere in particular — the discovery route never calls the
/// upstream, so it is only there to satisfy [`AppState`].
fn unused_openai() -> OpenAiClient {
    let cfg = OpenAiConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: SecretString::from("sk-unused".to_string()),
    };
    OpenAiClient::new(&cfg, Duration::from_secs(1), Duration::from_secs(1), Duration::from_secs(1)).expect("client builds")
}

fn app_for(fake: FakeIam, stream_enabled: bool) -> Router {
    let state = AppState {
        iam: Arc::new(fake),
        openai: Arc::new(unused_openai()),
        max_request_bytes: 1_048_576,
        capabilities: paigasus_gateway::service_info::Capabilities { chat_stream: stream_enabled },
    };
    router(state)
}

/// A `GET /v1/service-info` request with an optional bearer token. Built off the shared
/// [`paigasus_service_info::ROUTE`] constant so the path literal cannot drift from the handler.
fn discovery_request(bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(paigasus_service_info::ROUTE);
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    builder.body(Body::empty()).expect("build request")
}

/// A `POST /v1/chat/completions` request with an optional bearer token — used by case 4 to prove
/// a discovery-only credential does not gain chat access.
fn chat_request(bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/v1/chat/completions").header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    builder.body(Body::from(NON_STREAM_BODY)).expect("build request")
}

async fn body_json(resp: Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Reads the descriptor's capability list as a set — the proto declares the list unordered, so
/// tests must never assert it as a sequence.
fn capability_set(body: &Value) -> HashSet<String> {
    body["capabilities"]
        .as_array()
        .expect("capabilities must be an array, never absent")
        .iter()
        .map(|v| v.as_str().expect("capability keys are strings").to_string())
        .collect()
}

// ---- 1: no Authorization header -> 401 -------------------------------------------------------

#[tokio::test]
async fn missing_bearer_is_401() {
    // Every leg is `Unreachable` — a missing bearer must 401 before any IAM call at all.
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Unreachable,
        token: TokenOutcome::Unreachable,
        authz: AuthzOutcome::Unreachable,
    };
    let app = app_for(fake, true);
    let resp = app.oneshot(discovery_request(None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---- 2: valid API key -> 200 with the full descriptor, capabilities as a SET -----------------

#[tokio::test]
async fn valid_api_key_returns_200_with_the_full_descriptor() {
    // The API-key leg succeeds, so `introspect_token` must never be reached.
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Active,
        token: TokenOutcome::Unreachable,
        authz: AuthzOutcome::Unreachable,
    };
    let app = app_for(fake, true);
    let resp = app.oneshot(discovery_request(Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert_eq!(body["service"], "gateway");
    assert!(body["version"].as_str().is_some_and(|v| !v.is_empty()), "version must be a non-empty string: {body}");
    assert_eq!(
        capability_set(&body),
        HashSet::from(["gateway.chat.stream".to_string()]),
        "capability list must be compared as a set: {body}"
    );
}

// ---- 3: valid OIDC token -> 200, and discovery makes no authorization call --------------------

#[tokio::test]
async fn valid_oidc_token_returns_200_without_authorizing() {
    // The API-key leg fails (not an API key), the token leg succeeds; `is_authorized_self` is
    // `Unreachable` on this fake, so reaching it fails the test loudly.
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Unauthenticated,
        token: TokenOutcome::Active,
        authz: AuthzOutcome::Unreachable,
    };
    let app = app_for(fake, true);
    let resp = app.oneshot(discovery_request(Some(CONSOLE_TOKEN))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---- 4: validated-but-unprovisioned -> 200 on discovery, 401 on chat -------------------------

#[tokio::test]
async fn validated_but_unprovisioned_identity_is_200_on_discovery_and_401_on_chat() {
    const CREDENTIAL: &str = "validated-but-unprovisioned-token";
    // IAM returns `PermissionDenied` for a VALIDATED token whose identity has no local
    // principal — discovery must still accept it (ADR-0020 D4), while the SAME credential must
    // still be rejected on the authorizing chat path.
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Unauthenticated,
        token: TokenOutcome::PermissionDenied,
        authz: AuthzOutcome::Unreachable,
    };
    let app = app_for(fake, true);

    let resp = app.clone().oneshot(discovery_request(Some(CREDENTIAL))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a validated-but-unprovisioned identity must still pass discovery (ADR-0020 D4)");

    // `require_iam_auth` (the chat path) only ever tries `introspect_api_key`, which this fake
    // maps to `Unauthenticated` for this same credential — so the SAME token must not gain chat
    // access.
    let resp = app.oneshot(chat_request(Some(CREDENTIAL))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "the same credential must not gain chat access");
}

// ---- 5: stream_enabled = false -> 200 with capabilities emitted as [] -------------------------

#[tokio::test]
async fn streaming_disabled_serves_an_empty_capability_array() {
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Active,
        token: TokenOutcome::Unreachable,
        authz: AuthzOutcome::Unreachable,
    };
    let app = app_for(fake, false);
    let resp = app.oneshot(discovery_request(Some(CALLER_KEY))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert_eq!(body["capabilities"], serde_json::json!([]), "capabilities must be emitted as [], never omitted: {body}");
}

// ---- 6: IAM unreachable on both legs -> 503, metric increases by exactly 1 --------------------

/// The `gateway_iam_calls_total{operation="introspect_token",result="unavailable"}` series,
/// parsed out of the rendered Prometheus exposition — filtering `#`-prefixed lines is
/// load-bearing, not cosmetic: `PrometheusHandle::render()` writes a `# TYPE gateway_iam_calls_
/// total counter` line for every REGISTERED metric regardless of whether it was ever
/// incremented, so a `contains()`-style assertion is satisfied by the comment alone and can
/// never fail (this exact failure has shipped twice in this repo — see `tests/metrics.rs`'s
/// doc). Mirrors `paigasus-iam`'s `tests/relay_pg.rs::poll_wakeups_total_from`.
fn introspect_token_unavailable_total(rendered: &str) -> f64 {
    rendered
        .lines()
        .filter(|l| !l.starts_with('#'))
        .find(|l| l.contains("gateway_iam_calls_total") && l.contains(r#"operation="introspect_token""#) && l.contains(r#"result="unavailable""#))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

#[tokio::test]
async fn iam_unreachable_on_both_legs_is_503_and_records_the_metric() {
    let handle = paigasus_observability::init("test-gateway-service-info-iam-unavailable");
    let fake = FakeIam {
        api_key: ApiKeyOutcome::Connect,
        token: TokenOutcome::Connect,
        authz: AuthzOutcome::Unreachable,
    };
    let app: Router = app_for(fake, true).merge(paigasus_observability::metrics_router(handle.clone()));

    let baseline = introspect_token_unavailable_total(&handle.render());
    let resp = app.oneshot(discovery_request(Some("any-token"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    let after = introspect_token_unavailable_total(&handle.render());
    assert_eq!(
        after - baseline,
        1.0,
        "gateway_iam_calls_total{{operation=\"introspect_token\",result=\"unavailable\"}} must increase by exactly 1"
    );
}
