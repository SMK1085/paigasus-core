# SMA-463 — Gateway: supervise the dedicated `/metrics` listener

- **Linear:** [SMA-463](https://linear.app/smaschek/issue/SMA-463/gateway-supervise-the-dedicated-metrics-listener-restart-health-gate)
  (follow-up from SMA-446 #3, PR #89)
- **Scope crate:** `rs/crates/services/paigasus-gateway`
- **Priority:** Low; non-critical (scrape endpoint, M0)
- **Status:** design approved 2026-07-18

## Problem

In `paigasus-gateway/src/main.rs`, two long-lived tasks are **detached**
`tokio::spawn`s that `main()` never observes:

1. The Prometheus **upkeep** loop (`PrometheusHandle::run_upkeep()` on a 5s interval).
2. When `[metrics].addr` is configured, the **separate `/metrics` listener**.

The metrics listener runs under graceful shutdown and, on an `axum::serve` failure,
only logs at `tracing::error!` before its task dies. The main HTTP server — the sole
future `main()` actually awaits — keeps running. Net effect: an unexpected serve
failure **silently loses the configured observability endpoint** while the gateway
continues to serve traffic, with no signal to operators. The detached upkeep task has
the same fire-and-forget defect (a panic there goes unnoticed and histogram memory
then grows unbounded).

IAM (`paigasus-iam/src/main.rs`) already solved this: every long-lived task spawns into
a shared `JoinSet<anyhow::Result<()>>` on a common `watch` shutdown channel, and `main`
`select!`s on "shutdown signal *or* the first server task ending", surfacing any task
error and broadcasting graceful shutdown to the rest.

## Decisions

- **Supervision posture: align on IAM's shared `JoinSet` and _propagate_.** A metrics
  (or upkeep) failure triggers graceful shutdown of the remaining servers and a
  non-zero process exit, so the orchestrator restarts the pod. Chosen over
  restart-with-backoff (serve failures on an already-bound socket are rarely transient;
  YAGNI for a non-critical endpoint; diverges from IAM) and over a readiness-gate
  (would add mutable readiness state to `AppState` and conflate "IAM reachable"
  readiness with "metrics up" readiness). This is the issue's explicit steer and the
  smallest change that keeps one mental model across the two services.
- **Testability: extract a small, dependency-free supervision helper and unit-test it.**
  `main()` is a composition root and hard to test directly; the select/broadcast/drain
  logic is the part worth exercising, and it has no dependency on IAM/OpenAI/config.

## Target architecture

Refactor the composition root to IAM's shape:

- A `tokio::sync::watch::channel(())` → `(tx, rx)` carries graceful shutdown to every
  task.
- A `JoinSet<anyhow::Result<()>>` named `servers` holds **all** long-lived tasks. Each
  spawned task waits on its own `rx.clone()` via `rx.changed()`; tasks no longer call
  `shutdown_signal()` themselves (only the top-level supervisor listens on the OS
  signal):
  1. **Main HTTP server** — the `TcpListener` is bound up-front in `main()` (preserves
     fail-fast on the main port and the existing "started" log), then moved into the
     spawn. Each task takes its own `let mut rx = rx.clone();` and uses the wrapped
     future — `axum::serve(listener, app).with_graceful_shutdown(async move { let _ =
     rx.changed().await; })` — because `rx.changed()` yields `Result<(), RecvError>`
     and borrows `rx` mutably, so it can't be passed directly (mirrors IAM
     `main.rs:84-86`).
  2. **Metrics listener** — only when metrics enabled **and** `metrics.addr` is set.
     Binds **inside** the task, so a bind *or* serve failure becomes a surfaced task
     error. This is the crux of the fix. Keep the in-task
     `tracing::info!(%metrics_addr, "paigasus-gateway metrics listener started")` log
     (mirrors IAM `main.rs:101`); the old
     `"paigasus-gateway metrics listener exited"` error line is intentionally
     superseded by supervise's generic "a server task failed" + the non-zero-exit
     restart signal.
  3. **Upkeep loop** — only when metrics enabled. `select!` on interval-tick vs
     `rx.changed()`.

  **Invariant:** every `rx.clone()` is made *before* `tx.send(())` (which only happens
  inside `supervise`). A `watch::Receiver` cloned before the first send has version
  equal to the shared version, so its first `changed().await` *waits* rather than
  resolving immediately — the design relies on this (verified against IAM, which boots
  correctly on the same property).

### Build change (required)

`runtime::supervise` and the spawned tasks use `tokio::sync::watch`, but
`paigasus-gateway/Cargo.toml` currently declares `tokio` features
`["rt-multi-thread", "macros", "net", "signal"]` — no `sync`, and no `time` (already
used undeclared by `tokio::time::interval` in the upkeep loop; compiles today only via
cross-crate feature unification, which Moon's per-crate `cargo build` does not
guarantee). Add **`"time"` and `"sync"`** so the feature set matches IAM's and the
`rs/Cargo.toml` documented services posture. Verify with `cargo build -p
paigasus-gateway` (single-crate), not only `--workspace`.
- `main()` ends by calling the new helper:
  `runtime::supervise(servers, shutdown_signal(), tx).await`.

The same-port `/metrics` merge (when `metrics.addr` is **not** set) is unchanged — it
rides on the main router and therefore on the main HTTP server task.

### The extracted, tested unit — `paigasus_gateway::runtime::supervise`

New lib module `src/runtime.rs` (declared `pub mod runtime;` in `lib.rs`):

```rust
pub async fn supervise(
    mut servers: JoinSet<anyhow::Result<()>>,
    shutdown: impl Future<Output = ()>,
    tx: tokio::sync::watch::Sender<()>,
) -> anyhow::Result<()>
```

Behavior mirrors IAM's `main.rs` supervision block:

1. `tokio::select!` on `shutdown` (→ `None` early-error) vs `servers.join_next()`:
   - `Ok(Ok(()))` → warn "a server task exited before shutdown was requested", `None`.
   - `Ok(Err(e))` → error!, `Some(e)`.
   - `Err(join)` → error! "a server task panicked", `Some(join.into())`.
2. `tx.send(())` broadcasts graceful shutdown to the remaining tasks.
3. Drain remaining tasks (`while let Some(joined) = servers.join_next().await`),
   surfacing the first error; return the aggregated `Result`.

Dependency-free (only `JoinSet` / `watch` / `anyhow` / `Future`), so it unit-tests
without any IAM/OpenAI/config wiring.

**Documented caller invariant:** callers must pass either a non-empty `servers` set or
a `shutdown` future that resolves — an empty `JoinSet` with a `pending()` shutdown would
disable both `select!` arms and wait forever. The gateway always spawns the main HTTP
task, so it never hits this; the note guards the `pub` helper against future callers.

## Behavior preserved / changed

- **Preserved (must survive the rewrite):**
  - The Prometheus **recorder install** (`config.metrics.enabled.then(|| ...init...)`)
    and the **`describe_gateway_metrics()`** call — these register the `# HELP`/`# TYPE`
    exposition and must not be dropped when the `metrics_handle` block is rewritten (no
    existing test covers them — the metrics tests build the router directly, never
    `main`).
  - The **same-port `/metrics` merge** (`app.merge(metrics_router(...))` when
    `metrics.addr` is *not* set) — unchanged; it rides the main HTTP task.
  - **SIGINT/SIGTERM** still triggers graceful shutdown of all tasks (now via the watch
    broadcast). Main-**port** bind failure still aborts startup fast with a top-level
    error. `/healthz` and `/readyz` semantics unchanged.
- **Changed:** a metrics-listener or upkeep failure now **propagates** — graceful
  shutdown of the remaining servers, `main` returns `Err`, non-zero process exit,
  orchestrator restarts. No more silent loss of the endpoint.
- **Minor behavioral nuance (accepted):** a *metrics-port* bind failure previously
  aborted via `?` before the main server served anything; now the main listener is
  bound and spawned first, so a metrics bind failure surfaces as a task error that
  briefly lets the main server serve before supervise tears everything down. This
  tiny serve-then-shutdown window is acceptable (the endpoint is being torn down and
  restarted regardless).

## Testing

`runtime::supervise` unit tests (`#[tokio::test]`):

1. **`early_error_is_surfaced_and_triggers_shutdown`** — one task returns `Err`
   immediately while a peer waits on the watch; `shutdown = std::future::pending()`.
   Assert the aggregated result is that `Err`, and the peer was told to stop (it drains
   to completion rather than hanging).
2. **`clean_shutdown_drains_all_to_ok`** — all tasks wait on the watch; `shutdown` is
   ready immediately → `tx` broadcast → all tasks end `Ok` → aggregated result `Ok`.
3. **(bonus) `early_clean_return_warns_not_errors`** — a task returns `Ok(())` before
   shutdown → warn branch, aggregated result still `Ok`.
4. **`error_surfaced_even_when_shutdown_wins_the_select`** — a task returns `Err`
   immediately **and** `shutdown` is ready (`std::future::ready(())`). Whichever arm
   `select!` picks, the aggregated result must still be that `Err` (proves the drain
   loop, not just the select branch, is what surfaces the error). Deterministic
   regardless of `select!` fairness — this locks in the subtle "drain catches it"
   guarantee against future refactors.

Plus: the existing 80 gateway tests stay green; `cargo build` / `clippy -D warnings` /
`fmt --check` clean; and the full `moon ci` gate graph runs before pushing
(`:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
:parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`).

## Scope / non-goals

- **In scope:** `src/main.rs` (the `metrics_handle` / `config.metrics.addr` block +
  server wiring), new `src/runtime.rs`, `src/lib.rs` (`pub mod runtime;`),
  `Cargo.toml` (add tokio `"time"`/`"sync"` features — see Build change above). SPDX
  header on the new file.
- **Out of scope:** no restart-with-backoff loop; no readiness-flag state; no
  **runtime** config changes (`gateway.toml` / `GatewayConfig` untouched — the only
  manifest change is the tokio features above); no change to `/healthz` / `/readyz`
  semantics; no changes to IAM.
- **Acknowledged pre-existing (not addressed here):** the shutdown drain has no
  timeout — a long-lived streaming `/v1/chat/completions` response under axum graceful
  shutdown can delay process exit. This matches IAM's behavior and predates this change;
  bounding it is not part of SMA-463.
