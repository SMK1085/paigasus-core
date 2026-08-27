# SMA-571 — Bind and ready-gate the listeners before migrating

**Issue:** [SMA-571](https://linear.app/smaschek/issue/SMA-571/) · **Status:** approved, revised after one adversarial pass
**Related:** [SMA-559](https://linear.app/smaschek/issue/SMA-559/) (advisory lock — this was split out
of it, and this issue retires the coupling SMA-559 could only document), SMA-500 (container images,
owns `rs/Dockerfile` and `ci/images/run.sh`), SMA-513 (Helm chart, consumes the relaxed
`startupProbe` budget — but **not** a relaxed `maxSurge`, see §5.2)

## 1. Problem

`paigasus-iam`'s composition root migrates before it binds anything. In `main.rs` the order is

```
IamConfig::load → validate → paigasus_logging::init → metrics recorder
  → Database::connect
  → migrate_under_lock                          (line 120)
  → AppState::new                               (line 138)
  → publisher dial (NATS)                       (line 164)
  → servers.spawn(NATS gauge sampler)           (line 178)
  → servers.spawn(serve_http / metrics / grpc)  (lines 197, 213, 229)
```

so for the whole migration window the process holds no socket. To an orchestrator the replica is not
*unready*, it is **absent** — indistinguishable from a pod that has not started. The only lever is
probe tuning.

SMA-559 made this worse rather than better. Its advisory lock is correct, but a replica that loses
the lock race now legitimately *waits* for up to `migration.lock_wait_secs` (default 120s) with
nothing bound. SMA-559 had to compensate by coupling a static Dockerfile value to a runtime config
value across six sites:

* `rs/Dockerfile:76` — `HEALTHCHECK --start-period=180s`
* `migration_lock.rs:39,43` — `IMAGE_START_PERIOD_SECS = 180`, `MIGRATION_BUDGET_SECS = 60`
* `main.rs:113-119` — a boot warning when `lock_wait_secs + MIGRATION_BUDGET_SECS > IMAGE_START_PERIOD_SECS`
* `migration_lock.rs:266-276` — a unit test asserting the default fits
* `ci/images/run.sh:119-149` — `assert_pins`'s triple check tying all three together
* `RUNBOOK-containers.md` §5 — a startupProbe budget an operator computes by hand

One fact — "a migrating replica answers nothing" — expressed six times. This issue removes the fact,
and therefore all six.

### 1.1 Acceptance criteria (verbatim)

Quoted so coverage can be checked rather than inferred.

1. A replica that is migrating (or waiting on SMA-559's migration lock) answers `GET /healthz`
   **200** and `GET /readyz` **503** with a body distinguishing "migrating" from a failed database
   ping.
2. Traffic is never routed to a replica whose schema is not yet migrated.
3. A migration failure after the listeners are live shuts them down **gracefully** — `tx.send(())`
   and drain the `JoinSet` — rather than an early `?` return that aborts in-flight requests. This is
   the invariant `main.rs:135-141` currently protects by having nothing bound.
4. The router swap is tested directly: that it takes effect, that no window exists where the real
   router is live but `AppState` is not, and that an in-flight request across the swap behaves.
5. `rs/Dockerfile`'s `--start-period` and the `lock_wait_secs` ↔ `start_period` invariant SMA-559
   documented become vestigial and are removed or simplified.
6. `docs/ops/RUNBOOK-containers.md` is updated: the probe table, and the gateway-facing note that a
   migrating IAM now answers 503 on a live socket rather than refusing the connection.

## 2. Why this is not a reordering

`AppState::new` cannot run before the migration:

* `adapters/http/mod.rs:409` — `bootstrap::reconcile_starter` reconciles system policies **into**
  Postgres.
* `adapters/http/mod.rs:412-416` — `PolicySnapshot::new` → `load_and_compile` **reads** the policy
  store.

Both hit tables migration m0004 creates, so on a fresh database they fail with `relation does not
exist`. The listener must be *serving* before the state it would normally carry exists. That is the
whole difficulty, and it is why three obvious approaches were rejected during SMA-559's review:

* **Bind early, `axum::serve` later.** `listen()` succeeds so TCP connects, but nothing accepts. The
  probe hangs and times out — strictly worse than connection-refused, which at least fails fast.
* **Serve a boot router, then hand the listener to a second `axum::serve`.** A port-free race window
  across the handoff, dropping probes.
* **`OnceCell<AppState>`.** Changes the signature of every HTTP handler and every gRPC service.

Note the third rejection is about threading state through *handlers*. It says nothing about the
slot mechanism itself, which is why §4.1 uses a `OnceLock` for the slot without contradiction.

## 3. Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | **Both** HTTP and gRPC bind early — symmetric, no documented asymmetry | §4.2: the asymmetry is not untidiness, it is a correctness hazard for the gateway |
| D2 | Bind **after** `Database::connect`, before `migrate_under_lock` | covers exactly the window the issue names; a bad `database_url` still crash-loops with nothing bound |
| D3 | Sockets are bound **synchronously in `serve()`**, not inside spawned tasks | §3.3 — without this the feature is unproven |
| D4 | A post-bind failure drains gracefully (bounded), then exits **non-zero** | CrashLoopBackOff stays the signal for a broken replica |
| D5 | The `--start-period` ↔ `lock_wait_secs` coupling is **deleted**, not shrunk | a dead invariant nobody can re-derive is the hazard the issue names |
| D6 | One `OnceLock` slot holding **one struct** with a **single constructor** | makes AC 4's no-window property structural |
| D7 | gRPC's deferred fallback answers a well-formed gRPC `UNAVAILABLE` | the gateway's readiness classification depends on it — §4.2 |
| D8 | gRPC service registration gets **one site**, shared by production and tests | §4.4 — otherwise the deferred path silently forks |

### 3.1 D2 — why not bind before `Database::connect`

Binding even earlier is tempting: a replica whose Postgres is unreachable would then answer
`/healthz` 200 and `/readyz` 503 instead of crash-looping, and a transient DB outage during a
rolling update would self-heal without restarts.

Rejected because it trades a loud failure for a quiet one. sea-orm's `Database::connect` pings
eagerly, so today a misconfigured `database_url` fails fast and visibly. Under an earlier bind it
would instead sit permanently NotReady — which looks exactly like "still migrating" and is much
easier to miss. Restart-based recovery is also lost.

### 3.2 D4 — why not stay up serving 503

* **Stay up, 503 forever.** The orchestrator sees a steady NotReady pod rather than
  CrashLoopBackOff. But a permanently broken replica then looks identical to a slow one, and
  `kubectl get pods` loses the ability to distinguish "still migrating" from "migration failed".
* **Retry the migration in a loop.** Self-heals a transient blip, but turns a deterministic
  migration bug into an infinite retry that never surfaces, and is materially larger than the issue.

### 3.3 D3 — `servers.spawn` does not establish a bind

Both listeners currently bind *inside* their spawned task: `serve_http` calls `TcpListener::bind` at
`http/mod.rs:954`, and tonic's `Server::serve_with_shutdown` calls `bind_incoming` at
`tonic-0.14.6/src/transport/server/mod.rs:694`. Spawning a task and then awaiting
`migrate_under_lock` therefore gives **no ordering guarantee that either socket is listening
first** — the feature would be unproven, and would fail exactly under the load that delays the
spawned task.

It also breaks D4's fail-fast: a bind failure (`EADDRINUSE`, a taken `metrics.addr`) would surface
only after the whole deferred phase — up to `lock_wait_secs` plus the migration — and the error
reported by the `select!` at `main.rs:472-493` would be the migration's, not the bind's.

So `serve()` binds all three sockets itself, before `boot_deferred`, and `?`s on each:

* HTTP and the optional separate metrics listener: `TcpListener::bind(addr).await?`, with
  `serve_http`'s signature changed to accept an already-bound listener. It has exactly one caller
  (`main.rs:198`).
* gRPC: `TcpIncoming::bind(addr)?` (`transport/server/incoming.rs:59` — public and synchronous),
  then `serve_with_incoming_shutdown`.

**Trap, recorded because it is silent:** `serve_with_incoming*` "discards any provided `Server` TCP
configuration" (`mod.rs:1062,1091`), and `Server::default()` sets `tcp_nodelay: true`
(`mod.rs:132`). The `TcpIncoming` must therefore be built with `.with_nodelay(Some(true))`
(`incoming.rs:67`) or Nagle is silently re-enabled on every gRPC connection — a latency regression
no test would catch.

## 4. Design

### 4.1 The slot

A new module, `adapters::boot`:

```rust
/// Everything that only exists once the migration and `AppState::new` have completed.
/// Fields are private and there is exactly ONE constructor, so `http`, `grpc` and `state`
/// are necessarily derived from the same `AppState`.
pub struct Serving {
    http:  axum::Router,                       // app_routes + TraceLayer + TimeoutLayer
    grpc:  AuthEnforce<tonic::service::Routes>, // grpc::routes(state) under AuthLayer
    state: AppState,                           // for /readyz's DB ping
}

impl Serving {
    pub fn new(state: AppState, request_timeout: Duration) -> Self { /* derives all three */ }
}

#[derive(Clone)]
pub struct BootSlot {
    serving:  Arc<OnceLock<Serving>>,
    reporter: tonic_health::server::HealthReporter,
}

impl BootSlot {
    pub fn new(reporter: HealthReporter) -> Self;
    /// The ONLY way to become ready. Sets the slot first, then flips gRPC health.
    pub async fn install(&self, serving: Serving) -> Result<(), AlreadyInstalled>;
    fn get(&self) -> Option<&Serving>;
}
```

**`OnceLock`, not `ArcSwapOption`.** The slot is written exactly once. `OnceLock` gives the same
cheap per-request load, adds no dependency, and makes a double-install a visible `Err` rather than a
silent replace — which strengthens D6 rather than weakening it. (An earlier draft used `arc-swap`
and claimed the promotion would re-baseline `repo:affected-smoke`'s `lockfile->all-lint` set. That
was wrong: `ci/affected-graph/run.sh:330-336` enumerates *crates and tasks* keyed on `rs/Cargo.lock`,
not dependencies, so a new workspace dep changes nothing there and a re-baseline would itself red
the gate. Dropping the dependency moots it either way.)

**`install` is `async` and owns the reporter.** `HealthReporter::set_service_status` is
`pub async fn` (`tonic-health-0.14.6/src/server.rs:74`). Making the flip a second, separate call
from `main.rs` would reintroduce precisely the two-step window §4.5 claims does not exist — and a
forgotten call would leave gRPC health permanently `NOT_SERVING` after a *successful* boot, with
`/readyz` answering 200 so nothing else notices. Ordering is slot-first, then reporter: a request
arriving between the two sees a working service whose health has not yet flipped, which is safe;
the reverse is not.

Boot then proceeds in three phases:

| phase | bound | `/healthz` | `/readyz` | gRPC health | gRPC RPCs |
|---|---|---|---|---|---|
| **pre-bind** — config, validate, logging, metrics, `Database::connect`, the three binds | nothing | conn refused | conn refused | conn refused | conn refused |
| **deferred** — `migrate_under_lock`, `AppState::new`, publisher dial | all three sockets, slot empty | `200 {"status":"ok"}` | `503 {"status":"migrating"}` | `NOT_SERVING` | `UNAVAILABLE` |
| **serving** — after `install().await` | all three, slot set | `200` | `200 ready` / `503 unready` | `SERVING` | real |

`/readyz`'s three bodies are what AC 1 asks for: `migrating` (schema not yet applied) is distinct
from `unready` (the DB ping failed) is distinct from `ready`.

### 4.2 gRPC — why symmetry is a correctness requirement

`paigasus-gateway`'s `/readyz` infers IAM reachability from a sentinel `IntrospectApiKey`
(`gateway/adapters/http/mod.rs:140-151`) and classifies:

```rust
Err(IamError::Connect(_))                                            => not ready,
Err(IamError::Rpc(s)) if Unavailable | DeadlineExceeded | Internal   => not ready,
Err(IamError::Rpc(_)) | Ok(_)                                        => READY,
```

`UNIMPLEMENTED` falls in the last arm — and `Routes::default()` installs an `unimplemented` fallback
(`tonic-0.14.6/src/service/router.rs:51,138-141`). So a gRPC port that binds early serving *only*
health would make the gateway report **ready** against a migrating IAM, violating AC 2 while looking
like an improvement. Hence D7.

For the same reason, leaving gRPC to bind late would be *safe* but not good: the gateway's channel
is built with `connect_lazy` (`gateway/adapters/iam/client.rs:192`), so `IamError::Connect` is a
**build-time** fault only and a dead IAM actually surfaces as `Rpc(Status::Unavailable)`
(documented at `client.rs:28-31`). The conclusion holds — `Unavailable` is in the not-ready arm —
but via a different mechanism than an earlier draft of this spec claimed, and the mechanism is worth
stating correctly because a reader will re-derive it.

**Deployment topology.** The gateway holds one lazy channel to a single `iam.grpc_addr`, so a
migrating replica is simply a `/readyz` 503 away from the endpoint list. If IAM is ever fronted by a
headless Service with client-side load balancing, a subchannel to a migrating replica would stay
READY and return per-RPC `UNAVAILABLE` rather than being evicted on TRANSIENT_FAILURE — correct, but
worth knowing before that topology is adopted. Recorded in the RUNBOOK (§5.2).

### 4.3 HTTP wiring

```rust
pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router
```

owns `/healthz`, `/readyz` and `/metrics` **permanently**, with a `fallback_service` that reads the
slot. `Serving.http` is exactly today's `traced` value in `serve_http` —
`app_routes(state).layer(TraceLayer).layer(TimeoutLayer)` — and deliberately carries no health
routes.

This preserves the layering invariant `health_router`/`readyz_router` document: `/healthz`,
`/readyz` and `/metrics` stay outside `TraceLayer`, `TimeoutLayer` and `http_metrics_layer`, so a
15s Prometheus scrape or a probe poll still emits no request-span trace and does not inflate RED
metrics.

**`CorrelationLayer` on the fallback branch only.** `CorrelationLayer` and `http_metrics_layer` are
attached inside `app_routes` (`http/mod.rs:895,901`), i.e. inside `Serving.http`. Left alone, the
deferred phase's 503s would carry no `paigasus-request-id`/`paigasus-correlation-id` and no
`paigasus-retryable` — breaking SMA-504's cross-service contract for exactly the responses a caller
most wants to retry. So `CorrelationLayer` is attached to the fallback branch, and **not** to
`/healthz` or `/readyz`: `tests/correlation_headers.rs:42-52` pins that both are header-free, and
that pin must keep passing.

**`/metrics` during the deferred phase.** It is served, which makes a stuck replica scrapeable. But
`http_metrics_layer` lives inside `Serving.http`, so deferred-phase 503s increment nothing —
`/metrics` will show a live-but-empty IAM. That is the honest state of affairs, not a bonus; §7
records the decision not to add a family for it.

**404 semantics.** Today an unknown path is handled within one merged router; afterwards it comes
from inside `Serving.http` and is therefore inside `TraceLayer`/`TimeoutLayer`/`http_metrics_layer`.
Implementation must verify this is unchanged in observable behaviour and add a test pinning that an
unknown path still 404s and is still attributed the same way by `http_metrics_layer`.

`serve_http` has exactly one caller (`main.rs:198`), so its signature changes freely (D3 changes it
anyway). `pub fn router(state)` — the `oneshot` harness entry point used across the suite — is
**unchanged**.

### 4.4 gRPC wiring — one registration site

**The drift hazard.** tonic's `Router<L>` keeps `routes: Routes` private with no accessor
(`transport/server/mod.rs:151-154`), so `Serving.grpc` **cannot** be derived from the existing
`grpc::router(state, timeout)`. Keeping `grpc::router` "as-is" and adding an "additive" deferred
path therefore means a hand-maintained second copy of all nine `add_service` calls plus the
conditional `AuditService` (`grpc/mod.rs:96-120`). A future service added the way SMA-501 added
three would be mounted in `router()` — which all eleven Docker-gated gRPC suites drive — and
**absent in production**, with CI fully green. On a transport that serves `OutboxService`
break-glass and `UserService.CreateUser`, that is not an acceptable failure mode.

Hence D8. Registration is extracted to a single site:

```rust
pub fn routes(state: AppState) -> tonic::service::Routes   // the nine services + conditional audit
```

* `grpc::router(state, timeout)` — unchanged externally — becomes
  `Server::builder().timeout(t).layer(CorrelationLayer).layer(AuthLayer::new(state.clone()))
  .add_routes(routes(state))` (`add_routes` at `mod.rs:556`). All eleven suites keep working
  against identical behaviour.
* `Serving.grpc` is `AuthLayer::new(state).layer(routes(state))` → `AuthEnforce<Routes>`.

A test asserts both paths mount the same method set, so the fork cannot silently reopen.

**Serving.** `Server::serve_with_shutdown(addr, svc, signal)` is public on `impl<L> Server<L>`
(`mod.rs:678`) and takes an arbitrary `S` with `L: Layer<S>`; `.prepare()` is what tonic's own
`Router::serve_with_shutdown` does (`mod.rs:1054-1056`); `RecoverError`/`LoadShed`/
`ConcurrencyLimit`/`GrpcTimeout` are applied outside the user stack (`mod.rs:1234-1239`). So nothing
is reimplemented. With D3's pre-bound socket this becomes `serve_with_incoming_shutdown` over a
`TcpIncoming` — see §3.3's nodelay trap.

**`AuthLayer` moves onto the deferred routes**, because `AuthLayer::new(state)` needs `AppState`.
`CorrelationLayer` stays on the boot-time server so it remains outermost among our layers, as today.
Two consequences to handle rather than discover:

* `AuthEnforce<S>` implements `Service<http::Request<tonic::body::Body>>` only (`grpc/authn.rs:27,
  136-138`), while `axum::Router::layer` wants `Request<axum::body::Body>` — two distinct types
  (`tonic-0.14.6/src/body.rs:12`). The boot fallback must map `axum::body::Body → tonic::body::Body`
  before calling `Serving.grpc`, mirroring `router.rs:91`. axum's `fallback_service` additionally
  requires `Sync`, which tonic's `Server::layer` path does not — to be confirmed against
  `AuthEnforce<Routes>` at implementation time; a `BoxCloneSyncService` is the fallback plan.
* `is_exempt`'s health-prefix arm (`grpc/authn.rs:133`) becomes dead in *production* — health is
  matched by the outer boot routes and never reaches `AuthEnforce` — while the suites, which drive
  `grpc::router`, keep exercising it. Note it in the code rather than deleting it.

**The fallback must emit a real gRPC status.** `Routes` has no `fallback_service`; the override goes
through `axum_router_mut()`/`into_axum_router()` plus `From<axum::Router> for Routes`
(`router.rs:106-136`), and it must *replace* `Routes::default()`'s existing `unimplemented`
fallback. The response is `Status::unavailable("migrating").into_http()` — HTTP **200** with
`content-type: application/grpc` and `grpc-status: 14` (`status.rs:607-615`; precedent at
`grpc/authn.rs:199-201`) — **not** an HTTP 503, which no gRPC client can interpret and which would
silently defeat D7.

**The health reporter is kept.** `health_service()` currently builds a `HealthReporter` and drops it
(`grpc/mod.rs:60`), deferring dynamic readiness to M1. We retain it, set `NOT_SERVING` at bind, and
hand it to `BootSlot` (§4.1).

### 4.5 The swap — AC 4

`Serving`'s fields are private with a single constructor taking one `AppState`, so **there is no API
that installs a router without the state it was derived from** — nor any way to pass one `AppState`
to `app_routes` and a different one to `state`. AC 4's hardest clause is therefore a property of the
type. `install` being `async` and owning the reporter (§4.1) extends the same guarantee across the
gRPC health flip, which would otherwise be the one remaining two-step.

`OnceLock::get` is taken **once per request at dispatch**. A request in flight across an `install`
completes against the value it started with: no torn state, no mid-request upgrade. That is the
answer to AC 4's third clause.

### 4.6 Failure handling — AC 3

The entire deferred phase moves into one fallible function, **defined in `main.rs`** (this matters:
`migration_lock.rs:255-261`'s kept `include_str!("../../main.rs")` guard asserts `main.rs` still
contains `migrate_under_lock(` and `config.migration.lock_wait()`, both of which move with it):

```rust
async fn boot_deferred(
    db: &DatabaseConnection,
    config: &IamConfig,
    slot: &BootSlot,
    servers: &mut JoinSet<anyhow::Result<()>>,
    rx: &watch::Receiver<()>,
) -> anyhow::Result<()>       // `?` used freely inside
```

with exactly one caller, which also handles SIGTERM:

```rust
let outcome = tokio::select! {
    r = boot_deferred(&db, &config, &slot, &mut servers, &rx) => r,
    () = shutdown_signal() => { tracing::info!("shutdown signal received during boot"); Ok(()) }
};
if let Err(e) = &outcome { tracing::error!(error = %e, "boot failed after listeners were bound"); }
if outcome.is_err() || shutting_down {
    let _ = tx.send(());
    drain_bounded(&mut servers, DRAIN_TIMEOUT).await;
    return outcome;
}
```

AC 3 then cannot be violated by *adding* a fallible step, because the drain is structural rather
than per-`?`.

**SIGTERM during the deferred phase.** Today that window is also unhandled, but today nothing is
bound and the pod is absent. Afterwards the pod is present-and-unready, so a rolling update or a
`kubectl delete pod` is far more likely to arrive *during* it — and ignoring SIGTERM for
`lock_wait_secs` + migration then taking SIGKILL is exactly the "backend stranded holding the lock"
scenario at `RUNBOOK-containers.md:155-174`. Cancelling `migrate_under_lock` between polls is safe,
and cancelling inside `Migrator::up` rolls the transaction back and releases the xact lock by
construction (`migration_lock.rs:9-10`).

**The drain is bounded.** `main.rs:500-508`'s existing drain has no timeout. Reused unchanged for
the boot-failure path, a task that fails to observe `rx.changed()` — or an axum/tonic graceful
shutdown waiting on a wedged connection — would hang the process with three listening sockets
serving 503 **forever**: CrashLoopBackOff never happens and the replica looks like a slow migration
indefinitely, precisely the state §3.2 rejects. `drain_bounded` wraps it in `tokio::time::timeout`,
returns the boot error regardless, and logs which tasks were still outstanding. Its result is also
bound rather than discarded, so a concurrent listener failure is reported when `boot_deferred`'s
error is absent.

**Panics are explicitly out of scope.** `boot_deferred` returns `Result`, so only `Err` reaches the
drain; a panic unwinds through `#[tokio::main]`, drops the runtime, and aborts in-flight requests on
live sockets. `AppState::new` and axum route registration can panic (`http/mod.rs:963-972` documents
axum panicking at registration time; `migration_lock.rs:143-146` documents a
`DatabaseTransaction::Drop` panic path). Wrapping in `catch_unwind` requires `AssertUnwindSafe`
across a large async body and buys little: the registration-panic class is already covered by
`protected_router_merge_has_no_path_conflicts_in_any_capability_combination`, and a panic is a bug
to fix rather than a state to drain from. Recorded as a known limitation in D4 rather than left
implicit.

**The SMA-471 hoist dissolves.** `main.rs:151-181` constructs the NATS publisher above the first
`servers.spawn` with a comment explaining that an early `?` after the listeners are live would abort
in-flight requests. Under `boot_deferred` that hazard is gone; the publisher moves back to the
outbox block, and the comment is rewritten to record why. Note the NATS connection-gauge sampler
(`main.rs:178`) is itself a `servers.spawn` inside the deferred phase — `drain_bounded` must account
for tasks spawned in both phases.

## 5. Ops surface

### 5.1 Removals and edits — AC 5

| site | change |
|---|---|
| `rs/Dockerfile:76` | `--start-period=180s` → `30s`; comment rewritten to say it covers config load plus a *successful* `Database::connect` and the three binds — not a migration. Note `--interval=30s` already means the first probe fires ~30s in, so an interval is the effective floor regardless |
| `migration_lock.rs:39,43` | delete `IMAGE_START_PERIOD_SECS`, `MIGRATION_BUDGET_SECS` |
| `migration_lock.rs:266-276` | delete `the_default_wait_plus_the_migration_budget_fits_the_image_start_period` |
| `main.rs:12,113-119` | delete the import and the boot warning |
| `config.rs:679-681` | rewrite the `lock_wait_secs` doc comment, dropping the constant references |
| `ci/images/run.sh:119-148` | delete the start-period/lock-wait/budget triple and the `IMAGE_START_PERIOD_SECS` cross-check |
| `ci/images/run.sh:149` | **must be edited in the same change** — the success `echo` interpolates `${start_period}` and `${required}`; under `set -euo pipefail` (line 14) deleting the block without it fails the script |
| `ci/images/run.sh` `assert_pins` | keep a bare floor assertion (`start_period >= 30`) with a comment naming what it now protects — otherwise nothing reads `--start-period` at all and a future edit setting `0s`, or deleting the `HEALTHCHECK` line, goes uncaught |
| `ci/images/run.sh` `smoke()` | add a `wait_ready` helper polling `/readyz` for 200 with its own budget, called between lines 319 and 321 — see below |

**`smoke()` would otherwise red `main`.** `wait_healthy` (`ci/images/run.sh:222-233`) polls only the
container HEALTHCHECK, i.e. `/healthz` (`rs/Dockerfile:76-77`). Line 321 then immediately asserts
`/readyz` == 200. Today `healthy` implies migrated; afterwards `/healthz` answers 200 while
`/readyz` is still 503 `migrating`, against a **fresh** Postgres running m0001–m0008 plus
`reconcile_starter` and `PolicySnapshot::new`. `images.yml` is not a required check, so this reds
`main` after merge rather than the PR — the worst place to find it.

`migration.lock_wait_secs` survives as a pure runtime bound with no static counterpart. Overrunning
it is now a 503 on a live socket, not an invisible replica.

`the_composition_root_still_migrates_under_the_lock` is **kept**; §4.6 places `boot_deferred` in
`main.rs` specifically so all three of its assertions still hold.

### 5.2 `docs/ops/RUNBOOK-containers.md` — AC 6

* **Probe table (~line 92).** The startup row loses its "budget `lock_wait_secs` + the migration +
  `AppState::new`" note; startup now covers process start only.
* **§5 probe budgets (~lines 129-144).** Rewritten: a waiting replica has live sockets, `/healthz`
  answers 200 within a second of start, and `startupProbe` no longer has to be sized against
  `lock_wait_secs`.
* **Gateway-facing note (~line 188).** A migrating IAM answers HTTP `503 {"status":"migrating"}` and
  a well-formed gRPC `UNAVAILABLE` on live sockets rather than refusing; the gateway's existing
  classification already reports not-ready, so no gateway change is required. Add the client-side-LB
  caveat from §4.2.
* **gRPC health asymmetry.** Record that after install, gRPC health stays `SERVING` regardless of
  later DB health while `/readyz` can go 503 — so a `grpc_health_probe` readiness probe is *not*
  equivalent to the HTTP one. This is §7's non-goal 3, and an operator will only find it if it is
  written down.

**`maxSurge` guidance is NOT relaxed.** An earlier draft claimed SMA-571 meets the precondition
`RUNBOOK-containers.md:150-153` records. It does not: that precondition is that `AppState::new`'s
`reconcile_starter` "writes system policies and roles on every boot with no advisory lock of its own
and has never been tested under concurrency" — untouched by this issue. The two documented reasons
`replicas: 1`/`maxSurge: 0` are a requirement rather than a recommendation (RUNBOOK:116-122) are
also untouched: replicas of the *pre-lock* release migrating unguarded, and m0008-class DDL aborting
against a held `AUDIT_PARTITION_LOCK_KEY` (pinned at `tests/migration_lock_pg.rs:233-270`). Neither
is about visibility. This issue relaxes the `start-period`/`startupProbe` budget and nothing else;
the `reconcile_starter` concurrency question stays with SMA-513 unchanged.

## 6. Testing

### 6.1 Docker-free

1. **Empty slot.** `/healthz` → 200; `/readyz` → 503 `{"status":"migrating"}`; an app path
   (`/v1/organizations`) → 503 `{"status":"migrating"}` — asserting explicitly that it is *not* 404
   (fallback missing) and *not* 401 (the real router leaked through).
2. **gRPC fallback shape.** HTTP **200** with `content-type: application/grpc` and `grpc-status: 14`
   — asserted on the headers, not on an HTTP status code. Comment names
   `gateway/adapters/http/mod.rs:150` as the consumer and states why `UNIMPLEMENTED` would be wrong,
   because the two are indistinguishable to a casual reader and only one is correct.
3. **The swap takes effect on an already-built router.** `oneshot` → 503; install; `oneshot` on the
   *same* router value → delegated. This is what proves the router reads the slot per request rather
   than capturing its contents at build time — the failure mode that would make every other test
   here pass while the feature does nothing.
4. **In-flight across the swap.** A handler parked on a `tokio::sync::Barrier`; install while it is
   parked; release. Asserts the request completes against its pre-swap value.
5. **Double install** returns `Err` rather than replacing.
6. **`/readyz` body distinction** — `migrating` / `unready` / `ready` are three distinct bodies.
7. **`drain_bounded`** — synthetic `JoinSet` tasks watching `rx`: asserts it joins them, surfaces the
   first error, and — with a task that deliberately ignores the watch — that it returns at the
   timeout instead of hanging.
8. **Registration parity (D8)** — `grpc::routes(state)` and `grpc::router(state, t)` mount the same
   method set, so the production/test fork cannot reopen.
9. **404 attribution unchanged** (§4.3).

Tests 3–5 may use a stub `Serving` only if a real one is also exercised — see §6.2.

### 6.2 Docker-gated

Following `tests/support/docker.rs` (mandatory — `repo:iam-docker-policy-single-site` fails a
hand-rolled skip).

**(a) Real delegation through the boot router.** The Docker-free tests above use a stub, which would
leave production's actual composition — real `app_routes` under `TraceLayer`/`TimeoutLayer`, real
`AuthEnforce<Routes>` — exercised by nothing, since all 69 existing suites drive `http::router` and
`grpc::router` instead. So: build a real `AppState` via the existing `support` helpers, install a
real `Serving`, and drive an authenticated app route and an authenticated RPC **through the boot
router**.

**(b) The deferred phase end-to-end (AC 1, AC 2).** This one needs the composition root, which lives
in `src/main.rs` and is unreachable from `tests/`. It therefore spawns the built binary as a
subprocess via `env!("CARGO_BIN_EXE_paigasus-iam")`, with `IAM_DATABASE_URL`,
`IAM_AUTHN__ISSUERS` and `IAM_API_KEYS__PEPPER` set as `ci/images/run.sh:314-318` does, on
ephemeral ports. No existing suite uses this pattern, so the spec states it explicitly rather than
leaving two engineers to build two different things; teardown kills the child on drop.

1. Start Postgres; from a second connection take `pg_try_advisory_lock(MIGRATION_LOCK_KEY)` at
   **session** scope — `try`, not the blocking form, because `pg_advisory_lock` returns void and a
   holder that silently failed would make the whole test vacuous (`tests/migration_lock_pg.rs:109-114`
   documents exactly this). Session and transaction advisory locks share one lock space, so this
   deterministically blocks `migrate_under_lock`'s `pg_try_advisory_xact_lock` poll
   (`migration_lock.rs:151`) — already proven by that suite.
2. Boot the subprocess. Assert, while blocked: `/healthz` 200; `/readyz` 503 `migrating`; gRPC health
   `NOT_SERVING`; a `TenancyService` RPC → `UNAVAILABLE`.
3. Release the session lock. Poll until `/readyz` is 200; assert gRPC health `SERVING` and that the
   same unauthenticated RPC now returns **`UNAUTHENTICATED`** — not that it "reaches the real
   service". With `AuthLayer` inside `Serving.grpc` (§4.4), an unauthenticated call is rejected by
   `AuthEnforce` (`grpc/authn.rs:132-134,162-163`); the `UNAVAILABLE → UNAUTHENTICATED` transition is
   itself the proof that delegation happened.

**(c) SIGTERM during the deferred phase** — same harness: hold the lock, send SIGTERM, assert the
process exits 0 promptly rather than after `lock_wait_secs`.

## 7. Non-goals

* **No new metric family for deferred-phase 503s.** They are outside `http_metrics_layer` (§4.3) and
  therefore invisible in `/metrics`. A family costs `names.rs`, `describe_iam_metrics`,
  `RUNBOOK-observability.md` and the `:observability-drift` gate. Recorded as a known gap; the
  cheapest form if wanted later is one counter on the fallback.
* **`/readyz` does not distinguish "waiting for the lock" from "running the migration".** Both are
  `migrating`, so an operator seeing 120s of it must read logs to tell a lock-race loser from a slow
  migration. `migrate_under_lock` already computes `polls`/`waited` (`migration_lock.rs:79-86`) but
  only returns them at the end; surfacing the live phase needs a new observer parameter through its
  API. Deliberately deferred — AC 1 asks only that `migrating` be distinguishable from a failed DB
  ping, which it is.
* **`paigasus-gateway` is unchanged.** It does not migrate, and its classification already reads
  `UNAVAILABLE` as not-ready (§4.2). The client-side-LB caveat is documented, not coded.
* **gRPC health does not become fully dynamic.** Once installed it stays `SERVING` regardless of
  later DB health; tracking `/readyz` is the M1 concern `grpc/mod.rs:53-58` already names. Documented
  in the RUNBOOK (§5.2) so the asymmetry is discoverable.
* **No migration retry** (§3.2). **Panics are not drained** (§4.6).

## 8. Known costs

* No new dependency (`OnceLock` replaces the `arc-swap` an earlier draft proposed), so
  `repo:affected-smoke` needs no re-baseline and `rs/deny.toml` needs no waiver.
* `paigasus-iam`'s `moon.yml` `fileGroups.upstreams` is unaffected — no new in-tree dependency.
* `rs/Dockerfile` and `ci/images/run.sh` are touched, so `images.yml`'s `pull_request` filter runs
  the image build on this PR automatically. Given §5.1's `smoke()` change, that is wanted.
* `serve_http`'s signature changes (one caller) and `grpc::routes` is new public API on the crate;
  `grpc::router`'s signature and behaviour are unchanged.
