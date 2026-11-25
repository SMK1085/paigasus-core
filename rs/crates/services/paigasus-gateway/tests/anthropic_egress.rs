// SPDX-License-Identifier: Apache-2.0

//! Egress tests for the Anthropic client against the in-process mock upstream (SMA-750 Task 6).

mod support;

use std::time::Duration;

use axum::http::StatusCode;
use bytes::Bytes;
use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderValue};
use secrecy::SecretString;
use support::MockOpenAi;

use paigasus_gateway::adapters::anthropic::{AnthropicClient, AnthropicRoute};
use paigasus_gateway::adapters::upstream::{Provider, UpstreamError, UpstreamResponse};
use paigasus_gateway::config::AnthropicConfig;

const KEY: &str = "test-operator-key";

fn client_with(base_url: String, non_stream_secs: u64, idle: Duration) -> AnthropicClient {
    let cfg = AnthropicConfig {
        base_url,
        api_key: SecretString::from(KEY.to_string()),
        extra_ca_bundle_path: None,
        default_version: "2023-06-01".to_string(),
        non_stream_timeout_secs: non_stream_secs,
    };
    AnthropicClient::new(&cfg, Duration::from_secs(10), idle).expect("client builds")
}

fn client_for(mock: &MockOpenAi) -> AnthropicClient {
    client_with(mock.base_url.clone(), 600, Duration::from_secs(300))
}

fn body() -> Bytes {
    Bytes::from_static(br#"{"model":"m","max_tokens":1,"messages":[]}"#)
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap()
}

#[tokio::test]
async fn the_operator_key_and_only_anthropic_headers_go_upstream() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("x-api-key", "caller"),
        ("authorization", "Bearer caller"),
        ("paigasus-org", "org"),
        ("dpop", "x"),
        ("x-claude-code-session-id", "s"),
        ("anthropic-beta", "a"),
        ("user-agent", "u"),
    ] {
        h.insert(k, hv(v));
    }
    let fwd = client.forward(&h, None);
    client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let rec = mock.recorded().expect("recorded");
    assert_eq!(rec.header("x-api-key"), Some(KEY));
    assert_eq!(rec.header("anthropic-beta"), Some("a"));
    assert_eq!(rec.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(rec.header("content-type"), Some("application/json"));
    assert_eq!(rec.header("accept"), Some("application/json"));
    for gone in ["authorization", "paigasus-org", "dpop", "x-claude-code-session-id"] {
        assert!(rec.headers.get(gone).is_none(), "{gone} must not go upstream");
    }
    assert_ne!(rec.header("user-agent"), Some("u"));
}

#[tokio::test]
async fn forward_keeps_every_anthropic_value_in_order() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let mut h = HeaderMap::new();
    h.append("Anthropic-Beta", hv("b1"));
    h.append("anthropic-beta", hv("b2"));
    h.append("anthropic-version", hv("2023-06-01"));
    let fwd = client.forward(&h, None);
    assert_eq!(fwd.beta_values().as_deref(), Some("b1,b2"));
    client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let rec = mock.recorded().unwrap();
    let betas: Vec<&str> = rec.headers.get_all("anthropic-beta").iter().map(|v| v.to_str().unwrap()).collect();
    assert_eq!(betas, ["b1", "b2"]);
    assert_eq!(rec.headers.get_all("anthropic-version").iter().count(), 1);

    // No caller version: the default is sent once.
    let fwd = client.forward(&HeaderMap::new(), None);
    assert_eq!(fwd.beta_values(), None);
    client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let rec = mock.recorded().unwrap();
    let versions: Vec<&str> = rec.headers.get_all("anthropic-version").iter().map(|v| v.to_str().unwrap()).collect();
    assert_eq!(versions, ["2023-06-01"]);
}

#[tokio::test]
async fn the_query_cannot_change_the_host_or_the_path() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let q = "beta=true&next=http://evil.example/@x/../v1/models";
    let fwd = client.forward(&HeaderMap::new(), Some(q));
    client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let rec = mock.recorded().unwrap();
    assert_eq!(rec.path, "/v1/messages");
    assert_eq!(rec.query.as_deref(), Some(q));
    assert!(rec.header("host").unwrap().starts_with("127.0.0.1"));
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn a_get_has_no_content_type_and_a_stream_accepts_sse() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let fwd = client.forward(&HeaderMap::new(), None);
    client.send(AnthropicRoute::Models, Method::GET, Bytes::new(), false, &fwd).await.expect("get");
    let rec = mock.recorded().unwrap();
    assert_eq!(rec.method, "GET");
    assert_eq!(rec.path, "/v1/models");
    assert!(rec.headers.get("content-type").is_none());

    let sse = MockOpenAi::spawn_sse(vec!["x".to_string()]).await;
    let client = client_for(&sse);
    let resp = client.send(AnthropicRoute::Messages, Method::POST, body(), true, &fwd).await.expect("stream");
    assert!(matches!(resp, UpstreamResponse::Stream { .. }));
    assert_eq!(sse.recorded().unwrap().header("accept"), Some("text/event-stream"));
}

#[tokio::test]
async fn the_response_head_keeps_only_the_d8_headers() {
    let mock = MockOpenAi::spawn_json_with_headers(
        StatusCode::OK,
        "application/json",
        vec![
            ("request-id".into(), "req_1".into()),
            ("retry-after".into(), "7".into()),
            ("x-should-retry".into(), "true".into()),
            ("anthropic-ratelimit-requests-limit".into(), "50".into()),
        ],
        "{}",
    )
    .await;
    let client = client_for(&mock);
    let fwd = client.forward(&HeaderMap::new(), None);
    let resp = client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let UpstreamResponse::Full { head, .. } = resp else { panic!("expected Full") };
    assert_eq!(head.request_id, Some(hv("req_1")));
    assert_eq!(head.retry_after, Some(hv("7")));
    assert_eq!(head.should_retry, Some(hv("true")));
    assert_eq!(head.content_type, Some(hv("application/json")));
    let mut out = HeaderMap::new();
    head.apply(&mut out);
    assert_eq!(out.len(), 3, "apply writes the three D8 headers and nothing else: {out:?}");
    assert!(out.get("content-type").is_none());
    assert!(out.get("anthropic-ratelimit-requests-limit").is_none());
}

#[tokio::test]
async fn a_redirect_is_a_502_and_the_second_host_sees_nothing() {
    let b = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let a = MockOpenAi::spawn_redirect(StatusCode::TEMPORARY_REDIRECT, format!("{}/v1/messages", b.base_url)).await;
    let client = client_for(&a);
    let fwd = client.forward(&HeaderMap::new(), None);
    for stream in [false, true] {
        let err = client.send(AnthropicRoute::Messages, Method::POST, body(), stream, &fwd).await.expect_err("a 3xx is an error");
        assert!(
            matches!(err, UpstreamError::Redirect { provider: Provider::Anthropic, status } if status == reqwest::StatusCode::TEMPORARY_REDIRECT),
            "{err:?}"
        );
    }
    assert_eq!(a.request_count(), 2);
    assert_eq!(b.request_count(), 0);
}

#[tokio::test]
async fn a_slow_non_stream_head_survives_the_idle_timeout() {
    let mock = MockOpenAi::spawn_delayed_json(Duration::from_secs(2), StatusCode::OK, "{}").await;
    let client = client_with(mock.base_url.clone(), 3, Duration::from_secs(1));
    let fwd = client.forward(&HeaderMap::new(), None);
    let resp = client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("slow head is fine");
    assert!(matches!(resp, UpstreamResponse::Full { status, .. } if status == reqwest::StatusCode::OK));
}

#[tokio::test]
async fn a_non_2xx_is_a_full_response() {
    let text = r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::from_u16(529).unwrap(), text).await;
    let client = client_for(&mock);
    let fwd = client.forward(&HeaderMap::new(), None);
    let resp = client.send(AnthropicRoute::Messages, Method::POST, body(), false, &fwd).await.expect("send");
    let UpstreamResponse::Full { status, body, .. } = resp else { panic!("expected Full") };
    assert_eq!(status.as_u16(), 529);
    assert_eq!(body, text.as_bytes());
}

#[tokio::test]
async fn debug_never_leaks_the_anthropic_key() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let rendered = format!("{client:?}");
    assert!(!rendered.contains(KEY), "{rendered}");
}
