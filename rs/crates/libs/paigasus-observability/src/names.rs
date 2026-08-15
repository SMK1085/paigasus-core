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
/// Rewinds of a Redis authz generation counter (`iam:authz:policy_gen`/`iam:authz:entity_gen`),
/// by `counter`, `outcome` (`repaired`/`repair_failed`/`ceiling`) and `reason`
/// (`missing`/`lower`). Non-zero means a counter was observed to have **rewound** — read back
/// below what the process had already seen. `reason` says which kind: `missing` is the key
/// itself gone (most often `allkeys-*` eviction, which the RUNBOOK's `maxmemory-policy` mandate
/// exists to prevent), while `lower` is a key that still exists at a smaller value, e.g. a
/// failover to a replica carrying stale data. Only `missing` implies key loss (SMA-474).
pub const IAM_AUTHZ_GENERATION_REWINDS_TOTAL: &str = "iam_authz_generation_rewinds_total";
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
// IAM Redis circuit breaker (SMA-476)
/// The circuit-breaker state for one Redis connection: `0` = closed (commands pass through),
/// `1` = half_open (one probe admitted), `2` = open (every command short-circuits instantly).
///
/// `role` is a CLOSED set — `authz` | `api_keys` | `jwks` — derived from a Rust enum, never from
/// anything caller-supplied, so it cannot mint cardinality.
///
/// Three attribution caveats, all consequences of how `AppState::new` shares connections rather
/// than of the breaker itself:
/// - `role="api_keys"` exists ONLY when the API-key cache holds its own connection: either
///   `authz.cache.backend = "memory"` while `api_keys.introspect_cache.backend = "redis"`, or
///   both are redis-backed with `redis_url`s that differ textually after trimming (SMA-485 D1).
///   Otherwise the API-key cache reuses the authz connection and its commands are attributed to
///   `role="authz"` — a missing `api_keys` series does NOT mean the API-key cache is idle.
///   Conversely, because the comparison is textual, two spellings of ONE endpoint produce an
///   `api_keys` series fronting the same physical Redis — see the next caveat.
/// - Two roles may front the SAME physical Redis with independent breakers, so `authz` at 0 while
///   `jwks` is at 2 does not imply two backends.
/// - Set independently by every replica — aggregate `max by (job, role)`, never `sum`.
pub const IAM_REDIS_BREAKER_STATE: &str = "iam_redis_breaker_state";
/// One increment per circuit-breaker state transition; `to` = `closed` | `half_open` | `open`.
///
/// NOT redundant with [`IAM_REDIS_BREAKER_STATE`]. The open window is 2 s while scrapes are
/// 15–30 s apart, so a breaker that opens and re-closes between two scrapes is invisible to the
/// gauge — `changes()` over it undercounts by construction. A chronically sick backend that flaps
/// is exactly the condition worth catching early, and this counter is the only artifact that
/// survives a sub-scrape-interval state.
pub const IAM_REDIS_BREAKER_TRANSITIONS_TOTAL: &str = "iam_redis_breaker_transitions_total";
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
/// Labelled `scope` = `one` | `bulk` (which `DeadLetterService` call replayed the row(s)) and
/// `beyond_dedup_window` = `true` | `false` | `unknown` (SMA-471 D4) — a CLOSED set, never
/// derived from anything caller-supplied, so it cannot mint cardinality.
///
/// `beyond_dedup_window` exists because `REPLAY_ONE_SQL` un-parks a row by its EXISTING id,
/// which `NatsEventPublisher` sends as `Nats-Msg-Id`, and JetStream only deduplicates within
/// its `duplicate_window_secs` of the row's first publish. `scope="one"` computes the label
/// from the replayed row's own `parked_at` against a constant mirroring the shipped
/// `duplicate_window_secs` default (not plumbed config — see `ASSUMED_DEDUP_WINDOW_SECS`'s doc
/// in `dead_letters.rs`): `true` means the row parked longer ago than that window, so the
/// replay may republish an event the stream already holds; `false` means it parked recently
/// enough that JetStream's dedup almost certainly still covers it. `scope="bulk"` is always
/// `unknown` — `replay_matching_in` returns only a row COUNT, never the rows, so no per-row
/// `parked_at` exists to compare.
pub const IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL: &str = "iam_outbox_dead_letters_replayed_total";
pub const IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL: &str = "iam_outbox_dead_letters_discarded_total";
/// Relay ticks, labeled by what woke them: `notify` (a Postgres `LISTEN` notification),
/// `poll` (the `poll_interval_secs` timer) or `backlog` (SMA-489 D9's continuation after a
/// full batch that made progress).
///
/// **One increment per TICK, not per wakeup** — so
/// `sum without (source) (iam_outbox_relay_wakeups_total)` equals
/// `sum without (result) (iam_outbox_relay_ticks_total)`, an invariant the integration tests
/// assert. All three label values are primed at zero when the relay starts: a metrics-rs series
/// first appears already at 1, so an `increase()` rule could otherwise never fire on the first
/// occurrence of a label value.
pub const IAM_OUTBOX_RELAY_WAKEUPS_TOTAL: &str = "iam_outbox_relay_wakeups_total";
/// End-to-end outbox latency: `now - occurred_at` at the moment a row is successfully
/// published. **This is the only signal that proves the SMA-489 nudge is working in
/// production.** [`IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS`] cannot: it is reset to 0 on
/// every empty tick, and the nudge makes empty ticks far more frequent.
pub const IAM_OUTBOX_PUBLISH_LAG_SECONDS: &str = "iam_outbox_publish_lag_seconds";
/// Notifications the `PgOutboxListener` actually received. Distinguishes "Postgres never
/// notified us — e.g. a transaction-mode pooler silently swallowed `LISTEN`" from "the relay
/// never observed the permit", which `iam_outbox_relay_wakeups_total{source="notify"}` alone
/// cannot (SMA-489 §1.5).
pub const IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL: &str = "iam_outbox_listener_notifications_total";
/// Enqueues that emitted a `pg_notify` — the write-side twin of
/// [`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL`], and the control term
/// `IamOutboxNotificationsAbsent` gates on (SMA-495). It answers "was a nudge emitted at all in
/// this window", which `IAM_OUTBOX_RELAY_DRAINED_TOTAL` only ever approximated: a drain counts
/// every row the relay processes, including SMA-469 dead-letter replays, whose `REPLAY_ONE_SQL`
/// un-parks a row with a direct `UPDATE` and emits NO notification (SMA-489 D2). A replay during a
/// quiet period therefore used to satisfy that alert with a perfectly healthy listener.
///
/// **NOT 1:1 with [`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL`] — do not build a ratio from the
/// pair.** Postgres collapses notifications carrying an identical channel AND payload within one
/// transaction, and this payload is always empty (SMA-489 D3), so a transaction enqueuing N events
/// increments this counter N times while delivering exactly ONE notification. The alert is
/// unaffected: it asks only `> 0` of this counter and `== 0` of the listener's, never a rate
/// comparison.
///
/// **Counted pre-commit.** The outbox writes on a transaction it RECOVERS rather than owns, so
/// there is no post-commit hook to count from; this counts *attempted* notifying enqueues and can
/// only ever over-count delivered notifications, never under-count. A rolled-back mutation
/// increments it while delivering no notification and draining no row —
/// `IamOutboxNotificationsAbsent` absorbs that through its separate `drained` term, which is why
/// that term is retained rather than replaced.
///
/// Primed at zero in `main.rs` iff `[outbox].wake_on_commit = true`, so the series means "this
/// replica is configured to nudge" and an `increase()` control can fire on the very first
/// enqueue. `[outbox].relay_enabled = false` does NOT gate it: that deployment emits and primes
/// this counter while running no relay and no listener, and the alert stays silent there anyway
/// because the listener series is absent.
pub const IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL: &str = "iam_outbox_notifying_enqueues_total";
/// 1 when the outbox listener holds a live `LISTEN` connection, 0 otherwise.
///
/// **Per-replica, and the replicas do NOT agree** — the same caveat [`IAM_NATS_CONNECTED`]
/// carries. `max by (job)` returns 1 while any single replica is still connected, hiding
/// exactly the partial outage worth knowing about. Use `min by (job)` to ask "are all replicas
/// listening", or keep `instance` to see which one is down. Never `sum`.
pub const IAM_OUTBOX_LISTENER_CONNECTED: &str = "iam_outbox_listener_connected";
/// Successful re-establishments of the outbox listener's `LISTEN` connection. `PgOutboxListener`
/// increments this at a single site in its reconnect path, so it counts identically however the
/// loss surfaced — `try_recv() -> Ok(None)` and an `Err` both break to that one path (SMA-489).
///
/// **This counts recoveries, not failures**, which inverts the usual reading of a `_total`. A
/// listener down through a long outage increments this ONCE, on recovery — never once per failed
/// attempt — so a value that stops climbing mid-incident means "still down", and only
/// [`IAM_OUTBOX_LISTENER_CONNECTED`] separates that from "healthy and quiet". The alertable shape
/// is therefore a steadily CLIMBING value: it means Postgres is churning the listener connection,
/// and every cycle is a window in which notifications were dropped and delivery fell back to the
/// poll.
pub const IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL: &str = "iam_outbox_listener_reconnects_total";
// IAM audit partition maintenance
pub const IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL: &str = "iam_audit_partition_maintenance_ticks_total";
pub const IAM_AUDIT_PARTITIONS_CREATED_TOTAL: &str = "iam_audit_partitions_created_total";
pub const IAM_AUDIT_PARTITIONS_DROPPED_TOTAL: &str = "iam_audit_partitions_dropped_total";
pub const IAM_AUDIT_DEFAULT_PARTITION_ROWS: &str = "iam_audit_default_partition_rows";
// IAM system row retirement (SMA-481)
/// Labelled `outcome` = `refused` | `blocked` | `retired` — but this is NOT one increment per
/// `SystemRetirementService::retire` call. Four outcomes return without touching this counter
/// at all: `Forbidden` (the Root-only check), `SystemImmutable` (still code-defined),
/// `NotFound`, and `NotSystemOwned` — none of them are the fleet-skew/decision-change/
/// blast-radius concerns this metric exists to page on, so they are not instrumented here.
/// `refused` fires for `FleetNotConverged` (before any transaction opens) AND for an
/// unacknowledged static-policy retirement (`NeedsAcknowledgement` — this one DOES open a
/// transaction and lock the policy row before refusing; `lock_role_in` still runs
/// unconditionally, but a static policy has no `role` row to lock, so only the policy row is
/// actually held — "refused" describes the outcome, not "never opened a transaction"). `blocked`
/// fires when surviving grants stop a retirement after a transaction opened. `retired` fires
/// once the deletes, event and audit entry all committed.
/// Retirement is a destructive, Root-only, operator-triggered action, and unlike a routine
/// reconciliation drift, nothing else alerts on it: the `audit_log` row this call also writes is
/// durable evidence, but durable is not the same as monitored — nothing polls `audit_log` for
/// this action today, so this counter is the only thing that can page anyone on it.
pub const IAM_SYSTEM_ROWS_RETIRED_TOTAL: &str = "iam_system_rows_retired_total";
// IAM NATS publisher (SMA-471)
/// Acks returned with `duplicate = true` — JetStream collapsing a relay redelivery. A rising
/// rate means publish acks are being lost and the relay is retrying. Primed at zero by
/// `NatsEventPublisher::connect` so the FIRST duplicate can satisfy an `increase() > 0` alert.
pub const IAM_NATS_PUBLISH_DUPLICATES_TOTAL: &str = "iam_nats_publish_duplicates_total";
/// Ack round-trip latency. On the critical path of a lock-holding relay transaction, so this is
/// a database-health metric as much as a broker one.
pub const IAM_NATS_PUBLISH_DURATION_SECONDS: &str = "iam_nats_publish_duration_seconds";
/// 1 when the client reports a live connection, 0 otherwise. Sampled by a BACKGROUND task, not
/// set inside `publish`: during a total outage every row eventually parks, `publish` stops being
/// called, and a publish-driven gauge would freeze exactly when it matters.
///
/// **Per-replica, and unlike [`IAM_OUTBOX_PARKED_ROWS`] the replicas do NOT agree.** That gauge
/// reports one global fact every replica computes identically, so `max by (job)` is right there.
/// This one reports each replica's own connection state, so `max by (job)` returns 1 while any
/// single replica is still connected — hiding exactly the partial outage worth paging on
/// (CodeRabbit, PR 112). Keep `instance` to see which replica is down, or use `min by (job)` to
/// ask "are all replicas connected". Never `sum`.
pub const IAM_NATS_CONNECTED: &str = "iam_nats_connected";

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
    IAM_AUTHZ_GENERATION_REWINDS_TOTAL,
    IAM_REDIS_BREAKER_STATE,
    IAM_REDIS_BREAKER_TRANSITIONS_TOTAL,
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
    IAM_OUTBOX_RELAY_WAKEUPS_TOTAL,
    IAM_OUTBOX_PUBLISH_LAG_SECONDS,
    IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL,
    IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL,
    IAM_OUTBOX_LISTENER_CONNECTED,
    IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL,
    IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL,
    IAM_AUDIT_PARTITIONS_CREATED_TOTAL,
    IAM_AUDIT_PARTITIONS_DROPPED_TOTAL,
    IAM_AUDIT_DEFAULT_PARTITION_ROWS,
    IAM_SYSTEM_ROWS_RETIRED_TOTAL,
    IAM_NATS_PUBLISH_DUPLICATES_TOTAL,
    IAM_NATS_PUBLISH_DURATION_SECONDS,
    IAM_NATS_CONNECTED,
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
