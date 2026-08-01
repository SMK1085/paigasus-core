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

### 2.2 `paigasus-iam` — gRPC, authz, audit, outbox relay

| metric | type | labels | meaning / expected range |
|---|---|---|---|
| `iam_grpc_requests_total` | counter | `service`, `method`, `grpc_status` | One increment per completed tonic handler call. `service`/`method` are compile-time string literals (e.g. `service="Authorization"`, `method="IsAuthorized"`) — never derived from the request path, so cardinality is bounded to the known RPC set (Tenancy / Authentication / Authorization / ServiceAccount / Audit). `grpc_status` is `"ok"` or the canonical tonic status-code name (`permission_denied`, `unavailable`, `invalid_argument`, …). |
| `iam_grpc_request_duration_seconds` | histogram | `service`, `method` | gRPC handler latency, recorded at the same handler-boundary call site as the counter above. |
| `iam_authz_decisions_total` | counter | `decision`, `cache` | Every `CedarAuthorizer::is_authorized` outcome. `decision` ∈ `allow`/`deny`. `cache` ∈ `hit` (served from the generation-keyed decision cache — deny hits are still re-audited, allow hits are not), `miss` (computed fresh), or `bypass` (the Redis-backed entity-generation counter was unreadable, so the cache was skipped entirely and the decision was computed directly against the last-known-good policy snapshot — see §4 "Authz availability"). The highest-value operational signal in the catalog: allow/deny volume and cache effectiveness. |
| `iam_authz_policy_snapshot_reloads_total` | counter | `outcome` | Every `PolicySnapshot` reload attempt. `outcome` ∈ `installed` (a fresher compiled set replaced the live one), `rejected` (an out-of-order reload lost its race and was discarded — benign in isolation), `failed` (the load or Cedar compile errored; the last-known-good snapshot keeps serving). `installed` must stay non-zero: the TTL backstop installs one every `authz.policy_cache_ttl_secs` regardless of generation movement, and silence means revocations are not taking effect (SMA-470). |
| `iam_audit_records_total` | counter | `outcome`, `result` | Every `PgAuditLog::record`/`record_out_of_band` call. `outcome` ∈ `committed` (mutation audit rows) / `denied` (denial audit rows). `result` is `"ok"` for an INSERT that did not error. **Caveat:** this counts insert-attempts-not-erroring, not durably-committed rows — an in-transaction `record` call on a mutation's UoW bumps `result="ok"` before that transaction's outer `commit()`, so a rare downstream rollback leaves the row invisible even though the counter already incremented. This only diverges on the mutation-error path, which is itself visible elsewhere as a `result="error"`/5xx signal, so it doesn't mislead in steady state. |
| `iam_denial_audits_dropped_total` | counter | — | Bumped at `DenialAuditBuffer::push`'s drop-oldest site when the bounded denial-audit buffer is full. **Non-zero means the audit trail for denials has gaps** — see §4 "Denial-audit drops". |
| `iam_denial_audits_enqueued_total` | counter | — | Bumped on every `DenialAuditBuffer::push`, whether or not it also drops. Compare against the dropped counter to gauge loss ratio during a denial burst. |
| `iam_outbox_relay_ticks_total` | counter | `result` (`ok`/`error`) | One increment per relay tick (poll loop iteration), regardless of whether the tick found rows to drain. **This is the relay's liveness signal** — see §4 "Outbox stalled". |
| `iam_outbox_relay_drained_total` | counter | — | Rows locked and processed in a tick (published + failed, including newly-parked), summed from `TickReport.drained`. |
| `iam_outbox_relay_published_total` | counter | — | Rows successfully published in a tick (`drained − failures`). |
| `iam_outbox_relay_publish_failures_total` | counter | — | Rows whose `EventPublisher::publish` call failed in a tick (a subset of `drained`, superset of `parked`). |
| `iam_outbox_relay_parked_total` | counter | — | Rows that hit `[outbox].max_attempts` and were **parked** (poison) in a tick — a **counter of newly-parked rows this tick**, deliberately not a gauge (a gauge summed per-tick would read `0` on every tick that parks nothing new, hiding a growing parked backlog behind a flat-looking panel). See §4 "Outbox parked events". |
| `iam_outbox_oldest_unpublished_age_seconds` | gauge | — | Age (seconds) of the oldest unpublished-and-unparked row seen in the most recent non-empty tick's batch (`None` → reported as `0`). Freshness is bounded by `[outbox].poll_interval_secs`. **Freezes at its last value if the relay task wedges while the process stays alive** — it is a backlog-lag signal, not a liveness signal (see §4). |
| `iam_audit_partition_maintenance_ticks_total` | counter | `result` | One per audit partition-maintenance tick (create-ahead + prune). `result` ∈ `ok`/`error`. Liveness signal — see §4 "Audit partition maintenance stalled". |
| `iam_audit_partitions_created_total` | counter | — | Monthly leaf partitions created by create-ahead. |
| `iam_audit_partitions_dropped_total` | counter | `outcome` | Monthly leaf partitions dropped by retention. `outcome` ∈ `denied`/`committed`. |
| `iam_audit_default_partition_rows` | gauge | — | Rows currently in the audit `DEFAULT` partitions. **Should be 0**; nonzero ⇒ create-ahead fell behind (freezes when the task is stalled while retention stays enabled — the ticks counter is the primary liveness signal there; when retention is **disabled** neither metric exists at all, see §4 "Audit partition maintenance stalled"). |

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
- Audit write rate (by `outcome`).
- Denial-audit drop rate — **should be flat 0**; any nonzero value is worth investigating (§4).
- Outbox row: drained rate, published rate, publish-failure rate, parked events (15m window,
  stat panel), oldest-unpublished age (stat panel — the key backlog SLO), relay tick rate.

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
| `IamOutboxRelayStalled` | `rate(iam_outbox_relay_ticks_total[10m]) == 0` | critical |
| `IamPolicySnapshotReloadsStalled` | `(sum(increase(iam_authz_policy_snapshot_reloads_total{outcome="installed"}[15m])) or vector(0)) == 0` for 15m | critical |
| `IamAuditPartitionMaintenanceStalled` | `sum without (result) (increase(iam_audit_partition_maintenance_ticks_total[2d])) == 0` for 1h | warning |
| `IamHighErrorRate` | `sum(rate(iam_http_requests_total{status_class="5xx"}[5m])) / sum(rate(iam_http_requests_total[5m])) > 0.05` for 10m | critical |
| `IamGrpcHighErrorRate` | `sum(rate(iam_grpc_requests_total{grpc_status!="ok"}[5m])) / sum(rate(iam_grpc_requests_total[5m])) > 0.05` for 10m | critical |
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

**Likely causes:** the `EventPublisher` implementation is failing/erroring on most publishes (in
this repo, the only implementation is `TracingEventPublisher`, which only fails on
serialization-adjacent bugs — in a real deployment with a broker-backed publisher, this usually
means the broker is unreachable or rejecting writes); the relay is running but its
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
`event_outbox` row hit `[outbox].max_attempts` (default `5`) consecutive publish failures and was
marked `parked = true`, permanently excluded from future relay batches (the relay's poll
predicate is `published_at IS NULL AND parked = false`). This is deliberately a **counter of
newly-parked rows**, not a gauge of the current parked-row count — a gauge summed per-tick would
read `0` on every tick that parks nothing new and hide a slowly-growing parked backlog behind a
flat panel; the currently-parked-row count is a derivable Prometheus query
(`sum(increase(iam_outbox_relay_parked_total[…]))`) if needed, or a direct SQL count (below).

**Likely causes:** a single event's payload is fundamentally unpublishable (e.g. malformed for
the specific `EventPublisher` backend, or an event type the consumer rejects deterministically) —
retrying it forever would never succeed, hence the cap; or a broader outage caused every event in
a window to exhaust its retries.

**Confirm:**
1. IAM logs around the parked event's `id` — the relay emits `tracing::error!` with
   `id`/`event_type`/`attempts`/`reason` at the parking site, which usually explains *why* every
   attempt failed.
2. Count currently-parked rows directly against Postgres:
   ```sql
   SELECT id, event_type, attempts, occurred_at, aggregate_prn
   FROM event_outbox
   WHERE parked = true
   ORDER BY occurred_at;
   ```

**Remediation (interim — manual, no automated replay tool exists yet):**
- If the root cause was transient (e.g. a broker outage that has since recovered) and the
  event's payload is otherwise valid, replay it by clearing its parked/attempts state so the
  relay picks it back up on its next poll:
  ```sql
  UPDATE event_outbox
  SET parked = false, attempts = 0
  WHERE id = '<parked-row-id>';
  ```
  Do this for a small, deliberately-chosen set of rows after confirming the underlying failure
  is fixed — blindly un-parking everything re-triggers the same failure loop if the root cause
  wasn't transient.
- If the event's payload is genuinely malformed (a bug in the writer, not a downstream outage),
  it will never publish successfully; leave it parked and open a follow-up to fix the writer /
  investigate how a bad row was written, rather than looping it through more failed attempts.
- **A full dead-letter-queue subsystem and automated replay/pruning tooling are explicit
  follow-ups (§6), not yet implemented.** Today "the DLQ" is just `parked = true` rows sitting in
  `event_outbox` — there is no separate table, no UI, and no scheduled sweep. Treat the manual
  SQL above as the only remediation path until that follow-up lands.

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

### `IamPolicySnapshotReloadsStalled` — the policy snapshot has not installed a reload (critical)

**Meaning.** `(sum(increase(iam_authz_policy_snapshot_reloads_total{outcome="installed"}[15m])) or
vector(0)) == 0` for 15 minutes — no `PolicySnapshot` reload has been **installed** anywhere on
this fleet in 15 minutes, even though the IAM process is up. This is the telemetry SMA-470 added
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

**The `or vector(0)` guard.** A naive `sum(increase(...{outcome="installed"}[15m])) == 0` goes
silent in exactly the worst case: if `outcome="installed"` has **never** been emitted at all — the
totally-broken-backstop scenario this alert exists to catch, or simply a replica that just booted
— that label combination has no time series in Prometheus yet, and `increase()`/`sum()` over a
nonexistent series return an **empty** vector, not a 0-valued one; comparing empty `== 0` is also
empty, so the rule would evaluate to nothing and never fire. `or vector(0)` supplies a 0-valued
fallback only when the left side has no series, so the comparison always has something to
evaluate. This does not introduce false positives against normal boot: a healthy replica installs
well within the 15m `for` window (the default TTL is 30s), so the alert only fires when installs
are genuinely and persistently absent.

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
Redis outage, generation-keyed invalidation, and the TTL backstop).

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
Redis-backed entity-generation counter to build its decision-cache key. If that read errors (the
Redis-backed cache accelerator is unreachable), the decision cache is **bypassed entirely** for
that call — no key is built, no lookup, no cache population — and the decision is computed
**directly** against the last-known-good, in-memory-cached policy snapshot. This costs **latency
only** (an extra entity-slice load + policy evaluation instead of a cache hit); it never
compromises correctness, and it's directly observable as `iam_authz_decisions_total{cache="bypass"}`.
A fail-**closed** posture (denying everything during a Redis outage) was considered and explicitly
deferred to a separate authz-hardening effort — the current, intentional posture favors
availability.

**Revocation freshness is generation-keyed, with a TTL backstop.** A role grant/revoke bumps the
policy/entity generation counter **synchronously, before the mutation's use-case returns**
(the AC1 guarantee) — this changes the decision-cache key immediately, so a subsequent
authorization decision is never served a stale cached `Allow` from before the revoke; it simply
misses the (now differently-keyed) cache and recomputes. As a backstop against any delay in that
generation-bump propagating (e.g. degraded Redis connectivity that doesn't outright error but
lags), cached decisions also expire on a plain TTL — `authz.decision_cache_ttl_secs` (default
`30` seconds) — bounding the worst-case staleness window even if the generation-based
invalidation is somehow delayed. Underneath that decision-cache TTL sits a second, independent
TTL: `PolicySnapshot`'s own backstop (`authz.policy_cache_ttl_secs`, also default `30` seconds)
unconditionally reloads and installs a fresh compiled policy set every TTL regardless of whether
the Redis-backed generation counter visibly moved (SMA-470) — this is what makes a revoke take
effect even through a Redis outage, and `iam_authz_policy_snapshot_reloads_total{outcome="installed"}`
is its liveness signal (`IamPolicySnapshotReloadsStalled` above).

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
- **Outbox pruning + a full dead-letter-queue subsystem** for parked events (today: manual SQL
  per §4's `IamOutboxEventsParked` entry; no scheduled sweep, no separate DLQ table/UI).
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
