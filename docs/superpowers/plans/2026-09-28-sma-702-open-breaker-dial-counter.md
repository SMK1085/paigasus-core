# SMA-702 Open-Breaker Dial Counter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The seven open-breaker tests in `paigasus-iam` prove "no dial" with a count of accepted TCP connections, not with a 100 ms wall-clock bound that flaked on a loaded CI runner.

**Architecture:** The test blackhole listener (`redis_conn::test_support`) gets an `Arc<AtomicUsize>` that counts every accepted connection, and its accept loop no longer dies on an accept error. S1 to S6 assert `accepted() == 0`. S1 to S5 and S7 keep a 1 s clock only as a stall backstop. S6 moves from the closed port to the blackhole and drops its clock. S7 proves that the counter counts (`accepted() >= 1` after a real dial). No production code changes.

**Tech Stack:** Rust (edition 2024, rust 1.95), tokio `#[tokio::test]` (current_thread runtime), redis-rs 1.7.0 `ConnectionManager`, cargo-nextest 0.9.136.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-702-open-breaker-dial-counter-design.md` (revision 3, approved by Sven on 2026-09-28). Read it before you start. Section numbers below (§ N) refer to it.

## Global Constraints

- No production code changes. Every Rust change is `#[cfg(test)]` code or a doc comment (A6).
- Every command runs in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-702-open-breaker-dial-counter`, branch `feature/sma-702-open-breaker-dial-counter`. In this plan, `<worktree>` means that absolute path, and `<scratchpad>` means the session scratchpad directory from your system prompt (never a path in the repo). Begin each shell command with `cd <worktree>/rs &&` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`.
- Before each commit, `git branch --show-current` must print `feature/sma-702-open-breaker-dial-counter`.
- Commit type is `test(rs)`, with `(SMA-702)` in the subject. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` or `token: value` line in the body (commitlint `footer-leading-blank`).
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, or `git checkout -- <file>`.
- The workspace sets `warnings = "deny"` (`rs/Cargo.toml:274-275`). An unused import or binding is a build error.
- `rustfmt` `max_width = 200` (`rs/rustfmt.toml`). Run `cargo fmt` after each edit.
- Clippy must use `--all-targets`: `cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings`. A plain clippy does not compile `#[cfg(test)]` code.
- Use `--lib` for the S1 to S7 runs. All seven tests are unit tests. Without `--lib`, the filter also builds and runs the Docker-backed integration binaries.
- The clock backstop is exactly `std::time::Duration::from_secs(1)` in S1 to S5 and `Duration::from_secs(1)` in S7 (S7's module imports `Duration`). S6 has no clock.
- Do not install host software. Do not leave background processes running.
- Mutation runs are authorized by Sven (2026-09-28). Restore each mutation with the Edit tool, confirm with `git diff`, and never commit a mutation.

## Review Focus

1. **A call that dials AFTER the count is read.** If a cache call spawned a background task that dials later, `accepted() == 0` would pass although a dial happens. Expected: every Redis call in S1 to S5 is awaited in the test before the assertion. Task 2 Step 1 pins this with a grep for `spawn` in the five adapters' non-test code.
2. **The accept task is not polled before the assertion.** On the current_thread runtime the accept loop runs only when the test task yields. Expected: a real dial yields for about 1 s or more, so a dial always shows in the count. Task 3's M1 run pins this: each of S1 to S5 must red on the `accepted()` assertion, not on the clock.
3. **The accept loop dies on an accept error.** A dead loop drops the listener, so a later dial is refused in microseconds and the count stays 0 (a false green). Expected: the loop does `continue`. Task 1 Step 3 makes the change, and Task 1 Step 6 pins it with a grep that the `else { return }` form is gone from `start()`.
4. **The counter counts nothing.** A broken counter makes every zero assertion vacuous. Expected: S7 reds. Task 1 adds the `accepted() >= 1` assertion, and Task 3's M2 proves it bites.
5. **The CPU-load stress run leaves busy loops alive, or selects the wrong tests.** Expected: the load processes stop when the run ends, even on failure, and only unit tests run. Task 4 uses a script with an `EXIT` trap and `--lib`, and counts S1 to S7 failures by name.

---

### Task 1: The blackhole counter, S6 and S7 in `redis_conn.rs`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`
  - the `RedisHandle` doc, upgrade-coupling paragraph (about lines 63-70)
  - S6 `cloning_a_handle_shares_one_breaker` (about lines 885-905)
  - S7 `a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens` (about lines 921-985)
  - `mod test_support` (about lines 1095-1160)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `pub(crate) fn accepted(&self) -> usize` on `redis_conn::test_support::Blackhole`. Task 2 calls it as `blackhole.accepted()`.

- [ ] **Step 1: Write the failing A4 assertion in S7 (red first)**

In S7, directly after the existing assertion that ends with `(2 x connection_timeout + one jittered min_delay)"\n        );` (the `first_elapsed < Duration::from_millis(3500)` assertion), insert:

```rust

        // SMA-702 A4: command #1 was a real dial, so the counter must have seen it. Without
        // this, a broken counter would let every `accepted() == 0` assertion pass vacuously.
        let accepted_after_first = blackhole.accepted();
        assert!(
            accepted_after_first >= 1,
            "SMA-702 A4: command #1 really dialled the blackhole, but accepted() is {accepted_after_first} — \
             the counter does not count, so the `accepted() == 0` assertions in the open-breaker tests are vacuous"
        );
```

Use `>= 1`, not `== 1`: the retry makes a second attempt.

- [ ] **Step 2: Build to confirm it fails**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib -E 'test(a_blackholed_backend)'`
Expected: a build error `no method named `accepted` found for struct `Blackhole``. This proves only that the method is new (spec § 5.1). Task 3 is the real proof.

- [ ] **Step 3: Add the counter to `test_support`**

3a. Change the import line in `mod test_support`:

```rust
    use std::sync::atomic::{AtomicBool, Ordering};
```

to:

```rust
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
```

3b. At the end of the `Blackhole` struct doc (after the line `/// — do not add credentials or a db index to these tests' URLs.`), add:

```rust
    ///
    /// It also counts the connections it accepts ([`Blackhole::accepted`]). The open-breaker
    /// posture tests assert that this count stays 0 to prove "no dial". That count replaces a
    /// 100 ms wall-clock bound that failed on a loaded CI runner (SMA-702).
```

3c. In the struct, after the `responding: Arc<AtomicBool>,` field, add:

```rust
        // Incremented by the accept loop in `start` for every accepted connection, in both modes.
        accepted: Arc<AtomicUsize>,
```

3d. In `impl Blackhole`, after `start_responding`, add:

```rust

        /// TCP connections this listener has accepted so far, in either mode. A redis-rs dial
        /// always opens one, so a test that expects NO dial asserts this stays 0 (SMA-702).
        ///
        /// This counts application-level `accept()`, not the kernel handshake. It is exact only
        /// because a real dial suspends the calling test for about 1 s or more, which lets the
        /// runtime poll the accept task before the test asserts.
        pub(crate) fn accepted(&self) -> usize {
            self.accepted.load(Ordering::SeqCst)
        }
```

3e. In `start()`, replace this block:

```rust
        let accept_responding = Arc::clone(&responding);
        let accept_held = Arc::clone(&held);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                if accept_responding.load(Ordering::SeqCst) {
```

with:

```rust
        let accepted = Arc::new(AtomicUsize::new(0));

        let accept_responding = Arc::clone(&responding);
        let accept_held = Arc::clone(&held);
        let accept_count = Arc::clone(&accepted);
        let accept = tokio::spawn(async move {
            loop {
                // `continue`, not `return`: a dead loop drops the listener, and then every
                // "no dial" count reads 0 as a false green (SMA-702).
                let Ok((stream, _)) = listener.accept().await else { continue };
                accept_count.fetch_add(1, Ordering::SeqCst);
                if accept_responding.load(Ordering::SeqCst) {
```

3f. In the `Blackhole { .. }` literal at the end of `start()`, after `responding,` add `accepted,`.

- [ ] **Step 4: Widen the S7 per-command clock and update its doc**

4a. In S7's `for i in 4..=10` loop, replace:

```rust
            assert!(
                elapsed < Duration::from_millis(100),
                "command #{i} took {elapsed:?}; an open breaker must return without touching the network"
            );
```

with:

```rust
            assert!(
                elapsed < Duration::from_secs(1),
                "command #{i} took {elapsed:?} — stall backstop only: the BREAKER_OPEN_MESSAGE check above \
                 proved no dial, so this is probably runner load, not a breaker regression"
            );
```

4b. In S7's doc comment, after the paragraph that ends `not to pin exact timings.`, add:

```rust
    ///
    /// For commands #4 to #10 the proof of "no dial" is the `BREAKER_OPEN_MESSAGE` check. The
    /// 1 s clock only catches a stall (SMA-702). It cannot prove "no dial" alone: a command that
    /// joins an in-flight reconnect pays only the rest of that dial. No connection count is
    /// possible here, because the reconnect that `reconnect()` spawns after command #3 can be
    /// accepted while commands #4 to #10 run. Command #1 does assert the count, to prove that
    /// the counter counts (SMA-702 A4).
```

- [ ] **Step 5: Move S6 to the blackhole with a count, and drop its clock**

Replace the whole S6 test body and its doc with:

```rust
    /// SMA-476 D1. Every one of the eleven call sites does `self.conn.clone()` per command, so a
    /// `#[derive(Clone)]` over a non-`Arc` breaker field would compile and silently give every
    /// call its own breaker — which would never open. This is that guard.
    ///
    /// Pointed at a BLACKHOLE, not the closed port `127.0.0.1:1`: a dial to a closed port costs
    /// only about 100-200 ms, which no reasonable clock bound can tell from a short-circuit. The
    /// proof of "no dial" is `accepted() == 0` (SMA-702). There is no clock bound.
    #[tokio::test]
    async fn cloning_a_handle_shares_one_breaker() {
        use redis::AsyncCommands;

        let blackhole = test_support::start().await;
        let handle = with_open_breaker_for_tests(&blackhole.url, RedisRole::Authz).expect("well-formed url");
        let mut clone = handle.clone();

        let result: redis::RedisResult<Option<Vec<u8>>> = clone.get("sma476:probe").await;

        let err = result.expect_err("an open breaker must short-circuit with an error");
        assert!(
            err.to_string().contains(BREAKER_OPEN_MESSAGE),
            "a CLONE dialled instead of short-circuiting — the breaker is not Arc-shared: {err:?}"
        );
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the CLONE dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
    }
```

Do not change `an_open_breaker_short_circuits_asynccommands_without_dialling`. It stays on the closed port and has no clock (spec § 4.3).

- [ ] **Step 6: Add the upgrade coupling to the `RedisHandle` doc, and check the loop change**

6a. In the `RedisHandle` doc, after the line `/// one connect budget.` and before `#[derive(Clone, Debug)]`, add:

```rust
///
/// A second coupling (SMA-702): the open-breaker tests assert that the test blackhole accepted
/// 0 connections. That holds only because `ConnectionManager::new_lazy_with_config` (redis
/// 1.7.0) stores an un-polled lazy connect future and does not start the dial. If an upgrade
/// makes it dial eagerly, those `accepted() == 0` assertions red with no breaker regression.
```

Do not change the existing `redis-1.3.0` citations (spec § 9).

6b. Run: `cd <worktree>/rs && grep -n 'listener.accept().await else' crates/services/paigasus-iam/src/adapters/redis_conn.rs`
Expected: exactly one line, and it ends with `else { continue };`. (The other `else { return }` in `serve_minimal_resp` reads a stream, not the listener, and must stay.)

- [ ] **Step 7: Format, then run S6 and S7**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt && cargo nextest run -p paigasus-iam --lib -E 'test(cloning_a_handle) | test(a_blackholed_backend) | test(an_open_breaker_short_circuits_asynccommands)'`
Expected: 3 tests pass. S7 takes about 6.5 s.

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-702-open-breaker-dial-counter
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs
git commit -m "test(rs): count blackhole accepts and prove no dial in the breaker clone test (SMA-702)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The five posture tests S1 to S5, and the RUNBOOK line

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs` (S1, about lines 304-324)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs` (S2, about lines 294-316)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs` (S3, about lines 399-418)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs` (S4, about lines 545-572)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs` (S5, about lines 91-114)
- Modify: `docs/ops/RUNBOOK-observability.md` (line 1721)

**Interfaces:**
- Consumes: `Blackhole::accepted(&self) -> usize` from Task 1. Each test already binds `let blackhole = crate::adapters::redis_conn::test_support::start().await;`.
- Produces: nothing that later tasks call. Task 3 relies on the count assertion coming BEFORE the clock assertion in each test.

- [ ] **Step 1: Pin Review Focus 1 (no late background dial)**

Run: `cd <worktree>/rs/crates/services/paigasus-iam/src/adapters && grep -n 'spawn' authz/decision_cache.rs authz/entity_cache.rs api_keys/cache.rs authz/generation.rs oidc/redis_cache.rs`
Expected: only `authz/generation.rs:904`, which is a comment in a test. If a new `tokio::spawn` shows up in non-test code on any S1 to S5 call path, stop and report it: the count could then miss a dial that happens after the assertion.

- [ ] **Step 2: Change the doc sentence in all five tests**

In each of the five tests, the doc comment ends with these two lines:

```rust
    /// looks identical to a short-circuit. Here a command that actually dialled would cost
    /// ~2.1 s, so the elapsed assertion proves the breaker short-circuited.
```

Replace them in each file with:

```rust
    /// looks identical to a short-circuit. Here a command that actually dialled would open a
    /// TCP connection that the blackhole counts, so `accepted() == 0` proves the breaker
    /// short-circuited (SMA-702). The 1 s clock is only a stall backstop.
```

- [ ] **Step 3: S1, `decision_cache.rs`**

Replace:

```rust
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the calls dialled instead of short-circuiting");
```

with:

```rust
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the get/put dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
             probably runner load, not a breaker regression"
        );
```

- [ ] **Step 4: S2, `entity_cache.rs`**

Replace:

```rust
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the cache dialled instead of short-circuiting");
```

with:

```rust
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the slice cache load dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
             probably runner load, not a breaker regression"
        );
```

- [ ] **Step 5: S3, `api_keys/cache.rs`**

Replace:

```rust
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the calls dialled instead of short-circuiting");
```

with:

```rust
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the get/evict dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
             probably runner load, not a breaker regression"
        );
```

- [ ] **Step 6: S4, `generation.rs`**

Replace:

```rust
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the read dialled instead of short-circuiting");
```

with:

```rust
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the policy_gen read dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
             probably runner load, not a breaker regression"
        );
```

- [ ] **Step 7: S5, `oidc/redis_cache.rs`**

Replace:

```rust
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the get dialled instead of short-circuiting");
```

with:

```rust
        assert_eq!(
            blackhole.accepted(),
            0,
            "SMA-702: the blackhole accepted a connection — the JWKS get dialled instead of short-circuiting \
             (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
             probably runner load, not a breaker regression"
        );
```

In S1 to S5 the count assertion must stay AFTER the existing posture assertion (`got.is_none()`, `inner.loads()`, `matches!(..)`) and BEFORE the clock assertion. Task 3's M1 depends on that order.

- [ ] **Step 8: Check that no 100 ms bound is left**

Run: `cd <worktree>/rs && grep -rn 'from_millis(100)' crates/services/paigasus-iam/src/`
Expected: no output. (Spec § 12: the seven S1 to S7 bounds were the only ones of that kind in the crate's unit tests.)

- [ ] **Step 9: Reword the RUNBOOK line**

In `docs/ops/RUNBOOK-observability.md`, line 1721, replace:

```markdown
  action: commands 4–10 each cost under 100 ms once the breaker trips at command 3.
```

with:

```markdown
  action: commands 4–10 each short-circuit without dialling once the breaker trips at command 3
  (asserted by the breaker-open error).
```

Do not change the `~2.1 s` or `~6.46 s` figures. They describe the real dial.

- [ ] **Step 10: Format and run S1 to S7**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt && cargo nextest run -p paigasus-iam --lib -E 'test(an_open_breaker) | test(cloning_a_handle) | test(a_blackholed_backend)'`
Expected: all pass. The filter also selects `an_open_breaker_still_recovers_after_many_short_circuits` and `an_open_breaker_admits_a_probe_once_the_window_elapses` (spec § 5.2). They must pass too.

- [ ] **Step 11: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-702-open-breaker-dial-counter
git add rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs \
        rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs \
        rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs \
        rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs \
        rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs \
        docs/ops/RUNBOOK-observability.md
git commit -m "test(rs): prove no dial by a count in the open-breaker posture tests (SMA-702)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Full checks and the mutation battery

**Files:**
- Temporarily modify, then restore: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`
- No commit of any mutation. This task commits nothing unless Step 1 or Step 2 needs a fix.

**Interfaces:**
- Consumes: the Task 1 and Task 2 code.
- Produces: the mutation results (red-test names and first failure messages) for the PR body.

- [ ] **Step 1: Lint and format (A6)**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --check && cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings`
Expected: rc 0 for both.

- [ ] **Step 2: Full crate test run (A6)**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && env -u CI cargo nextest run -p paigasus-iam`
Expected: all pass. The Docker-backed suites need a reachable Docker daemon. If Docker is down, `docker_preflight` reds alone; record that and also run `cargo nextest run -p paigasus-iam --lib`, which must pass. If a fix was needed in Step 1 or 2, commit it as `test(rs): ... (SMA-702)` before Step 3.

- [ ] **Step 3: Mutation M1 — the forced-open breaker stays closed**

In `Breaker::force_open_for_tests` (about line 462), use the Edit tool to change:

```rust
        self.transition(&mut inner, BreakerState::Open);
    }
}
```

to:

```rust
        self.transition(&mut inner, BreakerState::Closed); // SMA-702 MUTATION M1
    }
}
```

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast 2>&1 | tee <scratchpad>/m1.log; echo "rc=${PIPESTATUS[0]}"`

Expected (A5, spec § 5.3):
- The build succeeds. The log shows nextest test failures, not a rustc error. rc is 100 (test failures), not 101.
- Exactly 7 tests fail:
  - `an_open_breaker_keeps_the_decision_cache_failing_open`
  - `an_open_breaker_falls_through_to_the_inner_slice_loader`
  - `an_open_breaker_keeps_the_api_key_cache_failing_open`
  - `an_open_breaker_keeps_redis_generations_propagating_the_error`
  - `an_open_breaker_keeps_the_jwks_cache_failing_closed`
  - `cloning_a_handle_shares_one_breaker`
  - `an_open_breaker_short_circuits_asynccommands_without_dialling`
- For S1 to S5, the first failure message is the `SMA-702: the blackhole accepted a connection` message, not the clock. This pins Review Focus 2. (S4 and S5 have a posture assertion before the count. Under M1 the dial still returns `Backend` / `Unavailable`, so the posture assertion passes and the count reds. If a posture assertion reds first, record the exact message and report it.)
- S6 reds on the message assertion or on the count.

Run: `grep -E '^\s+FAIL' <scratchpad>/m1.log` and record the names and count. Then use `grep -n 'SMA-702: the blackhole accepted' <scratchpad>/m1.log` to confirm the count message per test.

- [ ] **Step 4: Restore M1**

Use the Edit tool to change `self.transition(&mut inner, BreakerState::Closed); // SMA-702 MUTATION M1` back to `self.transition(&mut inner, BreakerState::Open);`.
Run: `cd <worktree> && git diff --stat && grep -rn 'SMA-702 MUTATION' rs/`
Expected: an empty diff (Tasks 1 and 2 are committed), and no grep output.

- [ ] **Step 5: Mutation M2 — the counter does not count**

In `test_support::start`, use the Edit tool to change:

```rust
                accept_count.fetch_add(1, Ordering::SeqCst);
```

to:

```rust
                accept_count.fetch_add(0, Ordering::SeqCst); // SMA-702 MUTATION M2
```

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(an_open_breaker) | test(cloning_a_handle) | test(a_blackholed_backend)' 2>&1 | tee <scratchpad>/m2.log; echo "rc=${PIPESTATUS[0]}"`

Expected: the build succeeds and rc is 100. Exactly one test fails: `a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`, with the message `SMA-702 A4: command #1 really dialled the blackhole, but accepted() is 0`. S1 to S6 pass. This is why A4 exists: without it, a dead counter stays green everywhere (Review Focus 4).

- [ ] **Step 6: Restore M2**

Use the Edit tool to change the M2 line back to `accept_count.fetch_add(1, Ordering::SeqCst);`.
Run: `cd <worktree> && git diff --stat && grep -rn 'SMA-702 MUTATION' rs/`
Expected: an empty diff and no grep output.

- [ ] **Step 7: Re-run the S1 to S7 set after the restores**

Run: `cd <worktree>/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib -E 'test(an_open_breaker) | test(cloning_a_handle) | test(a_blackholed_backend)'`
Expected: all pass.

If the permission system refuses a mutation run, restore the file, and write the exact manual steps (Steps 3 to 6 above) under a "Mutation proof pending" heading in the notes for the PR body.

---

### Task 4: Local stress run under CPU load (A7, first half)

**Files:**
- Create (scratchpad only, not in the repo): `<scratchpad>/sma702-stress.sh`

**Interfaces:**
- Consumes: the committed Task 1 and Task 2 code.
- Produces: the stress result for the PR body (iteration count, failure count for S1 to S7).

- [ ] **Step 1: Write the stress script**

```bash
#!/bin/bash
# SMA-702 A7: stress S1-S7 while every core runs a busy loop. The trap stops the load on any exit.
set -u
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-702-open-breaker-dial-counter/rs || exit 2
pids=()
cleanup() { kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null; }
trap cleanup EXIT
# Build first, without load, so the load hits only the test run.
cargo nextest run -p paigasus-iam --lib --no-run || exit 2
for _ in $(seq "$(sysctl -n hw.ncpu)"); do yes >/dev/null & pids+=("$!"); done
cargo nextest run -p paigasus-iam --lib --no-fail-fast --stress-count 50 \
  -E 'test(an_open_breaker) | test(cloning_a_handle) | test(a_blackholed_backend)'
echo "nextest rc=$?"
```

- [ ] **Step 2: Run it in the foreground**

Run: `/bin/bash <scratchpad>/sma702-stress.sh 2>&1 | tee <scratchpad>/stress.log` with a Bash timeout of 600000 ms. Expected run time: about 6 to 8 minutes (S7 takes about 6.5 s per iteration).
Then run: `pgrep -x yes`
Expected: no output (the trap stopped every busy loop). If a `yes` process is left, stop it with `pkill -x yes`.

- [ ] **Step 3: Read the result**

Run: `grep -E 'FAIL|rc=|Summary|passed|failed' <scratchpad>/stress.log | tail -20`
Expected: `nextest rc=0` and 0 failures of S1 to S7. Two extra tests are in the filter (spec § 5.2); if one of them fails, record it separately, and do not count it as an S1 to S7 failure. Record the iteration count and the failure count for the PR body. If the run exceeds the timeout, lower `--stress-count` to 25, re-run, and record the count used.

---

### Task 5: CI evidence (A7, second half — a merge gate)

This task runs after the PR is open (the pipeline's open-PR stage pushes the branch). It commits nothing.

**Files:** none.

**Interfaces:**
- Consumes: the open PR and its `CI` workflow runs.
- Produces: three run IDs in which `paigasus-iam-rs:test` really ran, for the PR body.

- [ ] **Step 1: Know what counts**

`ci.yml:114-120` restores `.moon/cache` by the key `moon-${{ runner.os }}-${{ github.sha }}`. A re-run on the same SHA can replay the old PASS without running the tests. A run counts only when the `moon-diagnostics` or moon output shows `paigasus-iam-rs:test` executed (not `cached`). Three runs are required before the merge (Q3).

- [ ] **Step 2: Get three non-cached runs**

The first push gives one run. To get a new SHA without a code change, rebase onto a newer `origin/main` if one exists (then `git push --force-with-lease`), or push an empty commit only if Sven allows it. Otherwise, re-run the job and check whether the task was a cache hit. For each run, find the `paigasus-iam-rs:test` line in the job log with `gh run view <id> --log | grep 'paigasus-iam-rs:test'`, and confirm it is not marked `cached`.

- [ ] **Step 3: Record**

In the PR body, list the three run IDs, and for each run one line: "`paigasus-iam-rs:test` ran (not cached), S1 to S7 green". Also list the Task 3 mutation results and the Task 4 stress result. A7 is a merge gate: the PR is not ready for merge until all three runs are recorded.
