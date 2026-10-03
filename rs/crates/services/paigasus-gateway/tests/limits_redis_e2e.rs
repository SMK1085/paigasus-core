// SPDX-License-Identifier: Apache-2.0

//! A11 end to end (SMA-677 spec § 5.4): two routers, each with its own `RedisLimitStore` on one
//! Redis, model two replicas. They share one org count, and a new replica (a restart) still sees it.

mod support;

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use paigasus_gateway::adapters::limits::RedisLimitStore;
use support::MockOpenAi;
use support::limits::{NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, PRINCIPAL_2, ScopedIam, app, limits, rules, send};

/// One replica: its own `RedisLimitStore` (its own connection and breaker) on the shared Redis.
async fn replica(redis_url: &str, upstream: &str, principal: &str) -> Router {
    let store = RedisLimitStore::connect(redis_url).await.expect("connect to the test Redis");
    app(ScopedIam::arc(principal, ORG_A_PRN), upstream, Some(limits(rules(None, Some(3), None), Arc::new(store))))
}

#[tokio::test]
async fn replicas_on_one_redis_share_the_org_limit() {
    let Some((_node, url)) = paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", "limits_redis_e2e").await else {
        return;
    };
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let a = replica(&url, &mock.base_url, PRINCIPAL_1).await;
    let b = replica(&url, &mock.base_url, PRINCIPAL_2).await;
    for app in [&a, &b, &a] {
        assert_eq!(send(app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    let refused = send(&b, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    // A restarted replica keeps the count: it lives in Redis, not in the process.
    let restarted = replica(&url, &mock.base_url, PRINCIPAL_1).await;
    assert_eq!(send(&restarted, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(mock.request_count(), 3);
}
