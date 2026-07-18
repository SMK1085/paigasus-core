# Harden outbox-relay observability tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the IAM outbox-relay's `ticks_total{result}` metric deterministic, timer-free test coverage on both the `ok` and `error` paths, and keep `run()`'s shutdown behavior covered.

**Architecture:** Extract the per-tick metric body out of `OutboxRelay::run()`'s `tokio::select!` poll arm into a small `pub async fn tick_and_record`, then drive it directly from tests (no poll loop, no timers). The error path is provoked by injecting `sea_orm::DatabaseConnection::Disconnected`, whose `begin()` returns `Err` synchronously — no Docker, no pool, no seeded row.

**Tech Stack:** Rust (edition 2024, rust 1.95), sea-orm 1.1.20, `metrics` crate, `paigasus-observability`, `tokio`, `cargo nextest`, Docker-gated Postgres via `testcontainers`.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (already present in both files — do not duplicate).
- Rust edition 2024, rust-version 1.95.
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean (docs on `pub` items, no unused imports).
- Conventional commits with workspace scope: subject starts lowercase, ≤100 chars; no `#NNN` GitHub refs in the body (a Linear key like `SMA-465` in the subject is fine).
- The refactor must be behavior-preserving: same counter name (`names::IAM_OUTBOX_RELAY_TICKS_TOTAL`), same `result` label values, same `tracing::warn!` line.
- Tests assume `nextest`'s process-per-test isolation for the process-global `metrics` recorder (the existing `tick_with_a_non_empty_batch_emits_relay_metrics` test already relies on this). CI runs `nextest`.

---

### Task 1: Extract `tick_and_record` from `run()` (behavior-preserving refactor)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs` (add method after `tick`, ~line 170; simplify `run`'s poll arm, ~lines 184–195)

**Interfaces:**
- Consumes: existing `OutboxRelay::tick(&self, publisher: &dyn EventPublisher) -> Result<TickReport, DbErr>`, `names::IAM_OUTBOX_RELAY_TICKS_TOTAL`.
- Produces: `pub async fn tick_and_record(&self, publisher: &dyn EventPublisher)` — runs one `tick` and increments `iam_outbox_relay_ticks_total{result="ok"|"error"}` accordingly (returns `()`). Consumed by `run` and by Task 2's tests.

- [ ] **Step 1: Add the `tick_and_record` method**

Insert immediately after the closing `}` of `tick` (before the `run` doc comment) in `relay.rs`:

```rust
    /// Runs one drain [`Self::tick`] and records its outcome on the `ticks_total{result}`
    /// run-loop counter (`result="ok"` on success; `result="error"` plus a `tracing::warn!`
    /// on a DB-level tick error). This is the exact body [`Self::run`] executes per poll
    /// interval, factored out so `run`'s only remaining logic is the `select!` shutdown loop.
    /// Intended for `run` and tests only — production callers should use [`Self::run`]; it is
    /// `pub` for the same reason [`Self::tick`] is: to let tests assert the ok/error tick
    /// counters deterministically without racing the poll loop (SMA-465).
    pub async fn tick_and_record(&self, publisher: &dyn EventPublisher) {
        match self.tick(publisher).await {
            Ok(_) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "ok").increment(1);
            }
            Err(err) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "error").increment(1);
                tracing::warn!(error = %err, "outbox relay tick failed; retrying next interval");
            }
        }
    }
```

- [ ] **Step 2: Collapse `run()`'s poll arm to call it**

Replace the `run` method body's `tokio::select!` poll arm. The whole `match self.tick(...) { ... }` block inside `() = tokio::time::sleep(self.poll_interval) => { ... }` becomes a single call. The final `run` reads:

```rust
    pub async fn run<S>(self, publisher: Arc<dyn EventPublisher>, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {
                    self.tick_and_record(publisher.as_ref()).await;
                }
                () = &mut shutdown => break,
            }
        }
    }
```

Leave `run`'s doc comment unchanged — it still accurately describes the loop (tick errors logged, loop continues).

- [ ] **Step 3: Verify it compiles and the lib unit tests + clippy pass**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo nextest run -p paigasus-iam --lib
cargo clippy -p paigasus-iam --all-targets -- -D warnings
```
Expected: lib tests PASS (the `row_to_domain_event` unit tests); clippy clean. This confirms the extraction compiles and integrates. (The Docker-gated `relay_pg` integration tests still reference `run()` and compile unchanged; they run in Task 2 / CI.)

- [ ] **Step 4: Confirm the refactor is byte-for-byte behavioral**

Visually diff: the counter name, both `result` label values, and the `warn!` message string are identical to the pre-refactor `run`. No other lines changed.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add rs/crates/services/paigasus-iam/src/adapters/events/relay.rs
git commit -m "refactor(rs): extract OutboxRelay::tick_and_record from run loop"
```

---

### Task 2: Deterministic ok/error tick-counter tests + shutdown-termination test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/relay_pg.rs` (replace `run_loop_emits_ticks_total_with_ok_result_label`, lines ~230–250; add two new tests)

**Interfaces:**
- Consumes: `OutboxRelay::tick_and_record` (Task 1), `OutboxRelay::run`, `OutboxRelay::new`, existing `CountingPublisher`, existing `seed_row`, `support::start_migrated_postgres`, `paigasus_observability::init`, `sea_orm::DatabaseConnection::Disconnected`.
- Produces: three tests — `tick_and_record_emits_ticks_total_with_ok_result`, `tick_and_record_emits_ticks_total_with_error_result_on_db_fault`, `run_terminates_on_shutdown`.

- [ ] **Step 1: Replace the wall-clock ok test with a timer-free one**

Delete the entire `run_loop_emits_ticks_total_with_ok_result_label` test (its doc comment through its closing `}`, lines ~230–250) and replace it with:

```rust
/// SMA-465 (replaces the old wall-clock-racing run-loop test): `tick_and_record` on a healthy,
/// non-empty tick emits `iam_outbox_relay_ticks_total{result="ok"}`. Driven directly — no poll
/// loop, no timers — so there is no wall-clock race; one row is seeded so it is a real successful
/// drain, not an empty no-op.
#[tokio::test]
async fn tick_and_record_emits_ticks_total_with_ok_result() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    seed_row(&db, Uuid::from_u128(8), Utc::now()).await;

    let handle = paigasus_observability::init("test-iam-relay-tick-ok");
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 10, 5);
    let publisher = CountingPublisher::default();

    relay.tick_and_record(&publisher).await;

    let out = handle.render();
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_relay_ticks_total") && l.contains(r#"result="ok""#)),
        "expected an iam_outbox_relay_ticks_total series labeled result=\"ok\":\n{out}"
    );
    assert!(
        !out.contains(r#"result="error""#),
        "a healthy tick must not emit a result=\"error\" series:\n{out}"
    );
}
```

- [ ] **Step 2: Add the error-path test (Docker-free)**

Append after the ok test:

```rust
/// SMA-465: the run-loop tick-error branch emits `iam_outbox_relay_ticks_total{result="error"}`.
/// `tick()` only returns `Err(DbErr)` on a DB-level fault (per-row publish failures are folded
/// into attempts/parked bookkeeping, never surfacing here), so we fault the DB deterministically
/// with `DatabaseConnection::Disconnected` — whose `begin()` returns `Err` synchronously — rather
/// than a failing publisher. No Docker, no pool, no seeded row.
#[tokio::test]
async fn tick_and_record_emits_ticks_total_with_error_result_on_db_fault() {
    let handle = paigasus_observability::init("test-iam-relay-tick-error");
    let relay = OutboxRelay::new(DatabaseConnection::Disconnected, Duration::from_secs(60), 10, 5);
    let publisher = CountingPublisher::default();

    relay.tick_and_record(&publisher).await;

    let out = handle.render();
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_relay_ticks_total") && l.contains(r#"result="error""#)),
        "expected an iam_outbox_relay_ticks_total series labeled result=\"error\":\n{out}"
    );
    assert!(
        !out.contains(r#"result="ok""#),
        "a faulted tick must not emit a result=\"ok\" series:\n{out}"
    );
}
```

- [ ] **Step 3: Add the shutdown-termination test (Docker-free)**

Append after the error test:

```rust
/// SMA-465: `run` terminates when its shutdown future resolves. A pre-resolved shutdown
/// (`std::future::ready(())`) makes the `select!` shutdown arm win deterministically before the
/// poll `sleep` is ever ready, so no tick fires and the connection is never used (`Disconnected`
/// is fine). Guards against a broken/removed `shutdown => break` arm (which would loop forever);
/// the `timeout` turns such a regression into a fast, explicit failure instead of a hang.
#[tokio::test]
async fn run_terminates_on_shutdown() {
    let relay = OutboxRelay::new(DatabaseConnection::Disconnected, Duration::from_secs(60), 10, 5);
    let publisher: Arc<dyn EventPublisher> = Arc::new(CountingPublisher::default());

    tokio::time::timeout(Duration::from_secs(5), relay.run(publisher, std::future::ready(())))
        .await
        .expect("run must return promptly once its shutdown future resolves");
}
```

- [ ] **Step 4: Run the new tests + clippy + fmt**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo nextest run -p paigasus-iam --test relay_pg
cargo clippy -p paigasus-iam --all-targets -- -D warnings
cargo fmt -p paigasus-iam -- --check
```
Expected:
- `tick_and_record_emits_ticks_total_with_error_result_on_db_fault` and `run_terminates_on_shutdown` PASS (they need no Docker).
- `tick_and_record_emits_ticks_total_with_ok_result` and the four pre-existing Docker-gated scenarios PASS if Docker is running, otherwise SKIP (return early) — same gating as their siblings.
- clippy clean, fmt clean.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add rs/crates/services/paigasus-iam/tests/relay_pg.rs
git commit -m "test(rs): cover outbox relay tick error path, drop wall-clock race (SMA-465)"
```

---

## Notes for the implementer

- `DatabaseConnection` is already imported in `relay_pg.rs` (`use sea_orm::{..., DatabaseConnection, ...}`); `Arc`, `Duration`, `Utc`, `Uuid`, `EventPublisher`, `CountingPublisher`, `seed_row` are all already in scope. `std::future::ready` is used via its full path — no new `use` needed. Do not add unused imports.
- After deleting the old ok test, `OutboxRelay::run` is still referenced (by `run_terminates_on_shutdown`) and `Arc<dyn EventPublisher>` is still used — no import will go stale.
- Before pushing, run the repo-level gates as CI does (this change adds no crates/deps/proto, so `:deny`/`:machete`/`:affected-smoke` should be no-ops, but confirm):
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`
