# SMA-702: the open-breaker tests prove "no dial" by a count, not by a 100 ms clock

- Linear: SMA-702
- Date: 2026-09-27
- Package: `rs/crates/services/paigasus-iam` (test code only)
- Related: SMA-476 (the circuit breaker and its tests), PR 316 run 36261530009 attempt 1 (the
  observed flake)
- Revision 3. Challenged once (verdict: APPROVE WITH CHANGES). See the challenge changelog.
- **Approved by Sven on 2026-09-28.** The answers to Q1 to Q4 are in § 10. The line references
  were re-checked against `origin/main` at `023755e4` (§ 12).

## 1. The problem

PR 316, run 36261530009, attempt 1, job "moon ci": the test
`adapters::authz::decision_cache::tests::an_open_breaker_keeps_the_decision_cache_failing_open`
failed at `decision_cache.rs:323` with "took 140.703695ms — the calls dialled instead of
short-circuiting". Attempt 2 passed with no code change. The PR did not touch the file.

The test forces the breaker OPEN and points the handle at a blackhole listener. A real dial
against that listener costs about 2.1 s (measured by
`a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`). The test uses a
wall-clock bound of 100 ms to prove that no dial happened. 140.7 ms is far below 2.1 s, so the
breaker short-circuited. The spec infers that the cause is a loaded CI runner. Nobody measured
the runner load for that run (see Q4).

The issue names four tests with the same bound. A repo search finds three more `< 100 ms`
short-circuit assertions of the same kind in `paigasus-iam`'s unit tests (measured on
`origin/main` at `83d446fc`):

| # | Site | Test | Target today |
|---|---|---|---|
| S1 | `src/adapters/authz/decision_cache.rs:323` | `an_open_breaker_keeps_the_decision_cache_failing_open` | blackhole |
| S2 | `src/adapters/authz/entity_cache.rs:315` | `an_open_breaker_falls_through_to_the_inner_slice_loader` | blackhole |
| S3 | `src/adapters/api_keys/cache.rs:417` | `an_open_breaker_keeps_the_api_key_cache_failing_open` | blackhole |
| S4 | `src/adapters/authz/generation.rs:571` | `an_open_breaker_keeps_redis_generations_propagating_the_error` | blackhole |
| S5 | `src/adapters/oidc/redis_cache.rs:113` | `an_open_breaker_keeps_the_jwks_cache_failing_closed` | blackhole |
| S6 | `src/adapters/redis_conn.rs:903` | `cloning_a_handle_shares_one_breaker` | closed port `127.0.0.1:1` |
| S7 | `src/adapters/redis_conn.rs:975` | `a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens` (commands #4 to #10) | blackhole |

S5, S6 and S7 are not in the issue. They have the same exposure, so this spec includes them
(decision D3). Sven approved all seven sites on 2026-09-28 (Q1). The line numbers in this
table are unchanged on `origin/main` at `023755e4`. One more bound of the same kind exists outside this set: see § 9
(`tests/nats_publisher.rs:327`).

## 2. Acceptance

- A1. S1 to S6 prove "no dial" with a count of accepted TCP connections on the blackhole. The
  count does not depend on wall-clock time. A regression that lets the call dial makes the count
  non-zero, and the test fails.
- A2. S1 to S5 keep a wall-clock bound as a stall backstop, widened from 100 ms to 1 s. Its
  failure message says that the count already proved no dial, so the clock failure is a stall,
  not a dial.
- A3. S6 moves to the blackhole (`test_support::start()`), asserts `accepted() == 0`, and drops
  its clock bound. S7 widens its per-command bound for #4 to #10 from 100 ms to 1 s. The S7 proof
  of "no dial" is the `BREAKER_OPEN_MESSAGE` check on the returned error. The S7 clock only
  catches stalls (see § 6 for why it cannot detect a dial that joins an in-flight reconnect).
- A4. The accepted-connection counter is proven to count: S7 asserts a non-zero count after a
  real dial, so the zero assertions in S1 to S6 cannot pass because the counter is broken.
- A5. The mutation battery in § 5.3 runs. Each mutation compiles, and each result is a test
  failure, not a build error (nextest reports failed tests, not a cargo rc 101 from rustc). M1
  reds exactly the 7 tests listed in § 5.3.
- A6. No production code changes. `cargo clippy --locked --all-targets -- -D warnings` (or
  `moon run paigasus-iam-rs:lint`), `cargo fmt --check` and `cargo nextest run -p paigasus-iam`
  pass. `--all-targets` is necessary: every change here is `#[cfg(test)]` code, and a plain
  `cargo clippy --workspace` does not compile it.
- A7. Flake evidence (issue AC "several consecutive CI runs with no new flake"). The
  deterministic proof is the count plus the mutation battery (A1, A4, A5). A7 covers only the
  remaining time dependency, the 1 s backstop. The PR records:
  - a local stress run of the S1 to S7 tests under induced CPU load (§ 5.4), and
  - at least three CI runs of `paigasus-iam-rs:test` in which the task actually ran. A CI run
    counts only if `ciReport.json` (or the moon output) shows the task was not a cache hit.
    `ci.yml:114-120` restores `.moon/cache` by the key `moon-${{ runner.os }}-${{ github.sha }}`,
    so a re-run on the same SHA replays the old PASS without running the tests.
  A7 is a merge gate: the stress run and the three CI runs happen before the merge (Q3,
  answered 2026-09-28).

## 3. Decisions

- **D1. A connection counter, not only a wider clock.** The issue offers two options: widen the
  bound, or count dials. A counter on the blackhole is easy to add: the accept loop in
  `redis_conn::test_support::start` already sees every connection. A count is exact and has no
  CI-load dependency. The issue's first option alone moves the flake threshold from 100 ms to
  1 s. It does not remove the dependency on time.
- **D2. Keep a widened clock bound in S1 to S5 as a stall backstop only.** The count covers a
  dial to the blackhole. The clock does not add dial detection. A retry sleep happens only inside
  a dial, which the count catches, and the breaker `Mutex` has no contention in a single-test
  process. The clock stays because the issue AC asks that the bound "still fails if the breaker
  stops short-circuiting", and it catches an unexpected stall of any cause. Its message says so
  (§ 4.2), so a red on it is not read as a dial. Sven approved this on 2026-09-28 (Q2): the 1 s
  clock stays only as a stall backstop in S1 to S5 and in S7. S6 has no clock (D5).
- **D3. Include S5, S6 and S7.** They have the identical `< 100 ms` shape and the same CI
  exposure. Leaving them makes the next flake a new ticket. The issue lists four sites because
  it was written from a search for the failure message, and S5 to S7 use other messages.
  Sven approved this on 2026-09-28 (Q1). This extends the scope of the Linear issue, and the
  issue description records the extension.
- **D4. No counter in S7.** In S7 the breaker opens after three real failures. `reconnect()`
  spawns a replacement dial the instant a command fails (`redis_conn.rs` doc on `OPEN_DURATION`;
  in the locked redis 1.7.0 the reconnect spawn is at `aio/connection_manager.rs:662`). That
  background dial can be accepted while commands #4 to #10 run. So the count can grow with no
  regression, and a "count does not grow" assertion would be a new flake. S7 keeps its
  error-message check and widens the clock only.
- **D5. S6 moves to the blackhole and uses the count.** S6 targets the closed port
  `127.0.0.1:1` today. A real dial there costs only about 100-200 ms (a refused first attempt
  plus a jittered `min_delay`, see the doc at `redis_conn.rs:554-556`). A 1 s clock bound could
  not detect it. So S6 points at `test_support::start()`, asserts `accepted() == 0`, and drops
  the clock. The `BREAKER_OPEN_MESSAGE` check stays. After this change, no posture test depends
  on a clock for its "no dial" proof. (Revision 1 kept S6 on the closed port with a 1 s clock.
  That clock was vacuous.)
- **D6. Why the count is exactly 0 in S1 to S6.** `with_open_breaker_for_tests` builds the
  handle with `ConnectionManager::new_lazy_with_config`. In the locked redis 1.7.0
  (`rs/Cargo.lock:4068-4069`) that call stores an un-polled lazy `Shared` connect future
  (`aio/connection_manager.rs:543-554`). The only spawn in `new_lazy_with_config` is
  `check_for_disconnect_pushes` (`:512`), and it does no network I/O. The breaker is forced open
  before any command. So with a working breaker, nothing polls the future, and the listener
  accepts nothing. This is a new coupling to redis-rs: `new_lazy_with_config` must not start the
  dial. § 4.1 adds it to the upgrade-coupling doc on `RedisHandle`, and § 4.2 names it in the
  count assertion's message.
- **D7. The counter is an `Arc<AtomicUsize>` incremented in the accept loop for every accepted
  connection, in both modes (blackholing and responding).** Exposed as
  `Blackhole::accepted(&self) -> usize` (`Ordering::SeqCst`, to match `responding`). It counts
  application-level `accept()` calls, not kernel handshakes. The name says so.
- **D8. The counter's two preconditions.**
  - The accept task must get polled while the test awaits. The kernel completes the TCP
    handshake at once, but the application `accept()` runs only when the runtime polls the
    accept task. Under `#[tokio::test]`'s current_thread runtime this happens because a real
    dial suspends the test task for at least about 1 s (the `connection_timeout`). That wait
    gives the runtime ample time to poll the accept task before the assertion runs. The
    `accepted()` doc states this.
  - The accept loop must stay alive. Today the loop ends on the first accept error
    (`let Ok(..) = listener.accept().await else { return };`, `redis_conn.rs:1145`). That drops
    the listener. After that, a dial is refused in microseconds, and both the count and the
    clock read "no dial": a false green. The loop changes to `continue` on an accept error.

## 4. Design

### 4.1 `src/adapters/redis_conn.rs`, `test_support` and the `RedisHandle` doc

- Add `accepted: Arc<AtomicUsize>` to `Blackhole`.
- In `start()`, clone it into the accept task. Increment it with `fetch_add(1, SeqCst)`
  immediately after `listener.accept()` returns `Ok`, before the `responding` branch.
- Change the accept loop so that an accept error does `continue`, not `return` (D8). Add a
  one-line comment: a dead loop would drop the listener and make every "no dial" count a
  false green.
- Add:

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

- Extend the struct doc with one sentence on the counter and why it replaces the clock in the
  posture tests.
- Add one item to the "Coupling to watch on a redis-rs upgrade" doc on `RedisHandle`
  (`redis_conn.rs:63`): `new_lazy_with_config` must not start the dial, or the `accepted() == 0`
  assertions in the open-breaker tests red with no breaker regression (SMA-702). Cite redis
  1.7.0 in the new text. The existing comments that cite 1.3.0 stay as they are (out of scope,
  § 9).

### 4.2 The five posture tests (S1 to S5)

In each test, after the calls and before the existing assertions:

```rust
assert_eq!(
    blackhole.accepted(),
    0,
    "SMA-702: the blackhole accepted a connection — the <call> dialled instead of \
     short-circuiting (or a redis-rs upgrade made new_lazy_with_config dial eagerly)"
);
assert!(
    elapsed < std::time::Duration::from_secs(1),
    "took {elapsed:?} — stall backstop only: the count above proved no dial, so this is \
     probably runner load, not a breaker regression"
);
```

Replace the old `from_millis(100)` assertion. The count assertion comes first, so a dial always
reds on the count. Update each test's doc comment: the count is the proof; the clock is a stall
backstop. The phrase "so the elapsed assertion proves the breaker short-circuited" changes to
name the count.

### 4.3 `redis_conn.rs` tests

- S6 (`cloning_a_handle_shares_one_breaker`): start a blackhole with `test_support::start()`,
  pass `&blackhole.url` to `with_open_breaker_for_tests`, keep the `BREAKER_OPEN_MESSAGE`
  assertion, add `assert_eq!(blackhole.accepted(), 0, ..)`, and delete the `started`/`elapsed`
  timing and the clock assertion (D5). The doc comment names the count.
- S7 (`a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`): the per-command
  bound for #4 to #10 becomes `from_secs(1)`. Update the message and the doc: the proof is the
  `BREAKER_OPEN_MESSAGE` check; the clock only catches stalls. The existing doc comment already
  says the bounds are "deliberately loose"; the new value matches that text.
- S7 also gets the A4 proof: after command #1, assert `blackhole.accepted() >= 1`, with a message
  that says the counter does not count and the S1 to S6 zero assertions are then vacuous. Use
  `>= 1`, not `== 1`, because the retry makes a second attempt.
- `an_open_breaker_short_circuits_asynccommands_without_dialling` (`redis_conn.rs:910`) stays on
  the closed port. It has no clock bound.

## 5. Tests

### 5.1 Red first

Before the counter exists, write the `accepted()` assertions in S1 and S7. The build fails
(no such method). That proves only that the method is new (see memory "Red-first is not proof").
§ 5.3 is the real proof.

### 5.2 Normal run

`cargo nextest run -p paigasus-iam -E 'test(an_open_breaker) | test(cloning_a_handle) |
test(a_blackholed_backend)'`, then the full `cargo nextest run -p paigasus-iam`.

The filter `test(an_open_breaker)` also selects two tests outside S1 to S7:
`an_open_breaker_still_recovers_after_many_short_circuits` (`redis_conn.rs:668`) and
`an_open_breaker_admits_a_probe_once_the_window_elapses` (`events/nats_publisher.rs:698`). They
are harmless in the run. When you count S1 to S7 results (§ 5.4), exclude them.

### 5.3 Proof that the tests bite

The workspace sets `[workspace.lints.rust] warnings = "deny"` (`rs/Cargo.toml:274-275`), and
`paigasus-iam` inherits it. So a mutation that leaves dead code or an unused binding stops at
rustc and proves nothing (memory "A mutation must COMPILE to prove anything"). Each mutation
below compiles. Run with `--no-fail-fast`. For each mutation, the report must show nextest test
failures, not a cargo build error (rc 101 from rustc). Restore each mutation by reverting the
marked edit, not with `git checkout --`.

- M1. In `Breaker::force_open_for_tests` (`redis_conn.rs:462-465`), change
  `BreakerState::Open` to `BreakerState::Closed`. Expected: exactly 7 red tests:
  - S1 to S5, each on the `accepted() == 0` assertion (the first failing message, which proves
    that the count alone catches the dial, because it runs before the clock),
  - S6 (`cloning_a_handle_shares_one_breaker`), on the count or the message assertion,
  - `an_open_breaker_short_circuits_asynccommands_without_dialling`, on the message assertion.
  Record the count of red tests and the first failing assertion message per test.
- M2. In `test_support::start`, change the increment to `fetch_add(0, Ordering::SeqCst)`.
  Expected: S7 red on the A4 `accepted() >= 1` assertion. S1 to S6 stay green (this is why A4
  exists).

(Revision 1 had an M3 that deleted the clock assertions. It is removed: it did not compile,
because `elapsed` then has no user, and M1 already proves that the count alone catches a dial.)

### 5.4 Flake evidence

- Local stress: `cargo nextest run -p paigasus-iam --stress-count 50 -E '<the § 5.2 filter>'`
  (nextest 0.9.136, the pinned version, has `--stress-count`), while every core runs a busy loop
  (for example one `yes > /dev/null` per core, stopped after the run). Do not install host
  software. The count of S1 to S7 failures must be 0.
- CI: at least three runs in which `paigasus-iam-rs:test` actually ran (not a cache hit, per A7).
- Record both results in the PR body.

## 6. Known limits

- The 1 s clock backstop in S1 to S5 can still fail on an extremely stalled runner. It is now a
  stall check with 10x more margin, and its message says that it is not a dial.
- The S7 clock cannot prove "no dial" by itself. A command that joins an in-flight reconnect
  pays only the remainder of that dial (see the `OPEN_DURATION` doc). The 1 s bound catches such
  a dial today only because a current_thread runtime does not poll the spawned reconnect between
  commands. The S7 proof is the `BREAKER_OPEN_MESSAGE` check. No count is possible there (D4).
- The count detects a dial to the blackhole only. A regression that dials a different address
  is not possible here: the handle has one URL.
- The count depends on the two preconditions in D8.

## 7. Documentation changes

- `docs/ops/RUNBOOK-observability.md:1721` says "commands 4–10 each cost under 100 ms once the
  breaker trips at command 3". That is S7's old assertion. After this change, no test asserts
  that figure. Reword the line, for example: "commands 4–10 short-circuit without dialling
  (asserted by the breaker-open error)".
- The `RedisHandle` upgrade-coupling doc (§ 4.1).
- The test doc comments.
- The `~2.1 s` figure in RUNBOOK text is about the real dial and does not change. No CLAUDE.md
  text names the 100 ms bound.

## 8. Files to touch

- `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` (`test_support`, the
  `RedisHandle` doc, S6, S7)
- `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs` (S1)
- `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs` (S2)
- `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs` (S3)
- `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs` (S4)
- `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs` (S5)
- `docs/ops/RUNBOOK-observability.md` (line 1721)

All Rust changes are inside `#[cfg(test)]` code or doc comments. Conventional commit:
`test(rs): ... (SMA-702)`. (The RUNBOOK line reword goes in the same commit; it describes the
test assertion.)

## 9. Out of scope

- Other wall-clock bounds in `redis_conn.rs` (the 2 s, 3.5 s and 14 s bounds). They are loose
  already and have no recorded flake.
- `tests/nats_publisher.rs:327` (a 200 ms "connection-state gate" bound of the same kind). It
  is an integration test. `rs/.config/nextest.toml:99-103` gives `package(=paigasus-iam) and
  kind(test)` two retries, so a flake there shows as FLAKY, not red. Record it; a follow-up
  issue can take it if it flakes.
- The existing code comments that cite redis 1.3.0 while the lock is 1.7.0.
- Any change to the breaker, its durations, or production code.
- A general "no-dial" helper shared across crates.

## 10. Open questions (all answered by Sven on 2026-09-28)

- Q1. Does Sven want S5, S6 and S7 in this ticket (D3), or only the four sites the issue names?
  The spec includes them; removing them is a scope cut with no design change.
  **Answer: fix all seven sites, S1 to S7.** D3 stands. This extends the issue's scope.
- Q2. Keep the 1 s clock backstop in S1 to S5 (D2), or rely on the count alone? The spec keeps
  it because the issue AC asks that the bound "still fails if the breaker stops
  short-circuiting". The challenge showed that the clock adds no dial detection that the count
  does not have, so its only value is a stall check.
  **Answer: keep a 1 s clock, only as a stall backstop, in S1 to S5 and in S7.** D2, A2 and A3
  stand. S6 drops its clock (D5).
- Q3. A7: does "several consecutive CI runs with no new flake" mean re-runs before the merge, or
  watching `main` after the merge? The spec reads it as a local stress run plus three CI runs
  that are not cache hits, before the merge. If Sven means `main` after the merge, A7 becomes a
  follow-up check and not a merge gate. The spec also reads "several" as three.
  **Answer: the proof is a local stress run plus three CI runs that are not cache hits, before
  the merge.** A7 is a merge gate. "Several" means three.
- Q4. The spec infers that the 140.7 ms stall came from runner load. Nobody measured it (no
  `moon-diagnostics` artifact or concurrency data was read). If the cause is something else, the
  1 s backstop may still be exposed. Does Sven want that measured before implementation?
  **Answer: no separate load measurement before implementation.** The runner-load cause stays
  an inference, and § 6 keeps the residual 1 s backstop risk.

## 11. Challenge changelog

Round 1, spec-challenger verdict: **APPROVE WITH CHANGES**. Each finding was checked against the
repo at `83d446fc`.

Folded:

- BLOCKER, mutations do not compile. Confirmed: `warnings = "deny"` at `rs/Cargo.toml:274-275`,
  `[lints] workspace = true` in `paigasus-iam`, and `force_open_for_tests` has one caller
  (`redis_conn.rs:150`). § 5.3 now uses M1 = `Open` to `Closed` in `force_open_for_tests`, and
  M2 = `fetch_add(0, ..)`. M3 is removed. A5 requires test failures, not rc 101.
- MAJOR, A7 evidence can be empty. Confirmed the cache key at `ci.yml:114-120`. A7 now requires
  CI runs that are not cache hits, plus a local `--stress-count` run under CPU load (nextest
  0.9.136 has the flag, checked). A7 now says that the deterministic proof is the count plus the
  mutations. The reading of the AC is Q3.
- MINOR, the S6 clock cannot detect a dial. Confirmed the 100-200 ms closed-port cost in the doc
  at `redis_conn.rs:554-556`. S6 moves to the blackhole with a count, and its clock goes (D5).
  S7's doc and § 6 now say the clock only catches stalls.
- MINOR, the RUNBOOK names the 100 ms figure. Confirmed at
  `docs/ops/RUNBOOK-observability.md:1721`. Added to § 7 and § 8.
- MINOR, the clippy command skips test code. Confirmed `--all-targets` in
  `.moon/tasks/rust.yml:79`. A6 fixed.
- MINOR, redis version citations. Confirmed 1.7.0 at `rs/Cargo.lock:4068-4069`. D4 and D6 cite
  1.7.0. The new coupling is added to the `RedisHandle` doc and the count message. The
  challenger's 1.7.0 line numbers are taken from the critique; this run did not open the
  redis-rs 1.7.0 source.
- MINOR, M1 red set incomplete. Confirmed 7 callers of `with_open_breaker_for_tests`. § 5.3
  lists all 7.
- MINOR, counter preconditions. Confirmed the `return` at `redis_conn.rs:1145`. Added D8, the
  `accepted()` doc text, and the `continue` change.
- MINOR, misleading backstop message. The message and D2 are rewritten.
- MINOR, narrow site search. Confirmed `tests/nats_publisher.rs:327` (200 ms) and the retries
  override at `rs/.config/nextest.toml:99-103`. Listed in § 9.
- MINOR, commit type. Now `test(rs)` only.
- QUESTIONS: the A7 timing question is folded into Q3; the runner-load question is Q4; the S6
  question is answered by D5.

Rejected: none.

## 12. Re-check against `origin/main` at `023755e4` (2026-09-28)

The spec was written at `83d446fc`. This re-check opened each cited path and line on
`origin/main` at `023755e4`. `git diff 83d446fc origin/main` shows no change in
`rs/crates/services/paigasus-iam`, `docs/ops/RUNBOOK-observability.md`,
`.github/workflows/ci.yml`, `rs/.config/nextest.toml`, `.moon/tasks/rust.yml` or `.prototools`.
`rs/Cargo.lock` and `rs/Cargo.toml` changed (the release commit `1a45803f` bumped crate
versions), but the two cited line ranges below still hold the same text.

Confirmed unchanged:

- S1 to S7 at the lines in the § 1 table. The seven `from_millis(100)` assertions are the only
  ones of that kind in the crate's unit tests.
- `with_open_breaker_for_tests` at `redis_conn.rs:148`, with its one `force_open_for_tests` call
  at `:150`, and seven callers.
- `force_open_for_tests` at `redis_conn.rs:462-465`.
- The closed-port cost doc ("~100-200 ms") at `redis_conn.rs:554-556`.
- The upgrade-coupling doc at `redis_conn.rs:63`, `start()` at `:1135`, and the accept-loop
  `return` at `:1145`.
- `an_open_breaker_short_circuits_asynccommands_without_dialling` at `redis_conn.rs:910`.
- redis 1.7.0 at `rs/Cargo.lock:4068-4069`, and `warnings = "deny"` at `rs/Cargo.toml:274-275`.
- The moon cache key at `.github/workflows/ci.yml:114-120` (the key is on line 118).
- `--all-targets` at `.moon/tasks/rust.yml:79`, and cargo-nextest 0.9.136 in `.prototools`.
- `docs/ops/RUNBOOK-observability.md:1721` and `tests/nats_publisher.rs:327`.

Changed in this revision:

- The nextest retries override is at `rs/.config/nextest.toml:99-103`, not at the repo-root
  path `.config/nextest.toml`. Fixed in § 9 and § 11.
- § 5.2 now says that the filter `test(an_open_breaker)` also selects two tests outside S1 to
  S7.
- Q1 to Q4 carry Sven's answers (§ 10), and D2, D3 and A7 name them.
