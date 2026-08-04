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
/// SMA-468: a bootstrap-admin seed attempt that failed and was swallowed. `stage="list"` is
/// the pre-seed existence check, `stage="txn"` the grant+audit+event transaction. A lost
/// `policy_gen` bump is NOT counted — `PolicyGenBumper::bump` returns `()` and swallows
/// internally, so it is structurally invisible here. A low nonzero value is not necessarily
/// pathological: two concurrent first authentications by the same admin race, and the loser
/// rolls back on the unique constraint with the net state still correct.
pub const IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL: &str = "iam_bootstrap_admin_seed_failures_total";
/// SMA-477: one increment per starter policy per boot, labelled by what reconciliation did —
/// PLUS one `failed` increment per system ROLE reconciliation error (`bootstrap::
/// reconcile_roles`'s own `count("failed")` call). A role's successful outcome
/// (seeded/converged/unchanged) is never counted here, only its failure. `outcome` is a closed
/// set — `unchanged` | `seeded` | `adopted` | `reconciled` | `externally_modified` |
/// `stale_binary` | `orphaned` | `failed` — never derived from anything caller-supplied, so it
/// cannot mint cardinality.
///
/// `externally_modified` is the one worth alerting on: it means something other than this
/// service wrote a system-owned policy row, which boot has just reverted. `stale_binary` means
/// an older replica declined to overwrite a newer release's row — expected briefly during a
/// deploy, suspicious if it persists. `orphaned` counts system POLICY rows whose id is no longer
/// code-defined; nothing can delete those automatically — an orphaned system ROLE row is
/// WARN-logged but NOT counted here, an asymmetry with the policy half.
///
/// `failed` is the one label the two halves share: a starter-policy AND a system-role
/// reconciliation error both land here under the identical label, distinguishable only via the
/// accompanying ERROR log's `policy_id` vs `role_key` field, never via this metric alone.
pub const IAM_STARTER_POLICY_RECONCILES_TOTAL: &str = "iam_starter_policy_reconciles_total";
// IAM outbox relay
pub const IAM_OUTBOX_RELAY_TICKS_TOTAL: &str = "iam_outbox_relay_ticks_total";
pub const IAM_OUTBOX_RELAY_DRAINED_TOTAL: &str = "iam_outbox_relay_drained_total";
pub const IAM_OUTBOX_RELAY_PUBLISHED_TOTAL: &str = "iam_outbox_relay_published_total";
pub const IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL: &str = "iam_outbox_relay_publish_failures_total";
pub const IAM_OUTBOX_RELAY_PARKED_TOTAL: &str = "iam_outbox_relay_parked_total";
pub const IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS: &str = "iam_outbox_oldest_unpublished_age_seconds";
// IAM outbox retention + dead letters (SMA-469)
pub const IAM_OUTBOX_RETENTION_TICKS_TOTAL: &str = "iam_outbox_retention_ticks_total";
pub const IAM_OUTBOX_ROWS_DELETED_TOTAL: &str = "iam_outbox_rows_deleted_total";
/// Current parked-row count — the dead-letter backlog. Refreshed by every
/// `PgOutboxMaintainer` tick, INCLUDING when `[outbox.retention].enabled = false` (the tick
/// still runs for this gauge), so disabling deletion never blinds the backlog alert.
/// Every replica sets the same global count, so this is PER-REPLICA: aggregate it
/// `max by (job)` in alerts and dashboards, never `sum`.
pub const IAM_OUTBOX_PARKED_ROWS: &str = "iam_outbox_parked_rows";
pub const IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL: &str = "iam_outbox_dead_letters_replayed_total";
pub const IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL: &str = "iam_outbox_dead_letters_discarded_total";
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
    IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,
    IAM_STARTER_POLICY_RECONCILES_TOTAL,
    IAM_OUTBOX_RELAY_TICKS_TOTAL,
    IAM_OUTBOX_RELAY_DRAINED_TOTAL,
    IAM_OUTBOX_RELAY_PUBLISHED_TOTAL,
    IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL,
    IAM_OUTBOX_RELAY_PARKED_TOTAL,
    IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS,
    IAM_OUTBOX_RETENTION_TICKS_TOTAL,
    IAM_OUTBOX_ROWS_DELETED_TOTAL,
    IAM_OUTBOX_PARKED_ROWS,
    IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL,
    IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL,
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
