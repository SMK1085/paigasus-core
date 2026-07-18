# SMA-465 — Harden outbox-relay observability tests

**Status:** approved (design) — revised after adversarial challenge
**Date:** 2026-07-18
**Linear:** [SMA-465](https://linear.app/smaschek/issue/SMA-465/iam-harden-outbox-relay-observability-tests-error-path-coverage)
**Follows up:** SMA-446 #3 (observability, PR #89)

## Problem

PR #89 added per-tick metrics to the IAM `OutboxRelay`
(`rs/crates/services/paigasus-iam/src/adapters/events/relay.rs`). Two test-quality gaps
were deferred (the DoD only required the ok-path assertion):

1. **Untested error path.** `OutboxRelay::run()` emits
   `iam_outbox_relay_ticks_total{result="error"}` (plus a `tracing::warn!`) when a tick
   returns `Err`, but no test exercises that branch.
2. **Wall-clock race.** `run_loop_emits_ticks_total_with_ok_result_label` races a real
   ~10 ms poll interval against a ~300 ms shutdown window. The ~30× margin makes a flake
   unlikely, but a loaded/throttled CI runner could still miss the first tick.

### Key constraint discovered while reading the code

A **publisher** failure cannot trigger the `result="error"` branch. `tick()` catches
per-row publish errors and folds them into `attempts`/`parked` bookkeeping on the same
transaction; it only returns `Err(DbErr)` on a **database-level** fault (the `begin`,
`find`, `update`, or `commit`). So the error path must be provoked by faulting the DB
layer, not the publisher. The `ticks_total{result}` counter is emitted **only inside
`run()`**, never inside `tick()`.

## Goal

Both `iam_outbox_relay_ticks_total{result="ok"}` and `{result="error"}` are asserted by
**deterministic, timer-free** tests, and `run()`'s shutdown behavior stays covered. No
wall-clock racing anywhere.

## Approach

Extract the per-tick metric body out of `run()`'s `select!` arm into a small public
method the tests call directly. Chosen over a `start_paused` virtual-clock approach
because a plain async-fn call has **zero timing surface** — the strongest possible fix
for the CI-flake risk — and it makes the error path trivially reachable ("call it with a
faulted DB"). It follows an existing precedent in this file: `tick` itself was already
made `pub` "so tests can drive individual, deterministic ticks rather than racing the
poll loop."

### Production change — `relay.rs` (pure refactor, behavior identical)

Factor the poll arm's body into:

```rust
/// Runs one drain [`Self::tick`] and records its outcome on the `ticks_total{result}`
/// run-loop counter (`result="ok"` on success; `result="error"` + a `tracing::warn!` on a
/// DB-level tick error). This is the exact body [`Self::run`] executes per poll interval,
/// factored out so `run`'s only remaining logic is the `select!` shutdown loop. Intended
/// for `run` and tests only — not for production callers, who should use `run`; it is
/// `pub` for the same reason [`Self::tick`] is: to let tests assert the ok/error tick
/// counters deterministically without racing the poll loop (SMA-465).
pub async fn tick_and_record(&self, publisher: &dyn EventPublisher) {
    match self.tick(publisher).await {
        Ok(_) => counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "ok").increment(1),
        Err(err) => {
            counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "error").increment(1);
            tracing::warn!(error = %err, "outbox relay tick failed; retrying next interval");
        }
    }
}
```

`run()`'s poll arm collapses to `self.tick_and_record(publisher.as_ref()).await;`. The
`select!`/shutdown structure, the counter names, the label values, and the `warn!` line
are all unchanged — this is a mechanical extraction.

### Fault-injection choice (revised after challenge)

**Inject `sea_orm::DatabaseConnection::Disconnected`** as the relay's connection. Verified
against the vendored sea-orm 1.1.20 (`database/db_connection.rs`): `Disconnected` is a
public variant (line 42), is the `Default`, and its `begin()` returns
`Err(conn_err("Disconnected"))` **synchronously** (line 264) — so `tick()` errors at its
very first `await` (`relay.rs:105`), before any row access. The `tick_and_record` error
branch matches `Err(_)`, so the specific `DbErr` variant is irrelevant; `Disconnected` is
a faithful stand-in for the "dropped connection" the `run()` doc already cites.

Consequences: the error-path test needs **no Docker, no pool, no seeded row**, and runs
**everywhere** (not just CI) — closing the very gap this ticket exists to close on a
Docker-less laptop too. (This supersedes the earlier "close the connection pool" idea and
its `DROP TABLE` fallback, which were Docker-gated and required clone-pool reasoning.)

### Test changes — `tests/relay_pg.rs`

Three focused tests; assertions match a **single rendered line** (not two independent
substrings) and include a negative assertion so a regression that stops faulting/draining
fails loudly:

1. **Replace** `run_loop_emits_ticks_total_with_ok_result_label` with
   `tick_and_record_emits_ticks_total_with_ok_result` (Docker-gated — a real successful
   tick needs a real DB): seed one row (so it's a *real* non-empty successful drain, not
   an empty no-op), `init` observability, call `tick_and_record` on a **healthy** DB,
   assert one rendered line contains both `iam_outbox_relay_ticks_total` and
   `result="ok"`, and assert the render does **not** contain `result="error"`. No `run()`,
   no timers.
2. **Add** `tick_and_record_emits_ticks_total_with_error_result_on_db_fault` (plain
   `#[tokio::test]`, Docker-free): `init` observability, build the relay with
   `DatabaseConnection::Disconnected`, call `tick_and_record`, assert one rendered line
   contains both `iam_outbox_relay_ticks_total` and `result="error"`, and assert the
   render does **not** contain `result="ok"`.
3. **Add** `run_terminates_on_shutdown` (plain `#[tokio::test]`, Docker-free): drive the
   real loop with a pre-resolved shutdown — `relay.run(publisher, std::future::ready(()))`
   — and assert it returns. Because the relay's `run()` *breaks* on shutdown (it does not
   drain on it) and the poll `sleep` is never ready first, the shutdown arm always wins the
   `select!` and no tick fires, so the connection is never used (`Disconnected` is fine).
   This guards against a broken/removed `shutdown => break` arm (an infinite loop /
   never-terminating service). The poll arm's one-line `tick_and_record` call is a
   mechanical extraction whose metric effect is fully asserted by tests 1–2.

### Determinism / recorder notes

- The metric tests have no timing surface: `tick_and_record` is awaited directly. The
  `run()` test uses a pre-resolved shutdown, so it too has no wall-clock race.
- Each test calls `paigasus_observability::init(unique_name)` and asserts its own
  `handle.render()`. Verified: `init` installs the process-global `metrics` recorder
  behind a `OnceLock` (a second in-process call returns the cached handle — no panic, no
  fresh recorder), so per-test isolation comes **entirely** from `nextest`'s
  process-per-test model (`.moon/tasks/rust.yml`), not from the unique name. Same pattern
  the existing `tick_with_a_non_empty_batch_emits_relay_metrics` test already relies on.

## Out of scope

- No new clock abstraction, no `start_paused`, no change to `run()`'s select/shutdown
  semantics. (A `start_paused` one-tick loop test was considered to additionally assert
  the poll arm calls `tick_and_record` inside the loop; deemed unnecessary — that call is
  mechanical and its effect is covered by tests 1–2 — and rejected to honor the
  zero-timing-surface design.)
- No production dependency changes.
- The four existing drain / poison-parking / skip-locked / non-empty-batch-metrics tests
  are untouched.

## Verification

- `cargo nextest run -p paigasus-iam --test relay_pg` (the error-path and shutdown tests
  run without Docker; the ok-path test is Docker-gated like its siblings — skips locally
  without Docker, hard-fails in CI without Docker).
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- Repo gates as needed before push (`moon ci …`), though this change adds no crates,
  deps, or proto.

## Acceptance criteria

- [ ] `iam_outbox_relay_ticks_total{result="error"}` is asserted by a deterministic,
      Docker-free test that faults the DB via `DatabaseConnection::Disconnected`.
- [ ] The ok-path tick-counter test asserts `result="ok"` with no wall-clock racing.
- [ ] `run()`'s shutdown/termination stays covered by a deterministic test.
- [ ] `run()`'s metric behavior is a pure mechanical extraction into `tick_and_record`
      (same counters, labels, and `warn!`); the other existing relay tests still pass.
