# SMA-473 — Bound the Redis Client Retry Budget: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cap `paigasus-iam`'s Redis reconnect retry budget so a Redis outage costs ~0.2–0.8 s per authz decision instead of a measured 19–28 s, and ship an alert for the outage that the fix makes quiet.

**Architecture:** A new `adapters::redis_conn` module becomes the single place a `redis::aio::ConnectionManager` is constructed, using a `ConnectionManagerConfig` with `number_of_retries = 1` (down from redis-rs's 6). All eight existing construction sites are converted to route through it, a CI grep gate keeps it that way, and a new Prometheus alert watches `iam_authz_decisions_total{cache="bypass"}` — the signal that replaces the latency spike operators used to notice.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `redis` 1.3.0 (`ConnectionManager` / `ConnectionManagerConfig`), `backon` 1.6.0 (transitive, supplies the retry schedule), `cargo nextest`, Moon 2.3.2 task graph, Prometheus + `promtool`.

**Spec:** `docs/superpowers/specs/2026-08-02-sma-473-redis-retry-budget-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates use **edition 2024 + rust-version 1.95**.
- Lint posture: `[workspace.lints.rust] warnings = "deny"` in-source; clippy is `all = "warn"` in-source and `-D warnings` in CI. **`clippy::pedantic` is NOT enabled** — so `float_cmp` will not fire, but this plan uses an epsilon comparison for `f32` anyway.
- Conventional commits with a workspace scope: `fix(rs): …`, `docs(repo): …`. **Subject must start lowercase and be ≤100 chars.** Do **not** write a bare `#NNN` issue reference in the commit body — it makes commitlint fail `footer-leading-blank`. Write `SMA-473` or `owner/repo PR NNN` instead.
- Bash PATH lacks the proto-managed CLIs. Prefix every command with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- Do **not** bypass the lefthook `commit-msg` hook with `--no-verify`.
- `cargo nextest` exits non-zero on a workspace with no tests — use `--no-tests=pass`.
- Branch is already created: `feature/sma-473-iam-bound-redis-retry-budget`. Do not create another.
- Never name a source file with a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`).

## File Structure

| File | Responsibility |
|---|---|
| **Create** `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` | The only place a `ConnectionManager` is constructed. Owns the tuned config, the constants, and their tests. |
| **Modify** `rs/crates/services/paigasus-iam/src/adapters/mod.rs` | Register the new module. |
| **Modify** `.../adapters/http/mod.rs:628-631` | `connect_redis` delegates to the helper. |
| **Modify** `.../adapters/oidc/redis_cache.rs:41-42` | `RedisJwksCache::connect` delegates. |
| **Modify** `.../adapters/authz/decision_cache.rs:127-128` | `RedisDecisionCache::connect` delegates. |
| **Modify** `.../adapters/authz/entity_cache.rs:66-67, 232` | `SliceCache::connect` + its `cfg(test)` lazy manager delegate. |
| **Modify** `.../adapters/authz/generation.rs:59-60` | `Generations::redis_connect` delegates. |
| **Modify** `.../adapters/api_keys/cache.rs:209-210, 384` | `RedisApiKeyCache::connect` + its `cfg(test)` lazy manager delegate. |
| **Modify** `moon.yml` | New `redis-connect-single-site` repo gate. |
| **Modify** `.github/workflows/ci.yml:184` | Add the new gate to the CI target list. |
| **Modify** `ops/observability/prometheus/rules/iam.rules.yml` | New `IamAuthzRedisCacheBypassed` alert. |
| **Modify** `ops/observability/prometheus/rules/tests/iam.test.yml` | promtool fixture with a non-firing control series. |
| **Modify** `docs/ops/RUNBOOK-observability.md` | Shipped numbers, new detection guidance, new alert entry, JWKS + API-key paths, residuals. |
| **Modify** `.../adapters/authz/cedar_authorizer.rs` (module doc, step 3) | Replace the stale 19–28 s passage. |
| **Modify** `.../tests/authz_acceptance.rs:~705-720` | Update the comments citing 19–28 s and "unshipped SMA-473". |

**Why `redis_conn` and not `redis`:** five files in this crate do `use redis::{AsyncCommands, Client};`. A sibling module literally named `redis` would make `use crate::adapters::redis;` shadow the extern crate in any module that imported it. `redis_conn` has no such hazard.

---

### Task 1: The `adapters::redis_conn` module

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs`
- Test: same file (`#[cfg(test)] mod tests`), following this crate's convention of unit tests co-located in the module.

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub(crate) fn connection_manager_config() -> redis::aio::ConnectionManagerConfig`
  - `pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<redis::aio::ConnectionManager>`

  Task 2 calls both. `connect` is **eager** — it awaits the initial connection, preserving `AppState::new`'s fail-fast-at-boot contract (spec D10).

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` with the test module only (the implementation follows in Step 3, so this fails to compile first — that is the intended red state):

```rust
// SPDX-License-Identifier: Apache-2.0

//! The single place this service constructs a Redis [`ConnectionManager`] (SMA-473).
//!
//! **Why this module exists.** `ConnectionManager::new` applies a stock
//! `ConnectionManagerConfig::default()`, whose reconnect budget is 6 retries on a
//! `100+200+400+800+1600+3200 ms` schedule. `backon` adds jitter (`delay × [1,2]`), so a
//! dead backend costs ~6.3–12.6 s per cycle — and a `ConnectionManager` burns a full cycle
//! per failed command, because the failing command triggers a background reconnect and the
//! NEXT command awaits a brand-new cycle. A single authz decision makes 2–3 such reads, for
//! a measured 19–28 s; a revoke, 28.4 s.
//!
//! **What the budget actually buys.** Only tolerance while ESTABLISHING a connection.
//! `send_packed_command` never retries a *command* — it surfaces the error to the caller and
//! reconnects in the background. So the case one retry covers is narrow and specific: a
//! first connect attempt landing in a failover gap (old primary gone, new one not yet
//! accepting), which `min_delay` (100–200 ms jittered) is well matched to.
//!
//! **What is deliberately left alone** (SMA-473 D1) — `min_delay`, `exponent_base`,
//! `connection_timeout` (1 s) and `response_timeout` (500 ms). The last two are already
//! bounded by redis-rs and are NOT what costs the time; tightening them was considered and
//! declined. Note the consequence: this bounds a *stopped* Redis (instant `ECONNREFUSED`),
//! not a *blackholed* one, where `connection_timeout` dominates at ~2.1 s per command.

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    /// The change itself. If this fails, a Redis outage costs seconds per authz decision
    /// again (SMA-473) — do not "fix" it by relaxing the assertion.
    #[test]
    fn the_tuned_config_caps_the_reconnect_retry_budget() {
        let cfg = connection_manager_config();
        assert_eq!(
            cfg.number_of_retries(),
            1,
            "SMA-473: the reconnect retry count must stay capped at 1 — redis-rs defaults to 6, \
             which costs ~6.3-12.6s per failed command and a measured 19-28s per authz decision"
        );
        assert_eq!(
            cfg.max_delay(),
            Some(Duration::from_millis(500)),
            "SMA-473: the max_delay guard must stay set — it is inert at 1 retry, but it is what \
             caps each step if the retry count is ever raised (backon's own default is 60s/step)"
        );
    }

    /// Pins the OTHER half of D1 — the knobs deliberately NOT touched. Asserted twice on
    /// purpose: against a stock config (catches us tightening one) AND against absolute
    /// values (catches a redis-rs bump moving a default under us).
    #[test]
    fn the_tuned_config_leaves_every_other_knob_at_the_redis_rs_default() {
        let cfg = connection_manager_config();
        let stock = ConnectionManagerConfig::new();

        assert_eq!(cfg.min_delay(), stock.min_delay(), "SMA-473 D1: min_delay must stay at the redis-rs default");
        assert_eq!(
            cfg.connection_timeout(),
            stock.connection_timeout(),
            "SMA-473 D1: connection_timeout must stay at the redis-rs default — it is already bounded \
             and is NOT what costs the time during an outage"
        );
        assert_eq!(
            cfg.response_timeout(),
            stock.response_timeout(),
            "SMA-473 D1: response_timeout must stay at the redis-rs default"
        );
        assert!(
            (cfg.exponent_base() - stock.exponent_base()).abs() < f32::EPSILON,
            "SMA-473 D1: exponent_base must stay at the redis-rs default"
        );

        assert_eq!(cfg.min_delay(), Duration::from_millis(100), "redis-rs 1.3.0's documented min_delay default moved — re-check the SMA-473 arithmetic");
        assert_eq!(cfg.connection_timeout(), Some(Duration::from_secs(1)), "redis-rs 1.3.0's documented connection_timeout default moved — re-check the SMA-473 arithmetic");
        assert_eq!(cfg.response_timeout(), Some(Duration::from_millis(500)), "redis-rs 1.3.0's documented response_timeout default moved — re-check the SMA-473 arithmetic");
    }

    /// Proves the config is actually APPLIED to a real manager rather than built and
    /// dropped. Deliberately loose (2 s vs a ~100-200 ms expectation) — the two tests above
    /// own exactness; this one only has to fail if the config never reaches the manager.
    ///
    /// `#[tokio::test]` is REQUIRED: `new_lazy_with_config` calls `runtime.spawn`, which
    /// panics outside a Tokio runtime. `127.0.0.1:1` is a closed port, so the connect is
    /// refused instantly rather than timing out (same pattern as
    /// `entity_cache`/`api_keys::cache`'s unreachable-backend tests).
    #[tokio::test]
    async fn a_command_against_an_unreachable_backend_fails_fast() {
        use redis::AsyncCommands;

        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never actually reachable");
        let mut conn = ConnectionManager::new_lazy_with_config(client, connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma473:probe").await;
        let elapsed = started.elapsed();

        // Control: without this the deadline could pass for the WRONG reason — a malformed
        // URL or an invalid config erroring instantly looks identical to a fast, correct
        // failure. (It cannot separate a fast refuse from a slow timeout; the deadline does.)
        let err = result.expect_err("an unreachable backend must error, not return a value");
        assert!(err.is_io_error(), "expected an IO/connection error, got {err:?} — the probe never actually dialed");

        assert!(
            elapsed < Duration::from_secs(2),
            "SMA-473: a command against a dead Redis took {elapsed:?}; the tuned config must bound it \
             well under 2s (stock redis-rs is ~6.3-12.6s and cost a measured 19-28s per authz decision)"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::redis_conn --no-tests=pass
```

Expected: **compile error** — `cannot find function connection_manager_config in this scope`.

Two notes so this red state does not derail you: the module is not registered until Step 4, so if `cargo` never compiles the file at all, do Step 4 first and re-run. And because `warnings = "deny"`, a **non-test** build of this intermediate state also errors on the `use` lines being unused (they are used only by the test module). Both resolve at Step 5; do not "fix" them now.

- [ ] **Step 3: Write the implementation**

Insert into `redis_conn.rs`, between the `use` lines and the `#[cfg(test)] mod tests`:

```rust
/// Retries AFTER the first attempt (redis-rs defaults to 6 — see the module doc for what
/// that costs). One retry covers a first connect attempt landing in a failover gap;
/// anything more just adds latency to a genuine outage.
const CONNECT_RETRIES: usize = 1;

/// Guard only — **inert** at `CONNECT_RETRIES = 1`. `backon` applies `max_delay` to the
/// pre-jitter base delay and never to the first step (the first delay is always
/// `min_delay`), so with a single retry this is never reached. It exists so that raising
/// `CONNECT_RETRIES` later caps each step here rather than at `backon`'s own 60 s default.
const RETRY_MAX_DELAY: Duration = Duration::from_millis(500);

/// The tuned config every Redis connection in this service is opened with.
///
/// `pub(crate)` and exposed separately from [`connect`] so the config tests can assert on it
/// directly, and so the two `#[cfg(test)]` lazy managers elsewhere in this crate can build
/// from the exact production config rather than a hand-rolled copy.
pub(crate) fn connection_manager_config() -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_number_of_retries(CONNECT_RETRIES)
        .set_max_delay(RETRY_MAX_DELAY)
}

/// Opens `redis_url` and wraps it in a [`ConnectionManager`] built with
/// [`connection_manager_config`] — the ONLY way this crate constructs one (enforced by the
/// `repo:redis-connect-single-site` CI gate).
///
/// **Eager**: `new_with_config` awaits the initial connection, so a Redis that is down at
/// boot still fails `AppState::new` rather than yielding a manager that fails later. That
/// preserves the pre-SMA-473 contract — but note the tolerance window shrinks from ~6–12 s
/// to ~200 ms, so a Redis slow to start now costs one crash-restart (SMA-473 D10).
///
/// Returns a bare [`redis::RedisResult`] rather than a domain error because the callers map
/// it differently on purpose: `http::connect_redis` to `AuthnError::Backend`,
/// `RedisJwksCache::connect` to the fail-closed `AuthnError::Unavailable`.
pub(crate) async fn connect(redis_url: &str) -> redis::RedisResult<ConnectionManager> {
    let client = redis::Client::open(redis_url)?;
    ConnectionManager::new_with_config(client, connection_manager_config()).await
}
```

- [ ] **Step 4: Register the module**

In `rs/crates/services/paigasus-iam/src/adapters/mod.rs`, add to the alphabetically-ordered list (between `pub mod persistence;` and the end — the list is currently `api_keys, auth, authz, clock, events, grpc, http, id, oidc, persistence`; insert `redis_conn` after `persistence`):

```rust
pub(crate) mod redis_conn;
```

`pub(crate)`, not `pub`: nothing outside this crate constructs Redis connections.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::redis_conn --no-tests=pass
```

Expected: **3 passed**. The `a_command_against_an_unreachable_backend_fails_fast` test should report well under 1 s.

- [ ] **Step 6: Lint and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: clean. If `cargo fmt --check` fails, run `cargo fmt` and re-check.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs rs/crates/services/paigasus-iam/src/adapters/mod.rs
git commit -m "fix(rs): add a redis_conn helper capping the reconnect retry budget (SMA-473)"
```

---

### Task 2: Route every construction site through the helper

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:628-631`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs:14, 41-42`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs:28, 127-128`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs:66-67, 232`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs:18, 59-60`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs:209-210, 384`

**Interfaces:**
- Consumes: `crate::adapters::redis_conn::{connect, connection_manager_config}` from Task 1.
- Produces: no new signatures. Every converted function keeps its **exact existing signature and error mapping**.

There are exactly **8** `ConnectionManager::new*` occurrences in `src/`. All 8 are converted here.

**Import hazard — read before editing.** Five files import `use redis::{AsyncCommands, Client};`. After conversion:
- `oidc/redis_cache.rs`, `authz/decision_cache.rs`, `authz/generation.rs` — each used `Client::` in exactly **one** place (the constructor being converted), so `Client` becomes **unused** → change the import to `use redis::AsyncCommands;`.
- `authz/entity_cache.rs`, `api_keys/cache.rs` — their `#[cfg(test)]` modules also call `Client::open`, so `Client` stays used → **leave the import alone**.
- `http/mod.rs` used the fully-qualified `redis::Client::open`, so there is no import to change; `use redis::aio::ConnectionManager;` (line 27) stays, as it is still the return type.

`warnings = "deny"` means a stale import is a hard compile error, so Step 5 catches any mistake here.

- [ ] **Step 1: Convert `http::connect_redis`**

`adapters/http/mod.rs`, replace the body (lines 628-631):

```rust
async fn connect_redis(redis_url: &str) -> Result<ConnectionManager, AuthnError> {
    let client = redis::Client::open(redis_url).map_err(|e| AuthnError::Backend(Box::new(e)))?;
    ConnectionManager::new(client).await.map_err(|e| AuthnError::Backend(Box::new(e)))
}
```

with:

```rust
async fn connect_redis(redis_url: &str) -> Result<ConnectionManager, AuthnError> {
    crate::adapters::redis_conn::connect(redis_url).await.map_err(|e| AuthnError::Backend(Box::new(e)))
}
```

Also append this sentence to `connect_redis`'s existing doc comment (which currently ends "...mirroring `RedisJwksCache::connect`'s connect pattern."):

```
/// Delegates to [`crate::adapters::redis_conn::connect`] for the tuned reconnect retry
/// budget (SMA-473) — this function owns only the `AuthnError` mapping.
```

- [ ] **Step 2: Convert `RedisJwksCache::connect`**

`adapters/oidc/redis_cache.rs` — change line 14 from `use redis::{AsyncCommands, Client};` to `use redis::AsyncCommands;`, then replace lines 41-42:

```rust
        let client = Client::open(redis_url).map_err(|err| log_unavailable(None, err.kind()))?;
        let conn = ConnectionManager::new(client).await.map_err(|err| log_unavailable(None, err.kind()))?;
```

with:

```rust
        let conn = crate::adapters::redis_conn::connect(redis_url).await.map_err(|err| log_unavailable(None, err.kind()))?;
```

The single mapping covers both the old `Client::open` and connect failures, because `connect` returns both as a `RedisError`. Behavior is identical.

- [ ] **Step 3: Convert the three remaining production-shaped constructors**

`adapters/authz/decision_cache.rs` — line 28 → `use redis::AsyncCommands;`, then replace lines 127-128:

```rust
        let client = Client::open(redis_url).map_err(redis_connect_err)?;
        let conn = ConnectionManager::new(client).await.map_err(redis_connect_err)?;
```

with:

```rust
        let conn = crate::adapters::redis_conn::connect(redis_url).await.map_err(redis_connect_err)?;
```

`adapters/authz/generation.rs` — line 18 → `use redis::AsyncCommands;`, then replace lines 59-60:

```rust
        let client = Client::open(redis_url).map_err(redis_err)?;
        let conn = ConnectionManager::new(client).await.map_err(redis_err)?;
```

with:

```rust
        let conn = crate::adapters::redis_conn::connect(redis_url).await.map_err(redis_err)?;
```

`adapters/authz/entity_cache.rs` — **leave line 39's import alone**, replace lines 66-67:

```rust
        let client = Client::open(redis_url).map_err(redis_connect_err)?;
        let conn = ConnectionManager::new(client).await.map_err(redis_connect_err)?;
```

with:

```rust
        let conn = crate::adapters::redis_conn::connect(redis_url).await.map_err(redis_connect_err)?;
```

`adapters/api_keys/cache.rs` — **leave line 41's import alone**, replace lines 209-210:

```rust
        let client = Client::open(redis_url)?;
        let conn = ConnectionManager::new(client).await?;
```

with:

```rust
        let conn = crate::adapters::redis_conn::connect(redis_url).await?;
```

- [ ] **Step 4: Convert the two `#[cfg(test)]` lazy managers**

These are the measured 28.4 s win. In `adapters/api_keys/cache.rs` line 384, replace:

```rust
        let conn = ConnectionManager::new_lazy_with_config(client, redis::aio::ConnectionManagerConfig::new()).expect("lazy ConnectionManager construction never connects");
```

with:

```rust
        let conn = ConnectionManager::new_lazy_with_config(client, crate::adapters::redis_conn::connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");
```

Apply the identical replacement at `adapters/authz/entity_cache.rs` line 232.

In `api_keys/cache.rs`, also update the test's doc comment — it currently says the manager "never dials out", which is wrong (it dials, is refused, and used to burn three full retry cycles for 28.4 s). Replace that doc comment with:

```rust
    /// D5's fail-open contract, exercised without any live Redis: a `get` against an
    /// unreachable backend degrades to `None`, and `put`/`evict` never panic. Uses the
    /// production `redis_conn::connection_manager_config()` — with a stock config this test
    /// took a measured **28.4 s** (three commands × a full ~9.5 s reconnect-retry cycle),
    /// which is the cost SMA-473 removed.
```

- [ ] **Step 5: Build, lint, and verify no site was missed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd /Users/smaschek/dev/paigasus/paigasus-core
grep -rn "ConnectionManager::new" rs/crates/services/paigasus-iam/src/
```

Expected: clippy clean (a missed import edit shows up here as `unused import`), and the grep returns **exactly two** lines, both in `adapters/redis_conn.rs` — the `new_with_config` in `connect` and the `new_lazy_with_config` in its test.

If the grep returns anything in another file, convert it before continuing.

- [ ] **Step 6: Run the full IAM unit suite and confirm the 28 s is gone**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && time cargo nextest run -p paigasus-iam --lib --no-tests=pass
```

Expected: all tests pass, and `api_keys::cache::tests::redis_cache_fails_open_when_the_backend_is_unreachable` reports **well under 1 s** (it was 28.403 s). Note the per-test duration nextest prints for it — Task 5 cites the before/after.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/
git commit -m "fix(rs): route every redis ConnectionManager through redis_conn (SMA-473)"
```

---

### Task 3: CI gate — no bypassing construction site

**Files:**
- Modify: `moon.yml` (add a task alongside `wasm-getrandom-free`)
- Modify: `.github/workflows/ci.yml:184` (add the target to the list)

**Interfaces:**
- Consumes: the post-Task-2 invariant that `ConnectionManager::new*` appears only in `adapters/redis_conn.rs`.
- Produces: a `repo:redis-connect-single-site` Moon target.

This is the only thing that catches a **new** call site added later; Task 1's tests only catch a changed constant.

- [ ] **Step 1: Add the Moon task**

In `moon.yml`, insert after the `wasm-getrandom-free` task block (before `promtool:`):

```yaml
  redis-connect-single-site:
    description: 'Assert redis connection config is owned solely by `adapters::redis_conn`, so no call site can restore the unbounded reconnect retry budget (SMA-473).'
    # WHAT IS GATED, and why it is not simply "ConnectionManager::new":
    #   - `ConnectionManager::new(` / `::new_with_config(` — the eager constructors the
    #     helper owns. Allowed only in redis_conn.rs.
    #   - the `ConnectionManagerConfig` TYPE — naming it is how you would build an untuned
    #     config. Allowed only in redis_conn.rs.
    # `ConnectionManager::new_lazy_with_config(...)` is deliberately NOT gated: two
    # `#[cfg(test)]` sites legitimately call it, and they are safe precisely because the
    # rule above means the only config they can obtain is `connection_manager_config()`.
    #
    # Comment lines are excluded so prose may still name the API.
    #
    # Two portability notes, both learned the hard way:
    #   - Do NOT anchor paths on `^\./`. GNU grep (CI, Linux) emits the `./` prefix; ugrep
    #     (some dev shells) strips it. An `^\./` anchor silently matches nothing on one of
    #     them, which would make this gate pass while guarding nothing.
    #   - The comment filter anchors on `:[0-9]+:[[:space:]]*//` so it tests the CONTENT,
    #     not any `://` that happens to appear inside a redis URL on a code line.
    #
    # The control (`expected` must be non-empty) matters: without it, a rename of
    # redis_conn.rs — or a typo in the pattern — would make BOTH greps empty and the gate
    # would pass while guarding nothing.
    script: |
      cd rs/crates/services/paigasus-iam/src
      hits="$(grep -rnE 'ConnectionManager::new\(|ConnectionManager::new_with_config\(|ConnectionManagerConfig' . | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
      expected="$(printf '%s\n' "$hits" | grep -E 'adapters/redis_conn\.rs:' || true)"
      offenders="$(printf '%s\n' "$hits" | grep -vE 'adapters/redis_conn\.rs:' || true)"
      if [ -z "$expected" ]; then
        echo "no redis connection-config construction found in adapters/redis_conn.rs — the guard is not guarding anything (renamed file? changed API?)" >&2
        exit 2
      fi
      if [ -n "$offenders" ]; then
        echo "redis connection config built outside adapters/redis_conn.rs (SMA-473 — use redis_conn::connect / connection_manager_config):" >&2
        printf '%s\n' "$offenders" >&2
        exit 1
      fi
    toolchain: 'system'
    # Narrow inputs — `repo` owns the whole tree, so without these the guard runs on every change.
    inputs:
      - 'rs/crates/services/paigasus-iam/src/**/*'
      - 'rs/crates/services/paigasus-iam/tests/**/*'
```

**This block is a sketch; `moon.yml` is the source of truth.** Review hardened it in
three ways after this plan was written, and the shipped gate differs accordingly —
read `moon.yml` rather than copying from here:

1. **Scope is `src tests`, not `src`** (AC1 says "production and test"): a Docker-gated
   integration test can construct a stock manager just as easily as production code.
2. **`.get_connection_manager` is a fourth alternation term.**
   `redis::Client::get_connection_manager()` internally does
   `ConnectionManager::new(self.clone())` (`redis-1.3.0/src/client.rs:453`), restoring the
   stock 6-retry config without ever naming a gated symbol — and it is the *first*
   `ConnectionManager` example in redis-rs's own docs, i.e. the likeliest accidental bypass.
3. **`new_lazy_with_config` gets its own second check.** A flat ban would be wrong (two
   `#[cfg(test)]` sites legitimately need a non-dialing constructor), and a path allowlist
   would destroy the strict-equality property — so instead every call outside
   `redis_conn.rs` must name `connection_manager_config()` on the same line.

**Note on the probe in Step 3 below:** it must name something the gate actually
catches. `ConnectionManager::new` without a paren is *not* gated (that is the bare
path) — use `ConnectionManagerConfig::new()` in the probe instead.

- [ ] **Step 2: Verify the gate passes on the current (correct) tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:redis-connect-single-site
```

Expected: **PASS**.

- [ ] **Step 3: Verify the gate actually fails on a violation**

A gate never seen red is not a gate. Introduce a temporary violation, confirm red, then revert:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '\nfn _sma473_guard_probe() { let _ = redis::aio::ConnectionManagerConfig::new(); }\n' \
  >> rs/crates/services/paigasus-iam/src/adapters/clock.rs
moon run repo:redis-connect-single-site   # expect FAIL, listing clock.rs
git checkout -- rs/crates/services/paigasus-iam/src/adapters/clock.rs
moon run repo:redis-connect-single-site   # expect PASS again
```

Expected: FAIL naming `./adapters/clock.rs`, then PASS after the revert. Confirm `git status` is clean for `clock.rs` before continuing.

- [ ] **Step 4: Add the target to CI**

In `.github/workflows/ci.yml` line 184, add `:redis-connect-single-site` to the `T=(...)` array, immediately after `:wasm-getrandom-free`:

```
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add moon.yml .github/workflows/ci.yml
git commit -m "ci(repo): gate that redis ConnectionManager is built in one place (SMA-473)"
```

---

### Task 4: `IamAuthzRedisCacheBypassed` alert + promtool fixture

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`

**Interfaces:**
- Consumes: the existing `iam_authz_decisions_total{cache="bypass"}` series, emitted at `adapters/authz/cedar_authorizer.rs:250` when `cache_key` returns `None`. The family is registered in `paigasus-observability`'s `names.rs:22`, so the `observability-drift` gate accepts it.
- Produces: alert name `IamAuthzRedisCacheBypassed`, referenced by Task 5's RUNBOOK entry.

**Why this exists:** the RUNBOOK currently tells operators to expect a Redis outage as `IamHighErrorRate`/`IamGrpcHighErrorRate`/client timeouts. Those fire *because of* the latency Tasks 1–2 remove. Without this alert, a total Redis outage becomes fast, correct, and **completely unalerted** (spec D9).

- [ ] **Step 1: Write the failing fixture**

Append to `ops/observability/prometheus/rules/tests/iam.test.yml` (the `tests:` list):

```yaml
  # IamAuthzRedisCacheBypassed (SMA-473): sum(rate(...{cache="bypass"}[5m])) > 0, for: 10m.
  #
  # The `cache="hit"` series is the CONTROL — it climbs across the whole window, so a rule
  # that dropped its `cache="bypass"` selector, or used `>= 0` instead of `> 0`, would fire
  # during the control window below and fail this test. An all-firing fixture could not tell
  # those apart (the SMA-466 lesson).
  - interval: 1m
    input_series:
      - series: 'iam_authz_decisions_total{cache="bypass",decision="allow"}'
        values: '0+0x5 0+1x18'
      - series: 'iam_authz_decisions_total{cache="hit",decision="allow"}'
        values: '0+5x23'
    alert_rule_test:
      # Control: only the `hit` series is moving — a healthy Redis must never page.
      - eval_time: 5m
        alertname: IamAuthzRedisCacheBypassed
        exp_alerts: []
      # Bypass starts at 6m, but `for: 10m` must hold it back well past that.
      - eval_time: 12m
        alertname: IamAuthzRedisCacheBypassed
        exp_alerts: []
      - eval_time: 20m
        alertname: IamAuthzRedisCacheBypassed
        exp_alerts:
          - exp_labels: { severity: critical }
            exp_annotations: { summary: "IAM authz is bypassing the Redis decision cache (Redis unhealthy)" }
```

- [ ] **Step 2: Run promtool to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool test rules ops/observability/prometheus/rules/tests/iam.test.yml
```

Expected: **FAIL** — the `eval_time: 20m` case reports no alert, because `IamAuthzRedisCacheBypassed` does not exist yet.

- [ ] **Step 3: Add the alert rule**

In `ops/observability/prometheus/rules/iam.rules.yml`, append to the `rules:` list under `- name: iam`:

```yaml
      - alert: IamAuthzRedisCacheBypassed
        # SMA-473. `cache="bypass"` is emitted ONLY when the entity-generation counter read
        # errors (`cedar_authorizer.rs` step 3), which under `authz.cache.backend = "memory"`
        # cannot happen — so a sustained nonzero rate means the Redis backend is unhealthy,
        # with no healthy-state false positives.
        #
        # This alert exists BECAUSE SMA-473 made the outage quiet. Before it, a Redis outage
        # announced itself as ~20-30s authz latency that tripped IamHighErrorRate and client
        # timeouts. Bounding the retry budget removed that signal; without this rule a total
        # Redis outage produces correct, fast decisions and NO page — while the decision cache
        # is gone, cross-replica API-key revocation stops being global, and (under
        # `jwks_cache.backend = "redis"`) every authenticated request 503s.
        #
        # `sum(...)` so the alert is one series, not one per `decision` label — and so the
        # result carries no labels beyond `severity`.
        #
        # `for: 10m`, not 0m: a routine Redis failover briefly bypasses too, and that must
        # not page. NOTE the `memory`-backend trap — the series does not exist there, `sum()`
        # over an empty vector is empty, and this alert is SILENT rather than firing. See the
        # RUNBOOK's "Authz availability posture".
        expr: sum(rate(iam_authz_decisions_total{cache="bypass"}[5m])) > 0
        for: 10m
        labels: { severity: critical }
        annotations: { summary: "IAM authz is bypassing the Redis decision cache (Redis unhealthy)" }
```

- [ ] **Step 4: Run promtool to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool check rules ops/observability/prometheus/rules/*.rules.yml
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
```

Expected: **SUCCESS** on both.

If the timing assertions fail, adjust only the `eval_time` values — the `for: 10m` window plus the 5m rate window makes the exact firing instant fiddly. Do **not** weaken the `eval_time: 5m` control case or the `> 0` comparison; those are the point of the fixture. Widen the firing eval_time (e.g. 20m → 22m) and re-run.

- [ ] **Step 5: Run the observability drift gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:observability-drift repo:promtool
```

Expected: PASS. This asserts the new rule references only registered metric families.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add ops/observability/prometheus/rules/
git commit -m "feat(repo): alert when iam authz bypasses the redis decision cache (SMA-473)"
```

---

### Task 5: Documentation

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (§"Authz availability posture", and the alert catalog)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs` (module doc, step 3)
- Modify: `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs` (~lines 705-720)

**Interfaces:**
- Consumes: the alert name `IamAuthzRedisCacheBypassed` from Task 4; the measured before/after timing from Task 2 Step 6.
- Produces: nothing consumed by later tasks.

**No CI gate validates any of this** — `observability-drift` reads only `ops/observability/**` and `paigasus-observability/**`, and nothing checks the RUNBOOK markdown. Review is the gate. Be thorough.

- [ ] **Step 1: Rewrite the RUNBOOK's cost paragraphs**

In `docs/ops/RUNBOOK-observability.md` §"Authz availability posture", replace the block starting **"Fail-open is NOT free: budget ~20–30 s per authz decision while Redis is down."** and running through **"...the only levers are to restore Redis or to run `authz.cache.backend = "memory"` (single-replica only)."** with prose covering:

- The bound is now ~0.2–0.8 s per authz decision, from a capped reconnect retry budget (`adapters::redis_conn`, `number_of_retries = 1`).
- Keep the retry-schedule-vs-timeouts explanation — it is still why the fix looks the way it does — but state it in the past tense as the diagnosis, not a live problem.
- Replace the measured table with the spec §3.3 table, including the **gated (cross-principal) query** row: `decide_gated` runs a full second `is_authorized` when `req.principal != actor` (`authorize.rs:76-79`), which the SMA-470 measurement never exercised because it used a self query.
- Delete "**None of this is shipped yet** — it is pre-existing and tracked as **SMA-473**" and the "no config-only workaround" sentence.
- Add the residual: a **blackholed** Redis (SYN dropped, not refused) still costs ~2.1 s per failed command because `connection_timeout` then dominates. The numbers above describe a stopped/refused Redis.
- Add the boot-tolerance change: `AppState::new` still fails fast when Redis is down at boot, but ~50× sooner, so a Redis slow to start now costs one crash-restart rather than being absorbed.

Do **not** edit the existing sentence "leaves `max_delay` unset (so no per-step cap is applied beyond `backon`'s own inert 60 s default, which this schedule never reaches)" — it is accurate. Instead add, where the new config is described, that once `max_delay` **is** set it still does not apply to the first delay step (the first delay is always `min_delay`), which is why it is inert at one retry.

- [ ] **Step 2: Replace the detection guidance**

Still in §"Authz availability posture", the sentences reading **"Page on 'Redis is down'; do not treat it as a background degradation. Expect the symptom to arrive as `IamGrpcHighErrorRate` / `IamHighErrorRate` / client-side timeouts rather than as a clean `cache="bypass"` signal."** are exactly what stops being true. Replace with guidance that the symptom now arrives as **`IamAuthzRedisCacheBypassed`** (Task 4), and that the error-rate alerts will **not** fire, because decisions stay correct and fast.

- [ ] **Step 3: Revise the revocation-freshness paragraph**

Replace the "**During a Redis outage, budget nearer ~55 s than ~31 s**" passage and the `ttl + poll + 2 × retry cycle` worst case: the retry-cycle term is now sub-second, so the worst case returns to `policy_cache_ttl_secs + refresh_interval_secs` (~31 s at defaults) plus the reload's own duration.

**Preserve** the surrounding notes that do not depend on the retry cycle:
- `IamConfig::validate` permits `refresh_interval_secs` **equal** to `policy_cache_ttl_secs`, so the worst case is a genuine sum;
- raising the poll interval to its permitted maximum doubles the bound to `2 × policy_cache_ttl_secs` (60 s at the default TTL);
- neither setting has an upper cap, so raising `policy_cache_ttl_secs` scales the bound linearly.

- [ ] **Step 4: Add the JWKS and API-key paths**

Add a new paragraph to §"Authz availability posture" (this is **new information**, not a numbers update):

- Under `authn.jwks_cache.backend = "redis"`, a Redis outage is a **fail-closed authentication** outage, not merely an authz slowdown. `RedisJwksCache::get` maps any Redis error to `AuthnError::Unavailable` (spec §4.3/D15 — correct for key material), and `JwksProvider::key_for` reads the cache on **every** token validation, so every authenticated request 503s for the duration. SMA-473 makes that failure fast (~0.1–0.2 s); it does not and should not make it succeed.
- `RedisApiKeyCache` shares the same connection and is read on every API-key-authenticated request — the RUNBOOK already calls the gateway's `IntrospectApiKey`/`IsAuthorized` pair the hottest gRPC path. A miss costs a `get` **and** a `put`; `RevokeApiKey`/`ArchiveServiceAccount` add an `evict`. During an outage, cross-replica revocation stops being global and degrades to per-replica TTL.

- [ ] **Step 5: Add the alert catalog entry**

Add an `### IamAuthzRedisCacheBypassed — authz is bypassing the Redis decision cache (critical)` section to the RUNBOOK's alert catalog, in the same shape as the neighbouring entries. Cover:

- **Meaning:** `sum(rate(iam_authz_decisions_total{cache="bypass"}[5m])) > 0` for 10m — the entity-generation counter has been unreadable for ten minutes, i.e. Redis is unhealthy. Decisions remain **correct** (computed against the Postgres-compiled snapshot) and **fast** (SMA-473) — this alert exists precisely because those two facts mean nothing else will tell you.
- **NOTE — `authz.cache.backend = "memory"` makes this alert SILENT, not firing.** The series does not exist on the memory backend and `sum()` over an empty vector is empty. Same trap as `audit.retention.enabled = false` for `IamAuditPartitionMaintenanceStalled`.
- **Likely causes:** Redis down/unreachable, credentials or TLS rejected, `maxmemory` pressure evicting under an `allkeys-*` policy, network partition.
- **Blast radius while firing:** no decision cache and no entity-slice cache (raw Postgres load per decision); cross-replica API-key revocation degrades to per-replica TTL; a revoke's `policy_gen` bump is lost so revocation freshness falls back to the TTL backstop (~31 s at defaults); and under `jwks_cache.backend = "redis"` **every authenticated request 503s**.
- **Confirm:** `up{job="iam"} == 1` (else it is `TargetDown`); check `iam_authz_decisions_total{cache="bypass"}` vs `{cache="hit"}`; check IAM logs for `cedar_authorizer: entity generation counter unreadable` and `redis jwks cache error`.
- **Remediation:** restore Redis. There is no config-only workaround; `authz.cache.backend = "memory"` is single-replica only.

- [ ] **Step 6: Update the `cedar_authorizer.rs` module doc**

In `rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs`, step 3 of the `is_authorized` flow, replace the passage running from **"Do not read "costs latency" as "costs little": the shared `ConnectionManager`..."** through **"...Capping `number_of_retries`/`max_delay` (or a circuit breaker) is the fix and is a tracked follow-up; tightening the timeouts would not help."**

with this (keep the surrounding fail-open explanation and the `Decision`/audit sentences that follow it exactly as they are — only this passage changes):

```rust
//!    proceeds unconditionally (D11/D12's fail-open property: an accelerator outage costs
//!    latency, never correctness). That latency is BOUNDED, but only because it was
//!    deliberately bounded: the shared `ConnectionManager` (`adapters::redis_conn`) caps the
//!    reconnect retry budget at ONE retry (SMA-473), so a counter read against a dead backend
//!    fails in ~100-200 ms. A decision makes 2-3 such reads — 3 while the policy-snapshot
//!    stamp is still trusted, 2 once it goes provisional and `reload_if_stale` stops reading
//!    `policy_gen` — so a full Redis outage costs ~0.2-0.6 s per decision. With redis-rs's
//!    stock `ConnectionManagerConfig::default()` (6 retries, `100+200+400+800+1600+3200 ms`,
//!    jittered to ~6.3-12.6 s per cycle, and a `ConnectionManager` burns a FULL cycle per
//!    failed command) the same decision cost a measured 19-28 s. Note the cost was the RETRY
//!    SCHEDULE, never the per-attempt timeouts — redis-rs already defaults
//!    `connection_timeout` to 1 s and `response_timeout` to 500 ms, and both are deliberately
//!    left alone. If the read succeeds and the key is
```

Two things to preserve deliberately: the phrase "an accelerator outage costs latency, never correctness" (it is D11/D12's contract, unchanged), and the trailing `//!    already cached, that cached` continuation that follows in the original. Do **not** carry over "availability, in practice, largely is not" — that sentence is what this issue falsified.

- [ ] **Step 7: Update the acceptance-test comments**

In `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs` (~lines 705-720), two comments justify the 90 s budget by citing "~20-30s per request" and "amendment A / SMA-473" as an unshipped follow-up. Update both to the shipped numbers, and **keep the 90 s budget** with a one-line note that it stays wide deliberately: it is a failure deadline against a slow runner, not an assertion of the `ttl + poll` bound, so widening headroom costs nothing on the happy path.

- [ ] **Step 8: Verify the docs build and nothing else broke**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
cd /Users/smaschek/dev/paigasus/paigasus-core
grep -rn "SMA-473" docs/ops/RUNBOOK-observability.md rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs
```

Expected: clippy/fmt clean. The grep should show SMA-473 referenced as **shipped** work — no remaining "not yet implemented", "unshipped", or "tracked follow-up" phrasing about it.

- [ ] **Step 9: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add docs/ops/RUNBOOK-observability.md rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs rs/crates/services/paigasus-iam/tests/authz_acceptance.rs
git commit -m "docs(repo): record the shipped redis retry bound and its new alert (SMA-473)"
```

---

### Task 6: Full-graph verification

**Files:** none modified (verification only; fix-ups land wherever the gates point).

**Interfaces:**
- Consumes: everything from Tasks 1–5.
- Produces: a branch ready for a PR.

Per-project Moon tasks do **not** run the repo-level gates. This task runs the graph the way CI does.

- [ ] **Step 1: Run the full CI gate graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green.

If Moon reports an unattributed failure count, diagnose with:

```bash
jq '.actions[] | select(.status=="failed")' .moon/cache/ciReport.json
```

No new crates or dependencies are added by this plan, so `:deny` and `:machete` should need no waivers. `:affected-smoke` should be unaffected — no new crate depends on `paigasus-kernel-rs`.

- [ ] **Step 2: Confirm the whole diff matches the plan**

```bash
git diff origin/main --stat
```

Expected files only: `adapters/redis_conn.rs` (new), `adapters/mod.rs`, the five adapter files, `moon.yml`, `.github/workflows/ci.yml`, the two Prometheus files, `RUNBOOK-observability.md`, `cedar_authorizer.rs`, `authz_acceptance.rs`, and the two spec/plan docs. No stray debug code, no `dbg!`, no commented-out blocks.

- [ ] **Step 3: Commit the plan document itself (if not already committed)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add docs/superpowers/plans/2026-08-02-sma-473-redis-retry-budget.md
git commit -m "docs(repo): implementation plan for the redis retry budget bound (SMA-473)"
```

---

## Notes for the implementer

**Do not skip the red state.** Task 1 Step 2 and Task 4 Step 2 exist because a test that has never failed proves nothing. Task 3 Step 3 applies the same rule to the CI gate.

**The 28.4 s is the headline evidence.** `api_keys::cache::tests::redis_cache_fails_open_when_the_backend_is_unreachable` takes a measured 28.403 s on `main` and should drop to well under 1 s after Task 2. If it does not, the config is not reaching the manager — stop and investigate rather than proceeding.

**What must NOT change.** Every converted function keeps its exact signature and error mapping. The JWKS path stays fail-closed (`AuthnError::Unavailable`) — SMA-473 makes it fail *fast*, never *open*. The authz caches stay fail-open. `AppState::new` still fails fast at boot.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
