# SMA-571 — Bind and ready-gate the listeners before migrating: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `paigasus-iam` bind its HTTP, metrics and gRPC sockets *before* running database migrations, serving `/healthz` 200 and `/readyz` 503 `migrating` (plus a well-formed gRPC `UNAVAILABLE`) until the real routers are atomically installed.

**Architecture:** All three sockets are bound synchronously in `serve()`, before the migration. A `BootSlot` (a `OnceLock<Serving>` plus the tonic health reporter) is read per request by an HTTP `fallback_service` and a gRPC fallback. The whole post-bind boot moves into one fallible `boot_deferred` function whose single caller handles SIGTERM and drains the `JoinSet` with a timeout on failure.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum 0.8, tonic 0.14 + tonic-health 0.14, tower/tower-http, sea-orm, Moon, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-27-sma-571-bind-before-migrate-design.md` — read it before starting. Every task below argues from it by section number.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- The workspace sets `warnings = deny`, and **dead code is a hard compile error on the lib target**. Every task must leave the crate compiling. New items in `adapters::boot` are `pub` (re-exported from the lib), so they are not dead code even before `main.rs` consumes them — this is what makes the staging below safe. Do not add a *private* helper before its caller exists.
- Shell commands need the proto-managed CLIs on PATH: prefix with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All `cargo` commands run from `rs/`.
- `cargo nextest` exits non-zero on a target with no tests — use `--no-tests=pass` where that is possible.
- Docker-gated tests **must** go through `tests/support/docker.rs`. Hand-rolling a skip fails `repo:iam-docker-policy-single-site`.
- A filtered nextest run (`-E 'test(foo)'`) skips the `docker_preflight` canary, so set `PAIGASUS_REQUIRE_DOCKER=1` for filtered runs or Docker-gated tests silently pass having run nothing.
- Commit messages: conventional commits with a workspace scope from `[rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace]`. Subject must **start lowercase** and be ≤100 chars. No `#NNN` in the body (breaks `footer-leading-blank`).
- Branch: `feature/sma-571-iam-bind-and-ready-gate-the-listeners-before-migrating` (already checked out in this worktree).

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs` | **Modify.** Extract `pub fn routes(state) -> Routes` as the single service-registration site; rebuild `router` from it via `add_routes`. | 1 |
| `rs/crates/services/paigasus-iam/src/adapters/boot.rs` | **Create.** `Serving`, `BootSlot`, `boot_http_router`, the deferred `/readyz` handler, and the HTTP fallback service. | 2 |
| `rs/crates/services/paigasus-iam/src/adapters/mod.rs` | **Modify.** `pub mod boot;` | 2 |
| `rs/crates/services/paigasus-iam/src/adapters/boot.rs` | **Modify.** Add `boot_grpc_routes` + the gRPC fallback. | 3 |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | **Modify.** `serve_http` takes an already-bound `TcpListener`. | 4 |
| `rs/crates/services/paigasus-iam/src/main.rs` | **Modify.** Synchronous binds, `boot_deferred`, `drain_bounded`, SIGTERM select. | 4 |
| `rs/crates/services/paigasus-iam/tests/boot_deferred.rs` | **Create.** Docker-free: empty-slot HTTP + gRPC status shape. | 2, 3 |
| `rs/crates/services/paigasus-iam/tests/boot_install_pg.rs` | **Create.** Docker-gated: swap, in-flight, double-install, real delegation. | 5 |
| `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs` | **Create.** Docker-gated subprocess e2e: deferred phase + SIGTERM. | 6 |
| `rs/Dockerfile` | **Modify.** `--start-period` 180s → 30s. | 7 |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs` | **Modify.** Delete the two constants and one test. | 7 |
| `rs/crates/services/paigasus-iam/src/config.rs` | **Modify.** Rewrite the `lock_wait_secs` doc comment. | 7 |
| `ci/images/run.sh` | **Modify.** Trim `assert_pins`; add `wait_ready` to `smoke()`. | 7 |
| `docs/ops/RUNBOOK-containers.md` | **Modify.** Probe table, §5 budgets, gateway note, gRPC health asymmetry. | 8 |

---

### Task 1: One gRPC service-registration site

Spec §4.4 / D8. tonic's `Router<L>` keeps `routes: Routes` private (`transport/server/mod.rs:151-154`), so the deferred path cannot reuse `grpc::router`. Without this task, Task 3 would hand-copy ten `add_service` calls, and a future service would mount in tests but not in production with CI green.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:84-128`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn routes(state: AppState) -> tonic::service::Routes`. Task 3 layers `AuthLayer` over it; Task 2's `Serving` stores the result.

- [ ] **Step 1: Write the failing guard test**

Append to the `#[cfg(test)] mod tests` block at the bottom of `src/adapters/grpc/mod.rs` (create the block if absent, with `use super::*;`):

```rust
/// SMA-571 D8: service registration must live at exactly ONE site. tonic's `Router` keeps its
/// `Routes` private, so production's deferred path (`adapters::boot`) cannot reuse `router()` —
/// it consumes `routes()` instead. If a future service is added to `router()` directly, it
/// mounts for the eleven Docker-gated suites that drive `router()` and is ABSENT in production,
/// with CI green. `include_str!` rather than a `repo:*` gate for the same reason
/// `migration_lock.rs`'s composition-root guard is: one call site does not justify a `T`-array
/// entry plus an `:affected-smoke` re-baseline.
#[test]
fn service_registration_lives_at_one_site() {
    const ME: &str = include_str!("mod.rs");
    let registrations = ME.matches(".add_service(").count();
    let in_routes = ME
        .split("pub fn routes(")
        .nth(1)
        .expect("a `pub fn routes(` must exist")
        .split("\npub ")
        .next()
        .expect("routes() must be followed by another item or EOF")
        .matches(".add_service(")
        .count();
    assert_eq!(
        registrations, in_routes,
        "every .add_service( must be inside `routes()` — found {registrations} total but only {in_routes} in routes()"
    );
    assert!(ME.contains(".add_routes("), "router() must build from routes() via Server::add_routes");
}
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(service_registration_lives_at_one_site)'
```

Expected: FAIL — `a \`pub fn routes(\` must exist` (the function does not exist yet).

- [ ] **Step 3: Extract `routes` and rebuild `router` from it**

In `src/adapters/grpc/mod.rs`, replace the body of `pub async fn router` (lines 84-128) with two items. Keep `router`'s existing doc comment on `router`, and move the service-inventory half of it onto `routes`.

```rust
/// The single service-registration site (SMA-571 D8). Both [`router`] — which the Docker-gated
/// integration suites drive — and production's deferred path (`adapters::boot::Serving`) build
/// from THIS function, because tonic's `Router` keeps its `Routes` private and cannot be
/// decomposed. Adding a service here mounts it on both; adding it anywhere else mounts it on
/// exactly one, which `service_registration_lives_at_one_site` exists to prevent.
///
/// Carries no layers: `CorrelationLayer` and `AuthLayer` are applied by the caller, because
/// production applies them at two different levels (`CorrelationLayer` on the tonic `Server`,
/// `AuthLayer` on these routes) while `router` applies both on the `Server`.
pub async fn routes(state: AppState) -> tonic::service::Routes {
    let (_reporter, health) = health_service().await;
    let audit_enabled = state.capabilities.audit_query;
    let mut routes = tonic::service::Routes::default()
        .add_service(health)
        .add_service(TenancyServiceServer::new(TenancyGrpc::new(state.clone())))
        .add_service(AuthnServiceServer::new(AuthnGrpc::new(state.clone())))
        .add_service(AuthorizationServiceServer::new(AuthzGrpc::new(state.clone())))
        .add_service(ServiceAccountServiceServer::new(ServiceAccountGrpc::new(state.clone())))
        .add_service(ServiceInfoServiceServer::new(ServiceInfoGrpc::new(state.clone())))
        .add_service(UserServiceServer::new(UserGrpc::new(state.clone())))
        .add_service(OutboxServiceServer::new(OutboxGrpc::new(state.clone())));
    if audit_enabled {
        routes = routes.add_service(AuditServiceServer::new(AuditGrpc::new(state)));
    }
    routes
}
```

Then `router` becomes:

```rust
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>> {
    let routes = routes(state.clone()).await;
    Server::builder()
        .timeout(timeout)
        .layer(CorrelationLayer)
        .layer(AuthLayer::new(state))
        .add_routes(routes)
}
```

`add_routes` takes `&mut self` (`transport/server/mod.rs:556`), so bind the builder to a `let mut` local if the chained form does not compile:

```rust
    let mut server = Server::builder().timeout(timeout).layer(CorrelationLayer).layer(AuthLayer::new(state));
    server.add_routes(routes)
```

Note the health reporter is dropped here exactly as before — Task 3 is what starts keeping it.

- [ ] **Step 4: Run the guard test and the whole lib**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
```

Expected: PASS, including `service_registration_lives_at_one_site`.

- [ ] **Step 5: Run the gRPC integration suites to prove `router` is unchanged**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam \
  --test grpc_tenancy --test grpc_authn --test grpc_authz --test grpc_audit \
  --test grpc_users --test grpc_dead_letters --test grpc_service_info \
  --test grpc_system_retirement --test api_keys_grpc
```

Expected: PASS. These eleven suites are the regression net for this refactor — if `routes` dropped a service they fail. If Docker is unreachable, `PAIGASUS_REQUIRE_DOCKER=1` turns the skip into a panic so you cannot get a false green.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs
git commit -m "refactor(rs): extract grpc::routes as the single service-registration site (SMA-571)"
```

---

### Task 2: The boot slot and the HTTP boot router

Spec §4.1, §4.3, §4.5. Nothing consumes this yet — that is Task 4. All items are `pub`, so the crate still compiles under `warnings = deny`.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/boot.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`
- Create: `rs/crates/services/paigasus-iam/tests/boot_deferred.rs`

**Interfaces:**
- Consumes: `grpc::routes` (Task 1), `http::app_routes` (already `pub(crate)`-visible within the crate — if it is private, widen it to `pub(crate)` in this task).
- Produces:
  - `pub struct Serving` with `pub async fn new(state: AppState, request_timeout: Duration) -> Self`
  - `pub struct BootSlot` with `pub fn new(reporter: HealthReporter) -> Self`, `pub async fn install(&self, serving: Serving) -> Result<(), AlreadyInstalled>`
  - `pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router`
  - `pub struct AlreadyInstalled` (a unit error type implementing `std::error::Error`)

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/boot_deferred.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-571: what the HTTP surface answers while the slot is EMPTY — i.e. for the whole window
//! between the bind and the end of the migration. These need no `AppState` and therefore no
//! Docker: an empty slot has no payload to build. The delegation half (a FULL slot) needs a real
//! `AppState` and lives in `boot_install_pg.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, boot_http_router};
use tower::ServiceExt; // for `oneshot`

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn empty_slot_router() -> axum::Router {
    let (reporter, _health) = paigasus_iam::adapters::grpc::health_service().await;
    boot_http_router(BootSlot::new(reporter), None)
}

/// AC 1's first half: liveness answers even though nothing is migrated.
#[tokio::test]
async fn healthz_is_200_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// AC 1's second half. `migrating` must be distinguishable from `unready` (the DB-ping failure
/// body, exercised in `boot_install_pg.rs`) — that distinction IS the acceptance criterion.
#[tokio::test]
async fn readyz_is_503_migrating_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(resp).await["status"], "migrating");
}

/// AC 2. The two wrong answers are called out by name because both are plausible bugs: a 404
/// means the fallback was never attached, a 401 means the real router leaked through and the
/// bearer layer answered — and a caller would read either as "this replica is up".
#[tokio::test]
async fn an_app_route_is_503_migrating_while_the_slot_is_empty() {
    let app = empty_slot_router().await;
    let resp = app.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an app route must 503 while migrating — NOT 404 (fallback missing) and NOT 401 (real router leaked)"
    );
    assert_eq!(body_json(resp).await["status"], "migrating");
}

/// SMA-504's cross-service contract: the deferred 503 is exactly the response a caller most wants
/// to retry, so it must carry correlation ids. `/healthz` and `/readyz` must NOT — that is pinned
/// by `tests/correlation_headers.rs:42-52` and this asserts the other side of the same line.
#[tokio::test]
async fn the_deferred_503_carries_correlation_headers_but_the_probes_do_not() {
    let app = empty_slot_router().await;
    let resp = app.clone().oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert!(resp.headers().contains_key("paigasus-request-id"), "the deferred fallback is inside CorrelationLayer");

    let resp = app.oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert!(!resp.headers().contains_key("paigasus-request-id"), "/readyz stays outside the API surface (D10)");
}
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test boot_deferred
```

Expected: FAIL to compile — `unresolved import paigasus_iam::adapters::boot`.

- [ ] **Step 3: Create the module**

Create `rs/crates/services/paigasus-iam/src/adapters/boot.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The deferred-router slot (SMA-571).
//!
//! `paigasus-iam` binds its sockets BEFORE it migrates, so for the whole migration window — and,
//! since SMA-559, for a lock-race loser's `migration.lock_wait_secs` wait — there is a live
//! listener with no `AppState` behind it. This module is what it serves: `/healthz` 200,
//! `/readyz` 503 `migrating`, and 503 `migrating` for every other path, swapped atomically for
//! the real routers once `AppState::new` returns.
//!
//! **Why one struct.** [`Serving`] carries the HTTP router, the gRPC routes AND the `AppState`
//! they were derived from, behind a single constructor. There is therefore no API that installs
//! a router without its state, which is how SMA-571 AC 4's "no window exists where the real
//! router is live but `AppState` is not" is satisfied — by the type, not by an ordering a future
//! edit could get wrong.
//!
//! **Why `OnceLock`, not `ArcSwapOption`.** The slot is written exactly once. `OnceLock` needs no
//! dependency, loads just as cheaply per request, and turns a double-install into a visible
//! `Err` rather than a silent replace.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use http::StatusCode;
use serde_json::json;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;

use crate::adapters::http::AppState;

/// Returned by [`BootSlot::install`] when the slot was already filled. A wiring defect rather
/// than an operator error: `install` has exactly one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyInstalled;

impl std::fmt::Display for AlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the boot slot was already installed")
    }
}

impl std::error::Error for AlreadyInstalled {}

/// Everything that only exists once the migration and `AppState::new` have completed.
///
/// Fields are private and [`Serving::new`] is the only constructor, so `http`, `grpc` and `state`
/// are necessarily derived from the SAME `AppState` — see the module doc.
pub struct Serving {
    http: Router,
    state: AppState,
}

impl Serving {
    /// Derives the full HTTP router from `state`. The router deliberately carries NO
    /// `/healthz`, `/readyz` or `/metrics` route: those live on [`boot_http_router`] permanently,
    /// outside `TraceLayer`/`TimeoutLayer`/`http_metrics_layer`, exactly as `serve_http` has
    /// always arranged them.
    /// `async` from the start even though this body awaits nothing yet: Task 3 adds the gRPC
    /// field, and `grpc::routes` is `async`. Declaring it now keeps the signature stable across
    /// tasks. An `async fn` with no `await` is not a warning.
    pub async fn new(state: AppState, request_timeout: Duration) -> Self {
        let http = crate::adapters::http::traced_app_routes(state.clone(), request_timeout);
        Self { http, state }
    }
}

/// The slot itself, plus the gRPC health reporter it flips.
///
/// Cheap to clone: an `Arc` and tonic-health's own `Arc`-backed handle.
#[derive(Clone)]
pub struct BootSlot {
    serving: Arc<OnceLock<Serving>>,
    reporter: HealthReporter,
}

impl BootSlot {
    pub fn new(reporter: HealthReporter) -> Self {
        Self { serving: Arc::new(OnceLock::new()), reporter }
    }

    /// The ONLY way to become ready.
    ///
    /// `async` and reporter-owning deliberately: `HealthReporter::set_service_status` is
    /// `async`, so leaving the flip to a second call from `main.rs` would reintroduce exactly the
    /// two-step window the module doc says does not exist — and forgetting it would leave gRPC
    /// health `NOT_SERVING` after a SUCCESSFUL boot while `/readyz` answered 200, so nothing
    /// would notice.
    ///
    /// Ordered slot-first: a request arriving between the two sees a working service whose health
    /// has not yet flipped, which is safe. The reverse is not.
    pub async fn install(&self, serving: Serving) -> Result<(), AlreadyInstalled> {
        self.serving.set(serving).map_err(|_| AlreadyInstalled)?;
        self.reporter.set_service_status("", ServingStatus::Serving).await;
        Ok(())
    }

    pub(crate) fn get(&self) -> Option<&Serving> {
        self.serving.get()
    }
}

/// The router bound BEFORE the migration. Owns `/healthz`, `/readyz` and `/metrics` permanently;
/// everything else falls through to the slot.
///
/// `metrics` is the same same-port `/metrics` router `main.rs` used to hand `serve_http` — `None`
/// when metrics are disabled or served on their own `metrics.addr`.
pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router {
    let deferred = Router::new()
        .fallback(deferred_fallback)
        .with_state(slot.clone())
        // SMA-504: the deferred 503 is precisely the response a caller wants to retry, so it
        // carries request/correlation ids. `/healthz` and `/readyz` below are merged OUTSIDE this
        // layer — `tests/correlation_headers.rs` pins that they stay header-free.
        .layer(paigasus_observability::CorrelationLayer);
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(slot)
        .fallback_service(deferred);
    if let Some(metrics) = metrics {
        app = app.merge(metrics);
    }
    app
}

/// Liveness. Unconditional: the process is running, which is all liveness ever claimed.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({ "status": "ok" })))
}

/// Readiness. Three distinct bodies, which is SMA-571 AC 1: `migrating` (no schema yet),
/// `unready` (schema present, database ping failed), `ready`.
async fn readyz(State(slot): State<BootSlot>) -> impl IntoResponse {
    let Some(serving) = slot.get() else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "migrating" })));
    };
    crate::adapters::http::ping_readiness(&serving.state).await
}

/// Every non-probe path while the slot is empty; delegation once it is full.
async fn deferred_fallback(State(slot): State<BootSlot>, req: axum::extract::Request) -> axum::response::Response {
    match slot.get() {
        None => (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "status": "migrating" }))).into_response(),
        // `OnceLock::get` is taken ONCE here, at dispatch — so a request in flight across an
        // `install` completes against the value it started with (AC 4's third clause).
        Some(serving) => {
            use tower::ServiceExt;
            serving.http.clone().oneshot(req).await.into_response()
        }
    }
}
```

- [ ] **Step 4: Add the two `http` helpers the module consumes**

`Serving::new` and `readyz` need two things `http/mod.rs` currently keeps inline. Add both to `src/adapters/http/mod.rs`, next to `serve_http`:

```rust
/// `app_routes` under the production `TraceLayer`/`TimeoutLayer` — extracted verbatim from
/// `serve_http`'s body so `adapters::boot::Serving` builds the SAME value the listener used to
/// build inline (SMA-571). `/healthz`, `/readyz` and `/metrics` stay outside it, as always.
pub fn traced_app_routes(state: AppState, request_timeout: Duration) -> Router {
    app_routes(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, request_timeout))
}

/// The database-ping half of readiness, split out of [`readyz`] so `adapters::boot`'s
/// slot-aware handler can reuse it rather than duplicate the ping and its logging (SMA-571).
pub async fn ping_readiness(state: &AppState) -> (StatusCode, Json<serde_json::Value>) {
    match state.db.execute(Statement::from_string(state.db.get_database_backend(), "SELECT 1")).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed: database ping error");
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unready" })))
        }
    }
}
```

Then rewrite the existing `readyz` handler to delegate, so the ping lives at one site:

```rust
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    ping_readiness(&state).await
}
```

and rewrite `serve_http`'s `traced` local to call the new helper:

```rust
    let traced = traced_app_routes(state.clone(), request_timeout);
```

- [ ] **Step 5: Register the module**

In `src/adapters/mod.rs`, add `pub mod boot;` in alphabetical order with the existing `pub mod` lines.

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test boot_deferred --lib
```

Expected: PASS — four tests in `boot_deferred`, plus the lib tests still green.

If `boot_http_router`'s two-state composition does not typecheck (both `Router`s carry `BootSlot` state, and `fallback_service` requires a `Service`, not a `Router<S>`), call `.with_state(slot)` on the inner router *before* passing it to `fallback_service` — which the code above already does — and if axum still rejects it, wrap with `.into_service()`. Do not reach for a different design; the shape is what the spec's AC 4 argument depends on.

- [ ] **Step 7: Confirm the existing readiness pin still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test correlation_headers --test health
```

Expected: PASS. `correlation_headers` pins that `/healthz` and `/readyz` carry no correlation ids; Step 3 keeps them outside the layer, and this is the check.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/boot.rs \
        rs/crates/services/paigasus-iam/src/adapters/mod.rs \
        rs/crates/services/paigasus-iam/src/adapters/http/mod.rs \
        rs/crates/services/paigasus-iam/tests/boot_deferred.rs
git commit -m "feat(rs): add the deferred-router slot and HTTP boot router (SMA-571)"
```

---

### Task 3: The gRPC boot routes

Spec §4.4, D7. The fallback must emit a **well-formed gRPC status**, not an HTTP 503 — `Status::unavailable(..).into_http()` produces HTTP 200 with `content-type: application/grpc` and `grpc-status: 14`. This is the single most consequential detail in the change: the gateway reads `UNIMPLEMENTED` (`Routes::default()`'s own fallback) as **ready**, so getting it wrong silently breaks AC 2 while looking like an improvement.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/boot.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/boot_deferred.rs`

**Interfaces:**
- Consumes: `grpc::routes` (Task 1), `BootSlot` (Task 2).
- Produces: `pub fn boot_grpc_routes<H: Health>(slot: BootSlot, health: HealthServer<H>) -> tonic::service::Routes`; `Serving` gains a private `grpc` field populated by `Serving::new`.

- [ ] **Step 1: Write the failing test**

Append to `rs/crates/services/paigasus-iam/tests/boot_deferred.rs`:

```rust
/// SMA-571 D7 — the single most consequential assertion in this change.
///
/// `paigasus-gateway`'s readiness probe classifies IAM's gRPC replies
/// (`gateway/src/adapters/http/mod.rs:146-150`): `Unavailable`/`DeadlineExceeded`/`Internal` mean
/// NOT ready, and **anything else — including `Unimplemented` — means READY**. `Routes::default()`
/// installs an `unimplemented` fallback, so a boot router that merely mounts health would make the
/// gateway report ready against a migrating IAM: AC 2 broken, and it would look like a fix.
///
/// A bare HTTP 503 is equally wrong — no gRPC client can interpret it. So this asserts the wire
/// shape, not a status code.
#[tokio::test]
async fn the_grpc_fallback_is_a_wellformed_unavailable_not_unimplemented() {
    use tower::ServiceExt;

    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    let routes = paigasus_iam::adapters::boot::boot_grpc_routes(BootSlot::new(reporter), health);

    let req = http::Request::builder()
        .method("POST")
        .uri("/paigasus.iam.v1.TenancyService/CreateOrganization")
        .header("content-type", "application/grpc")
        .body(tonic::body::Body::empty())
        .unwrap();
    let resp = routes.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), http::StatusCode::OK, "a gRPC status rides on HTTP 200, never a bare 503");
    assert_eq!(resp.headers()["content-type"], "application/grpc");
    assert_eq!(
        resp.headers()["grpc-status"],
        "14",
        "must be UNAVAILABLE (14). UNIMPLEMENTED (12) is Routes::default()'s own fallback and the \
         gateway reads it as READY — see gateway/src/adapters/http/mod.rs:150"
    );
}

/// Health must answer during the deferred phase, and answer NOT_SERVING — a `grpc_health_probe`
/// readiness probe is the gRPC-side equivalent of `/readyz` 503 `migrating`.
#[tokio::test]
async fn grpc_health_answers_not_serving_while_the_slot_is_empty() {
    use tower::ServiceExt;

    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await;
    let routes = paigasus_iam::adapters::boot::boot_grpc_routes(BootSlot::new(reporter.clone()), health);

    let req = http::Request::builder()
        .method("POST")
        .uri("/grpc.health.v1.Health/Check")
        .header("content-type", "application/grpc")
        .body(tonic::body::Body::empty())
        .unwrap();
    let resp = routes.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    assert_ne!(
        resp.headers().get("grpc-status").map(|v| v.to_str().unwrap()),
        Some("14"),
        "health must be served by the boot routes, not swallowed by the migrating fallback"
    );
}
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test boot_deferred
```

Expected: FAIL to compile — `boot_grpc_routes` not found.

- [ ] **Step 3: Add the gRPC half to `boot.rs`**

Add the `grpc` field to `Serving` and populate it in `new`:

```rust
pub struct Serving {
    http: Router,
    grpc: crate::adapters::grpc::authn::AuthEnforce<tonic::service::Routes>,
    state: AppState,
}
```

In `Serving::new` (now `async`, because `grpc::routes` is):

```rust
    pub async fn new(state: AppState, request_timeout: Duration) -> Self {
        let http = crate::adapters::http::traced_app_routes(state.clone(), request_timeout);
        // `AuthLayer` moves HERE from the tonic `Server`'s layer stack, because it needs
        // `AppState` and the boot-time server has none. Behaviour is unchanged: health and the
        // two introspect RPCs are `:path`-exempt (`grpc::authn::is_exempt`) and every non-exempt
        // RPC lives inside these routes.
        let grpc = tower::Layer::layer(
            &crate::adapters::grpc::authn::AuthLayer::new(state.clone()),
            crate::adapters::grpc::routes(state.clone()).await,
        );
        Self { http, grpc, state }
    }
```

Add the boot routes and their fallback:

```rust
/// The gRPC routes bound BEFORE the migration: the health service (reporting `NOT_SERVING` until
/// [`BootSlot::install`] flips it) plus a catch-all that answers `UNAVAILABLE` while the slot is
/// empty and delegates afterwards.
///
/// `Routes` has no `fallback_service` of its own, so the override goes through the inner
/// `axum::Router` — and it MUST override, because `Routes::default()` ships an `unimplemented`
/// fallback whose `UNIMPLEMENTED` the gateway reads as READY (see the test named for it).
pub fn boot_grpc_routes<H>(slot: BootSlot, health: tonic_health::pb::health_server::HealthServer<H>) -> tonic::service::Routes
where
    H: tonic_health::pb::health_server::Health,
{
    // Health is mounted HERE, on the boot routes, so it answers during the deferred phase and
    // reports whatever `BootSlot`'s reporter says. Note `grpc::routes(state)` mounts a health
    // service of its own with its own (dropped, statically SERVING) reporter — in production that
    // one is unreachable, because a health request matches this route and never falls through.
    // `router()` still needs it, which is why `routes()` keeps it.
    let routes = tonic::service::Routes::new(health);
    let inner: axum::Router = routes
        .into_axum_router()
        .fallback(deferred_grpc_fallback)
        .with_state(slot);
    tonic::service::RoutesBuilder::from(inner).routes()
}

async fn deferred_grpc_fallback(State(slot): State<BootSlot>, req: axum::extract::Request) -> axum::response::Response {
    match slot.get() {
        // HTTP 200 + `content-type: application/grpc` + `grpc-status: 14`, via `Status::into_http`
        // — a bare 503 is not a gRPC status and no client can interpret it. Mirrors
        // `grpc::authn::reject`.
        None => tonic::Status::unavailable("migrating").into_http::<axum::body::Body>(),
        Some(serving) => {
            use tower::ServiceExt;
            // `AuthEnforce` is a `Service<Request<tonic::body::Body>>` while axum hands us
            // `Request<axum::body::Body>` — two distinct types. Mirrors `Routes::add_service`'s
            // own `map_request` (tonic-0.14.6/src/service/router.rs:91).
            let req = req.map(tonic::body::Body::new);
            serving.grpc.clone().oneshot(req).await.expect("AuthEnforce is Infallible").map(axum::body::Body::new)
        }
    }
}
```

- [ ] **Step 4: Make `grpc::authn`'s items reachable**

`AuthLayer` is already `pub`. `AuthEnforce` is `pub` but the `authn` module may be `pub(crate)` — check `src/adapters/grpc/mod.rs`'s module list and widen `pub mod authn;` if needed so `boot.rs` can name `AuthEnforce<Routes>` in a `pub struct`'s private field. A private field of a non-nameable type is allowed, so if widening is awkward, keep `authn` as-is and add `type BootGrpc = ...` inside `boot.rs`.

- [ ] **Step 5: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test boot_deferred --lib
```

Expected: PASS — six tests in `boot_deferred`.

**Known risk (spec §4.4):** axum's `fallback_service`/`fallback` path may require `Sync` on the delegated service, which tonic's `Server::layer` path does not. If `AuthEnforce<Routes>` is not `Sync`, wrap it: store `tower::util::BoxCloneSyncService<http::Request<tonic::body::Body>, http::Response<tonic::body::Body>, std::convert::Infallible>` in `Serving.grpc` instead, built with `BoxCloneSyncService::new(...)`. Do not change the layering to work around it.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/boot.rs \
        rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs \
        rs/crates/services/paigasus-iam/tests/boot_deferred.rs
git commit -m "feat(rs): serve a well-formed grpc unavailable while the boot slot is empty (SMA-571)"
```

---

### Task 4: Bind synchronously and restructure the composition root

Spec §3.3, §4.6. This is the task that actually delivers the feature. `servers.spawn` does **not** establish a bind — `serve_http` binds inside its spawned task (`http/mod.rs:954`) and tonic binds inside `serve_with_shutdown` — so without this, everything above is inert.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (`serve_http` signature)
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`

**Interfaces:**
- Consumes: `boot_http_router`, `boot_grpc_routes`, `BootSlot`, `Serving` (Tasks 2-3).
- Produces: `serve_http(listener: TcpListener, app: Router, shutdown) -> io::Result<()>`; `boot_deferred(db, config, slot, servers, rx, request_timeout, metrics_handle) -> anyhow::Result<()>` and `drain_bounded(servers, budget) -> usize` as private fns in `main.rs`; `const DRAIN_TIMEOUT: Duration`.

- [ ] **Step 1: Write the failing test for the bounded drain**

Add to `src/main.rs`'s `#[cfg(test)] mod tests` block (create it at the bottom of the file if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-571 §4.6: the boot-failure drain MUST be bounded. `main.rs`'s shutdown-path drain has
    /// no timeout; reused unchanged for a boot failure, a task that never observes the watch would
    /// hang the process with three listening sockets serving 503 FOREVER — CrashLoopBackOff never
    /// happens and the replica is indistinguishable from a slow migration, which is exactly the
    /// state D4 rejects.
    #[tokio::test]
    async fn drain_bounded_returns_at_the_timeout_when_a_task_ignores_the_watch() {
        let (tx, rx) = tokio::sync::watch::channel(());
        let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let mut good = rx.clone();
        servers.spawn(async move {
            let _ = good.changed().await;
            Ok(())
        });
        servers.spawn(async move {
            std::future::pending::<()>().await;
            Ok(())
        });
        let _ = tx.send(());
        let started = tokio::time::Instant::now();
        let outstanding = drain_bounded(&mut servers, Duration::from_millis(200)).await;
        assert!(started.elapsed() < Duration::from_secs(2), "must return at the timeout, not hang");
        assert_eq!(outstanding, 1, "the task that ignored the watch is reported, not silently dropped");
    }

    /// The first error is surfaced rather than swallowed.
    #[tokio::test]
    async fn drain_bounded_joins_cooperative_tasks_and_reports_none_outstanding() {
        let (tx, rx) = tokio::sync::watch::channel(());
        let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();
        for _ in 0..3 {
            let mut r = rx.clone();
            servers.spawn(async move {
                let _ = r.changed().await;
                Ok(())
            });
        }
        let _ = tx.send(());
        assert_eq!(drain_bounded(&mut servers, Duration::from_secs(5)).await, 0);
    }
}
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --bin paigasus-iam
```

Expected: FAIL to compile — `drain_bounded` not found.

- [ ] **Step 3: Change `serve_http` to take a bound listener**

In `src/adapters/http/mod.rs`, replace `serve_http`'s signature and body. It has exactly one caller.

```rust
/// Serve `app` on an ALREADY-BOUND `listener` until `shutdown` resolves.
///
/// The bind moved out to `main.rs` (SMA-571 §3.3): binding inside this function meant binding
/// inside a spawned task, which gave no ordering guarantee that the socket was listening before
/// the migration started — and deferred a bind failure (`EADDRINUSE`) until after the whole
/// migration window, reporting the migration's error rather than the bind's.
///
/// Router composition also moved out: `main.rs` now builds `boot_http_router`, which owns
/// `/healthz`, `/readyz` and `/metrics` permanently and falls through to the deferred slot.
pub async fn serve_http(
    listener: tokio::net::TcpListener,
    app: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}
```

- [ ] **Step 4: Restructure `serve()` in `main.rs`**

Four edits, in order.

**(a)** Delete the `IMAGE_START_PERIOD_SECS`/`MIGRATION_BUDGET_SECS` import (line 12) and the boot warning (lines 113-119) — Task 7 finishes the rest of that removal, but the import must go now or the crate will not compile once Task 7 deletes the constants. Leave `migrate_under_lock` and `migrate_under_lock`'s other imports alone.

**(b)** After `Database::connect` and before the migration, build the slot and bind all three sockets:

```rust
    let db = Database::connect(config.database_url.as_str()).await?;

    // SMA-571: bind BEFORE migrating, so a replica that is migrating — or, since SMA-559, waiting
    // up to `migration.lock_wait_secs` for the lock — is visibly UNREADY to its orchestrator
    // rather than absent. Bound HERE, synchronously, and not inside the spawned tasks below:
    // `servers.spawn` gives no ordering guarantee that a socket is listening before the `await`
    // that follows it, and it would defer an `EADDRINUSE` past the whole migration window.
    let (health_reporter, health_server) = grpc::health_service().await;
    health_reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await;
    let slot = boot::BootSlot::new(health_reporter);

    let http_listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    // `TcpIncoming::bind` is synchronous and public. NOTE `serve_with_incoming*` discards the
    // `Server`'s TCP configuration, and `Server::default()` sets `tcp_nodelay: true` — so the
    // nodelay must be re-applied HERE or Nagle is silently re-enabled on every gRPC connection.
    let grpc_incoming = tonic::transport::server::TcpIncoming::bind(config.grpc_addr)?.with_nodelay(Some(true));
    let metrics_listener = match config.metrics.addr {
        Some(addr) if metrics_handle.is_some() => Some((tokio::net::TcpListener::bind(addr).await?, addr)),
        _ => None,
    };

    let request_timeout = Duration::from_secs(30);
    let (tx, rx) = tokio::sync::watch::channel(());
    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();

    let http_metrics_router = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => Some(paigasus_observability::metrics_router(handle.clone())),
        _ => None,
    };
    {
        let mut rx = rx.clone();
        let app = boot::boot_http_router(slot.clone(), http_metrics_router);
        servers.spawn(async move {
            serve_http(http_listener, app, async move {
                let _ = rx.changed().await;
            })
            .await
            .map_err(anyhow::Error::from)
        });
    }
    if let Some((listener, metrics_addr)) = metrics_listener {
        let mut rx = rx.clone();
        let metrics_app = paigasus_observability::metrics_router(metrics_handle.clone().expect("guarded above"));
        servers.spawn(async move {
            tracing::info!(%metrics_addr, "paigasus-iam metrics listener started");
            axum::serve(listener, metrics_app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    {
        let mut rx = rx.clone();
        let routes = boot::boot_grpc_routes(slot.clone(), health_server);
        servers.spawn(async move {
            tonic::transport::Server::builder()
                .timeout(request_timeout)
                .layer(paigasus_observability::CorrelationLayer)
                .serve_with_incoming_shutdown(routes.prepare(), grpc_incoming, async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }
    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam listeners bound; migrating");
```

**(c)** Move everything from `migrate_under_lock` through the last background-task spawn into `boot_deferred`, and call it under a `select!`:

```rust
    // SMA-571 §4.6: the whole post-bind boot is ONE fallible function so `?` can be used freely
    // inside it and the drain is structural rather than per-`?`. Adding a fallible step here can
    // no longer skip the graceful shutdown.
    let shutting_down = std::sync::atomic::AtomicBool::new(false);
    let outcome = tokio::select! {
        r = boot_deferred(&db, &config, &slot, &mut servers, &rx, request_timeout, metrics_handle.clone()) => r,
        () = shutdown_signal() => {
            // Unhandled until SMA-571, but the pod is now PRESENT-and-unready rather than absent,
            // so a rolling update is far more likely to land here. Ignoring SIGTERM for
            // `lock_wait_secs` and then taking SIGKILL is the stranded-lock scenario in
            // RUNBOOK-containers.md. Cancelling `migrate_under_lock` between polls is safe, and
            // cancelling inside `Migrator::up` rolls the transaction back and releases the
            // transaction-scoped lock by construction.
            tracing::info!("shutdown signal received during boot");
            shutting_down.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    };
    if outcome.is_err() || shutting_down.load(std::sync::atomic::Ordering::Relaxed) {
        if let Err(e) = &outcome {
            tracing::error!(error = %e, "boot failed after the listeners were bound; draining");
        }
        let _ = tx.send(());
        let outstanding = drain_bounded(&mut servers, DRAIN_TIMEOUT).await;
        if outstanding > 0 {
            tracing::warn!(outstanding, "drain timed out with tasks still running");
        }
        return outcome;
    }

    tracing::info!(%config.http_addr, %config.grpc_addr, "paigasus-iam started");
```

**(d)** Add the two new functions and the constant near `shutdown_signal`:

```rust
/// How long the boot-failure drain waits before giving up and returning anyway. Bounded because
/// an unbounded drain turns a boot failure into a process that serves 503 forever (SMA-571 §4.6).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything between the bind and "ready": the migration, `AppState::new`, the publisher dial,
/// and every background task. Returns `Err` rather than `?`-ing out of `serve()` so its single
/// caller can drain the already-bound listeners gracefully (SMA-571 AC 3).
///
/// Lives in `main.rs` deliberately: `migration_lock.rs`'s `the_composition_root_still_migrates_
/// under_the_lock` guard reads THIS file for `migrate_under_lock(` and `config.migration.lock_wait()`.
///
/// Panics are NOT drained — a panic unwinds through `#[tokio::main]` and aborts in-flight requests.
/// `catch_unwind` across this body would need `AssertUnwindSafe` and buys little: the
/// route-registration panic class is already covered by
/// `protected_router_merge_has_no_path_conflicts_in_any_capability_combination`.
async fn boot_deferred(
    db: &sea_orm::DatabaseConnection,
    config: &IamConfig,
    slot: &boot::BootSlot,
    servers: &mut JoinSet<anyhow::Result<()>>,
    rx: &tokio::sync::watch::Receiver<()>,
    request_timeout: Duration,
    metrics_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
) -> anyhow::Result<()> {
    let migration = migrate_under_lock(db, config.migration.lock_wait()).await?;
    tracing::info!(
        waited = ?migration.waited,
        polls = migration.polls,
        migrations_applied = migration.migrations_applied,
        "database migrations complete"
    );
    let state = AppState::new(db.clone(), config).await?;

    // Then, moved here VERBATIM from `serve()` and in this order — each keeps its existing doc
    // comment, its `rx.clone()`, and its `state`/`db.clone()` captures:
    //   1. the `db_for_maintenance` / `db_for_outbox_retention` clones
    //   2. the publisher selection (`PublisherBackend::Nats` / `Tracing`) + the NATS
    //      connection-gauge sampler spawn
    //   3. the metrics `run_upkeep` task, gated on the `metrics_handle` parameter
    //   4. the policy-snapshot background reload
    //   5. the denial-audit drain
    //   6. the outbox relay + the `wake_on_commit` listener
    //   7. the audit partition maintainer (startup tick + loop)
    //   8. the outbox retention maintainer (startup sweep + loop)
    //
    // The three listener spawns are NOT here — they moved above `boot_deferred`'s call site.
    //
    // The NATS publisher moves back INTO block 2 where it belongs: SMA-471 hoisted it above the
    // first `servers.spawn` because an early `?` past a live listener would abort in-flight
    // requests. This function's caller drains instead, so rewrite that comment to say so rather
    // than deleting it — the reason the hoist existed is worth keeping.

    slot.install(boot::Serving::new(state, request_timeout).await).await?;
    tracing::info!("boot slot installed; serving");
    Ok(())
}

/// Drain `servers`, bounded. Returns how many tasks were STILL running at the timeout so the
/// caller can log it — a silent give-up would hide exactly the wedged task worth naming.
async fn drain_bounded(servers: &mut JoinSet<anyhow::Result<()>>, budget: Duration) -> usize {
    let _ = tokio::time::timeout(budget, async {
        while let Some(joined) = servers.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "a server task failed during drain"),
                Err(join_err) => tracing::warn!(error = %join_err, "a server task panicked during drain"),
            }
        }
    })
    .await;
    servers.len()
}
```

Keep the existing `select!` + drain tail at the end of `serve()` unchanged — that is the *shutdown* path, not the boot path.

- [ ] **Step 5: Run the drain tests and the whole crate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --bin paigasus-iam --lib --test boot_deferred
```

Expected: PASS, including both `drain_bounded` tests.

- [ ] **Step 6: Confirm nothing else regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy --workspace -- -D warnings && cargo fmt --check
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: clean clippy/fmt; the full IAM suite green. This is the first point at which the whole 69-suite net is meaningful, so do not skip it.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/main.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): bind iam's listeners before migrating and drain on boot failure (SMA-571)"
```

---

### Task 5: Docker-gated — the install, in-flight, and real delegation

Spec §6.2(a). Tasks 2-3 test the *empty* slot, which needs no `AppState`. This task tests the full one, so production's actual composition is exercised rather than a stub — otherwise all 69 existing suites drive `http::router`/`grpc::router` and nothing drives the boot path.

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/boot_install_pg.rs`

**Interfaces:**
- Consumes: `BootSlot`, `Serving`, `boot_http_router` (Tasks 2-4); `support::{start_migrated_postgres, app_with_state}`.
- Produces: nothing.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/tests/boot_install_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-571: the boot router with a FULL slot — a real `Serving` built from a real `AppState`,
//! not a stub. Docker-gated because `AppState::new` reconciles system policies into Postgres and
//! compiles a policy snapshot out of it.
//!
//! Without this file, production's composition (`boot_http_router` → fallback → the real
//! `app_routes` under `TraceLayer`/`TimeoutLayer`) would be exercised by nothing: every existing
//! suite drives `http::router` instead.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, Serving, boot_http_router};
use std::time::Duration;
use tower::ServiceExt;

async fn slot_and_router() -> Option<(BootSlot, axum::Router, paigasus_iam::adapters::http::AppState)> {
    let (_node, db) = support::start_migrated_postgres().await?;
    let (_app, state, _idp) = support::app_with_state(db).await;
    let (reporter, _health) = paigasus_iam::adapters::grpc::health_service().await;
    let slot = BootSlot::new(reporter);
    let router = boot_http_router(slot.clone(), None);
    Some((slot, router, state))
}

/// AC 4's first clause, and the failure mode that would make every other test here pass while the
/// feature did nothing: the router must read the slot PER REQUEST, not capture its contents when
/// it was built. Same router value on both sides of the install.
#[tokio::test]
async fn the_swap_takes_effect_on_an_already_built_router() {
    let Some((slot, router, state)) = slot_and_router().await else {
        return;
    };

    let resp = router.clone().oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE, "empty slot => migrating");

    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("first install");

    let resp = router.clone().oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the SAME router value must now see the installed slot");

    // Real delegation: an app route now reaches the real `app_routes`, so an unauthenticated call
    // is a 401 from the bearer layer — not the 503 it was a moment ago, and not a 404.
    let resp = router.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "503 -> 401 is what proves delegation happened");
}

/// AC 4's third clause. `OnceLock::get` is taken once at dispatch, so a request that started
/// before the install completes against the value it started with.
#[tokio::test]
async fn a_request_in_flight_across_the_install_completes_against_its_pre_swap_value() {
    let Some((slot, router, state)) = slot_and_router().await else {
        return;
    };

    let in_flight = tokio::spawn({
        let router = router.clone();
        async move { router.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap() }
    });
    // Let the request reach dispatch before the slot changes under it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("install");

    let resp = in_flight.await.expect("in-flight task");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a request dispatched before the install must finish as 503 migrating, never tear across the swap"
    );
}

/// D6: `OnceLock` makes a double install a visible error rather than a silent replace.
#[tokio::test]
async fn a_second_install_is_rejected() {
    let Some((slot, _router, state)) = slot_and_router().await else {
        return;
    };
    slot.install(Serving::new(state.clone(), Duration::from_secs(30)).await).await.expect("first install");
    assert!(slot.install(Serving::new(state, Duration::from_secs(30)).await).await.is_err());
}
```

- [ ] **Step 2: Run and confirm they fail or skip correctly**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test boot_install_pg
```

Expected: with Docker up, these should mostly pass already (Tasks 2-4 built the mechanism); the point of writing them is that they *can* fail. If any fails, fix the implementation — not the test. If Docker is down, `PAIGASUS_REQUIRE_DOCKER=1` panics rather than silently passing.

- [ ] **Step 3: Verify the in-flight test actually bites**

Temporarily change `deferred_fallback` in `boot.rs` to re-read the slot after an `await` (e.g. `tokio::task::yield_now().await;` before `slot.get()`), re-run `a_request_in_flight_across_the_install_completes_against_its_pre_swap_value`, and confirm it FAILS. Then revert.

This is the step that proves the test is not vacuous. Use `Edit` to revert, not a `.bak` file — restoring by `mv` rolls mtime backwards and cargo will serve the binary built from the temporary edit.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/boot_install_pg.rs
git commit -m "test(rs): cover the boot-slot install, in-flight swap, and real delegation (SMA-571)"
```

---

### Task 6: Docker-gated — the deferred phase end to end

Spec §6.2(b)(c). This is the only test that exercises bind → wait → migrate → install through the real composition root, and the one that would catch a regression re-ordering the bind back behind the migration. `serve()` lives in `src/main.rs` and is unreachable from `tests/`, so this spawns the built binary as a subprocess.

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs`

**Interfaces:**
- Consumes: `support::{start_raw_postgres, connection_url}`; `env!("CARGO_BIN_EXE_paigasus-iam")`; `MIGRATION_LOCK_KEY`.
- Produces: nothing.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-571 AC 1 + AC 2 end to end, through the real composition root.
//!
//! `serve()` lives in `src/main.rs` and integration tests link the LIB, so this spawns the built
//! binary as a subprocess — the only suite in the crate that does. The alternative, re-creating
//! the boot ordering in-process, would be vacuous: the ordering IS what is under test.
//!
//! The deferred phase is pinned open by holding SMA-559's migration advisory lock from a second
//! session, so this does not race a fast migration.

mod support;

use paigasus_iam::adapters::persistence::migration_lock::MIGRATION_LOCK_KEY;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::process::Stdio;
use std::time::Duration;

/// Kills the child on drop, so a failing assertion cannot leave a service holding a port.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn scalar_bool(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<bool>("", "v")
        .expect("bool column")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

async fn http_status(port: u16, path: &str) -> Option<(u16, String)> {
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}{path}"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let status = resp.status().as_u16();
    Some((status, resp.text().await.ok()?))
}

fn spawn_iam(db_url: &str, http_port: u16, grpc_port: u16) -> Child {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-iam"));
    cmd.env("IAM_DATABASE_URL", db_url)
        .env("IAM_HTTP_ADDR", format!("127.0.0.1:{http_port}"))
        .env("IAM_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
        .env("IAM_AUTHN__ISSUERS", r#"[{issuer="https://idp.example.com",audiences=["paigasus"]}]"#)
        .env("IAM_API_KEYS__PEPPER", "cGFpZ2FzdXMtc21va2UtcGVwcGVyLW5vdC1hLXJlYWwtc2VjcmV0LTAwMA==")
        .env("IAM_MIGRATION__LOCK_WAIT_SECS", "60")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Child(cmd.spawn().expect("spawn paigasus-iam"))
}

/// AC 1 and AC 2: while the migration lock is held elsewhere, the replica is BOUND and visibly
/// unready — not absent. Before SMA-571 both requests below would be connection-refused.
#[tokio::test]
async fn a_lock_blocked_replica_is_bound_and_reports_migrating() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot lifecycle test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;

    // `pg_try_advisory_lock`, not the blocking form: the latter returns void and cannot assert
    // its own setup, so a holder that silently failed would make this whole test vacuous.
    let holder = Database::connect(&url).await.expect("holder connection");
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let (http_port, grpc_port) = (free_port(), free_port());
    let _child = spawn_iam(&url, http_port, grpc_port);

    // Poll for the bind — the process still has to load config and connect to Postgres.
    let mut healthz = None;
    for _ in 0..100 {
        if let Some(r) = http_status(http_port, "/healthz").await {
            healthz = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (status, _) = healthz.expect("the listener must bind while the migration lock is held");
    assert_eq!(status, 200, "AC 1: /healthz answers 200 while migrating");

    let (status, body) = http_status(http_port, "/readyz").await.expect("readyz");
    assert_eq!(status, 503, "AC 1: /readyz is 503 while migrating");
    assert!(body.contains("migrating"), "AC 1: the body distinguishes migrating from a failed ping, got {body}");

    assert!(
        scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually release the lock"
    );

    // Once the lock is free the replica migrates and flips to ready.
    let mut ready = false;
    for _ in 0..300 {
        if let Some((200, _)) = http_status(http_port, "/readyz").await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "the replica must become ready once the migration lock is released");
}

/// §4.6: SIGTERM during the deferred phase must drain promptly, not be ignored until the lock
/// wait expires and SIGKILL arrives — which is the stranded-backend scenario in the RUNBOOK.
#[cfg(unix)]
#[tokio::test]
async fn sigterm_during_the_deferred_phase_exits_promptly() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot lifecycle test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = Database::connect(&url).await.expect("holder connection");
    assert!(scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await);

    let (http_port, grpc_port) = (free_port(), free_port());
    let mut child = spawn_iam(&url, http_port, grpc_port);
    for _ in 0..100 {
        if http_status(http_port, "/healthz").await.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let pid = child.0.id() as i32;
    unsafe { libc::kill(pid, libc::SIGTERM) };

    let started = std::time::Instant::now();
    loop {
        if child.0.try_wait().expect("try_wait").is_some() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "SIGTERM must be honoured during the deferred phase, not after lock_wait_secs");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

- [ ] **Step 2: Add the test-only dependencies**

`reqwest` and `libc` are needed by this suite only. Add to `rs/crates/services/paigasus-iam/Cargo.toml` under `[dev-dependencies]`, using `workspace = true` if the workspace already declares them (check `rs/Cargo.toml` first — `reqwest` almost certainly exists for the authn adapter):

```toml
reqwest = { workspace = true }
libc = "0.2"
```

If `libc` is not already a workspace dependency, add it to `rs/Cargo.toml`'s `[workspace.dependencies]` too and reference it as `{ workspace = true }`, matching the repo's default form. A new workspace dep may need a `rs/deny.toml` licence check — `libc` is MIT/Apache-2.0, so expect a no-op.

- [ ] **Step 3: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test boot_lifecycle_pg
```

Expected: PASS. `CARGO_BIN_EXE_paigasus-iam` requires the binary to be built — nextest builds it as part of the package, but if the env var is missing at compile time, run `cargo build -p paigasus-iam --bin paigasus-iam` first.

- [ ] **Step 4: Prove the test bites**

Temporarily move the three `servers.spawn` blocks in `main.rs` back to *after* `boot_deferred`, re-run `a_lock_blocked_replica_is_bound_and_reports_migrating`, and confirm it FAILS on "the listener must bind while the migration lock is held". Revert with `Edit`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs rs/crates/services/paigasus-iam/Cargo.toml rs/Cargo.toml rs/Cargo.lock
git commit -m "test(rs): prove iam binds and reports migrating while the lock is held (SMA-571)"
```

---

### Task 7: Retire the start-period coupling

Spec §5.1, AC 5. Six sites express one fact that no longer holds. Note the two edits that are easy to miss: `ci/images/run.sh:149` interpolates variables the deleted block defines (`set -u` will fail the script), and `smoke()` asserts `/readyz` 200 immediately after `wait_healthy`, which only polls `/healthz` — that one reds `main` after merge rather than the PR, because `images.yml` is not a required check.

**Files:**
- Modify: `rs/Dockerfile:70-77`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs:37-43, 263-276`
- Modify: `rs/crates/services/paigasus-iam/src/config.rs:676-681`
- Modify: `ci/images/run.sh:119-149, 222-233, 319-321`

**Interfaces:** none produced or consumed.

- [ ] **Step 1: Delete the constants and their test**

In `migration_lock.rs`, delete `IMAGE_START_PERIOD_SECS` (line 39), `MIGRATION_BUDGET_SECS` (line 43) with their doc comments, and the whole `the_default_wait_plus_the_migration_budget_fits_the_image_start_period` test (lines ~263-276).

**Keep** `the_composition_root_still_migrates_under_the_lock` — Task 4 kept `boot_deferred` in `main.rs` precisely so all three of its `include_str!` assertions still hold.

- [ ] **Step 2: Rewrite the config doc comment**

In `src/config.rs`, replace the final paragraph of `lock_wait_secs`'s doc (lines 679-681):

```rust
    /// There is no static ceiling any more. Before SMA-571 the container's `HEALTHCHECK
    /// --start-period` had to cover this wait, because a waiting replica had no listener bound
    /// and was invisible; now it is bound and answers `/readyz` 503 `migrating`, so overrunning
    /// this wait is a visible unready replica rather than an absent one.
    pub lock_wait_secs: u64,
```

- [ ] **Step 3: Shrink the Dockerfile start period**

Replace `rs/Dockerfile` lines 70-77's comment block and `HEALTHCHECK`:

```dockerfile
# 30s start period: config load, a successful Database::connect, and the three binds. Since
# SMA-571 IAM binds BEFORE it migrates, so /healthz answers within a second of start and the
# start period no longer has to cover migration.lock_wait_secs or the migration itself.
# ci/images/run.sh's assert_pins keeps a floor on this. Note --interval=30s means the first
# probe fires ~30s in regardless, so an interval is the effective floor either way.
# This governs docker run / Compose / Swarm and ci/images/run.sh only — the kubelet ignores a
# HEALTHCHECK entirely, so Kubernetes sizes startupProbe instead (docs/ops/RUNBOOK-containers.md).
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/usr/local/bin/paigasus-service", "healthcheck"]
```

- [ ] **Step 4: Trim `assert_pins` and fix its success line**

In `ci/images/run.sh`, delete the lock-wait/budget/`IMAGE_START_PERIOD_SECS` logic (lines ~119-148) but keep a floor, and **edit line 149** — it interpolates `${start_period}` and `${required}`, and under `set -euo pipefail` (line 14) leaving it would fail the script:

```bash
  local start_period
  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" | head -1 | grep -oE '[0-9]+' || true)"
  if [ -z "$start_period" ]; then
    echo "::error::could not read the HEALTHCHECK --start-period; the grep anchor moved, or the HEALTHCHECK was removed." >&2
    return 1
  fi
  # SMA-571 removed the start-period <-> lock_wait_secs coupling: IAM binds before migrating, so
  # this only has to cover config load + Database::connect + the binds. A floor is kept so that
  # deleting the HEALTHCHECK, or setting --start-period=0s, is still caught — after the coupling
  # was removed, nothing else reads this value at all.
  if [ "$start_period" -lt 30 ]; then
    echo "::error::rs/Dockerfile's HEALTHCHECK --start-period=${start_period}s is below the 30s floor." >&2
    return 1
  fi

  echo "  pins OK: rustc ${channel}, bookworm builder, ubuntu ${ubuntu_from} == chisel release, no baked service config, start-period ${start_period}s >= 30s"
}
```

- [ ] **Step 5: Add `wait_ready` to `smoke()`**

`wait_healthy` (line 222) polls only the container HEALTHCHECK, i.e. `/healthz`. After SMA-571 that goes 200 while `/readyz` is still 503 `migrating` against a fresh database running m0001–m0008 plus `reconcile_starter` and `PolicySnapshot::new`. Add next to `wait_healthy`:

```bash
# SMA-571: `healthy` no longer implies migrated — IAM binds before it migrates, so /healthz
# answers 200 while /readyz is still 503 "migrating". Without this the very next assertion races
# a fresh database's full migration set. Its own budget, deliberately separate from wait_healthy's.
wait_ready() {
  local name="$1" url="$2" i code
  for i in $(seq 1 120); do
    code="$(docker run --rm --network "$NET" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
    [ "$code" = "200" ] && { echo "  $name is ready (${i}s)"; return 0; }
    sleep 1
  done
  echo "::error::$name never became ready (last /readyz status: $code)" >&2
  docker logs "$name" 2>&1 | tail -30 >&2
  return 1
}
```

Match the existing `expect_status` helper's mechanism for issuing the HTTP request rather than the `curlimages/curl` invocation above if it differs — read `expect_status` first and reuse it. Then call it between lines 319 and 321:

```bash
  wait_healthy "$IAM_NAME"
  wait_ready "$IAM_NAME" "http://${IAM_NAME}:8080/readyz"
  expect_status "iam /healthz" "http://${IAM_NAME}:8080/healthz" 200
  expect_status "iam /readyz"  "http://${IAM_NAME}:8080/readyz"  200
```

- [ ] **Step 6: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
bash -n ci/images/run.sh && shellcheck ci/images/run.sh || true
bash ci/images/run.sh all
```

Expected: lib tests pass (with the deleted test gone and the composition-root guard still green); `run.sh` parses; the image builds and the smoke suite passes with the new `wait_ready`. `ci/images/run.sh all` is slow (a `--release` build) — run it, because `images.yml` is not a required check and this is the only place the change is exercised before merge.

- [ ] **Step 7: Commit**

```bash
git add rs/Dockerfile ci/images/run.sh \
        rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs \
        rs/crates/services/paigasus-iam/src/config.rs
git commit -m "refactor(rs): retire the start-period to lock-wait coupling (SMA-571)"
```

---

### Task 8: Operator documentation

Spec §5.2, AC 6. Note what this task must **not** do: relax the `maxSurge` guidance. The precondition `RUNBOOK-containers.md:150-153` records is `reconcile_starter`'s untested boot concurrency, which SMA-571 does not touch.

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md:81-101, 111-153, 186-192`

**Interfaces:** none.

- [ ] **Step 1: Update the probe table**

At line ~92, the startup row currently reads "IAM migrates at boot, and since SMA-559 a replica that loses the migration-lock race also *waits* with nothing bound — budget `lock_wait_secs` + the migration + `AppState::new`, see §5". Replace with:

```markdown
| startup | `GET /healthz` | Since SMA-571 IAM binds before it migrates, so this only covers process start: config load, `Database::connect`, and the binds. A migrating replica is *unready*, not absent — `/readyz` carries the distinction |
```

- [ ] **Step 2: Rewrite the probe-budget block**

Replace the "**Probe budgets.**" paragraph (lines ~129-144) with:

```markdown
  **Probe budgets.** A migrating or lock-waiting replica now has all three sockets bound and
  answers `/healthz` 200 within a second of process start, so `startupProbe` no longer has to be
  sized against `migration.lock_wait_secs` at all — budget it for config load plus
  `Database::connect`. What a long migration now costs is readiness, not existence: `/readyz`
  answers `503 {"status":"migrating"}` for as long as it takes, and the replica stays out of the
  Service's endpoint list until it flips. Set `readinessProbe.failureThreshold ×
  periodSeconds` above your worst-case `lock_wait_secs` + migration + `AppState::new` if you would
  rather a slow migration not restart the pod.

  `/readyz` has three bodies and they are not interchangeable: `migrating` means the schema is not
  yet applied, `unready` means the schema is there but the database ping failed, `ready` means
  serving. Alert on sustained `migrating`, page on `unready`.
```

- [ ] **Step 3: Correct the chart-defaults paragraph**

At lines ~146-153, SMA-571's mention currently says it "will make the `start-period` coupling vestigial" — that is now done. Replace only that sentence, and **leave the `maxSurge` precondition and both exceptions exactly as they are**:

```markdown
  `startupProbe` no longer needs sizing against `IAM_MIGRATION__LOCK_WAIT_SECS` (SMA-571 removed
  the `start-period` coupling entirely — see the probe budgets above), but still expose the env
  var so a slow migration can be given more room.
```

- [ ] **Step 4: Update the gateway-facing note**

At line ~188, after the existing "Gateway `/readyz` issues a real gRPC introspect call to IAM" bullet, add:

```markdown
- **A migrating IAM now answers on a live socket rather than refusing the connection** (SMA-571).
  HTTP returns `503 {"status":"migrating"}`; gRPC returns a well-formed `UNAVAILABLE` (HTTP 200 with
  `grpc-status: 14`), and gRPC health reports `NOT_SERVING`. The gateway needs no change: its
  readiness classification already treats `Unavailable` as not-ready, and its channel is built with
  `connect_lazy`, so a dead IAM has always surfaced as `Rpc(Status::Unavailable)` rather than a
  connect error. One caveat for a future topology: if IAM is ever fronted by a headless Service with
  client-side load balancing, a subchannel to a migrating replica stays READY and returns per-RPC
  `UNAVAILABLE` instead of being evicted on TRANSIENT_FAILURE — correct, but worth knowing before
  adopting that shape.
- **gRPC health is not equivalent to `/readyz` after startup.** `grpc.health.v1.Health` reports
  `NOT_SERVING` during the migration and `SERVING` once installed, and then stays `SERVING`
  regardless of later database health, while `/readyz` can go 503 `unready` on a failed ping.
  A `grpc_health_probe` readiness probe therefore catches the boot case but not a later database
  outage; use the HTTP probe for readiness. Making gRPC health track `/readyz` is a deferred
  follow-up.
```

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "start-period\|maxSurge\|migrating" docs/ops/RUNBOOK-containers.md
```

Expected: no surviving claim that a migrating replica has nothing bound; the `maxSurge` precondition text unchanged.

- [ ] **Step 6: Run the full CI graph as CI does**

Per-project tasks do not run the repo-level gates. This is the last task, so run the whole thing:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep --base origin/main \
  --include-relations
```

If it reports an unattributed failure, find it with `jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

Expect to check specifically: `:iam-docker-policy-single-site` (the two new Docker-gated suites must go through `tests/support/docker.rs`), `:machete` (the new `libc` dev-dependency must actually be used — it is, by the SIGTERM test), and `:deny` (a new workspace dependency may need a licence entry).

- [ ] **Step 7: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md
git commit -m "docs(ops): record that a migrating iam answers on a live socket (SMA-571)"
```

---

## Spec coverage check

| Spec section | Task |
|---|---|
| §3.3 / D3 synchronous binds | 4 |
| §4.1 slot, `Serving`, `BootSlot`, async `install` | 2, 3 |
| §4.2 / D7 gRPC `UNAVAILABLE` not `UNIMPLEMENTED` | 3 |
| §4.3 HTTP wiring, `CorrelationLayer` on the fallback only, `/metrics` | 2 |
| §4.4 / D8 one registration site, `AuthLayer` relocation, `into_http` | 1, 3 |
| §4.5 swap semantics (AC 4) | 5 |
| §4.6 `boot_deferred`, bounded drain, SIGTERM, panics out of scope | 4, 6 |
| §5.1 six-site removal + `ci/images` line 149 + `wait_ready` | 7 |
| §5.2 RUNBOOK, `maxSurge` left alone | 8 |
| §6.1 Docker-free tests | 2, 3, 4 |
| §6.2 Docker-gated tests | 5, 6 |
| AC 1 | 2 (unit), 6 (e2e) |
| AC 2 | 2, 3 (unit), 6 (e2e) |
| AC 3 | 4 (drain), 6 (SIGTERM) |
| AC 4 | 5 |
| AC 5 | 7 |
| AC 6 | 8 |
