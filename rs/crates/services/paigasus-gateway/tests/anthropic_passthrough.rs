// SPDX-License-Identifier: Apache-2.0
//! SMA-750 D11, D20: `POST /v1/messages/count_tokens` and `GET /v1/models` through the real
//! router, the Anthropic auth, the re-render layer and the limits, against the in-process mock
//! upstream.

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use paigasus_gateway::adapters::anthropic::AnthropicClient;
use paigasus_gateway::adapters::http::{AppState, MessagesSettings, router};
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::config::AnthropicConfig;
use paigasus_gateway::service_info::Capabilities;
use paigasus_logging::test_support::capture_logs;
use secrecy::SecretString;
use serde_json::Value;
use support::MockOpenAi;
use support::limits::{CALLER_KEY, ORG_A_PRN, PRINCIPAL_1, RecordingStore, ScopedIam, Sent, limits, openai, rules, sample};
use tower::ServiceExt;

const MIB: usize = 1 << 20;
const OPERATOR_KEY: &str = "operator-anthropic-test-key";
/// A `count_tokens` request body. It has duplicate keys on purpose: the gateway never parses it.
const COUNT_BODY: &str = r#"{"model":"claude-x","messages":[{"role":"user","content":"hi"}],"model":"claude-y"}"#;
/// The Task 1 answers of the two routes (shapes only; the gateway forwards them as they are).
const COUNT_ANSWER: &str = r#"{"input_tokens":14}"#;
const MODELS_ANSWER: &str = r#"{"data":[{"type":"model","id":"claude-x","display_name":"X","created_at":"2025-01-01T00:00:00Z"}],"has_more":false,"first_id":"claude-x","last_id":"claude-x"}"#;
const ERROR_404: &str = r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-nope"},"request_id":"req_up_1"}"#;
const ERROR_401: &str = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_up_2"}"#;

fn settings() -> MessagesSettings {
    MessagesSettings {
        max_request_bytes: 32 * MIB,
        max_record_bytes: 8 * MIB,
        max_held_bytes: 32 * MIB,
        max_hold: Duration::from_secs(600),
    }
}

fn anthropic(base_url: &str) -> AnthropicClient {
    let cfg = AnthropicConfig {
        base_url: base_url.to_owned(),
        api_key: SecretString::from(OPERATOR_KEY.to_owned()),
        extra_ca_bundle_path: None,
        default_version: "2023-06-01".to_owned(),
        non_stream_timeout_secs: 600,
    };
    AnthropicClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(300)).expect("client builds")
}

fn app(base_url: &str, on: bool, limits: Option<Arc<Limits>>) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: true },
        limits,
        dpop_enabled: false,
        tool_interception: None,
        tool_policy: None,
        responses: None,
        anthropic: on.then(|| Arc::new(anthropic(base_url))),
        messages: on.then(|| Arc::new(settings())),
    })
}

fn req(method: &str, uri: &str, body: &str, key: Option<&str>, extra: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        b = b.header("x-api-key", key);
    }
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(Body::from(body.to_owned())).expect("build request")
}

async fn read(app: &Router, req: Request<Body>) -> Sent {
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

const VERSION: [(&str, &str); 1] = [("anthropic-version", "2023-06-01")];

async fn count(app: &Router) -> Sent {
    read(app, req("POST", "/v1/messages/count_tokens?beta=true", COUNT_BODY, Some(CALLER_KEY), &VERSION)).await
}

async fn models(app: &Router) -> Sent {
    read(app, req("GET", "/v1/models?limit=1000", "", Some(CALLER_KEY), &VERSION)).await
}

fn envelope_type(sent: &Sent) -> String {
    let v: Value = sent.json();
    assert_eq!(v["type"], "error", "{v}");
    v["error"]["type"].as_str().expect("a type").to_owned()
}

fn metric(name: &str, labels: &[(&str, &str)]) -> f64 {
    sample(&paigasus_observability::init("test-gateway-anthropic-passthrough").render(), name, labels).unwrap_or(0.0)
}

#[tokio::test]
async fn count_tokens_passes_through_with_zero_charge() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, COUNT_ANSWER).await;
    let store = RecordingStore::new();
    let app = app(&mock.base_url, true, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let sent = count(&app).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(sent.body.as_ref(), COUNT_ANSWER.as_bytes(), "the answer is byte-identical");
    let rec = mock.recorded().expect("recorded");
    assert_eq!((rec.method.as_str(), rec.path.as_str(), rec.query.as_deref()), ("POST", "/v1/messages/count_tokens", Some("beta=true")));
    assert_eq!(rec.body.as_ref(), COUNT_BODY.as_bytes(), "the body goes up unparsed and unchanged");
    assert_eq!(rec.header("x-api-key"), Some(OPERATOR_KEY));
    assert_eq!(store.checks(), 1, "one admission, one unit of the request rate");
    assert_eq!(store.charges(), Vec::<u64>::new(), "no token is charged");
}

#[tokio::test]
async fn models_passes_through_with_zero_charge() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, MODELS_ANSWER).await;
    let store = RecordingStore::new();
    let app = app(&mock.base_url, true, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let sent = models(&app).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(sent.body.as_ref(), MODELS_ANSWER.as_bytes());
    let rec = mock.recorded().expect("recorded");
    assert_eq!((rec.method.as_str(), rec.path.as_str(), rec.query.as_deref()), ("GET", "/v1/models", Some("limit=1000")));
    assert!(rec.body.is_empty());
    assert_eq!(store.checks(), 1);
    assert_eq!(store.charges(), Vec::<u64>::new(), "no token is charged");
}

/// D20: no `anthropic-version`, no Anthropic route: the OpenAI-envelope 404, with a good, a bad or
/// no credential, and no upstream request.
#[tokio::test]
async fn models_without_anthropic_version_is_a_404_in_the_openai_envelope() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, MODELS_ANSWER).await;
    let app = app(&mock.base_url, true, None);
    for key in [Some(CALLER_KEY), Some("not-a-key"), None] {
        for method in ["GET", "POST"] {
            let sent = read(&app, req(method, "/v1/models", "", key, &[])).await;
            assert_eq!(sent.status, StatusCode::NOT_FOUND, "{method} {key:?}: {}", sent.text());
            let v = sent.json();
            assert_eq!(v["error"]["type"], "invalid_request_error", "{v}");
            assert!(v.get("type").is_none(), "an OpenAI envelope, not the Anthropic one: {v}");
            assert!(sent.headers.get("www-authenticate").is_none(), "no credential challenge");
        }
    }
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn the_pass_through_routes_need_a_credential() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, COUNT_ANSWER).await;
    let app = app(&mock.base_url, true, None);
    let none = read(&app, req("POST", "/v1/messages/count_tokens", COUNT_BODY, None, &VERSION)).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    assert_eq!(envelope_type(&none), "authentication_error");
    let none = read(&app, req("GET", "/v1/models", "", None, &VERSION)).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    assert_eq!(envelope_type(&none), "authentication_error");
    assert_eq!(mock.request_count(), 0);
    assert_eq!(count(&app).await.status, StatusCode::OK, "x-api-key is accepted");
    assert_eq!(models(&app).await.status, StatusCode::OK, "x-api-key is accepted");
}

/// Review Focus 5: a non-2xx pass-through answer arrives unchanged, with its own `content-type`,
/// and charges nothing.
#[tokio::test]
async fn a_pass_through_error_is_forwarded_unchanged() {
    for (status, body, ct) in [
        (StatusCode::NOT_FOUND, ERROR_404, "application/json"),
        (StatusCode::UNAUTHORIZED, ERROR_401, "application/vnd.anthropic.error+json; charset=utf-8"),
    ] {
        let mock = MockOpenAi::spawn_json_with_headers(status, ct, vec![("request-id".to_owned(), "req_up".to_owned())], body).await;
        let store = RecordingStore::new();
        let app = app(&mock.base_url, true, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
        for sent in [count(&app).await, models(&app).await] {
            assert_eq!(sent.status, status);
            assert_eq!(sent.body.as_ref(), body.as_bytes(), "never re-rendered");
            assert_eq!(sent.header("content-type"), ct);
            assert_eq!(sent.header("request-id"), "req_up");
        }
        assert_eq!(store.charges(), Vec::<u64>::new(), "a non-2xx answer charges nothing");
    }
}

#[tokio::test]
async fn a_rate_refusal_on_count_tokens_is_429_in_the_anthropic_envelope() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, COUNT_ANSWER).await;
    let app = app(&mock.base_url, true, Some(limits(rules(Some(1), None, None), Arc::new(MemoryLimitStore::new()))));
    assert_eq!(count(&app).await.status, StatusCode::OK);
    let refused = count(&app).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{}", refused.text());
    assert_eq!(envelope_type(&refused), "rate_limit_error");
    assert_eq!(mock.request_count(), 1, "the refused request never reached the upstream");
}

#[tokio::test]
async fn the_pass_through_routes_are_404_when_disabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, COUNT_ANSWER).await;
    let app = app(&mock.base_url, false, None);
    for key in [Some(CALLER_KEY), None] {
        let sent = read(&app, req("POST", "/v1/messages/count_tokens", COUNT_BODY, key, &VERSION)).await;
        assert_eq!(sent.status, StatusCode::NOT_FOUND);
        let v = sent.json();
        assert_eq!((v["type"].as_str(), v["error"]["type"].as_str()), (Some("error"), Some("not_found_error")), "{v}");
        assert!(v["error"].get("code").is_none());
    }
    // `/v1/models` is not mounted: the plain 404 of an unknown path, with or without the header.
    for extra in [&VERSION[..], &[][..]] {
        let sent = read(&app, req("GET", "/v1/models", "", Some(CALLER_KEY), extra)).await;
        assert_eq!(sent.status, StatusCode::NOT_FOUND);
        assert!(sent.body.is_empty(), "the router's own 404, not an envelope: {}", sent.text());
    }
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn the_pass_through_routes_forward_the_d8_headers() {
    let upstream_headers: Vec<(String, String)> = [
        ("request-id", "req_up_9"),
        ("retry-after", "7"),
        ("x-should-retry", "true"),
        ("anthropic-ratelimit-requests-limit", "50"),
        ("x-upstream-other", "x"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let mock = MockOpenAi::spawn_json_with_headers(StatusCode::OK, "application/json", upstream_headers, COUNT_ANSWER).await;
    let app = app(&mock.base_url, true, None);
    for sent in [count(&app).await, models(&app).await] {
        assert_eq!(sent.status, StatusCode::OK);
        assert_eq!((sent.header("request-id"), sent.header("retry-after"), sent.header("x-should-retry")), ("req_up_9", "7", "true"));
        assert!(sent.headers.get("anthropic-ratelimit-requests-limit").is_none());
        assert!(sent.headers.get("x-upstream-other").is_none());
    }
    // D5: the `anthropic-*` headers go up, the caller's key and the others do not.
    let extra = [
        ("anthropic-version", "2099-01-01"),
        ("anthropic-beta", "a"),
        ("anthropic-beta", "b"),
        ("paigasus-org", "x"),
        ("authorization", "Bearer pgs-other"),
    ];
    assert_eq!(read(&app, req("POST", "/v1/messages/count_tokens", COUNT_BODY, Some(CALLER_KEY), &extra)).await.status, StatusCode::OK);
    let rec = mock.recorded().expect("recorded");
    let all = |name: &str| rec.headers.get_all(name).iter().map(|v| v.to_str().expect("ascii").to_owned()).collect::<Vec<_>>();
    assert_eq!(all("x-api-key"), [OPERATOR_KEY]);
    assert_eq!(all("anthropic-version"), ["2099-01-01"]);
    assert_eq!(all("anthropic-beta"), ["a", "b"]);
    for gone in ["authorization", "paigasus-org"] {
        assert!(rec.headers.get(gone).is_none(), "{gone} must not go upstream");
    }
}

#[tokio::test]
async fn the_pass_through_routes_label_their_endpoint() {
    let (logs, _guard) = capture_logs();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, COUNT_ANSWER).await;
    let app = app(&mock.base_url, true, None);
    let series = |endpoint: &str| metric("gateway_upstream_requests_total", &[("status_class", "2xx"), ("endpoint", endpoint)]);
    let (c0, m0) = (series("messages_count_tokens"), series("models"));
    assert_eq!(count(&app).await.status, StatusCode::OK);
    assert_eq!(models(&app).await.status, StatusCode::OK);
    assert_eq!(series("messages_count_tokens") - c0, 1.0);
    assert_eq!(series("models") - m0, 1.0);
    let text = logs.text();
    for endpoint in ["messages_count_tokens", "models"] {
        let line = text
            .lines()
            .find(|l| l.contains("anthropic request proxied") && l.contains(&format!("endpoint=\"{endpoint}\"")))
            .unwrap_or_else(|| panic!("no {endpoint} line:\n{text}"));
        for field in ["status=200", "latency_ms=", "principal=", "scope=", "auth=\"api_key\""] {
            assert!(line.contains(field), "{field} missing: {line}");
        }
    }
}
