// SPDX-License-Identifier: Apache-2.0

//! A6/A7: without `[limits]`, no limit series exists at all. Its own binary, which never calls
//! `prime_metrics`.

mod support;

use axum::http::StatusCode;
use support::MockOpenAi;
use support::limits::{NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, ScopedIam, app, send};

#[tokio::test]
async fn no_limit_series_exists_without_a_limits_table() {
    let handle = paigasus_observability::init("test-gateway-limits-absent");
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    assert_eq!(send(&app(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), NON_STREAM_BODY).await.status, StatusCode::OK);
    let out = handle.render();
    let limit_lines: Vec<&str> = out
        .lines()
        .filter(|line| {
            ["gateway_limit_", "gateway_tokens_charged_", "gateway_redis_breaker_"]
                .iter()
                .any(|prefix| line.trim_start_matches("# HELP ").trim_start_matches("# TYPE ").starts_with(prefix))
        })
        .collect();
    assert!(limit_lines.is_empty(), "A6: no limit series without [limits]: {limit_lines:?}");
}
