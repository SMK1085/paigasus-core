// SPDX-License-Identifier: Apache-2.0
//! Canonical metric-name registry — the single source of truth for instrumentation AND the
//! dashboard/alert name-drift test (Task B4).

// Gateway HTTP
pub const GATEWAY_HTTP_REQUESTS_TOTAL: &str = "gateway_http_requests_total";
pub const GATEWAY_HTTP_REQUEST_DURATION_SECONDS: &str = "gateway_http_request_duration_seconds";
pub const GATEWAY_HTTP_INFLIGHT_REQUESTS: &str = "gateway_http_inflight_requests";
// Gateway dependencies
pub const GATEWAY_IAM_CALLS_TOTAL: &str = "gateway_iam_calls_total";
pub const GATEWAY_IAM_CALL_DURATION_SECONDS: &str = "gateway_iam_call_duration_seconds";
pub const GATEWAY_UPSTREAM_REQUESTS_TOTAL: &str = "gateway_upstream_requests_total";
pub const GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS: &str = "gateway_upstream_request_duration_seconds";
// IAM HTTP
pub const IAM_HTTP_REQUESTS_TOTAL: &str = "iam_http_requests_total";
pub const IAM_HTTP_REQUEST_DURATION_SECONDS: &str = "iam_http_request_duration_seconds";
pub const IAM_HTTP_INFLIGHT_REQUESTS: &str = "iam_http_inflight_requests";
// IAM gRPC
pub const IAM_GRPC_REQUESTS_TOTAL: &str = "iam_grpc_requests_total";
pub const IAM_GRPC_REQUEST_DURATION_SECONDS: &str = "iam_grpc_request_duration_seconds";
// IAM authz / audit
pub const IAM_AUTHZ_DECISIONS_TOTAL: &str = "iam_authz_decisions_total";
pub const IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL: &str = "iam_authz_policy_snapshot_reloads_total";
pub const IAM_AUDIT_RECORDS_TOTAL: &str = "iam_audit_records_total";
pub const IAM_DENIAL_AUDITS_DROPPED_TOTAL: &str = "iam_denial_audits_dropped_total";
pub const IAM_DENIAL_AUDITS_ENQUEUED_TOTAL: &str = "iam_denial_audits_enqueued_total";
// IAM outbox relay
pub const IAM_OUTBOX_RELAY_TICKS_TOTAL: &str = "iam_outbox_relay_ticks_total";
pub const IAM_OUTBOX_RELAY_DRAINED_TOTAL: &str = "iam_outbox_relay_drained_total";
pub const IAM_OUTBOX_RELAY_PUBLISHED_TOTAL: &str = "iam_outbox_relay_published_total";
pub const IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL: &str = "iam_outbox_relay_publish_failures_total";
pub const IAM_OUTBOX_RELAY_PARKED_TOTAL: &str = "iam_outbox_relay_parked_total";
pub const IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS: &str = "iam_outbox_oldest_unpublished_age_seconds";
// IAM audit partition maintenance
pub const IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL: &str = "iam_audit_partition_maintenance_ticks_total";
pub const IAM_AUDIT_PARTITIONS_CREATED_TOTAL: &str = "iam_audit_partitions_created_total";
pub const IAM_AUDIT_PARTITIONS_DROPPED_TOTAL: &str = "iam_audit_partitions_dropped_total";
pub const IAM_AUDIT_DEFAULT_PARTITION_ROWS: &str = "iam_audit_default_partition_rows";

/// Every metric family this workspace emits — the drift test (`tests/drift.rs`) extracts every
/// `iam_`/`gateway_`-prefixed identifier from the committed dashboard/rule `expr`s, strips a
/// trailing `_bucket`/`_sum`/`_count` histogram/summary suffix, and asserts each one is in `ALL`.
/// Label keys, PromQL function/keyword tokens, and template vars never match the prefix filter,
/// so they need no separate allowlist.
pub const ALL: &[&str] = &[
    GATEWAY_HTTP_REQUESTS_TOTAL,
    GATEWAY_HTTP_REQUEST_DURATION_SECONDS,
    GATEWAY_HTTP_INFLIGHT_REQUESTS,
    GATEWAY_IAM_CALLS_TOTAL,
    GATEWAY_IAM_CALL_DURATION_SECONDS,
    GATEWAY_UPSTREAM_REQUESTS_TOTAL,
    GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS,
    IAM_HTTP_REQUESTS_TOTAL,
    IAM_HTTP_REQUEST_DURATION_SECONDS,
    IAM_HTTP_INFLIGHT_REQUESTS,
    IAM_GRPC_REQUESTS_TOTAL,
    IAM_GRPC_REQUEST_DURATION_SECONDS,
    IAM_AUTHZ_DECISIONS_TOTAL,
    IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL,
    IAM_AUDIT_RECORDS_TOTAL,
    IAM_DENIAL_AUDITS_DROPPED_TOTAL,
    IAM_DENIAL_AUDITS_ENQUEUED_TOTAL,
    IAM_OUTBOX_RELAY_TICKS_TOTAL,
    IAM_OUTBOX_RELAY_DRAINED_TOTAL,
    IAM_OUTBOX_RELAY_PUBLISHED_TOTAL,
    IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL,
    IAM_OUTBOX_RELAY_PARKED_TOTAL,
    IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS,
    IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL,
    IAM_AUDIT_PARTITIONS_CREATED_TOTAL,
    IAM_AUDIT_PARTITIONS_DROPPED_TOTAL,
    IAM_AUDIT_DEFAULT_PARTITION_ROWS,
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_names_are_unique_and_snake_case() {
        let mut seen = std::collections::HashSet::new();
        for n in ALL {
            assert!(seen.insert(*n), "duplicate metric name {n}");
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "non-snake_case {n}");
        }
    }
}
