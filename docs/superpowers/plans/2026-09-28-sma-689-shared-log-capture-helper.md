# SMA-689 Shared Log-Capture Helper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the four copies of the test log-capture helper (`LogBuffer` + `capture_logs`) with one helper in `paigasus_logging::test_support`, behind a `test-support` feature that the services turn on from `[dev-dependencies]` only.

**Architecture:** `paigasus-logging` gets a `[features] test-support = []` table and a module `test_support`, compiled under `#[cfg(any(test, feature = "test-support"))]`. The module holds the C3 code made `pub`, plus `capture_logs_at(level)`. `paigasus-gateway` and `paigasus-iam` list `paigasus-logging` a second time under `[dev-dependencies]` with the feature, delete their copies, import the shared helper, and drop their `tracing-subscriber` dev-dependency.

**Tech Stack:** Rust edition 2024 (rust-version 1.95), `tracing` 0.1, `tracing-subscriber` 0.3.23, Cargo resolver 3, `cargo nextest`, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-689-shared-log-capture-helper-design.md` (approved by Sven on 2026-09-28; section 12 holds the approval decisions A1-A7). Read it with this plan.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-689-shared-log-capture-helper`, branch `feature/sma-689-shared-log-capture-helper`. Start every Bash command with `cd <worktree> &&`. Before each commit, `git branch --show-current` must print `feature/sma-689-shared-log-capture-helper`.
- Prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Commit type and scope: `test(rs)` (spec section 8). End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` or `token: value` line in the commit body.
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, or a force push. If signing fails with "failed to fill whole buffer", 1Password is locked: stop and report a blocker.
- `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` after each edit and accept its import order.
- Workspace lints: `warnings = "deny"`, `clippy::all = "warn"`, so every warning is an error.
- No new external dependency (AC 3). `tracing` and `tracing-subscriber` are already normal dependencies of `paigasus-logging`.
- The service dev-dependency line is exactly `paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }`. The `version = "0.0.0"` is required: `rs/deny.toml:51` sets `wildcards = "deny"`.
- `capture_logs()` captures at `TRACE`. `chat_proxy.rs` calls `capture_logs_at(tracing::Level::INFO)` (A2).
- `LogBuffer` is `pub` and carries `#[doc(hidden)]` (A5). Keep the `(LogBuffer, DefaultGuard)` tuple (A6).
- No `repo:*` gate against a new `LogBuffer` copy (A3). No Notion entry (A4).
- Callers import from `paigasus_logging::test_support` directly. No local re-export or wrapper (D5).
- Mutation runs: revert the exact edit with the Edit tool, never with `git checkout --`. Confirm with `git diff`. Never commit a mutation. Each mutation must compile.
- Do not install host software. Do not leave background jobs running.

## Review Focus

1. **A log event from another OS thread.** The helper installs a thread-local subscriber, so a line logged on another thread is not captured. A reader expects the module doc to say so, and a test to pin it. Task 1 adds `other_thread_is_not_captured`.
2. **A nested capture.** A test (or a helper it calls) can start a second capture while one is active. A reader expects the inner guard drop to restore the outer capture, and the two buffers to stay separate. Task 1 adds `nested_capture_restores_the_outer_capture`.
3. **`let (logs, _) = capture_logs();`.** No lint catches it (spec D4). The doc rule says the capture stops at once. Task 1 adds `underscore_guard_captures_nothing`, so the doc rule stays true.
4. **A non-ASCII field value.** `text()` calls `String::from_utf8(..).unwrap()`. A reader expects a line with `ü` or an emoji to come back intact, not to panic. Task 1 adds `non_ascii_text_round_trips`.
5. **The feature leaks into the production binary.** A later edit could move the feature to `[dependencies]`. Task 4 proves AC 6 with `cargo tree -e features,no-dev` and the real `rustc` line; no unit test can see this.

---

### Task 1: The shared helper in `paigasus-logging`

**Files:**
- Modify: `rs/crates/libs/paigasus-logging/Cargo.toml`
- Modify: `rs/crates/libs/paigasus-logging/src/lib.rs` (crate doc, module declaration)
- Create: `rs/crates/libs/paigasus-logging/src/test_support.rs` (helper plus eight unit tests)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (Tasks 2 and 3 rely on these exact names):
  - feature `test-support` of package `paigasus-logging`
  - `paigasus_logging::test_support::LogBuffer` with `pub fn text(&self) -> String`
  - `paigasus_logging::test_support::capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard)` (TRACE)
  - `paigasus_logging::test_support::capture_logs_at(level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard)`

- [ ] **Step 1: Add the feature to `Cargo.toml`**

In `rs/crates/libs/paigasus-logging/Cargo.toml`, insert this block between the `[dependencies]` table and `[lints]`:

```toml
[features]
# Test-only log capture (`test_support` module). Turn it on from [dev-dependencies] only (SMA-689).
test-support = []
```

- [ ] **Step 2: Declare the module in `src/lib.rs`**

In `rs/crates/libs/paigasus-logging/src/lib.rs`, add one paragraph at the end of the crate doc (after the line that ends `the first consumer is \`paigasus-iam\`).`):

```rust
//!
//! The `test-support` feature adds the `test_support` module, a log-capture helper for tests
//! that assert on a log line. Turn it on from `[dev-dependencies]` only (SMA-689).
```

Then, directly after the `use tracing_subscriber::{EnvFilter, fmt, prelude::*};` line, add:

```rust

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
```

The crate doc names the module as plain code, not as a rustdoc link: the module does not exist in a build without the feature, so a link would break there.

- [ ] **Step 3: Write the module with the failing tests first**

Create `rs/crates/libs/paigasus-logging/src/test_support.rs` with the tests and stub functions that do nothing useful yet, so the tests compile and fail:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Test-only log capture for tests that assert on a `tracing` line (SMA-689).
//!
//! This is the one copy of this helper in the workspace. Do not write a new `LogBuffer` in a
//! service crate. Turn the helper on from `[dev-dependencies]` only, so that the production
//! binary never contains it:
//!
//! ```toml
//! [dev-dependencies]
//! paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }
//! ```
//!
//! # Usage rules
//!
//! - Bind the guard to a named variable: `let (logs, _guard) = capture_logs();`. Never bind it
//!   to `_`: `let (logs, _) = capture_logs();` drops the guard at once, and the capture then
//!   sees nothing. No lint catches this.
//! - The subscriber is thread-local. `#[tokio::test]` is current-thread, so the subscriber sees
//!   every task of the test, spawned tasks included. A `multi_thread` runtime test, or a line
//!   logged on another OS thread, is NOT captured.
//! - [`capture_logs`] captures at `TRACE`. A "this is never logged" assertion is sound only at
//!   `TRACE`, because a lower level hides the lines that it does not capture.
//!
//! # Limits
//!
//! - **Supported runner: `cargo nextest`** (one process per test). `tracing` caches callsite
//!   interest for the whole process. Under `cargo test` (one process, many test threads) a
//!   log assertion can miss a line and flake. The repo's Moon `test` task and CI use nextest.
//! - The captured text is the `fmt` text format with no ANSI escapes. It is NOT the JSON line
//!   format that [`crate::init`] writes in production, so captured text is not evidence of the
//!   production line format.

use std::sync::{Arc, Mutex};

/// A `tracing` writer that keeps every formatted line in memory. An implementation detail of
/// [`capture_logs`]; read it with [`LogBuffer::text`] only.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuffer {
    /// Everything written so far, as text.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Installs a thread-local subscriber at `TRACE` and returns its buffer and guard. Bind the
/// guard to a named variable (`_guard`), never to `_`. See the module doc for the limits.
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    // STUB for Step 4: replaced in Step 5.
    capture_logs_at(tracing::Level::ERROR)
}

/// Installs a thread-local subscriber at `level` and returns its buffer and guard. Bind the
/// guard to a named variable (`_guard`), never to `_`. See the module doc for the limits.
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs_at(level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    // STUB for Step 4: replaced in Step 5.
    let _ = level;
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_max_level(tracing::Level::ERROR).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}

#[cfg(test)]
mod tests {
    use super::{capture_logs, capture_logs_at};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use tracing::Level;

    // Every test here installs a scoped subscriber, and one test mutates `NO_COLOR`. Under
    // `cargo test` (thread-parallel) that can flake (see the module doc), so every test holds
    // this lock for its whole body. `cargo nextest` does not need it; it is harmless there. A
    // panicking test poisons the lock; take it anyway, so one failure does not red every test.
    static LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn capture_logs_sees_trace() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        tracing::trace!("probe-trace");
        assert!(logs.text().contains("probe-trace"), "capture_logs must capture TRACE: {:?}", logs.text());
    }

    #[test]
    fn capture_logs_at_info_drops_debug() {
        let _serial = serial();
        let (logs, _guard) = capture_logs_at(Level::INFO);
        tracing::debug!("d-probe");
        tracing::info!("i-probe");
        let text = logs.text();
        assert!(text.contains("i-probe"), "INFO must be captured: {text:?}");
        assert!(!text.contains("d-probe"), "DEBUG must not be captured at INFO: {text:?}");
    }

    #[test]
    fn capture_has_no_ansi_escape() {
        let _serial = serial();
        // `tracing-subscriber` 0.3.23 turns ANSI on by default (the `ansi` feature) unless
        // `NO_COLOR` is set. Remove it, so this test proves `with_ansi(false)` and not the
        // runner's environment.
        // SAFETY: `serial()` holds the lock that every test of this module takes; no other
        // test of this crate reads `NO_COLOR`.
        unsafe { std::env::remove_var("NO_COLOR") };
        let (logs, _guard) = capture_logs();
        tracing::info!("ansi-probe");
        let text = logs.text();
        assert!(text.contains("ansi-probe"), "the line must be captured: {text:?}");
        assert!(!text.contains('\u{1b}'), "the capture must have no ANSI escape: {text:?}");
    }

    #[test]
    fn guard_drop_stops_capture() {
        let _serial = serial();
        let (logs, guard) = capture_logs();
        tracing::info!("before-drop");
        drop(guard);
        tracing::info!("after-drop");
        let text = logs.text();
        assert!(text.contains("before-drop"), "a line before the drop is captured: {text:?}");
        assert!(!text.contains("after-drop"), "a line after the drop is not captured: {text:?}");
    }

    // Review Focus 3: the module doc says `_` drops the guard at once. Pin that.
    #[test]
    fn underscore_guard_captures_nothing() {
        let _serial = serial();
        let (logs, _) = capture_logs();
        tracing::info!("underscore-probe");
        assert!(!logs.text().contains("underscore-probe"), "a `_` guard drops at once: {:?}", logs.text());
    }

    // Review Focus 1: the subscriber is thread-local.
    #[test]
    fn other_thread_is_not_captured() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        std::thread::spawn(|| tracing::info!("other-thread-probe")).join().unwrap();
        tracing::info!("this-thread-probe");
        let text = logs.text();
        assert!(text.contains("this-thread-probe"), "this thread is captured: {text:?}");
        assert!(!text.contains("other-thread-probe"), "another thread is not captured: {text:?}");
    }

    // Review Focus 2: an inner capture restores the outer one when its guard drops.
    #[test]
    fn nested_capture_restores_the_outer_capture() {
        let _serial = serial();
        let (outer, _outer_guard) = capture_logs();
        {
            let (inner, _inner_guard) = capture_logs_at(Level::INFO);
            tracing::info!("inner-probe");
            assert!(inner.text().contains("inner-probe"), "inner capture: {:?}", inner.text());
        }
        tracing::info!("outer-probe");
        let text = outer.text();
        assert!(text.contains("outer-probe"), "the outer capture is active again: {text:?}");
        assert!(!text.contains("inner-probe"), "the inner line went only to the inner buffer: {text:?}");
    }

    // Review Focus 4: `text()` decodes UTF-8; a non-ASCII value must round-trip.
    #[test]
    fn non_ascii_text_round_trips() {
        let _serial = serial();
        let (logs, _guard) = capture_logs();
        tracing::info!(name = "Jürgen 🚀", "unicode-probe");
        let text = logs.text();
        assert!(text.contains("unicode-probe") && text.contains("Jürgen 🚀"), "non-ASCII survives: {text:?}");
    }
}
```

- [ ] **Step 4: Run the tests and verify that they fail**

Run: `cd <worktree>/rs && cargo nextest run --locked -p paigasus-logging --no-fail-fast`
Expected: FAIL. At least `capture_logs_sees_trace`, `capture_logs_at_info_drops_debug`, `capture_has_no_ansi_escape`, `guard_drop_stops_capture`, `other_thread_is_not_captured`, `nested_capture_restores_the_outer_capture` and `non_ascii_text_round_trips` fail, because the stub captures only `ERROR` (and `capture_has_no_ansi_escape` sees no line). `underscore_guard_captures_nothing` passes; that is correct. The three `env_filter_*` tests pass.

- [ ] **Step 5: Write the real implementation**

In `test_support.rs`, replace the two stub function bodies:

```rust
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    capture_logs_at(tracing::Level::TRACE)
}
```

```rust
#[must_use = "bind the guard to a named variable, or the capture stops at once"]
pub fn capture_logs_at(level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(level).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}
```

Keep the doc comments above each function as in Step 3. Remove both `// STUB` comments and the `let _ = level;` line.

- [ ] **Step 6: Run the tests, lint and format**

Run:
```bash
cd <worktree>/rs && cargo fmt -p paigasus-logging && cargo nextest run --locked -p paigasus-logging --no-fail-fast
cargo test --locked -p paigasus-logging --lib
cargo clippy --locked -p paigasus-logging --all-targets -- -D warnings
cargo clippy --locked -p paigasus-logging --features test-support -- -D warnings
```
Expected: all 11 tests pass under nextest AND under `cargo test` (the static lock makes the thread-parallel run safe). Both clippy runs are clean. `git diff --stat` shows no change to `rs/Cargo.lock` (a feature adds no lock line).

If clippy reports `clippy::double_must_use`, it is because a `#[must_use]` attribute lost its message. Keep the message.

- [ ] **Step 7: Mutation proof (spec section 5.4)**

Run each mutation alone, with `cargo nextest run --locked -p paigasus-logging --no-fail-fast`. Revert each one with the Edit tool before the next, and confirm with `git diff` that only the Task 1 work remains.

| # | Mutation (in `test_support.rs`) | Must go red |
|---|---|---|
| M1 | `capture_logs_at(tracing::Level::TRACE)` → `capture_logs_at(tracing::Level::INFO)` in `capture_logs` | `capture_logs_sees_trace` |
| M2 | `.with_max_level(level)` → `.with_max_level(tracing::Level::TRACE)`, and add `let _ = level;` as the first line of `capture_logs_at` so that it still compiles | `capture_logs_at_info_drops_debug` |
| M3 | delete `.with_ansi(false)` | `capture_has_no_ansi_escape` |
| M4 | in `capture_has_no_ansi_escape`, delete the `unsafe { std::env::remove_var("NO_COLOR") };` line, keep M3, and run with `NO_COLOR=1` | the test goes GREEN. This shows why the test removes `NO_COLOR`. Revert both M3 and M4 |

Record each result (test name, red or green) for the PR body. If M3 stays green, record that and do not claim that `capture_has_no_ansi_escape` guards the setting. After all mutations: `git diff` shows the Step 5 code, and the full test run from Step 6 is green again.

Record also for the PR body: `capture_logs_sees_trace` is the ONLY guard of the TRACE default. The gateway row 9 test and the IAM unit tests stay green at INFO, because their "never logged" checks pass at any level.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && test "$(git branch --show-current)" = feature/sma-689-shared-log-capture-helper
git add rs/crates/libs/paigasus-logging/Cargo.toml rs/crates/libs/paigasus-logging/src/lib.rs rs/crates/libs/paigasus-logging/src/test_support.rs
git commit -m "test(rs): add the shared log-capture helper to paigasus-logging (SMA-689)

The test-support feature adds paigasus_logging::test_support with
capture_logs (TRACE) and capture_logs_at(level).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Move `paigasus-gateway` to the shared helper

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml:102-107` (dev-dependencies)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` (tests module, lines 792-828 and the `use` block at 455-464)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (lines 38-84, 500, 519)
- Modify: `rs/Cargo.lock` (one removed line)

**Interfaces:**
- Consumes: `paigasus_logging::test_support::{capture_logs, capture_logs_at}` and the `test-support` feature from Task 1.
- Produces: nothing for later tasks.

- [ ] **Step 1: Record the Moon project graph before any service change (spec section 4.5)**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects > <scratchpad>/moon-projects-before.json; echo "rc=$?"
```
Expected: `rc=0`. Do not pipe the command (a pipe hides its exit status). Keep the file for Task 4.

- [ ] **Step 2: Change the gateway `Cargo.toml`**

In `rs/crates/services/paigasus-gateway/Cargo.toml`, replace these three lines at the end of `[dev-dependencies]`:

```toml
# Captures log lines in the auth and chat tests (SMA-635): one `paigasus-org ignored` warning, and
# the `auth`/`scope` fields of `chat completion proxied`.
tracing-subscriber = { workspace = true }
```

with:

```toml
# The shared log-capture helper for the auth and chat tests (SMA-689, spec D3). Listed a second
# time, here, so that the `test-support` feature reaches test builds only, never the shipped
# binary. `version = "0.0.0"` is REQUIRED, not redundant: `rs/deny.toml` sets
# `wildcards = "deny"`, and a path dependency with no version counts as a wildcard.
paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }
```

- [ ] **Step 3: Remove copy C1 from `auth.rs`**

In `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs`, delete everything from the line `    // ---- log capture ---...` down to and including the closing `    }` of `fn capture_logs`, and the blank line after it. The next line is then `    // ---- \`iam_result\` bounded-label mapping ---...`. This deletes the "A second copy of this helper exists" comment too.

In the same tests module, insert directly before `    use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, ...};`:

```rust
    // TRACE (the default of `capture_logs`), not INFO: row 9 asserts that the `paigasus-org`
    // header value is never logged, and only TRACE makes that check see every level.
    use paigasus_logging::test_support::capture_logs;
```

Keep `use std::sync::Mutex;`: the tests module still uses `Arc<Mutex<..>>` at the `recorded` field (about line 536) and `Arc::new(Mutex::new(None))` (about line 545).

- [ ] **Step 4: Remove copy C2 from `chat_proxy.rs`**

In `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`, delete everything from the line `// ---- log capture (SMA-635) ---...` down to and including the closing `}` of `fn capture_logs`, and the blank line after it. The next line is then `/// The real OpenAI key the gateway is configured with ...`.

Insert after `use paigasus_gateway::config::OpenAiConfig;`:

```rust
use paigasus_logging::test_support::capture_logs_at;
```

At both call sites (the tests `the_request_log_names_the_credential_and_the_scope` and `an_oidc_user_is_proxied_with_the_org_scope`), replace `let (logs, _guard) = capture_logs();` with:

```rust
    // INFO, as before SMA-689: these tests look for the `chat completion proxied` line only.
    let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
```

Keep `use std::sync::Arc;`: `chat_proxy.rs` still uses `Arc::new` at about lines 206-207 (before the deletion).

- [ ] **Step 5: Confirm that no code names `tracing_subscriber` any more**

Run: `cd <worktree> && git grep -n "tracing_subscriber" -- rs/crates/services/paigasus-gateway`
Expected: no output (exit 1). If a line remains that is code, not a comment, stop: the dev-dependency removal of Step 2 is then wrong.

- [ ] **Step 6: Update the lock and check its diff**

```bash
cd <worktree>/rs && cargo metadata --format-version 1 > /dev/null && git diff -U0 Cargo.lock
```
Expected: exactly one removed line, `-  "tracing-subscriber",`, in the `dependencies` list of `name = "paigasus-gateway"` (about line 3187), and nothing else. Then `cargo metadata --locked --format-version 1 > /dev/null` exits 0.

- [ ] **Step 7: Run the gateway tests, lint and format**

```bash
cd <worktree>/rs && cargo fmt -p paigasus-gateway && git diff --stat
cargo nextest run --locked -p paigasus-gateway --no-fail-fast
cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```
Expected: all gateway tests pass, including the `auth.rs` row 9 test (the one that calls `capture_logs`) and both `chat_proxy.rs` tests named in Step 4. Clippy is clean. If `cargo fmt` moved the new `use` line, accept its order.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && test "$(git branch --show-current)" = feature/sma-689-shared-log-capture-helper
git add rs/crates/services/paigasus-gateway/Cargo.toml rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs rs/crates/services/paigasus-gateway/tests/chat_proxy.rs rs/Cargo.lock
git commit -m "test(rs): use the shared log-capture helper in paigasus-gateway (SMA-689)

auth.rs keeps TRACE through capture_logs; chat_proxy.rs keeps INFO
through capture_logs_at. The tracing-subscriber dev-dependency goes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Move `paigasus-iam` to the shared helper

**Files:**
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml:176-177` (dev-dependencies)
- Delete: `rs/crates/services/paigasus-iam/src/log_capture.rs`
- Modify: `rs/crates/services/paigasus-iam/src/lib.rs:13-14`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:367`
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:378`
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:956-998`
- Modify: `rs/crates/services/paigasus-iam/tests/{authn_identities,grpc_whoami,http_authn}.rs` (7 calls)
- Conditional: `rs/crates/services/paigasus-iam/src/application/{api_keys,service_accounts}.rs` (only if PR 348 merged first)
- Modify: `rs/Cargo.lock` (one removed line)

**Interfaces:**
- Consumes: `paigasus_logging::test_support::capture_logs` and the `test-support` feature from Task 1.
- Produces: nothing for later tasks.

- [ ] **Step 1: Check PR 348 (SMA-646) and rebase if it merged (A7)**

```bash
cd <worktree> && git status --porcelain && gh pr view 348 --json state,mergedAt && git fetch origin main && git log --oneline HEAD..origin/main
```
If `state` is `MERGED`: the working tree must be clean; then `git rebase origin/main`. Resolve no conflict by guesswork: Tasks 1-2 touch only files that PR 348 does not touch, so a conflict is a surprise, and you report it. After the rebase, `git grep -n "log_capture::capture_logs" -- rs` lists the extra call sites (spec section 4.3 expects `src/application/api_keys.rs` and `src/application/service_accounts.rs`). Migrate each of them in Step 4 in the same way.
If `state` is `OPEN`: do not rebase. Record for the PR body that SMA-646 must migrate its own two call sites if it merges after this branch, because `crate::log_capture` no longer exists.

- [ ] **Step 2: Change the IAM `Cargo.toml`**

In `rs/crates/services/paigasus-iam/Cargo.toml`, replace these two lines at the end of `[dev-dependencies]`:

```toml
# SMA-686: captures the validator's refusal log line in a unit test.
tracing-subscriber = { workspace = true }
```

with:

```toml
# The shared log-capture helper for the unit and integration tests (SMA-689, spec D3). Listed a
# second time, here, so that the `test-support` feature reaches test builds only, never the
# shipped binary. `version = "0.0.0"` is REQUIRED, not redundant: `rs/deny.toml` sets
# `wildcards = "deny"`, and a path dependency with no version counts as a wildcard.
paigasus-logging = { path = "../../libs/paigasus-logging", version = "0.0.0", features = ["test-support"] }
```

- [ ] **Step 3: Remove copy C3**

```bash
cd <worktree> && git rm rs/crates/services/paigasus-iam/src/log_capture.rs
```

In `rs/crates/services/paigasus-iam/src/lib.rs`, delete these two lines:

```rust
#[cfg(test)]
mod log_capture;
```

- [ ] **Step 4: Change the unit-test imports**

In `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` and `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (and, only after a Step 1 rebase, in each file that the Step 1 `git grep` listed), replace:

```rust
    use crate::log_capture::capture_logs;
```

with:

```rust
    use paigasus_logging::test_support::capture_logs;
```

Then run `git grep -n "log_capture\|share a .cfg(test)" -- rs/crates/services/paigasus-iam`. Expected: no output. If a comment says that a crate cannot share a `cfg(test)` helper, delete that comment.

- [ ] **Step 5: Remove copy C4 from `tests/support/mod.rs`**

In `rs/crates/services/paigasus-iam/tests/support/mod.rs`, delete everything from the line `// --- SMA-698: log capture for the JIT provisioning-failure line ---...` down to and including the closing `}` of `pub fn capture_logs`, and the blank line after it. The next line is then `/// The fixed prefix of both SMA-698 JIT failure messages. ...`. In its place, insert:

```rust
// --- SMA-698 / SMA-707: the fixed prefixes of the JIT log lines ------------------------------
//
// The tests capture log lines with `paigasus_logging::test_support::capture_logs` (SMA-689) and
// count only the lines that contain one of these prefixes.

```

Keep both constants, `JIT_FAILURE_LINE` and `JIT_DISABLED_LINE`, with their `#[allow(dead_code)]`. Keep `use std::sync::{Arc, RwLock};` (line 52): `Arc` is still used at about lines 171 and 267.

- [ ] **Step 6: Change the seven integration-test calls**

```bash
cd <worktree>/rs/crates/services/paigasus-iam/tests && sed -i '' 's/support::capture_logs()/paigasus_logging::test_support::capture_logs()/' authn_identities.rs grpc_whoami.rs http_authn.rs
cd <worktree> && git grep -n "test_support::capture_logs()" -- rs/crates/services/paigasus-iam/tests | wc -l
```
Expected: `7` (1 in `authn_identities.rs`, 3 in `grpc_whoami.rs`, 3 in `http_authn.rs`).

- [ ] **Step 7: Confirm that only comments name `tracing_subscriber`**

Run: `cd <worktree> && git grep -n "tracing_subscriber" -- rs/crates/services/paigasus-iam`
Expected: exactly two lines, both comments, in `tests/boot_lifecycle_pg.rs` (about lines 46 and 193). This is the control for the dev-dependency removal. `repo:machete` is not a control here: it matches `tracing_subscriber::` in comments too.

- [ ] **Step 8: Update the lock and check its diff**

```bash
cd <worktree>/rs && cargo metadata --format-version 1 > /dev/null && git diff -U0 Cargo.lock
```
Expected: exactly one removed line, `-  "tracing-subscriber",`, in the `dependencies` list of `name = "paigasus-iam"` (about line 3240), and nothing else. Then `cargo metadata --locked --format-version 1 > /dev/null` exits 0.

- [ ] **Step 9: Run the IAM tests, lint and format**

Docker must run. Check with `docker info > /dev/null; echo rc=$?` (expected `rc=0`).

```bash
cd <worktree>/rs && cargo fmt -p paigasus-iam && git diff --stat
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --no-fail-fast
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings
```
Expected: all tests pass. `PAIGASUS_REQUIRE_DOCKER=1` turns a silent Docker skip into a failure, so the seven C4 callers really ran. Confirm by name that these tests passed: the `validator.rs` and `authenticate_token.rs` log tests, and the callers in `authn_identities.rs`, `grpc_whoami.rs` and `http_authn.rs`. If a Docker-backed suite fails with a known flake from `rs/CLAUDE.md` (for example `authz_policy_store.rs` under parallel load), re-run that one binary once and record both runs. Clippy is clean.

- [ ] **Step 10: Commit**

```bash
cd <worktree> && test "$(git branch --show-current)" = feature/sma-689-shared-log-capture-helper
git add -A rs/crates/services/paigasus-iam rs/Cargo.lock
git status --short
git commit -m "test(rs): use the shared log-capture helper in paigasus-iam (SMA-689)

Delete src/log_capture.rs and the tests/support copy. The unit and
integration tests keep TRACE. The tracing-subscriber dev-dependency goes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Before the commit, `git status --short` must list only paigasus-iam files and `rs/Cargo.lock`.

---

### Task 4: Document the convention and verify the whole change

**Files:**
- Modify: `rs/CLAUDE.md` (one bullet in "Cargo, the lockfile and nextest")

**Interfaces:**
- Consumes: the finished Tasks 1-3; `<scratchpad>/moon-projects-before.json` from Task 2 Step 1.
- Produces: the evidence for the PR body.

- [ ] **Step 1: Add the `rs/CLAUDE.md` bullet (spec section 7, A4)**

In `rs/CLAUDE.md`, section `## Cargo, the lockfile and nextest`, add this bullet directly after the first bullet (the `--no-tests=pass` one):

```markdown
- Tests that assert on a log line use `paigasus_logging::test_support` (`capture_logs` at TRACE,
  `capture_logs_at(level)`). Turn on its `test-support` feature from `[dev-dependencies]` only,
  never from `[dependencies]`, so that the production binary does not contain it. This is the
  first `[features]` table in a workspace crate; follow the same pattern for a new test-only
  helper. Do not write a new `LogBuffer`. The helper supports `cargo nextest` only (SMA-689).
```

This file does not name `ciReport.json`, so no `moon-diagnosis` marker is needed (actionlint check 12). Confirm: `git diff rs/CLAUDE.md | grep -c ciReport` prints `0`.

- [ ] **Step 2: AC 1, one helper**

```bash
cd <worktree> && git grep -nE 'struct LogBuffer|fn capture_logs' -- rs; test ! -e rs/crates/services/paigasus-iam/src/log_capture.rs && echo "log_capture.rs gone"
```
Expected: every match is in `rs/crates/libs/paigasus-logging/src/test_support.rs`, and the second command prints `log_capture.rs gone`.

- [ ] **Step 3: The lock diff against the branch base**

```bash
cd <worktree> && git diff origin/main -- rs/Cargo.lock
```
Expected: exactly two removed `"tracing-subscriber",` lines (the `paigasus-gateway` and `paigasus-iam` blocks) and nothing else.

- [ ] **Step 4: AC 2 and AC 4, the full test run**

```bash
cd <worktree>/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-logging -p paigasus-gateway -p paigasus-iam --no-fail-fast
```
Expected: all tests pass. Record the summary line for the PR body. If `PAIGASUS_REQUIRE_DOCKER` is not honoured by a suite, record instead that `docker_preflight` passed.

- [ ] **Step 5: AC 6, the helper is not in the production build (spec section 5.3)**

For `<crate>` in `paigasus-gateway` and `paigasus-iam`:

```bash
cd <worktree>/rs
cargo tree --locked -p <crate> -e features,no-dev -i paigasus-logging | grep -c test-support   # expected 0
cargo tree --locked -p <crate> -e features,dev -i paigasus-logging | grep -c test-support      # expected >= 1
cargo clean -p paigasus-logging
cargo build --locked -p <crate> --bin <crate> -v 2>&1 | grep -- '--crate-name paigasus_logging' | grep -c 'feature="test-support"'   # expected 0
```
Cargo prints the flag as `--cfg 'feature="test-support"'`, so the pattern has no backslashes. Also check that the third command found the `rustc` line at all: `... | grep -c -- '--crate-name paigasus_logging'` must print at least 1. Without that check, a `Fresh` build prints no line and a `0` proves nothing. Record all outputs for the PR body. The package and target selection matches `rs/Dockerfile:36` (`cargo auditable build --release --locked -p "${BIN}" --bin "${BIN}"`); the profile does not change features.

- [ ] **Step 6: The Moon project graph after the change (spec section 4.5)**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects > <scratchpad>/moon-projects-after.json; echo "rc=$?"
diff <scratchpad>/moon-projects-before.json <scratchpad>/moon-projects-after.json; echo "diff rc=$?"
```
Expected: `rc=0`. Record the diff (most probably empty) for the PR body. If the edge scope of `paigasus-logging-rs` changed for a service, record the exact change; do not edit a `moon.yml` to hide it.

- [ ] **Step 7: The full gate graph (spec section 5.5)**

Check the pipe state first: `/opt/homebrew/bin/bash ci/actionlint/run.sh` prints `pipe capacity N bytes` in its preflight. Then run the command between the `ci-targets` markers in the root `CLAUDE.md`, exactly as written there (`moon ci :build :test ... --base origin/main --include-relations`). Read `moon query tasks --affected` per the root `CLAUDE.md` (one target per `tasks[project][task]`) and record the selection.

Re-run the gates that need another bash directly, and read those results instead of the `moon ci` verdict for them:

| Gate | Command |
|---|---|
| `repo:affected-smoke` | `/bin/bash ci/affected-smoke/run.sh` |
| `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions` | `/opt/homebrew/bin/bash ci/<gate>/run.sh` |
| `repo:actionlint` (if selected) | `/opt/homebrew/bin/bash ci/actionlint/run.sh` when the preflight passes (>= 8192 bytes); else run it in a Linux container (`docker run ubuntu:24.04`), per the `small-pipe-linux-container-workaround` memory |

Expected: every selected gate is green. On a failure, follow the "Diagnosing an unattributed `moon ci` failure" procedure in the root `CLAUDE.md` (Step 0 first: copy the evidence before any re-run). Known host artifacts (the two false `cargo-lock-step` rows under bash 3.2, the `version-lockstep` `tar: Write error`) are recorded, not fixed. Expect `.github/workflows/images.yml` to run on the PR, because its path filter lists `rs/Cargo.lock`.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && test "$(git branch --show-current)" = feature/sma-689-shared-log-capture-helper
git add rs/CLAUDE.md
git commit -m "docs(rs): name the shared log-capture helper and the test-support feature (SMA-689)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Evidence for the PR body

Collect these while you work (the PR body is written in the open-pr stage):

- Task 1 Step 7: the mutation table (M1-M4, red or green each), and the note that `capture_logs_sees_trace` is the only guard of the TRACE default.
- Task 3 Step 1: PR 348 state, and either the migrated SMA-646 call sites or the note that SMA-646 must migrate them.
- Task 4 Steps 3-6: the lock diff, the nextest summary, the AC 6 outputs, the Moon project graph diff.
- Task 4 Step 7: the gate selection and the result of each gate, with the bash that ran it.
