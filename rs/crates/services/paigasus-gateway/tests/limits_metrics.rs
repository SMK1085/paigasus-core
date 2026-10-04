// SPDX-License-Identifier: Apache-2.0

//! A7: the limit counters move by the right NUMBER. The recorder is process-global, so each test
//! reads a delta (after - before) of one series, parsed from the Prometheus text.

mod support;

use axum::http::StatusCode;
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::UnavailableKind;
use std::sync::Arc;
use support::MockOpenAi;
use support::limits::{FailingStore, NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, ScopedIam, UNSCOPED, USAGE_BODY, app, limits, rules, sample, send};

fn read(name: &str, labels: &[(&str, &str)]) -> f64 {
    sample(&paigasus_observability::init("test-gateway-limits-metrics").render(), name, labels).unwrap_or(0.0)
}

#[tokio::test]
async fn a_budget_refusal_adds_one_org_budget_refusal() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(None, None, Some(1)), Arc::new(MemoryLimitStore::new()))),
    );
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    let before = read("gateway_limit_refusals_total", &[("reason", "org_budget")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(read("gateway_limit_refusals_total", &[("reason", "org_budget")]) - before, 1.0);
}

#[tokio::test]
async fn a_reported_charge_adds_its_tokens() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(None, None, Some(1_000)), Arc::new(MemoryLimitStore::new()))),
    );
    let before = read("gateway_tokens_charged_total", &[("source", "reported")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(read("gateway_tokens_charged_total", &[("source", "reported")]) - before, 5.0);
}

#[tokio::test]
async fn a_fail_open_admission_adds_one_check_failure_of_its_kind() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(Some(1), None, None), FailingStore::new(UnavailableKind::Server))),
    );
    let before = read("gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(read("gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]) - before, 1.0);
}

#[tokio::test]
async fn an_unscoped_request_adds_one_unscoped_request() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(
        ScopedIam::arc(PRINCIPAL_1, UNSCOPED),
        &mock.base_url,
        Some(limits(rules(None, None, Some(10)), Arc::new(MemoryLimitStore::new()))),
    );
    let before = read("gateway_limit_unscoped_requests_total", &[]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(read("gateway_limit_unscoped_requests_total", &[]) - before, 1.0);
}
