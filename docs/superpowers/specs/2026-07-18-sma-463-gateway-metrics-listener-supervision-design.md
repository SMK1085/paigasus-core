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
     spawn: `axum::serve(listener, app).with_graceful_shutdown(rx.changed())`.
  2. **Metrics listener** — only when metrics enabled **and** `metrics.addr` is set.
     Binds **inside** the task, so a bind *or* serve failure becomes a surfaced task
     error. This is the crux of the fix.
  3. **Upkeep loop** — only when metrics enabled. `select!` on interval-tick vs
     `rx.changed()`.
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

## Behavior preserved / changed

- **Preserved:** SIGINT/SIGTERM still triggers graceful shutdown of all tasks (now via
  the watch broadcast). Main-port bind failure still aborts startup fast with a
  top-level error. Same-port `/metrics` behavior unchanged. `/healthz` and `/readyz`
  semantics unchanged.
- **Changed:** a metrics-listener or upkeep failure now **propagates** — graceful
  shutdown of the remaining servers, `main` returns `Err`, non-zero process exit,
  orchestrator restarts. No more silent loss of the endpoint.

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

Plus: the existing 80 gateway tests stay green; `cargo build` / `clippy -D warnings` /
`fmt --check` clean; and the full `moon ci` gate graph runs before pushing
(`:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
:parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`).

## Scope / non-goals

- **In scope:** `src/main.rs` (the `metrics_handle` / `config.metrics.addr` block +
  server wiring), new `src/runtime.rs`, `src/lib.rs` (`pub mod runtime;`). SPDX header
  on the new file.
- **Out of scope:** no restart-with-backoff loop; no readiness-flag state; no config
  changes; no change to `/healthz` / `/readyz` semantics; no changes to IAM.
