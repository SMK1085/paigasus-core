// SPDX-License-Identifier: Apache-2.0

//! Egress round-trips for the OpenAI adapter against the in-process mock upstream
//! ([`support::MockOpenAi`]): non-stream body/status passthrough, non-2xx passthrough, ordered
//! UNBUFFERED streaming, and header/secret hygiene (the real key is injected; the caller's headers
//! are never forwarded; the key never appears in a `Debug` render).

mod support;

use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use secrecy::SecretString;
use support::MockOpenAi;

use paigasus_gateway::adapters::openai::{ChatResponse, OpenAiClient};
use paigasus_gateway::config::OpenAiConfig;

const REAL_KEY: &str = "sk-real-secret-server-key-9f8e7d";

/// An `OpenAiClient` pointed at the mock, holding the real key.
fn client_for(mock: &MockOpenAi) -> OpenAiClient {
    let cfg = OpenAiConfig {
        base_url: mock.base_url.clone(),
        api_key: SecretString::from(REAL_KEY.to_string()),
    };
    OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300)).expect("client builds")
}

/// A minimal, valid-looking request body the tests forward upstream.
fn sample_body() -> Bytes {
    Bytes::from(r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#)
}

#[tokio::test]
async fn non_stream_returns_upstream_status_and_body_verbatim() {
    let canned = r#"{"id":"chatcmpl-1","object":"chat.completion","choices":[]}"#;
    let mock = MockOpenAi::spawn_json(axum::http::StatusCode::OK, canned).await;
    let client = client_for(&mock);

    let resp = client.chat_completion(sample_body(), false).await.expect("egress succeeds");

    match resp {
        ChatResponse::Full { status, body } => {
            assert_eq!(status.as_u16(), 200);
            assert_eq!(body, Bytes::from(canned), "the upstream body is forwarded verbatim");
        }
        ChatResponse::Stream { .. } => panic!("expected a Full (non-stream) response"),
    }
}

#[tokio::test]
async fn non_2xx_upstream_is_returned_verbatim_not_an_error() {
    // A non-2xx upstream (e.g. OpenAI's own 429 rate-limit envelope) is NOT an `OpenAiError` — it
    // comes back as `Full` with the upstream status + body so G7 forwards it verbatim.
    let envelope = r#"{"error":{"message":"Rate limit reached","type":"rate_limit_error"}}"#;
    let mock = MockOpenAi::spawn_json(axum::http::StatusCode::TOO_MANY_REQUESTS, envelope).await;
    let client = client_for(&mock);

    let resp = client.chat_completion(sample_body(), false).await.expect("a non-2xx upstream is not a send error");

    match resp {
        ChatResponse::Full { status, body } => {
            assert_eq!(status.as_u16(), 429);
            assert_eq!(body, Bytes::from(envelope));
        }
        ChatResponse::Stream { .. } => panic!("expected a Full response"),
    }
}

#[tokio::test]
async fn stream_yields_chunks_in_order_unbuffered() {
    let events = vec!["first".to_string(), "second".to_string(), "third".to_string(), "[DONE]".to_string()];
    let mock = MockOpenAi::spawn_sse(events.clone()).await;
    let client = client_for(&mock);

    let resp = client.chat_completion(sample_body(), true).await.expect("egress succeeds");

    let mut stream = match resp {
        ChatResponse::Stream { status, stream } => {
            assert_eq!(status.as_u16(), 200);
            stream
        }
        ChatResponse::Full { .. } => panic!("expected a Stream response"),
    };

    // Consume the UNBUFFERED stream chunk by chunk, concatenating as chunks arrive. Concatenation
    // equality proves ORDER independent of how the bytes were framed across chunks.
    let mut assembled = Vec::new();
    let mut chunk_count = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("stream chunk is not an error");
        chunk_count += 1;
        assembled.extend_from_slice(&chunk);
    }

    let expected: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    assert_eq!(
        String::from_utf8(assembled).expect("utf8 body"),
        expected,
        "SSE frames must arrive in the order the upstream emitted them"
    );
    assert!(chunk_count >= 1, "the stream must yield at least one chunk");
}

#[tokio::test]
async fn injects_the_real_key_and_never_forwards_caller_headers() {
    let mock = MockOpenAi::spawn_json(axum::http::StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let body = sample_body();

    let _ = client.chat_completion(body.clone(), false).await.expect("egress succeeds");

    let recorded = mock.recorded().expect("the mock recorded the upstream request");

    // The real key was injected as a FRESH `Authorization: Bearer <key>` header...
    assert_eq!(
        recorded.header("authorization"),
        Some(format!("Bearer {REAL_KEY}").as_str()),
        "the real OpenAI key must be injected as the upstream bearer"
    );
    // ...and the curated content negotiation headers are present and correct.
    assert_eq!(recorded.header("content-type"), Some("application/json"));
    assert_eq!(recorded.header("accept"), Some("application/json"), "the non-stream path advertises JSON");
    // No caller-supplied credential-bearing headers are ever forwarded (the client never even
    // receives the caller's headers — it is handed only the body).
    assert!(recorded.header("cookie").is_none(), "a caller cookie must never reach the upstream");
    // The body flows upstream byte-for-byte (lossless passthrough).
    assert_eq!(recorded.body, body, "the caller's raw body is forwarded verbatim");
}

#[tokio::test]
async fn stream_path_advertises_event_stream_accept() {
    let mock = MockOpenAi::spawn_sse(vec!["x".to_string()]).await;
    let client = client_for(&mock);

    let resp = client.chat_completion(sample_body(), true).await.expect("egress succeeds");
    // Drain the stream so the request completes and the header is recorded.
    if let ChatResponse::Stream { mut stream, .. } = resp {
        while stream.next().await.is_some() {}
    }

    let recorded = mock.recorded().expect("recorded");
    assert_eq!(recorded.header("accept"), Some("text/event-stream"), "the stream path advertises SSE");
    assert_eq!(recorded.header("authorization"), Some(format!("Bearer {REAL_KEY}").as_str()));
}

#[tokio::test]
async fn debug_render_never_contains_the_real_key() {
    let mock = MockOpenAi::spawn_json(axum::http::StatusCode::OK, "{}").await;
    let client = client_for(&mock);
    let rendered = format!("{client:?}");
    assert!(!rendered.contains(REAL_KEY), "the real key must never appear in a Debug render: {rendered}");
}
