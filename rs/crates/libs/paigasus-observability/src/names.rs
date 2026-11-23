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
// Gateway limits (SMA-677). No label carries a principal or an org id (unbounded).
/// Chat requests refused before egress, one per request; `reason` = `principal_rate` |
/// `org_rate` | `org_budget`, chosen by the D8 precedence.
pub const GATEWAY_LIMIT_REFUSALS_TOTAL: &str = "gateway_limit_refusals_total";
/// Tokens the charge guard sent to the limit store; `source` = `reported` | `estimated`.
pub const GATEWAY_TOKENS_CHARGED_TOTAL: &str = "gateway_tokens_charged_total";
/// Chat requests whose scope PRN names no organization (D2). Expected 0.
pub const GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL: &str = "gateway_limit_unscoped_requests_total";
/// Limit-store calls that failed or met an open breaker; `op` = `check` | `charge`, `kind` = `io` |
/// `server` | `decode`. A `check` here is a request admitted with no limit (fail-open, D10).
pub const GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL: &str = "gateway_limit_store_unavailable_total";
/// Charges that were not sent; `reason` = `no_runtime` | `shutdown` | `period_expired`.
pub const GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL: &str = "gateway_limit_charges_dropped_total";
/// The gateway's limits Redis circuit breaker: 0 closed, 1 half-open, 2 open; `role` = `limits`.
/// Emitted by `paigasus-redis` with this name. Set by every replica: aggregate
/// `max by (job, role)`, never `sum`.
pub const GATEWAY_REDIS_BREAKER_STATE: &str = "gateway_redis_breaker_state";
/// One increment per breaker transition; `role` = `limits`, `to` = `closed` | `half_open` | `open`.
pub const GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL: &str = "gateway_redis_breaker_transitions_total";
// Gateway tool-call interception (SMA-749). No label carries a tool name, an argument, a principal
// or an org id.
/// Upstream SSE records the interceptor did not forward; `reason` = `malformed` | `oversized` |
/// `after_done` | `late_fragment`. A conforming upstream keeps this at 0.
pub const GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL: &str = "gateway_tool_interception_records_dropped_total";
/// One per decided tool call; `decision` = `allow` | `block`, `basis` = `decider` | `on_failure`.
pub const GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL: &str = "gateway_tool_interception_decisions_total";
/// Failures that took the `on_failure` path; `code` = `hold_limit` | `truncated` |
/// `decider_error` | `malformed`.
pub const GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL: &str = "gateway_tool_interception_on_failure_total";
/// Seconds from a hold's first tool-call frame to its completion.
pub const GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS: &str = "gateway_tool_interception_hold_seconds";
/// Tool-policy loads (SMA-752). `result` = `ok` | `source_error` | `compile_error` | `timeout`.
/// A `timeout` is counted once per load, not once per waiter.
pub const GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL: &str = "gateway_tool_policy_cache_loads_total";
/// Seconds one policy source call takes (floor or tenant).
pub const GATEWAY_TOOL_POLICY_LOAD_SECONDS: &str = "gateway_tool_policy_load_seconds";
/// Seconds one policy compile takes (floor or tenant).
pub const GATEWAY_TOOL_POLICY_COMPILE_SECONDS: &str = "gateway_tool_policy_compile_seconds";
/// Cache entries whose last refresh failed. They are served until `max_stale_secs`. Set by each refresh pass.
pub const GATEWAY_TOOL_POLICY_STALE_ENTRIES: &str = "gateway_tool_policy_stale_entries";
/// Tenant entries evicted because nobody used them for `idle_evict_secs`.
pub const GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL: &str = "gateway_tool_policy_evictions_total";
/// Entries whose refresh failed (source error, compile error or timeout).
pub const GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL: &str = "gateway_tool_policy_refresh_errors_total";
/// Tool-policy decisions (SMA-752 § 6.7). `side` = `request` | `response`; `mode` = `off` |
/// `log_only` | `enforce`; `effect` = `allow` | `observe` | `block`; `basis` = `policy` |
/// `errored_forbid` | `unavailable`.
pub const GATEWAY_TOOL_POLICY_DECISIONS_TOTAL: &str = "gateway_tool_policy_decisions_total";
/// Requests that the tool policy refused, and refusals that the mode did not enforce (D17, P-10,
/// SMA-776 G5). `mode` = `off` | `log_only` | `enforce`; `reason` = `unreadable_turn` |
/// `unknown_tool_choice` | `unreplaceable_result`; `refused` = `true` (a 400) | `false`
/// (forwarded; enforce mode would refuse it). The metric never counts an unreadable turn in mode
/// `off`. An unknown `tool_choice` with an enforced floor removal is refused in every mode.
pub const GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL: &str = "gateway_tool_policy_request_refusals_total";
/// Seconds the tool policy adds to one request: the request side plus the decider (preflight F14).
pub const GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS: &str = "gateway_tool_policy_overhead_seconds";
/// Tool-policy cache invalidations from NATS (SMA-752 § 6.3.5). `cause` = `message` (an event on
/// `iam.policy.>`) | `connect` (a connect or a reconnect, because messages may have been missed) |
/// `resubscribe` (a new subscription after the old one ended) | `slow_consumer` (the client
/// dropped messages).
pub const GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL: &str = "gateway_tool_policy_nats_invalidations_total";
/// 1 while the tool-policy NATS client is connected, else 0. Set on each connect and disconnect event.
pub const GATEWAY_TOOL_POLICY_NATS_CONNECTED: &str = "gateway_tool_policy_nats_connected";
/// Tool-policy findings (SMA-755 § 5.9, SMA-753, SMA-855 § 6.11.3). `side` = `request` | `response`; the
/// labels `kind`, `detector` and `severity` come from a closed table of 16 valid triples. A finding
/// outside the table counts as `kind="other", detector="other"` with its severity.
pub const GATEWAY_TOOL_POLICY_FINDINGS_TOTAL: &str = "gateway_tool_policy_findings_total";
/// Seconds one detector stage of the tool-call detector pipeline takes (SMA-855 § 6.11.3).
/// `side` = `request` | `response`; `stage` = `built_in` | `webhook` | `judge`. No label carries a
/// detector name, a host or any content.
pub const GATEWAY_TOOL_POLICY_DETECTOR_SECONDS: &str = "gateway_tool_policy_detector_seconds";
/// Tool-history store calls that failed or ran past the request budget (SMA-753 D13). `op` = `lookup` |
/// `write` | `reserve` | `settle` | `append`; `kind` = `io` | `server` | `decode` | `deadline`. Primed at 0
/// for all twenty pairs.
pub const GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL: &str = "gateway_tool_history_store_errors_total";
/// Call, result or `ref=` ids that a request sent past the cap of 128 per list (SMA-753 D4). `list` =
/// `calls` | `results` | `refs`. Primed at 0 for all three values.
pub const GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL: &str = "gateway_tool_history_ids_capped_total";
/// History events that a write removed (SMA-753 D11). `reason` = `retention` (older than the retention) |
/// `cap` (past 10,000 events for one tool). Primed at 0 for both values.
pub const GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL: &str = "gateway_tool_history_events_trimmed_total";
// Gateway tool decision log (SMA-755 § 5.9). No label carries a tool name, an org or an argument.
/// Records that the decision log accepted from the recorder (counted before the queue).
pub const GATEWAY_DECISION_LOG_RECORDED_TOTAL: &str = "gateway_decision_log_recorded_total";
/// Events that the writer put into the Redis stream.
pub const GATEWAY_DECISION_LOG_WRITTEN_TOTAL: &str = "gateway_decision_log_written_total";
/// Events that the gateway dropped; `reason` = `queue_full` | `closed` | `redis_unavailable` |
/// `write_error` | `breaker_open` | `backlog_full` | `publish_rejected` | `redis_deleted` | `shutdown`.
pub const GATEWAY_DECISION_LOG_DROPPED_TOTAL: &str = "gateway_decision_log_dropped_total";
/// Panics that a decision-log task caught and survived; `task` = `writer` | `relay`.
pub const GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL: &str = "gateway_decision_log_task_restarts_total";
/// Events with a JetStream PubAck (the relay).
pub const GATEWAY_DECISION_LOG_PUBLISHED_TOTAL: &str = "gateway_decision_log_published_total";
/// Losses of entries of the Redis decision stream that the relays detected: a lost stream, or a return to an
/// older state (SMA-821). One per incident over all relays.
pub const GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL: &str = "gateway_decision_log_redis_resets_total";
/// An estimate of the events in the Redis losses. The estimate is the last stored `XLEN` sample. It can undercount
/// (writes after the sample) or overcount (publishes after the sample).
pub const GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL: &str = "gateway_decision_log_lost_estimate_total";
/// Seconds from the publish of an event to its PubAck.
pub const GATEWAY_DECISION_LOG_PUBLISH_SECONDS: &str = "gateway_decision_log_publish_seconds";
/// `XLEN` of the decision stream, sampled every 15 s by the relay.
pub const GATEWAY_DECISION_LOG_BACKLOG: &str = "gateway_decision_log_backlog";
/// Failed Redis reset checks of the relay against the NATS KV bucket `GATEWAY_DECISIONS_GEN`; `kind` = `timeout` |
/// `permission` | `no_bucket` | `redis` | `foreign_writer` | `other` (SMA-821).
pub const GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL: &str = "gateway_decision_log_generation_check_errors_total";
/// Unix time in seconds of the last Redis reset that a relay of this pod detected (SMA-821). The gauge is not primed.
pub const GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS: &str = "gateway_decision_log_last_reset_timestamp_seconds";
// Agent host HTTP
pub const AGENT_HOST_HTTP_REQUESTS_TOTAL: &str = "agent_host_http_requests_total";
pub const AGENT_HOST_HTTP_REQUEST_DURATION_SECONDS: &str = "agent_host_http_request_duration_seconds";
pub const AGENT_HOST_HTTP_INFLIGHT_REQUESTS: &str = "agent_host_http_inflight_requests";
// Agent host inbox. Each series carries the label `channel` (`http` or `nats`).
/// Events stored as new rows by the one insert path.
pub const AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL: &str = "agent_host_inbox_events_inserted_total";
/// Inserts that found a stored row with the same key.
pub const AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL: &str = "agent_host_inbox_events_duplicate_total";
/// Duplicates whose type or data differ from the stored row.
pub const AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL: &str = "agent_host_inbox_events_payload_mismatch_total";
/// Lease losses of the agent host. The label `operation` is `heartbeat`, `claim`, `complete` or `deadline`.
pub const AGENT_HOST_LEASE_LOST_TOTAL: &str = "agent_host_lease_lost_total";
/// Claims that committed with at least one event. Each claim opens one run.
pub const AGENT_HOST_CLAIMS_TOTAL: &str = "agent_host_claims_total";
/// Inbox events that a claim moved to `dead` (spec §8.4.1).
pub const AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL: &str = "agent_host_inbox_events_dead_total";
// Agent host NATS source (spec §9.3).
/// NATS messages ended with `term` because the body does not parse.
pub const AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL: &str = "agent_host_nats_messages_terminated_total";
/// NATS messages acked and skipped because the event does not name the inbox org (spec §9.4).
pub const AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL: &str = "agent_host_nats_messages_tenancy_skipped_total";
/// NATS messages on a subject that no channel maps. Each one is a boot-check gap.
pub const AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL: &str = "agent_host_nats_messages_unmapped_total";
// Agent host timers and scheduler (SMA-788 spec §13).
/// Timer firings. Labels: `kind` (`follow_up` or `schedule`), `origin` (`agent` or `system`), `outcome` (`inserted` or `duplicate`).
pub const AGENT_HOST_TIMER_FIRED_TOTAL: &str = "agent_host_timer_fired_total";
/// Timer rows that a pass could not fire. Label: `class` (`permanent` or `transient`).
pub const AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL: &str = "agent_host_timer_fire_errors_total";
/// Timer rows that a pass set to `failed`. Labels: `kind`, `origin`.
pub const AGENT_HOST_TIMER_FAILED_TOTAL: &str = "agent_host_timer_failed_total";
/// Buffered timer cancels that found the timer not active.
pub const AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL: &str = "agent_host_timer_cancel_not_active_total";
/// The duration of one scheduler pass, in seconds.
pub const AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS: &str = "agent_host_scheduler_pass_duration_seconds";
/// The age of the oldest due active timer after a pass, in seconds. It is 0 when nothing is due.
pub const AGENT_HOST_SCHEDULER_LAG_SECONDS: &str = "agent_host_scheduler_lag_seconds";
/// The Unix time in seconds of the last scheduler pass that finished.
pub const AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS: &str = "agent_host_scheduler_last_pass_timestamp_seconds";
/// Calls of the timer tools. Labels: `tool`, `outcome` (`ok` or `refused`).
pub const AGENT_HOST_TIMER_TOOL_CALLS_TOTAL: &str = "agent_host_timer_tool_calls_total";
// Agent host S3 memory store (SMA-829 spec §6.11). No label holds a path or a key.
/// Objects under a memory scope that a read skipped or refused. Label: `reason` (`key`, `size`,
/// `header` or `unparsable`). It counts events at each read, not distinct objects.
pub const AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL: &str = "agent_host_memory_s3_skipped_objects_total";
/// S3 memory change sets that were partly applied. The store may hold part of the set. No label.
pub const AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL: &str = "agent_host_memory_s3_partial_applies_total";
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
/// SMA-698: a just-in-time provisioning attempt that IAM refused, by `defect`
/// (`missing_email` | `email_conflict`). It counts refused REQUESTS, not identities: one person
/// with a bad token makes several refused requests, because a console session makes several IAM
/// calls. The log rate limit (one `warn` line per issuer and defect in 10 s) does NOT apply to
/// this counter. A lost race between two first logins of one identity is a success and is not
/// counted. Both series are primed at zero when metrics are on
/// (`authenticate_token::prime_jit_provisioning_failures`).
pub const IAM_JIT_PROVISIONING_FAILURES_TOTAL: &str = "iam_jit_provisioning_failures_total";
/// SMA-477: one increment per starter policy per boot, labelled by what reconciliation did —
/// PLUS one `failed` increment per system ROLE reconciliation error (`bootstrap::
/// reconcile_roles`'s own `count("failed")` call). A role's successful outcome
/// (seeded/converged/unchanged) is never counted here, only its failure. `outcome` is a closed
/// set — `unchanged` | `seeded` | `adopted` | `reconciled` | `externally_modified` |
/// `stale_binary` | `orphaned` | `failed` | `operator_collision` — never derived from anything
/// caller-supplied, so it cannot mint cardinality.
///
/// `externally_modified` is the one worth alerting on: it means something other than this
/// service wrote a system-owned policy row, which boot has just reverted. `stale_binary` means
/// an older replica declined to overwrite a newer release's row — expected briefly during a
/// deploy, suspicious if it persists. `orphaned` counts system POLICY rows whose id is no longer
/// code-defined; nothing can delete those automatically — an orphaned system ROLE row is
/// WARN-logged but NOT counted here, an asymmetry with the policy half.
///
/// `operator_collision` means that boot refused to start, because an operator policy uses a
/// starter policy id. It is counted once for each such id. Alert on it: the replica does not start.
///
/// `failed` is the one label the two halves share: a starter-policy AND a system-role
/// reconciliation error both land here under the identical label, distinguishable only via the
/// accompanying ERROR log's `policy_id` vs `role_key` field, never via this metric alone.
pub const IAM_STARTER_POLICY_RECONCILES_TOTAL: &str = "iam_starter_policy_reconciles_total";
/// The tool policy re-compile job (SMA-781): one increment per set result, labeled `result`
/// (`identical`, `held_different`, `held_failed`, `conflict`, `error`). `held_*` means an admin
/// must accept or discard a held version. `error` also covers a per-set storage or decode fault
/// (SMA-781).
pub const IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL: &str = "iam_tool_policy_recompile_results_total";
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
/// `iam_`/`gateway_`/`agent_host_`-prefixed identifier from the committed dashboard/rule `expr`s, strips a
/// trailing `_bucket`/`_sum`/`_count` histogram/summary suffix, and asserts each one is in `ALL`.
/// Label keys, PromQL function/keyword tokens, and template vars never match the prefix filter,
/// so they need no separate allowlist.
pub const ALL: &[&str] = &[
    AGENT_HOST_HTTP_REQUESTS_TOTAL,
    AGENT_HOST_HTTP_REQUEST_DURATION_SECONDS,
    AGENT_HOST_HTTP_INFLIGHT_REQUESTS,
    AGENT_HOST_INBOX_EVENTS_INSERTED_TOTAL,
    AGENT_HOST_INBOX_EVENTS_DUPLICATE_TOTAL,
    AGENT_HOST_INBOX_EVENTS_PAYLOAD_MISMATCH_TOTAL,
    AGENT_HOST_LEASE_LOST_TOTAL,
    AGENT_HOST_CLAIMS_TOTAL,
    AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL,
    AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL,
    AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL,
    AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL,
    AGENT_HOST_TIMER_FIRED_TOTAL,
    AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL,
    AGENT_HOST_TIMER_FAILED_TOTAL,
    AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL,
    AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS,
    AGENT_HOST_SCHEDULER_LAG_SECONDS,
    AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS,
    AGENT_HOST_TIMER_TOOL_CALLS_TOTAL,
    AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL,
    AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL,
    GATEWAY_HTTP_REQUESTS_TOTAL,
    GATEWAY_HTTP_REQUEST_DURATION_SECONDS,
    GATEWAY_HTTP_INFLIGHT_REQUESTS,
    GATEWAY_IAM_CALLS_TOTAL,
    GATEWAY_IAM_CALL_DURATION_SECONDS,
    GATEWAY_UPSTREAM_REQUESTS_TOTAL,
    GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS,
    GATEWAY_LIMIT_REFUSALS_TOTAL,
    GATEWAY_TOKENS_CHARGED_TOTAL,
    GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL,
    GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
    GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL,
    GATEWAY_REDIS_BREAKER_STATE,
    GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
    GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL,
    GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL,
    GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL,
    GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS,
    GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL,
    GATEWAY_TOOL_POLICY_LOAD_SECONDS,
    GATEWAY_TOOL_POLICY_COMPILE_SECONDS,
    GATEWAY_TOOL_POLICY_STALE_ENTRIES,
    GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL,
    GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL,
    GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
    GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL,
    GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS,
    GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL,
    GATEWAY_TOOL_POLICY_NATS_CONNECTED,
    GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
    GATEWAY_TOOL_POLICY_DETECTOR_SECONDS,
    GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL,
    GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL,
    GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL,
    GATEWAY_DECISION_LOG_RECORDED_TOTAL,
    GATEWAY_DECISION_LOG_WRITTEN_TOTAL,
    GATEWAY_DECISION_LOG_DROPPED_TOTAL,
    GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL,
    GATEWAY_DECISION_LOG_PUBLISHED_TOTAL,
    GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL,
    GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL,
    GATEWAY_DECISION_LOG_PUBLISH_SECONDS,
    GATEWAY_DECISION_LOG_BACKLOG,
    GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL,
    GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS,
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
    IAM_JIT_PROVISIONING_FAILURES_TOTAL,
    IAM_STARTER_POLICY_RECONCILES_TOTAL,
    IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL,
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
    fn the_agent_host_timer_families_are_registered() {
        for name in [
            AGENT_HOST_TIMER_FIRED_TOTAL,
            AGENT_HOST_TIMER_FIRE_ERRORS_TOTAL,
            AGENT_HOST_TIMER_FAILED_TOTAL,
            AGENT_HOST_TIMER_CANCEL_NOT_ACTIVE_TOTAL,
            AGENT_HOST_SCHEDULER_PASS_DURATION_SECONDS,
            AGENT_HOST_SCHEDULER_LAG_SECONDS,
            AGENT_HOST_SCHEDULER_LAST_PASS_TIMESTAMP_SECONDS,
            AGENT_HOST_TIMER_TOOL_CALLS_TOTAL,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_agent_host_memory_s3_families_are_registered() {
        assert!(ALL.contains(&AGENT_HOST_MEMORY_S3_SKIPPED_OBJECTS_TOTAL), "the S3 skipped counter is missing from ALL");
        assert!(ALL.contains(&AGENT_HOST_MEMORY_S3_PARTIAL_APPLIES_TOTAL), "the S3 partial counter is missing from ALL");
    }

    #[test]
    fn the_gateway_limit_families_are_registered() {
        for name in [
            GATEWAY_LIMIT_REFUSALS_TOTAL,
            GATEWAY_TOKENS_CHARGED_TOTAL,
            GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL,
            GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
            GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL,
            GATEWAY_REDIS_BREAKER_STATE,
            GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_tool_interception_families_are_registered() {
        for name in [
            GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_tool_policy_cache_families_are_registered() {
        for name in [
            GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL,
            GATEWAY_TOOL_POLICY_LOAD_SECONDS,
            GATEWAY_TOOL_POLICY_COMPILE_SECONDS,
            GATEWAY_TOOL_POLICY_STALE_ENTRIES,
            GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL,
            GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_tool_policy_decision_families_are_registered() {
        for name in [
            GATEWAY_TOOL_POLICY_DECISIONS_TOTAL,
            GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL,
            GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS,
            GATEWAY_TOOL_POLICY_FINDINGS_TOTAL,
            GATEWAY_TOOL_POLICY_DETECTOR_SECONDS,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_tool_history_families_are_registered() {
        for name in [
            GATEWAY_TOOL_HISTORY_STORE_ERRORS_TOTAL,
            GATEWAY_TOOL_HISTORY_IDS_CAPPED_TOTAL,
            GATEWAY_TOOL_HISTORY_EVENTS_TRIMMED_TOTAL,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_decision_log_families_are_registered() {
        for name in [
            GATEWAY_DECISION_LOG_RECORDED_TOTAL,
            GATEWAY_DECISION_LOG_WRITTEN_TOTAL,
            GATEWAY_DECISION_LOG_DROPPED_TOTAL,
            GATEWAY_DECISION_LOG_TASK_RESTARTS_TOTAL,
            GATEWAY_DECISION_LOG_PUBLISHED_TOTAL,
            GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL,
            GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL,
            GATEWAY_DECISION_LOG_PUBLISH_SECONDS,
            GATEWAY_DECISION_LOG_BACKLOG,
            GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL,
            GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_gateway_tool_policy_nats_families_are_registered() {
        for name in [GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, GATEWAY_TOOL_POLICY_NATS_CONNECTED] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_iam_tool_policy_recompile_family_is_registered() {
        assert!(ALL.contains(&IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL), "{IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL} is missing from ALL");
    }

    #[test]
    fn all_names_are_unique_and_snake_case() {
        let mut seen = std::collections::HashSet::new();
        for n in ALL {
            assert!(seen.insert(*n), "duplicate metric name {n}");
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "non-snake_case {n}");
        }
    }
}
