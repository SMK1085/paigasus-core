# SMA-446 (M5, sub-project 3) — Prometheus metrics, dashboards & RUNBOOK

**Status:** Design (brainstormed) · **Date:** 2026-07-13 ·
**Linear:** SMA-446 (part of; **closes the epic** once merged) ·
**Services:** `paigasus-iam`, `paigasus-gateway` (+ new lib `paigasus-observability`)

> This is the epic's 5th (and final) scope bullet — an observability deliverable, **not**
> one of the three already-merged ACs (denial audit + query #80, UoW + outbox + mutation
> audit #81, Gateway M0 + IAM auth #88). It makes the backbone those PRs built *operable*:
> metrics, dashboards, alerts, a local stack, and a RUNBOOK.

---

## 1. Context

M5 (SMA-446) was decomposed during intake into three follow-on cycles (see
`2026-07-12-sma-446-m5-audit-log-outbox-design.md` §1):

1. IAM persistent audit log + domain-event outbox — **merged** (#80, #81).
2. Gateway service + IAM integration — **merged** (#88).
3. **Prometheus dashboards + RUNBOOK** ← *this spec*.

### What exists today (relevant substrate)

- **No metrics infrastructure of any kind.** `rs/Cargo.lock` carries no `metrics`,
  `metrics-exporter-prometheus`, `prometheus`, `opentelemetry`, or `axum-prometheus`. There is
  **no `/metrics` endpoint** anywhere, **no** Grafana/Prometheus config, **no** Dockerfile /
  compose / k8s, and **no** RUNBOOK or `docs/ops/` — this cycle sets those precedents.
- **Observability today = structured JSON `tracing`** via the shared `paigasus-logging` lib
  (`rs/crates/libs/paigasus-logging/src/lib.rs:22`, a global JSON `tracing-subscriber`). Both
  services call `paigasus_logging::init(...)` first thing in `main`.
- **Rich "metrics-as-logs" already exist and are the instrumentation substrate:**
  - **Gateway** logs one structured line per proxied request
    (`paigasus-gateway/src/adapters/http/chat.rs:110`) with `model`, `stream`, `status`,
    `latency_ms` (latency already measured via `Instant::now()` at `chat.rs:76`), `principal`,
    `key_id`. Auth failures log at `auth.rs:70,93`. The gateway has **no** `tower-http`/`TraceLayer`.
  - **IAM** already applies `tower-http` `TraceLayer` + `TimeoutLayer`
    (`paigasus-iam/src/adapters/http/mod.rs:698-700`). Its **relay** emits a per-tick
    `TickReport { drained, failures, parked, oldest_unpublished_age_secs }`
    (`adapters/events/relay.rs:40-51`), both returned and logged (`relay.rs:149`). The
    **denial-audit buffer** keeps a `dropped: AtomicU64` (`adapters/authz/denial_audit.rs:44`,
    bumped on drop-oldest at `:75`) which a **60-second ticker task** in `main.rs:121-151` emits
    as a `tracing::warn!(dropped_denial_audits=…)`. That task's own comment
    (`main.rs:124-125`) says *"a persistent-metrics backend is a later slice"* — **this cycle is
    that slice.**
- **Config = figment** (defaults < `<service>.toml` < `<PREFIX>_*` env). Gateway
  `GatewayConfig` (`paigasus-gateway/src/config.rs:17`, `http_addr` default `0.0.0.0:8088`);
  IAM `IamConfig` (`paigasus-iam/src/config.rs`, `http_addr` `0.0.0.0:8080`, `grpc_addr`
  `0.0.0.0:9090`). Each has a `<service>.toml.example`.
- **Moon:** service crates use the `-rs` id suffix, `layer: application`; the logging lib is
  `paigasus-logging-rs`, `layer: library`.

### Coordination note (avoid double work)

The gateway M0 design doc defers gateway metrics/dashboards to a future **gateway M5** epic
(`2026-07-13-sma-446-gateway-m0-iam-auth-design.md` §11). This cycle **pulls the M0-path
metrics forward** so SMA-446 closes with its backbone observable, but **explicitly scopes
gateway metrics to the M0 auth + proxy surface** (§2 non-goals). Gateway M5's provider-routing,
cost/budget, and cache metrics — for surfaces that do not exist yet — remain that epic's work
and build on this foundation.

---

## 2. Goals / Non-goals

### Goals

- **G1.** A new **`paigasus-observability`** lib crate provides one-call recorder init, a
  `GET /metrics` axum handler (Prometheus exposition), a shared HTTP request-metrics middleware,
  and a `const` metric-name registry — mirroring how `paigasus-logging` centralises tracing.
- **G2.** **`paigasus-iam`** exposes `/metrics` with a bounded-cardinality metric set covering:
  HTTP + gRPC request rate/latency/errors, authz decisions (allow/deny × cache hit/miss),
  audit-write rate, **denial-audit drops** (promoting the `dropped_denial_audits` placeholder to
  a real counter), and the **outbox relay** (drain rate, publish failures, parked count, backlog
  age).
- **G3.** **`paigasus-gateway`** exposes `/metrics` covering its M0 path: HTTP request
  rate/latency/status, in-flight requests, the **IAM dependency** (introspect + authorize
  call rate/latency/outcome), and the **OpenAI upstream** (call rate/latency/status).
- **G4.** **Grafana dashboards** (JSON models, one per service) + **Prometheus** scrape config +
  **alert rules** are committed under `ops/observability/`, and a **docker-compose** local stack
  (Prometheus + Grafana, fully provisioned) scrapes host-run services so an operator can see the
  dashboards with `docker compose up`.
- **G5.** A **RUNBOOK** (`docs/ops/`) documents the metric catalog, scrape/exposition setup, how
  to run the local stack, a dashboard tour, and — for **every alert** — a diagnosis →
  remediation procedure, folding in the operational procedures the existing specs already
  specified (audit retention/partitioning, outbox backlog & parked-event handling, denial-drop
  meaning + capacity tuning, durability tiers, gateway spend-cap / internal-only constraint,
  fail-open authz + Redis-outage behaviour).
- **G6.** Dashboards and alert rules reference **only metrics that are actually emitted** —
  enforced by a **drift test** against the `const` name registry (G1), so the ops artifacts
  cannot silently rot.

### Non-goals (out; tracked elsewhere)

- **Gateway M5** metrics: provider routing / `ProviderAdapter`, cost & budget, response cache,
  rate-limiting — those surfaces do not exist yet (gateway PRD future epic).
- **OpenTelemetry / distributed tracing export** (OTLP spans). We keep the existing JSON
  `tracing` logs; metrics are Prometheus-pull only. OTel export is a future option, noted in §14.
- **Containerising the services** (service `Dockerfile`s / a full app compose). The local stack
  runs Prometheus + Grafana only and scrapes host-run `cargo` services — introducing Docker for
  *observability tooling*, not for the app. Service images are a separate deployment concern.
- **A hosted/prod Prometheus + Grafana deployment**, long-term storage, or paging integration
  (PagerDuty/Opsgenie). Alert *rules* are authored; alert *routing* is deployment-specific.
- **Authenticating `/metrics`** (§6 D4): served unauthenticated per Prometheus convention;
  network-restriction is an operational control documented in the RUNBOOK.
- **New audit/outbox/authz behaviour.** This cycle only *observes* the merged backbone; it adds
  no new domain logic and changes no decision/mutation semantics (the denial-drop counter is
  bumped at the existing drop site; no path is stalled).

---

## 3. Architecture overview

```
  ┌──────────────────────── paigasus-observability (new lib) ────────────────────────┐
  │  init(service) -> PrometheusHandle   (installs global `metrics` recorder,         │
  │                                       metrics-exporter-prometheus, no http feat)  │
  │  metrics_router(handle) -> Router     GET /metrics  →  handle.render()            │
  │  http_metrics_layer()                 axum middleware: requests_total + duration  │
  │                                       + inflight, keyed on MatchedPath (bounded)  │
  │  names::*                             const &str registry = single source of truth│
  └───────────────┬──────────────────────────────────────────────┬───────────────────┘
                  │ (global facade — like `tracing`; no registry   │
                  │  threaded through AppState)                     │
     ┌────────────▼───────────────┐                   ┌────────────▼──────────────────┐
     │ paigasus-iam  (main.rs)     │                   │ paigasus-gateway (main.rs)     │
     │  observability::init(...)   │                   │  observability::init(...)      │
     │  router.merge(metrics_router)                   │  router.merge(metrics_router)  │
     │  + http_metrics_layer       │                   │  + http_metrics_layer          │
     │  + tonic metrics layer      │                   │  chat handler: iam/upstream    │
     │  relay tick → counters/gauges                   │    call counters + durations   │
     │  denial drop site → counter │                   │  inflight gauge                │
     │  authz decision → counter   │                   └────────────────────────────────┘
     └─────────────────────────────┘
                  │ scrape /metrics                                │ scrape /metrics
                  ▼                                                ▼
      ┌───────────────────────── ops/observability (compose) ──────────────────────────┐
      │  Prometheus (scrape iam:8080, gateway:8088 via host.docker.internal)            │
      │           + alert rules   →   Grafana (provisioned datasource + 2 dashboards)   │
      └────────────────────────────────────────────────────────────────────────────────┘
                  ▲ documented + operated by  →  docs/ops/RUNBOOK-observability.md
```

**Why a global facade (`metrics` crate).** It is the metrics analogue of `tracing`: a global
recorder is installed once at startup, and instrumentation sites call `counter!`/`gauge!`/
`histogram!` macros with **zero plumbing** through `AppState`. This matters because our
instrumentation sites are spread across HTTP handlers, a tonic server, a background relay task,
and an authz cache — a threaded `Registry` (tikv `prometheus` crate) would touch all of them and
bloat every `AppState`. The tradeoff (global mutable state) is the same one the repo already
accepts for `tracing` via `paigasus-logging`.

---

## 4. `paigasus-observability` crate

`rs/crates/libs/paigasus-observability/` — `moon.yml` id `paigasus-observability-rs`,
`layer: library`. **No dependency on `paigasus-kernel`** (so `:affected-smoke`'s strict
kernel→bindings set is untouched, SMA-409) and no `getrandom` (keeps the `wasm-getrandom-free`
posture trivially — the crate is server-only and never bound to wasm, but it stays feature-clean).

### 4.1 Public surface

```rust
/// Install the global Prometheus recorder for `service`. Idempotent-friendly: a second call
/// in-process is a no-op (mirrors paigasus_logging::init). Returns the render handle.
pub fn init(service: &str) -> PrometheusHandle;

/// An axum Router serving `GET /metrics` -> text/plain; version=0.0.4 exposition.
pub fn metrics_router(handle: PrometheusHandle) -> axum::Router;

/// axum middleware recording <prefix>_http_requests_total{route,method,status_class},
/// <prefix>_http_request_duration_seconds{route,method}, and <prefix>_http_inflight_requests.
/// `route` is the MatchedPath template (bounded); unmatched paths collapse to "<unmatched>".
pub fn http_metrics_layer(prefix: &'static str) -> /* tower Layer */;

/// Metric-name + label-key constants — the single source of truth (G6 drift test reads these).
pub mod names { /* pub const GATEWAY_HTTP_REQUESTS_TOTAL: &str = "gateway_http_requests_total"; … */ }
```

- **Deps (new workspace deps):** `metrics` and `metrics-exporter-prometheus` with
  **`default-features = false`** (we do **not** use its built-in hyper listener / push-gateway —
  we render via `PrometheusHandle::render()` inside our own axum route, minimising the dep tree
  and the `deny`/`machete` surface). Plus already-present workspace deps `axum`, `tower`.
- **`init` uses `PrometheusBuilder`** with sane default histogram buckets for `*_seconds`
  latencies (e.g. `[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10]`) and `describe_*!`
  help text for every metric family. A `build_info`/`service` label is **not** baked in globally
  (kept per-metric to avoid a high-cardinality constant label); the scrape job label identifies
  the service in Prometheus.
- **Idempotency:** `init` uses `install_recorder()` and treats an already-installed recorder as a
  no-op (like `try_init`), so tests and multi-init are safe.

### 4.2 Cardinality rules (load-bearing; challenge-relevant)

- **Never a label:** `model` (caller-supplied, unbounded), any PRN, key id, principal, prompt/
  body, raw path, or free-form error string. This preserves the gateway's privacy bar
  (`chat.rs:106-109`: never log/label prompt/keys) and prevents cardinality blow-ups.
- **Allowed labels** are closed enums or bounded templates: `route` (MatchedPath template),
  `method` (fixed verbs), `status_class` (`2xx`/`4xx`/`5xx`), `grpc_status` (canonical codes),
  `decision` (`allow`/`deny`), `cache` (`hit`/`miss`), `outcome` (`committed`/`denied`),
  `operation` (`introspect`/`authorize`), `result` (`ok`/`denied`/`unavailable`/`error`).

---

## 5. Metric catalog

Names follow Prometheus conventions: `_total` suffix on counters, base-unit `_seconds`
histograms, snake_case, service prefix. Every name is a `const` in `observability::names`.

### 5.1 Shared HTTP (via `http_metrics_layer`, prefix per service)

| metric | type | labels |
|---|---|---|
| `<svc>_http_requests_total` | counter | `route`, `method`, `status_class` |
| `<svc>_http_request_duration_seconds` | histogram | `route`, `method` |
| `<svc>_http_inflight_requests` | gauge | — |

`/metrics` and `/healthz`/`/readyz` are **excluded** from the layer (or land under their own
`route` template) so scrape traffic doesn't dominate the RED metrics.

### 5.2 `paigasus-iam`

| metric | type | labels | source site |
|---|---|---|---|
| `iam_grpc_requests_total` | counter | `service`, `method`, `grpc_status` | tonic metrics layer (`grpc/mod.rs`) |
| `iam_grpc_request_duration_seconds` | histogram | `service`, `method` | tonic metrics layer |
| `iam_authz_decisions_total` | counter | `decision`, `cache` | `CedarAuthorizer::is_authorized` (compute + cache-hit branch) |
| `iam_audit_records_total` | counter | `outcome`, `result` | `PgAuditLog::record` / `record_out_of_band` |
| `iam_denial_audits_dropped_total` | counter | — | `denial_audit.rs` drop site (replaces the 60s tracing poll) |
| `iam_denial_audits_enqueued_total` | counter | — | `denial_audit.rs` enqueue |
| `iam_outbox_relay_ticks_total` | counter | `result` (`ok`/`error`) | relay tick (`relay.rs`) |
| `iam_outbox_relay_drained_total` | counter | — | `TickReport.drained` |
| `iam_outbox_relay_publish_failures_total` | counter | — | `TickReport.failures` |
| `iam_outbox_relay_parked` | gauge | — | `TickReport.parked` |
| `iam_outbox_oldest_unpublished_age_seconds` | gauge | — | `TickReport.oldest_unpublished_age_secs` |

- **Authz decisions** are the highest-value operational signal; the counter is bumped inside
  `is_authorized` on **both** the compute path and the cache-hit branch
  (`cedar_authorizer.rs:157-163`) so allow/deny volume and cache effectiveness are both visible —
  a non-blocking `counter!` bump, adding no Postgres I/O to the hot path.
- **Denial drops:** the counter is bumped **at the existing drop-oldest site** in
  `denial_audit.rs` (where `dropped` is incremented), making the metric live without the 60s
  poll. The `tracing::warn!` is retained for log visibility; the standalone 60s ticker task in
  `main.rs:121-151` is **removed** (superseded — a monotonic counter needs no periodic re-emit).
- **Relay:** the tick loop bumps the counters/gauges from the `TickReport` it already computes —
  no new query, just an emit alongside the existing `tracing::info!`.

### 5.3 `paigasus-gateway`

| metric | type | labels | source site |
|---|---|---|---|
| `gateway_http_requests_total` | counter | `route`, `method`, `status_class` | shared HTTP layer |
| `gateway_http_request_duration_seconds` | histogram | `route`, `method` | shared HTTP layer |
| `gateway_http_inflight_requests` | gauge | — | shared HTTP layer |
| `gateway_iam_calls_total` | counter | `operation`, `result` | `chat`/`auth` around `Iam::introspect_api_key` + `is_authorized_self` |
| `gateway_iam_call_duration_seconds` | histogram | `operation` | same |
| `gateway_upstream_requests_total` | counter | `status_class` | `chat` around `OpenAiClient::chat_completion` |
| `gateway_upstream_request_duration_seconds` | histogram | — | same |

- `operation` ∈ {`introspect`, `authorize`}; `result` ∈ {`ok`, `denied`, `unavailable`,
  `error`} — maps the M0 failure taxonomy (401/403/500/503) to a bounded outcome label without
  leaking status text.
- The gateway gains its **first** middleware layer (`http_metrics_layer`); this cycle does **not**
  add `tower-http`/`TraceLayer` (out of scope — request logging already exists at `chat.rs:110`).
  If the metrics layer needs `tower`/`axum::middleware`, both are already workspace deps.

---

## 6. Endpoint, config & security

### 6.1 Exposition endpoint

- `GET /metrics` is `merge`d into each service's existing axum router on the **existing HTTP
  listener** (gateway `8088`, IAM `8080`). IAM's gRPC port is unchanged (no `/metrics` over
  gRPC). Content-Type `text/plain; version=0.0.4`.
- Guarded by `[metrics] enabled` (default `true`). When `false`, the route is not mounted and the
  recorder is not installed (instrumentation macros become cheap no-ops against the default
  no-op recorder) — a `tracing::info!` notes metrics are disabled at startup.

### 6.2 Config

New `[metrics]` table in both `GatewayConfig` and `IamConfig` + their `.toml.example` files:

```toml
[metrics]
enabled = true   # expose GET /metrics on the service HTTP port
```

`validate()` needs no new bounds (single bool). Env override via `GATEWAY_METRICS__ENABLED` /
`IAM_METRICS__ENABLED` (figment `__` nesting).

### 6.3 D4 — `/metrics` is unauthenticated; network-restriction is operational

`/metrics` is served **unauthenticated** (Prometheus scrape convention; the exposition carries no
secrets — only counters/gauges with bounded labels). The RUNBOOK documents that the metrics port
**must be network-restricted** (private network / firewall / service mesh), which aligns with the
gateway M0 constraint that M0 runs internal-only or behind a hard spend cap
(`gateway-m0` doc §5/D6). A separate metrics *port* is noted as a future hardening option (§14)
but rejected for v1 (doubles listener wiring on both services for no v1 benefit given the
internal-only posture).

---

## 7. Ops artifacts (`ops/observability/`)

```
ops/observability/
  docker-compose.yml            # Prometheus + Grafana; scrapes host.docker.internal:{8080,8088}
  prometheus/
    prometheus.yml              # scrape config (iam, gateway jobs) + rule_files
    rules/iam.rules.yml         # IAM alert rules
    rules/gateway.rules.yml     # gateway alert rules
  grafana/
    provisioning/datasources/prometheus.yml
    provisioning/dashboards/dashboards.yml
    dashboards/iam.json
    dashboards/gateway.json
  README.md                     # quick "docker compose up" pointer into the RUNBOOK
```

- **Scrape model:** Prometheus scrapes `host.docker.internal:8080/metrics` (IAM) and
  `:8088/metrics` (gateway) — the operator runs the services via `cargo run` on the host. No
  service Dockerfiles (§2 non-goal).
- **Grafana** is fully provisioned (datasource + dashboard providers) so the stack is
  zero-click after `up`. Default anon-admin, localhost-only (documented as dev-only).

### 7.1 Dashboards (Grafana JSON models)

- **`iam.json` — Paigasus IAM:** RED (req rate / p50-p95-p99 latency / error ratio) for HTTP &
  gRPC; authz decisions allow-vs-deny rate + cache hit ratio; audit write rate by outcome;
  **denial-audit drop rate** (should be flat 0); **outbox** panel row — drain rate, publish
  failures, parked count, **oldest-unpublished-age** (backlog lag, the key SLO).
- **`gateway.json` — AI Gateway (M0):** RED for HTTP; status_class breakdown (401/403/413/
  5xx); **IAM dependency** latency + outcome (introspect/authorize); **OpenAI upstream** latency
  + status_class; in-flight requests.

### 7.2 Alert rules

| rule | expr (sketch) | severity |
|---|---|---|
| `IamDenialAuditDrops` | `rate(iam_denial_audits_dropped_total[5m]) > 0` | warning |
| `IamOutboxBacklogAgeHigh` | `iam_outbox_oldest_unpublished_age_seconds > 300` | warning |
| `IamOutboxEventsParked` | `iam_outbox_relay_parked > 0` | warning |
| `IamHighErrorRate` | 5xx ratio `> 5%` for 10m | critical |
| `GatewayHighErrorRate` | 5xx ratio `> 5%` for 10m | critical |
| `GatewayIamDependencyUnavailable` | `rate(gateway_iam_calls_total{result="unavailable"}[5m]) > 0` | critical |
| `GatewayUpstreamErrors` | `rate(gateway_upstream_requests_total{status_class="5xx"}[5m])` high | warning |
| `TargetDown` | `up == 0` for 2m | critical |

Thresholds are documented as **starting points** in the RUNBOOK (tune per environment).

---

## 8. RUNBOOK (`docs/ops/RUNBOOK-observability.md`)

The operator-facing document. Sections:

1. **Overview** — what's instrumented, the pull model, where `/metrics` lives per service.
2. **Metric catalog** — every metric: name, type, labels, meaning, expected range. Generated to
   match §5 exactly (and kept honest by the G6 drift test).
3. **Run the local stack** — `docker compose up` in `ops/observability/`, run the two services,
   open Grafana, tour each dashboard.
4. **Alerts → runbook entries** — for each §7.2 alert: what it means, likely causes, how to
   confirm, and remediation. Folds in the backbone procedures already specified by the merged
   specs:
   - **Denial-audit drops** (`IamDenialAuditDrops`): denial rows are **best-effort/observably
     lossy** under saturation (audit/outbox spec D8); meaning of a non-zero drop counter, and how
     to raise `[audit].denial_buffer_capacity`.
   - **Outbox backlog / parked** (`IamOutboxBacklogAgeHigh`, `IamOutboxEventsParked`): the relay
     drains `FOR UPDATE SKIP LOCKED` (D12); what a growing `oldest_unpublished_age` means (relay
     down / publisher failing), where parked (poison) events go, and manual replay — noting the
     full DLQ + pruning are §14 follow-ups.
   - **Durability tiers:** mutation audit rows are **exactly-once**; denial rows **best-effort** —
     stated so on-call reads the drop counter correctly.
   - **Audit retention/partitioning:** the monthly-partition + outcome-aware retention policy
     from the audit/outbox spec §4/D14 — the RUNBOOK operationalises the already-specified
     `DROP`/detach of old denial partitions.
   - **Gateway posture:** M0 must run **internal-only or behind a hard OpenAI spend cap** (no
     rate-limit/budget until gateway M3/M4) — gateway-m0 doc D6; plus the `/metrics`
     network-restriction control (§6.3).
   - **Authz availability:** current **fail-open-on-Redis-outage** posture and TTL-bounded
     revocation freshness (audit/outbox spec §1, M3 handoff) — so a Redis alert is interpreted
     correctly.
5. **Cardinality & privacy** — why `model`/PRNs are never labels; how to add a metric safely.
6. **Future** — OTel export, separate metrics port, hosted stack, paging (all §14).

---

## 9. Testing strategy

- **`paigasus-observability` unit tests:** `init` installs a recorder and a subsequent
  `counter!` bump renders in `handle.render()`; `metrics_router` returns `200` +
  `text/plain; version=0.0.4`; `http_metrics_layer` records a request with the MatchedPath
  `route` label and a `status_class`; a second `init` is a no-op.
- **`paigasus-iam` integration (real Postgres via `tests/support`):** after exercising an authz
  decision, `GET /metrics` contains `iam_authz_decisions_total`; after a forced denial-buffer
  overflow, `iam_denial_audits_dropped_total` is present and > 0; a relay tick surfaces
  `iam_outbox_oldest_unpublished_age_seconds`. `[metrics] enabled=false` omits the route (404).
- **`paigasus-gateway` integration:** with the existing IAM+OpenAI fakes, a proxied request
  increments `gateway_http_requests_total` and records `gateway_upstream_request_duration_seconds`
  and `gateway_iam_calls_total`.
- **G6 drift test** (`observability` or a small `ops`-adjacent test): parse the committed Grafana
  dashboard JSON + the Prometheus rule YAML, extract every referenced metric name (PromQL), and
  assert each is present in `observability::names`. Fails CI if a dashboard/alert references a
  metric that isn't emitted (or is misspelled). It **whitelists Prometheus built-ins** (`up`,
  and PromQL scrape/aggregation names like `scrape_duration_seconds`) so `TargetDown`'s `up == 0`
  does not false-fail; the whitelist is a small explicit `const` set beside the name registry.
- **Config tests:** `[metrics]` parses/defaults/env-overrides in both services (mirrors existing
  config tests).

---

## 10. CI / gate considerations

- **New crate `paigasus-observability`** → `Cargo.toml` `[workspace] members`, a `moon.yml`
  (`paigasus-observability-rs`, `layer: library`), and `dependsOn` added to **both service
  `moon.yml`s** (`paigasus-observability-rs`).
- **New workspace deps** `metrics` + `metrics-exporter-prometheus` (default-features off) →
  expect **`rs/deny.toml` `[licenses] exceptions`** review and possibly a temporary
  **`cargo-machete` `ignored`** allowlist if a dep is introduced a commit before it's consumed
  (prune once consumed). Pin exact versions during implementation.
- **`:affected-smoke`** — observability does **not** depend on `paigasus-kernel`, so the strict
  kernel→bindings expected set (`ci/affected-graph/run.sh`, SMA-409) is **untouched**. But adding
  a new crate + editing two services shifts the affected graph — run the **full** gate list as CI
  does:
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
  :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.
- **No proto changes** → no `buf format`/codegen-drift/`:breaking` risk from this cycle (the
  audit gRPC surface already exists from #80/#81; we only add a metrics *layer* around tonic, no
  `.proto` edit).
- **`ts:fmt`/py gates** untouched (no ts/py changes). Grafana JSON + YAML live under `ops/` — not
  under a Prettier-governed tree; confirm no gate claims them (they shouldn't).

---

## 11. Decision log

| # | Decision | Rationale |
|---|---|---|
| **D1** | **`metrics` facade + `metrics-exporter-prometheus`** (not tikv `prometheus`) | Global recorder = zero `AppState` plumbing across HTTP/gRPC/relay/authz sites; the `tracing`-analogue the repo already embraces |
| **D2** | A **new `paigasus-observability` lib** owns init + `/metrics` + HTTP layer + name registry | DRY across two services; mirrors `paigasus-logging`; a single source of truth for names (enables D-drift test) |
| **D3** | **Both services** instrumented; gateway **scoped to M0** auth+proxy | Closes the epic's observability bullet; avoids doing gateway-M5's provider/cost/cache metrics against non-existent surfaces |
| **D4** | `/metrics` **unauthenticated on the existing HTTP port**; network-restriction is operational | Prometheus convention; exposition has no secrets; matches M0 internal-only posture. Separate port deferred (§14) |
| **D5** | **Promote** `dropped_denial_audits` (and relay `TickReport`) from tracing-only to real metrics; **remove** the 60s poll task | A monotonic counter bumped at the drop site needs no periodic re-emit; relay already computes the numbers |
| **D6** | Full ops kit: dashboards + scrape config + **alert rules** + **docker-compose** local stack + RUNBOOK | Highest operational value; sets the repo's ops precedent; makes the RUNBOOK runnable |
| **D7** | Local stack scrapes **host-run services** (no service Dockerfiles) | Keeps scope to observability tooling, not app containerisation (a separate deployment concern) |
| **D8** | **Bounded-cardinality labels only**; `model`/PRNs/paths never labels | Prevents cardinality blow-ups and preserves the gateway privacy bar |
| **D9** | A **dashboard/alert ↔ name-registry drift test** | Ops artifacts can't silently reference non-existent/misspelled metrics |
| **D10** | Implementable in **two slices** (A: lib + `/metrics` + instrumentation; B: dashboards + alerts + compose + RUNBOOK) | A-first ships the exposition surface; B is pure ops artifacts atop it. Plan may land as one PR if size allows |

## 12. Implementation slices (D10)

- **Slice A — instrumentation:** `paigasus-observability` crate (init, `metrics_router`,
  `http_metrics_layer`, `names`); `[metrics] enabled` config in both services; IAM metrics
  (HTTP layer, tonic layer, authz-decision counter, audit counter, denial-drop counter promotion
  + 60s-task removal, relay gauges/counters); gateway metrics (HTTP layer, IAM-call + upstream
  counters/histograms, inflight); all §9 code tests. **PR 1 ("Part of SMA-446").**
- **Slice B — ops artifacts:** `ops/observability/` (compose, Prometheus config + rules, Grafana
  provisioning + 2 dashboards); the RUNBOOK; the G6 drift test. **PR 2 ("Part of SMA-446",
  closes the epic).**

  Both slices are designed together (this spec); the plan may land B as one PR if review size
  allows, but A-first is the sequencing (dashboards need emitted metrics to reference).

## 13. Follow-ups (not this cycle)

- **Gateway M5** observability: provider/cost/cache/rate-limit metrics + gateway dashboards
  extension (builds on this crate).
- **OpenTelemetry** OTLP trace/metric export (correlate with the JSON logs).
- **Separate metrics port** / mTLS on the scrape endpoint (hardening).
- **Hosted stack + paging** (Alertmanager routing to PagerDuty/Opsgenie), long-term storage.
- **Outbox pruning + full DLQ** (already an audit/outbox-spec §14 follow-up; the RUNBOOK
  documents the interim manual procedure).

## 14. Open items resolved by defaults (called out for the challenge)

- `/metrics` port sharing + unauthenticated (D4) — accepted for v1.
- Local stack scrapes host services (D7) — accepted; RUNBOOK covers the `host.docker.internal`
  caveat on Linux (`--add-host=host.docker.internal:host-gateway`).
- Gateway gains a middleware layer but **not** `tower-http`/`TraceLayer` (§5.3).
- `metrics-exporter-prometheus` used **without** its http-listener feature (render via handle).
