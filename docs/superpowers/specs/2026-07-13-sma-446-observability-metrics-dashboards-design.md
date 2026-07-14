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
/// Install the global Prometheus recorder (once per process) and return the render handle.
/// A second in-process call returns a CLONE of the cached first handle — never a freshly-built,
/// disconnected one (see §4.3). Mirrors paigasus_logging::init's idempotency.
pub fn init(service: &str) -> PrometheusHandle;

/// An axum Router serving `GET /metrics` -> text/plain; version=0.0.4 exposition.
pub fn metrics_router(handle: PrometheusHandle) -> axum::Router;

/// axum middleware recording <prefix>_http_requests_total{route,method,status_class},
/// <prefix>_http_request_duration_seconds{route,method}, and <prefix>_http_inflight_requests.
/// `route` is the MatchedPath template (bounded); unmatched paths collapse to "<unmatched>".
pub fn http_metrics_layer(prefix: &'static str) -> /* tower Layer */;

/// One-line gRPC instrumentation for a tonic handler boundary: records
/// iam_grpc_requests_total{service,method,grpc_status} + iam_grpc_request_duration_seconds
/// from a COMPLETED handler Result. `service`/`method` are static &'static str literals passed by
/// the caller (never derived from the request `:path`), `grpc_status` from the Result (§4.3).
pub fn record_grpc<T>(service: &'static str, method: &'static str, started: Instant,
                      result: &Result<T, tonic::Status>);

/// Metric-name + label-key constants — the single source of truth (G6 drift test reads these).
pub mod names { /* pub const GATEWAY_HTTP_REQUESTS_TOTAL: &str = "gateway_http_requests_total"; … */ }
```

- **Deps (new workspace deps):** `metrics` and `metrics-exporter-prometheus` with
  **`default-features = false`** (we do **not** use its built-in hyper listener / push-gateway —
  we render via `PrometheusHandle::render()` inside our own axum route, minimising the dep tree
  and the `deny`/`machete` surface). `metrics` is **also a direct dep of both service crates**
  (the `counter!`/`gauge!`/`histogram!` macros are called at instrumentation sites inside
  `paigasus-iam`/`paigasus-gateway`, not only inside this lib). Plus already-present workspace
  deps `axum`, `tower`, `tonic` (types only, for `record_grpc`). **Verify the resolved dep tree
  before merge** — `metrics-exporter-prometheus` with `default-features = false` must not pull a
  second `hyper`/`hyper-util` (http-listener) that would aggravate the known CI-disk-exhaustion
  with the cedar-bloated tree.
- **`init` uses `PrometheusBuilder`** with sane default histogram buckets for `*_seconds`
  latencies (e.g. `[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10]`) and `describe_*!`
  help text for every metric family. No constant `service` label is baked onto metrics (avoids a
  high-cardinality constant label); the Prometheus scrape **`job`** label identifies the service.
  The `service` argument to `init` is used **only** for a one-time startup `tracing::info!`
  ("metrics recorder installed for <service>").

### 4.3 Global-recorder lifecycle (challenge finding — correctness)

`metrics_exporter_prometheus::PrometheusBuilder::install_recorder()` **consumes** the recorder
built alongside the handle and can succeed **at most once per process**. A naive "second call is a
no-op that still returns a freshly-built handle" would return a handle **not wired to the installed
global recorder**, whose `render()` is permanently empty — silently breaking `/metrics` and any
test that builds the app twice. Therefore:

- `init` builds `(recorder, handle)` **once**, installs the recorder, and stores the handle in a
  crate-level `OnceLock<PrometheusHandle>`. Every call (first or subsequent) returns
  `handle.clone()` from that `OnceLock` (`PrometheusHandle` is `Clone` and shares the recorder).
- **Test isolation** relies on the recorder being process-global and counters being cumulative:
  under **`cargo nextest` (process-per-test)** each test gets a fresh process, so counters don't
  leak between tests. Under `cargo test` (shared process) they would — the repo runs nextest, and
  the observability tests assert on **presence/deltas**, not absolute totals, to stay robust
  either way.

### 4.2 Cardinality rules (load-bearing; challenge-relevant)

- **Never a label:** `model` (caller-supplied, unbounded), any PRN, key id, principal, prompt/
  body, raw path, or free-form error string. This preserves the gateway's privacy bar
  (`chat.rs:106-109`: never log/label prompt/keys) and prevents cardinality blow-ups.
- **Allowed labels** are closed enums or bounded templates: `route` (MatchedPath template),
  `method` (fixed verbs), `status_class` (`2xx`/`4xx`/`5xx`), `grpc_status` (canonical codes),
  `decision` (`allow`/`deny`), `cache` (`hit`/`miss`/`bypass` — `bypass` distinguishes a
  Redis-outage fail-open that skips the cache entirely, `cedar_authorizer.rs:129-137`, from a
  genuine miss), `outcome` (`committed`/`denied`), `operation` (`introspect`/`authorize`),
  `result` (`ok`/`denied`/`unavailable`/`error`).
- **gRPC labels are static, never `:path`-derived.** `service`/`method` are compile-time string
  literals supplied at each handler site (§5.2) — a scanning client hitting `/foo/bar` **cannot**
  mint label values, so the tonic surface has no MatchedPath equivalent and needs none.

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
`route` template) so scrape traffic doesn't dominate the RED metrics. For **IAM**, `/metrics` and
health are **also excluded from the existing `TraceLayer`** (`http/mod.rs:698-700`) — otherwise
every ~15s scrape emits a request-span log. (Achieved by mounting `/metrics` + health outside the
`TraceLayer`-wrapped subtree, or a span filter; the plan picks the mechanism.)

### 5.2 `paigasus-iam`

| metric | type | labels | source site |
|---|---|---|---|
| `iam_grpc_requests_total` | counter | `service`, `method`, `grpc_status` | `observability::record_grpc` at each tonic handler boundary |
| `iam_grpc_request_duration_seconds` | histogram | `service`, `method` | same |
| `iam_authz_decisions_total` | counter | `decision`, `cache` | `CedarAuthorizer::is_authorized` (compute + cache-hit branch) |
| `iam_audit_records_total` | counter | `outcome`, `result` | `PgAuditLog::record` / `record_out_of_band` |
| `iam_denial_audits_dropped_total` | counter | — | `DenialAuditBuffer::push` drop site (`denial_audit.rs:73-75`) |
| `iam_denial_audits_enqueued_total` | counter | — | `DenialAuditBuffer::push` enqueue |
| `iam_outbox_relay_ticks_total` | counter | `result` (`ok`/`error`) | relay tick (`relay.rs`) — also the liveness signal |
| `iam_outbox_relay_drained_total` | counter | — | `TickReport.drained` (rows *processed*, incl. retries/failures) |
| `iam_outbox_relay_published_total` | counter | — | `TickReport.drained − failures` (rows *successfully published*) |
| `iam_outbox_relay_publish_failures_total` | counter | — | `TickReport.failures` |
| `iam_outbox_relay_parked_total` | counter | — | `TickReport.parked` summed per tick (**counter, not gauge** — see below) |
| `iam_outbox_oldest_unpublished_age_seconds` | gauge | — | `TickReport.oldest_unpublished_age_secs` (`None` → set `0`) |

- **Authz decisions** are the highest-value operational signal; the counter is bumped inside
  `is_authorized` on **both** the compute path and the cache-hit branch
  (`cedar_authorizer.rs:159-168`) so allow/deny volume and cache effectiveness are both visible —
  a non-blocking `counter!` bump, adding no Postgres I/O to the hot path. `cache="bypass"` is
  recorded when the entity-generation read fails and the call skips the cache (fail-open,
  `cedar_authorizer.rs:129-137`).
- **Audit records semantics (challenge finding):** `iam_audit_records_total{result="ok"}` counts
  audit inserts that **did not error**, not rows durably committed — an in-txn `PgAuditLog::record`
  bump precedes the enclosing UoW `commit`, so a subsequent (rare, exceptional) rollback leaves the
  row invisible while the counter already incremented. Documented as insert-attempts-not-errored;
  the divergence only appears on the mutation-error path (itself a `result="error"` signal
  elsewhere), so it does not mislead in steady state.
- **gRPC (challenge finding — no tower trailer layer):** instrumented at each **tonic handler
  boundary** via `observability::record_grpc(SERVICE, METHOD, started, &result)`. The handler
  already holds the `Result<Response, Status>`, so `grpc_status` comes from the returned value
  (no response-trailer inspection, which a tower `Server::layer` would require), and
  `service`/`method` are **static literals** (no unbounded `:path`-derived labels). One helper
  line per RPC across the bounded, known set of IAM services (Tenancy / Authn / Authorization /
  ServiceAccount / Audit). This is what lets `IamGrpcHighErrorRate` (§7.2) exist — the gateway's
  gRPC calls into IAM are otherwise only observable **client-side** (`gateway_iam_calls_total`).
- **Denial drops (challenge finding):** the counter is bumped **at the existing drop-oldest site**
  in `DenialAuditBuffer::push` (`denial_audit.rs:73-75`, where `dropped` is incremented),
  alongside `iam_denial_audits_enqueued_total` on each `push`. The standalone 60s ticker task in
  `main.rs:121-151` is **removed** (a monotonic counter needs no periodic re-emit). **No warn is
  added at the drop site** — that would log-spam during the exact denial burst that causes
  overflow (the removed ticker's whole point was to throttle); the counter +
  `IamDenialAuditDrops` alert (§7.2) provide visibility. Removing the ticker also orphans
  `AppState::denial_buffer()` and `DenialAuditBuffer::dropped()` if unused elsewhere — the plan
  removes the now-dead accessors so the workspace `warnings = "deny"` (`Cargo.toml`) stays green.
- **Relay (challenge findings):** the tick loop bumps counters/gauges from the `TickReport` it
  already computes (no new query), alongside the existing `tracing::info!`.
  - `iam_outbox_relay_parked_total` is a **counter** summing `TickReport.parked` (rows newly
    parked *this tick*), **not a gauge** — `TickReport.parked` is a per-tick delta and
    already-parked rows are filtered out of future batches (`relay.rs:107`), so a gauge would read
    `0` on every tick that parks nothing new, hiding a growing parked backlog. Alert on
    `increase(iam_outbox_relay_parked_total[15m]) > 0`.
  - `iam_outbox_oldest_unpublished_age_seconds` is only as fresh as `poll_interval_secs` and
    **freezes if the relay task wedges while the process stays alive** — so the real relay-health
    signal is `iam_outbox_relay_ticks_total` (a stalled-but-alive relay stops incrementing it,
    which `TargetDown` (`up==0`) would miss). Alert `IamOutboxRelayStalled` on
    `rate(iam_outbox_relay_ticks_total[10m]) == 0` (§7.2).

### 5.3 `paigasus-gateway`

| metric | type | labels | source site |
|---|---|---|---|
| `gateway_http_requests_total` | counter | `route`, `method`, `status_class` | shared HTTP layer |
| `gateway_http_request_duration_seconds` | histogram | `route`, `method` | shared HTTP layer |
| `gateway_http_inflight_requests` | gauge | — | shared HTTP layer |
| `gateway_iam_calls_total` | counter | `operation`, `result` | **`auth.rs` middleware** (`require_iam_auth`, `auth.rs:57,84`) |
| `gateway_iam_call_duration_seconds` | histogram | `operation` | same |
| `gateway_upstream_requests_total` | counter | `status_class` | `chat` around `OpenAiClient::chat_completion` (`chat.rs:78`) |
| `gateway_upstream_request_duration_seconds` | histogram | — | same |

- `operation` ∈ {`introspect`, `authorize`}; `result` ∈ {`ok`, `denied`, `unavailable`,
  `error`} — maps the M0 failure taxonomy (401/403/500/503) to a bounded outcome label without
  leaking status text.
- **Source site (challenge correction):** the IAM calls live in the **`require_iam_auth`
  middleware** (`auth.rs:57` introspect, `:84` authorize), **not** the `chat` handler (which never
  calls IAM). `gateway_iam_calls_total`/`_duration` are recorded there.
- **Streaming = time-to-first-byte (challenge finding):** for an SSE response,
  `OpenAiClient::chat_completion` returns the head immediately and the body streams lazily *after*
  the handler returns (`chat.rs:86-95`). So `gateway_upstream_request_duration_seconds` and the
  HTTP duration histogram measure **TTFB**, not full stream duration, and a **mid-stream terminal
  SSE error is not counted** as an upstream error (it happens in the streamed body, past the
  measured boundary). Stated explicitly in the metric catalog + RUNBOOK so latency panels aren't
  misread as end-to-end.
- The gateway gains its **first** middleware layer (`http_metrics_layer`); this cycle does **not**
  add `tower-http`/`TraceLayer` (out of scope — request logging already exists at `chat.rs:110`).
  If the metrics layer needs `tower`/`axum::middleware`, both are already workspace deps.

---

## 6. Endpoint, config & security

### 6.1 Exposition endpoint

- `GET /metrics` is exposed on **one of two shapes**, chosen by config:
  - `[metrics].addr` **unset** (default) → `/metrics` is `merge`d into the service's existing HTTP
    router (gateway `8088`, IAM `8080`).
  - `[metrics].addr = "<ip:port>"` **set** → `/metrics` is served on its **own listener** bound to
    that (internal) address, via a second small `axum::serve` task on the same graceful-shutdown
    watch; it is **not** mounted on the public router. This is the recommended shape for any
    **non-internal gateway** deployment (§6.3).
  - IAM's gRPC port is unchanged (no `/metrics` over gRPC). Content-Type `text/plain; version=0.0.4`.
- Guarded by `[metrics] enabled` (default `true`). When `false`, neither shape is mounted and the
  recorder is not installed (instrumentation macros become cheap no-ops against `metrics`' default
  no-op recorder) — a `tracing::info!` notes metrics are disabled at startup.

### 6.2 Config

New `[metrics]` table in both `GatewayConfig` and `IamConfig` + their `.toml.example` files:

```toml
[metrics]
enabled = true        # install the recorder + expose GET /metrics
# addr  = "127.0.0.1:9091"   # optional: serve /metrics on its OWN (internal) listener instead of
                             # the main HTTP port. RECOMMENDED for a non-internal gateway (§6.3).
```

`validate()` bounds: `addr`, when present, is a parseable `SocketAddr` and must not equal the
service's `http_addr` (a same-addr collision is a config error, not a silent merge). Env override
via `GATEWAY_METRICS__ENABLED` / `GATEWAY_METRICS__ADDR` / `IAM_METRICS__*` (figment `__` nesting).

### 6.3 D4 — `/metrics` is unauthenticated; exposure is a bind/network control

`/metrics` is served **unauthenticated** (Prometheus scrape convention; the exposition carries no
secrets — only counters/gauges with bounded labels). Exposure is controlled by **where it binds**,
not by auth:

- **IAM** is an internal service (only the gateway calls it, privately) → same-port `/metrics` is
  fine; the RUNBOOK still says the port must be network-restricted.
- **Gateway (challenge finding):** the gateway-M0 posture is "internal-only **OR** behind a hard
  spend cap" (`gateway-m0` doc §5/D6). In the **spend-capped-but-public** mode, same-port
  `/metrics` would be reachable **unauthenticated by any external caller** that can reach
  `/v1/chat/completions` — leaking request volumes, error rates, and upstream/IAM latencies
  (recon-grade disclosure), guarded only by fragile L7 path-filtering on a shared port. Therefore
  the RUNBOOK **mandates** `[metrics].addr` (a separate internal bind) for any non-internal gateway
  deployment; same-port unauth `/metrics` is **only** acceptable when the whole listener is
  network-isolated (the internal-only mode). mTLS on the scrape endpoint remains a future
  hardening option (§14).

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
- **Cross-platform host access (challenge finding):** `host.docker.internal` resolves on Docker
  Desktop (macOS/Windows) but **not** on native Linux Docker without help — the compose file bakes
  in `extra_hosts: ["host.docker.internal:host-gateway"]` on the Prometheus service (the compose
  equivalent of `--add-host`; a `docker run` flag would not apply). Documented in the RUNBOOK.
- **Reproducibility:** Prometheus and Grafana images are **pinned to explicit version tags** (no
  `:latest`).
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
| `IamOutboxEventsParked` | `increase(iam_outbox_relay_parked_total[15m]) > 0` | warning |
| `IamOutboxRelayStalled` | `rate(iam_outbox_relay_ticks_total[10m]) == 0` | critical |
| `IamHighErrorRate` | HTTP 5xx ratio `> 5%` for 10m (HTTP-only — gRPC has no 5xx) | critical |
| `IamGrpcHighErrorRate` | non-OK `grpc_status` ratio `> 5%` for 10m | critical |
| `GatewayHighErrorRate` | HTTP 5xx ratio `> 5%` for 10m | critical |
| `GatewayIamDependencyUnavailable` | `rate(gateway_iam_calls_total{result="unavailable"}[5m]) > 0` | critical |
| `GatewayUpstreamErrors` | `rate(gateway_upstream_requests_total{status_class="5xx"}[5m])` high | warning |
| `TargetDown` | `up == 0` for 2m | critical |

Notes: `IamOutboxEventsParked` uses `increase(...parked_total...)` (a counter of newly-parked
rows), not a gauge that would flap to 0 (§5.2). `IamOutboxRelayStalled` catches a
**stalled-but-alive relay** that `TargetDown` misses (the process is `up`, but no ticks). gRPC
gets its own error alert because gRPC responses are HTTP 200 + a `grpc-status` trailer, so
`IamHighErrorRate`'s 5xx ratio can't see gRPC failures. Thresholds are documented as **starting
points** in the RUNBOOK (tune per environment).

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
   - **Outbox backlog / parked / stalled** (`IamOutboxBacklogAgeHigh`, `IamOutboxEventsParked`,
     `IamOutboxRelayStalled`): the relay drains `FOR UPDATE SKIP LOCKED` (D12); what a growing
     `oldest_unpublished_age` means (publisher failing), why the age gauge **freezes** on a wedged
     relay so `IamOutboxRelayStalled` (no ticks) is the true liveness alert, where parked (poison)
     events go and manual replay — noting the full DLQ + pruning are §14 follow-ups.
   - **gRPC errors** (`IamGrpcHighErrorRate`): IAM's gRPC surface returns HTTP 200 + a
     `grpc-status` trailer, so the HTTP 5xx alert can't see it; this alert watches the non-OK
     `grpc_status` ratio (the gateway's introspect/authorize calls are the hot path).
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
6. **Future** — OTel export, mTLS scrape endpoint, hosted stack, paging (all §14).

---

## 9. Testing strategy

- **`paigasus-observability` unit tests:** after `init`, a `counter!` bump renders in
  `handle.render()`; `metrics_router` returns `200` + `text/plain; version=0.0.4`;
  `http_metrics_layer` records a request with the MatchedPath `route` label and a `status_class`;
  **a second `init` returns a handle whose `render()` still reflects the global recorder** (the
  §4.3 `OnceLock` fix — not a disconnected empty handle); `record_grpc` records a static
  `service`/`method` and the `grpc_status` from an `Err(Status::…)`.
- **Test isolation:** assertions check **presence / deltas**, never absolute cumulative totals,
  because the recorder is process-global (§4.3); the repo runs `cargo nextest` (process-per-test)
  so no cross-test leakage in practice.
- **`paigasus-iam` integration (real Postgres via `tests/support`):** after exercising an authz
  decision, `GET /metrics` contains `iam_authz_decisions_total`; after a forced denial-buffer
  overflow, `iam_denial_audits_dropped_total` is present and > 0; a relay tick surfaces
  `iam_outbox_relay_ticks_total` and (with a non-empty batch) `iam_outbox_oldest_unpublished_age_seconds`.
  `[metrics] enabled=false` omits the route (404); `[metrics].addr` set serves `/metrics` on the
  separate listener and **not** on the main port.
- **`paigasus-gateway` integration:** with the existing IAM+OpenAI fakes, a proxied request
  increments `gateway_http_requests_total` and records `gateway_upstream_request_duration_seconds`
  and `gateway_iam_calls_total` (from the auth middleware).
- **Prometheus artifacts validated by `promtool` (challenge finding):** a CI step runs
  `promtool check config` + `promtool check rules` (valid PromQL/YAML) and **`promtool test rules`**
  (unit-tests that each alert fires on a synthetic series — which also pins the alert expressions'
  metric names). `promtool` is added as a **proto-pinned CLI** (mirroring the cargo-deny/machete
  pattern), not a cargo dep.
- **G6 name-drift test (revised — no bespoke PromQL parser):** a small test extracts metric
  identifiers from the committed **Grafana dashboard JSON** (`serde_json` over `panels[].targets[].expr`)
  and the **rule YAML** `expr` fields, then asserts each is in `observability::names` — after
  (a) **stripping histogram/summary suffixes** `_bucket`/`_sum`/`_count` (a registered
  `foo_seconds` family is referenced as `foo_seconds_bucket` etc., so an un-normalized match would
  false-fail every latency panel) and (b) **whitelisting** Prometheus built-ins (`up`,
  `scrape_*`) and PromQL function/keyword tokens (`rate`, `sum`, `histogram_quantile`, `by`, …,
  a fixed `const` set beside the registry) and template vars (`$…`). Reading the rule YAML needs a
  **test-only** YAML dep (e.g. `serde_norway`/`serde_yaml`) — budgeted in §10; the dashboard side
  needs only `serde_json` (already present). The heuristic is deliberately simple; `promtool`
  carries the burden of PromQL *validity*, this test only guards **name drift**.
- **Config tests:** `[metrics]` parses/defaults/env-overrides + the `addr != http_addr` validation
  in both services (mirrors existing config tests).

---

## 10. CI / gate considerations

- **New crate `paigasus-observability`** → `Cargo.toml` `[workspace] members`, a `moon.yml`
  (`paigasus-observability-rs`, `layer: library`), and `dependsOn` added to **both service
  `moon.yml`s** (`paigasus-observability-rs`).
- **New workspace deps** `metrics` + `metrics-exporter-prometheus` (default-features off), with
  `metrics` **also a direct dep of both service crates** (macros at instrumentation sites) and a
  **test-only** YAML dep (`serde_norway`/`serde_yaml`) for the drift test → expect
  **`rs/deny.toml` `[licenses] exceptions`** review and possibly a temporary
  **`cargo-machete` `ignored`** allowlist if a dep is introduced a commit before it's consumed
  (prune once consumed). Pin exact versions during implementation, and **verify the resolved dep
  tree does not add a second `hyper`/`hyper-util`** (the http-listener path we disabled) — a
  duplicate heavy tree risks the cedar CI-disk-exhaustion issue.
- **`promtool`** added as a **proto-pinned CLI** (per the cargo-deny/machete proto-plugin pattern)
  for the §9 Prometheus-artifact gate; not a cargo dependency.
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
| **D4** | `/metrics` **unauthenticated**; exposure controlled by **bind address**, not auth — optional `[metrics].addr` separate internal listener, **mandated for a non-internal gateway** | Prometheus convention; exposition has no secrets, but the gateway can run public (spend-cap mode) where same-port `/metrics` would be recon disclosure (challenge finding) |
| **D5** | **Promote** `dropped_denial_audits` + relay `TickReport` to real metrics; **remove** the 60s poll task **and its now-dead accessors**; **no** drop-site warn | A counter bumped at the drop site needs no re-emit; a drop-site warn would log-spam the overflow burst (the removed ticker throttled it) |
| **D6** | Full ops kit: dashboards + scrape config + **alert rules** + **docker-compose** local stack + RUNBOOK | Highest operational value; sets the repo's ops precedent; makes the RUNBOOK runnable |
| **D7** | Local stack scrapes **host-run services** (no service Dockerfiles); `extra_hosts` for Linux, pinned images | Keeps scope to observability tooling, not app containerisation (a separate deployment concern) |
| **D8** | **Bounded-cardinality labels only**; `model`/PRNs/paths never labels; gRPC labels static, not `:path`-derived | Prevents cardinality blow-ups (incl. a gRPC scan-DoS vector) and preserves the gateway privacy bar |
| **D9** | Prometheus artifacts gated by **`promtool` check/test**; a **suffix-normalised name-drift** test (no bespoke PromQL parser) for dashboards+rules ↔ registry | `promtool` is the standard validity tool; a hand-rolled PromQL parser is brittle and would false-fail histograms (challenge finding) |
| **D10** | Implementable in **two slices** (A: lib + `/metrics` + instrumentation; B: dashboards + alerts + compose + RUNBOOK) | A-first ships the exposition surface; B is pure ops artifacts atop it. Plan may land as one PR if size allows |
| **D11** | gRPC metrics recorded **at each tonic handler boundary** (`record_grpc`, static labels, status from the `Result`) — **not** a tower `Server::layer` | grpc-status is a response *trailer* a plain layer can't see, and `:path`-derived method labels are unbounded (challenge finding) |
| **D12** | `iam_outbox_relay_parked_total` is a **counter** (not a gauge); relay liveness via `iam_outbox_relay_ticks_total`; oldest-age `None`→0 | `TickReport.parked`/age are per-tick deltas; a gauge would flap to 0 and an age gauge freezes on a wedged-but-alive relay (challenge findings) |
| **D13** | `init` caches the handle in a `OnceLock` and returns a clone on re-call | `install_recorder()` succeeds once per process; a re-built handle would be disconnected and render empty (challenge finding) |

## 12. Implementation slices (D10)

- **Slice A — instrumentation:** `paigasus-observability` crate (`init` w/ `OnceLock` handle,
  `metrics_router`, `http_metrics_layer`, `record_grpc`, `names`); `[metrics] { enabled, addr }`
  config + validation in both services (incl. the optional separate-listener wiring); IAM metrics
  (HTTP layer + TraceLayer scrape-exclusion, per-handler gRPC `record_grpc`, authz-decision counter
  w/ `bypass`, audit counter, denial drop/enqueue counters + 60s-task & dead-accessor removal,
  relay counters/gauge); gateway metrics (HTTP layer, IAM-call + upstream counters/histograms,
  inflight, TTFB note); all §9 code tests. **PR 1 ("Part of SMA-446").**
- **Slice B — ops artifacts:** `ops/observability/` (compose w/ `extra_hosts` + pinned images,
  Prometheus config + rules, Grafana provisioning + 2 dashboards); the RUNBOOK; the `promtool`
  proto-pin + CI gate; the G6 name-drift test. **PR 2 ("Part of SMA-446", closes the epic).**

  Both slices are designed together (this spec); the plan may land B as one PR if review size
  allows, but A-first is the sequencing (dashboards need emitted metrics to reference).

## 13. Follow-ups (not this cycle)

- **Gateway M5** observability: provider/cost/cache/rate-limit metrics + gateway dashboards
  extension (builds on this crate).
- **OpenTelemetry** OTLP trace/metric export (correlate with the JSON logs).
- **mTLS on the scrape endpoint** (the separate-listener bind, D4, is in-scope; mutual TLS is the
  next hardening step).
- **Full tower-layer gRPC metrics** (trailer-classifying body wrapper) if a non-handler-scoped
  approach is ever wanted; the handler-boundary `record_grpc` (D11) covers v1.
- **Hosted stack + paging** (Alertmanager routing to PagerDuty/Opsgenie), long-term storage.
- **Outbox pruning + full DLQ** (already an audit/outbox-spec §14 follow-up; the RUNBOOK
  documents the interim manual procedure).

## 14. Open items & their resolution

- **`/metrics` exposure (D4):** unauthenticated, but the gateway's public (spend-cap) mode gets an
  **optional separate internal bind** (`[metrics].addr`), RUNBOOK-mandated — not same-port-only.
- **Local stack (D7):** scrapes host services; compose bakes `extra_hosts:
  ["host.docker.internal:host-gateway"]` for Linux and pins image versions.
- **Gateway** gains a middleware layer but **not** `tower-http`/`TraceLayer` (§5.3).
- **`metrics-exporter-prometheus`** used **without** its http-listener feature (render via handle);
  the resolved dep tree is verified for `hyper` duplication before merge (§10).

## 15. Changelog — Stage-2 challenge fold-in

Verdict: **APPROVE WITH CHANGES**. All findings were justified (verified against code) and folded
in — nothing rejected:

- **Relay `parked` gauge would never alert** → `iam_outbox_relay_parked_total` **counter** +
  `increase(...)` alert; added `iam_outbox_relay_ticks_total` liveness + `IamOutboxRelayStalled`;
  oldest-age `None`→0; added `iam_outbox_relay_published_total` (D12, §5.2, §7.2).
- **gRPC layer hand-waved (trailer status + unbounded `:path` labels)** → `record_grpc` at each
  **handler boundary**, static labels, status from `Result`; added `IamGrpcHighErrorRate`; clarified
  `IamHighErrorRate` is HTTP-only (D11, §5.2, §7.2).
- **`init` handle foot-gun** (`install_recorder` once/process) → `OnceLock`-cached handle returned
  by clone (D13, §4.3).
- **Same-port unauth `/metrics` under public gateway** → optional separate `[metrics].addr`,
  RUNBOOK-mandated for non-internal gateway (D4, §6).
- **Drift test reinvents PromQL / false-fails histograms / unbudgeted dep** → `promtool`
  check/test gate + a suffix-normalised, built-in-whitelisted name-drift test; test-only YAML dep
  budgeted (D9, §9, §10).
- **Removing the 60s ticker** → no drop-site warn (avoid burst log-spam); remove now-dead
  `denial_buffer()`/`dropped()` accessors to keep `warnings = "deny"` green (D5, §5.2).
- **`metrics` must be a direct dep of both services**; **verify no duplicate `hyper`** (§4.1, §10).
- Corrections/caveats: gateway IAM-call metrics live in **`auth.rs`**, not `chat` (§5.3);
  streaming latency = **TTFB**, mid-stream SSE errors uncounted (§5.3); `cache="bypass"` for the
  Redis-outage fail-open (§4.2, §5.2); `iam_audit_records_total` counts non-erroring inserts, not
  committed rows (§5.2); exclude `/metrics`+health from IAM's `TraceLayer` (§5.1); `extra_hosts`
  for Linux + pinned images (§7).
