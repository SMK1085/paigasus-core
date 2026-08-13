# Observability RUNBOOK — `paigasus-iam` / `paigasus-gateway`

Operator-facing reference for the Prometheus metrics, Grafana dashboards, and alert rules
that make the SMA-446 backbone (IAM audit log + transactional outbox + AI Gateway M0) observable.
For the underlying design decisions, see:

- `docs/superpowers/specs/2026-07-13-sma-446-observability-metrics-dashboards-design.md` (this
  deliverable's design — metric catalog §5, endpoint/security §6, alert rules §7.2).
- `docs/superpowers/specs/2026-07-12-sma-446-m5-audit-log-outbox-design.md` (audit log +
  transactional outbox architecture).
- `docs/superpowers/specs/2026-07-13-sma-446-gateway-m0-iam-auth-design.md` (AI Gateway M0
  walking skeleton + IAM-backed auth).

---

## 1. Overview

Both services expose Prometheus metrics via a **pull** model: each process installs a global
`metrics`-facade recorder at startup (`paigasus_observability::init`) and serves the accumulated
counters/gauges/histograms as a `GET /metrics` endpoint in Prometheus text exposition format
(`text/plain; version=0.0.4`). There is no push gateway and no OpenTelemetry export — metrics are
scrape-only (see §6 for the future OTel option).

| service | default `/metrics` location | notes |
|---|---|---|
| `paigasus-iam` | `http://0.0.0.0:8080/metrics` | merged onto the same HTTP router as the tenancy/authn/authz API (`http_addr`); gRPC (`9090`) is unaffected — there is no `/metrics` over gRPC |
| `paigasus-gateway` | `http://0.0.0.0:8088/metrics` | merged onto the same HTTP router as `/v1/chat/completions` (`http_addr`) |

Both services accept a `[metrics]` config table:

```toml
[metrics]
enabled = true        # install the recorder + expose GET /metrics (default true)
# addr  = "127.0.0.1:9091"   # optional: serve /metrics on its OWN internal listener instead of
                             # the main HTTP port.
```

- `enabled = false` skips installing the recorder entirely — no `/metrics` route is mounted, and
  every `counter!`/`gauge!`/`histogram!` call in the process becomes a cheap no-op against the
  `metrics` crate's default no-op recorder.
- `addr` unset (default): `/metrics` is merged onto the service's main port (`http_addr`) —
  `8080` for IAM, `8088` for the gateway.
- `addr` set to a `SocketAddr`: `/metrics` is served on a **second, separate listener** bound to
  that address instead, on its own graceful-shutdown-aware `axum::serve` task, and is **not**
  mounted on the main router. `validate()` in both `IamConfig` and `GatewayConfig` rejects an
  `addr` equal to `http_addr` (a same-address collision is a config error, not a silent merge).
- Env override: `GATEWAY_METRICS__ENABLED` / `GATEWAY_METRICS__ADDR`, `IAM_METRICS__ENABLED` /
  `IAM_METRICS__ADDR` (figment `__`-nesting), or the `[metrics]` table in `gateway.toml` /
  `iam.toml`.

**IAM's `TraceLayer`** (structured per-request JSON logs) excludes `/metrics` and the health
routes, so a Prometheus scrape (default `scrape_interval: 15s`) does not spam the request-span
logs.

**Security posture (D4) — read this before exposing either service beyond localhost.** `/metrics`
is served **unauthenticated** by design (standard Prometheus scrape convention; the exposition
carries only bounded-cardinality counters/gauges, never secrets, tokens, or PRNs). Exposure is
controlled by **where the endpoint binds**, not by auth — see §4's gateway-posture and
authz-availability entries for what that means operationally for each service.

---

## 2. Metric catalog

Every name below is a `const` in `rs/crates/libs/paigasus-observability/src/names.rs`
(`observability::names`) — the single source of truth. Dashboards and alert rules are checked
against this registry by an automated name-drift test (§5), but **this table is prose and is not
covered by that test** — names here were copy-pasted from `names.rs` and cross-checked
individually.

Histogram buckets (all `*_seconds` histograms): `[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1,
2.5, 5, 10]` seconds. No `service` label is baked onto any metric — the Prometheus scrape `job`
label (`iam` / `gateway`) distinguishes the two.

### 2.1 Shared HTTP (both services, via `http_metrics_layer`)

| metric | type | labels | meaning / expected range |
|---|---|---|---|
| `iam_http_requests_total` | counter | `route`, `method`, `status_class` | HTTP request count on IAM's HTTP router. `route` is the axum `MatchedPath` template (bounded — never a raw path); `status_class` ∈ `2xx`/`4xx`/`5xx`. |
| `iam_http_request_duration_seconds` | histogram | `route`, `method` | HTTP request latency (full request-response cycle). |
| `iam_http_inflight_requests` | gauge | — | Requests currently being handled on IAM's HTTP router. Expected to track request concurrency; should return to a low baseline between bursts. |
| `gateway_http_requests_total` | counter | `route`, `method`, `status_class` | Same shape, gateway HTTP router (`/healthz`, `/readyz`, `/v1/chat/completions`, …). |
| `gateway_http_request_duration_seconds` | histogram | `route`, `method` | Gateway HTTP latency. **Streaming caveat:** for `stream: true` chat completions this measures **time to the handler returning** (i.e. time-to-first-byte of the SSE stream), not the full stream duration — the SSE body streams lazily after the handler returns (§2.3). Don't read gateway p99 latency panels as end-to-end stream time. |
| `gateway_http_inflight_requests` | gauge | — | Requests currently in flight on the gateway HTTP router. |

`/metrics` and `/healthz`/`/readyz` are excluded from `http_metrics_layer` (or collapse to their
own bounded `route` template) so scrape/health traffic doesn't dominate the RED metrics.

### 2.2 `paigasus-iam` — gRPC, authz, audit, outbox relay, NATS publisher

| metric | type | labels | meaning / expected range |
|---|---|---|---|
| `iam_grpc_requests_total` | counter | `service`, `method`, `grpc_status` | One increment per completed tonic handler call. `service`/`method` are compile-time string literals (e.g. `service="Authorization"`, `method="IsAuthorized"`) — never derived from the request path, so cardinality is bounded to the known RPC set (Tenancy / Authentication / Authorization / ServiceAccount / Audit). `grpc_status` is `"ok"` or the canonical tonic status-code name (`permission_denied`, `unavailable`, `invalid_argument`, …). |
| `iam_grpc_request_duration_seconds` | histogram | `service`, `method` | gRPC handler latency, recorded at the same handler-boundary call site as the counter above. |
| `iam_authz_decisions_total` | counter | `decision`, `cache` | Every `CedarAuthorizer::is_authorized` outcome. `decision` ∈ `allow`/`deny`. `cache` ∈ `hit` (served from the decision cache, keyed on the compiled policy set's **content hash** plus the entity generation — deny hits are still re-audited, allow hits are not), `miss` (computed fresh), or `bypass` (the Redis-backed entity-generation counter was unreadable, so the cache was skipped entirely and the decision was computed directly against the last-known-good policy snapshot — see §4 "Authz availability"). The highest-value operational signal in the catalog: allow/deny volume and cache effectiveness. |
| `iam_redis_breaker_state` | gauge | `role` | Per-connection Redis circuit breaker state (SMA-476): `0` = closed, `1` = half_open, `2` = open. Set at construction as well as on every transition, so "no data" always means a scrape/registration problem, never an unset breaker. `role` ∈ `authz`/`api_keys`/`jwks` (closed set — `api_keys` requires `api_keys.introspect_cache.backend = "redis"` AND that cache holding its own connection, i.e. either `authz.cache.backend = "memory"`, or both Redis-backed with `redis_url`s that differ after trimming — SMA-485). **Per-replica** — aggregate with `max by (job, role)`, never `sum`. See §4 "Authz availability posture" and the three breaker alerts (`IamRedisBreakerOpen`/`IamJwksRedisBreakerOpen`/`IamRedisBreakerFlapping`). |
| `iam_redis_breaker_transitions_total` | counter | `role`, `to` | Every breaker state transition. `to` ∈ `open`/`half_open`/`closed`. **Not redundant with the gauge above**: a breaker that opens for 2 s every 30 s reads `0` in most 15–30 s scrapes, so this counter is the only artifact that survives a sub-scrape-interval state — it is what `IamRedisBreakerFlapping` watches. |
| `iam_authz_policy_snapshot_reloads_total` | counter | `outcome` | Every `PolicySnapshot` reload attempt. `outcome` ∈ `installed` (a fresher compiled set replaced the live one), `rejected` (an out-of-order reload lost its race and was discarded — benign in isolation), `failed` (the load or Cedar compile errored; the last-known-good snapshot keeps serving). `installed` must stay non-zero: the TTL backstop installs one every `authz.policy_cache_ttl_secs` regardless of generation movement, and silence means revocations are not taking effect (SMA-470). |
| `iam_authz_generation_rewinds_total` | counter | `counter`, `outcome`, `reason` | A Redis authz generation counter read back **below** what the process had already observed (SMA-474). `counter` ∈ `policy_gen`/`entity_gen`. `outcome` ∈ `repaired` (jumped forward with an atomic `INCRBY`, persisted so other replicas converge) / `repair_failed` (Redis rejected the write — `INCRBY` is `denyoom`, so `maxmemory` pressure does this; the replica falls back to a process-local generation, which is safe but stops cross-replica cache sharing) / `ceiling` (the repair would overflow Redis's i64 counter — see the remediation below). `reason` ∈ `missing` (the key was gone) / `lower` (it came back at a smaller value, e.g. a failover to a stale replica). **Only ever emitted on the `redis` backend** — the `memory` backend's in-process counters cannot rewind, and the series is *absent* there, which is what keeps `IamAuthzGenerationRewound` silent on a single-replica deployment. On the `redis` backend all 12 label combinations are **registered at zero from boot** (`Generations::from_connection`), so a flat line of zeros is the healthy state, not a missing metric — without that, the series would first appear already at `1` and `increase()` would baseline on it, so a single rewind could never fire the alert. |
| `iam_audit_records_total` | counter | `outcome`, `result` | Every `PgAuditLog::record`/`record_out_of_band` call. `outcome` ∈ `committed` (mutation audit rows) / `denied` (denial audit rows). `result` is `"ok"` for an INSERT that did not error. **Caveat:** this counts insert-attempts-not-erroring, not durably-committed rows — an in-transaction `record` call on a mutation's UoW bumps `result="ok"` before that transaction's outer `commit()`, so a rare downstream rollback leaves the row invisible even though the counter already incremented. This only diverges on the mutation-error path, which is itself visible elsewhere as a `result="error"`/5xx signal, so it doesn't mislead in steady state. |
| `iam_denial_audits_dropped_total` | counter | — | Bumped at `DenialAuditBuffer::push`'s drop-oldest site when the bounded denial-audit buffer is full. **Non-zero means the audit trail for denials has gaps** — see §4 "Denial-audit drops". |
| `iam_denial_audits_enqueued_total` | counter | — | Bumped on every `DenialAuditBuffer::push`, whether or not it also drops. Compare against the dropped counter to gauge loss ratio during a denial burst. |
| `iam_outbox_relay_ticks_total` | counter | `result` (`ok`/`error`) | One increment per relay tick (poll loop iteration), regardless of whether the tick found rows to drain. **This is the relay's liveness signal** — see §4 "Outbox stalled". |
| `iam_outbox_relay_drained_total` | counter | — | Rows locked and processed in a tick (published + failed, including newly-parked), summed from `TickReport.drained`. |
| `iam_outbox_relay_published_total` | counter | — | Rows successfully published in a tick (`drained − failures`). |
| `iam_outbox_relay_publish_failures_total` | counter | — | Rows whose `EventPublisher::publish` call failed in a tick (a subset of `drained`, superset of `parked`). |
| `iam_outbox_relay_parked_total` | counter | — | Rows that hit `[outbox].max_attempts` and were **parked** (poison) in a tick — a **counter of newly-parked rows this tick**, deliberately not a gauge (a gauge summed per-tick would read `0` on every tick that parks nothing new, hiding a growing parked backlog behind a flat-looking panel). See §4 "Outbox parked events". |
| `iam_outbox_oldest_unpublished_age_seconds` | gauge | — | Age (seconds) of the oldest unpublished-and-unparked row seen in the most recent **poll** tick's batch (`None` → reported as `0`). **Refreshed by poll ticks only** (SMA-489): nudge- and backlog-driven ticks run in `TickMode::Fresh`, which excludes rows with `attempts > 0`, so their batch is not representative of the backlog and they deliberately leave this gauge alone — otherwise, the moment a publisher started failing, every nudged tick would overwrite the real backlog age with `0` or a fresh row's age, resetting `IamOutboxBacklogAgeHigh`'s `for: 5m` pending state on each scrape and (under steady commit traffic) possibly stopping it firing at all. Its refresh rate is therefore exactly `[outbox].poll_interval_secs`, regardless of commit rate. **Freezes at its last value if the relay task wedges while the process stays alive** — it is a backlog-lag signal, not a liveness signal (see §4). |
| `iam_outbox_retention_ticks_total` | counter | `result` (`ok`/`error`) | One increment per `PgOutboxMaintainer` sweep tick (SMA-469), regardless of whether the tick deleted anything. **This is the retention sweep's liveness signal, and it fires on every tick even when `[outbox.retention].enabled = false`** — unlike the audit-retention task, the outbox maintainer is spawned unconditionally and its tick always runs the gauge refresh below, so `enabled = false` never explains silence here. See §4 "IamOutboxRetentionStalled". |
| `iam_outbox_rows_deleted_total` | counter | `reason` (`published`/`parked`) | Rows deleted by a sweep, split by which cutoff (`[outbox.retention].published_days` vs. `parked_days`) triggered the delete. `reason="parked"` staying at `0` is expected in steady state (`parked_days` defaults to `0` = never). |
| `iam_outbox_parked_rows` | gauge | — | The **current** count of parked (dead-letter) rows (`SELECT count(*) … WHERE parked = true`), refreshed on every retention tick — including when `[outbox.retention].enabled = false`, so this gauge (and the alert below) never goes stale just because deletion is paused. **Per-replica, not global-unique**: every IAM replica's maintainer queries the same table and sets the identical count on its own gauge, so N replicas emit N identical series for one fact. **Aggregate with `max by (job)`, never `sum`** — summing reports N× the real backlog. See §4 "IamOutboxDeadLetterBacklog". |
| `iam_outbox_dead_letters_replayed_total` | counter | `scope` (`one`/`bulk`), `beyond_dedup_window` (`true`/`false`/`unknown`) | Parked rows returned to the live queue. `scope="one"` increments by 1 per `POST /v1/outbox/dead-letters/{id}/replay` call; `scope="bulk"` increments by the **number of rows replayed** (not calls) per `POST /v1/outbox/dead-letters/replay` call — counted only after the enclosing transaction commits, so a rolled-back replay is never counted. `beyond_dedup_window` (SMA-471 D4) flags replays that JetStream's `duplicate_window` will **not** collapse, because replay keeps the row's id and therefore its `Nats-Msg-Id`: `"true"` = the row parked longer ago than the assumed window (3600s) so a consumer will very likely see the event twice; `"false"` = parked recently enough that dedup should still absorb it; `"unknown"` = the bulk path, which returns a row count rather than rows and so cannot answer per-row (it is therefore **always** `"unknown"` for `scope="bulk"`). Read it as an exposure estimate, not a verdict — it measures from `parked_at`, which is `max_attempts × poll_interval_secs` (~5 min at defaults) AFTER the first publish attempt JetStream would have deduplicated against, so it under-reports `"true"` by roughly that margin. |
| `iam_outbox_dead_letters_discarded_total` | counter | — | Parked rows permanently discarded via `POST /v1/outbox/dead-letters/{id}/discard`. Counted only after commit, same as the replay counters. |
| `iam_outbox_relay_wakeups_total` | counter | `source` (`notify`/`poll`/`backlog`) | SMA-489: one increment per relay wakeup, labelled by what woke it — a Postgres `LISTEN` notification, the `[outbox].poll_interval_secs` timer, or a backlog continuation after a full batch made progress. **One increment per tick, not per wakeup**, so `sum without (source) (…)` equals `sum without (result) (iam_outbox_relay_ticks_total)`. All three label values are primed at zero at relay start, so `increase()` can fire on the first occurrence of any source. |
| `iam_outbox_publish_lag_seconds` | histogram | — | SMA-489: end-to-end outbox latency (`now - occurred_at`) at the moment a row is successfully published. **This is the only signal that proves the SMA-489 nudge is working in production** — `iam_outbox_oldest_unpublished_age_seconds` cannot, because it resets to `0` on every empty tick and the nudge makes empty ticks far more frequent. |
| `iam_outbox_listener_notifications_total` | counter | — | SMA-489: notifications the `PgOutboxListener` actually received. Distinguishes "Postgres never notified us" (e.g. a transaction-mode pooler silently swallowed `LISTEN`) from "the relay never observed the permit", which `iam_outbox_relay_wakeups_total{source="notify"}` alone cannot. See §4 "IamOutboxNotificationsAbsent". |
| `iam_outbox_notifying_enqueues_total` | counter | — | SMA-495: enqueues that emitted a `pg_notify` — the write-side twin of `iam_outbox_listener_notifications_total`, and the control term `IamOutboxNotificationsAbsent` gates on. **Not 1:1 with the listener counter — do not build a delivery-loss ratio from the pair.** Postgres collapses notifications carrying an identical channel *and* payload within one transaction, and this payload is always empty, so a transaction enqueuing N events increments this N times while delivering exactly **one** notification. **Counted pre-commit**: the outbox writes on a transaction it recovers rather than owns, so there is no post-commit hook — a rolled-back mutation increments this while delivering no notification and draining no row (the alert absorbs that through its separate `drained` term, which is why that term is retained). A **dead-letter replay increments it not at all**, which is the property that makes the alert immune to a replay. Primed at zero iff `[outbox].wake_on_commit = true`, so the series existing means "this replica is configured to nudge"; `[outbox].relay_enabled = false` does not gate it. |
| `iam_outbox_listener_connected` | gauge | — | SMA-489: `1` when the outbox listener holds a live `LISTEN` connection, `0` otherwise. **Per-replica, and the replicas do NOT agree** — use `min by (job)` to ask "are all replicas listening" (never `max` or `sum`), or keep `instance` to see which one is down. |
| `iam_outbox_listener_reconnects_total` | counter | — | SMA-489: successful re-establishments of the outbox listener's `LISTEN` connection. **Counts recoveries, not failures** — a listener down through a long outage increments this once, on recovery, never per failed attempt, so a value that stops climbing mid-incident means "still down". A steadily climbing value means Postgres is churning the listener connection, and every cycle is a window in which notifications were dropped and delivery fell back to the poll. |
| `iam_audit_partition_maintenance_ticks_total` | counter | `result` | One per audit partition-maintenance tick (create-ahead + prune). `result` ∈ `ok`/`error`. Liveness signal — see §4 "Audit partition maintenance stalled". |
| `iam_audit_partitions_created_total` | counter | — | Monthly leaf partitions created by create-ahead. |
| `iam_audit_partitions_dropped_total` | counter | `outcome` | Monthly leaf partitions dropped by retention. `outcome` ∈ `denied`/`committed`. |
| `iam_audit_default_partition_rows` | gauge | — | Rows currently in the audit `DEFAULT` partitions. **Should be 0**; nonzero ⇒ create-ahead fell behind (freezes when the task is stalled while retention stays enabled — the ticks counter is the primary liveness signal there; when retention is **disabled** neither metric exists at all, see §4 "Audit partition maintenance stalled"). |
| `iam_bootstrap_admin_seed_failures_total` | counter | `stage` | Swallowed `BootstrapAdminSeeder` seed failures (SMA-468 D6). `stage` ∈ `list` (the `list_by_principal` existence check errored) / `txn` (the `begin`/`grant_in`/`enqueue`/`record`/`commit` sequence errored). Deliberately has **no alert** (D6) — watch the "Bootstrap-admin seed failures" panel on the IAM dashboard, or query `/metrics` directly. **Read it as a rate, not a level.** It is monotonic, so a single historical failure leaves it nonzero forever, including long after the identity was successfully seeded on a later attempt — an absolute nonzero value proves nothing on its own. What indicates an ongoing lockout is a counter that is **still climbing** (`increase(iam_bootstrap_admin_seed_failures_total[15m]) > 0`, sustained): the seed is idempotent-by-existence, so once the grant row commits this stops incrementing for that identity, and a seed that never commits is retried on every subsequent authentication. Confirm by looking for the grant row itself before concluding lockout. **A low, one-off increase is benign**: two concurrent first authentications by the same bootstrap identity can both pass the existence check and both attempt `grant_in`; the loser violates the unique grant constraint and rolls back under `stage="txn"` while the winner's grant commits — net state is correct and self-correcting. |
| `iam_system_rows_retired_total` | counter | `outcome` | Retirements of orphaned system-owned `policy`/`role` rows via `POST /v1/authz/system-policies/{id}/retire` (SMA-481, §4 "Retiring an orphaned system-owned row"). `outcome` ∈ `retired` (the deletes, the `PolicyDeleted` event and the audit row all committed) / `blocked` (surviving grants stopped it — nothing written) / `refused` (`fleet-not-converged`, or a static policy retired without `acknowledge_decision_change` — nothing written). **Not one increment per call:** `403` (non-Root), `409 system-immutable` (the id is still code-defined), `404`, and `409 not-system-owned` all return WITHOUT touching this counter — none of them are the fleet-skew / decision-change / blast-radius concerns it exists to page on. **No alert rule ships for it today** — retirement is a rare, deliberate, Root-only action, so this is a signal you query rather than one that pages you. It is nevertheless the only *monitorable* trace of the action: the `RetireSystemPolicy` `audit_log` row written in the same transaction is durable evidence, but nothing polls `audit_log` for it, and durable is not the same as monitored. |
| `iam_nats_publish_duplicates_total` | counter | — | Only emitted under `[outbox.publisher].backend = "nats"` (SMA-471). Every JetStream publish ack that came back `duplicate = true` — the `Nats-Msg-Id` dedup (D3) collapsing a relay redelivery of a row it already published once (the common case: the first publish's own ack was lost, so the relay retried a row JetStream already has). The adapter treats a duplicate ack as `Ok(())`, same as a fresh publish. Primed at zero in `NatsEventPublisher::connect`, before any publish, specifically so the *first* duplicate can still satisfy an `increase() > 0` query (a metrics-rs counter otherwise only appears at its first increment's value). A rising rate is not itself broken — it means acks are being lost somewhere in the round trip — but a rate that tracks the publish rate closely is worth investigating (§ D3 in the design doc: dedup covers a lost-ack retry, not a crash/DB-outage republish or an operator dead-letter replay, all of which are legitimately-intentional duplicates too). No dedicated alert ships for it (spec §7 — dashboard panels are out of scope); read it from `/metrics` or the dashboard panel. |
| `iam_nats_publish_duration_seconds` | histogram | — | Only emitted under `backend = "nats"`. The JetStream publish-ack round trip (`send_publish` request leg **and** the ack await, both — D2), recorded around every `publish_ack` call regardless of outcome. This sits inside the outbox relay's single lock-holding transaction (§1.3 of the design doc), so a rising p99 here is not just "NATS is slow" — it is directly lengthening how long `event_outbox` row locks and a pool connection are held per tick. Compare against `[outbox.publisher].publish_timeout_secs` (default 2s): a p99 approaching that ceiling means publishes are close to timing out, which is a leading indicator for `IamOutboxPublishFailures` below. |
| `iam_nats_connected` | gauge | — | Only emitted under `backend = "nats"`. `1` when the NATS client reports a live connection, `0` otherwise (`async_nats::connection::State::Disconnected`). Sampled by a **background task** (`spawn_connection_gauge_sampler`) on a 5s interval, deliberately **not** set inside `publish` — during a total outage every row eventually parks, `publish` stops being called at all, and a publish-driven gauge would freeze exactly when it matters most. **Per-replica, and the replicas genuinely disagree** — unlike `iam_outbox_parked_rows` (one global count every replica computes identically, where `max by (job)` is correct), this reports each replica's *own* connection state. `max by (job)` therefore reads `1` while any single replica is still connected, hiding exactly the partial outage worth investigating. Keep `instance` to identify which replica is down, or use `min by (job)` to ask "are **all** replicas connected". Never `sum`. This is the fastest-to-update of the three NATS signals — see "NATS backend: boot hard-fails…" below for what a `0` reading alongside a crash-looping *new* pod means versus a `0` on an already-running one. |

### 2.3 `paigasus-gateway` — IAM dependency, OpenAI upstream

| metric | type | labels | meaning / expected range |
|---|---|---|---|
| `gateway_iam_calls_total` | counter | `operation`, `result` | Every call the gateway's `require_iam_auth` middleware makes to IAM. `operation` ∈ `introspect` (`IntrospectApiKey`) / `authorize` (`IsAuthorized`, issued as a self-query — §4 gateway-M0 design §4.3). `result` ∈ `ok` / `denied` (authz `allowed == false`, or `Unauthenticated` from introspect) / `unavailable` (IAM transport/connection failure — maps to a `503` to the caller) / `error` (any other IAM-side error, maps to `500`). |
| `gateway_iam_call_duration_seconds` | histogram | `operation` | Latency of each IAM call, from the same middleware call sites. |
| `gateway_upstream_requests_total` | counter | `status_class` | One increment per OpenAI upstream call, `status_class` derived from the upstream HTTP status (`2xx`/`4xx`/`5xx`). **Streaming caveat (TTFB):** for `stream: true` this only covers the initial POST/headers exchange — `OpenAiClient::chat_completion` returns as soon as the response head arrives, and the SSE body streams lazily afterward. A mid-stream terminal SSE error (emitted as a `data: {"error":…}` event, §5 gateway-m0 spec) happens **past** this measured boundary and is **not** counted here. |
| `gateway_upstream_request_duration_seconds` | histogram | — | Upstream call latency — same TTFB caveat as above; do not read this as end-to-end stream duration. |

---

## 3. Run the local stack

The local stack (Prometheus + Grafana only — **no service containers**; both services run
natively via `cargo run`) lives under `ops/observability/`. It is dev-only, not for production
use.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"

# 1. Start the two services (each exposes /metrics on its main HTTP listener by default):
cargo run -p paigasus-iam       # http://0.0.0.0:8080/metrics
cargo run -p paigasus-gateway   # http://0.0.0.0:8088/metrics

# 2. Start Prometheus + Grafana:
cd ops/observability
docker compose up

# 3. Open:
#    Prometheus — http://localhost:9090  (scrapes both services via host.docker.internal)
#    Grafana    — http://localhost:3000  (anonymous admin login; provisioned dashboards
#                 under Dashboards, no manual datasource/import step needed)

# 4. Stop:
docker compose down       # add -v to also drop Prometheus's TSDB volume
```

Prometheus scrapes `host.docker.internal:8080` (job `iam`) and `host.docker.internal:8088` (job
`gateway`) every 15s (`ops/observability/prometheus/prometheus.yml`), and loads every
`prometheus/rules/*.rules.yml` file. The compose file adds `extra_hosts:
["host.docker.internal:host-gateway"]` on the Prometheus service so `host.docker.internal`
resolves on native Linux Docker as well as Docker Desktop (macOS/Windows resolve it natively).
Prometheus and Grafana images are pinned to explicit tags (`prom/prometheus:v3.13.1`,
`grafana/grafana:13.0.3`) — never `:latest`.

### Dashboard tour

Two dashboards are provisioned automatically (`grafana/dashboards/{iam,gateway}.json`, wired via
`grafana/provisioning/`):

**Paigasus IAM** (`iam.json`) — one row per concern:
- HTTP request rate + p95 latency (RED, HTTP).
- gRPC request rate + non-OK ratio (RED, gRPC — gRPC failures don't show up as HTTP 5xx, so this
  panel is the only place to see them at a glance).
- Authz decisions (allow vs. deny rate) + cache hit ratio.
- Redis circuit breaker state (SMA-476, stat panel, `max by (job, role)` — **never** `sum` across
  replicas, every replica reports its own state; `job` is kept so prod and canary don't collapse
  into one series): 0=closed, 1=half-open, 2=open, per `role`. See §4 "Authz availability posture"
  and the three breaker alerts.
- Audit write rate (by `outcome`).
- Denial-audit drop rate — **should be flat 0**; any nonzero value is worth investigating (§4).
- Outbox row: drained rate, published rate, publish-failure rate, parked events (15m window,
  stat panel), oldest-unpublished age (stat panel — the key backlog SLO), relay tick rate,
  dead-letter backlog (stat panel, `max by (job)` — never read this as `sum`, see §2.2), rows
  deleted by the retention sweep (rate, by `reason`).

**Paigasus Gateway** (`gateway.json`):
- HTTP request rate + p95 latency (RED, HTTP).
- Inflight requests (stat).
- IAM-call rate + p95 latency (the introspect/authorize dependency).
- Upstream (OpenAI) request rate + p95 latency — remember the TTFB caveat (§2.3) when reading
  this for streamed traffic.

---

## 4. Alerts → runbook entries

Alert rules live in `ops/observability/prometheus/rules/*.rules.yml` — one file per service plus
`targets.rules.yml` for cross-service scrape-target health — and each is unit tested against
synthetic series via `promtool test rules` using the paired `rules/tests/*.test.yml` as part of
CI. Thresholds
below are **starting points** — tune `for:` durations and numeric thresholds per environment
(traffic volume, SLOs) once real data is available.

| alert | expr | severity |
|---|---|---|
| `IamDenialAuditDrops` | `rate(iam_denial_audits_dropped_total[5m]) > 0` | warning |
| `IamOutboxBacklogAgeHigh` | `iam_outbox_oldest_unpublished_age_seconds > 300` | warning |
| `IamOutboxEventsParked` | `increase(iam_outbox_relay_parked_total[15m]) > 0` | warning |
| `IamOutboxPublishFailures` | `increase(iam_outbox_relay_publish_failures_total[5m]) > 0` for 5m | warning |
| `IamOutboxRelayStalled` | `rate(iam_outbox_relay_ticks_total[10m]) == 0` | critical |
| `IamOutboxNotificationsAbsent` | `(sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0) and (sum by (job, instance) (increase(iam_outbox_relay_drained_total[30m])) > 0) and on (job) (sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m])) > 0)` for 15m | warning |
| `IamPolicySnapshotReloadsStalled` | `(sum by (job, instance) (increase(iam_authz_policy_snapshot_reloads_total{outcome="installed"}[10m])) or (up{job="iam"} == 1) * 0) == 0` for 5m | critical |
| `IamAuditPartitionMaintenanceStalled` | `sum without (result) (increase(iam_audit_partition_maintenance_ticks_total[2d])) == 0` for 1h | warning |
| `IamOutboxRetentionStalled` | `(sum by (job, instance) (increase(iam_outbox_retention_ticks_total[6h])) or (up{job="iam"} == 1) * 0) == 0` for 2h | warning |
| `IamOutboxRetentionErroring` | `increase(iam_outbox_retention_ticks_total{result="error"}[6h]) > 0` for 2h | warning |
| `IamOutboxDeadLetterBacklog` | `max by (job) (iam_outbox_parked_rows) > 0` for 1h | warning |
| `IamHighErrorRate` | `sum(rate(iam_http_requests_total{status_class="5xx"}[5m])) / sum(rate(iam_http_requests_total[5m])) > 0.05` for 10m | critical |
| `IamGrpcHighErrorRate` | `sum(rate(iam_grpc_requests_total{grpc_status!="ok"}[5m])) / sum(rate(iam_grpc_requests_total[5m])) > 0.05` for 10m | critical |
| `IamAuthzRedisCacheBypassed` | `sum(rate(iam_authz_decisions_total{cache="bypass"}[5m])) > 0` for 10m | critical |
| `IamRedisBreakerOpen` | `max by (job, role) (iam_redis_breaker_state{role!="jwks"}) != 0` for 2m | warning |
| `IamJwksRedisBreakerOpen` | `max by (job, role) (iam_redis_breaker_state{role="jwks"}) != 0` for 1m | critical |
| `IamRedisBreakerFlapping` | `max by (job, role) (increase(iam_redis_breaker_transitions_total{to="open"}[10m])) > 5` | warning |
| `IamAuthzGenerationRewound` | `sum by (counter, outcome) (increase(iam_authz_generation_rewinds_total[15m])) > 0` for 5m | warning |
| `GatewayHighErrorRate` | `sum(rate(gateway_http_requests_total{status_class="5xx"}[5m])) / sum(rate(gateway_http_requests_total[5m])) > 0.05` for 10m | critical |
| `GatewayIamDependencyUnavailable` | `rate(gateway_iam_calls_total{result="unavailable"}[5m]) > 0` for 5m | critical |
| `GatewayUpstreamErrors` | `sum(rate(gateway_upstream_requests_total{status_class="5xx"}[5m])) / sum(rate(gateway_upstream_requests_total[5m])) > 0.05` for 10m | warning |
| `TargetDown` | `up == 0` for 2m | critical |

### `IamDenialAuditDrops` — denial-audit rows being dropped

**Meaning.** The denial-audit path is a **bounded, non-blocking, in-process buffer**
(`DenialAuditBuffer`) sitting between `CedarAuthorizer.is_authorized` and the `audit_log` table.
`is_authorized` only ever does a non-blocking enqueue onto this buffer — it never awaits a
Postgres insert, so a denial flood can never stall authorization decisions. When the buffer is
full, a new denial **evicts the oldest queued denial** (drop-oldest) and bumps
`iam_denial_audits_dropped_total`.

**Durability tier (read this before treating a drop as data loss):** mutation audit rows
(`outcome="committed"`) are **exactly-once** — written in the same Postgres transaction as the
mutation, rolling back together. Denial audit rows (`outcome="denied"`) are **best-effort** —
under sustained denial volume that outpaces the drain task, some denial audit entries are
observably lost (never silently). This is a deliberate design tradeoff (M5 audit/outbox spec D8):
a denial flood degrades the audit trail gracefully instead of becoming a write-amplification /
DoS vector against the shared connection pool. There is deliberately **no warning log** at the
drop site itself — that would spam during exactly the burst that's already causing drops (this is
the same throttling concern that motivated removing the old 60-second ticker task in favor of
this counter). The counter + this alert are the intended visibility mechanism.

**Likely causes:** a sustained spike in denied requests (a misconfigured/compromised client
hammering a forbidden action, a broad policy change that turned previously-allowed traffic into
denials, or a scanning/DoS attempt), or `audit.denial_buffer_capacity` set too low for normal
peak denial volume.

**Confirm:**
1. Check `iam_denial_audits_enqueued_total` vs. `iam_denial_audits_dropped_total` rate to gauge
   the loss ratio.
2. Check `iam_authz_decisions_total{decision="deny"}` rate and the IAM logs for which principals/
   actions are being denied — is this a legitimate traffic pattern or an attack?
3. Check `iam_audit_records_total{outcome="denied"}` (successful denial audit inserts) against
   the enqueue rate to see how much of the denial trail actually landed.

**Remediation:**
- If this is a **legitimate but larger-than-provisioned** denial workload, raise
  `[audit].denial_buffer_capacity` (default `4096`, in `iam.toml` / `IAM_AUDIT__DENIAL_BUFFER_CAPACITY`)
  to give the drain task more headroom before the ring buffer wraps, and redeploy.
- If this is an **attack/abuse pattern**, the fix is upstream of the buffer (revoke the offending
  credential, tighten a policy, block at the network layer) — enlarging the buffer alone doesn't
  address a sustained adversarial flood, it only raises the bar.
- A drop event does **not** need a service restart or data-repair action — the buffer keeps
  operating normally; only the dropped entries' audit history is unrecoverable (there is no
  redundant copy of a denial that never made it into `audit_log`).

### `IamOutboxBacklogAgeHigh` — outbox events are backing up

**Meaning.** `iam_outbox_oldest_unpublished_age_seconds` has stayed above 300s (5 minutes) for at
least 5 minutes. The outbox relay task polls `event_outbox` on an interval
(`[outbox].poll_interval_secs`, default `5`), locking a batch of unpublished rows with `SELECT …
FOR UPDATE SKIP LOCKED` (safe across multiple IAM replicas — two replicas never grab the same
row) and handing each to `EventPublisher::publish`. A growing oldest-unpublished age means rows
are being enqueued (by mutation use-cases) faster than the relay is successfully publishing them.

**Likely causes:** the `EventPublisher` implementation is failing/erroring on most publishes.
Which implementation is running depends on `[outbox.publisher].backend`: the default `tracing`
(`TracingEventPublisher`) only fails on serialization-adjacent bugs, while the optional
production backend `nats` (`NatsEventPublisher`, SMA-471) fails whenever the broker is
unreachable or rejecting writes — on that backend check `IamOutboxPublishFailures` and
`iam_nats_connected` first, they are both faster signals than backlog age. Other causes: the relay is running but its
`poll_interval_secs`/`batch_size` are too conservative for current write volume; or the relay
task itself is wedged (in which case `IamOutboxRelayStalled`, below, is the more precise signal —
check that alert too).

**Confirm:**
1. `rate(iam_outbox_relay_ticks_total[5m])` — is the relay still ticking? If it's `0`, this is
   really an `IamOutboxRelayStalled` situation (the age gauge is frozen, not actually growing).
2. `rate(iam_outbox_relay_publish_failures_total[5m])` vs.
   `rate(iam_outbox_relay_drained_total[5m])` — what fraction of drained rows are failing to
   publish?
3. `increase(iam_outbox_relay_parked_total[15m])` — are rows being permanently parked (poison),
   which would explain a subset of the backlog never clearing?
4. IAM logs for `EventPublisher::publish` failures / relay tick errors.

**Remediation:**
- If publish failures dominate: fix/restart whatever `EventPublisher` depends on (broker
  connectivity, credentials, etc.).
- If the relay is healthy but under-provisioned for volume: raise `[outbox].batch_size` and/or
  lower `[outbox].poll_interval_secs` (both in `iam.toml` / `IAM_OUTBOX__*`), then redeploy.
- If `[outbox].relay_enabled = false` was set (which only stops the relay task — outbox rows
  still accrue transactionally and safely), re-enable it; IAM emits a startup `warn` when the
  relay is disabled precisely because an undrained backlog is unbounded in that mode.

### `IamOutboxEventsParked` — a poison event was parked

**Meaning.** `iam_outbox_relay_parked_total` increased in the last 15 minutes — at least one
`event_outbox` row hit `[outbox].max_attempts` (default **`60`**, raised from `5` by SMA-471 —
see the arithmetic and the "why" below) consecutive publish failures and was
marked `parked = true`, permanently excluded from future relay batches (the relay's poll
predicate is `published_at IS NULL AND parked = false`). This is deliberately a **counter of
newly-parked rows**, not a gauge of the current parked-row count — a gauge summed per-tick would
read `0` on every tick that parks nothing new and hide a slowly-growing parked backlog behind a
flat panel; `iam_outbox_parked_rows` (§2.2, refreshed on every retention tick) is the exact answer
to "how many rows are parked right now" if needed, or a direct SQL count (below).

**Likely causes:** two distinct shapes, and telling them apart matters for what you do next.
- **A single bad payload.** One event's payload is fundamentally unpublishable (malformed for the
  specific `EventPublisher` backend, or an event type the consumer rejects deterministically) —
  retrying it forever would never succeed, hence the attempt cap.
- **Mass parking — bounded by `[outbox].batch_size`, not "the whole backlog" — is still the
  expected outage signature, not just a poison-message symptom.** The relay drains at most
  `batch_size` (default `100`) rows per tick (`ORDER BY id LIMIT batch_size`), so within any single
  poll interval only up to `batch_size` distinct rows can even be attempted, let alone parked — on
  a backlog larger than `batch_size`, most of it is never selected during a short outage, so it
  cannot possibly park. A row parks after `[outbox].max_attempts` (default **`60`**, see below)
  consecutive failed attempts; because the relay re-selects the same still-unpublished row on
  every subsequent tick, that spans **59** poll intervals between its first and sixtieth attempt
  (295s at the default `poll_interval_secs = 5`), plus up to one further interval for that first
  attempt to happen at all — so **~300 seconds (5 minutes) is the worst case for the first
  `batch_size` rows to park**, not for the whole backlog to park at once. If the outage continues
  past that, the relay moves on to the next `batch_size` rows (the previous batch is now excluded,
  `parked = true`) and repeats the same ~5-minute cycle, so a longer outage parks proportionally
  more rows in `batch_size`-sized waves rather than parking an unbounded backlog instantly. Many
  rows parking in a short window is still the signal to suspect a resolved-or-ongoing outage before
  suspecting a payload bug — that conclusion doesn't change, only the blast-radius number does;
  `IamOutboxDeadLetterBacklog` (below) is the complementary alert for "a parked backlog nobody has
  dealt with yet."

  **This window used to be ~25 seconds** (`max_attempts = 5`) before SMA-471. It was widened
  deliberately, not relaxed carelessly: with a real broker-backed `EventPublisher`, a routine
  broker restart is now a realistic multi-second outage, and 25 seconds of tolerance dead-lettered
  the *entire in-flight backlog* into the dead-letter surface for something as ordinary as a NATS
  rolling restart, forcing an operator to manually replay it afterward (itself a
  duplicate-delivery event — see the NATS section below). ~5 minutes of tolerance absorbs that
  routine case; the accepted cost is that a *genuinely* poison row — one that will never succeed no
  matter how many times it's retried — now burns 60 attempts and up to `batch_size` other rows
  behind it head-of-line block for ~5 minutes instead of ~25 seconds before the relay parks it and
  moves on. See `rs/crates/services/paigasus-iam/src/config.rs`'s doc comment on
  `OutboxConfig::max_attempts` for the full rationale.

**Confirm:**
1. IAM logs around the parked event's `id` — the relay emits `tracing::error!` with
   `id`/`event_type`/`attempts`/`reason` at the parking site, which usually explains *why* every
   attempt failed.
2. **The API is the primary way to inspect parked rows.** `GET /v1/outbox/dead-letters?event_type=&parked_from=&parked_to=&cursor=&limit=`
   (Root-only — enforced inside `DeadLetterService`, not by the Cedar action schema itself; in
   practice this means the caller needs a `platform_admin` grant scoped at Root) returns each
   parked row's `last_error`, `attempts`, `parked_at`, and the raw `payload`, keyset-paginated via
   `cursor`/`next_cursor` exactly like the audit query API. Absent/empty query params are
   unfiltered; `limit` defaults to 50 and is capped at 200. **A caveat, not a bug:** a row with
   `parked_at IS NULL` can never satisfy `parked_from`/`parked_to` (Postgres never evaluates a
   `NULL` comparison as true), so such a row is invisible to `list` whenever either time bound is
   set, and to bulk replay whenever a time bound narrows the match — it stays reachable via an
   unfiltered (or only `event_type`-filtered) query, so nothing is permanently lost. The `m0009`
   migration backfilled `parked_at = now()` for every pre-existing parked row, so exposure to this
   is small in practice, but a triaging operator who knows a row is parked and can't find it in a
   windowed query should think of this before assuming a bug.
3. **Break-glass fallback (API unreachable only).** Count/inspect parked rows directly against
   Postgres:
   ```sql
   SELECT id, event_type, attempts, occurred_at, aggregate_prn
   FROM event_outbox
   WHERE parked = true
   ORDER BY occurred_at;
   ```

**Before upgrading to (or past) the `m0009` migration, run `SELECT count(*) FROM event_outbox;`.**
`m0009`'s `ALTER TABLE` takes an `ACCESS EXCLUSIVE` lock, and — unlike `m0008`'s partition
DDL — that lock is held for the migration's *entire* remaining body, not released before the
backfill: the `UPDATE … WHERE parked = true AND parked_at IS NULL` (a sequential scan, since its
own supporting index is created only afterward) and both non-concurrent `CREATE INDEX` builds all
run while still holding the same `ACCESS EXCLUSIVE` request the `ALTER TABLE` already queued,
blocking every `PgOutbox::enqueue` for as long as they take. `SET LOCAL lock_timeout = '5s'`
bounds how long Postgres will wait to *acquire* that lock — it does nothing to bound how long the
migration *holds* it once acquired. On a small table this whole sequence is sub-second and
invisible; on a table with a large accumulated backlog — precisely the deployment this feature
exists to rescue — the backfill and index builds scale with row count, not disk size, so a large
`count(*)` means a materially longer migration window with writes blocked throughout. Schedule the
upgrade for a low-traffic window if the count is large.

**Remediation.** The API is the primary recovery path — reach for the SQL fallback at the very
end of this section only when the API itself is unreachable.
- **Replay one row:** `POST /v1/outbox/dead-letters/{id}/replay` returns the named row to the live
  queue; the relay picks it up on its next poll. **Discard one row permanently:**
  `POST /v1/outbox/dead-letters/{id}/discard`. Both are Root-only, the same posture as the `GET`
  listed under Confirm above.
- **Bulk replay:** `POST /v1/outbox/dead-letters/replay` with a JSON body
  `{"event_type": …, "parked_from": …, "parked_to": …, "max_rows": N}`. **`max_rows` is required**
  — an absent or zero value is rejected with `400 invalid-bulk-replay` *before any store access*.
  This is deliberate, not an oversight: an "at least one filter must be present" check was
  considered and rejected, because `parked_from = "1970-01-01T00:00:00Z"` trivially satisfies such
  a check while still matching every row — `max_rows` is the actual guard on blast radius. The
  server additionally caps the effective replay count at 10,000 rows regardless of the requested
  `max_rows`; the audit entry records both the requested `max_rows` and the enforced
  `capped_max_rows`, so a request over the cap is auditable rather than silently truncated-looking.
  **There is deliberately no bulk-discard endpoint** — see the `parked_days` bullet below for the
  supported way to retire a backlog in bulk.
- **Confirm the root cause is actually fixed before any bulk replay.** Mass parking is usually an
  outage (see Likely causes above); replaying into a still-broken publisher just re-parks the same
  rows on their very next failed attempt.
- **A 10k-row replay adds roughly 8 minutes of low-id backlog for a relay to work through — an
  idealized aggregate-capacity estimate, not a guaranteed delay before live traffic is touched.** A
  replayed row keeps its original — older, lower — `id`, and the relay drains strictly
  `ORDER BY id ASCENDING` at `[outbox].batch_size = 100` rows every
  `[outbox].poll_interval_secs = 5` seconds (10,000 / 100 × 5s = 500s ≈ 8m for a single relay, if
  it did nothing else the whole time). It is **not** true that the replay is drained to completion,
  in full, before a relay touches any newer/live row: `FOR UPDATE SKIP LOCKED` means a relay that
  finds the next lower-id replayed rows already locked by a peer (or mid-transaction) simply moves
  on to the next available rows in the batch, which can include newer, live-traffic rows, rather
  than blocking behind them — there is no strict global ordering guarantee here, only the relay's
  poll predicate and `ORDER BY`. With N IAM replicas each running their own relay and partitioning
  the work this way, the aggregate capacity consumed by the replay is roughly **8/N minutes' worth**
  spread across the fleet, not a flat 8 charged to any one relay — schedule a large bulk replay for
  a low-traffic window, or replay in smaller batches, if even that capacity hit to live traffic is
  unacceptable.
- **`404` on any of the three replay/discard endpoints conflates several distinct states**: no row
  exists with that id, a row that was never parked (still live, or already published), a row
  another actor (or another attempt of the *same* retry) already replayed or discarded, and —
  rarely — a row the relay is mid-tick on and about to park. Do not chase a phantom id; re-`GET`/
  `list` to confirm the row's current state before assuming a bug.
- **Replay is not idempotent, and that is intentional — a `404` on retry is the expected
  success-after-timeout signal, not a failure.** If a client times out waiting for a `replay`
  response, the safe move is to simply retry the call: if the first attempt actually succeeded,
  the retry gets `404`, and that `404` *is* confirmation recovery already happened — it is not an
  error to keep chasing. A retried *bulk* replay is a fresh query, though, and can match a
  different row set than the first attempt did (rows outside the original cursor/time bounds may
  now qualify) — don't assume a retried bulk call is a strict no-op.
- **Replay deliberately exercises the outbox's at-least-once delivery contract.** The relay was
  already at-least-once (a publish that succeeds followed by a failed commit re-publishes the same
  event), so every downstream consumer must already be idempotent — a manual replay just forces
  that path to run on demand rather than introducing a new failure mode.
- **Discard destroys delivery, not just evidence.** The underlying event already committed inside
  IAM; discarding means it will now **never** reach any consumer — with a real broker-backed
  `EventPublisher` (SMA-471) that becomes a permanent, silent divergence between IAM's own state
  and everything downstream of it. The discard's audit entry is deliberately **lossless**: it
  carries the complete event, payload included, and is the documented reconciliation input for
  whatever manual/compensating action the now-undelivered event requires. **Write down a
  reconciliation plan before discarding** — once discarded, that audit entry is the event's only
  remaining trace.
- **Bulk retirement of an old backlog uses `[outbox.retention].parked_days`, not a bulk-discard
  call — there is deliberately no bulk-discard endpoint.** To retire a large accumulated parked
  backlog on a schedule: set `parked_days` to a deliberate window (e.g. `30`), let the retention
  sweep delete rows older than that window on its normal `interval_secs` cadence, then set
  `parked_days` back to `0`. Unlike a bulk `DELETE`, this stays **reversible right up until the
  sweep actually runs** — an operator who changes their mind before the next tick loses nothing.
  **Unlike a discard through the API, this path leaves no audit trail at all** — `PgOutboxMaintainer`
  deletes each row with only a counter increment (`iam_outbox_rows_deleted_total{reason="parked"}`),
  none of the discarded-event audit entry's payload, actor, or correlation id — so choose the
  per-row API discard above instead whenever the deleted events might need reconstructing later,
  and reserve `parked_days` for backlogs you're confident are safe to lose without a trace.
- If a row's payload is genuinely malformed (a bug in the writer, not a downstream outage), it will
  never publish successfully no matter how many times it's replayed; leave it parked (or discard it
  with a recorded reconciliation plan) and open a follow-up to fix the writer, rather than looping
  it through more failed attempts.

**Break-glass fallback (API unreachable only).** If the HTTP API itself is down, a row can still
be un-parked directly in Postgres — this mirrors the API's own `REPLAY_ONE_SQL` exactly, including
its `AND parked = true` guard, not just the `parked_at`/`attempts`/`parked` reset:
```sql
UPDATE event_outbox
SET parked = false, attempts = 0, parked_at = NULL
WHERE id = '<parked-row-id>' AND parked = true;
```
The `AND parked = true` guard is not decoration: it is what makes a live or already-published row
untouchable through this path, so a mistyped id can never silently zero a healthy row's `attempts`
or clear its `parked_at`. If the id you paste in isn't actually parked, this statement affects
**zero rows** — that is the guard working as intended (your id was wrong, or the row already
recovered), not a failure to retry by dropping the guard.
Prefer the API's `replay` endpoint whenever it is reachable — besides being the documented path, it
is also what produces the audit trail (`ReplayOutboxDeadLetter`, in `audit_log`); a direct SQL
`UPDATE` bypasses that entirely and leaves no record that the recovery action happened.

### `IamOutboxPublishFailures` — outbox publishes are failing (warning)

**Meaning.** `iam_outbox_relay_publish_failures_total` increased in the last 5 minutes and stayed
increased for the `for: 5m` hold — a row's `EventPublisher::publish` call errored during a relay
tick. This is deliberately the **earliest** outbox signal: it can fire before
`IamOutboxBacklogAgeHigh` (which needs the backlog to actually age past 5 minutes) and well before
`IamOutboxEventsParked` (which needs a row to exhaust `[outbox].max_attempts`, now ~5 minutes at
the default `poll_interval_secs`, see above). The counter is primed at zero from boot
(`relay.rs` increments it by 0 on every tick, even a tick that fails nothing), so `increase() > 0`
can fire on the very first failure rather than needing a pre-existing nonzero baseline.

**Likely causes.** With the `tracing` backend (the default) this alert should never fire —
`TracingEventPublisher::publish` only errors on serialization-adjacent bugs. With
`backend = "nats"` (SMA-471) the far more common cause is the broker itself: unreachable
(`NatsPublisherError::Disconnected`, `::Connect`, or a tripped [D11] breaker short-circuiting
before dialling), refusing the write (stream deleted out from under a running service, subject
permission revoked), or an oversized payload past NATS's `max_payload`.

**Confirm:**
1. `iam_nats_connected` (§2.2) — is the NATS client's own view of the connection down? A flat `0`
   points straight at broker connectivity; a `1` alongside failing publishes points at something
   more specific (permissions, a deleted stream, an oversized payload).
2. IAM logs / `event_outbox.last_error` on the affected rows — `relay.rs`'s `describe_error` walks
   the full `source()` chain of `NatsPublisherError`, so the row's `last_error` names which
   `NatsPublisherError` variant fired, not just "backend error".
3. `rate(iam_outbox_relay_publish_failures_total[5m])` vs. `rate(iam_outbox_relay_drained_total[5m])`
   — is this every row failing (broker down) or a subset (one bad payload)?
4. `histogram_quantile(0.99, rate(iam_nats_publish_duration_seconds_bucket[5m]))` (§2.2) — publishes
   consistently near `[outbox.publisher].publish_timeout_secs` point at a slow/blackholed broker
   rather than a fast, clean rejection.

**Remediation:**
- Broker down or unreachable: restore NATS connectivity (or wait out a routine restart — D9 raised
  `max_attempts` specifically so this no longer needs urgent action within the first ~25 seconds).
  `async-nats` reconnects in the background on its own once the broker returns; no service restart
  is needed.
- Stream deleted or permission revoked: this does not self-heal — `NatsEventPublisher` never
  recreates or re-authenticates a stream after boot (D7 is boot-time only). Restore the stream
  (or the permission) and, if the stream itself was recreated from scratch, restart IAM so
  `connect`'s D7 verification runs again against the new stream's config.
- Oversized payload: `NatsPublisherError::Publish`'s message should name the size problem
  directly (D9's guard test asserts this). This is treated as a permanently-failing row today
  (`PublishError::Permanent` — immediate parking instead of retrying — is a documented follow-up,
  spec §7); it will keep failing every attempt until parked, at which point `IamOutboxEventsParked`
  is the relevant playbook.
- If failures stop but the backlog is still elevated, check `IamOutboxBacklogAgeHigh` next — a
  resolved publish-failure spell can still leave a backlog that needs `batch_size`/
  `poll_interval_secs` tuning to drain promptly.

### NATS backend: boot hard-fails on an unreachable broker or a drifted stream (SMA-471)

**What happens.** With `[outbox.publisher].backend = "nats"`, `NatsEventPublisher::connect` runs
**before any listener is spawned** — before HTTP, gRPC, metrics, and before the outbox relay
starts (D7). If it returns `Err`, `main.rs` propagates it straight out of `main()` via `?`: the
process exits nonzero with **no port bound**, so this never shows up as a `/healthz`/`/readyz`
failure, an `IamHighErrorRate` blip, or any Prometheus series at all — there is nothing scraping
yet. Under an orchestrator this presents as **`CrashLoopBackOff`**, not a degraded-but-serving
pod.

**Why boot-time rather than a background retry.** A NATS-backed deployment that started serving
HTTP/gRPC traffic with no working delivery sink would look healthy on every existing check while
silently accumulating an outbox backlog with no bound (D7's rationale). Failing fast, before
anything binds, is the deliberate trade — see spec §3.3 and §7 for why `/readyz` does *not* also
gate on NATS health post-boot (a broker outage on an already-running replica does not take it out
of rotation; `IamOutboxPublishFailures` above, `IamOutboxBacklogAgeHigh`, and
`iam_nats_connected` are that signal instead).

**Two distinct causes, both fatal at boot, told apart by the log line and error text:**

- **Broker unreachable or credentials rejected.** `NatsPublisherError::Connect` /
  `NatsPublisherError::Credentials`. The process log's final line before exit is the propagated
  error's `Display` chain, e.g. `nats connect failed: IO error: connection refused: connection
  refused` (upstream `async-nats` renders the cause in both its own `Display` and `source()`, so
  the innermost text can appear twice — that duplication is upstream's rendering, not a bug here).
  `NatsPublisherError::Credentials` names the unreadable `credentials_file` path directly.
  **Remediation:** restore broker reachability (network, DNS, the `nats-server` process itself) or
  fix the `url`/`credentials_file` config, then let the orchestrator restart the pod (or restart
  it manually).
- **Stream config drift (D7).** `NatsPublisherError::StreamConfigDrift`. `connect` ensures the
  `IAM_EVENTS` stream exists (`get_or_create_stream`) but — deliberately — does **not** reconcile
  an already-existing stream's config, because this service must never silently reshape a stream
  external consumers already depend on. Instead it verifies the live stream's `retention`,
  `duplicate_window`, `storage`, `subjects`, and `max_age` against what `[outbox.publisher]`
  requires and hard-fails if any is weaker. The error names exactly which field and both values,
  e.g. `stream IAM_EVENTS has duplicate_window = 120s, but this service requires 3600s`, or
  `stream IAM_EVENTS has retention = interest, but this service requires limits` — the
  retention case is the most consequential: an `Interest`- or `WorkQueue`-retention stream
  silently discards messages on arrival once this PR ships (no consumer subscribes yet), so
  rejecting it at boot rather than adopting it is intentional, not overly strict. **Remediation:**
  which operator action works depends on the field the error names. `duplicate_window` is editable
  in place with `nats stream edit`, and so is a `retention` drift to `interest` — neither needs
  touching the stream's data. `storage` is never editable in place, and `retention` cannot be
  changed to or from `workqueue` either (both rejected outright by JetStream's Stream Update API):
  fixing either means delete-and-let-`connect`-recreate `IAM_EVENTS`, which is a maintenance window
  and, unless drained first, data loss — only safe if no other consumer already depends on the
  stream's current config. Either way, an alternative is to relax `[outbox.publisher]`'s own config
  to match an intentionally different existing stream. This is never transient — restarting the pod
  without changing anything reproduces the identical failure every time.

**Confirm which case you're in** from the log line alone; no metrics exist to query for a pod that
never finished booting. `kubectl logs` (or the platform-equivalent) on the crash-looping pod is
the only signal.

> **Permissions, TLS and credentials** have their own runbook: [`RUNBOOK-nats.md`](./RUNBOOK-nats.md)
> (SMA-493). A denied publish does not fail its *request* with a permissions error — it times out,
> so it never crash-loops like anything in this section. It is not silent, though: the broker's
> permissions-violation text is still logged at `error` level (`RUNBOOK-nats.md` §1), and
> `IamOutboxPublishFailures` (§4 above) still fires once a tick's publish times out.

### `IamOutboxRelayStalled` — the relay has stopped ticking (critical)

**Meaning.** `rate(iam_outbox_relay_ticks_total[10m]) == 0` for 10 minutes — the relay's poll
loop has stopped incrementing its tick counter even though the IAM **process is still up**
(`up == 1`, so `TargetDown` does **not** fire for this). This is the **true relay-liveness
signal**, and it exists specifically because `iam_outbox_oldest_unpublished_age_seconds` cannot
serve that role: the age gauge is only refreshed when a tick observes a non-empty batch, so a
relay task that has wedged (deadlocked, panicked out of its loop without crashing the process,
stuck on a hung Postgres query, etc.) **freezes the age gauge at its last observed value** instead
of climbing — a naive "is the backlog age high" check would miss a stalled-but-alive relay
entirely, or worse, look falsely healthy if the freeze happened while the age was still low.

**Likely causes:** the relay task panicked/exited without bringing down the process (should not
happen under normal Rust panic-unwind semantics inside a `tokio::spawn`ed task, but a bug is
possible); the relay is stuck on a long-running or deadlocked Postgres query (e.g. lock
contention on `event_outbox`); the shutdown-watch fired unexpectedly and the loop exited but
the process didn't restart; or `[outbox].relay_enabled = false` in config (in which case this
"alert" is expected and should be silenced/acknowledged, not treated as an incident — check
config first).

**Confirm:**
1. Verify the IAM process is actually up (`up{job="iam"} == 1`) — if it's down instead, this is
   really a `TargetDown` situation and a process restart, not a stalled-task situation.
2. Check `[outbox].relay_enabled` in the running config — `false` means no relay task was ever
   spawned, which is a config/deployment fact, not an incident.
3. Check for long-running/blocked queries against `event_outbox` on the Postgres side
   (`pg_stat_activity`), and IAM logs for signs of a panic or an unhandled error in the relay
   loop around the time ticks stopped.

**Remediation:**
- If the relay is disabled by config and that's intentional, silence the alert for that
  environment rather than treating it as an incident.
- If the relay task has genuinely wedged, a **process restart** of `paigasus-iam` is the fastest
  recovery (the relay task is re-spawned on boot; `event_outbox` rows are durable in Postgres, so
  a restart loses no data — the relay simply resumes where it left off via its
  `published_at IS NULL AND parked = false` poll predicate).
- If a Postgres-side lock/hang caused the stall, resolving that (killing a blocking session,
  etc.) may let the existing relay task recover without a restart — check whether ticks resume
  before restarting the service.

### `IamOutboxNotificationsAbsent` — commit nudges are not arriving (warning)

Rows are being written and drained, but the listener has received no `iam_outbox_event`
notification for 30 minutes. Delivery has silently fallen back to `[outbox].poll_interval_secs`
(~5 s), which is correct but not what this deployment is configured for.

There are three terms. The two that describe *this replica* — the listener term and the `drained`
term — aggregate `by (job, instance)`, so this fires **per replica**. The third,
`iam_outbox_notifying_enqueues_total`, aggregates `by (job)`: a notifying enqueue lands on whichever
replica served the mutation, so "was a nudge emitted at all" is a deployment-level question, not a
per-replica one. Start by checking how many replicas are alerting — that alone splits the two causes
below.

Most likely causes, in order:

1. **A transaction- or statement-mode connection pooler** in front of Postgres — the cause when
   **every** replica is alerting. PgBouncer's `transaction` and `statement` modes do not support
   `LISTEN`; the writer's `pg_notify` still succeeds, so nothing else looks wrong. Point
   `[outbox].listen_database_url` at a direct or session-mode endpoint.
2. **One replica's listener is down or wedged** — the cause when only *some* replicas are
   alerting. `NOTIFY` is broadcast to every listening session, so a healthy replica's counter
   climbs regardless of what its neighbours do. Check `iam_outbox_listener_connected` (keep
   `instance`, or aggregate with `min by (job)` — never `max` or `sum`, the replicas do not
   agree) and `iam_outbox_listener_reconnects_total` for that instance.
3. ~~A dead-letter replay draining during a quiet period.~~ **No longer possible (SMA-495).** This
   used to be this alert's one false positive: not every drained row was ever notified about, because
   a replay (SMA-469, `POST /v1/outbox/dead-letters/…/replay`) returns parked rows to the live queue
   with a direct `UPDATE` that emits **no** `pg_notify`, so replayed rows wait for the poll by
   design. The evidence term is now `iam_outbox_notifying_enqueues_total`, which a replay does not
   increment, so a replay cannot satisfy this alert however quiet the deployment is. There is
   nothing to rule out here any more.

   The counterpart caveat: that counter is incremented **pre-commit** (the outbox has no post-commit
   hook), so a window in which every mutation *rolls back* climbs it while delivering nothing. The
   `iam_outbox_relay_drained_total` term is retained precisely to absorb that — nothing commits, so
   nothing drains, and the alert stays silent. See also the full-notification-queue note below,
   which is one way to reach exactly that state.

`[outbox].wake_on_commit = false` is **not** a possible cause: with the flag off no listener is
spawned at all, so `iam_outbox_listener_notifications_total` is never registered, `increase()`
over the absent series returns an empty vector, and this alert is structurally silent rather than
firing. Since SMA-495 there is a second structural reason: with the flag off the writer never emits a
notification, so `iam_outbox_notifying_enqueues_total` is never registered either, and the alert's
third term is empty as well.

**One deploy-ordering caveat.** `ops/` and the IAM binary ship separately. Until at least one
replica per job runs a binary emitting `iam_outbox_notifying_enqueues_total`, that term is an empty
vector and this alert is silent — correct once the binary lands, but a blind window if the rules go
first. Deploy the binary before these rules, and roll the rules back together with it. (The service
does log an `info` line at boot when the nudge is disabled — that, not this alert, is where a
disabled nudge shows up.)

**If IAM mutations are also failing at commit** with an opaque backend error, suspect a full
async notification queue — a listening session that stopped consuming prevents Postgres from
truncating it. Check `SELECT pg_notification_queue_usage();` (1.0 means full). Setting
`[outbox].wake_on_commit = false` and restarting stops the writer emitting notifications and
restores mutations immediately.

**Preventing that wedge.** A listener whose socket goes half-open leaves the *server* believing
the session is alive and still `LISTEN`ing, so the queue cannot be truncated. Client-side TCP
keepalives do not help — they let the client notice, not the server reap. The lever that works is
the server-side GUC family, set per deployment through the listener's own DSN (all three are
`PGC_USERSET`, so no elevated privilege is needed):

```toml
[outbox]
listen_database_url = "postgres://…?options[tcp_keepalives_idle]=30&options[tcp_keepalives_interval]=10&options[tcp_keepalives_count]=3"
```

This is deliberately not hardcoded: a startup `options` parameter is rejected outright by
PgBouncer and unsupported by RDS Proxy and Supavisor, so baking it in would turn "no nudge behind
this pooler" into "the listener never connects at all".

### `IamPolicySnapshotReloadsStalled` — the policy snapshot has not installed a reload (critical)

**Meaning.** `(sum by (job, instance) (increase(iam_authz_policy_snapshot_reloads_total{outcome="installed"}[10m]))
or (up{job="iam"} == 1) * 0) == 0` for 5m — 15 minutes of total detection (a 10m window that has to
reach zero, plus a 5m debounce) with no `PolicySnapshot` reload **installed** on the target named
in the alert's labels, even though that target is still being scraped. This is the telemetry SMA-470 added
specifically because nothing else in this catalog would have caught the defect it fixed: the
snapshot's TTL backstop (`spawn_reload`'s `ttl_elapsed` branch, `authz.policy_cache_ttl_secs`,
default 30s) is supposed to force an unconditional reload-and-install every TTL **regardless of
whether the Redis-backed `policy_gen` counter visibly moved** — that is precisely the mechanism
that makes a role revocation take effect even during a Redis outage (see "Authz availability
posture" below). If that mechanism regresses — silently reverting to requiring `r#gen` to
strictly advance before installing, say — reloads keep recompiling and discarding forever
(`outcome="rejected"` climbing, `outcome="installed"` flat), and a revoke committed during a Redis
outage is never picked up, with **no other symptom**: decisions keep flowing, latency looks
normal, and `iam_authz_decisions_total{cache="bypass"}` (the one adjacent existing metric) only
reflects the decision-cache's own bypass behavior, not backstop health.

**Why the expression looks like that.** A naive `sum(increase(...{outcome="installed"}[15m])) == 0`
is wrong in three separate ways, and each part of the shipped expression answers one of them.

*The absent-series trap.* If `outcome="installed"` has **never** been emitted at all — the
totally-broken-backstop scenario this alert exists to catch, or simply a replica that just booted —
that label combination has no time series in Prometheus yet, and `increase()`/`sum()` over a
nonexistent series return an **empty** vector, not a 0-valued one; comparing empty `== 0` is also
empty, so the rule would evaluate to nothing and never fire. The `or` branch supplies the missing
zero so the comparison always has something to evaluate.

*The masked-replica trap.* A bare `sum()` drops `job` and `instance`, so one healthy replica's
installs keep the fleet-wide total non-zero while another replica's backstop is wedged — and a
wedged replica is still serving revoked grants. `sum by (job, instance)` makes the alert per
target, which is also why the fallback is `(up{job="iam"} == 1) * 0` rather than `vector(0)`: the
`* 0` drops `__name__` and leaves a 0-valued series carrying exactly `{job, instance}` for every
**live** iam target, matching the left side's label set so `or` composes them per target. A target
that is **down** is deliberately excluded — `TargetDown` already pages for it, and an unlabelled
`vector(0)` fallback would page twice for one fault.

*The detection-time trap.* `increase(...[15m])` cannot reach zero until 15 minutes after the last
install, so pairing it with `for: 15m` pages roughly 30 minutes late — twice what this section
claims. `[10m]` + `for: 5m` is 15 minutes total while keeping a 5m debounce against a scrape gap or
a rolling restart. There is no false-positive risk against normal boot: the backstop installs every
`authz.policy_cache_ttl_secs + authz.refresh_interval_secs` (~31s at the defaults), so 10 minutes
of silence is already ~19 missed cycles.

**Likely causes:** the monotonic-write guard (`install_if_fresher`'s `load_seq` ordering) has
regressed to requiring the Redis-sourced generation to strictly advance, so a same-generation
backstop recompile is rejected every time (`outcome="rejected"` climbing while `installed` stays
flat); every `load_and_compile` call is erroring (`outcome="failed"` climbing) — most likely
Postgres is unreachable (policies/grants live there, not in Redis, so this is NOT the Redis
fail-open case) or a malformed policy/template row is aborting Cedar compilation on every attempt;
or the `spawn_reload` background task itself panicked/exited without bringing down the process
(mirrors `IamOutboxRelayStalled`'s and `IamAuditPartitionMaintenanceStalled`'s failure shape).

**Confirm:**
1. Break down by `outcome` — `sum by (outcome) (rate(iam_authz_policy_snapshot_reloads_total[15m]))`
   — to see whether reloads are happening at all and, if so, which outcome dominates.
2. If `failed` is climbing: check IAM logs for `policy_snapshot: reload failed; keeping the
   last-good snapshot` (the `spawn_reload` loop's own `warn!`), and confirm Postgres connectivity
   (the load reads `PolicyStore::list_all`/`RoleGrantStore::list_all` — a Redis outage alone must
   **not** cause this given SMA-470's fail-open design, so a `failed` outcome during a Redis
   outage points at Postgres specifically, or a genuinely malformed policy row rather than the
   generation counter).
3. If `rejected` dominates and `installed` is flat: this points at the monotonic-write guard
   itself regressing — check `install_if_fresher`'s `seq > state.installed_seq` comparison hasn't
   been reverted to compare `CompiledPolicies::r#gen` instead (the SMA-470 D-B defect this task's
   test suite guards against).
4. Verify the IAM process is actually up (`up{job="iam"} == 1`) — if it's down instead, this is
   really a `TargetDown` situation.

**Remediation:**
- `failed` dominant, Postgres-side: restore Postgres connectivity/health; the last-known-good
  snapshot keeps serving decisions throughout (never poisoned by a transient load error), so this
  is not an outage of authorization itself, only of revocation freshness.
- `failed` dominant, malformed-row-side: find and fix/remove the offending policy/template row
  (Cedar compile errors typically name the policy id); until fixed, every reload attempt keeps
  failing and no grant/revoke made after the row was introduced takes effect.
- `rejected` dominant with `installed` flat: this is a code regression of the monotonic-write
  guard, not an operational condition — no config change or restart fixes it; revert/patch the
  guard and redeploy.
- A process restart clears a wedged `spawn_reload` task the same way it clears a wedged outbox
  relay or partition-maintenance task (the snapshot rebuilds fresh via `PolicySnapshot::new` on
  boot).

See "Authz availability posture" below for the full design this alert protects (fail-open on
Redis outage and what it really costs, content-keyed decision invalidation, and the TTL
backstop that is the actual revocation guarantee).

### `IamAuditPartitionMaintenanceStalled` — audit partition maintenance is not ticking (warning)

**Meaning.** `sum without (result) (increase(iam_audit_partition_maintenance_ticks_total[2d])) == 0`
for 1 hour — no successful **or failed** tick in ~2 days from the audit partition-maintenance task
(`PgPartitionMaintainer`, SMA-467), even though the IAM process is up. Each tick does two independent units of work — see
"Audit retention & partitioning" below for the full design — so a stall here means **neither**
create-ahead nor pruning is happening. Unlike `IamOutboxRelayStalled`, this is `warning` rather
than `critical`: a stalled maintenance task never fails or blocks a live audit insert (the
`*_default` partitions backstop writes indefinitely), so the immediate blast radius is slow
index/table bloat and a stuck create-ahead horizon, not an outage.

**NOTE — `audit.retention.enabled = false` makes this alert go SILENT, not fire.** When
`[audit.retention].enabled = false`, IAM does not spawn the maintenance task at all
(`main.rs`), so `iam_audit_partition_maintenance_ticks_total` is never incremented and the
series does not exist. `increase()` over an absent series returns empty, so the alert has
nothing to evaluate and stays silent for as long as the config stays that way. **Disabling
retention is therefore unalerted** — nothing will tell you that create-ahead and pruning have
stopped, and there is **no metric-based fallback signal either**: `iam_audit_default_partition_rows`
is only set inside the same gated task (`pg_partition_maintainer.rs`), so it is equally absent from
`/metrics` when retention is disabled. The only signal is a one-time startup log line —
`audit.retention.enabled = false — no partition create-ahead or pruning will run; the DEFAULT
partitions will fill over time and can block create-ahead until manually reattached (see RUNBOOK)`
(`main.rs:264`). If you rely on retention being on, assert `[audit.retention].enabled` at deploy
time rather than expecting this alert — or any metric — to catch it.

**Likely causes:** the maintenance task
panicked/exited without bringing down the process; the task is stuck on a long-running or
lock-contended DDL statement despite the per-op `lock_timeout` back-off (Postgres itself may be
unhealthy); or the shutdown-watch fired unexpectedly and the loop exited but the process didn't
restart.

**Confirm:**
1. Confirm `[audit.retention].enabled` is true — if it is false this alert cannot be firing at all, so
   you are looking at the wrong alert.
2. Verify the IAM process is actually up (`up{job="iam"} == 1`) — if it's down instead, this is
   really a `TargetDown` situation.
3. Check `iam_audit_default_partition_rows` — nonzero or climbing corroborates that create-ahead
   has fallen behind for real. Note this gauge only refreshes on a *successful* tick, so once the
   task is fully stalled the gauge **freezes rather than climbs** — a flat gauge does not by itself
   rule out a stall; the tick counter (this alert) is the primary signal, the gauge is secondary.
4. IAM logs for a panic, an unhandled error in the maintenance loop, or the task's own `warn`s
   (`audit partition create-ahead failed`, `audit partition prune failed`) around the time ticks
   stopped.

**Remediation:**
- If the task has genuinely wedged, a **process restart** of `paigasus-iam` is the fastest
  recovery — the task re-spawns on boot, runs an awaited startup tick immediately, then resumes its
  normal interval.
- If a Postgres-side lock/hang caused the stall, resolving that may let the existing task recover
  without a restart — check whether ticks resume before restarting the service.
- If a `*_default` partition has accumulated rows while the task was down, restarting the task
  alone does **not** move those rows back into a proper monthly leaf — see "Audit retention &
  partitioning" below for the manual reattach procedure.

### `IamOutboxRetentionStalled` — the outbox retention sweep is not ticking (warning)

**Meaning.** `(sum by (job, instance) (increase(iam_outbox_retention_ticks_total[6h])) or
(up{job="iam"} == 1) * 0) == 0` for 2h — no `PgOutboxMaintainer` sweep tick in ~6 hours on the
named target, even though the IAM process is up. The window is scaled to *this* task's own hourly
default (`[outbox.retention].interval_secs = 3600`) — it is deliberately **not** copied from
`IamAuditPartitionMaintenanceStalled`'s `[2d]` above, which matches the audit maintainer's *daily*
interval; reusing that window here would tolerate ~48 consecutive missed ticks before paging. The
`or (up{job="iam"} == 1) * 0` fallback mirrors the two alerts above it for the same reason: without
it, a replica that spawned the maintainer but never completed a single tick emits no series at
all, `increase()`/`sum()` over that absent series is empty, and `empty == 0` is also empty — the
alert would stay silent exactly when things are worst.

**`for: 2h` exceeds the sweep's own hourly tick interval on purpose — this is not a copy-paste of a
bigger number for safety margin.** `main.rs` runs an awaited startup tick before the maintainer's
first `interval_secs` sleep, so a freshly booted replica's counter gets its first sample at boot
and its second an hour later. `increase()` over a single sample is `0` (there is no earlier point
in the window to diff against), so the condition is already true at `t=0` — a perfectly healthy
replica goes pending the instant it boots, regardless of hold length. A `for: 1h` hold would then
need the second hourly tick to land *and* be scraped before the hold elapses — a photo finish
against a 3600s interval that any tick jitter or scrape delay turns into a page against a healthy
replica, on every restart. `for: 2h` gives the second tick a full extra cycle of slack to clear the
condition well before the hold elapses. Worst-case detection is therefore ~8h (6h window + 2h
hold) — an accepted trade-off for a sweep whose failure mode is slow, unbounded table growth, not
something that needs minute-scale paging.

**Unlike its audit-retention sibling, `[outbox.retention].enabled = false` does NOT silence this
alert — that difference is deliberate and worth internalizing before you page on it.** Setting
`[audit.retention].enabled = false` stops `PgPartitionMaintainer` from being spawned at all, so its
tick counter never exists and `IamAuditPartitionMaintenanceStalled` goes quiet for as long as that
config holds. `PgOutboxMaintainer` does not work that way: it is spawned **unconditionally**
regardless of `[outbox.retention].enabled`, and `enabled = false` only skips the two `DELETE`
steps inside `tick` — the tick itself still runs on `interval_secs`, still refreshes
`iam_outbox_parked_rows` (the dead-letter backlog gauge), and still increments
`iam_outbox_retention_ticks_total` either way. So if this alert fires, `enabled = false` is never
an innocent explanation for it — silence here always means the maintainer task has actually
stopped doing anything at all, ticking included.

**Likely causes:** the maintainer task panicked/exited without bringing down the process; the task
is stuck on a long-running or lock-contended `DELETE` against `event_outbox` (contention with the
*relay* is structurally impossible — the sweep predicates and the relay's poll predicate are
provably disjoint, per the module's own doc comment — but contention from a long-running manual
operator query against the same table is not); or the shutdown-watch fired unexpectedly and the
loop exited but the process didn't restart.

**Confirm:**
1. Verify the IAM process is actually up (`up{job="iam"} == 1`) — if it's down instead, this is
   really a `TargetDown` situation.
2. Check `iam_outbox_parked_rows` — frozen rather than moving corroborates a genuinely stalled
   tick, since that gauge is refreshed as the last step of every tick, successful or not.
3. IAM logs for a panic, an unhandled error, or the tick's own `warn!`s (`outbox published-row
   sweep failed; will retry next tick`, `outbox parked-row sweep failed; will retry next tick`,
   `outbox parked-row gauge query failed`) around the time ticks stopped.
4. Check for long-running/blocked queries against `event_outbox` on the Postgres side
   (`pg_stat_activity`).

**Remediation:**
- If the task has genuinely wedged, a **process restart** of `paigasus-iam` is the fastest
  recovery — the maintainer re-spawns on boot and resumes on its normal `interval_secs` cadence;
  `event_outbox` rows are durable in Postgres, so nothing is lost by restarting.
- If a Postgres-side lock/hang caused the stall, resolving that may let the existing task recover
  without a restart — check whether ticks resume before restarting the service.
- **`DELETE` alone does not shrink `event_outbox`'s on-disk footprint.** Deleting rows only marks
  their space reclaimable; autovacuum reclaims it asynchronously into the table's free space map
  (available for future inserts, not a smaller file on disk). After a long stall, the first sweep
  once ticking resumes may need to retire an unusually large accumulated backlog — bounded per tick
  by `batch_size * max_batches_per_tick`, so a huge backlog drains over several ticks rather than
  in one. If disk pressure is acute, a manual `VACUUM event_outbox;` reclaims space faster than
  waiting on autovacuum's own schedule; `VACUUM FULL event_outbox;` reclaims it fully but takes an
  **exclusive lock**, so run it only in a maintenance window, never casually against a live table.

### `IamOutboxRetentionErroring` — the retention sweep had a recent error (warning)

**Meaning.** `increase(iam_outbox_retention_ticks_total{result="error"}[6h]) > 0` for 2h — at
least one `PgOutboxMaintainer` tick has errored on the named target in the last 6 hours. This is
a **different failure mode from `IamOutboxRetentionStalled` above, and the two are deliberately
separate alerts**: that one sums `result="ok"` and `result="error"` together, so it can only tell
you the maintainer has stopped ticking at all — a sweep that ticks every hour but fails one of
its steps *every single time* looks perfectly healthy to it (the tick count keeps advancing).
Before this alert existed, the only signal for that failure mode was a `warn!` log line nobody
was necessarily watching, while `event_outbox` grew without bound underneath it.

**`enabled = false` does NOT cause this alert — internalize that before you page on it.** Setting
`[outbox.retention].enabled = false` only skips the two `DELETE` steps inside `tick`; the tick
still runs, still refreshes `iam_outbox_parked_rows`, and still reports `result="ok"` (see
`PgOutboxMaintainer::tick`: `errored` is only ever set by an actual failure, never by the
`enabled` flag). So a firing `IamOutboxRetentionErroring` always means a **real** failure in one
of the tick's steps — the published-row sweep, the parked-row sweep, or the parked-row gauge
query — never an operator's intentional pause.

**`for: 2h` is a time-to-page bound, not proof of repeated failure — read this before assuming a
firing alert means "erroring for 2+ hours."** `increase(...[6h]) > 0` cannot distinguish a single
error tick that already recovered from one still recurring: once any error lands in the 6h
window the condition stays true for nearly the whole window regardless of what happens
afterward, so `for: 2h` only delays the page by up to 2 hours relative to the first error — it
does not require a second one. A genuinely one-off, already-resolved blip can still trip this
alert around the 2-hour mark and will self-resolve a few hours later once that sample ages out of
the 6h window; a sweep erroring on every tick (the case this alert exists for) will still be
erroring when you look, and keeps re-firing until it's fixed.

**Likely causes:** a Postgres-side permission or connectivity problem specific to `event_outbox`
(so other queries against other tables keep working while this fails); a lock/statement-timeout
being hit repeatedly on the same `DELETE`/count query (contention from a long-running manual
operator query against `event_outbox`, most plausibly); or a `published_days`/`parked_days` value
large enough to overflow `DateTime`'s representable range (`PgOutboxMaintainer::cutoff` degrades
that to a logged, counted error rather than a panic — see its doc comment).

**Confirm:**
1. IAM logs for the tick's own `warn!`s — `outbox published-row sweep failed; will retry next
   tick`, `outbox parked-row sweep failed; will retry next tick`, `outbox parked-row gauge query
   failed` — which pinpoint which step is failing and (via the logged `error`) why.
2. Check `iam_outbox_parked_rows` — if the gauge query is the failing step, this value goes stale
   rather than merely growing, which narrows the cause.
3. Check for long-running/blocked queries or a permissions change against `event_outbox` on the
   Postgres side (`pg_stat_activity`, recent `GRANT`/`REVOKE` history).
4. Confirm `[outbox.retention].published_days`/`parked_days` are the values you expect — an
   accidental near-`u32::MAX` value degrades to a logged `Overflow` error every tick, which reads
   identically to a Postgres-side failure in this alert alone; the log line names which cutoff
   overflowed.

**Remediation:**
- Fix the underlying Postgres-side issue (restore connectivity/permissions, resolve the blocking
  query); the maintainer needs no restart to recover — the very next tick tries again on its
  normal `interval_secs` cadence.
- If a misconfigured `published_days`/`parked_days` is the cause, correct it in `iam.toml` /
  `IAM_OUTBOX__RETENTION__*` and redeploy.
- Once fixed, expect this alert to keep firing for up to ~6 hours after the last error tick
  (until that sample ages out of the window) even though the sweep is healthy again — that is
  the same window-driven lag `IamOutboxRetentionStalled` has, not a sign the fix didn't take;
  corroborate with `iam_outbox_parked_rows` moving again and a fresh absence of the `warn!` lines
  above.

### `IamOutboxDeadLetterBacklog` — dead letters are awaiting an operator (warning)

**Meaning.** `max by (job) (iam_outbox_parked_rows) > 0` for 1h — at least one `event_outbox` row
has sat parked for over an hour and nobody has replayed or discarded it. This complements
`IamOutboxEventsParked` above: that alert fires the moment something *just* parked (a 15-minute
increase window); this one fires when a parked backlog has gone **unattended** for an hour,
regardless of when the rows originally parked.

**`max by (job)` is required here, not cosmetic — a bare `sum()` panel or alert would be wrong by
a factor of the replica count.** Every IAM replica runs its own `PgOutboxMaintainer`, and every
replica's sweep queries the *same* `event_outbox` table and sets the *same* global parked-row count
on its own gauge — so N replicas emit N **identical** series for one underlying fact, not N
independent counts to add together. `sum()` across them would report N× the real backlog, and a
bare `iam_outbox_parked_rows > 0` with no aggregation at all would page once per replica for a
single condition. The Grafana backlog panel (§3) uses the identical `max by (job)` aggregation for
the same reason — see §2.2's metric-catalog entry too.

**Likely causes:** the same causes as `IamOutboxEventsParked` above, just left unresolved for over
an hour — either a genuine payload/writer bug nobody has triaged yet, or a since-resolved outage
whose parked backlog nobody has bulk-replayed.

**Confirm:** `GET /v1/outbox/dead-letters?limit=200` (Root-only) to list what's actually parked.
`list` has no ordering knob — it is always **newest first** (`ORDER BY id DESC`), keyset-paginated
via `cursor`/`next_cursor`; to find the OLDEST parked rows, page forward with `cursor` until the
response is shorter than `limit` (the last page), or narrow with `parked_from`/`parked_to` and
inspect the tail of that window. Use the break-glass SQL under `IamOutboxEventsParked` above
(`ORDER BY occurred_at`) if the HTTP API itself is unreachable.

**Remediation:** see the full remediation playbook under `IamOutboxEventsParked` above — replay a
single row, bulk-replay a filtered set (`max_rows` required), or discard with a recorded
reconciliation plan. This alert exists purely to make sure a parked backlog doesn't sit forgotten;
it does not introduce a different recovery path from the one documented there.

### `IamGrpcHighErrorRate` — elevated non-OK gRPC status ratio (critical)

**Meaning.** More than 5% of IAM's gRPC responses (over a 5m window, sustained for 10m) carried a
non-`ok` `grpc_status`. This alert exists as a **separate** rule from `IamHighErrorRate` because
gRPC failures are carried in a response **trailer** (`grpc-status`), not the HTTP status line —
every gRPC response is HTTP `200` at the transport level, so `IamHighErrorRate`'s HTTP 5xx ratio
is structurally blind to gRPC failures. `iam_grpc_requests_total{grpc_status!="ok"}` is the only
place this shows up. The gateway's `IntrospectApiKey`/`IsAuthorized` calls into IAM are the
hottest gRPC path in normal operation, so this alert is often the earliest signal of an IAM-side
problem affecting the gateway.

**Likely causes:** a spike in legitimate `permission_denied` (denial volume, not a bug),
`unauthenticated` (bad/expired credentials being presented), `unavailable` (IAM's own
dependencies — Postgres, Redis — degraded), or `internal` (a genuine bug/exception).

**Confirm:** break down by `grpc_status` and `service`/`method` — 
`sum by (grpc_status, service, method) (rate(iam_grpc_requests_total{grpc_status!="ok"}[5m]))` —
to see which status dominates and on which RPC.

**Remediation:** status-dependent — `unavailable` points at IAM's own Postgres/Redis health
(check those services first); `internal` needs IAM log correlation to find the underlying
exception; a `permission_denied`/`unauthenticated` spike may be legitimate (see
`IamDenialAuditDrops` above for the audit-trail angle) or may indicate a misbehaving/compromised
client that should be investigated/revoked.

### `IamAuthzRedisCacheBypassed` — authz is bypassing the Redis decision cache (critical)

**Meaning.** `sum(rate(iam_authz_decisions_total{cache="bypass"}[5m])) > 0` sustained for 10m —
authz has been computing decisions with the decision cache **bypassed entirely**. That label is
emitted on exactly one condition (`cedar_authorizer.rs` step 3): the Redis-backed
entity-generation counter read **errored**, so no cache key could be built.

**Mind the arithmetic before you reason about duration.** `for: 10m` on a `rate(…[5m])`
expression does *not* mean ten straight minutes of bypassing. `rate()` keeps returning `> 0` for
a full 5m after the **last** bypass sample falls inside its window, so a bypass window of only
**~5 minutes** already satisfies `for: 10m`. Read a firing alert as "the Redis backend has been
unhealthy for at least ~5 minutes", not ten. That is still far longer than a single failover
blip, which is all the `for:` clause is there to filter — the 10m value is deliberate and stays.

Decisions throughout remain **correct** — they are computed against the in-memory snapshot
compiled from **Postgres**, which is the authoritative policy set (see "Authz availability
posture" below) — and, since SMA-473 capped the reconnect retry budget, **fast**: ~0.2–0.6 s per
decision and ~0.3–0.8 s per authz-mutating request, up to ~1.2 s for a gated cross-principal
decision, against ~19–28 s, ~28 s and ~38–57 s respectively before the cap. This alert exists
precisely *because* those two facts mean nothing else
will tell you: correct, prompt answers produce no error-rate signal and no client timeouts.

**NOTE — `authz.cache.backend = "memory"` makes this alert go SILENT, not fire.** On the memory
backend the generation counters are in-process and can never fail to be read, so `cache="bypass"`
is never emitted and the series does not exist at all. `rate()`/`sum()` over an absent series
return an **empty** vector, the `> 0` comparison has nothing to evaluate, and the rule stays quiet
forever. Memory is also the **default** backend. This is the same trap as
`audit.retention.enabled = false` for `IamAuditPartitionMaintenanceStalled`: assert the backend at
deploy time rather than expecting the alert to tell you which one you are running.

**Likely causes:** Redis is down or unreachable (process stopped, container gone, network
partition); credentials or TLS were rejected (`authz.cache.redis_url` wrong or rotated); a
proxy/failover in front of Redis is refusing connections; Redis is unresponsive enough to blow the
500 ms `response_timeout` (a fork/save stall, a long-running command, heavy `maxmemory` pressure);
or Redis 7 client eviction (`maxmemory-clients`) dropped IAM's connection.

Note what is *not* in this list: **`maxmemory` rejecting or evicting keys**, neither of which can
fire this alert. The read behind `cache="bypass"` is normally a plain `GET`
(`Generations::read`), which is `readonly fast` and **not** `denyoom` — it keeps succeeding at `maxmemory` even under
`noeviction`, where it is the `INCR` that bumps the counter which gets `OOM command not
allowed`, and a failed bump is swallowed (see "Revocation freshness" below), never bypassed.
**Since SMA-474 there is one exception:** when the read detects a rewind it issues a repairing
`INCRBY`, which *is* `denyoom` — so under `maxmemory` pressure the repair can be rejected. That
does not bypass either: a rejected repair falls back to a process-local generation and is
counted as `iam_authz_generation_rewinds_total{outcome="repair_failed"}`. An *evicted* counter
is likewise a **missing** key, which no longer reads back as a silently wrong `0` — it is
detected and repaired, and `IamAuthzGenerationRewound` fires. Both are real failure modes, just
not this one; see the `maxmemory-policy` mandate below.

**Blast radius while firing.** There is no decision cache and no entity-slice cache, so every
decision pays a raw Postgres entity-slice load; a revoke's `policy_gen` bump is swallowed, so
revocation freshness falls back to the TTL backstop (~31 s at the defaults). If the *same* Redis
also backs `api_keys.introspect_cache`, cross-replica API-key revocation stops being global and
degrades to per-replica TTL; and if it also backs `authn.jwks_cache.backend = "redis"` — which is
fail-closed by design — **every token-authenticated request 503s** for the duration (API-key
authentication fails open onto Postgres and keeps working). That last shape is
the one that *will* also trip `IamHighErrorRate`/`IamGrpcHighErrorRate`. (Each cache has its own
`redis_url`, so a deployment that splits them may see only a subset of this.)

**Confirm:**
1. Verify the IAM process is actually up (`up{job="iam"} == 1`) — if it is down instead, this is
   really a `TargetDown` situation.
2. Compare `sum(rate(iam_authz_decisions_total{cache="bypass"}[5m]))` against
   `{cache="hit"}`/`{cache="miss"}` — an all-bypass mix is a total Redis outage, a partial one
   points at intermittent connectivity or a failover in progress.
3. Check IAM logs for `cedar_authorizer: entity generation counter unreadable` and, if a Redis
   JWKS cache is configured, `redis jwks cache error`.
4. Reach Redis directly from the IAM host to separate "Redis is down" from "IAM cannot reach a
   healthy Redis". Do **not** paste `authz.cache.redis_url` into `redis-cli -u` — that URL
   carries the password, and a `-u` argument lands in shell history and in every `ps` listing
   on the box. Pass the secret out of band and only the non-secret parts on the command line:
   `REDISCLI_AUTH="$REDIS_PASSWORD" redis-cli -h <host> -p <port> PING`.
5. **Check Postgres connection-pool headroom** — this is the failure mode SMA-473 newly
   enables. Every bypassed decision pays a raw entity-slice load against Postgres, and the old
   19–28 s retry stall was also an accidental ~50× throttle on how fast those loads could be
   issued. With the cap in place there is no throttle: during an outage the *uncached* decision
   rate hitting Postgres is your **full** request rate. Nothing here is silent — pool saturation
   surfaces as 5xx and trips `IamHighErrorRate` — but if this alert and `IamHighErrorRate` are
   firing together, suspect the pool before you suspect a second, independent fault. Watch pool
   acquire waits/timeouts and Postgres `pg_stat_activity` counts, and size or shed accordingly
   until Redis is back.

**Remediation:** restore Redis — that is the only fix. There is **no config-only workaround**:
`authz.cache.backend = "memory"` removes the dependency but is **single-replica only** (its caches
and counters are per-process, so two replicas would never invalidate each other), and switching
backends needs a redeploy anyway. If the cause is memory pressure rather than an outage, relieve
it *and* check `maxmemory-policy` per the mandate below — under `allkeys-*` the counters will have
been rewinding, which since SMA-474 shows up as `IamAuthzGenerationRewound` rather than passing
silently. Note that memory pressure also rejects the repairing `INCRBY` itself
(`outcome="repair_failed"`), so a fleet under sustained pressure loses cross-replica cache
sharing until it is relieved. Nothing about the decision path needs manual repair afterwards:
the caches repopulate on their own, the snapshot recovers on generation *inequality*, and a
rewound counter is repaired forward automatically. The one exception is
`iam_authz_generation_rewinds_total{outcome="ceiling"}` — a counter within a factor of two of
`i64::MAX`, which cannot be repaired further.

<a id="ceiling-remediation"></a>
**Ceiling remediation (manual, three steps — all three are required):**

1. Delete every key in the `iam:authz:slice:*` and `iam:authz:dec:*` namespaces. Do this
   **first**: it is what makes step 3 safe, because the fleet comes back at generation `0` into
   an empty key space.

   **`DEL` does not expand globs.** `DEL iam:authz:slice:*` deletes a key *literally named*
   `iam:authz:slice:*`, finds nothing, and reports success — which is worse than failing, because
   step 3's safety argument assumes the namespaces are empty. Iterate with `SCAN` and delete in
   batches with `UNLINK`, which frees the keys on a background thread rather than blocking the
   server:

   **Target the configured Redis explicitly.** A bare `redis-cli` talks to `127.0.0.1:6379`
   db `0`, so on a jump host it will happily sweep — or fail to sweep — the wrong instance while
   reporting success. Bind host, port and database on every command, exactly as the confirm step
   for `IamAuthzRedisCacheBypassed` above does, and note that the `xargs` child needs the same
   flags as the scan:

   ```bash
   # Same host/port/db as the confirm step above. Do NOT paste authz.cache.redis_url into -u.
   RC="redis-cli -h <host> -p <port> -n <db>"
   export REDISCLI_AUTH="$REDIS_PASSWORD"

   $RC --scan --pattern 'iam:authz:slice:*' | xargs -r -n 500 $RC UNLINK
   $RC --scan --pattern 'iam:authz:dec:*'   | xargs -r -n 500 $RC UNLINK
   ```

   Then re-run both `--scan` commands and confirm each returns nothing before moving to step 2.
   Never use `KEYS` for this — it blocks the server for the whole sweep.

   **The sweep does not have to be atomic, and the replicas do not have to be stopped.** `SCAN`
   is not a snapshot, so a live replica can write a cache key between the final scan and step 2 —
   but it cannot write one that matters here. A ceiling-state replica is serving its *own*
   process-local generation (a value near `i64::MAX/2`), so every key it writes during the window
   lands in that key space, never in generation `0`'s. The fleet returns to `0` only after step 3
   restarts it, at which point those stragglers are in a disjoint space and simply age out at
   their TTL. Quiescing the fleet would convert a rare, non-urgent condition into an authz outage
   for no correctness gain, which is the opposite of the posture in "Authz availability" above.
2. `SET iam:authz:policy_gen 0` and `SET iam:authz:entity_gen 0`.

   **Re-check both values once the roll-restart in step 3 is underway.** If any replica was *not*
   yet at the ceiling, its next read sees `0` far below its own mark, takes the repair path, and
   `INCRBY`s the counter straight back off `0`. The end state is then that value rather than `0`,
   and step 1's "comes back at generation `0`" no longer describes it. Re-run step 2 if you want
   the tidier end state.

   The value it lands on is `mark + 1000000`, which is beyond every generation **that replica**
   has observed — but that is a process-local guarantee, not a fleet-wide one. A replica whose
   mark lags the fleet maximum badly enough can in principle land inside a generation another
   replica has live entries under. That residue is the documented limit of a process-local
   high-water mark (design §3.4: the jump reduces the window by roughly six orders of magnitude,
   it does not eliminate it), and any collision it does cause is bounded by the same
   `slice_cache_ttl_secs + decision_cache_ttl_secs` window as an unrepaired rewind. That bound is
   on the *duration*; it is **not** a proof that repairing is always at least as safe as not
   repairing. A badly lagging replica can land on a generation another replica is still using,
   where the un-repaired rewound value might have been a long-dead key space — so for that
   replica, in that window, the repair can be the riskier of the two. What makes the trade worth
   taking is that the lag is sub-second for any replica serving traffic, while an un-repaired
   rewind is hazardous for every replica at once. Eliminating the residue needs a durable
   generation floor, which is SMA-475. During
   *this* procedure the exposure is smaller still, because the fleet is being restarted onto
   fresh marks anyway.
3. **Roll-restart the IAM replicas.** Not optional, and not merely hygiene. The ceiling is a
   property of each process's *own* high-water mark, which lives in memory and is untouched by
   anything you do to Redis — so after step 2 every running replica reads `0`, sees it far below
   its own mark, takes the ceiling arm again, and keeps serving its own process-local generation
   with no cross-replica cache sharing. Only a restart resets the marks to `0`. Skipping this step
   leaves the alert firing and the fleet fragmented, with Redis looking healthy.

### `IamAuthzGenerationRewound` — a Redis authz generation counter rewound (warning)

**Meaning.** `sum by (counter, outcome) (increase(iam_authz_generation_rewinds_total[15m])) > 0`
for 5m — at least one of the two Redis-backed generation counters (`iam:authz:policy_gen` /
`iam:authz:entity_gen`) was read back **below** what this process had already observed
(SMA-474). `Generations` (`adapters/authz/generation.rs`) keeps a process-local high-water mark
per counter; an observation below it is a rewind — the key was evicted (`reason="missing"`,
reading back as Redis's missing-key `0`) or came back at a lower non-zero value from a failover
to a stale replica (`reason="lower"`). This is `warning`, not `critical`, and deliberately so:
the mechanism self-heals (see Blast radius below), and — per Triage below — most of the possible
causes are entirely benign.

**Confirm:**
1. `CONFIG GET maxmemory-policy` — the `maxmemory-policy` mandate below is the single most likely
   explanation for a *repeated* firing; a `volatile-*` policy fixes it at the root.
2. Break down by
   `sum by (counter, outcome, reason) (increase(iam_authz_generation_rewinds_total[15m]))` to see
   which counter rewound, whether the repair succeeded (`repaired`), was rejected
   (`repair_failed`), or hit the ceiling (`ceiling`), and whether the key vanished (`missing`) or
   came back lower (`lower`).

**Triage the benign vs. hazardous split.** A rewind has five possible causes, and the split is
not by cause name — it is by **whether the cache key spaces died with the counter**.

*Benign (cold cache, not stale):* a `FLUSHALL`, a restart without persistence, and a failover to
an **empty** replica. All three are whole-Redis loss, so they also destroy `iam:authz:slice:*`
and `iam:authz:dec:*` — `AppState::new` wires the generations, the slice cache and the decision
cache off the same Redis connection — leaving nothing stale to re-enter.

*Hazardous (caches survive):* selective eviction of just the two generation keys under
`allkeys-*`, **and a failover to a stale but non-empty replica**. The second one is easy to miss
because it does not look like data loss at all: the replica answers normally, returns a *lower*
generation (`reason="lower"`), and still holds cache entries written under the generations it is
reporting — so a repair that lands short of clearing them can re-enter a still-live key space
(§3.4 of the design doc). Note this is the one cause that presents as `lower` rather than
`missing`, which makes the `reason` label the fastest way to spot it.

**So triage on the caches, not the cause.** Check whether `iam:authz:slice:*` and
`iam:authz:dec:*` are also empty. If they are, this was whole-Redis loss — benign. If they
survived, treat it as hazardous regardless of which cause you suspect, and read the blast-radius
paragraph below.

**Blast radius.** All three outcomes land the observing replica `REWIND_JUMP = 1_000_000`
generations past everything **that process** has observed. That is the residue described in the
triage paragraph above and in the `maxmemory-policy` mandate below: the jump reduces the chance of
landing back in a live key space by roughly six orders of magnitude, it does **not** eliminate it
— a replica that has not read the counter in a very long time (a canary held out of the load
balancer, say) can still in principle repair into the live band. Read "disjoint" below as "disjoint
from what this replica has seen", never as a guarantee against the rest of the fleet. Structural
elimination is SMA-475.

- `outcome="repaired"` — self-heals, and it is the only outcome that heals the *fleet*: the jump
  landed in Redis, so every other replica converges on it. No action needed beyond the mandate.
- `outcome="repair_failed"` — Redis rejected the repairing `INCRBY` (most likely `maxmemory`
  OOM), so nothing was written and only this replica moved. It serves a process-local
  generation: disjoint from its own recent key space (subject to the residue above), but it stops
  sharing cache entries with the rest of the fleet until Redis accepts writes again — see the
  `IamAuthzRedisCacheBypassed` remediation above.
- `outcome="ceiling"` — the repair *delta* would overflow Redis's i64 counter (the high-water mark
  is within a factor of two of `i64::MAX`), so no `INCRBY` is even attempted. The replica serves
  the same shape of process-local generation as `repair_failed` — it does **not** replay its own
  high-water mark — but unlike `repair_failed` this cannot self-heal when Redis recovers, because
  nothing about Redis is what is wrong. It needs the three-step
  [ceiling remediation](#ceiling-remediation) above, **including the rolling restart**.

**Remediation:** set `maxmemory-policy` to a `volatile-*` value (the mandate below) — that is
what stops selective eviction of the two generation keys from happening at all. A `repaired` or
`repair_failed` occurrence needs no other action beyond that; a `ceiling` occurrence needs the
[three-step manual remediation](#ceiling-remediation) above.

### `IamRedisBreakerOpen` — Redis circuit breaker is not closed (warning)

**Meaning.** `max by (job, role) (iam_redis_breaker_state{role!="jwks"}) != 0` sustained for 2m —
the per-connection Redis circuit breaker (SMA-476) for `role` (`authz`, or `api_keys` when that
cache holds its own connection) has read Open or HalfOpen for at least 2 minutes, not just a
momentary probe. `!= 0`
rather than `== 2` (open) is deliberate: the gauge legitimately reads `1` (half_open) while a probe
is in flight, and comparing on the exact open value with a `for:` clause could reset every time a
scrape happened to land during a probe. `max by (job, role)` because the gauge is **per-replica** —
every replica sets its own copy, so `sum()` would add unrelated replicas' states together.
`role="jwks"` is deliberately excluded — see `IamJwksRedisBreakerOpen` below, paged separately at
critical severity because that path is fail-closed.

**What this means for correctness: nothing.** `authz` and `api_keys` are both fail-open handles —
while their breaker is open, `RedisDecisionCache`/`SliceCache`/`RedisApiKeyCache` calls
short-circuit instantly instead of dialling, and every affected decision or lookup falls through to
Postgres. This is the same underlying condition `IamAuthzRedisCacheBypassed` above surfaces via a
decision-cache label; this alert observes it via the breaker's own state instead, and is the more
direct signal of the two.

**Likely causes — distinguish these, because the fix differs:**
- **Redis is genuinely down or unreachable** (stopped, network partition, connections refused) —
  fix Redis; nothing in IAM resolves this on its own.
- **The backend is intermittently unhealthy (flapping)** rather than cleanly down — see
  `IamRedisBreakerFlapping` below. A single firing of this alert does not distinguish a clean
  outage from a flapping one; only the transitions counter does.
- **The breaker is stuck open** — it has read non-closed for far longer than the ~6 s worst-case
  recovery bound documented above, or every `HalfOpen` probe keeps failing even though Redis itself
  looks reachable. **Check credentials before you check reachability**: redis-rs's
  `reconnect_if_io_error!` only replaces the memoized connection future on an IO-class error, so an
  `AuthenticationFailed` (a rotated password) leaves it stale forever — the breaker's classifier
  still counts that as a failure, so a stuck-open breaker from a bad credential looks identical to
  an outage from the gauge alone.

**Confirm:** `PING` Redis directly from the IAM host (see the credential-handling note under
`IamAuthzRedisCacheBypassed` above — never pass a secret-bearing URL on the command line); check
IAM logs for `redis circuit breaker open` and any authentication errors; compare
`iam_redis_breaker_transitions_total{role="<role>", to="open"}` (substitute the alert's own `role`
label) over the same window — one transition that has not since closed points at stuck-open (check
credentials), several point at flapping.

**Remediation:** restore Redis reachability if that is the cause — the breaker re-probes
automatically once it clears, no IAM-side action needed. If the cause is a rejected credential,
fixing it is **not** sufficient by itself: fix the credential (`authz.cache.redis_url` /
`api_keys.introspect_cache.redis_url`) **and** restart the process — the memoized failed connection
future is never replaced without one, so a credential fix alone leaves the breaker stuck open
indefinitely (see "A blackholed Redis is the residual" above).

### `IamJwksRedisBreakerOpen` — JWKS Redis circuit breaker is not closed, token auth is failing closed (critical)

**Meaning.** `max by (job, role) (iam_redis_breaker_state{role="jwks"}) != 0` sustained for 1m —
the JWKS Redis breaker has read Open or HalfOpen for at least a minute. Unlike the two fail-open
roles above, `RedisJwksCache` fails **closed** (unchanged posture, SMA-476 D9): every Redis error
maps to `AuthnError::Unavailable`, and `JwksProvider::key_for` consults the cache on every token
validation. So while this fires, **every token-authenticated request 503s** — API-key
authentication is unaffected, it fails open onto Postgres — for as long as the breaker stays
non-closed, plus up to the ~6 s recovery bound (see "Authz availability posture" above) after Redis
itself comes back. The short `for: 1m`, versus 2m/10m elsewhere, reflects that this is a total
outage of one auth path, not a graceful degradation: page immediately, do not wait for
corroboration from `IamHighErrorRate`.

**Likely causes, Confirm, and Remediation:** identical triage to `IamRedisBreakerOpen` above —
Redis down, a rejected/rotated credential (check this first if the breaker looks stuck open), or a
flapping backend (`IamRedisBreakerFlapping` below). The only difference is blast radius: this is a
**total token-auth outage**, not a cache bypass. There is no config-only mitigation for the outage
window itself — `authn.jwks_cache.backend = "memory"` removes the dependency entirely but needs a
redeploy and is single-replica-appropriate only, so it is not a live incident response.

### `IamRedisBreakerFlapping` — Redis circuit breaker is flapping (warning)

**Meaning.** `max by (job, role) (increase(iam_redis_breaker_transitions_total{to="open"}[10m])) >
5` — the breaker for `role` opened more than five times in the last 10 minutes. This exists because
neither breaker-state alert above can see a breaker that opens and re-closes **inside one scrape
interval**: `OPEN_DURATION` is 2 s while Prometheus scrapes every 15–30 s, so a breaker opening for
2 s every 30 s — chronically sick, exactly the condition worth catching early — is sampled at `0`
in most scrapes and neither `for:` clause above ever holds. The transitions counter is the only
artifact that survives a sub-scrape-interval open window, which is why this rule watches it instead
of the gauge (`for: 0m` — the `increase()` window already provides the debounce).

**What this means: the backend is intermittently unhealthy, not cleanly down.** Each open costs one
recovery window against whichever caller tripped it (a cache-bypass window for `authz`/`api_keys`,
a 503 window for `jwks`) — five-plus of those in 10 minutes is a materially worse user-facing
experience than one clean outage of the same total duration, because it repeats without settling.

**Likely causes:** a Redis under memory or CPU pressure that intermittently misses its
`response_timeout` (500 ms) or `connection_timeout` (1 s); a flapping network path or a
load-balancer/proxy in front of Redis; Redis client eviction (`maxmemory-clients`) repeatedly
dropping IAM's connection; or ordinary connection churn crossing `FAILURE_THRESHOLD` (3
consecutive) periodically — see `IamRedisBreakerOpen` above for why 3 is a low bar under real
concurrency.

**Confirm:** graph `iam_redis_breaker_transitions_total{role="<role>", to="open"}` (substitute the
alert's own `role` label) as a rate/increase to see the actual cadence; correlate against Redis-side
metrics (memory, CPU, connected-clients) and any proxy/load-balancer health checks in front of it;
rule out a client-eviction loop via Redis's own `CLIENT LIST` output and eviction logs.

**Remediation:** stabilize the backend (relieve memory/CPU pressure, fix the flapping network path,
address client eviction) — this is a Redis-health problem, not something to fix IAM-side. Do not
conflate this with `IamRedisBreakerOpen`/`IamJwksRedisBreakerOpen`'s "Redis is genuinely down" case:
those two triage toward "is Redis reachable at all", this one toward "why does Redis keep becoming
briefly unreachable".

### `IamHighErrorRate` / `GatewayHighErrorRate` — elevated HTTP 5xx ratio (critical)

**Meaning.** More than 5% of HTTP responses on the respective service's main router were `5xx`
over a 5m window, sustained for 10m.

**Confirm:** break down by `route` — `sum by (route) (rate(iam_http_requests_total{status_class="5xx"}[5m]))`
(swap `iam_http_requests_total` for `gateway_http_requests_total`) — to isolate which endpoint is
failing, then correlate with service logs for that route/time window.

**Remediation:** route-dependent; for the gateway specifically, cross-check
`GatewayIamDependencyUnavailable` and `GatewayUpstreamErrors` below first — a gateway 5xx spike
is very often a downstream (IAM or OpenAI) problem surfacing at the gateway's own HTTP layer,
not a gateway bug.

### `GatewayIamDependencyUnavailable` — gateway can't reach IAM (critical)

**Meaning.** `gateway_iam_calls_total{result="unavailable"}` is incrementing — the gateway's
`require_iam_auth` middleware is getting transport-level failures calling IAM's
`IntrospectApiKey`/`IsAuthorized` gRPC endpoints, which the gateway maps to a `503` (retryable) to
its own callers, deliberately distinct from `401`/`403` so client SDKs don't treat it as a fatal
auth failure.

**Confirm:** is IAM up (`up{job="iam"} == 1`)? Is the gateway's `iam.grpc_addr` correct and
network-reachable from wherever the gateway is running? Check IAM's own health
(`IamHighErrorRate`/`IamGrpcHighErrorRate`/`TargetDown`) for a root cause on IAM's side.

**Remediation:** this is fundamentally an availability-coupling issue — **every gateway request
needs IAM** (gateway-m0 design §4.1), so gateway health during an IAM outage is bounded by IAM's
own recovery. Restore IAM connectivity/health; there is no gateway-side fallback in M0 (a
combined introspect-and-authorize RPC and reduced IAM coupling are noted follow-ups, §6).

### `GatewayUpstreamErrors` — elevated OpenAI upstream 5xx ratio (warning)

**Meaning.** More than 5% of `gateway_upstream_requests_total` carried `status_class="5xx"` over
a 5m window, sustained for 10m — OpenAI itself is returning server errors to a meaningful
fraction of requests. Remember the TTFB caveat (§2.3): this only sees upstream failures that
happen at/before the response head; a mid-stream SSE error is invisible to this metric.

**Confirm/remediate:** check OpenAI's own status page / your account's usage dashboard; this is
typically not actionable on the gateway side beyond confirming it isn't a gateway-side
malformed-request pattern (check `gateway_upstream_requests_total{status_class="4xx"}` isn't
also elevated, which would point at a gateway bug instead).

### `TargetDown` — a scrape target is unreachable (critical)

**Meaning.** `up == 0` for 2 minutes — Prometheus could not scrape `/metrics` on the named
`job`/`instance` at all (process down, network partition, or the service crashed before binding
its listener). This is a coarser signal than the per-service HTTP/gRPC error-rate alerts above;
it fires even before any request-level metric exists to alert on.

**Confirm:** `up{job="iam"}` / `up{job="gateway"}` in Prometheus; is the process actually running
(`ps`/orchestrator status)? Can you reach `/metrics` manually (`curl`) from the Prometheus
container's network vantage point?

**Remediation:** restart/redeploy the affected service; investigate the crash/startup logs if it
isn't coming back up on its own.

---

### Gateway deployment posture (applies across the alerts above)

**M0 is internal-only or spend-capped.** The gateway M0 walking skeleton ships **no** rate
limiting and **no** cost/budget enforcement (both are gateway M3/M4 work) — a single
over-provisioned or leaked API key means effectively unbounded OpenAI spend. Per the gateway-m0
design (D6), M0 must run in **one** of two postures:
1. **Internal/non-production** — not reachable from untrusted networks, or
2. **Behind a hard OpenAI account-level spend cap** set at the OpenAI account/org level, so a
   worst-case abuse scenario is bounded by that cap rather than by anything the gateway itself
   enforces.

**`/metrics` network-restriction is part of this posture, not separate from it.** `/metrics` is
unauthenticated (D4, §1). For an **internal-only** deployment, same-port `/metrics` (the default,
merged onto `http_addr`) is fine — the whole listener is already network-isolated. For a
**spend-capped-but-otherwise-public** gateway, same-port `/metrics` would be reachable
**unauthenticated by any external caller** that can reach `/v1/chat/completions` — leaking
request volumes, error rates, and IAM/upstream latencies (a reconnaissance-grade disclosure)
behind nothing but fragile L7 path filtering on a shared port. **Set `[metrics].addr` to a
separate, internally-bound address** (e.g. `127.0.0.1:9091` or an address only your scrape
network can reach) for any gateway deployment that isn't fully internal-only — this is a RUNBOOK
mandate, not optional hardening. IAM, being an internal-only service in every deployment
topology described so far, does not need the separate listener, but the RUNBOOK still recommends
network-restricting its port regardless (defense in depth). mTLS on the scrape endpoint is a
further hardening step, not yet implemented (§6).

### Authz availability posture (applies to `IamGrpcHighErrorRate`, authz-related denial patterns)

**Fail-open on Redis outage, never fail-closed.** `CedarAuthorizer::is_authorized` reads a
Redis-backed entity-generation counter (`GenerationsReader::entity_gen`) to build its
decision-cache key. If that read errors, the decision cache is **bypassed entirely** for that
call — no key is built, no lookup, no cache population — and the decision is computed
**directly** against the in-memory compiled policy snapshot, which is compiled from
**Postgres**. Correctness is never compromised: the authoritative policy set never lives in
Redis. The bypass is directly observable as `iam_authz_decisions_total{cache="bypass"}`.

**A fail-closed posture is not offered — by decision, not by omission** (SMA-470 D1). Because
Redis is a pure accelerator here, denying every request during its outage would convert a
degradation into a total authorization outage. The contract is bounded-staleness fail-open, and
the bound below is what makes that defensible. There is no config knob to opt into fail-closed,
and adding one is explicitly out of scope.

**Fail-open is bounded, not free: while Redis is down, budget ~0.2–0.6 s per authz decision,
~0.3–0.8 s per authz-mutating request, and up to ~1.2 s for a gated cross-principal decision
(the table below).** That bound exists only because it was
deliberately imposed. `adapters::redis_conn::connect` is the
**single** place this service constructs the shared `ConnectionManager` (enforced by the
`repo:redis-connect-single-site` CI gate), and it caps the reconnect budget at
**`number_of_retries = 1`** — down from redis-rs's stock 6 (SMA-473). A counter read against a
stopped backend therefore fails in **~100–200 ms** instead of burning a full reconnect cycle.
`set_max_delay(500 ms)` is set alongside it as a **guard only**: `backon` applies `max_delay` to
the pre-jitter base delay and never to the *first* step (the first delay is always `min_delay`),
so at one retry it is inert — it exists so that raising the retry count later caps each step at
500 ms rather than at `backon`'s own 60 s default. `min_delay` (100 ms), `exponent_base`,
`connection_timeout` (1 s) and `response_timeout` (500 ms) are all deliberately left at the
redis-rs defaults (SMA-473 D1).

**The cost was the retry schedule, not the per-attempt timeouts** — kept here because it is why
the fix looks the way it does, and because the timeouts remain the wrong knob to reach for. In
pinned redis-rs 1.3.0 the per-attempt timeouts **were already bounded by default**:
`connection_timeout` = **1 s**, which bounds establishing a connection, and `response_timeout` =
**500 ms**, which bounds waiting for a command's response (`client.rs`'s
`DEFAULT_CONNECTION_TIMEOUT`/`DEFAULT_RESPONSE_TIMEOUT`). Both apply under whichever
`ConnectionManager` constructor is used — production's eager `new_with_config` and the tests'
lazy `new_lazy_with_config` alike, since the timeouts live in the shared
`ConnectionManagerConfig` and not in the constructor. (`connection_timeout` wraps the *whole*
connect, DNS resolution included — `client.rs:505-510` puts
`get_multiplexed_async_connection_inner` inside `rt.timeout(…)` and the resolver runs inside
that — so a hung resolver is bounded the same way a blackholed socket is.) Tightening those
would not have moved the latency. What was **not**
bounded was the reconnect **retry count and backoff schedule**: the default config sets
`number_of_retries = 6`, `min_delay = 100 ms`, `exponent_base = 2.0` and leaves `max_delay`
unset (so no per-step cap is applied beyond `backon`'s own inert 60 s default, which this
schedule never reaches), meaning a dead backend is retried on a
`100+200+400+800+1600+3200 ms` schedule — **~6.3 s per reconnect cycle as a floor**. The real delay was higher: redis-rs always enables `backon`'s
jitter, which *adds* `rand(0, delay)` to each step (`delay × 1.0–2.0`), so a cycle ran ~6.3–12.6 s
with an expected ~9.5 s — and a `ConnectionManager` burns a **full cycle per failed command**,
because the failing command only kicks off a background reconnect and the *next* command awaits a
brand-new cycle. **That "full cycle per command" behavior holds only when commands arrive faster
than a dial completes** — true throughout this table, since nothing here introduced any deliberate
gap. Since SMA-476 that is no longer the whole story: the circuit breaker described under "A
blackholed Redis is the residual" below deliberately opens a gap (2 s) far longer than any dial,
specifically so that *not every command* pays a fresh cycle — only the handful that trip the
breaker do. A single request performs several such reads (`policy_gen`, `entity_gen`,
the slice cache's own `entity_gen`, plus a post-commit bump on a mutation), which is how one
decision reached 19–28 s.

Per-path cost against a **stopped or refused** Redis, before and after the cap (spec §3.3; a
"cycle" is one failed read paying the reconnect budget):

| path | cycles | before (6 retries) | after (1 retry) |
|---|---|---|---|
| `POST /v1/authz/is-authorized`, self query, steady state | 2 | ~19 s | **0.2–0.4 s** |
| `POST /v1/authz/is-authorized`, self query, stamp still trusted | 3 | ~28 s | **0.3–0.6 s** |
| `POST /v1/authz/is-authorized`, gated (cross-principal) query | 4–6 | ~38–57 s | **0.4–1.2 s** |
| `DELETE /v1/authz/role-grants/{id}` (+ post-commit bump) | 3–4 | 28.4 s measured | **0.3–0.8 s** |
| API-key authenticated request (cache miss: `get` + `put`) | 2 | ~19 s | **0.2–0.4 s** |
| any token-authenticated request under a Redis JWKS cache (then 503) | 1 | ~6–12 s | **0.1–0.2 s** |

Two rows deserve a note. The 3-cycle row is the *provisional-stamp* distinction: while the policy
snapshot's generation stamp is still trusted, each decision's `reload_if_stale` adds its own
`policy_gen` read; once the stamp goes provisional that read stops (see "Same-replica revocation
immediacy" below), which is why steady state is 2. The **gated** row is the one the SMA-470
measurement never exercised at all: `Authorize::decide_gated`
(`application/authorize.rs`) runs a full **second** `is_authorized` whenever
`req.principal != actor`, and the acceptance test that produced the 19–28 s figures used a *self*
query — so a cross-principal check doubled every "before" number, and still lands under ~1.2 s
after. The one directly measured post-fix number is the Docker-free unit test
`api_keys::cache::tests::redis_cache_fails_open_when_the_backend_is_unreachable`, which went from
**28.403 s to 0.471 s**; it issues three Redis commands, i.e. ~9.5 s → ~0.16 s per failed command.

**A blackholed Redis is the residual, and it is now bounded by a circuit breaker (SMA-476).** Every
number in the table above assumes the TCP connect **fails immediately** — the process is stopped or
the port refuses (`ECONNREFUSED`). If the backend instead swallows SYNs (a `DROP` firewall rule, a
partitioned network, a wedged host, or the accept-and-never-reply shape `docker pause` reproduces —
see "Manual blackhole verification" below), no attempt errors early and each one runs to
`connection_timeout` instead, so one capped cycle costs **~2.15 s** per failed command (two 1 s
attempts plus the ~100–200 ms delay between them) rather than ~100–200 ms. The same 1 s bound
covers a **hung DNS resolver**, not just a dropped SYN: `connection_timeout` wraps the entire
connect including address resolution (`redis-1.3.0/src/client.rs:505-510`).

**That ~2.15 s figure is now measured, not calculated — the single most important correction in
this section.**
`adapters::redis_conn::tests::a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`
drives a real dial against a Docker-free blackholed listener and pins it directly. Three runs of one command against a Closed
breaker: **2.1531 s / 2.1540 s / 2.1523 s** — tight enough to be a floor (two ~1 s
`connection_timeout` attempts plus a jittered retry delay), not an estimate. The same test's
ten-command aggregate — three real dials that trip the breaker, then seven short-circuits — measured
**~6.46 s** (6.4616 / 6.4598 / 6.4583 s across the three runs), against **~21.5 s** for what ten
un-broken commands would cost. Both figures are the authoritative source for this section; "Manual
blackhole verification" below reproduces the *shape* by hand but is not where these numbers come
from.

**Since SMA-476, that ~2.15 s cost applies only to the failures that open the breaker, and to the
request cohort already in flight when the outage starts — not to every command.** A per-connection
circuit breaker (`adapters::redis_conn::RedisHandle`; one breaker per connection — one instance per
`RedisRole`, i.e. `authz`, `api_keys` when that cache holds its own connection, and `jwks`) now sits in front of
every Redis command:

- **`FAILURE_THRESHOLD = 3`** consecutive connection-class failures open it. Three, not one,
  deliberately — SMA-473 already capped the retry budget at `number_of_retries = 1` specifically to
  tolerate a first attempt landing in a failover gap, and opening on a single failure would defeat
  that. **Stated plainly because it surprises people: three concurrent connection failures is a low
  bar, so a routine Redis failover under load will trip this breaker.** For the fail-open caches
  (`authz`, `api_keys`) that costs one bypass window; for `jwks` — fail-closed — it means every
  token-authenticated request 503s for that window (numbers below).
- **A dropped or cancelled command does not count as a failure unless the breaker is already
  half-open.** A client disconnect means the request observed no result at all — that is not
  evidence about the backend — so counting it while Closed would let three merely-cancelled
  requests trip the breaker against a perfectly healthy Redis. In the HalfOpen state a dropped probe
  *does* count, by design: there the alternative is a wedge that never re-arms.
- **`OPEN_DURATION = 2 s`.** While open, every command short-circuits with a synthetic error in
  microseconds instead of dialling — the measured ~6.46 s-for-ten-commands figure above *is* this in
  action: commands 4–10 each cost under 100 ms once the breaker trips at command 3.
- **`HALF_OPEN_DEADLINE = 5 s`.** After the open window, exactly one probe is admitted; if the
  breaker has sat half-open longer than this (an abandoned probe), another is admitted regardless,
  so it cannot wedge open forever on a dropped future.

**Recovery costs one or two open windows depending on the shape of the outage — not a fixed two.**
An earlier draft of this document claimed recovery always costs two windows; that was wrong and has
been corrected here. `ConnectionManager::reconnect()` **spawns** the replacement dial the instant a
command fails (`redis-1.3.0/src/aio/connection_manager.rs:649`), not when the next probe asks for
one, so which window the recovering probe lands in depends on dial duration versus the 2 s window:

- **Timeout-class outage** (dial time ≳ the window — e.g. a blackholed backend's ~2.15 s dial
  against a 2 s window): the replacement dial spawned when the breaker opened is still in flight
  when the probe arrives, so the probe joins it and pays only the remainder. **Typically one
  window.**
- **Refusal-class outage** (dial time ≪ the window — e.g. a stopped/refused Redis's ~0.2 s dial):
  the replacement dial has long since resolved to a memoized `Err` by the time a probe arrives, so
  that probe consumes the stale `Err` in microseconds without touching the network, and only *then*
  spawns the dial the *next* probe will see recovered. **Typically two windows.**

Production sees both shapes — a blackhole and a refusal are different flavors of the same class of
outage — so **`recovery ≤ 2 × OPEN_DURATION + one connect budget ≈ 6 s` is the number to operate
against either way**, even though the common case is faster.

**One documented exception to that bound: a rotated Redis password can make recovery unbounded.**
redis-rs's `reconnect_if_io_error!` only spawns a replacement dial on an IO-class error
(`connection_manager.rs:402-411`); `ErrorKind::AuthenticationFailed` is not IO-class, so a stale,
failed connection future is never replaced — nothing about the mechanism above reconnects on
anything but an IO-class failure. This is **pre-existing redis-rs behaviour, not a breaker defect**
— but the breaker's classifier (SMA-476 D5) deliberately counts `AuthenticationFailed` as a breaker
failure, because redis-rs itself treats it as connection-fatal. **If you see a breaker that has been
open far longer than the ~6 s bound above, check credentials before you check reachability** — but
fixing the credential is not enough by itself: **both** fixing the credential **and** restarting
the process are required. The memoized `Err` lives in the running process's `ArcSwap`, so fixing
the password alone leaves it exactly where it was — there is no event left to trigger a fresh dial,
so a process left running against a now-correct credential waits forever. Restarting alone without
fixing the credential just reproduces the same stuck breaker after the boot dial (unaffected by the
breaker — SMA-476 D11) also fails. The breaker cannot recover from this on its own either way.

**The JWKS asymmetry, stated with numbers.** `RedisJwksCache` still fails **closed** on every Redis
error — that posture is unchanged — the breaker just makes the failure instant instead of ~2.15 s.
So while the `jwks`-role breaker is open or half-open, **100% of token-authenticated requests
503**, and that continues for up to the ~6 s recovery bound above *even after Redis itself has
recovered*, because the breaker's own window has to run its course. Combined with the failover-trip
note above: a routine failover under load now costs up to a ~6 s token-auth outage on a Redis-backed
JWKS cache, where the `authz`/`api_keys` roles pay only a cache-bypass window of the same length.
API-key authentication is unaffected either way — it fails open onto Postgres.

**Reading the breaker's metrics.** `iam_redis_breaker_state{role}` is a gauge — `0 = closed`,
`1 = half_open`, `2 = open` — set at construction as well as on every transition, so "no data"
always means a scrape or registration problem, never an unset breaker.
`iam_redis_breaker_transitions_total{role, to}` is a counter (`to` ∈ `open|half_open|closed`); it is
**not redundant with the gauge** — a breaker that opens for 2 s every 30 s reads as `0` in most
15–30 s scrapes, so the counter is the only artifact that survives a sub-scrape-interval state. Three
attribution caveats, all worth knowing before reading either metric:
- `role="api_keys"` exists **only** when the API-key cache is Redis-backed
  (`api_keys.introspect_cache.backend = "redis"` — a memory-backed one opens no connection at all,
  so it can never mint this series) **and** holds its own connection. Since SMA-485 the latter
  means `api_keys.introspect_cache.redis_url` differs from `authz.cache.redis_url` as a **string**
  (compared after trimming), or the authz cache is `memory`-backed. Ordinarily the two URLs are
  identical and the API-key cache reuses the `authz` handle, so a missing `api_keys` series does
  not mean the API-key cache is idle — check `role="authz"` instead. The comparison being textual
  cuts the other way too: two spellings of one endpoint (`…:6379` vs `…:6379/0`, a host alias, a
  differing password) produce an `api_keys` series fronting the same physical Redis, which is the
  next caveat's case arrived at by accident rather than by design.
- Two roles may front the **same physical Redis** with independent breakers, so `role="authz"` at 0
  while `role="jwks"` is at 2 does not imply two separate backends — it can be one Redis that one
  handle happened to reconnect to first.
- The gauge is **per-replica**: aggregate with `max by (job, role)`, never `sum` — summing would add
  unrelated replicas' states together into a meaningless number.

Since SMA-476, `IamRedisBreakerOpen` / `IamJwksRedisBreakerOpen` / `IamRedisBreakerFlapping` (see
the alert catalog above) are the most direct signal of everything in this subsection — keyed off the
breaker's own state rather than a decision-cache side effect. Reach for those first; the narrative
below (`IamAuthzRedisCacheBypassed`) remains accurate but is one step removed.

**Boot still fails fast — just ~50× sooner.** `redis_conn::connect` is eager
(`ConnectionManager::new_with_config` awaits the initial connection), so a Redis that is down when
IAM starts still fails `AppState::new` and the process exits, rather than coming up with a manager
that only fails on first use. What changed is the tolerance window: ~6–12 s of retries became
~200 ms. A Redis that is merely **slow to start** is therefore no longer absorbed at boot and now
costs one crash-restart. Depend on the orchestrator's restart policy or a readiness/ordering
constraint for that, not on the connect budget (SMA-473 D10).

**A boot failure naming `api_keys.introspect_cache.redis_url`, on a deployment that started fine
yesterday.** Since SMA-485 that URL is actually dialled. It was previously ignored whenever
`authz.cache.backend = "redis"`, so a wrong or unreachable value was harmless — and
`IamConfig::validate` **requires** the field, so a config written under the old behaviour is
exactly the kind likely to carry a stale or placeholder value that has never been dialled. The
process exits with `backend error` and, under "Caused by:", `api_keys.introspect_cache.redis_url
is unreachable (…)`. **Remediation:** fix the endpoint, or — to restore the previous behaviour
exactly — point `api_keys.introspect_cache.redis_url` at `authz.cache.redis_url`. The sharing
predicate compares the two after trimming, so equal-after-trimming is what it actually requires;
byte-identical is simply the version you can verify at a glance, so prefer it.
Note this dial happens *late* in `AppState::new`, after boot reconciliation and the initial
policy-snapshot compile, so each crash-loop attempt pays a full Postgres reconcile and Cedar
compile before failing — which makes the next paragraph's backoff warning sharper, not milder.

**Verify that your orchestrator actually applies restart backoff — do not assume it.** D10's
reasoning rests on restart backoff dominating the recovery time either way, and the cadence it
cites (10 s → 20 s → 40 s …) is **Kubernetes `CrashLoopBackOff`** specifically. This repo ships
**no deployment manifest for IAM**, so nothing here supplies it. Under a supervisor with a fixed
short restart delay — `Restart=always` with a small `RestartSec`, `docker run --restart=always`,
a shell loop — there is no backoff, and during a Redis outage IAM now crash-loops **far faster**
than it did before the cap (~200 ms per attempt instead of ~6–12 s). Either run under something
with exponential backoff, or add a readiness/ordering constraint so IAM does not start before
Redis is accepting connections.

**How a Redis outage announces itself now.** Correct *and* fast is exactly what makes the outage
quiet, so the old expectation is inverted: `IamGrpcHighErrorRate` and client-side timeouts will
**not** fire on the authz path, because every decision still returns the right answer in a
fraction of a second. Note the old signal was weaker than it looked, which strengthens the case
for the new alert rather than weakening it: `IamHighErrorRate` **never** carried the HTTP half of
it even before the cap. `serve_http` applies `TimeoutLayer` *outside* `app_routes`, and
`http_metrics_layer` lives *inside* `app_routes`, so a timed-out HTTP request short-circuits
before the metrics layer records anything — `iam_http_requests_total` never counts it — and the
status it short-circuits with is `408`, i.e. `status_class="4xx"`, which the rule's `5xx` filter
excludes twice over. Neither error-rate rule carried it, for the same structural reason:
`record_grpc` runs inside each RPC body, but tonic's `GrpcTimeout` wraps the routed service —
user layers included — from outside and, on expiry, drops the inner future before the handler
ever reaches it, so a timed-out gRPC call never reaches `record_grpc` either. The pre-cap authz
signal was client-side timeouts only, full stop.
The signal now is **`IamAuthzRedisCacheBypassed`**
(`sum(rate(iam_authz_decisions_total{cache="bypass"}[5m])) > 0` for 10m, critical), whose catalog
entry above carries the confirm/remediate steps and the `authz.cache.backend = "memory"` silence
trap. Still page on it rather than treating it as a background degradation: the blast radius (no
decision cache, no slice cache, revocation freshness on the TTL backstop — plus, for whichever of
`api_keys.introspect_cache` / `authn.jwks_cache` point at the *same* Redis, per-replica API-key
revocation and a fail-closed authentication path) is real even though latency no longer advertises
it. The error-rate alerts do stay relevant for exactly one shape — a Redis-backed JWKS cache,
which is fail-closed (next paragraph).

**Under a Redis JWKS cache, a Redis outage is an authentication outage, not an authz slowdown.**
With `authn.jwks_cache.backend = "redis"`, `RedisJwksCache::get` maps **any** Redis error to
`AuthnError::Unavailable` — deliberately fail-closed (`adapters::oidc::redis_cache`, spec
§4.3/D15: key material is not something to guess at) — and `JwksProvider::key_for` consults the
cache on **every** token validation. So every **token**-authenticated request (the OIDC bearer
path — API-key authentication is the next paragraph, and fails open) `503`s for the duration of
the outage, and *that* is what moves
`IamHighErrorRate`. Not `IamGrpcHighErrorRate`: gRPC bearer enforcement is `AuthEnforce`, a tower
layer that short-circuits via `reject()` (`adapters::grpc::authn::AuthEnforce::call`) without
ever calling the inner service, so a JWKS-driven `Unavailable` never reaches `record_grpc` there
either. SMA-473 makes the failure **fast** (~0.1–0.2 s instead
of ~6–12 s); it does not, and should not, make it succeed.

**Expect brief authentication 503s where you used to see nothing at all.** SMA-473 changed how
*often* this path fails, not only how fast — a consequence worth predicting rather than
discovering at 3am. The old 6-retry schedule made the shared connection future take ~6.3–12.6 s
to resolve, which meant any Redis interruption **shorter than that was absorbed**: the command
succeeded, just slowly, and authentication never noticed. The capped budget absorbs only one
~100–200 ms retry. So a routine event like a **2 s primary failover**, previously invisible here,
now `503`s every OIDC-bearer request for at least those ~2 s of actual disruption — and, since
SMA-476, for longer than that if the failover trips the breaker: three concurrent connection
failures is a low bar under load (see "A blackholed Redis is the residual" above). Two numbers,
two different starting points: the breaker itself contributes up to **~6 s of recovery lag measured
from when it opens** (the headline figure used throughout this section) — and because the breaker
does not open until three failures have accumulated, roughly ~2 s into the failover, the **total
token-auth impact measured from the start of the failover** is up to **~8 s**, not the ~6 s alone.
Use the ~6 s figure to reason about the breaker's own behavior; use ~8 s to size a client timeout or
readiness probe against the failover as a whole. A failover that stays under the breaker's
`FAILURE_THRESHOLD` (3 consecutive failures on that connection) still costs only the original ~2 s.

**None of the three original `for: 10m` rules fire on an isolated blip of that size** —
`IamAuthzRedisCacheBypassed`, `IamHighErrorRate` and `IamGrpcHighErrorRate` all need ten sustained
minutes, and even the ~8 s breaker-inflated case (measured from the start of the failover, per
above) is over in seconds. That much is unchanged from
before SMA-476. What SMA-476 adds is two narrower signals, and it is no longer accurate to say **no**
alert fires: **`IamRedisBreakerFlapping`** (`for: 0m`, counting transitions rather than requiring
sustained duration) fires if this kind of event *repeats* — five-plus breaker trips in 10 minutes,
which a genuinely flapping primary or a string of failovers can produce even though no single trip
lasts anywhere near that long; and **`IamJwksRedisBreakerOpen`**'s much shorter `for: 1m` is the
alert that *could* catch a single event, if its recovery runs long enough to approach a minute (a
slower or blackhole-flavored failover, or a breaker that does not cleanly reclose) — a clean,
isolated ~2 s failover with the typical ~6 s recovery still stays comfortably under that bound and
stays silent, same as before. That is the accepted trade (SMA-473 D6, still standing after
SMA-476): the alternative is a multi-second stall on *every* authenticated request during a real,
unbounded outage. If users report sporadic 503-then-fine authentication, correlate against Redis
failover/restart events before hunting for an IAM bug, and check
`iam_redis_breaker_transitions_total` for the same window to see whether the breaker was involved.

The default backend is `memory`, which
has no such coupling — if you run the Redis one, treat Redis as a hard availability dependency of
authentication itself and size its redundancy accordingly.

**`RedisApiKeyCache` usually shares the same connection, and sits on the hottest path.** Since
SMA-485 it reuses the `redis_conn` handle **only when `api_keys.introspect_cache.redis_url` and
`authz.cache.redis_url` are the same string** (compared textually, after trimming — see the
`iam_redis_breaker_state` caveats above). Otherwise — the two URLs differ, or the authz cache is
on `memory` — it dials its own connection from `api_keys.introspect_cache.redis_url`
(`adapters/http/mod.rs`), with its own breaker, its own reconnect state and its own
`role="api_keys"` metrics. So a single Redis outage need not hit both paths, and in the split
configuration each fails on its own schedule. Before SMA-485 that promise did not hold with authz
on Redis: the API-key URL was ignored, both caches shared one connection and therefore one
breaker, so an authz-Redis outage short-circuited API-key introspection against a Redis that was
perfectly healthy.
The gateway's
`IntrospectApiKey`/`IsAuthorized` pair is the busiest gRPC traffic in normal operation (see
`IamGrpcHighErrorRate` above), and every API-key-authenticated request reads this cache: a miss
costs a `get` **and** a `put`, while `RevokeApiKey`/`ArchiveServiceAccount` add an `evict`. Unlike
the JWKS cache this path fails **open** onto Postgres, so requests keep succeeding — but the
`evict` is what makes a revocation take effect across replicas immediately, so during an outage
cross-replica revocation stops being global and degrades to per-replica TTL
(`api_keys.introspect_cache.ttl_secs`).

**Revocation freshness is TTL-bounded; the generation bump is best-effort.** A grant/revoke bumps
`policy_gen` **after** its Postgres transaction has committed, via an awaited but
**best-effort** `GenerationsPolicyGenBumper::bump` that **logs and swallows** its own error. With
Redis unavailable the revoke therefore commits while its invalidation signal is silently lost —
the bump is an accelerator, never the guarantee.

The guarantee is the policy snapshot's **unconditional TTL backstop**: once
`authz.policy_cache_ttl_secs` (default `30`) has elapsed since the last install, the next
`authz.refresh_interval_secs` poll tick (default `1`) recompiles from Postgres and installs the
result **regardless of whether the generation counter moved**. Worst-case revocation latency is
therefore **`policy_cache_ttl_secs + refresh_interval_secs`** — **31 s at the defaults** — plus
the reload's own duration. **That bound now holds during a Redis outage too.** The retry-cycle
term that used to inflate it is gone: the reload's `policy_gen` read still fails before the
snapshot installs anyway, but since SMA-473 it fails in ~0.1–0.2 s rather than burning a
~6–12 s reconnect cycle, and the same is true of the second read that used to compound it —
until the first backstop install flips the generation stamp to provisional, every poll tick ALSO
runs `reload_if_stale`, whose own `policy_gen` read pays the identical (now sub-second) cost. The
honest worst case used to be `ttl + poll + 2 × retry cycle`, nearer **~55 s** than ~31 s; both
retry terms are now noise against a 30 s TTL. (The tick-side term still disappears entirely once
the stamp goes provisional, at which point `reload_if_stale` returns immediately.)
`IamConfig::validate` only rejects `refresh_interval_secs` **greater** than
`policy_cache_ttl_secs`; **equal is permitted**, so the worst case is a genuine *sum* and an
operator who raises the poll interval to its permitted maximum doubles the bound to
`2 × policy_cache_ttl_secs` (60 s at the default TTL). Neither setting has an upper cap, so
raising `policy_cache_ttl_secs` scales the bound linearly.

All of that assumes **Postgres is reachable**. A persistently failing load — or a malformed
policy row that aborts Cedar compilation — aborts every reload and keeps the last-known-good
snapshot **indefinitely**, with no bound at all. That is what
`IamPolicySnapshotReloadsStalled` and `iam_authz_policy_snapshot_reloads_total{outcome="failed"}`
exist to surface.

It also assumes Postgres is **current**, which nothing enforces: `config.database_url` carries no
primary-read or causal-consistency requirement. Point it at a lagging replica and a reload can
read **pre-revocation** rows, install them, and refresh its TTL clock — the snapshot then reports
itself freshly loaded, `iam_authz_policy_snapshot_reloads_total{outcome="installed"}` keeps
incrementing, and `IamPolicySnapshotReloadsStalled` stays quiet, all while serving stale policy.
**No signal in this catalog detects that**; it is the one way the bound can be silently void with
everything green. Point IAM at the primary, or add replica lag to every number above.

**Same-replica revocation immediacy is degraded during an outage.** Normally
`CedarAuthorizer::is_authorized` calls `PolicySnapshot::reload_if_stale` synchronously, so a
revoke is visible on its own replica on the very next decision. While the generation stamp is
**provisional** (the counter was unreadable at the last load, so the stamp was carried over
rather than observed), request-driven reloads are **suppressed entirely** — comparing a guessed
stamp against a live read would report permanent staleness and trigger a recompile per decision.
During a Redis outage the TTL backstop is therefore the *only* refresh path, and a revoke
committed during the outage becomes visible on its own replica in up to
`policy_cache_ttl_secs + refresh_interval_secs` (~31 s at defaults) plus the reload's own
duration — of which the failed `policy_gen` read is now a sub-second term, see the bound above —
not immediately. The
transition is logged once per state change: `policy_gen unreadable — serving a Postgres-compiled
snapshot on a provisional generation stamp` on the way in, `policy_gen readable again` on the way
out.

**The decision cache does not add to that bound.** Its key's policy component is a **content
hash** of the compiled policy set (SMA-470), not the `policy_gen` counter, so a reload that picks
up a revoke moves every affected request into a disjoint key space immediately — no cached
pre-revoke `Allow` can be re-entered even if the counter stalls or resets to `0`. Note that
decision-cache **`Allow` hits are not re-audited** (cached `Deny`s are, on every call), so any
staleness window is also an **audit gap**: `audit_log` will show no entry for the allowed calls
served from cache during it.

**The content hash covers *stored* inputs only.** It is computed over the policy/template
documents and the role-grant rows — not over the Cedar `schema()` or the `Action`→Cedar-UID
mapping, which are **compile-time constants**. A release that changes evaluation semantics
without touching any stored policy or grant therefore produces the *same* hash on old and new
replicas, and during a rolling deploy both share decision-cache keys. This is not a regression
(the `policy_gen` counter it replaced was equally content-independent), but for a
semantics-changing deploy either flush `iam:authz:dec:*` or accept up to
`authz.decision_cache_ttl_secs` of mixed-semantics cache hits.

**This bound covers policy and role-grant revocation only.** Access changes driven by *tenancy*
state — an organization archived, a membership removed — flow through `entity_gen` and the
entity-slice cache instead. With `authz.cache.backend = "redis"` those are bounded by
`authz.slice_cache_ttl_secs` (default `60`) plus `authz.decision_cache_ttl_secs` (default `30`)
— **90 s at the defaults**, roughly three times the policy-path bound. Both caches exist only on the
Redis backend; the `memory` backend has no slice cache at all and its in-process counters can
never fail to be read.

Since SMA-474 that 90 s figure is the **residual** exposure after rewind repair, not the raw
one. The entity path did **not** get the policy path's content-addressed key, and could not:
`CompiledPolicies::content_hash` works because the compiled policy set is one global object
already in memory when the key is built, whereas an entity slice is per-`(resource, principal)`
and only exists *after* the Postgres load the slice cache exists to avoid — so hashing it to
derive the key would require performing that load on every lookup. See
`docs/superpowers/specs/2026-08-06-sma-474-generation-counter-rewind-design.md` D1. Eliminating
the window structurally rather than bounding it is SMA-475.

**Redis `maxmemory-policy` must be `volatile-*`, never `allkeys-*`.** `iam:authz:policy_gen` and
`iam:authz:entity_gen` are written with a bare `INCR` and **carry no TTL**, so under
`allkeys-lru`/`allkeys-lfu`/`allkeys-random` they are ordinary eviction candidates.
`Generations::read` maps a missing key to `0`, so evicting one rewinds that counter. Since
SMA-474 this is no longer *silent*: each process keeps a high-water mark per counter, a value
below it is repaired forward with an atomic `INCRBY` (persisted, so other replicas converge),
and every occurrence increments `iam_authz_generation_rewinds_total` and fires
`IamAuthzGenerationRewound`. **The mandate still stands** — the repair reduces the exposure by
roughly six orders of magnitude but does not eliminate it (a replica that has not read the
counter in a very long time can still, in principle, repair into a live generation), and an
`allkeys-*` policy turns a routine memory-pressure event into an authz-freshness event for no
benefit. Verify with `CONFIG GET maxmemory-policy`.

### Manual blackhole verification (`docker pause` / `DOCKER-USER`)

The Docker-free automated test cited throughout the section above is the authoritative source for
every number in it. This procedure exists to let an operator confirm the *mechanism* by hand — one
straightforward way to reproduce it, and one plausible-looking way that does not work. Both
descriptions below are corrections: the obvious mental model of each is wrong.

**`docker pause` reproduces the blackhole shape — it does not drop SYNs.** The cgroup freezer
`docker pause` uses stops the **process**, not the network stack: the listening socket stays live
in the kernel, which keeps completing TCP handshakes into the accept backlog (Redis's
`tcp-backlog` defaults to 511) whether or not anything is scheduled to `accept()` them. The result
is exactly the accept-and-never-reply shape this section is about — connect succeeds, the read
hangs — right up until the backlog fills. It is the easiest way to reproduce the shape; it is
simply not a SYN drop. `docker unpause` turns the same setup into a recovery test.

```bash
docker run -d --name sma476-redis -p 6399:6379 redis:7-alpine
docker exec sma476-redis redis-cli ping    # PONG — confirm it answers before pausing
docker pause sma476-redis
# probe against the published port while paused (see below for what to expect)
docker unpause sma476-redis
# probe again to see the recovery
docker rm -f sma476-redis
```

**What was actually run for this RUNBOOK entry, and what was observed.** The host this was written
on had neither a host-installed `redis-cli` nor `iptables` (Docker Desktop on macOS — no Linux
netfilter chains are exposed to the host at all), so the probe step above used a raw TCP client
(`nc`) against the host-published port instead of `redis-cli`, and only the `docker pause` leg
could be exercised end to end. What that showed:

- **Paused:** `printf 'PING\r\n' | nc -w 5 localhost 6399` connected immediately (no
  `ECONNREFUSED`) and received **no reply** for the full 5 s the probe was allowed to wait — it
  timed out idle, not refused. That is the accept-and-never-reply shape, confirmed live.
- **Unpaused:** the identical probe returned `+PONG` in **0.009 s**.

**This does not reproduce the ~2.15 s figure, and that is expected, not a discrepancy.**
`nc`/`redis-cli` have no client-side response timeout analogous to redis-rs's `response_timeout`
(500 ms) or `connection_timeout` (1 s) — left unbounded, a probe against a paused Redis simply
hangs until you give up, the backlog fills, or you unpause it (this was confirmed directly: an
earlier attempt with `redis-cli -t 30` neither returned nor errored inside 40 s and had to be
killed). The ~2.15 s / ~6.46 s figures in this section come **only** from Task 4's hermetic test,
which exercises the actual production client configuration
(`adapters::redis_conn::connect`'s `ConnectionManagerConfig`), not a generic client against a
manually paused container. Use this procedure to confirm the *mechanism*; use the automated test's
numbers to reason about *duration*.

**`iptables -I INPUT -p tcp --dport 6379 -j DROP` will not catch traffic to a Docker-published
port — this leg was not run here (no Linux netfilter on this host), but the reason is
architectural, not host-specific, and is worth stating plainly so nobody reaches for it during an
incident.** A connection to a `-p 6399:6379`-published port is DNAT'd before routing decides where
it goes, and the post-NAT packet never reaches `INPUT` at all, because the destination is the
container's network namespace, not the host's own stack. A rule dropped into `INPUT` never sees
this traffic.

**Neither does `DOCKER-USER`, for a probe run *from the Docker host itself* — the exact case this
procedure's `nc localhost 6399` above is an example of.** `DOCKER-USER` is a hook on the `filter`
table's `FORWARD` chain, and a packet only takes the `FORWARD` path when it arrives on one
interface and leaves on another — true for traffic reaching the published port from **outside**
the host (`nat PREROUTING` → routing → `FORWARD`/`DOCKER-USER`), but not for traffic a **local**
process originates. A locally-originated packet to a DNAT'd destination stays on the
**`nat OUTPUT` → `filter OUTPUT` → `POSTROUTING`** path — it is never forwarded, so `DOCKER-USER`
never evaluates it either, regardless of the rule's contents. The only mechanism that reliably
black-holes a **host-originated** probe is dropping the traffic inside the **container's own
network namespace** (e.g. `nsenter --net=<container-netns> iptables …`, or an `ip netns` variant),
which acts on the traffic after it has already crossed into the container's stack. `DOCKER-USER` is
the right tool only for traffic arriving from a genuinely external host. On a Linux operator host,
prefer `docker pause` anyway — it needs no `iptables` access, no netns entry, and reproduces the
identical shape regardless of where the probe originates.

### Audit retention & partitioning

`audit_log` is a **two-level partitioned table** (migration `m0008_partition_audit_log`, SMA-467):
`PARTITION BY LIST (outcome)` at the top, with each outcome subtree further `PARTITION BY RANGE
(occurred_at)` monthly. Full design rationale:
`docs/superpowers/specs/2026-07-15-sma-467-audit-log-partitioning-design.md`.

```text
audit_log                         PARTITION BY LIST (outcome)
├─ audit_log_committed            PARTITION BY RANGE (occurred_at)
│   ├─ audit_log_committed_2026_07   FROM ('2026-07-01 00:00:00+00') TO ('2026-08-01 00:00:00+00')
│   ├─ audit_log_committed_2026_08   …
│   └─ audit_log_committed_default    DEFAULT   ← RANGE write-safety backstop
├─ audit_log_denied               PARTITION BY RANGE (occurred_at)
│   ├─ audit_log_denied_2026_07      FROM (…) TO (…)
│   ├─ audit_log_denied_2026_08      …
│   └─ audit_log_denied_default       DEFAULT   ← RANGE write-safety backstop
└─ audit_log_other                DEFAULT (plain leaf)   ← LIST catch-all for a stray `outcome`
```

Retention drops whole aged-out **denied** month leaves; **committed** (compliance) leaves are kept
indefinitely by default — the outcome-aware asymmetry the original M5 design called for. All
partition bounds are fully-qualified UTC `TIMESTAMPTZ` literals (never bare dates), so leaf
boundaries can't drift with a session's `TimeZone` GUC.

**The maintenance task (`PgPartitionMaintainer`).** An in-app background task — no `pg_cron`/
`pg_partman` extension dependency — mirrors the outbox relay's shape: an awaited startup tick, then
a `tokio::select!` loop between an interval sleep and the shutdown-watch. Each tick does two
independent units of work (a create-ahead failure never blocks pruning, or vice versa):
1. **create-ahead** (`ensure_partitions_ahead`) — `CREATE TABLE IF NOT EXISTS` the monthly leaf
   (both outcome subtrees) for every month from now through `ahead_months` ahead.
2. **prune** — enumerates each subtree's actual child leaves from the Postgres catalog
   (`pg_inherits`) and `DROP TABLE IF EXISTS` any whose parsed `YYYY_MM` is older than the
   configured cutoff (never touches a `*_default`).

Every DDL statement runs in its own short transaction that takes
`pg_advisory_xact_lock(AUDIT_PARTITION_LOCK_KEY)` (the same lock key the `m0008` migration itself
uses, so a maintenance tick and a migration swap can never race) plus a bounded `SET LOCAL
lock_timeout = '5s'` — a CREATE/DROP that would block behind a live-insert lock on the parent
**backs off** (errors, logged, retried next tick) instead of queueing and stalling audit writes.

**Config — `[audit.retention]`** (in `iam.toml` / `IAM_AUDIT__RETENTION__*`; every field has a
default, so an absent block is valid config):

| key | default | meaning |
|---|---|---|
| `enabled` | `true` | `false` → the maintenance task is **not spawned at all** — no create-ahead, no pruning. See the recovery-trap caveat below before using this to "pause" retention. |
| `interval_secs` | `86400` (daily) | seconds between ticks; validated `> 0`. |
| `ahead_months` | `1` | months of leaf partitions to pre-create ahead of time; validated `1..=24`. |
| `denied_months` | `3` | drop denied monthly leaves older than this; `0` = never drop denied. |
| `committed_months` | `0` | drop committed monthly leaves older than this; `0` = never auto-drop committed (default — a non-zero value auto-deletes compliance rows and logs a startup `warn` stating the effective window). |

**`enabled = false` is a full off-switch, not a "pause deletes" mode — a recovery trap if used that
way.** Disabling stops create-ahead *and* pruning together. After `ahead_months` of real time
elapses, every new audit row for an uncovered month lands in the RANGE `*_default` instead of a
proper leaf, and a **polluted default then blocks create-ahead from ever creating that month's leaf
again — even after `enabled` is flipped back to `true`** (Postgres refuses to create/attach a range
partition whose bounds overlap rows already sitting in the default; see the manual reattach
procedure below). IAM logs a startup `warn` stating this consequence whenever `enabled = false`. **To
pause only deletions while keeping create-ahead healthy** (the usual intent when someone reaches
for "pause retention"), leave `enabled = true` and set `denied_months = 0` **and**
`committed_months = 0` instead — the task keeps running and creating leaves, it just drops nothing.

**Dropping a partition.** In steady state this is fully automatic — `prune` runs every tick and
issues `DROP TABLE IF EXISTS audit_log_denied_YYYY_MM;` (or `audit_log_committed_YYYY_MM;` if
`committed_months > 0`) for any leaf whose month is older than the configured cutoff. Both
`iam_audit_partitions_dropped_total{outcome="denied"|"committed"}` and IAM's per-tick `info` log
record what was dropped. To drop a specific month **ad hoc** (e.g. an emergency disk-pressure prune
ahead of schedule), the same DDL is safe to run by hand — it's the identical statement the task
itself runs:

```sql
BEGIN;
SET LOCAL lock_timeout = '5s';
SELECT pg_advisory_xact_lock(5580467); -- AUDIT_PARTITION_LOCK_KEY — same key the maintenance task uses
DROP TABLE IF EXISTS audit_log_denied_2026_04;
COMMIT;
```

Wrapping the manual DROP in the same `lock_timeout` + advisory-lock guardrails the automated task
uses avoids waiting indefinitely on a live-insert lock and avoids racing a concurrent maintenance
tick (which takes the same advisory-lock key before its own DDL).

Confirm the target is actually a monthly leaf (never a `_default`) and is genuinely outside the
range you want to keep before running this — there is no undo.

**The `*_default` partitions and `audit_log_other`.** `audit_log_committed_default` /
`audit_log_denied_default` (the RANGE defaults) and `audit_log_other` (the top-level LIST default,
catching any `outcome` other than `committed`/`denied`) exist purely as a write-safety backstop —
**no committed-audit insert may ever fail for lack of a partition**, since that insert runs inside
the triggering mutation's own transaction and a hard failure there rolls back the mutation itself.
In steady state (`ahead_months ≥ 1`, task ticking normally) **all three should stay permanently
empty** — `iam_audit_default_partition_rows` (§2.2) is the metric that verifies this, refreshed once
per successful tick. **Known blind spot:** this gauge freezes (doesn't climb) once the task is
stalled while retention stays enabled — exactly when a default is most likely to be actively
filling — so `iam_audit_partition_maintenance_ticks_total` (the `IamAuditPartitionMaintenanceStalled`
alert above) is the primary "is this even running" signal in that case, and the gauge is the
secondary "has it already fallen behind" signal. When retention is **disabled**, the gauge is not
frozen but **absent** — it is set from the same gated task and is never set in the first place — so
neither metric is a signal there; see the `IamAuditPartitionMaintenanceStalled` NOTE above. Treat a
nonzero gauge as urgent: it means live audit rows are currently landing outside any proper monthly
leaf.

**Manual reattach for a non-empty default.** Auto-remediation is a deliberate non-goal for v1 —
recovering a polluted default is a manual, rare, off-peak operation. Postgres refuses to create or
attach a new leaf whose bounds would overlap rows still sitting in a default, so the rows must be
moved out of the default *before* the proper leaf can exist. Example for a RANGE default holding
rows for July 2026 that never got its own leaf in time (substitute the actual affected year/month
and its follow-on month throughout):

```sql
BEGIN;
SET LOCAL lock_timeout = '5s'; -- back off rather than stall live inserts, same as the automated path.
SELECT pg_advisory_xact_lock(5580467); -- AUDIT_PARTITION_LOCK_KEY (m0008_partition_audit_log.rs) —
                                        -- serializes against a concurrent maintenance tick/migration.

-- 1. Build the leaf as a standalone table (same shape as the partitioned parent).
CREATE TABLE audit_log_denied_2026_07 (LIKE audit_log_denied INCLUDING ALL);

-- 2. Move the qualifying rows out of the default and into it.
WITH moved AS (
  DELETE FROM audit_log_denied_default
  WHERE occurred_at >= TIMESTAMPTZ '2026-07-01 00:00:00+00'
    AND occurred_at <  TIMESTAMPTZ '2026-08-01 00:00:00+00'
  RETURNING *
)
INSERT INTO audit_log_denied_2026_07 SELECT * FROM moved;

-- 3. Attach it as a proper leaf — now safe, since the default no longer holds any row in range.
ALTER TABLE audit_log_denied ATTACH PARTITION audit_log_denied_2026_07
  FOR VALUES FROM (TIMESTAMPTZ '2026-07-01 00:00:00+00')
  TO (TIMESTAMPTZ '2026-08-01 00:00:00+00');
COMMIT;
```

Swap `denied` for `committed` for the committed subtree. A non-empty `audit_log_other` (the LIST
default) is a different, rarer signal — it means a row was written with an `outcome` other than
`committed`/`denied`, which the domain never does deliberately; treat that as a data-integrity bug
to investigate (a writer bypassing `AuditOutcome`), not a create-ahead timing issue, and do not
attempt an automatic reattach without first understanding how the stray value got there.

**Retrieving the bootstrap `platform_admin` grant's audit row (SMA-468).** This row is not
findable the obvious way, so a quick `AuditFilter` guess comes back empty even when the row
exists. It is written as `action="GrantRole"` with `resource_prn` set to the Root PRN and
`actor_prn` **null** — null because operator configuration, not a principal, authorized the
grant. `AuditFilter` has no way to filter for a null `actor_prn` and no filter on `detail` at
all, so the grantee has to be recognized after the fact: it's in `detail.principal_prn`, and
`detail.source = "bootstrap_admins"` is what distinguishes this row from an operator-issued
`GrantRole`. The sharper trap is the lookback window — `PgAuditLog::query` applies a default
window whenever both `from` and `to` are absent, and `audit.query_default_window_days` defaults
to 90, so an unfiltered query against a database more than 90 days old silently returns nothing.
**Always pass an explicit `from`** at or before the deployment date when querying for this row
(`action=GrantRole` + `resource_prn=prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000`,
the literal value of `root_prn()`, `paigasus-iam-core/src/authz/model.rs`). This row is also a **one-shot artifact**, which is easy to
misread: `ensure_platform_admin` itself runs on *every* authenticated HTTP or gRPC request from
a configured identity, and a failed listing or transaction is retried on the next one. What does
not repeat is the *write* — once the grant row exists the existence check short-circuits, so no
second audit row is ever produced. Consequently, if `audit.retention.committed_months` is ever
set to a nonzero value, the row is eventually pruned like any other committed leaf and is **not
reproducible** once gone.

### Starter-policy reconciliation at boot

**What happens.** On every boot, `bootstrap::reconcile_policies` reconciles each starter Cedar
policy row against the code-defined content from `authz::roles::starter_policies()`, and
`reconcile_roles` does the same for the `role` table. Most rows converge, but two documented cases
deliberately do not: a row claiming a `starter_revision` newer than the running binary's is left
untouched (`stale_binary`, or the forged-revision case below), and a row whose convergence errors
is kept for that boot (`failed`). These rows are **code-owned**: the
`PutPolicy` API refuses both a persisted `system = true` row and any policy id in the starter
namespace (`authz::roles::STARTER_POLICY_IDS`), so the database is not a supported place to
customize them.

Each policy emits `iam_starter_policy_reconciles_total{outcome=...}`. System-ROLE reconciliation
shares the same counter but only for the `failed` row below — a role that seeds, converges, or is
already unchanged is not counted at all, and (unlike a policy) neither is an orphaned role row:

| `outcome` | Meaning | Action |
|---|---|---|
| `unchanged` | Content matches and provenance checks out. | None. |
| `seeded` | The row was absent and has been created. | None (expected on a fresh database). |
| `reconciled` | A release changed the policy; the row was converged. | None — this is the routine case that used to warn forever. |
| `adopted` | Both provenance columns were NULL, so the row's provenance was unknowable. That is expected for a row seeded before m0010 — but it is not proof of one; see below. | None on the first boot after upgrading. Afterwards, investigate. Also see below if it changed content. |
| `stale_binary` | The stored row was written by a NEWER release **and its provenance checks out**; this replica left it alone. | Expected briefly during a deploy. Persisting means an old replica is still running — or that the fleet was permanently rolled back, in which case it persists forever until a build with a higher `STARTER_POLICY_REVISION` ships. See below. |
| `externally_modified` | Something other than this service wrote the row. Converged and audited — **except** when the row also claims a newer revision, which is warned about but *not* repaired (see below). | **Investigate.** |
| `orphaned` | A `system = true` **policy** row whose id is no longer code-defined. An orphaned system **role** row is WARN-logged (`reconcile_roles`) but does NOT increment this counter — a deliberate asymmetry with the policy half. | Retire it with `POST /v1/authz/system-policies/{id}/retire` once the fleet has converged — see "Retiring an orphaned system-owned row (SMA-481)" below. |
| `failed` | Converging one row errored — a starter **policy** row or a system **role** row, both under this same label. A row that already existed is kept for this boot; an absent row that couldn't be seeded is fatal. So is **any** failure when the pre-loop id snapshot was itself unreadable, because no row can then be proven to exist. | Check the ERROR log line — it names which of the three cases this was, and its `policy_id` vs `role_key` field says which half failed (the metric alone can't). Transient at low volume in the survivable case. |

**`externally_modified` — the one that matters.** It logs

```text
a system-owned starter policy was modified outside this service; converging it back to the code-defined content
```

and writes one `audit_log` entry capturing what was overwritten, because converging destroys the
evidence. Retrieve it with `action = "PutPolicy"` and `resource_prn` = the Root PRN, then match
`detail.source = "starter_policy_reconcile"` and `detail.reason = "external_modification"`.
`detail.previous_content.source` and `detail.previous_content.description` hold the overwritten
Cedar source and description — each capped independently at 8 KiB, flagged by its own
`detail.previous_content.truncated` (source) / `detail.previous_content.description_truncated`
(description) — and `detail.previous_content.kind` holds the overwritten `static`/`template`
value, which matters because a template stored as `static` fails to compile and takes boot down.

The one `externally_modified` case that writes **no** audit row is the forged-revision case below,
where nothing was overwritten because nothing was written at all.

**Always pass an explicit `from`.** `PgAuditLog::query` applies a default lookback whenever both
`from` and `to` are absent (`audit.query_default_window_days`, default 90), so an unfiltered
query against an older database silently returns nothing.

**What the warning is and is not.** It detects accidental and naive edits. It is a *provenance
hint*, not tamper evidence: the only actor who can modify a `system = true` row is one with
direct SQL access, and that same access recomputes the fingerprint trivially, at which point the
edit reads as a routine code change. Do not treat a quiet log as proof nothing was touched.

**A hand-patched starter policy is normally reverted on the next replica boot** — with one
exception: a patch that also raises `starter_revision` above the running binary's is *not*
repaired by any running replica, and needs the remediation in the next section. Otherwise there is
effectively no
escape hatch: a forked non-system policy can add a `forbid` but can never remove a code-defined
one, and a forked role *template* is never linked by any grant (a grant resolves its template by
`role_key`). Starter policies can be tightened out-of-band and cannot be loosened. If you need a
different starter policy, change the code.

**A newer revision this binary will not repair.** Reconcile defers unconditionally to a row whose
`starter_revision` exceeds the running binary's `STARTER_POLICY_REVISION` — there is one `policy`
table for the whole fleet, and an older replica rewriting a newer release's row is exactly what
the revision guard exists to prevent. Two situations produce it, and they are told apart by the
row's provenance:

- **Provenance intact** (`system = true` and a fingerprint matching the row's own content, which
  is what a genuine newer release always writes) → `outcome = "stale_binary"`, logged at INFO. A
  mixed-version deploy window. If it persists, an old replica is still running; if it persists
  *after* a deliberate rollback, that is also expected — vN leaves vN+1's policy set in place
  rather than self-healing backwards into a looser one, and the only way forward is a build whose
  `STARTER_POLICY_REVISION` exceeds the stored value.
- **Provenance broken** → `outcome = "externally_modified"`, logged at **WARN**, message
  `a starter policy row claims a revision newer than this binary's but its provenance does not
  check out`. Nothing but a hand edit produces this: a real newer release stamps both columns
  together. The row is **diverged and will not be repaired by any running replica** until a build
  with a higher revision ships. Treat it as an unresolved divergence of the authorization boundary
  — the row state alone does not say whether the stored policy is weaker, stricter, or merely
  differently worded, which is what the first remediation step below establishes.
  **Remediation:** compare the stored `source` against `authz::roles::starter_policies()` for that
  id, then either repair the row directly (restoring `system = true`, setting `starter_revision`
  at or below the running binary's value, and clearing `content_fingerprint` so the next boot
  converges and reports it) or ship a build with a higher `STARTER_POLICY_REVISION`.
  There is deliberately **no audit row** for this case: unlike a converged edit, which is a
  one-off because the next boot finds the row repaired, this recurs on every boot of every replica
  until a human acts, and `audit_log` is append-only — the same reasoning that keeps orphans
  unaudited. The WARN and this metric are the whole signal.

**`adopted` on the first boot after upgrading** is expected: the fingerprint column starts NULL
for every pre-existing row and is stamped on that boot. If the row's content had also drifted, an
audit entry with `reason = "adopted_unfingerprinted"` records what was replaced.

**`adopted` seen after that first post-upgrade boot is not routine.** Only a genuine pre-m0010 row
adopts, and such a row has *both* `content_fingerprint` and `starter_revision` NULL — this service
writes the two together and m0010 back-fills neither. A row that reappears as `adopted` therefore
means somebody cleared both columns by hand. (Clearing only the fingerprint, leaving the revision
stamped, is detected: it classifies `externally_modified` and is converged and audited like any
other edit.)

**A pure provenance stamp still bumps `updated_at`** without changing any content and without
bumping `policy_gen`, so an `updated_at` change visible through `ListPolicies` is not by itself
evidence of a policy change.

### Retiring an orphaned system-owned row (SMA-481)

**What this is for.** The `orphaned` outcome above, and the sibling role-half `WARN` (`reconcile_roles`
does not increment a counter for it), name a `policy`/`role` row whose id
`authz::roles`/`authz::roles::starter_policies()` no longer defines. That row still compiles,
still links grants, and `PutPolicy`/`DeletePolicy` refuse to touch it (SMA-481 D3/D7) — it stays
in this half-alive state forever unless an operator acts. `POST
/v1/authz/system-policies/{id}/retire` is the only supported way to remove it: Root-only,
enforced inside `SystemRetirementService::retire` (in practice, the caller needs a
`platform_admin` grant at Root — the same posture as the dead-letter endpoints, §4). Follow the
steps below **in order**: the order is what keeps you from getting stuck, more than any of the
prose around it.

**1. Precondition, before anything else: every replica must be on a binary that no longer defines
the id.** `classify_starter_policy` (`paigasus-iam-core::authz::reconcile`) classifies an absent
row as `Absent` — "seed it" — *before* the revision guard ever runs. So a replica whose code
catalog still defines the retiring id will silently re-seed the policy row (and, for a role id,
its paired `role` row too, via `reconcile_roles`) the moment it next boots or reconciles.
Retiring mid-rollout is not merely risky, it is **silently undone**. `retire` does guard this
in-band (`409 fleet-not-converged`, step 3 below), but that guard only sees rows that still
exist at call time — it cannot stop a replica from re-seeding a row you just successfully
deleted a moment earlier. Confirm the rollout that dropped the id from the code catalog has
reached every replica before calling `retire` at all.

**2. Read the orphan `WARN`, then call the endpoint** as an actor holding a `platform_admin`
grant at Root:
```http
POST /v1/authz/system-policies/{id}/retire
Content-Type: application/json

{"acknowledge_decision_change": false}
```
**The body is optional, but "no body" and "empty body" are not the same request.** Omit the
`Content-Type` header entirely to send no body — that extracts as `None` and means "not
acknowledged". If you *do* send `Content-Type: application/json`, the body must be valid JSON:
`{}` (the field defaults to `false`) or an explicit `{"acknowledge_decision_change": …}`. A
`Content-Type: application/json` header with a genuinely empty body is a malformed request and
returns `400`, not an unacknowledged retirement — so `curl -X POST -H 'Content-Type:
application/json'` with no `--data` fails, while the same `curl` without the header succeeds.

A `200` returns `{"policy_id", "kind", "role_deleted"}` — the operator's only immediate record of
exactly what was destroyed (the durable copy is the `RetireSystemPolicy` audit entry, written in
the same transaction). Anything else below is a refusal, not a partial success — none of them
write anything.

**3. Handle a refusal:**

| response | meaning | what to do |
|---|---|---|
| `409 fleet-not-converged` | Some remaining **starter policy** row — one whose id the running binary still defines — was last written by a binary older than this one's `STARTER_POLICY_REVISION`, carries no revision at all (pre-m0010), or no starter row exists yet. Step 1's precondition, checked in-band. The orphan's own revision is deliberately not consulted: it is always older by construction, so counting it would refuse every genuine orphan. | Wait for the rollout to finish, then retry. There is no override. |
| `409 grants-survive` | The role still has live grants (the body lists `grants[]`, `total_surviving`, `truncated`). | Revoke each listed grant — `DELETE /v1/authz/role-grants/{id}` — then retry `retire`. **If a revoke 403s because its scope node is archived** (`RevokeRole` is a write action, and `forbid-archived-writes` blocks it even for `platform_admin`): restore the node (`POST /v1/{organizations\|projects\|teams}/{id}/restore`, matching the node's kind), revoke the grant, then re-archive it (`POST .../archive`). Skip this detour and there is no way to ever revoke a grant at an archived scope — the operator loops on `grants-survive` forever. |
| `409 decision-change-unacknowledged` | The id is a **static** policy — evaluated on every request rather than through a grant — so removing it changes decisions fleet-wide the instant it commits. The body's `source`/`description`/`kind` is exactly the content that would be destroyed; the refusal IS the preview. | Read `source`, decide whether the change is really wanted, then re-send with `{"acknowledge_decision_change": true}`. The flag is a harmless no-op if the id turns out to be a template instead. |
| `409 system-immutable` | The id is still code-defined — this was never an orphan. | Nothing to retire; a live starter row is governed by `PutPolicy`/`DeletePolicy`, not this endpoint. |
| `404` | No row exists at that id. | **Retirement is deliberately not idempotent** — a second retirement of an id already retired also 404s. Treat an unexpected `404` as "the operator's model of the system is wrong," not as a no-op repeat: re-`GET`/list the row before assuming this was a retry. |

**4. Confirm, and know what a returning `WARN` means.** After a `200`, confirm the orphan `WARN`
is gone on the next boot of every replica. **If it comes back, the fleet had not actually
converged when `retire` was called** — some replica's code catalog still defined the id and
re-seeded the row per step 1 — and the fix is simply to repeat the retirement once convergence is
confirmed. **Nothing is corrupted**; the row was re-seeded, not left half-deleted.

**5. Watch the metric.** `iam_system_rows_retired_total{outcome="retired"}` increments once the
deletes, the `PolicyDeleted` event, and the audit entry all commit (see `names.rs`'s doc comment,
§2.2, for exactly which outcomes do — and, just as importantly, do NOT — touch this counter).
`outcome="blocked"`/`outcome="refused"` exist too, but watching only `retired` misses a
retirement that keeps getting blocked or refused without ever succeeding.

**6. What this endpoint cannot reach.** A hand-inserted `role` row whose `template_id != key`.
Every row this service itself ever wrote satisfies `policy_id == role.key == role.template_id`
(`authz::roles` module doc), and the endpoint keys off that one shared id — `lock_policy_in`/
`lock_role_in` both look it up directly, with no separate lookup for a mismatched
`template_id`. A `role` row with a mismatched `template_id` can only have been written by hand,
outside this service, and is unreachable through `retire`; it needs direct database work instead
(remove the row and whatever grants now reference it in FK order — grants, then role, then
policy — since `fk_role_grant_role` and `fk_role_template` are both restrict). **That is the
order the foreign keys force on you by hand; it is not the order the service follows.** `retire`
never deletes a grant at all: it refuses (`409 grants-survive`) while any survive and makes you
revoke them through the audited `RevokeRole` path first (design D4), so its own delete order is
only role → policy. Deleting grants by hand here bypasses that audit trail and the
`policy_gen` bump — it is the unsupported path, which is exactly why it is the last resort.

---

## 5. Cardinality & privacy

**Never a metric label:** `model` (caller-supplied, unbounded — a single malicious or buggy
client could mint unlimited series), any PRN (principal/resource/tenant identifiers), API key
id, raw request path, or any free-form error string. This preserves the same privacy bar the
gateway already applies to its structured logs (never log/label prompt bodies or credentials) and
prevents Prometheus cardinality blow-ups / OOM.

**Only bounded, closed-enum or template labels are allowed:**
- `route` — the axum `MatchedPath` **template** (e.g. `/v1/chat/completions`), never the raw
  incoming path.
- `method` — the fixed HTTP verb set.
- `status_class` — `2xx`/`4xx`/`5xx` (never the raw numeric status).
- `grpc_status` — the canonical tonic status-code name (a fixed enum), never derived from
  `:path`.
- `decision` — `allow`/`deny`.
- `cache` — `hit`/`miss`/`bypass`.
- `outcome` — `committed`/`denied`.
- `operation` — `introspect`/`authorize`.
- `result` — `ok`/`denied`/`unavailable`/`error`.
- `role` — the closed `RedisRole` enum (SMA-476): `authz`/`api_keys`/`jwks`. Bounded by the type
  system at the call site, not derived from anything caller-supplied.
- `to` — the breaker's target state on a transition (SMA-476): `open`/`half_open`/`closed`.
- `counter` — which authz generation counter rewound (SMA-474): `policy_gen`/`entity_gen`.
  Derived from a Rust enum (`Which`), never from anything caller-supplied.
- `reason` — how a rewind presented (SMA-474): `missing` (the key was gone) / `lower` (it came
  back at a smaller value). Two literals chosen at the emit site.
- gRPC `service`/`method` — **compile-time string literals** supplied at each `record_grpc` call
  site (e.g. `"Authorization"`/`"IsAuthorized"`), never derived from the request `:path` — a
  scanning client hitting an arbitrary RPC path cannot mint new label values, unlike an HTTP
  `MatchedPath`, which is why gRPC needed no additional guard beyond "use literals."

**How to add a new metric safely** (so it doesn't silently rot or blow up cardinality):
1. Add the metric name as a `const` in `paigasus_observability::names` (snake_case,
   service-prefixed, `_total` suffix for counters, base-unit `_seconds` for latency histograms),
   and add it to the `names::ALL` slice.
2. Instrument the call site using only the bounded label set above (or extend that set
   deliberately, with the same never-unbounded discipline — get a second pair of eyes on any new
   label).
3. Reference the metric from a Grafana dashboard panel and/or an alert rule as needed.
4. Run the drift test (`paigasus-observability`'s `tests/drift.rs`, part of the normal CI gate) —
   it extracts every `iam_`/`gateway_`-prefixed identifier from the committed dashboard JSON and
   rule YAML `expr` fields (a prefix-anchored scan, so label keys like `status_class`, PromQL
   function/keyword tokens like `rate`/`sum`/`by`, and template vars like `$__rate_interval`
   never match and need no separate allowlist), strips a trailing histogram/summary suffix
   (`_bucket`/`_sum`/`_count`), and asserts every remaining identifier is in `names::ALL`.
   **A metric emitted by code but never added to `names::ALL` — or referenced in a dashboard/rule
   but never in `names::ALL` — fails this test.** This is what keeps the ops artifacts from
   silently rotting relative to the code.
5. Run `promtool check config`, `promtool check rules`, and `promtool test rules` (also part of
   CI) to validate the PromQL/YAML itself and confirm any new alert actually fires against a
   synthetic series.

---

## 6. Future

Not implemented in this cycle; tracked as explicit follow-ups:

- **OpenTelemetry (OTLP) trace/metric export**, to correlate metrics with the existing structured
  JSON `tracing` logs. Today metrics are Prometheus-pull-only.
- **mTLS on the `/metrics` scrape endpoint** — the separate-listener bind (`[metrics].addr`) is
  in-scope today; mutual TLS on top of it is the next hardening step.
- **Full tower-layer gRPC metrics** (a trailer-classifying body-wrapper layer), if a
  non-handler-scoped instrumentation approach is ever wanted instead of the current
  per-handler-boundary `record_grpc` calls.
- **Hosted Prometheus + Grafana deployment**, long-term metrics storage, and alert **routing**
  (Alertmanager → PagerDuty/Opsgenie or similar) — this RUNBOOK's alert rules define *what* fires,
  not *where it pages*; routing is deployment-specific and not yet configured anywhere.
- **A gRPC mirror of the `/v1/outbox/dead-letters` surface**, if a non-HTTP operator client ever
  needs one — untracked, no follow-up issue filed. Deliberately out of scope for SMA-469 — the
  HTTP adapter's own module doc calls this a scope decision, not an API-boundary principle, and
  keeping it HTTP-only keeps `contracts/` untouched.
- **Bulk discard**, if `[outbox.retention].parked_days` proves insufficient in practice for
  retiring a mass-parked backlog. There is deliberately no bulk-discard endpoint today (SMA-469);
  the supported bulk-retirement path is the retention sweep — see `IamOutboxEventsParked`'s
  remediation (§4) for the `parked_days` procedure.
- **`DETACH … CONCURRENTLY` partition drops** (PG14+) instead of a `lock_timeout`'d
  `DROP TABLE IF EXISTS`, and **auto-remediating a non-empty `DEFAULT` partition** (moving leaked
  rows into a freshly created leaf automatically instead of the manual §4 reattach procedure) —
  both explicit SMA-467 non-goals for v1, which favored simplicity + observability over auto-repair
  machinery.
- **Gateway M5** observability: provider-routing, cost/budget, and response-cache metrics, and a
  corresponding dashboard extension — those surfaces (multi-provider routing, caching, budgets)
  don't exist yet; this cycle scoped gateway metrics to the M0 auth+proxy surface only.
- **A combined IAM introspect-and-authorize RPC**, which would also reduce the gateway's
  per-request round-trip count and the surface area of `GatewayIamDependencyUnavailable`.
- **A Redis circuit breaker shipped in SMA-476** — every Redis command now runs behind a
  per-connection breaker (`adapters::redis_conn::RedisHandle`) that stops attempting a known-down
  backend, capping the recovery lag added on top of any Redis outage at ~6 s instead of paying
  ~2.15 s **per failed command** for the outage's entire duration. Degradation (cache bypass, or
  503s on the fail-closed JWKS path) still lasts as long as the breaker itself stays non-closed,
  plus that ~6 s recovery lag once Redis is back — it is not a flat "6 s and done". See §4 "Authz
  availability posture" for the full mechanism, the measured numbers, and the three alerts. What
  remains genuinely open:
  - **`connection_timeout` stays at redis-rs's 1 s default** (SMA-476 D2) — a remote/managed Redis
    (higher baseline RTT, a proxy hop in front of it) makes a global tightening a false-trip risk
    against connections that are merely slow, not down, so it was deliberately left alone rather
    than tuned down alongside the breaker.
  - **SMA-473 D10's boot-tolerance residual.** `redis_conn::connect` is still eager and
    breaker-independent at boot (SMA-476 D11: a single boot dial has nothing to break on, so the
    breaker starts Closed and wraps commands only), so a Redis that is down or slow to start at
    boot still fails `AppState::new` and costs a crash-restart, exactly as before this cycle. If a
    deferred retry-loop-at-boot ever ships to close that gap, it must be revisited **together with**
    D11's decision: a boot that retries is a boot that *can* accumulate failures, and whether those
    should seed the breaker is a real question this cycle deliberately left open rather than
    answered.
- **Postgres-backed generation counters**, so a grant/revoke and its invalidation bump commit in
  one transaction. That removes §4's revocation-staleness window entirely rather than bounding
  it, and removes the `maxmemory-policy` mandate; it needs an ADR and a migration (SMA-470 D3).
