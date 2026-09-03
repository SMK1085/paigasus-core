# SMA-446 #3 — Observability (metrics, dashboards, RUNBOOK) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the merged SMA-446 backbone (IAM audit/outbox/relay + gateway M0 auth/proxy) operable — Prometheus metrics on both services, Grafana dashboards, alert rules, a docker-compose local stack, and a RUNBOOK.

**Architecture:** A new `paigasus-observability` lib installs a global `metrics`-facade recorder (mirroring `paigasus-logging`), exposes `GET /metrics`, and provides a shared axum request-metrics layer + a `record_grpc` helper + a `const` metric-name registry. Both services instrument their hot paths through the global facade (no `AppState` plumbing). Ops artifacts under `ops/observability/` are gated by `promtool` + a name-drift test.

**Tech Stack:** Rust (edition 2024, rustc 1.95), `metrics` + `metrics-exporter-prometheus` (default-features off), axum 0.8, tonic, Moon, figment config; Prometheus + Grafana (docker-compose); `promtool` (proto-pinned CLI).

**Spec:** `docs/superpowers/specs/2026-07-13-sma-446-observability-metrics-dashboards-design.md` (rev 2). Read it before starting.

## Global Constraints

- **SPDX header** on every source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for TOML/YAML where a comment is conventional; not required for JSON).
- **Rust edition 2024, rust-version 1.95** in every new crate `Cargo.toml`.
- **Moon project id** = crate name + `-rs` suffix; `layer:` field (not `type:`); `paigasus-observability-rs`, `layer: library`.
- **Workspace `warnings = "deny"`** — no dead code / unused warnings may remain.
- **Bounded-cardinality labels only** — never label with `model`, any PRN, key id, principal, raw path, or free-form error string. Allowed: `route` (MatchedPath), `method`, `status_class` (`2xx`/`4xx`/`5xx`), `grpc_status`, `decision` (`allow`/`deny`), `cache` (`hit`/`miss`/`bypass`), `outcome`, `operation` (`introspect`/`authorize`), `result` (`ok`/`denied`/`unavailable`/`error`). gRPC `service`/`method` are **static string literals**, never `:path`-derived.
- **Metric naming:** snake_case, service prefix, `_total` counters, base-unit `_seconds` histograms. Every metric name is a `const` in `paigasus_observability::names`.
- **Commits:** Conventional Commits with a scope from the allowlist (`rs`, `ci`, `repo`, `docs`, …); subject lowercase, ≤100 chars; body lines ≤100 chars; blank line before the footer; **no `#NNN`** in the body (write "SMK1085/paigasus-core PR NNN"); end every commit with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **PATH for tooling:** prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims first) so moon/cargo/nextest/buf resolve to the repo-pinned versions.
- **Do NOT bypass git hooks** with `--no-verify`.
- **Full CI gate list** (run from repo root before each PR):
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`
- **Two slices / two PRs:** Slice A = Tasks A1–A11 (instrumentation), Slice B = Tasks B1–B6 (ops artifacts). Each PR title/body says "Part of SMA-446"; Slice B's PR closes the epic.

---

## File Structure

**New crate — `rs/crates/libs/paigasus-observability/`:**
- `Cargo.toml` — crate manifest (deps: `metrics`, `metrics-exporter-prometheus` default-features off, `axum`, `tower`, `tonic` types; dev: `tower`/`http` test helpers).
- `moon.yml` — `paigasus-observability-rs`, `layer: library`.
- `src/lib.rs` — module wiring + `init` + `metrics_router` + the `OnceLock` handle.
- `src/http.rs` — `http_metrics_layer` (axum middleware).
- `src/grpc.rs` — `record_grpc` helper.
- `src/names.rs` — the `const` metric-name registry + PromQL/builtin whitelist consts.
- `tests/` inline `#[cfg(test)]` in each module; a `tests/drift.rs` integration test lands in Slice B (Task B4).

**Modified — `paigasus-gateway`:** `Cargo.toml` (+`paigasus-observability`, `metrics`), `src/config.rs` (`[metrics]`), `src/main.rs` (init + `/metrics` wiring), `src/adapters/http/mod.rs` (`http_metrics_layer`), `src/adapters/http/auth.rs` (IAM-call metrics), `src/adapters/http/chat.rs` (upstream metrics), `gateway.toml.example`.

**Modified — `paigasus-iam`:** `Cargo.toml`, `src/config.rs` (`[metrics]`), `src/main.rs` (init + `/metrics` wiring + remove the 60s ticker), `src/adapters/http/mod.rs` (`http_metrics_layer` + TraceLayer scrape-exclusion), `src/adapters/grpc/*` (per-handler `record_grpc`), `src/adapters/authz/cedar_authorizer.rs` (authz-decision counter), `src/adapters/authz/denial_audit.rs` (drop/enqueue counters), `src/adapters/persistence/pg_audit_log.rs` (audit counter), `src/adapters/events/relay.rs` (relay counters/gauge), `iam.toml.example`.

**Modified — workspace:** `rs/Cargo.toml` (workspace deps + members), `rs/deny.toml` (license exceptions if needed), CODEOWNERS is Moon-generated (don't hand-edit).

**New — ops (Slice B):** `ops/observability/{docker-compose.yml, README.md, prometheus/prometheus.yml, prometheus/rules/{iam,gateway}.rules.yml, prometheus/rules/tests/{iam,gateway}.test.yml, grafana/provisioning/datasources/prometheus.yml, grafana/provisioning/dashboards/dashboards.yml, grafana/dashboards/{iam,gateway}.json}`; `docs/ops/RUNBOOK-observability.md`; `.proto/plugins/*` + `.prototools` entry for `promtool`; a CI job step for `promtool` + drift test.

---

# SLICE A — Instrumentation (PR 1)

## Task A1: Scaffold the `paigasus-observability` crate

**Files:**
- Create: `rs/crates/libs/paigasus-observability/Cargo.toml`
- Create: `rs/crates/libs/paigasus-observability/moon.yml`
- Create: `rs/crates/libs/paigasus-observability/src/lib.rs`
- Modify: `rs/Cargo.toml` (workspace `members` + `[workspace.dependencies]`)

**Interfaces:**
- Produces: the crate `paigasus_observability` (empty for now) buildable in the workspace; workspace deps `metrics`, `metrics-exporter-prometheus`.

- [ ] **Step 1: Add workspace deps + member.** In `rs/Cargo.toml`, under `[workspace.dependencies]` add (pin to the latest compatible versions — check crates.io at implementation time; these are the expected majors):

```toml
metrics = "0.24"
metrics-exporter-prometheus = { version = "0.16", default-features = false }
```

Add `"crates/libs/paigasus-observability"` to `[workspace] members` (keep the list sorted if it is).

- [ ] **Step 2: Write the crate manifest.** `rs/crates/libs/paigasus-observability/Cargo.toml`:

```toml
[package]
name = "paigasus-observability"
version = "0.0.0"
edition = "2024"
rust-version = "1.95"
license = "Apache-2.0"
publish = false

[dependencies]
metrics = { workspace = true }
metrics-exporter-prometheus = { workspace = true }
axum = { workspace = true }
tower = { workspace = true }
tonic = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
http = { workspace = true }
tower = { workspace = true }
```

(If any of `http` is not already a workspace dep, use the version axum re-exports; verify at build time.)

- [ ] **Step 3: Write `moon.yml`.**

```yaml
id: paigasus-observability-rs
layer: library
language: rust
```

- [ ] **Step 4: Write a placeholder `src/lib.rs`** (real API arrives in A2–A5):

```rust
// SPDX-License-Identifier: Apache-2.0

//! Shared observability plumbing for Paigasus services: a global `metrics`-facade Prometheus
//! recorder, a `GET /metrics` router, an axum request-metrics layer, a gRPC handler helper, and
//! the canonical metric-name registry. Mirrors `paigasus-logging`'s role for tracing.
```

- [ ] **Step 5: Build.** Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo build -p paigasus-observability`
Expected: compiles clean.

- [ ] **Step 6: Commit.**

```bash
git add rs/Cargo.toml rs/crates/libs/paigasus-observability
git commit -m "feat(rs): scaffold paigasus-observability crate (SMA-446)"
```

---

## Task A2: `init` + `metrics_router` + OnceLock handle

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn init(service: &str) -> metrics_exporter_prometheus::PrometheusHandle` — installs the global recorder once (idempotent via a `OnceLock`), returns a cloned handle.
  - `pub fn metrics_router(handle: PrometheusHandle) -> axum::Router` — `GET /metrics` → exposition.

- [ ] **Step 1: Write failing tests** in `src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use metrics::counter;

    #[test]
    fn init_installs_recorder_and_second_call_returns_working_handle() {
        let h1 = init("test-svc");
        counter!("obs_test_init_counter").increment(1);
        assert!(h1.render().contains("obs_test_init_counter"), "first handle renders the metric");
        // Second call must NOT return a disconnected, empty handle (the install_recorder foot-gun).
        let h2 = init("test-svc");
        assert!(h2.render().contains("obs_test_init_counter"), "second handle still reflects the global recorder");
    }
}
```

- [ ] **Step 2: Run — verify fails.** `cd rs && cargo test -p paigasus-observability init_installs -- --nocapture`
Expected: FAIL (`init` not found).

- [ ] **Step 3: Implement `init` + `metrics_router`** in `src/lib.rs` (append after the module docs):

```rust
use std::sync::OnceLock;

use axum::{Router, http::header, response::IntoResponse, routing::get};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Default latency histogram buckets (seconds) for every `*_seconds` family.
const LATENCY_BUCKETS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder for `service` (once per process) and return the render
/// handle. A second in-process call returns a clone of the cached first handle — never a freshly
/// built, disconnected one (`install_recorder` succeeds at most once per process). `service` is
/// used only for the startup log line; the Prometheus scrape `job` label identifies the service.
pub fn init(service: &str) -> PrometheusHandle {
    HANDLE
        .get_or_init(|| {
            let handle = PrometheusBuilder::new()
                .set_buckets(LATENCY_BUCKETS)
                .expect("static non-empty buckets")
                .install_recorder()
                .expect("global metrics recorder installs once");
            tracing::info!(service, "metrics recorder installed");
            handle
        })
        .clone()
}

/// An axum router serving `GET /metrics` as Prometheus text exposition.
pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let body = handle.render();
            std::future::ready(([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response())
        }),
    )
}
```

- [ ] **Step 4: Add a router test** to the test module:

```rust
    #[tokio::test]
    async fn metrics_router_returns_exposition() {
        use tower::ServiceExt;
        let handle = init("test-svc");
        counter!("obs_test_router_counter").increment(1);
        let app = metrics_router(handle);
        let resp = app.oneshot(http::Request::builder().uri("/metrics").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()[http::header::CONTENT_TYPE], "text/plain; version=0.0.4");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("obs_test_router_counter"));
    }
```

(`tokio` is needed as a dev-dependency — add `tokio = { workspace = true, features = ["macros", "rt"] }` under `[dev-dependencies]`.)

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-observability`
Expected: PASS. (Note: `set_buckets` API name may differ by version — if the pinned `metrics-exporter-prometheus` uses `set_buckets_for_metric`/a `Matcher`, adjust; the test proves rendering works regardless.)

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/libs/paigasus-observability
git commit -m "feat(rs): add observability recorder init + /metrics router (SMA-446)"
```

---

## Task A3: `http_metrics_layer` (axum request-metrics middleware)

**Files:**
- Create: `rs/crates/libs/paigasus-observability/src/http.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/lib.rs` (`pub mod http;` + re-export)

**Interfaces:**
- Produces: `pub fn http_metrics_layer(prefix: &'static str) -> axum::middleware::FromFnLayer<...>` — records `<prefix>_http_requests_total{route,method,status_class}`, `<prefix>_http_request_duration_seconds{route,method}`, `<prefix>_http_inflight_requests`.

- [ ] **Step 1: Write the failing test** in `src/http.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Shared axum request-metrics middleware.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{init, metrics_router};
    use axum::{Router, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn records_request_with_matched_route_and_status_class() {
        let handle = init("http-test");
        let app: Router = Router::new()
            .route("/v1/thing/{id}", get(|| async { "ok" }))
            .layer(http_metrics_layer("gwtest"))
            .merge(metrics_router(handle.clone()));
        // Drive a request through the templated route.
        let _ = app.clone().oneshot(http::Request::builder().uri("/v1/thing/42").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let out = handle.render();
        assert!(out.contains("gwtest_http_requests_total"), "counter emitted:\n{out}");
        assert!(out.contains("route=\"/v1/thing/{id}\""), "uses the MatchedPath template, not /v1/thing/42");
        assert!(out.contains("status_class=\"2xx\""));
        assert!(out.contains("gwtest_http_request_duration_seconds"));
    }
}
```

- [ ] **Step 2: Run — verify fails.** `cd rs && cargo test -p paigasus-observability records_request_with -- --nocapture`
Expected: FAIL (`http_metrics_layer` undefined).

- [ ] **Step 3: Implement** at the top of `src/http.rs` (above the test module):

```rust
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use metrics::{counter, gauge, histogram};

/// axum middleware recording request count (by route/method/status_class), duration, and an
/// in-flight gauge. `route` is the MatchedPath template (bounded); an unmatched request collapses
/// to `<unmatched>`. `prefix` is the service metric prefix (e.g. `"gateway"`, `"iam"`).
pub fn http_metrics_layer(prefix: &'static str) -> middleware::FromFnLayer<
    impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> + Clone,
    (),
    Request,
> {
    middleware::from_fn(move |req: Request, next: Next| {
        Box::pin(async move {
            let route = req
                .extensions()
                .get::<MatchedPath>()
                .map(|m| m.as_str().to_owned())
                .unwrap_or_else(|| "<unmatched>".to_owned());
            let method = req.method().as_str().to_owned();
            let inflight = gauge!(format!("{prefix}_http_inflight_requests"));
            inflight.increment(1.0);
            let started = Instant::now();
            let resp = next.run(req).await;
            inflight.decrement(1.0);
            let elapsed = started.elapsed().as_secs_f64();
            let status_class = format!("{}xx", resp.status().as_u16() / 100);
            counter!(format!("{prefix}_http_requests_total"),
                "route" => route.clone(), "method" => method.clone(), "status_class" => status_class).increment(1);
            histogram!(format!("{prefix}_http_request_duration_seconds"),
                "route" => route, "method" => method).record(elapsed);
            resp
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
    })
}
```

> Implementation note: the exact `from_fn` return-type signature is finicky. If the explicit type is unwieldy, define the closure as an `async fn middleware_fn(...)` and return `middleware::from_fn(middleware_fn_with_prefix)` via a small wrapper, OR expose the middleware as an `apply_http_metrics(router, prefix)` helper that calls `.layer(from_fn(...))` internally and returns the `Router`. Prefer whichever compiles cleanly; the **test in Step 1 is the contract**, not the signature.

- [ ] **Step 4: Wire the module** in `src/lib.rs`: add `pub mod http;` and `pub use http::http_metrics_layer;`.

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-observability`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/libs/paigasus-observability
git commit -m "feat(rs): add shared http request-metrics layer (SMA-446)"
```

---

## Task A4: `record_grpc` helper

**Files:**
- Create: `rs/crates/libs/paigasus-observability/src/grpc.rs`
- Modify: `src/lib.rs` (`pub mod grpc;` + re-export)

**Interfaces:**
- Produces: `pub fn record_grpc<T>(service: &'static str, method: &'static str, started: std::time::Instant, result: &Result<T, tonic::Status>)` — records `iam_grpc_requests_total{service,method,grpc_status}` + `iam_grpc_request_duration_seconds{service,method}`.

- [ ] **Step 1: Write the failing test** in `src/grpc.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! One-line gRPC handler-boundary instrumentation (static labels; status from the Result).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;
    use std::time::Instant;

    #[test]
    fn records_static_labels_and_status_from_result() {
        let handle = init("grpc-test");
        let ok: Result<(), tonic::Status> = Ok(());
        record_grpc("Authorization", "IsAuthorized", Instant::now(), &ok);
        let err: Result<(), tonic::Status> = Err(tonic::Status::permission_denied("no"));
        record_grpc("Authorization", "IsAuthorized", Instant::now(), &err);
        let out = handle.render();
        assert!(out.contains("iam_grpc_requests_total"));
        assert!(out.contains("service=\"Authorization\""));
        assert!(out.contains("method=\"IsAuthorized\""));
        assert!(out.contains("grpc_status=\"ok\""));
        assert!(out.contains("grpc_status=\"permission_denied\""));
    }
}
```

- [ ] **Step 2: Run — verify fails.** `cd rs && cargo test -p paigasus-observability records_static_labels -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Implement** above the test module in `src/grpc.rs`:

```rust
use std::time::Instant;

use metrics::{counter, histogram};

/// Record a completed tonic handler call. `service`/`method` are compile-time literals (never
/// `:path`-derived — bounded cardinality); `grpc_status` is `"ok"` or the canonical code name.
pub fn record_grpc<T>(service: &'static str, method: &'static str, started: Instant, result: &Result<T, tonic::Status>) {
    let grpc_status = match result {
        Ok(_) => "ok",
        Err(status) => grpc_code_name(status.code()),
    };
    counter!("iam_grpc_requests_total", "service" => service, "method" => method, "grpc_status" => grpc_status).increment(1);
    histogram!("iam_grpc_request_duration_seconds", "service" => service, "method" => method).record(started.elapsed().as_secs_f64());
}

fn grpc_code_name(code: tonic::Code) -> &'static str {
    use tonic::Code::*;
    match code {
        Ok => "ok",
        Cancelled => "cancelled",
        Unknown => "unknown",
        InvalidArgument => "invalid_argument",
        DeadlineExceeded => "deadline_exceeded",
        NotFound => "not_found",
        AlreadyExists => "already_exists",
        PermissionDenied => "permission_denied",
        ResourceExhausted => "resource_exhausted",
        FailedPrecondition => "failed_precondition",
        Aborted => "aborted",
        OutOfRange => "out_of_range",
        Unimplemented => "unimplemented",
        Internal => "internal",
        Unavailable => "unavailable",
        DataLoss => "data_loss",
        Unauthenticated => "unauthenticated",
    }
}
```

- [ ] **Step 4: Wire** in `src/lib.rs`: `pub mod grpc;` + `pub use grpc::record_grpc;`.

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-observability`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/libs/paigasus-observability
git commit -m "feat(rs): add record_grpc handler-boundary helper (SMA-446)"
```

---

## Task A5: `names` registry + PromQL/builtin whitelist

**Files:**
- Create: `rs/crates/libs/paigasus-observability/src/names.rs`
- Modify: `src/lib.rs` (`pub mod names;`)

**Interfaces:**
- Produces: `paigasus_observability::names::*` — a `const &str` per metric family, an `ALL: &[&str]` slice, `PROM_BUILTINS: &[&str]`, `PROMQL_TOKENS: &[&str]` (for the Task B4 drift test).

- [ ] **Step 1: Write the failing test** in `src/names.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Canonical metric-name registry — the single source of truth for instrumentation AND the
//! dashboard/alert name-drift test (Task B4).

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
```

- [ ] **Step 2: Run — verify fails.** `cd rs && cargo test -p paigasus-observability all_names_are_unique -- --nocapture`
Expected: FAIL (`ALL` undefined).

- [ ] **Step 3: Implement** the registry above the tests. Include every metric family from spec §5 (HTTP families are recorded per-prefix, so register both prefixes' concrete names):

```rust
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

/// Every metric family this workspace emits — the drift test asserts dashboards/alerts reference
/// only these (after suffix-normalisation) plus the built-ins/tokens below.
pub const ALL: &[&str] = &[
    GATEWAY_HTTP_REQUESTS_TOTAL, GATEWAY_HTTP_REQUEST_DURATION_SECONDS, GATEWAY_HTTP_INFLIGHT_REQUESTS,
    GATEWAY_IAM_CALLS_TOTAL, GATEWAY_IAM_CALL_DURATION_SECONDS,
    GATEWAY_UPSTREAM_REQUESTS_TOTAL, GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS,
    IAM_HTTP_REQUESTS_TOTAL, IAM_HTTP_REQUEST_DURATION_SECONDS, IAM_HTTP_INFLIGHT_REQUESTS,
    IAM_GRPC_REQUESTS_TOTAL, IAM_GRPC_REQUEST_DURATION_SECONDS,
    IAM_AUTHZ_DECISIONS_TOTAL, IAM_AUDIT_RECORDS_TOTAL,
    IAM_DENIAL_AUDITS_DROPPED_TOTAL, IAM_DENIAL_AUDITS_ENQUEUED_TOTAL,
    IAM_OUTBOX_RELAY_TICKS_TOTAL, IAM_OUTBOX_RELAY_DRAINED_TOTAL, IAM_OUTBOX_RELAY_PUBLISHED_TOTAL,
    IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL, IAM_OUTBOX_RELAY_PARKED_TOTAL,
    IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS,
];

/// Prometheus built-in series that dashboards/alerts may reference without them being in `ALL`.
pub const PROM_BUILTINS: &[&str] = &["up", "scrape_duration_seconds", "scrape_samples_scraped"];

/// PromQL function/keyword tokens the drift test must not mistake for metric names.
pub const PROMQL_TOKENS: &[&str] = &[
    "rate", "irate", "increase", "sum", "avg", "min", "max", "count", "by", "without", "on",
    "group_left", "group_right", "histogram_quantile", "clamp_min", "clamp_max", "vector",
    "le", "job", "instance", "and", "or", "unless", "offset", "bool", "delta", "idelta",
];
```

- [ ] **Step 4: Wire** in `src/lib.rs`: `pub mod names;`.

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-observability`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/libs/paigasus-observability
git commit -m "feat(rs): add canonical metric-name registry (SMA-446)"
```

---

## Task A6: Gateway `[metrics]` config + `/metrics` wiring + HTTP layer

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml`, `src/config.rs`, `src/main.rs`, `src/adapters/http/mod.rs`, `gateway.toml.example`

**Interfaces:**
- Consumes: `paigasus_observability::{init, metrics_router, http_metrics_layer}` (A2, A3).
- Produces: `GatewayConfig.metrics: MetricsConfig { enabled: bool, addr: Option<SocketAddr> }` with `validate()` rejecting `addr == http_addr`; `/metrics` served (merged or on a separate listener); the gateway router carries `http_metrics_layer("gateway")`.

- [ ] **Step 1: Add deps.** In `paigasus-gateway/Cargo.toml` `[dependencies]`: `paigasus-observability = { workspace = true }` and `metrics = { workspace = true }`. Add `paigasus-observability = { path = "crates/libs/paigasus-observability" }` to `rs/Cargo.toml` `[workspace.dependencies]` if not present. Add `paigasus-observability-rs` to the gateway `moon.yml` `dependsOn`.

- [ ] **Step 2: Write failing config test** in `src/config.rs` `#[cfg(test)]`:

```rust
#[test]
fn metrics_addr_must_differ_from_http_addr() {
    let mut cfg = GatewayConfig::default_for_test(); // or the existing test constructor
    cfg.metrics.enabled = true;
    cfg.metrics.addr = Some(cfg.http_addr); // collision
    assert!(cfg.validate().is_err(), "metrics.addr == http_addr is a config error");
}
```

(Use whatever test-config constructor the file already provides; if none, build via `GatewayConfig::load()` against a temp figment or mirror the existing config tests' pattern.)

- [ ] **Step 3: Run — verify fails.** `cd rs && cargo test -p paigasus-gateway metrics_addr_must_differ -- --nocapture`
Expected: FAIL (`metrics` field missing).

- [ ] **Step 4: Add the config struct.** In `src/config.rs`:
  - Add `#[derive(...)] pub struct MetricsConfig { pub enabled: bool, pub addr: Option<SocketAddr> }` matching the file's serde/derive conventions; default `enabled = true`, `addr = None`.
  - Add `pub metrics: MetricsConfig` to `GatewayConfig`, with the same figment default wiring the other sub-configs use.
  - In `GatewayConfig::validate()`, add: `if let Some(a) = self.metrics.addr { if a == self.http_addr { return Err("metrics.addr must not equal http_addr".into()); } }` (match the existing error type).

- [ ] **Step 5: Run — verify passes.** `cd rs && cargo test -p paigasus-gateway metrics_addr_must_differ`
Expected: PASS.

- [ ] **Step 6: Wire the HTTP layer + `/metrics` into the router.** In `src/adapters/http/mod.rs`, change `router` to take an optional merged metrics router and apply the layer. Simplest: add a parameter-free helper the main uses:
  - Add `use paigasus_observability::http_metrics_layer;`.
  - Apply the layer to the whole tree: in `router`, before `.with_state(state)`, add `.layer(http_metrics_layer("gateway"))`. (Health + `/metrics` get a `route` label but that's acceptable — or exclude by mounting `/metrics` from `main` on the merged router built AFTER the layer; see Step 7.)

- [ ] **Step 7: Wire init + `/metrics` in `main.rs`:**

```rust
// after paigasus_logging::init(...)
let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-gateway"));
// ... build state, then:
let app = router(state);
let app = match (&metrics_handle, config.metrics.addr) {
    (Some(h), None) => app.merge(paigasus_observability::metrics_router(h.clone())), // same-port
    _ => app, // disabled, or separate listener (spawned below)
};
// separate metrics listener:
if let (Some(h), Some(maddr)) = (metrics_handle.clone(), config.metrics.addr) {
    let mrouter = paigasus_observability::metrics_router(h);
    let mlistener = tokio::net::TcpListener::bind(maddr).await?;
    tokio::spawn(async move { let _ = axum::serve(mlistener, mrouter).await; });
}
```

(Adapt to the existing graceful-shutdown pattern; the separate listener may share `shutdown_signal()` via a broadcast if desired, but a plain spawn is acceptable for M0 — note it in the RUNBOOK.)

- [ ] **Step 8: Update `gateway.toml.example`** — add:

```toml
[metrics]
enabled = true
# addr = "127.0.0.1:9091"   # optional separate internal listener; RECOMMENDED for a public gateway
```

- [ ] **Step 9: Write an integration test** `tests/metrics.rs` (or extend an existing one): build the router with a fake IAM, hit `/metrics`, assert `200`; drive a `/healthz` request then assert `gateway_http_requests_total` appears. Use the existing test fakes (`UnusedIam`/`ProbeIam` patterns from `http/mod.rs`).

- [ ] **Step 10: Run tests + build.** `cd rs && cargo test -p paigasus-gateway && cargo build -p paigasus-gateway`
Expected: PASS.

- [ ] **Step 11: Commit.**

```bash
git add rs/crates/services/paigasus-gateway rs/Cargo.toml
git commit -m "feat(rs): expose gateway /metrics + http request metrics (SMA-446)"
```

---

## Task A7: Gateway IAM-call + upstream instrumentation

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs`, `src/adapters/http/chat.rs`

**Interfaces:**
- Consumes: `metrics::{counter, histogram}`, `paigasus_observability::names`.
- Produces: `gateway_iam_calls_total{operation,result}` + `_duration`; `gateway_upstream_requests_total{status_class}` + `_duration`.

- [ ] **Step 1: Write a failing integration test** in the gateway `tests/` (extend `chat_proxy.rs` or a new `tests/metrics_proxy.rs`): with the existing IAM+OpenAI fakes, drive a successful proxied request and assert the rendered `/metrics` contains `gateway_iam_calls_total` with `operation="introspect"` and `operation="authorize"`, and `gateway_upstream_requests_total`.

- [ ] **Step 2: Run — verify fails.** Expected: FAIL (metrics absent).

- [ ] **Step 3: Instrument `auth.rs`** in `require_iam_auth`, around each IAM call. Wrap `introspect_api_key` (line ~57) and `is_authorized_self` (line ~84):

```rust
use std::time::Instant;
use metrics::{counter, histogram};

// introspect:
let started = Instant::now();
let resp = match iam.introspect_api_key(&key).await {
    Ok(resp) => { record_iam_call("introspect", "ok", started); resp }
    Err(err) => { record_iam_call("introspect", iam_result(&err), started); return introspect_error(err).into_response(); }
};
```

Add a private helper in `auth.rs`:

```rust
fn record_iam_call(operation: &'static str, result: &'static str, started: Instant) {
    counter!("gateway_iam_calls_total", "operation" => operation, "result" => result).increment(1);
    histogram!("gateway_iam_call_duration_seconds", "operation" => operation).record(started.elapsed().as_secs_f64());
}

/// Map an IamError to a bounded result label (never leaks status text).
fn iam_result(err: &IamError) -> &'static str {
    match err {
        IamError::Connect(_) => "unavailable",
        IamError::Rpc(s) if matches!(s.code(), tonic::Code::Unavailable | tonic::Code::DeadlineExceeded) => "unavailable",
        IamError::Rpc(s) if s.code() == tonic::Code::Unauthenticated => "denied",
        IamError::Rpc(_) => "error",
    }
}
```

For the `authorize` call: `record_iam_call("authorize", ...)` — `Ok(true)` → `"ok"`, `Ok(false)` → `"denied"`, `Err(e)` → `iam_result(&e)`.

- [ ] **Step 4: Instrument `chat.rs`** around `OpenAiClient::chat_completion` (line ~78). Record `gateway_upstream_requests_total{status_class}` + `gateway_upstream_request_duration_seconds` using the response status (TTFB boundary — for SSE this measures time-to-headers; add a `// NOTE: TTFB for streams` comment). On upstream error, map connect/transport→`5xx` class label appropriately (or an `error` bucket — keep it a `status_class` derived from the HTTP status when there is one, else count under the mapped 502/504).

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-gateway`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): instrument gateway iam + upstream calls (SMA-446)"
```

---

## Task A8: IAM `[metrics]` config + `/metrics` wiring + HTTP layer + TraceLayer exclusion

**Files:**
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml`, `moon.yml`, `src/config.rs`, `src/main.rs`, `src/adapters/http/mod.rs`, `iam.toml.example`

**Interfaces:**
- Mirrors A6 for IAM. Produces `IamConfig.metrics: MetricsConfig`, `/metrics` on the IAM HTTP port (or separate listener), `http_metrics_layer("iam")`, and `/metrics`+health excluded from the existing `TraceLayer`.

- [ ] **Step 1: Add deps** (`paigasus-observability`, `metrics`) to `paigasus-iam/Cargo.toml` + `paigasus-observability-rs` to `moon.yml` `dependsOn`.

- [ ] **Step 2: Add `MetricsConfig`** to `src/config.rs` exactly as A6 Step 4 (reuse the same field shape; IAM's `validate()` compares `metrics.addr` against `http_addr`). Write the same `metrics_addr_must_differ_from_http_addr` failing test first, run it red, implement, run green.

- [ ] **Step 3: Exclude `/metrics` + health from the TraceLayer.** In `src/adapters/http/mod.rs` `router()`/`serve_http`, restructure so the `TraceLayer` wraps only the app routes, and `/metrics` (+ `/healthz`/`/readyz`) are merged **outside** the traced subtree. Pattern:

```rust
let traced = app_routes.layer(TraceLayer::new_for_http()).layer(TimeoutLayer::new(...));
let router = Router::new().merge(health_routes).merge(traced);
// /metrics merged in main (or here) is outside `traced`.
```

Apply `http_metrics_layer("iam")` to the `traced` app routes (so scrapes/health are excluded from RED too).

- [ ] **Step 4: Wire init + `/metrics` in `src/main.rs`** exactly as A6 Step 7 (`paigasus_observability::init("paigasus-iam")`, merged or separate listener on the same shutdown-watch the other IAM tasks use).

- [ ] **Step 5: Update `iam.toml.example`** with the same `[metrics]` block as A6 Step 8.

- [ ] **Step 6: Integration test** (extend `tests/support`-based tests): hit `/metrics` → `200`; `[metrics] enabled=false` → `/metrics` is `404`.

- [ ] **Step 7: Run + build.** `cd rs && cargo test -p paigasus-iam && cargo build -p paigasus-iam`
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git add rs/crates/services/paigasus-iam rs/Cargo.toml
git commit -m "feat(rs): expose iam /metrics + http request metrics (SMA-446)"
```

---

## Task A9: IAM authz-decision + audit-record counters

**Files:**
- Modify: `src/adapters/authz/cedar_authorizer.rs`, `src/adapters/persistence/pg_audit_log.rs`

**Interfaces:**
- Produces: `iam_authz_decisions_total{decision,cache}`; `iam_audit_records_total{outcome,result}`.

- [ ] **Step 1: Failing unit test** (in `cedar_authorizer.rs` tests, using the module's existing in-memory fixtures + `paigasus_observability::init` + a render assertion): a compute-path decision emits `iam_authz_decisions_total` with `cache="miss"`; a cache-hit deny emits `cache="hit"`. (Assert on presence + label, not absolute counts — §4.3.)

- [ ] **Step 2: Run — verify fails.**

- [ ] **Step 3: Instrument `is_authorized`.** At each decision-return site record the counter:
  - After a compute (cache miss) decision: `counter!("iam_authz_decisions_total", "decision" => effect_label(&decision), "cache" => "miss").increment(1);`
  - In the cache-hit branch (`cedar_authorizer.rs:159-168`): `"cache" => "hit"`.
  - When `cache_key` returned `None` (generations-read failure, fail-open bypass, `:129-137`): `"cache" => "bypass"`.
  - `fn effect_label(d: &Decision) -> &'static str` → `"allow"`/`"deny"`.

- [ ] **Step 4: Instrument `PgAuditLog`.** In `record` and `record_out_of_band`, on success bump `counter!("iam_audit_records_total", "outcome" => outcome_label, "result" => "ok").increment(1);`, on the error path `"result" => "error"`. `outcome_label` from the `AuditEntry.outcome` (`committed`/`denied`). Add a `// counts non-erroring inserts, not committed rows (spec §5.2)` comment.

- [ ] **Step 5: Run tests — verify pass.** `cd rs && cargo test -p paigasus-iam cedar_authorizer`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): instrument iam authz decisions + audit records (SMA-446)"
```

---

## Task A10: IAM denial-drop counters + remove the 60s ticker

**Files:**
- Modify: `src/adapters/authz/denial_audit.rs`, `src/main.rs` (remove ticker + dead accessors), `src/adapters/http/mod.rs` (remove `denial_buffer()` if now unused)

**Interfaces:**
- Produces: `iam_denial_audits_dropped_total`, `iam_denial_audits_enqueued_total`.

- [ ] **Step 1: Failing unit test** in `denial_audit.rs`: after `init`, push `capacity+2` entries into a small-capacity `DenialAuditBuffer`; assert the rendered exposition contains `iam_denial_audits_dropped_total` with value ≥ 1 and `iam_denial_audits_enqueued_total`.

- [ ] **Step 2: Run — verify fails.**

- [ ] **Step 3: Instrument `DenialAuditBuffer::push`** (`denial_audit.rs:70-80`): after `self.dropped.fetch_add(...)` add `counter!("iam_denial_audits_dropped_total").increment(1);`; unconditionally on each push `counter!("iam_denial_audits_enqueued_total").increment(1);`. Add `use metrics::counter;`.

- [ ] **Step 4: Remove the 60s ticker task** from `main.rs:121-151` (the whole `{ ... }` block with the `interval(Duration::from_secs(60))`). Do **not** add a drop-site warn.

- [ ] **Step 5: Remove now-dead accessors.** If `AppState::denial_buffer()` (`http/mod.rs`) and `DenialAuditBuffer::dropped()` are unused after removing the ticker (check with `cargo build` under `warnings = "deny"` — an unused-method is a warning→error only if `pub(crate)`/private; `pub` methods don't warn but keep the surface clean), remove them if they are not part of the public API or tests. Run `cargo build -p paigasus-iam` and fix any `dead_code` errors.

- [ ] **Step 6: Run tests + build — verify pass, no warnings.** `cd rs && cargo test -p paigasus-iam denial && cargo build -p paigasus-iam`
Expected: PASS, clean build.

- [ ] **Step 7: Commit.**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): promote denial-audit drop counter, drop 60s ticker (SMA-446)"
```

---

## Task A11: IAM relay counters/gauge + per-handler gRPC metrics

**Files:**
- Modify: `src/adapters/events/relay.rs`, `src/adapters/grpc/*.rs` (each tonic handler)

**Interfaces:**
- Produces: `iam_outbox_relay_ticks_total{result}`, `_drained_total`, `_published_total`, `_publish_failures_total`, `_parked_total`, `iam_outbox_oldest_unpublished_age_seconds`; plus `iam_grpc_*` at each handler.

- [ ] **Step 1: Failing test — relay.** In `relay.rs` tests (they already drive deterministic `tick()`s): after a tick with a non-empty batch, the rendered exposition contains `iam_outbox_relay_ticks_total` and `iam_outbox_oldest_unpublished_age_seconds`.

- [ ] **Step 2: Run — verify fails.**

- [ ] **Step 3: Instrument the relay tick.** In `tick()` after computing `report` and before/after the existing `tracing::info!` (relay.rs:149), emit:

```rust
use metrics::{counter, gauge};
counter!("iam_outbox_relay_drained_total").increment(report.drained);
counter!("iam_outbox_relay_publish_failures_total").increment(report.failures);
counter!("iam_outbox_relay_published_total").increment(report.drained.saturating_sub(report.failures));
counter!("iam_outbox_relay_parked_total").increment(report.parked);
gauge!("iam_outbox_oldest_unpublished_age_seconds").set(report.oldest_unpublished_age_secs.unwrap_or(0) as f64);
```

In `run()` (the loop), on each successful `tick()` emit `counter!("iam_outbox_relay_ticks_total", "result" => "ok").increment(1);` and on the tick-error branch (relay.rs:175) `"result" => "error"`.

- [ ] **Step 4: Run relay tests — verify pass.** `cd rs && cargo test -p paigasus-iam relay`
Expected: PASS.

- [ ] **Step 5: Failing test — gRPC.** Pick one tonic service (e.g. `Authorization`): a handler test (or integration) asserts `iam_grpc_requests_total{service="Authorization",method="IsAuthorized"}` appears after a call.

- [ ] **Step 6: Instrument each gRPC handler.** In every tonic service impl method in `src/adapters/grpc/`, wrap the body:

```rust
use std::time::Instant;
use paigasus_observability::record_grpc;
async fn is_authorized(&self, request: Request<...>) -> Result<Response<...>, Status> {
    let started = Instant::now();
    let result = self.is_authorized_inner(request).await; // or inline the existing body into a local `result`
    record_grpc("Authorization", "IsAuthorized", started, &result);
    result
}
```

Prefer the minimal-diff shape: compute `let result = { <existing body> };` then `record_grpc(...); result`. Use the tonic service name (`Tenancy`/`Authentication`/`Authorization`/`ServiceAccount`/`Audit`) and the RPC method name as static literals. Do the same for the Health service or skip it (health is a static SERVING — skipping is fine; note it).

- [ ] **Step 7: Run tests + build.** `cd rs && cargo test -p paigasus-iam && cargo build -p paigasus-iam`
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): instrument iam outbox relay + grpc handlers (SMA-446)"
```

---

## Task A12: Slice A gate run + deny/machete waivers

**Files:**
- Modify (if gates require): `rs/deny.toml`, `Cargo.toml` `[package.metadata.cargo-machete]`

- [ ] **Step 1: Run the full gate list** from repo root:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-446-observability
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations
```

- [ ] **Step 2: Fix `:deny`** — if `metrics`/`metrics-exporter-prometheus` (or a transitive) trips a license check, add a minimal `[licenses] exceptions` entry to `rs/deny.toml` with a comment. If an advisory trips, add a dev-only `[advisories] ignore` with a rationale.

- [ ] **Step 3: Fix `:machete`** — if `metrics` is flagged unused in a crate where it's only used a commit later, add a temporary `[package.metadata.cargo-machete] ignored = ["metrics"]` (prune once consumed). It should be consumed within Slice A, so ideally no waiver is needed.

- [ ] **Step 4: Verify the resolved dep tree** has no duplicate `hyper`/`hyper-util` from `metrics-exporter-prometheus`:

```bash
cd rs && cargo tree -i hyper 2>/dev/null | head -40
```

If a second hyper appears solely from the exporter, confirm `default-features = false` is set (it disables the http-listener). Document the finding.

- [ ] **Step 5: Re-run gates until green.** Diagnose any unattributed "N failed" via `.moon/cache/ciReport.json` (`jq '.actions[]|select(.status=="failed")'`).

- [ ] **Step 6: Commit any waivers.**

```bash
git add rs/deny.toml rs/crates
git commit -m "chore(rs): deny/machete waivers for metrics deps (SMA-446)"
```

- [ ] **Step 7: Open PR 1** (via the open-pr stage — see the pipeline). PR title: `feat(rs): add prometheus metrics to iam + gateway (SMA-446)`. Body: "Part of SMA-446 (sub-project 3, Slice A)."

---

# SLICE B — Ops artifacts (PR 2)

> Slice B produces static artifacts validated by `promtool` + the drift test. It depends on Slice A's `names` registry and emitted metrics.

## Task B1: `ops/observability/` skeleton — compose + Prometheus scrape config

**Files:**
- Create: `ops/observability/README.md`, `docker-compose.yml`, `prometheus/prometheus.yml`

- [ ] **Step 1: Write `docker-compose.yml`** — Prometheus + Grafana, pinned images, `extra_hosts` for Linux:

```yaml
# SPDX-License-Identifier: Apache-2.0
services:
  prometheus:
    image: prom/prometheus:v3.1.0
    ports: ["9090:9090"]
    extra_hosts: ["host.docker.internal:host-gateway"]
    volumes:
      - ./prometheus/prometheus.yml:/etc/prometheus/prometheus.yml:ro
      - ./prometheus/rules:/etc/prometheus/rules:ro
  grafana:
    image: grafana/grafana:11.4.0
    ports: ["3000:3000"]
    environment:
      - GF_AUTH_ANONYMOUS_ENABLED=true
      - GF_AUTH_ANONYMOUS_ORG_ROLE=Admin
      - GF_SECURITY_ALLOW_EMBEDDING=true
    volumes:
      - ./grafana/provisioning:/etc/grafana/provisioning:ro
      - ./grafana/dashboards:/var/lib/grafana/dashboards:ro
```

(Pin to whatever the latest stable Prometheus/Grafana tags are at implementation time.)

- [ ] **Step 2: Write `prometheus/prometheus.yml`** — scrape both services + load the rules:

```yaml
# SPDX-License-Identifier: Apache-2.0
global:
  scrape_interval: 15s
rule_files:
  - /etc/prometheus/rules/*.rules.yml
scrape_configs:
  - job_name: iam
    static_configs:
      - targets: ["host.docker.internal:8080"]
  - job_name: gateway
    static_configs:
      - targets: ["host.docker.internal:8088"]
```

- [ ] **Step 3: Write `README.md`** — one screen: `docker compose up`, run the two services (`cargo run -p paigasus-iam`, `-p paigasus-gateway`), open Grafana at `:3000`, pointer into the RUNBOOK.

- [ ] **Step 4: Commit.**

```bash
git add ops/observability
git commit -m "feat(ci): add observability compose stack + prometheus scrape config (SMA-446)"
```

---

## Task B2: Alert rules + `promtool` proto-pin + CI gate

**Files:**
- Create: `ops/observability/prometheus/rules/{iam,gateway}.rules.yml`, `ops/observability/prometheus/rules/tests/{iam,gateway}.test.yml`
- Modify: `.prototools` + `.proto/plugins/` (promtool), a CI workflow step

- [ ] **Step 1: Write `iam.rules.yml`** with the spec §7.2 IAM alerts, using only registry metric names:

```yaml
# SPDX-License-Identifier: Apache-2.0
groups:
  - name: iam
    rules:
      - alert: IamDenialAuditDrops
        expr: rate(iam_denial_audits_dropped_total[5m]) > 0
        for: 0m
        labels: { severity: warning }
        annotations: { summary: "IAM denial-audit entries are being dropped (drain not keeping up)" }
      - alert: IamOutboxBacklogAgeHigh
        expr: iam_outbox_oldest_unpublished_age_seconds > 300
        for: 5m
        labels: { severity: warning }
        annotations: { summary: "IAM outbox oldest unpublished event age > 5m" }
      - alert: IamOutboxEventsParked
        expr: increase(iam_outbox_relay_parked_total[15m]) > 0
        for: 0m
        labels: { severity: warning }
        annotations: { summary: "IAM outbox parked a poison event" }
      - alert: IamOutboxRelayStalled
        expr: rate(iam_outbox_relay_ticks_total[10m]) == 0
        for: 10m
        labels: { severity: critical }
        annotations: { summary: "IAM outbox relay is not ticking (stalled but process alive)" }
      - alert: IamHighErrorRate
        expr: sum(rate(iam_http_requests_total{status_class="5xx"}[5m])) / sum(rate(iam_http_requests_total[5m])) > 0.05
        for: 10m
        labels: { severity: critical }
        annotations: { summary: "IAM HTTP 5xx ratio > 5%" }
      - alert: IamGrpcHighErrorRate
        expr: sum(rate(iam_grpc_requests_total{grpc_status!="ok"}[5m])) / sum(rate(iam_grpc_requests_total[5m])) > 0.05
        for: 10m
        labels: { severity: critical }
        annotations: { summary: "IAM gRPC non-OK ratio > 5%" }
```

- [ ] **Step 2: Write `gateway.rules.yml`** with the gateway alerts (`GatewayHighErrorRate`, `GatewayIamDependencyUnavailable`, `GatewayUpstreamErrors`) + a shared `TargetDown` (`up == 0`) group.

- [ ] **Step 3: Write `promtool test rules` fixtures** (`rules/tests/iam.test.yml`, `gateway.test.yml`): synthetic series that make each alert fire (and not fire), per the promtool unit-test schema. Example for one alert:

```yaml
# SPDX-License-Identifier: Apache-2.0
rule_files: [../iam.rules.yml]
evaluation_interval: 1m
tests:
  - interval: 1m
    input_series:
      - series: 'iam_denial_audits_dropped_total'
        values: '0 1 2 3 4 5'
    alert_rule_test:
      - eval_time: 5m
        alertname: IamDenialAuditDrops
        exp_alerts:
          - exp_labels: { severity: warning }
```

- [ ] **Step 4: Proto-pin `promtool`.** Add a proto plugin + `.prototools` entry for `promtool` following the existing cargo-deny/machete proto-plugin pattern (per-platform exe path). Verify `promtool --version` resolves via the shim.

- [ ] **Step 5: Add a CI step** (in `.github/workflows/ci.yml` or a Moon task on the root `repo` project) running:

```bash
promtool check config ops/observability/prometheus/prometheus.yml
promtool check rules ops/observability/prometheus/rules/*.rules.yml
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
```

- [ ] **Step 6: Run locally — verify green.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool check rules ops/observability/prometheus/rules/*.rules.yml
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
```

Expected: `SUCCESS`.

- [ ] **Step 7: Commit.**

```bash
git add ops/observability/prometheus/rules .prototools .proto .github
git commit -m "feat(ci): add prometheus alert rules + promtool gate (SMA-446)"
```

---

## Task B3: Grafana dashboards + provisioning

**Files:**
- Create: `ops/observability/grafana/provisioning/datasources/prometheus.yml`, `.../dashboards/dashboards.yml`, `grafana/dashboards/{iam,gateway}.json`

- [ ] **Step 1: Datasource provisioning** (`datasources/prometheus.yml`):

```yaml
apiVersion: 1
datasources:
  - name: Prometheus
    type: prometheus
    access: proxy
    url: http://prometheus:9090
    isDefault: true
```

- [ ] **Step 2: Dashboard provider** (`dashboards/dashboards.yml`): point Grafana at `/var/lib/grafana/dashboards`.

- [ ] **Step 3: Author `iam.json`** — a Grafana dashboard JSON model. Panels (each with a PromQL `expr` referencing only registry metrics):
  - HTTP request rate `sum(rate(iam_http_requests_total[$__rate_interval])) by (status_class)`
  - HTTP p95 latency `histogram_quantile(0.95, sum(rate(iam_http_request_duration_seconds_bucket[$__rate_interval])) by (le))`
  - gRPC request rate + non-ok ratio (`iam_grpc_requests_total`)
  - Authz decisions `sum(rate(iam_authz_decisions_total[$__rate_interval])) by (decision)`; cache hit ratio (`cache`)
  - Audit write rate `sum(rate(iam_audit_records_total[$__rate_interval])) by (outcome)`
  - Denial drops `rate(iam_denial_audits_dropped_total[$__rate_interval])`
  - Outbox: drain/published/failure rates, `iam_outbox_relay_parked_total` increase, `iam_outbox_oldest_unpublished_age_seconds` gauge, `iam_outbox_relay_ticks_total` rate.

  Build it by exporting from a running Grafana (Step against the local stack) or hand-authoring the minimal schema (`{ "title": "...", "panels": [ { "type": "timeseries", "targets": [ { "expr": "..." } ] } ], "schemaVersion": 39 }`). Keep `datasource` as `{ "type": "prometheus", "uid": "${DS_PROMETHEUS}" }` or the provisioned name.

- [ ] **Step 4: Author `gateway.json`** — panels: HTTP rate by status_class, p95 latency, inflight (`gateway_http_inflight_requests`), IAM-call rate+latency by `operation`/`result`, upstream rate+latency by `status_class`.

- [ ] **Step 5: Manually verify** against the local stack: `docker compose up`, run both services, drive a little traffic, confirm panels render (this is the G4 manual verification — the drift test + promtool are the automated gates).

- [ ] **Step 6: Commit.**

```bash
git add ops/observability/grafana
git commit -m "feat(ci): add grafana dashboards + provisioning (SMA-446)"
```

---

## Task B4: Name-drift test (dashboards/rules ↔ registry)

**Files:**
- Create: `rs/crates/libs/paigasus-observability/tests/drift.rs`
- Modify: `paigasus-observability/Cargo.toml` (`[dev-dependencies]` add a YAML parser, e.g. `serde_norway` or `serde_yaml`, + `serde_json` if not present)

**Interfaces:**
- Consumes: `paigasus_observability::names::{ALL, PROM_BUILTINS, PROMQL_TOKENS}`.

- [ ] **Step 1: Write the test** `tests/drift.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Asserts every metric identifier referenced in the committed Grafana dashboards + Prometheus
//! rules is a registered family (after suffix-normalisation) — so ops artifacts can't reference a
//! metric we don't emit. `promtool` covers PromQL *validity*; this covers *name drift*.

use paigasus_observability::names::{ALL, PROMQL_TOKENS, PROM_BUILTINS};

/// Strip histogram/summary/counter suffixes so `foo_seconds_bucket` matches registered `foo_seconds`.
fn normalize(id: &str) -> &str {
    for suf in ["_bucket", "_sum", "_count"] {
        if let Some(base) = id.strip_suffix(suf) { return base; }
    }
    id
}

fn is_known(id: &str) -> bool {
    let n = normalize(id);
    ALL.contains(&n) || ALL.contains(&id) || PROM_BUILTINS.contains(&id) || PROMQL_TOKENS.contains(&id)
}

/// Extract `[a-z_][a-z0-9_]*` identifiers that look like metric names (not preceded by `$`,
/// not a label key inside `{...}` value position). A simple token scan is sufficient given the
/// whitelist absorbs PromQL functions/keywords.
fn metric_idents(expr: &str) -> Vec<String> { /* regex-free scan: split on non-[a-z0-9_] and keep tokens that start with a lowercase letter and contain '_' or are in ALL */ unimplemented!() }

#[test]
fn dashboards_and_rules_reference_only_known_metrics() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."); // repo root from crate dir
    let mut unknown: Vec<String> = Vec::new();
    // Dashboards: parse JSON, walk panels[].targets[].expr
    for path in ["ops/observability/grafana/dashboards/iam.json", "ops/observability/grafana/dashboards/gateway.json"] {
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{root}/{path}")).unwrap()).unwrap();
        for expr in collect_exprs_from_dashboard(&json) {
            for id in metric_idents(&expr) { if !is_known(&id) { unknown.push(format!("{path}: {id}")); } }
        }
    }
    // Rules: parse YAML, walk groups[].rules[].expr
    for path in ["ops/observability/prometheus/rules/iam.rules.yml", "ops/observability/prometheus/rules/gateway.rules.yml"] {
        let doc: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(format!("{root}/{path}")).unwrap()).unwrap();
        for expr in collect_exprs_from_rules(&doc) {
            for id in metric_idents(&expr) { if !is_known(&id) { unknown.push(format!("{path}: {id}")); } }
        }
    }
    assert!(unknown.is_empty(), "dashboards/rules reference unknown metrics:\n{}", unknown.join("\n"));
}
```

Implement `metric_idents`, `collect_exprs_from_dashboard`, `collect_exprs_from_rules` as small helpers (recursive `serde_json`/`serde_yaml` walks collecting every `"expr"` string; a token scan that keeps identifiers containing `_` or present in `ALL`). Keep it deliberately simple — the whitelist absorbs false positives.

- [ ] **Step 2: Run — verify it PASSES** against the committed artifacts (they were authored from the registry). If it fails, the failure names the offending metric — fix the dashboard/rule or add the missing `const` to `names`. `cd rs && cargo test -p paigasus-observability --test drift`

- [ ] **Step 3: Deliberately break + confirm red** (sanity): temporarily add `expr: rate(iam_bogus_total[5m])` to a rule, run the test, confirm it fails naming `iam_bogus_total`, then revert.

- [ ] **Step 4: Commit.**

```bash
git add rs/crates/libs/paigasus-observability
git commit -m "test(rs): add dashboard/alert metric-name drift guard (SMA-446)"
```

---

## Task B5: RUNBOOK

**Files:**
- Create: `docs/ops/RUNBOOK-observability.md`

- [ ] **Step 1: Write the RUNBOOK** per spec §8 — sections: (1) Overview + where `/metrics` lives per service; (2) **Metric catalog** (every metric from §5: name/type/labels/meaning/expected range — must match `names` exactly); (3) Run the local stack (`ops/observability` `docker compose up` + run services + Grafana tour); (4) **Alerts → runbook entries** — for each alert in §7.2: meaning, likely causes, confirm, remediation, folding in: denial-drop best-effort tier + `[audit].denial_buffer_capacity` tuning; outbox backlog/parked/**stalled** (age gauge freezes → `IamOutboxRelayStalled` is the liveness signal), parked-event manual replay, DLQ/pruning are follow-ups; durability tiers (mutation exactly-once vs denial best-effort); audit retention/partitioning (monthly partitions, outcome-aware `DROP`/detach); gateway M0 internal-only-or-spend-cap + the `[metrics].addr` mandate for a public gateway; fail-open authz + Redis-outage TTL-bounded freshness; (5) Cardinality & privacy (why `model`/PRNs are never labels; how to add a metric safely — update `names` + dashboards + drift test); (6) Future (§14).

- [ ] **Step 2: Cross-check** every metric name mentioned against `names.rs` (copy-paste discipline; the drift test does not cover prose).

- [ ] **Step 3: Commit.**

```bash
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(rs): add observability RUNBOOK (SMA-446)"
```

---

## Task B6: Slice B gate run + close-out

- [ ] **Step 1: Run the full gate list** (as A12 Step 1) + the promtool + drift gates. Fix any failures.
- [ ] **Step 2: Confirm** `moon ci ...` is green and `promtool test rules` + `cargo test -p paigasus-observability --test drift` pass.
- [ ] **Step 3: Open PR 2.** Title: `feat(ci): add observability dashboards, alerts + RUNBOOK (SMA-446)`. Body: "Part of SMA-446 (sub-project 3, Slice B). Closes the epic's observability bullet."

---

## Verification summary (what "done" looks like)

- `paigasus-observability` builds, all unit tests pass; both services expose `/metrics`.
- Driving traffic through each service populates the spec §5 metric families (asserted by integration tests).
- `promtool check`/`test rules` pass; the drift test passes and fails-red on a bogus metric.
- `docker compose up` in `ops/observability` renders both dashboards against locally-run services.
- Full `moon ci` gate list green on both PRs.
- The RUNBOOK documents every metric + every alert's remediation.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
