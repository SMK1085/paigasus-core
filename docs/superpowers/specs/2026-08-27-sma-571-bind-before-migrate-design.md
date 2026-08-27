# SMA-571 — Bind and ready-gate the listeners before migrating

**Issue:** [SMA-571](https://linear.app/smaschek/issue/SMA-571/) · **Status:** draft, pre-challenge
**Related:** [SMA-559](https://linear.app/smaschek/issue/SMA-559/) (advisory lock — this was split out
of it, and this issue retires the coupling SMA-559 could only document), SMA-500 (container images,
owns `rs/Dockerfile` and `ci/images/run.sh`), SMA-513 (Helm chart, consumes the relaxed `maxSurge`
precondition)

## 1. Problem

`paigasus-iam`'s composition root migrates before it binds anything. In `main.rs` the order is

```
IamConfig::load → validate → paigasus_logging::init → metrics recorder
  → Database::connect
  → migrate_under_lock                    (line 120)
  → AppState::new                         (line 138)
  → servers.spawn(serve_http / grpc::serve)   (lines 190+)
```

so for the whole migration window the process holds no socket. To an orchestrator the replica is not
*unready*, it is **absent** — indistinguishable from a pod that has not started. The only lever is
probe tuning.

SMA-559 made this worse rather than better. Its advisory lock is correct, but a replica that loses
the lock race now legitimately *waits* for up to `migration.lock_wait_secs` (default 120s) with
nothing bound. SMA-559 had to compensate by coupling a static Dockerfile value to a runtime config
value:

* `rs/Dockerfile` — `HEALTHCHECK --start-period=180s`
* `migration_lock.rs` — `IMAGE_START_PERIOD_SECS = 180`, `MIGRATION_BUDGET_SECS = 60`
* `main.rs:113-119` — a boot warning when `lock_wait_secs + MIGRATION_BUDGET_SECS > IMAGE_START_PERIOD_SECS`
* `migration_lock.rs` — a unit test asserting the default fits
* `ci/images/run.sh` `assert_pins` — a triple check tying all three together
* `RUNBOOK-containers.md` §5 — a startupProbe budget an operator has to compute by hand

That is six coupled sites expressing one fact — "a migrating replica answers nothing" — that this
issue removes.

## 2. Why this is not a reordering

`AppState::new` cannot run before the migration:

* `adapters/http/mod.rs:409` — `bootstrap::reconcile_starter` reconciles system policies **into**
  Postgres.
* `adapters/http/mod.rs:412-416` — `PolicySnapshot::new` → `load_and_compile` **reads** the policy
  store.

Both hit tables migration m0004 creates, so on a fresh database they fail with `relation does not
exist`. The listener must be *serving* before the state it would normally carry exists. That is the
whole difficulty, and it is why three obvious approaches were rejected on inspection during SMA-559's
review:

* **Bind early, `axum::serve` later.** `listen()` succeeds so TCP connects, but nothing accepts. The
  probe hangs and times out — strictly worse than connection-refused, which at least fails fast.
* **Serve a boot router, then hand the listener to a second `axum::serve`.** A port-free race window
  across the handoff, dropping probes.
* **`OnceCell<AppState>`.** Changes the signature of every HTTP handler and every gRPC service.

## 3. Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | **Both** HTTP and gRPC bind early — symmetric, no documented asymmetry | see §4.2; an asymmetry here is not merely untidy, it is a correctness hazard for the gateway |
| D2 | Bind **after** `Database::connect`, before `migrate_under_lock` | covers exactly the window the issue names; a bad `database_url` still crash-loops with nothing bound |
| D3 | A post-bind failure drains gracefully, then exits **non-zero** | CrashLoopBackOff stays the signal for a broken replica; AC 3's drain is satisfied without hiding failures |
| D4 | The `--start-period` ↔ `lock_wait_secs` coupling is **deleted**, not shrunk | a dead invariant that nobody can re-derive is the exact hazard the issue names |
| D5 | One `ArcSwapOption` slot holding **one struct** carrying router *and* state | makes AC 4's no-window property structural rather than a test obligation |
| D6 | gRPC's deferred fallback answers `UNAVAILABLE`, never `UNIMPLEMENTED` | the gateway's readiness classification depends on the distinction — see §4.2 |

### 3.1 D2 in detail — why not bind before `Database::connect`

Binding even earlier is tempting: a replica whose Postgres is unreachable would then answer
`/healthz` 200 and `/readyz` 503 instead of crash-looping, and a transient DB outage during a rolling
update would self-heal without restarts.

Rejected because it trades a loud failure for a quiet one. sea-orm's `Database::connect` pings
eagerly, so today a misconfigured `database_url` fails fast and visibly. Under an earlier bind it
would instead sit permanently NotReady — which looks exactly like "still migrating" and is much
easier to miss. Restart-based recovery is also lost. The migration window is the problem this issue
exists to solve; the connect window is a different tradeoff and does not need to ride along.

### 3.2 D3 in detail — why not stay up serving 503

Two alternatives were considered and rejected:

* **Stay up, 503 forever.** The orchestrator sees a steady NotReady pod rather than
  CrashLoopBackOff. But a permanently broken replica then looks identical to a slow one, and
  `kubectl get pods` loses the ability to distinguish "still migrating" from "migration failed".
* **Retry the migration in a loop.** Self-heals a transient blip mid-migration, but turns a
  deterministic migration bug into an infinite retry that never surfaces, and is materially larger
  than the issue.

## 4. Design

### 4.1 The slot

A new module, `adapters::boot`:

```rust
/// Everything that only exists once the migration and `AppState::new` have completed.
/// Router and state are ONE value so they cannot be installed separately.
pub struct Serving {
    http:  axum::Router,           // app_routes + TraceLayer + TimeoutLayer
    grpc:  tonic::service::Routes, // real services + AuthLayer
    state: AppState,               // for /readyz's DB ping
}

#[derive(Clone)]
pub struct BootSlot(Arc<ArcSwapOption<Serving>>);

impl BootSlot {
    pub fn empty() -> Self;
    pub fn install(&self, serving: Serving);       // the ONLY way to become ready
    fn load(&self) -> Option<Arc<Serving>>;
}
```

Boot proceeds in three phases:

| phase | bound | `/healthz` | `/readyz` | gRPC health | gRPC RPCs |
|---|---|---|---|---|---|
| **pre-bind** — config, validate, logging, metrics, `Database::connect` | nothing | conn refused | conn refused | conn refused | conn refused |
| **deferred** — `migrate_under_lock`, `AppState::new`, publisher dial | both ports, slot `None` | `200 {"status":"ok"}` | `503 {"status":"migrating"}` | `NOT_SERVING` | `UNAVAILABLE` |
| **serving** — after `install` | both ports, slot `Some` | `200` | `200 {"status":"ready"}` / `503 {"status":"unready"}` | `SERVING` | real |

`/readyz`'s three bodies are what AC 1 asks for: `migrating` (schema not yet applied) is distinct
from `unready` (the DB ping failed) is distinct from `ready`.

### 4.2 gRPC — why symmetry is a correctness requirement, not a nicety

`paigasus-gateway`'s `/readyz` infers IAM reachability from a sentinel `IntrospectApiKey`
(`gateway/adapters/http/mod.rs:140-151`) and classifies:

```rust
Err(IamError::Connect(_))                                            => not ready,
Err(IamError::Rpc(s)) if Unavailable | DeadlineExceeded | Internal   => not ready,
Err(IamError::Rpc(_)) | Ok(_)                                        => READY,
```

`UNIMPLEMENTED` falls in the last arm. So a gRPC port that binds early serving *only* health would
make the gateway report **ready** against a migrating IAM — violating AC 2 while looking like an
improvement. Hence D6: the deferred gRPC fallback must return `Status::unavailable`, which the
gateway already classifies correctly and which is also gRPC's own semantic for "not ready, retry".

This also rules out the "HTTP only, document the asymmetry" option in the weak form. Leaving gRPC to
bind late is *safe* (connection-refused → `Connect` → not ready), but it leaves the orchestrator
seeing a half-absent replica, and the RUNBOOK explaining why one port answers and the other refuses.

### 4.3 HTTP wiring

```rust
pub fn boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router
```

owns `/healthz`, `/readyz` and `/metrics` **permanently**, with a `fallback_service` that reads the
slot. `Serving.http` is exactly today's `traced` value in `serve_http` —
`app_routes(state).layer(TraceLayer).layer(TimeoutLayer)` — and deliberately carries no health
routes.

This preserves the layering invariant `health_router`/`readyz_router` document today: `/healthz`,
`/readyz` and `/metrics` stay outside `TraceLayer`, `TimeoutLayer` and `http_metrics_layer`, so a
15s Prometheus scrape or a probe poll still emits no request-span trace and does not inflate RED
metrics.

`/metrics` being live during the deferred phase is a deliberate bonus: a replica stuck waiting on the
migration lock becomes scrapeable, where today it is invisible.

`serve_http` has exactly one caller (`main.rs`), so its signature can change freely. `pub fn
router(state)` — the `oneshot` test harness entry point used across the integration suite — is
**unchanged**.

### 4.4 gRPC wiring

`tonic::service::Routes` is an `axum::Router` underneath (`tonic-0.14.6/src/service/router.rs:14`,
`into_axum_router` at :106, `axum_router_mut` at :109), and `Server::serve_with_shutdown(addr, svc,
signal)` is public on `impl<L> Server<L>` (`transport/server/mod.rs:678`) and accepts an arbitrary
service. So:

```rust
Server::builder()
    .timeout(request_timeout)
    .layer(CorrelationLayer)
    .serve_with_shutdown(addr, boot_grpc_routes(slot, health).prepare(), signal)
```

keeps tonic's own h2 configuration, `GrpcTimeout`, `RecoverError`, `LoadShed` and
`ConcurrencyLimit` intact — nothing is reimplemented.

`boot_grpc_routes` is a `Routes` carrying the health service plus a `fallback_service` returning
`Status::unavailable("migrating")` while the slot is empty, delegating to `Serving.grpc` once
installed.

**`AuthLayer` moves.** `AuthLayer::new(state)` (`grpc/authn.rs:91`) needs `AppState`, so it cannot
sit on the boot-time server stack. It moves onto `Serving.grpc`. Behaviour is unchanged: health and
`Introspect`/`IntrospectApiKey` are already `:path`-exempt from it, and every non-exempt RPC lives
inside `Serving.grpc`. `CorrelationLayer` stays on the server so it remains outermost among our
layers, exactly as today.

**`grpc::router(state, timeout)` is kept as-is.** Eleven Docker-gated integration suites
(`grpc_tenancy`, `grpc_authn`, `grpc_authz`, `grpc_audit`, `grpc_users`, `grpc_dead_letters`,
`grpc_service_info`, `grpc_system_retirement`, `api_keys_grpc`, `authz_acceptance`,
`authz_bootstrap_admin`) build it directly. The deferred path is additive.

**The health reporter is kept.** `health_service()` currently builds a `HealthReporter` and drops it,
with a comment deferring dynamic readiness to M1. We retain it, set `NOT_SERVING` at bind, and flip
to `SERVING` inside `install`. This closes the M1 note for the migration case only — gRPC health does
not otherwise track `/readyz` (see §7).

### 4.5 The swap — AC 4

Because `Serving` bundles router and state, **there is no API that installs one without the other**.
AC 4's hardest clause — "no window exists where the real router is live but `AppState` is not" — is
therefore a property of the type, not of an ordering that a future edit could get wrong. The
corresponding test asserts a guarantee the compiler already enforces, which is the correct direction.

`ArcSwapOption::load` is taken **once per request at dispatch**. A request in flight across an
`install` completes against the value it started with: no torn state, no mid-request upgrade. That is
the defined answer to AC 4's third clause.

### 4.6 Failure handling — AC 3

Today a post-`Database::connect` `?` is safe *because nothing is bound*. Once listeners are live,
every fallible step between bind and install must drain instead. Rather than converting each `?` by
hand — which a future contributor would eventually forget — the entire deferred phase moves into one
fallible function:

```rust
async fn boot_deferred(
    db: &DatabaseConnection,
    config: &IamConfig,
    slot: &BootSlot,
    servers: &mut JoinSet<anyhow::Result<()>>,
    rx: &watch::Receiver<()>,
) -> anyhow::Result<()>       // `?` used freely inside
```

with exactly one caller:

```rust
if let Err(e) = boot_deferred(&db, &config, &slot, &mut servers, &rx).await {
    let _ = tx.send(());
    drain(&mut servers).await;   // the same drain the shutdown path uses
    return Err(e);
}
```

AC 3 then cannot be violated by *adding* a fallible step, because the drain is structural rather than
per-`?`. The process still exits non-zero (D3).

`drain` is extracted from the existing tail of `serve()` so the shutdown path and the boot-failure
path share one implementation, and so it is unit-testable without a runtime full of real servers.

**The SMA-471 hoist dissolves.** `main.rs:151-181` currently constructs the NATS publisher *above*
the first `servers.spawn` with a comment explaining that an early `?` after the listeners are live
would abort in-flight requests. Under `boot_deferred` that hazard is gone; the publisher moves back
to the outbox block where it naturally belongs, and the comment is rewritten to record why the hoist
is no longer needed.

## 5. Ops surface

### 5.1 Removals — AC 5

| site | change |
|---|---|
| `rs/Dockerfile` | `--start-period=180s` → `30s`; comment rewritten to say it covers config load + `Database::connect` against a cold Postgres, not a migration |
| `migration_lock.rs` | delete `IMAGE_START_PERIOD_SECS`, `MIGRATION_BUDGET_SECS` |
| `migration_lock.rs` | delete `the_default_wait_plus_the_migration_budget_fits_the_image_start_period` |
| `main.rs:113-119` | delete the boot warning |
| `config.rs:681` | rewrite the `lock_wait_secs` doc comment, dropping the constant references |
| `ci/images/run.sh` `assert_pins` | delete the start-period/lock-wait/budget triple and the `IMAGE_START_PERIOD_SECS` cross-check; keep the rustc-channel, bookworm-builder, ubuntu-vs-chisel and no-baked-config pins |

`migration.lock_wait_secs` survives as a pure runtime bound with no static counterpart —
overrunning it is now a 503 on a live socket, not an invisible replica.

`the_composition_root_still_migrates_under_the_lock` is **kept**: all three of its `include_str!`
assertions (`migrate_under_lock(` present, `Migrator::up` absent, `config.migration.lock_wait()`
passed) still hold after the restructure.

### 5.2 `docs/ops/RUNBOOK-containers.md` — AC 6

* **Probe table (line ~92).** The startup row loses its "budget `lock_wait_secs` + the migration +
  `AppState::new`" note; startup now covers process start only.
* **§5 probe budgets (lines ~129-144).** Rewritten: a waiting replica has a live socket, `/healthz`
  answers 200 within a second of start, and `startupProbe` no longer has to be sized against
  `lock_wait_secs`.
* **Gateway-facing note (§ near line 188).** A migrating IAM answers HTTP `503 {"status":
  "migrating"}` and gRPC `UNAVAILABLE` on a live socket rather than refusing the connection; the
  gateway's existing `Connect`/`Unavailable`/`DeadlineExceeded`/`Internal` classification already
  reports not-ready for this, so no gateway change is required.
* **`maxSurge` precondition (line ~149).** The precondition SMA-559 recorded is now met: relax the
  `replicas: 1` / `maxSurge: 0` guidance to a recommendation, since a surging replica that loses the
  lock race is now visibly unready rather than absent. This is the operational payoff.

## 6. Testing

### 6.1 Docker-free

1. **Empty slot.** `/healthz` → 200; `/readyz` → 503 `{"status":"migrating"}`; an app path
   (`/v1/organizations`) → 503 `{"status":"migrating"}` — asserting explicitly that it is *not* 404
   (fallback missing) and *not* 401 (bearer layer reached, i.e. the real router leaked through).
2. **The swap takes effect on an already-built router.** `oneshot` → 503; `install(stub)`; `oneshot`
   on the *same* router value → delegated. This is what proves the router reads the slot per request
   rather than capturing its contents at build time — the failure mode that would make every other
   test here pass while the feature does nothing.
3. **In-flight across the swap.** A stub handler parked on a `tokio::sync::Barrier`; `install` while
   it is parked; release. Asserts the request completes against its pre-swap value.
4. **gRPC fallback code.** Returns `UNAVAILABLE`, not `UNIMPLEMENTED` — pinned as its own test with
   a comment naming `gateway/adapters/http/mod.rs:148` as the consumer, because the two codes are
   indistinguishable to a casual reader and only one is correct.
5. **`/readyz` body distinction.** `migrating` vs `unready` vs `ready` are three distinct bodies.
6. **Drain helper.** Synthetic `JoinSet` tasks watching `rx`; assert `tx.send(())` + `drain` joins
   all of them and surfaces the first error.

### 6.2 Docker-gated — the real AC 1 / AC 2 proof

A new suite following `tests/support/docker.rs` (mandatory — `repo:iam-docker-policy-single-site`
fails a hand-rolled skip):

1. Start Postgres; open a second connection and take `pg_advisory_lock(MIGRATION_LOCK_KEY)` at
   **session** scope. Session and transaction advisory locks share one lock space, so this
   deterministically blocks `migrate_under_lock`'s `pg_try_advisory_xact_lock` poll
   (`migration_lock.rs:151`).
2. Boot the service. Assert, while blocked: `/healthz` 200; `/readyz` 503 `migrating`; gRPC health
   `NOT_SERVING`; a `TenancyService` RPC → `UNAVAILABLE`.
3. Release the session lock. Poll until `/readyz` is 200; assert gRPC health `SERVING` and the same
   RPC now reaches the real service.

This is the only test that exercises bind → wait → migrate → install end to end, and it is the one
that would catch a regression re-ordering the bind back behind the migration.

## 7. Non-goals

Stated explicitly so they are reviewed rather than silently omitted.

* **No new metric family for migrating-503s.** The window is bounded and already logged, and a new
  family costs `paigasus-observability`'s `names.rs`, `describe_iam_metrics`,
  `RUNBOOK-observability.md` and the `:observability-drift` gate. If the challenger disagrees, the
  cheapest form is a single counter on the fallback.
* **`paigasus-gateway` is unchanged.** It does not migrate, and its existing classification already
  reads `UNAVAILABLE` as not-ready (§4.2).
* **gRPC health does not become fully dynamic.** Once installed it stays `SERVING` regardless of
  later DB health. Tracking `/readyz` is the M1 concern `grpc/mod.rs` already names; this issue
  closes only the migration case.
* **No migration retry.** §3.2.

## 8. Known costs

* `arc-swap` is promoted from a transitive `Cargo.lock` entry (1.9.2) to a declared workspace
  dependency. This re-baselines `repo:affected-smoke`'s `lockfile->all-lint` expected set in
  `ci/affected-graph/run.sh` and wants a `rs/deny.toml` licence check — arc-swap is MIT/Apache-2.0,
  so that should be a no-op.
* `paigasus-iam`'s `moon.yml` `fileGroups.upstreams` is unaffected (no new in-tree dependency).
* `rs/Dockerfile` and `ci/images/run.sh` are touched, so `images.yml`'s `pull_request` filter will
  run the image build on this PR automatically.
