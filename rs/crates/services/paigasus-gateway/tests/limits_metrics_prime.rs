// SPDX-License-Identifier: Apache-2.0

//! A7: every limit series reads 0 right after boot. Its own binary: the Prometheus recorder is
//! process-global, and no other test here may move a value first.

mod support;

use paigasus_gateway::adapters::limits::build_limits;
use paigasus_gateway::config::LimitsConfig;
use support::limits::sample;

#[tokio::test]
async fn every_limit_series_reads_zero_after_boot() {
    let handle = paigasus_observability::init("test-gateway-limits-prime");
    let config = LimitsConfig {
        principal_requests_per_minute: Some(5),
        ..LimitsConfig::default()
    };
    build_limits(Some(&config)).await.expect("the memory backend boots");
    let out = handle.render();
    let mut series: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for reason in ["principal_rate", "org_rate", "org_budget"] {
        series.push(("gateway_limit_refusals_total", vec![("reason", reason)]));
    }
    for source in ["reported", "estimated"] {
        series.push(("gateway_tokens_charged_total", vec![("source", source)]));
    }
    series.push(("gateway_limit_unscoped_requests_total", vec![]));
    for op in ["check", "charge"] {
        for kind in ["io", "server", "decode"] {
            series.push(("gateway_limit_store_unavailable_total", vec![("op", op), ("kind", kind)]));
        }
    }
    for reason in ["no_runtime", "shutdown", "period_expired"] {
        series.push(("gateway_limit_charges_dropped_total", vec![("reason", reason)]));
    }
    for (name, labels) in &series {
        assert_eq!(sample(&out, name, labels), Some(0.0), "{name}{labels:?} must read 0 after boot:\n{out}");
    }
    assert_eq!(sample(&out, "gateway_redis_breaker_state", &[("role", "limits")]), None, "no breaker series for the memory backend");
}
