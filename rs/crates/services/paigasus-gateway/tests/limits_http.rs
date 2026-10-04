// SPDX-License-Identifier: Apache-2.0

//! HTTP integration for the gateway limits (SMA-677 spec § 5.4): the real router and auth
//! middleware with a fake IAM, the real handler and OpenAI client against the in-process mock
//! upstream, and a memory, recording or failing limit store.

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use futures::StreamExt;
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::UnavailableKind;
use paigasus_logging::test_support::capture_logs_at;
use support::MockOpenAi;
use support::limits::*;
use tower::ServiceExt;

fn memory() -> Arc<MemoryLimitStore> {
    Arc::new(MemoryLimitStore::new())
}

/// A1.
#[tokio::test]
async fn the_request_over_the_principal_rate_is_429_rate_limited() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(2), None, None), memory())));
    for _ in 0..2 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    assert_eq!(refused.json()["error"]["type"], "requests");
    let retry_after: u32 = refused.header("retry-after").parse().expect("an integer Retry-After");
    assert!(retry_after >= 1);
    assert_eq!(refused.header("paigasus-retryable"), "true");
    assert_eq!(refused.header("x-should-retry"), "true");
    assert_eq!(mock.request_count(), 2, "the refused request made no upstream call");
}

/// A2 and D2: the org count is summed over principals, and a project scope counts against its org.
#[tokio::test]
async fn the_org_rate_is_shared_by_every_principal_of_the_org() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let shared = limits(rules(None, Some(2), None), memory());
    let by_org = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(shared.clone()));
    let by_project = app(ScopedIam::arc(PRINCIPAL_2, PROJECT_IN_A), &mock.base_url, Some(shared));
    assert_eq!(send(&by_org, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(send(&by_project, NON_STREAM_BODY).await.status, StatusCode::OK);
    let refused = send(&by_org, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    assert_eq!(mock.request_count(), 2);
}

/// A3.
#[tokio::test]
async fn the_request_after_the_budget_is_used_is_429_budget_exhausted() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1)), memory())));
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    let body = refused.json();
    assert_eq!(body["error"]["code"], "budget-exhausted");
    assert_eq!(body["error"]["type"], "insufficient_quota");
    assert_eq!(body["error"]["message"], "The organization token budget for 2026-10 is used up. It resets at 2026-11-01T00:00:00Z.");
    assert_eq!(refused.header("paigasus-retryable"), "false");
    assert_eq!(refused.header("x-should-retry"), "false");
    assert_eq!(refused.header("retry-after"), "", "no Retry-After on a budget refusal");
    assert_eq!(mock.request_count(), 1);
}

/// Send one non-stream request through a recording store with a large budget; return the status
/// and the charges.
async fn charged(base_url: &str, first_byte: Duration) -> (StatusCode, Vec<u64>) {
    let store = RecordingStore::new();
    let app = app_with(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        base_url,
        Some(limits(rules(None, None, Some(1_000_000)), store.clone())),
        first_byte,
        true,
    );
    let sent = send(&app, NON_STREAM_BODY).await;
    (sent.status, store.charges())
}

/// A4, non-stream rows. The request estimate of `NON_STREAM_BODY` is ceil(2 / 4) = 1.
#[tokio::test]
async fn a_non_stream_answer_with_usage_charges_the_reported_tokens() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::OK, vec![5]));
}

#[tokio::test]
async fn a_non_stream_answer_without_usage_charges_the_estimate() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"abcdefgh"}}]}"#).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::OK, vec![1 + 2]));
}

#[tokio::test]
async fn a_non_stream_timeout_charges_the_request_estimate() {
    let mock = MockOpenAi::spawn_delayed_json(Duration::from_secs(3), StatusCode::OK, USAGE_BODY).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(1)).await, (StatusCode::GATEWAY_TIMEOUT, vec![1]));
}

#[tokio::test]
async fn a_connect_failure_charges_nothing() {
    assert_eq!(charged("http://127.0.0.1:1", Duration::from_secs(30)).await, (StatusCode::BAD_GATEWAY, vec![]));
}

#[tokio::test]
async fn an_upstream_429_charges_nothing() {
    let mock = MockOpenAi::spawn_json(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"quota","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#,
    )
    .await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::TOO_MANY_REQUESTS, vec![]));
}

/// A5, non-stream: the body is byte-identical with and without limits, and a success carries no
/// limit header.
#[tokio::test]
async fn a_non_stream_body_is_byte_identical_with_limits() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let plain = send(&app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), NON_STREAM_BODY).await;
    let limited = send(
        &app(
            ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
            &mock.base_url,
            Some(limits(rules(Some(100), Some(100), Some(1_000_000)), memory())),
        ),
        NON_STREAM_BODY,
    )
    .await;
    assert_eq!((plain.status, &plain.body), (limited.status, &limited.body));
    assert_eq!(limited.header("x-should-retry"), "");
    assert_eq!(limited.header("retry-after"), "");
}

/// A12: a failing store admits, reaches the upstream, never charges, and logs the fail-open spend.
#[tokio::test]
async fn a_failing_store_admits_and_logs_store_unavailable() {
    let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = FailingStore::new(UnavailableKind::Server);
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(1), Some(1), Some(1)), store.clone())));
    for _ in 0..2 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK, "fail-open: no limit applies");
    }
    assert_eq!(mock.request_count(), 2);
    assert_eq!(store.charges(), 0, "no ticket, no charge");
    let text = logs.text();
    let line = text.lines().find(|l| l.contains("chat completion metered")).expect("the metered line");
    assert!(line.contains("outcome=\"store_unavailable\"") && line.contains("tokens=5"), "{line}");
}

/// D11: an exempt org with no principal rate makes no store call.
#[tokio::test]
async fn an_exempt_org_makes_no_store_call() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = RecordingStore::new();
    let app = app(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules_with_exempt(rules(None, Some(1), Some(1)), ORG_A), store.clone())),
    );
    for _ in 0..3 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    assert_eq!((store.checks(), store.charges()), (0, vec![]));
}

/// D9: a refused body and a refused stream do not use quota.
#[tokio::test]
async fn a_refused_body_or_stream_does_not_use_quota() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_with(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(Some(1), None, None), memory())),
        Duration::from_secs(30),
        false,
    );
    assert_eq!(send(&app, "{not json").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, STREAM_BODY).await.json()["error"]["code"], "streaming-disabled");
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK, "the one slot is still free");
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
}

/// D2/Q11: a scope with no org and a budget configured is `500 internal`, with no upstream call.
#[tokio::test]
async fn an_unscoped_request_with_a_budget_is_500_internal() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::arc(PRINCIPAL_1, UNSCOPED), &mock.base_url, Some(limits(rules(None, None, Some(10)), memory())));
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(refused.json()["error"]["code"], "internal");
    assert_eq!(mock.request_count(), 0);
}

/// Send `STREAM_BODY` through a recording store with a large budget; read the whole stream.
async fn stream_charged(base_url: &str) -> (Sent, Vec<u64>) {
    let store = RecordingStore::new();
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let sent = send(&app, STREAM_BODY).await;
    (sent, store.charges())
}

/// A4: a stream with a usage record charges the reported total.
#[tokio::test]
async fn a_stream_with_a_usage_record_charges_the_reported_tokens() {
    let mock = MockOpenAi::spawn_raw_sse(vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7), "data: [DONE]\n\n".to_owned()]).await;
    let (sent, charges) = stream_charged(&mock.base_url).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(charges, vec![7]);
}

/// A4/D14: no usage gives the request estimate (1) plus one token per record, with and without
/// `"usage":null`.
#[tokio::test]
async fn a_stream_without_usage_charges_one_token_per_record() {
    let mock = MockOpenAi::spawn_raw_sse(vec![chunk("a", false), chunk("b", true), chunk("c", true), "data: [DONE]\n\n".to_owned()]).await;
    assert_eq!(stream_charged(&mock.base_url).await.1, vec![1 + 3]);
}

/// Section 4.6: CRLF record delimiters (vLLM, LiteLLM; SMA-558).
#[tokio::test]
async fn crlf_records_are_counted_and_a_crlf_usage_record_is_read() {
    let crlf = |record: String| record.replace("\n\n", "\r\n\r\n");
    let estimated = MockOpenAi::spawn_raw_sse(vec![crlf(chunk("a", true)), crlf(chunk("b", true)), "data: [DONE]\r\n\r\n".to_owned()]).await;
    assert_eq!(stream_charged(&estimated.base_url).await.1, vec![1 + 2]);
    let reported = MockOpenAi::spawn_raw_sse(vec![crlf(chunk("a", true)), crlf(usage_chunk(9)), "data: [DONE]\r\n\r\n".to_owned()]).await;
    assert_eq!(stream_charged(&reported.base_url).await.1, vec![9]);
}

/// A4: a mid-stream upstream error charges exactly once (two records forwarded plus the estimate).
#[tokio::test]
async fn a_mid_stream_error_charges_exactly_once() {
    let base_url = support::spawn_truncated_sse().await;
    let (sent, charges) = stream_charged(&base_url).await;
    assert!(sent.text().contains("\"code\":\"upstream-error\""), "{}", sent.text());
    assert_eq!(charges, vec![1 + 2]);
}

/// A4: a client that disconnects mid-stream is charged the estimate, once, when the body drops.
#[tokio::test]
async fn a_client_disconnect_mid_stream_charges_the_estimate_once() {
    let (mock, _cancelled) = MockOpenAi::spawn_abortable_stream().await;
    let store = RecordingStore::new();
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let resp = app.oneshot(post(STREAM_BODY)).await.expect("the router answers");
    let mut body = resp.into_body().into_data_stream();
    let first = body.next().await.expect("a first frame").expect("not a transport error");
    assert!(first.starts_with(b"data: hold"), "{first:?}");
    assert!(store.charges().is_empty(), "nothing is charged while the stream is open");
    drop(body);
    assert_eq!(store.charges(), vec![1 + 1], "the request estimate plus the one forwarded record");
}

/// A5, stream: the SSE bytes are byte-identical with and without limits.
#[tokio::test]
async fn the_sse_bytes_are_byte_identical_with_limits() {
    let records = vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7), "data: [DONE]\n\n".to_owned()];
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let plain = send(&app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), STREAM_BODY).await;
    let limited = send(
        &app(
            ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
            &mock.base_url,
            Some(limits(rules(Some(100), Some(100), Some(1_000_000)), memory())),
        ),
        STREAM_BODY,
    )
    .await;
    assert_eq!(plain.body, limited.body);
    assert_eq!(limited.text(), records.concat());
}

/// A4: a client disconnect before the upstream answers drops the handler future; the guard
/// charges the request estimate, once. No sleep race: wait for the upstream request, drop, then
/// poll the store with a bound.
#[tokio::test]
async fn a_non_stream_client_disconnect_charges_the_request_estimate() {
    let mock = MockOpenAi::spawn_delayed_json(Duration::from_secs(30), StatusCode::OK, USAGE_BODY).await;
    let store = RecordingStore::new();
    let app = app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let request = send(&app, NON_STREAM_BODY);
    let upstream_reached = async {
        while mock.request_count() < 1 {
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        _ = request => panic!("the delayed upstream must not answer"),
        _ = tokio::time::timeout(Duration::from_secs(10), upstream_reached) => {}
    }
    // The select dropped the request future: that is the client disconnect.
    assert_eq!(mock.request_count(), 1, "the upstream request was made");
    for _ in 0..200 {
        if !store.charges().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(store.charges(), vec![1], "exactly the request estimate, one charge");
}
