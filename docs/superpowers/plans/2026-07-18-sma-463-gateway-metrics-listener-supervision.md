# Gateway `/metrics` Listener Supervision Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the gateway's dedicated `/metrics` listener (and the Prometheus upkeep loop) failures observable and recoverable instead of silently swallowed, by aligning the composition root on IAM's shared-`JoinSet` supervision.

**Architecture:** Extract a dependency-free `paigasus_gateway::runtime::supervise(servers, shutdown, tx)` helper (mirrors IAM's `main.rs` select→broadcast→drain), unit-test it, then refactor `paigasus-gateway/src/main.rs` so the main HTTP server, the separate metrics listener, and the upkeep loop all spawn into one `JoinSet<anyhow::Result<()>>` on a shared `tokio::sync::watch` shutdown channel. A metrics/upkeep failure now propagates → graceful shutdown of the rest → non-zero exit → orchestrator restart.

**Tech Stack:** Rust (edition 2024, rust 1.95), tokio (`JoinSet` + `watch`), axum `with_graceful_shutdown`, anyhow, tracing, `cargo nextest`, Moon.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates use **edition 2024 + rust-version 1.95**.
- `cargo clippy --workspace -- -D warnings` must be clean; `cargo fmt --check` must pass.
- `cargo nextest` exits non-zero on no-tests — use `--no-tests=pass` where relevant.
- Bash tool PATH lacks proto CLIs — prefix commands with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All work happens in the worktree:
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-463-gateway-metrics-supervise`
  (run cargo from its `rs/` subdir).
- Do NOT bypass the commit-msg hook with `--no-verify`. Commit subjects start lowercase,
  ≤100 chars; do not put `#NNN` in the commit body.
- Scope is confined to `paigasus-gateway`; do not touch `paigasus-iam`, config, or
  `/healthz`/`/readyz` semantics.

---

### Task 1: `runtime::supervise` helper + unit tests

Extract the select/broadcast/drain supervision primitive into a new, dependency-free lib
module and unit-test the four behaviors that matter. This task also makes the required
`tokio` feature change the module depends on.

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml:28` (add tokio `"time"`, `"sync"`)
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs` (add `pub mod runtime;`)
- Create: `rs/crates/services/paigasus-gateway/src/runtime.rs` (helper + `#[cfg(test)]` tests)

**Interfaces:**
- Produces: `pub async fn supervise(servers: tokio::task::JoinSet<anyhow::Result<()>>, shutdown: impl std::future::Future<Output = ()>, tx: tokio::sync::watch::Sender<()>) -> anyhow::Result<()>`
  — consumed by `main.rs` in Task 2.

- [ ] **Step 1: Add the required tokio features**

Edit `rs/crates/services/paigasus-gateway/Cargo.toml` line 28. Change:

```toml
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "signal"] }
```

to:

```toml
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "signal", "time", "sync"] }
```

(`sync` is needed by the new `watch` channel; `time` is already used by
`tokio::time::interval` in the upkeep loop and compiles today only via cross-crate
feature unification, which Moon's per-crate build does not guarantee. This matches IAM's
tokio features and the `rs/Cargo.toml` documented services posture.)

- [ ] **Step 2: Register the module in lib.rs**

Edit `rs/crates/services/paigasus-gateway/src/lib.rs`. After the existing `pub mod domain;`
line, add:

```rust
pub mod runtime;
```

- [ ] **Step 3: Create `src/runtime.rs` with the four failing tests and a stub body**

Create `rs/crates/services/paigasus-gateway/src/runtime.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Server-task supervision for the `paigasus-gateway` composition root.
//!
//! [`supervise`] runs a set of long-lived server tasks on a shared graceful-shutdown
//! [`watch`] channel: it waits for the first of a shutdown signal or any task ending,
//! then broadcasts graceful shutdown to the rest and drains them, surfacing the first
//! error. This turns a metrics-listener (or upkeep) failure — previously a detached
//! task that only logged before dying — into an error that propagates out of `main`
//! (SMA-463). Mirrors `paigasus-iam`'s composition-root supervision so both services
//! share one model.

use std::future::Future;

use tokio::sync::watch;
use tokio::task::JoinSet;

/// Supervise a set of server tasks on a shared graceful-shutdown watch.
///
/// Waits for the first of: `shutdown` resolving (an OS signal), or any task in
/// `servers` ending (cleanly, with an error, or by panic). Then broadcasts graceful
/// shutdown via `tx` and drains the remaining tasks, returning the first error
/// observed. A clean early task return is logged (warn) but is not an error.
///
/// # Invariants
/// - Every task in `servers` must observe shutdown through a [`watch::Receiver`] cloned
///   from the same channel as `tx` **before** this function is called, so `tx.send`
///   reaches it. Receivers cloned before the first send do not wake spuriously, so the
///   first `changed().await` correctly waits.
/// - Callers must pass either a non-empty `servers` or a `shutdown` that resolves; an
///   empty set with a non-resolving `shutdown` would disable both `select!` arms and
///   wait forever. The gateway always spawns its main HTTP task, so it never hits this.
pub async fn supervise(
    mut servers: JoinSet<anyhow::Result<()>>,
    shutdown: impl Future<Output = ()>,
    tx: watch::Sender<()>,
) -> anyhow::Result<()> {
    todo!("implemented in Step 5")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    /// Spawn a task that returns `Ok(())` only once the shutdown broadcast fires.
    fn spawn_until_shutdown(servers: &mut JoinSet<anyhow::Result<()>>, mut rx: watch::Receiver<()>) {
        servers.spawn(async move {
            let _ = rx.changed().await;
            Ok(())
        });
    }

    #[tokio::test]
    async fn early_error_is_surfaced_and_triggers_shutdown() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        // A peer that only ends when told to shut down — proves the broadcast reaches it.
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately.
        servers.spawn(async { Err(anyhow::anyhow!("boom")) });

        // `shutdown` never fires; the only way out is the failing task.
        let result = supervise(servers, pending(), tx).await;

        let err = result.expect_err("a failing task must surface as Err");
        assert_eq!(err.to_string(), "boom");
    }

    #[tokio::test]
    async fn clean_shutdown_drains_all_to_ok() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        spawn_until_shutdown(&mut servers, rx.clone());

        // `shutdown` is ready immediately → supervise broadcasts, both tasks drain Ok.
        let result = supervise(servers, ready(()), tx).await;

        assert!(result.is_ok(), "clean shutdown must drain all tasks to Ok, got {result:?}");
    }

    #[tokio::test]
    async fn early_clean_return_warns_not_errors() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that returns Ok before any shutdown — the warn branch, not an error.
        servers.spawn(async { Ok(()) });

        let result = supervise(servers, pending(), tx).await;

        assert!(result.is_ok(), "a clean early return is not an error, got {result:?}");
    }

    #[tokio::test]
    async fn error_surfaced_even_when_shutdown_wins_the_select() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        // A task that fails immediately, while shutdown is ALSO ready.
        servers.spawn(async { Err(anyhow::anyhow!("late boom")) });

        // Whichever `select!` arm wins, the drain must still surface the error.
        let result = supervise(servers, ready(()), tx).await;

        let err = result.expect_err("the error must survive even when shutdown wins the select");
        assert_eq!(err.to_string(), "late boom");
    }
}
```

- [ ] **Step 4: Run the tests to verify they FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway -E 'test(runtime::)'
```
Expected: the four `runtime::tests::*` tests **FAIL** — each panics with
`not yet implemented: implemented in Step 5` (the `todo!` stub).

- [ ] **Step 5: Implement the real `supervise` body**

Replace the `todo!("implemented in Step 5")` body with the select→broadcast→drain logic
(mirrors `paigasus-iam/src/main.rs:273-310`):

```rust
    // Stop on the first of: shutdown signal, or a server task ending.
    let early_error: Option<anyhow::Error> = tokio::select! {
        () = shutdown => {
            tracing::info!("shutdown signal received");
            None
        }
        Some(joined) = servers.join_next() => {
            match joined {
                Ok(Ok(())) => {
                    tracing::warn!("a server task exited before shutdown was requested");
                    None
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "a server task failed");
                    Some(e)
                }
                Err(join_err) => {
                    tracing::error!(error = %join_err, "a server task panicked");
                    Some(join_err.into())
                }
            }
        }
    };

    // Ask any still-running server to shut down gracefully.
    let _ = tx.send(());

    // Drain the remaining server task(s); surface the first error.
    let mut result = early_error.map_or(Ok(()), Err);
    while let Some(joined) = servers.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) if result.is_ok() => result = Err(e),
            Ok(Err(_)) => {}
            Err(join_err) if result.is_ok() => result = Err(join_err.into()),
            Err(_) => {}
        }
    }
    result
```

- [ ] **Step 6: Run the tests to verify they PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway -E 'test(runtime::)'
```
Expected: **4 tests run: 4 passed**.

- [ ] **Step 7: Lint + format the new code**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt -p paigasus-gateway
git -C .. diff --stat
```
Expected: clippy clean, no fmt changes to `runtime.rs` (or apply them if any).

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-463-gateway-metrics-supervise
git add rs/crates/services/paigasus-gateway/Cargo.toml \
        rs/crates/services/paigasus-gateway/src/lib.rs \
        rs/crates/services/paigasus-gateway/src/runtime.rs
git commit -m "feat(rs): add gateway runtime::supervise server-task supervisor (SMA-463)

Dependency-free select/broadcast/drain primitive mirroring paigasus-iam's
composition root, so a supervised task's failure propagates instead of being
swallowed. Adds tokio time/sync features the gateway was relying on transitively.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Refactor `main.rs` onto the shared `JoinSet` + supervise

Rewire the composition root so the main HTTP server, the separate metrics listener, and
the upkeep loop all spawn into the shared `JoinSet` on one `watch` shutdown channel, and
`main` ends by awaiting `runtime::supervise`. Removes both detached `tokio::spawn`s.

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (imports + the whole server-wiring section of `main`)

**Interfaces:**
- Consumes: `paigasus_gateway::runtime::supervise` (from Task 1).

- [ ] **Step 1: Add the two new imports**

In `rs/crates/services/paigasus-gateway/src/main.rs`, after the existing
`use paigasus_gateway::config::GatewayConfig;` line add:

```rust
use paigasus_gateway::runtime;
```

and after `use std::time::Duration;` add:

```rust
use tokio::task::JoinSet;
```

- [ ] **Step 2: Replace the `main` body from the metrics-upkeep block through the final `serve`**

Replace the entire `main` function body **from** the `// Periodic Prometheus upkeep`
block **down to and including** the final
`axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;` +
`Ok(())`. The recorder install (`metrics_handle = ...`) and `describe_gateway_metrics()`
call above it stay untouched. The new `main` reads:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = GatewayConfig::load()?;
    config.validate().map_err(anyhow::Error::msg)?;
    paigasus_logging::init("paigasus-gateway", &config.log_level);

    // Metrics (SMA-446 Unit 2): install the global Prometheus recorder only when `[metrics]` is
    // enabled — `!enabled` means no recorder is installed AND `/metrics` is never mounted below.
    // `config.validate()` already rejected `metrics.addr == http_addr` above.
    let metrics_handle = config.metrics.enabled.then(|| paigasus_observability::init("paigasus-gateway"));
    // Register `# HELP`/`# TYPE` exposition text for every family this service emits (spec
    // §4.1) — only when a recorder was actually installed above.
    if metrics_handle.is_some() {
        describe_gateway_metrics();
    }

    // Outbound clients. IAM connects lazily (a dead IAM does not block startup); the OpenAI client
    // is built with the three split timeout budgets. Neither construction logs the key.
    let iam = IamClient::connect(&config.iam).await?;
    let openai = OpenAiClient::new(
        &config.upstream.openai,
        Duration::from_secs(config.connect_timeout_secs),
        Duration::from_secs(config.first_byte_timeout_secs),
        Duration::from_secs(config.stream_idle_timeout_secs),
    )?;

    let state = AppState {
        iam: Arc::new(iam),
        openai: Arc::new(openai),
        max_request_bytes: config.max_request_bytes,
    };

    let app = router(state);
    // Same-port `/metrics`: merged onto the main router only when enabled AND no separate
    // `metrics.addr` is configured. In the `addr` case `/metrics` is served on its own listener
    // (spawned below) instead.
    let app = match (&metrics_handle, config.metrics.addr) {
        (Some(handle), None) => app.merge(paigasus_observability::metrics_router(handle.clone())),
        _ => app,
    };

    // All long-lived tasks share one graceful-shutdown `watch` and one `JoinSet`, so a failure in
    // any of them (metrics listener, upkeep) propagates instead of being silently swallowed
    // (SMA-463). Mirrors `paigasus-iam`'s composition root.
    let (tx, rx) = tokio::sync::watch::channel(());
    let mut servers: JoinSet<anyhow::Result<()>> = JoinSet::new();

    // Main HTTP server. The listener is bound up-front so a bind failure aborts startup fast (a
    // gateway that can't bind its public port should fail immediately), then moved into the task.
    let http_listener = tokio::net::TcpListener::bind(config.http_addr).await?;
    {
        let mut rx = rx.clone();
        servers.spawn(async move {
            axum::serve(http_listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }

    // Separate metrics listener, only when both enabled AND `metrics.addr` is configured — the
    // RECOMMENDED posture for a public gateway (keeps `/metrics` off the public HTTP port). Binds
    // INSIDE the task so a bind OR serve failure surfaces as a task error the `JoinSet` observes,
    // rather than a detached task that only logged before dying (SMA-463). On the same
    // shutdown-watch as every other task.
    if let (Some(handle), Some(metrics_addr)) = (metrics_handle.clone(), config.metrics.addr) {
        let mut rx = rx.clone();
        let metrics_app = paigasus_observability::metrics_router(handle);
        servers.spawn(async move {
            let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
            tracing::info!(%metrics_addr, "paigasus-gateway metrics listener started");
            axum::serve(listener, metrics_app)
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });
    }

    // Periodic Prometheus upkeep (CodeRabbit round-1 fix): `PrometheusBuilder::install_recorder()`
    // (unlike `install()`) does NOT spawn the maintenance task `PrometheusHandle::run_upkeep()`
    // needs to periodically drain/decay histograms — without calling it ourselves, memory grows
    // unbounded over the life of the process. `init()` itself stays runtime-agnostic (it's also
    // called from plain `#[test]` code with no Tokio runtime), so the spawn lives here. Now on the
    // shared `JoinSet` + shutdown-watch (SMA-463) rather than a detached task.
    if let Some(handle) = metrics_handle.clone() {
        let mut rx = rx.clone();
        servers.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = interval.tick() => handle.run_upkeep(),
                    _ = rx.changed() => break,
                }
            }
            Ok(())
        });
    }

    tracing::info!(%config.http_addr, "paigasus-gateway started");

    // Supervise: stop on the first of a shutdown signal or any task ending, broadcast graceful
    // shutdown to the rest, and surface the first error (SMA-463).
    runtime::supervise(servers, shutdown_signal(), tx).await
}
```

Leave `describe_gateway_metrics()` and `shutdown_signal()` (defined below `main`)
unchanged.

- [ ] **Step 3: Build the crate on its own (catches the tokio-feature issue)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-gateway
```
Expected: **Finished** with no errors (single-crate build proves `time`/`sync` are declared).

- [ ] **Step 4: Run the full gateway test suite**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway --no-tests=pass
```
Expected: **84 tests run: 84 passed** (the original 80 + the 4 new `runtime::` tests).

- [ ] **Step 5: Lint + format**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings && cargo fmt -p paigasus-gateway --check
```
Expected: clippy clean; `fmt --check` reports no diff.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-463-gateway-metrics-supervise
git add rs/crates/services/paigasus-gateway/src/main.rs
git commit -m "feat(rs): supervise gateway metrics listener via shared JoinSet (SMA-463)

Spawn the main HTTP server, the dedicated /metrics listener, and the Prometheus
upkeep loop into one JoinSet on a shared watch shutdown channel, ending main on
runtime::supervise. An unexpected metrics-listener failure now propagates and
restarts the process instead of silently losing the endpoint. Graceful shutdown
on SIGINT/SIGTERM preserved.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 7: Full CI gate graph (pre-PR sanity)**

Run the repo-level gates as CI does (per CLAUDE.md), since this touches a crate manifest
and adds a module:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-463-gateway-metrics-supervise
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations
```
Expected: all actions pass. If `:affected-smoke` or `:machete`/`:deny` flag something,
diagnose via `.moon/cache/ciReport.json`
(`jq '.actions[]|select(.status=="failed")'`) before proceeding. (No new external dep was
added — only tokio features were enabled — so `:deny`/`:machete` should be unaffected; the
gateway is not a `kernel->bindings` crate, so `:affected-smoke` should not trip.)

---

## Notes for the implementer

- The four `supervise` tests are deterministic. `tokio::select!` short-circuits on the
  first ready branch, so in `error_surfaced_even_when_shutdown_wins_the_select` the
  erroring task's result is captured whether `select!` picks the shutdown arm (task stays
  in the set → drained) or the `join_next` arm (captured as `early_error`) — the assertion
  holds either way.
- `metrics_handle.clone()` is used by both the separate-listener block and the upkeep
  block, so the clones are required (they are mutually exclusive at runtime with the
  same-port merge, but the compiler sees all three references). This mirrors IAM and is
  clippy-clean.
- Do not run cargo from the repo root — run it from the worktree's `rs/` directory.
