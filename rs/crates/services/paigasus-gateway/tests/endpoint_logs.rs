// SPDX-License-Identifier: Apache-2.0

//! SMA-769 § 8.1: the request log lines carry the `endpoint` field, for chat and for Responses.
//! These tests drive the real router with limits on, so the pipeline wiring is covered (the
//! `proxied` line, the `refused by a limit` line, and the `metered` line of the charge guard).

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::service_info::Capabilities;
use paigasus_logging::test_support::capture_logs_at;
use support::MockOpenAi;
use support::limits::*;
use tower::ServiceExt;

const RESPONSES_BODY: &str = r#"{"model":"gpt-6","input":"hi"}"#;
const RESPONSES_OK: &str = r#"{"id":"resp_1","object":"response","status":"completed","output":[],"usage":{"total_tokens":5}}"#;

fn app_for(base_url: &str) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: 1_048_576,
        capabilities: Capabilities { chat_stream: true },
        // One request per window: the second request is refused by a limit.
        limits: Some(limits(rules(Some(1), None, None), Arc::new(MemoryLimitStore::new()))),
        dpop_enabled: false,
        tool_interception: None,
        tool_policy: None,
        responses: Some(Arc::new(ResponsesSettings {
            max_request_bytes: 1_048_576,
            max_record_bytes: 1_048_576,
            max_held_bytes: 2_097_152,
        })),
    })
}

async fn status_of(app: &Router, path: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(body.to_owned()))
        .expect("request");
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    status
}

/// One admitted request, then one refused request. Each of the three lines must name `endpoint`.
async fn lines_name_the_endpoint(path: &str, body: &str, upstream: &str, endpoint: &str) {
    let (logs, _guard) = capture_logs_at(tracing::Level::DEBUG);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, upstream).await;
    let app = app_for(&mock.base_url);
    assert_eq!(status_of(&app, path, body).await, StatusCode::OK);
    assert_eq!(status_of(&app, path, body).await, StatusCode::TOO_MANY_REQUESTS);
    let text = logs.text();
    let want = format!("endpoint=\"{endpoint}\"");
    for message in ["chat completion metered", "chat completion proxied", "chat completion refused by a limit"] {
        let line = text.lines().find(|l| l.contains(message)).unwrap_or_else(|| panic!("no `{message}` line:\n{text}"));
        assert!(line.contains(&want), "`{message}` must carry {want}: {line}");
    }
}

#[tokio::test]
async fn responses_request_lines_name_the_responses_endpoint() {
    lines_name_the_endpoint("/v1/responses", RESPONSES_BODY, RESPONSES_OK, "responses").await;
}

#[tokio::test]
async fn chat_request_lines_name_the_chat_endpoint() {
    lines_name_the_endpoint("/v1/chat/completions", NON_STREAM_BODY, USAGE_BODY, "chat_completions").await;
}
