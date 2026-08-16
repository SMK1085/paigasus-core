# SMA-521 — iam nextest retries + container-concurrency cap: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `paigasus-iam`'s Docker-backed integration tests a retry budget and a container-concurrency cap that every nextest entry point picks up, and fix the millisecond-scale port-mapping failures that retries alone cannot absorb.

**Architecture:** A new `rs/.config/nextest.toml` carries the policy on `profile.default`, so Moon, the manual command, and a bare `cargo nextest` all read one definition. A new standalone `tests/support/docker.rs` provides a retrying `mapped_port`, replacing 11 unguarded `get_host_port_ipv4().unwrap()` calls. Moon `inputs` are extended so the profile actually busts the two task caches that depend on it.

**Tech Stack:** Rust (edition 2024, rustc 1.95), cargo-nextest 0.9.136, testcontainers 0.27, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-16-sma-521-iam-nextest-retries-design.md`

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- `rs/Cargo.toml:217` sets `[workspace.lints.rust] warnings = "deny"` — **`dead_code` is a hard compile error**. Every item in `tests/support/docker.rs` MUST carry `#[allow(dead_code)]`, because most binaries that include it will use only some of its items.
- Run all cargo commands from `rs/`. Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`nextest` resolve to the repo-pinned versions (shims first).
- `cargo nextest` needs `--no-tests=pass` on targets with no tests.
- Branch is `feature/sma-521-iam-nextest-retries`. Commit messages: Conventional Commits with a workspace scope (`feat(rs):`, `ci(repo):`, `docs(rs):`), subject **lowercase**, ≤100 chars, and **no `#NNN` issue refs in the body** (commitlint rejects them as a malformed footer — write "PR NNN" instead).
- Commits are SSH-signed via 1Password. If a commit fails with `1Password: agent returned an error` or `failed to fill whole buffer`, **stop and ask the user to unlock 1Password** — do not use `--no-verify` and do not skip signing.
- Do NOT run tasks in the background and end your turn waiting on them. Run builds and test suites in the foreground.
- The cap value `max-threads` is floored at **4** (spec D5a) — never set it below 4.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `rs/.config/nextest.toml` | create | The whole retry/concurrency/JUnit policy. One definition, read by every nextest entry point. |
| `rs/crates/services/paigasus-iam/tests/support/docker.rs` | create | Standalone container helpers. Currently only `PortSource` + `mapped_port`. Depends on nothing else in `support/`. |
| `rs/crates/services/paigasus-iam/tests/support_docker_retry.rs` | create | The one binary that unit-tests `mapped_port`'s retry loop, Docker-free. |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | modify | Declares `pub mod docker;`; its own port call site moves to `mapped_port`. |
| 9 other `tests/*.rs` files | modify | Port call sites move to `mapped_port`. |
| `.moon/tasks/rust.yml` | modify | Adds the profile to every Rust `test` task's `inputs`. |
| `moon.yml` | modify | Adds the profile to `repo:nats-permissions`' narrow `inputs`. |
| `.github/workflows/ci.yml` | modify | Uploads the JUnit report as an artifact. |
| `CLAUDE.md`, `docs/dev-setup.md` | modify | Document the policy and the sub-second-suite tell. |

**The 11 port call sites**, split by how they reach the helper:

*Have `mod support;` → use `support::docker::mapped_port`:*
- `tests/authz_acceptance.rs:87` · `tests/api_key_cache_connection.rs:48` · `tests/keycloak_e2e.rs:94` · `tests/authz_system_retirement_pg.rs:413` · `tests/outbox_retention_concurrency_pg.rs:70` and `:160`

*Inside `support/` → use `docker::mapped_port`:*
- `tests/support/mod.rs:94`

*No `mod support;` → add `#[path = "support/docker.rs"] mod docker;` and use `docker::mapped_port`:*
- `tests/redis_jwks_cache.rs:35` · `tests/authz_cache_redis.rs:41` · `tests/authz_generations_redis.rs:42` · `tests/api_key_cache_redis.rs:37`

---

### Task 1: The nextest profile and its Moon input wiring

Lands the policy and makes both dependent Moon tasks re-key on it. No Rust changes.

**Files:**
- Create: `rs/.config/nextest.toml`
- Modify: `.moon/tasks/rust.yml:22-24`
- Modify: `moon.yml` (`repo:nats-permissions` `inputs`, around line 205-211)

**Interfaces:**
- Consumes: nothing.
- Produces: a `docker-containers` test group and a `profile.default` override matching `package(=paigasus-iam) and kind(test)`; a JUnit report at `rs/target/nextest/default/junit.xml` (consumed by Task 5).

- [ ] **Step 1: Create the profile**

Create `rs/.config/nextest.toml`:

```toml
# The retry/concurrency policy for paigasus-iam's Docker-backed integration tests (SMA-521).
#
# Deliberately on `profile.default`, NOT a `[profile.ci]`: a `ci` profile only applies with
# `--profile ci`, which the inherited `.moon/tasks/rust.yml` command
# (`cargo nextest run --no-tests=pass`) does not pass. Putting it on the default profile is what
# makes Moon, the manual command, and a developer's bare `cargo nextest` all read ONE definition.
# Note `cargo test` picks up none of this — nextest config is nextest-only.
nextest-version = { required = "0.9.136" }

# Bounds how many tests in this group start containers concurrently, ON TOP OF the global
# `test-threads` (left at its default so non-container crates keep full parallelism).
#
# Floored at 4 on purpose: CI (`ubuntu-latest`, 4 vCPU on this public repo) already runs only
# ~4-way, so a cap >= 4 is provably a no-op there and cannot serialize the graph into ci.yml's
# 30-minute job timeout. The win is local, where a bare `cargo nextest` otherwise fires 18
# concurrent container startups at an 8-CPU Docker VM.
#
# NOTE this cap is per nextest PROCESS, not global: `moon ci` runs `paigasus-iam-rs:test` and
# `repo:nats-permissions` as separate tasks, each with its own budget.
#
# MEASUREMENTS: see Task 4 — replace this line with the recorded numbers.
[test-groups.docker-containers]
max-threads = 4

[[profile.default.overrides]]
# `=` is an exact-match matcher. Verified that the bare form does NOT match `paigasus-iam-core`
# either, but the explicit form means no reader has to know the matcher's semantics.
# `kind(test)` selects integration-test targets only, so paigasus-iam's own --lib unit tests and
# every other crate keep retries = 0 — a flaky pure-logic test must still red immediately.
filter = 'package(=paigasus-iam) and kind(test)'
# Attempts land at roughly t, t+15s, t+45s. Sized against a MEASURED contention window: see
# tests/nats_publisher.rs:51-58, where a full-suite run that normally takes 3.7s took 33s under
# load. A flat 2s backoff would put all three attempts inside that window and absorb nothing.
retries = { backoff = "exponential", count = 2, delay = "15s", max-delay = "60s", jitter = true }
test-group = 'docker-containers'

[[profile.default.overrides]]
# keycloak_e2e starts a Keycloak with a 240s startup timeout (tests/keycloak_e2e.rs:79). Three
# attempts of a genuinely failing run would be ~18 minutes against ci.yml's 30-minute budget.
filter = 'package(=paigasus-iam) and test(keycloak)'
retries = 1

# `.moon/tasks.yml` sets `taskOptions.outputStyle: 'buffer-only-failure'`, so a task that goes
# green-with-flakes prints NOTHING — the FLAKY signal this design's safety argument depends on is
# otherwise invisible in CI. ci.yml uploads this file as an artifact.
[profile.default.junit]
path = 'junit.xml'
```

- [ ] **Step 2: Verify the profile parses, and prove the check is not vacuous**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest list -p paigasus-kernel --list-type binaries-only
```
Expected: PASS, listing `paigasus-kernel` binaries.

Now prove the validation actually bites — temporarily change `kind(test)` to `kindzz(test)` in the first override and re-run the same command.
Expected: FAIL with `failed to parse profile.default.overrides at index 0` and a caret pointing at `kindzz`.
**Restore `kind(test)` before continuing.**

- [ ] **Step 3: Verify the override matches exactly the intended set**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
# must be 0 — no other crate may inherit the retry budget
cargo nextest list -E 'package(=paigasus-iam) and kind(test)' 2>/dev/null | grep -c 'paigasus-iam-core'
# control: proves the grep above would have found them if they matched
cargo nextest list 2>/dev/null | grep -c 'paigasus-iam-core'
```
Expected: first command prints `0`; second prints a number well above 0 (~116). If the control prints 0, the check is vacuous — stop and investigate.

- [ ] **Step 4: Add the profile to every Rust test task's inputs**

In `.moon/tasks/rust.yml`, replace the `test` task:

```yaml
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
```

with:

```yaml
  test:
    command: 'cargo nextest run --no-tests=pass'
    # `/rs/.config/nextest.toml` is workspace-relative (leading `/`). Required because `rs/` is
    # NOT a Moon project — `.moon/workspace.yml` globs cover only `rs/crates/{libs,bindings,
    # services}/*` — so the profile is outside every project's project-relative inputs and
    # `implicitInputs` covers only `.moon/*`. Without this, editing the retry policy would not
    # bust this task's cache (stale PASS) and a profile-only PR would not select it under
    # `moon ci --affected`.
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml', '/rs/.config/nextest.toml']
```

- [ ] **Step 5: Add the profile to `repo:nats-permissions` inputs**

`repo:nats-permissions` is a `language: 'bash'` task on the root `repo` project, so `.moon/tasks/rust.yml`'s `inheritedBy: languages: ['rust']` does **not** attach to it — yet its script runs `cargo nextest run -p paigasus-iam --test nats_permissions`, which the new override matches.

In `moon.yml`, in the `nats-permissions` task's `inputs` list, add one entry after `'rs/Cargo.lock'`:

```yaml
      - 'rs/Cargo.lock'
      #   - 'rs/.config/nextest.toml' — this task runs its own `cargo nextest` against
      #     -p paigasus-iam, which the SMA-521 retry/test-group override matches. Without this
      #     input a profile edit would leave this gate serving a cached PASS. (It is NOT
      #     inherited from .moon/tasks/rust.yml: that file is `inheritedBy: languages: ['rust']`
      #     and this task is `language: 'bash'` on the root `repo` project.)
      - 'rs/.config/nextest.toml'
```

Also extend the existing comment block above `inputs` so the per-entry list documents it, matching the file's established style.

Do **not** add it to `repo:observability-drift`: that task runs `-p paigasus-observability`, which the override's filter does not match, so the profile cannot change its result and the input would only cause needless cache busts.

- [ ] **Step 6: Prove both tasks now re-key on the profile**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
touch rs/.config/nextest.toml
moon query tasks --affected
```
Expected: output includes **both** `paigasus-iam-rs:test` and `repo:nats-permissions`.

Then prove the check is not vacuous — `git stash` the `.moon/tasks/rust.yml` change only, re-run, and confirm `paigasus-iam-rs:test` is **absent**; restore it afterwards. (Use `git stash push -u -m "sma521-vacuity-check" -- .moon/tasks/rust.yml`, capture the SHA with `git stash list --format='%H %gs'`, restore with `git stash apply <sha>`, then drop that entry by tag — the stash stack is shared with other worktrees, so never use bare `git stash pop`.)

- [ ] **Step 7: Commit**

```bash
git add rs/.config/nextest.toml .moon/tasks/rust.yml moon.yml
git commit -m "feat(rs): add a nextest retry budget and container-concurrency cap for iam (SMA-521)"
```

---

### Task 2: `mapped_port` — a retrying port lookup

TDD. The test binary comes first and must fail before `docker.rs` exists.

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/support_docker_retry.rs`
- Create: `rs/crates/services/paigasus-iam/tests/support/docker.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces, for Task 3:
  - `pub trait PortSource { fn host_port(&self, port: u16) -> impl Future<Output = Result<u16, String>> + Send; }`
  - `pub async fn mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16`
  - `impl<I: Image> PortSource for ContainerAsync<I>` — so call sites pass `&node` directly.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/support_docker_retry.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Docker-free unit tests for `support::docker::mapped_port`'s retry loop (SMA-521).
//!
//! Lives in its OWN test binary, included via `#[path]`, for two reasons. A `#[cfg(test)]`
//! module inside `docker.rs` would be silently compiled out — `cfg(test)` is not enabled when
//! rustc builds an integration-test binary — and would therefore never run. Plain
//! `#[tokio::test]` functions inside `docker.rs` would instead run once per binary that
//! includes it, duplicating the same assertions ~11 times.

#[path = "support/docker.rs"]
mod docker;

use docker::{PortSource, mapped_port};
use std::sync::atomic::{AtomicU32, Ordering};

/// Fails its first `fails` probes, then reports `port` — the shape of a container whose runtime
/// has not yet published the host-side mapping (`PortNotExposed`).
struct FlakyPort {
    remaining_failures: AtomicU32,
    port: u16,
}

impl PortSource for FlakyPort {
    fn host_port(&self, _port: u16) -> impl std::future::Future<Output = Result<u16, String>> + Send {
        let left = self.remaining_failures.load(Ordering::SeqCst);
        let result = if left > 0 {
            self.remaining_failures.store(left - 1, Ordering::SeqCst);
            Err("PortNotExposed".to_string())
        } else {
            Ok(self.port)
        };
        async move { result }
    }
}

#[tokio::test]
async fn mapped_port_retries_until_the_mapping_is_published() {
    let src = FlakyPort { remaining_failures: AtomicU32::new(3), port: 54321 };

    let port = mapped_port(&src, 6379, "flaky test source").await;

    assert_eq!(port, 54321, "must return the port once the source finally reports it");
    assert_eq!(src.remaining_failures.load(Ordering::SeqCst), 0, "must have consumed every simulated failure");
}

#[tokio::test]
async fn mapped_port_returns_immediately_when_the_mapping_is_already_published() {
    let src = FlakyPort { remaining_failures: AtomicU32::new(0), port: 5432 };

    let port = mapped_port(&src, 5432, "ready test source").await;

    assert_eq!(port, 5432);
}
```

- [ ] **Step 2: Run to verify it fails**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test support_docker_retry
```
Expected: FAIL — a compile error, `couldn't read .../tests/support/docker.rs` (the file does not exist yet).

- [ ] **Step 3: Write the implementation**

Create `rs/crates/services/paigasus-iam/tests/support/docker.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Standalone container helpers for the integration suites (SMA-521).
//!
//! Deliberately depends on NOTHING else in `support/`, so the four Redis-only test files that
//! have no `mod support;` can pull it in with `#[path = "support/docker.rs"] mod docker;`
//! without dragging in the 791-line support surface (axum, rcgen, the mock IdP) — and without
//! tripping the `dead_code` hard error that `[workspace.lints.rust] warnings = "deny"` makes of
//! `support/mod.rs`'s two non-`#[allow(dead_code)]` items.
//!
//! Every item here carries `#[allow(dead_code)]` for that same reason: most binaries that
//! include this file use only part of it.

use std::time::Duration;
use testcontainers::core::ContainerAsync;
use testcontainers::Image;

/// How long [`mapped_port`] waits for the container runtime to publish a host-side port mapping.
/// A LOAD BUDGET, not an expectation — it returns on the first success, which on an idle machine
/// is immediate. Matches the 90s ceiling `tests/nats_publisher.rs` already uses for the same race.
#[allow(dead_code)]
const PORT_READY_BUDGET: Duration = Duration::from_secs(90);

/// Anything that can report a host-side port for a container port.
///
/// Exists so [`mapped_port`]'s retry loop can be tested without Docker: production code uses the
/// `ContainerAsync<I>` impl below, and `tests/support_docker_retry.rs` substitutes a counter that
/// fails a fixed number of times first.
#[allow(dead_code)]
pub trait PortSource {
    fn host_port(&self, port: u16) -> impl std::future::Future<Output = Result<u16, String>> + Send;
}

impl<I: Image> PortSource for ContainerAsync<I> {
    fn host_port(&self, port: u16) -> impl std::future::Future<Output = Result<u16, String>> + Send {
        async move { self.get_host_port_ipv4(port).await.map_err(|e| e.to_string()) }
    }
}

/// Resolves a container's mapped host port, retrying until the runtime publishes it.
///
/// **Why this is not a bare `get_host_port_ipv4(..).unwrap()`** (which is what it replaced at 11
/// sites): `AsyncRunner::start` returns once the server has logged that it is listening, but the
/// runtime publishes the host-side port mapping independently — an inspect issued in that gap
/// comes back `PortNotExposed`. It is rare for one container and reproducible when the suite
/// races many of them (`tests/nats_publisher.rs:46-50` documents the same race).
///
/// This is the FAST failure class of SMA-521: it fails in milliseconds, so a nextest retry
/// budget cannot absorb it — all attempts land inside the same contention burst. Retrying here,
/// where the race actually is, is the fix; the retry budget is the backstop.
///
/// Panics after [`PORT_READY_BUDGET`] so a genuinely missing port still fails loudly.
#[allow(dead_code)]
pub async fn mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16 {
    let deadline = std::time::Instant::now() + PORT_READY_BUDGET;
    loop {
        match src.host_port(port).await {
            Ok(mapped) => return mapped,
            Err(e) if std::time::Instant::now() >= deadline => {
                panic!("{what}: container port {port} was never published within {PORT_READY_BUDGET:?}: {e}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}
```

If `testcontainers::core::ContainerAsync` does not resolve, use the path the existing tests use — `testcontainers::ContainerAsync` (see `tests/nats_permissions.rs:32`). `rs/Cargo.lock` resolves exactly one `testcontainers 0.27.3`, so this is the same type the `testcontainers_modules::testcontainers::` re-export names.

- [ ] **Step 4: Run to verify it passes**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test support_docker_retry
```
Expected: PASS, 2 tests.

- [ ] **Step 5: Prove the retry test is not vacuous**

Temporarily change `mapped_port`'s `Err(_) => tokio::time::sleep(..)` arm to `Err(e) => panic!("no retry: {e}")` and re-run Step 4.
Expected: `mapped_port_retries_until_the_mapping_is_published` FAILS; `mapped_port_returns_immediately_...` still passes.
**Restore the retry arm and re-run to confirm both pass again.**

Note: `cargo` decides freshness by mtime. If you restore by copying a `.bak` file over the original, the mtime rolls *backwards* and cargo will reuse the binary built from your temporary edit — making the restored run look wrong. Restore by editing the file directly (or `touch` it afterwards).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/support/docker.rs \
        rs/crates/services/paigasus-iam/tests/support_docker_retry.rs
git commit -m "feat(rs): add a retrying mapped_port helper for container port lookups (SMA-521)"
```

---

### Task 3: Migrate the 11 unguarded port call sites

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (add `pub mod docker;`; site at `:94`)
- Modify: `tests/authz_acceptance.rs:87`, `tests/api_key_cache_connection.rs:48`, `tests/keycloak_e2e.rs:94`, `tests/authz_system_retirement_pg.rs:413`, `tests/outbox_retention_concurrency_pg.rs:70` and `:160`
- Modify: `tests/redis_jwks_cache.rs:35`, `tests/authz_cache_redis.rs:41`, `tests/authz_generations_redis.rs:42`, `tests/api_key_cache_redis.rs:37`

**Interfaces:**
- Consumes: `mapped_port` and `PortSource` from Task 2.
- Produces: nothing new.

- [ ] **Step 1: Declare the module in `support/mod.rs`**

In `rs/crates/services/paigasus-iam/tests/support/mod.rs`, immediately after the `use` block (before the `start_migrated_postgres` doc comment at line 61), add:

```rust
/// Standalone container helpers — see `support/docker.rs`. Declared `pub` so the ~52 files that
/// carry `mod support;` reach it as `support::docker::*`; the four Redis-only files that have no
/// `mod support;` include the same file directly via `#[path = "support/docker.rs"]`.
pub mod docker;
```

- [ ] **Step 2: Migrate the site inside `support/mod.rs`**

Replace line 94:

```rust
    let port = pg.get_host_port_ipv4(5432).await.expect("mapped postgres port");
```

with:

```rust
    let port = docker::mapped_port(pg, 5432, "postgres").await;
```

- [ ] **Step 3: Migrate the five sites in files that already have `mod support;`**

`tests/authz_acceptance.rs:87`, `tests/api_key_cache_connection.rs:48` — replace:
```rust
    let port = node.get_host_port_ipv4(6379).await.unwrap();
```
with:
```rust
    let port = support::docker::mapped_port(&node, 6379, "redis").await;
```

`tests/authz_system_retirement_pg.rs:413` — replace:
```rust
    let port = container.get_host_port_ipv4(5432).await.unwrap();
```
with:
```rust
    let port = support::docker::mapped_port(container, 5432, "postgres (second connection)").await;
```

`tests/outbox_retention_concurrency_pg.rs:70` and `:160` — replace each:
```rust
    let port = node.get_host_port_ipv4(5432).await.unwrap();
```
with:
```rust
    let port = support::docker::mapped_port(&node, 5432, "postgres (lock holder)").await;
```

`tests/keycloak_e2e.rs:94` — replace:
```rust
    let https_port = keycloak.get_host_port_ipv4(HTTPS_PORT).await.expect("mapped https port");
```
with:
```rust
    let https_port = support::docker::mapped_port(&keycloak, HTTPS_PORT, "keycloak https").await;
```

Note the receiver: pass `&node` where the binding is owned, and `container` / `node` directly where it is already a reference (`authz_system_retirement_pg.rs`'s `container: &ContainerAsync<Postgres>`). If a borrow does not typecheck, adjust the `&` — `mapped_port` takes `&impl PortSource`.

- [ ] **Step 4: Migrate the four files with no `mod support;`**

For each of `tests/redis_jwks_cache.rs`, `tests/authz_cache_redis.rs`, `tests/authz_generations_redis.rs`, `tests/api_key_cache_redis.rs`:

Add, immediately after the `use` block at the top of the file:

```rust
// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-521).
#[path = "support/docker.rs"]
mod docker;
```

Then replace the site:
```rust
    let port = node.get_host_port_ipv4(6379).await.unwrap();
```
with:
```rust
    let port = docker::mapped_port(&node, 6379, "redis").await;
```

- [ ] **Step 5: Verify no unguarded sites remain**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-521/rs/crates/services/paigasus-iam/tests
grep -rn "get_host_port_ipv4" . | grep -v "docker.rs" | grep "unwrap()\|expect("
```
Expected: **no output**. Any remaining line is an unmigrated site.

- [ ] **Step 6: Verify it compiles and lints clean**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```
Expected: PASS. A `dead_code` error here means an item in `docker.rs` is missing `#[allow(dead_code)]`.

- [ ] **Step 7: Run the full iam suite**

Docker must be running. Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```
`CI=1` is essential: without it, every Docker-backed test silently returns and "passes" in under a second having run nothing.

Expected: PASS. If tests fail, first re-run the same command on unmodified `origin/main` to establish whether the failure is pre-existing flakiness rather than this diff — this suite is known to fail a different random subset each run under load.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/
git commit -m "feat(rs): retry container port lookups at all 11 unguarded call sites (SMA-521)"
```

---

### Task 4: Measure and finalize the concurrency cap

**Files:**
- Modify: `rs/.config/nextest.toml` (the `max-threads` line and its `MEASUREMENTS` comment)

**Interfaces:**
- Consumes: the profile from Task 1.
- Produces: a final `max-threads` value with its evidence recorded in-file.

- [ ] **Step 1: Establish the uncapped baseline**

Do **not** check out `main` to get this baseline. This is a git worktree sharing one `.git` with the primary checkout and other active sessions; switching branches here can reparent another session's work.

Get the uncapped number on this branch instead, by temporarily commenting out the group assignment in `rs/.config/nextest.toml`:

```toml
# test-group = 'docker-containers'
```

With that line commented, the retry budget still applies but no concurrency cap does — which is exactly the "uncapped" baseline. Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && time CI=1 cargo nextest run -p paigasus-iam
```

Record wall-clock and any failures. Run it **twice** and keep both numbers — this suite is known to fail a different random subset each run, so a single sample is noise.

**Restore the `test-group` line before Step 2.**

- [ ] **Step 2: Time the capped configurations**

Back on the feature branch, for each of `max-threads = 4`, `6`, `8` in `rs/.config/nextest.toml`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && time CI=1 cargo nextest run -p paigasus-iam
```
Use `cargo nextest` directly, never `moon run` — Moon's task cache would serve a previous timing instead of re-running. Run the finalists twice each.

- [ ] **Step 3: Choose the value**

Pick the **lowest cap ≥ 4** whose wall-clock stays within ~30% of the Step 1 baseline. If even 4 is within 30%, keep 4.

- [ ] **Step 4: Record the evidence in the file**

Replace the `# MEASUREMENTS: see Task 4 — replace this line with the recorded numbers.` line with the real numbers, e.g.:

```toml
# MEASURED 2026-08-16 on a 18-logical-CPU macOS host with an 8-CPU / 8 GB Docker VM,
# `CI=1 cargo nextest run -p paigasus-iam`, two runs each:
#   uncapped (18-way): <t0a>s / <t0b>s
#   max-threads = 8:   <t8a>s / <t8b>s
#   max-threads = 6:   <t6a>s / <t6b>s
#   max-threads = 4:   <t4a>s / <t4b>s
# Chose <N>: the lowest cap >= 4 (D5a's floor) within ~30% of the uncapped baseline.
# Re-derive by repeating the above; the optimum tracks Docker VM capacity, not host CPU count.
```

- [ ] **Step 5: Commit**

```bash
git add rs/.config/nextest.toml
git commit -m "perf(rs): pin the iam container-concurrency cap to a measured value (SMA-521)"
```

---

### Task 5: Upload the JUnit report from CI

Without this the `FLAKY` signal is invisible: `.moon/tasks.yml:25-26` sets `outputStyle: 'buffer-only-failure'`, so a green-with-flakes task prints nothing.

**Files:**
- Modify: `.github/workflows/ci.yml` (after the `moon ci (affected graph)` step, around line 192)

**Interfaces:**
- Consumes: the `[profile.default.junit]` setting from Task 1, which writes `rs/target/nextest/default/junit.xml`.
- Produces: a CI artifact named `nextest-junit`.

- [ ] **Step 1: Add the upload step**

In `.github/workflows/ci.yml`, immediately after the `moon ci (affected graph)` step and before `Codegen drift gate`, insert:

```yaml
      # `.moon/tasks.yml` sets outputStyle: 'buffer-only-failure', so a test task that goes
      # green-WITH-FLAKES prints nothing at all — the only signal that the SMA-521 retry budget
      # absorbed something. Without this artifact there is no way to notice a test degrading from
      # "rare container-startup flake" into a genuine ~50%-failure regression. `if: always()` so
      # it also uploads when the graph fails, which is exactly when it matters most.
      - name: Upload nextest JUnit report (flaky-test visibility)
        if: always()
        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02  # v4.6.2
        with:
          name: nextest-junit
          path: rs/target/nextest/default/junit.xml
          if-no-files-found: ignore
          retention-days: 14
```

`if-no-files-found: ignore` matters: `moon ci` is affected-graph driven, so a PR that touches no Rust at all never runs a test task and produces no report — that must not fail the job.

- [ ] **Step 2: Verify the report is actually produced at that path**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam --test support_docker_retry
ls -l target/nextest/default/junit.xml
```
Expected: the file exists. If the path differs, correct the workflow's `path:` to match what nextest actually wrote — do not assume.

- [ ] **Step 3: Verify the workflow YAML is well-formed**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint 2>/dev/null || python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('yaml ok')"
```
Expected: no errors. (A `repo:actionlint` gate may not exist on this branch — the YAML parse is the fallback.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(repo): upload the nextest junit report so flaky reruns stay visible (SMA-521)"
```

---

### Task 6: Documentation (AC3)

**Files:**
- Modify: `CLAUDE.md` (Gotchas section, after the Bash-tool-PATH bullet ending at line 89)
- Modify: `docs/dev-setup.md` (the `## Gotchas (verified during the gate run)` section)

**Interfaces:** none.

- [ ] **Step 1: Add the CLAUDE.md gotcha**

In `CLAUDE.md`, after the `Bash tool PATH lacks the proto-managed CLIs` bullet and before `## Workflow`, add:

```markdown
- `paigasus-iam`'s Docker-backed suites get their retry budget and container-concurrency cap from
  `rs/.config/nextest.toml` (`profile.default`), so **Moon, `moon run …:test`, and a bare
  `cargo nextest` all pick it up** — but `cargo test` does NOT, since nextest config is
  nextest-only. Don't add `--retries` to a Moon task or a doc: that recreates the
  documented-vs-executed split SMA-521 closed. A test that fails every attempt still reds; one
  that passes on a retry is reported FLAKY, and CI uploads the JUnit report as the
  `nextest-junit` artifact because `outputStyle: 'buffer-only-failure'` otherwise prints nothing
  on a green-with-flakes run.
- The iam suite **silently skips** without Docker: `support::start_migrated_postgres()` returns
  `None` and each test `return`s, reporting a PASS in under a second having run nothing (nextest's
  skip count does not reveal it, because stderr from a *passing* test is discarded —
  `success-output` defaults to `never`). The tell is the clock: a `paigasus-iam` suite that
  finishes in ~1s skipped, it did not pass. Always verify with `CI=1 cargo nextest run -p
  paigasus-iam`, which makes a missing daemon a hard failure. SMA-538 tracks fixing this properly.
```

- [ ] **Step 2: Add the dev-setup.md note**

In `docs/dev-setup.md`, under `## Gotchas (verified during the gate run)`, add:

```markdown
- **The `paigasus-iam` integration suites need Docker, and say nothing when they don't have it.**
  Without a daemon each test returns early and reports a pass in under a second. Run them as
  `CI=1 cargo nextest run -p paigasus-iam` — `CI=1` turns a missing daemon into a hard failure, so
  you find out immediately instead of trusting a green run that executed nothing.
- Retries and the container-concurrency cap for those suites live in `rs/.config/nextest.toml`, so
  they apply to `moon run`, to `cargo nextest` typed by hand, and to anything else that shells out
  to nextest. `cargo test` bypasses them entirely — prefer `cargo nextest` in this repo.
```

- [ ] **Step 3: Verify the claims are true**

Run each documented command and confirm it behaves as written:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam --test health
```
Expected: PASS having actually started a container (takes seconds, not milliseconds). Docs that state something untrue are worse than no docs.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/dev-setup.md
git commit -m "docs(repo): document the iam nextest retry policy and the silent-skip tell (SMA-521)"
```

---

### Task 7: Full-graph verification

Per `CLAUDE.md`, per-project tasks do NOT run the repo-level gates, and this change adds new files plus `.moon/tasks/rust.yml` and `moon.yml` edits.

**Files:** none modified unless a gate fails.

- [ ] **Step 1: Run the full affected graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts :publish-metadata --base origin/main --include-relations
```
Expected: all green.

- [ ] **Step 2: If Moon reports an unattributed failure, find it**

Moon's summary often says only "N failed". Identify the task with:
```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm the acceptance criteria hold**

- AC1 — `cargo nextest show-config test-groups` lists the iam integration tests under `docker-containers`, and the retry budget is on the same override.
- AC2 — the same profile is read whether nextest is invoked from `rs/` or from `rs/crates/services/paigasus-iam/`; both `paigasus-iam-rs:test` and `repo:nats-permissions` appear in `moon query tasks --affected` after touching the profile.
- AC3 — `CLAUDE.md`'s documented command is `CI=1 cargo nextest run -p paigasus-iam`, with no `--retries` flag anywhere, because the profile supplies it.

- [ ] **Step 4: Confirm no stray debug code**

```bash
git diff origin/main --stat
git diff origin/main | grep -n "dbg!\|println!(\"DEBUG\|TODO\|FIXME" || echo "clean"
```
Expected: `clean`, and the changed-file list matches this plan's File Structure table.

---

## Self-Review

**Spec coverage.** §1 profile → Task 1. §2 cap → Tasks 1 and 4. §3 `mapped_port` → Tasks 2 and 3. §4 Moon wiring → Task 1 (both tasks). §5 measurement → Task 4. §6 verification → verification steps distributed through every task, plus Task 7. §7 docs → Task 6. §8 rollback → not a code change; the revert path is stated in the spec. Deferred skip-policy work → SMA-538, correctly absent here.

**Placeholders.** The only intentionally unfilled values are Task 4's `<t0a>`/`<N>` measurement placeholders, which that task's own steps fill with real numbers — the procedure is fully specified.

**Type consistency.** `PortSource::host_port(&self, port: u16) -> impl Future<Output = Result<u16, String>> + Send` is defined in Task 2 Step 3 and used unchanged in Task 2 Step 1's stub and all of Task 3's call sites. `mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16` is called with three arguments everywhere.
