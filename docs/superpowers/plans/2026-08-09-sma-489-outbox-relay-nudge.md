# SMA-489 Outbox Relay Commit-Nudge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wake the IAM outbox relay the moment a mutation commits — via Postgres `LISTEN`/`NOTIFY`, cross-replica — so event delivery is no longer gated by the 5 s poll interval.

**Architecture:** `PgOutbox::enqueue` emits `SELECT pg_notify('iam_outbox_event','')` *inside* the mutation's transaction; Postgres holds it until commit and drops it on rollback. A `PgOutboxListener` task owns a private 1-connection `PgPool`, `LISTEN`s, and pokes an `Arc<tokio::sync::Notify>`. `OutboxRelay::run` races that `Notify` against the existing poll sleep. Nudged ticks drain only never-attempted rows so retry cadence stays pinned to the poll interval.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM 1.1.20, `sea_orm::sqlx` (re-exported — **no new dependency**), tokio, `metrics`, testcontainers Postgres.

**Spec:** `docs/superpowers/specs/2026-08-09-sma-489-outbox-relay-nudge-design.md` — read it before starting. Decision IDs (D1-D15) below refer to its §2.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust edition 2024, rust-version 1.95. Workspace lints are `warnings = deny` — **dead code is a hard compile error on the lib target**, so never add an item in one task intending to wire it up in a later one. Every task must leave the crate compiling.
- **No new Cargo dependency.** `sea-orm-1.1.20/src/lib.rs:519` is `pub use sqlx;` and the workspace already enables `sqlx-postgres`, so use `sea_orm::sqlx::...` (D6). Adding `sqlx` directly would duplicate it in the tree and trip `:deny`/`:machete`.
- Channel name is the lowercase literal `iam_outbox_event`, everywhere, no config knob (D3).
- Bash tool PATH lacks proto CLIs — prefix every command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-489-outbox-relay-nudge` on branch `feature/sma-489-iam-wake-the-outbox-relay-on-commit-so-delivery-is-not-gated`. **Never commit to `main`.**
- Commit messages: conventional commit, workspace scope (`feat(rs):`), subject **lowercase** after the type and ≤100 chars, **no `#NNN`** anywhere in the body (it breaks commitlint's `footer-leading-blank`), one contiguous footer block. Write "SMA-489", never "#489".
- Run `cargo fmt` before every commit. Do **not** use `--no-verify`.
- Docker-gated tests follow the existing pattern: `let Some((_pg, db)) = support::start_migrated_postgres().await else { return };` — hard failure in CI, silent skip on a Docker-less laptop.

---

## File Structure

| File | Responsibility |
|---|---|
| `rs/crates/libs/paigasus-observability/src/names.rs` | Five new metric-name consts + `ALL` registration |
| `rs/crates/services/paigasus-iam/src/config.rs` | `wake_on_commit`, `wake_debounce_ms`, `listen_database_url` + defaults + validation |
| `.../src/adapters/events/relay.rs` | `TickMode`, `tick_with`, `tick_and_record` return type, lag histogram, the `run` loop |
| `.../src/adapters/persistence/pg_outbox.rs` | `PgOutbox::new(notify: bool)` + the `pg_notify` statement |
| `.../src/adapters/persistence/pg_outbox_listener.rs` | **New.** Owns sqlx, keepalives, reconnect, gauge/counters |
| `.../src/adapters/persistence/mod.rs` | Re-export `PgOutboxListener` |
| `.../src/adapters/http/mod.rs` | Five `PgOutbox::new()` call sites gain the flag |
| `.../src/main.rs` | `Arc<Notify>`, spawn listener, 5 × `describe_*!`, family count 32 → 37 |
| `.../tests/relay_nudge_pg.rs` | **New.** Notify semantics + D13 metering + backlog + listener |
| `.../tests/relay_pg.rs` | Existing `run` caller at line 296 |
| `ops/observability/prometheus/rules/iam.rules.yml` | `IamOutboxNotificationsAbsent` alert |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | promtool fixture for it |
| `docs/ops/RUNBOOK-observability.md` | §2.2 metric entries + a runbook section for the alert |

---

### Task 1: Config — three new `[outbox]` fields

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs`

**Interfaces:**
- Produces: `OutboxConfig::wake_on_commit: bool` (default `true`), `OutboxConfig::wake_debounce_ms: u64` (default `200`, validated non-zero), `OutboxConfig::listen_database_url: Option<String>` (default `None`). Later tasks read all three from `config.outbox`.

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn outbox_wake_defaults_are_on_with_a_200ms_debounce() {
    figment::Jail::expect_with(|jail| {
        jail.create_file("iam.toml", "database_url = \"postgres://x/y\"\n")?;
        let cfg = IamConfig::from_figment(IamConfig::figment()).expect("loads");
        assert!(cfg.outbox.wake_on_commit, "the nudge is on by default (SMA-489 D11)");
        assert_eq!(cfg.outbox.wake_debounce_ms, 200, "SMA-489 D14 default");
        assert_eq!(cfg.outbox.listen_database_url, None, "falls back to database_url");
        Ok(())
    });
}

#[test]
fn zero_wake_debounce_is_rejected() {
    let mut cfg = IamConfig::default_for_test();
    cfg.outbox.wake_debounce_ms = 0;
    let err = cfg.validate().expect_err("0 would remove the tick-rate floor (SMA-489 D14)");
    assert!(err.contains("wake_debounce_ms"), "{err}");
}
```

**Before writing these:** open `config.rs`'s test module and copy the *exact* idiom the neighbouring tests use to build and validate a config (there is an existing `validate_result(...)` helper used around line 2390, and `figment::Jail` usage around line 2358). Match it rather than inventing `default_for_test`/`from_figment` if those names do not exist — the two test bodies above are about the *assertions*, and the construction idiom must match the file.

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib config::tests::outbox_wake 2>&1 | tail -20
```
Expected: FAIL — `no field wake_on_commit on type OutboxConfig`.

- [ ] **Step 3: Add the fields to `OutboxConfig`**

In `config.rs`, inside `pub struct OutboxConfig` (around line 285), after `max_attempts`:

```rust
    /// SMA-489 D11. When `true` (the default) each `PgOutbox::enqueue` emits
    /// `pg_notify('iam_outbox_event','')` on the mutation's own transaction, and `main.rs`
    /// spawns the `PgOutboxListener` that turns those notifications into relay wakeups.
    ///
    /// Gates **both halves on purpose.** The writer is not free to leave on: a listener that
    /// wedges while still holding its `LISTEN` fills Postgres's async notification queue, and a
    /// full queue makes every transaction that calls `NOTIFY` **fail at commit** — i.e. every
    /// IAM mutation. An escape hatch that could not switch the writer off would not be one.
    ///
    /// `false` restores today's *wakeup* behaviour exactly (poll-only, no notify statement). It
    /// does NOT restore today's *drain* behaviour: the relay's backlog continuation (D9) is
    /// independent of this flag and stays active.
    pub wake_on_commit: bool,
    /// SMA-489 D14. Minimum gap between two nudge-driven ticks, in milliseconds (± up to 25%
    /// jitter so replicas do not converge). Validated non-zero.
    ///
    /// `Notify::notify_one` stores a permit, so under sustained write traffic there is always
    /// one pending and the relay would otherwise tick back-to-back with zero idle. NOTIFY is
    /// broadcast to every listening session, so R commits/s × N replicas produces R×N wakeups
    /// and `SKIP LOCKED` makes N-1 of those ticks do wasted work. At the design point
    /// (<10 mutations/s, 2-3 replicas) this is never reached; it bounds the worst case.
    ///
    /// Does NOT apply to the poll arm — that is already bounded by `poll_interval_secs`.
    pub wake_debounce_ms: u64,
    /// SMA-489 D6/§1.5. Connection string for the listener only; falls back to `database_url`.
    ///
    /// **`LISTEN` requires a direct connection or a SESSION-mode pooler.** PgBouncer's
    /// transaction and statement modes do not support it, and the failure is silent and total:
    /// `pg_notify` still succeeds on the writer side while the listener receives nothing
    /// forever. This field exists so a deployment that fronts Postgres with a transaction-mode
    /// pooler can point the listener at a direct endpoint without moving the main connection.
    /// `IamOutboxNotificationsAbsent` is the alert that detects the misconfiguration.
    pub listen_database_url: Option<String>,
```

- [ ] **Step 4: Add them to `OutboxDefaults` and both `Default` impls**

In `struct OutboxDefaults` (line 571), after `max_attempts: u32,`:

```rust
    wake_on_commit: bool,
    wake_debounce_ms: u64,
    listen_database_url: Option<String>,
```

In `impl Default for OutboxDefaults`, after `max_attempts: 60,`:

```rust
            wake_on_commit: true,
            wake_debounce_ms: 200,
            listen_database_url: None,
```

In `impl Default for OutboxConfig` (line 730), after `max_attempts: d.max_attempts,`:

```rust
            wake_on_commit: d.wake_on_commit,
            wake_debounce_ms: d.wake_debounce_ms,
            listen_database_url: d.listen_database_url,
```

- [ ] **Step 5: Add validation + the relay-disabled warning**

In `IamConfig::validate`, immediately after the existing `max_attempts == 0` check (around line 976):

```rust
        // SMA-489 D14: a zero debounce removes the tick-rate floor entirely, which is the
        // busy-loop the whole design exists to avoid.
        if self.outbox.wake_debounce_ms == 0 {
            return Err("outbox.wake_debounce_ms must be at least 1 (0 would remove the nudge tick-rate floor)".to_string());
        }
```

And immediately after the existing `relay_enabled = false` + `nats` rejection (around line 1035) — a **warning**, not a rejection, because the combination is harmless, merely pointless:

```rust
        // SMA-489 §3.4: no relay means nothing to wake. Not an error — just dead config.
        if !self.outbox.relay_enabled && self.outbox.wake_on_commit {
            tracing::warn!("outbox.wake_on_commit = true with outbox.relay_enabled = false — no relay is spawned, so no listener is spawned either and the setting has no effect");
        }
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib config:: 2>&1 | tail -20
```
Expected: PASS, and no other config test regresses.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/src/config.rs
git commit -m "feat(rs): add the outbox wake-on-commit config knobs (SMA-489)"
```

---

### Task 2: Register the five new metric families

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Produces: `names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL`, `names::IAM_OUTBOX_PUBLISH_LAG_SECONDS`, `names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL`, `names::IAM_OUTBOX_LISTENER_CONNECTED`, `names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL`. Tasks 3, 4, 6, 7 and 9 all consume these.

Safe to land alone: these are `pub const`s, which never trip `dead_code`.

- [ ] **Step 1: Add the consts**

In `names.rs`, after `IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL` (line 120):

```rust
/// Relay ticks, labeled by what woke them: `notify` (a Postgres `LISTEN` notification),
/// `poll` (the `poll_interval_secs` timer) or `backlog` (SMA-489 D9's continuation after a
/// full batch that made progress).
///
/// **One increment per TICK, not per wakeup** — so
/// `sum without (source) (iam_outbox_relay_wakeups_total)` equals
/// `sum without (result) (iam_outbox_relay_ticks_total)`, an invariant the integration tests
/// assert. All three label values are primed at zero when the relay starts: a metrics-rs series
/// first appears already at 1, so an `increase()` rule could otherwise never fire on the first
/// occurrence of a label value.
pub const IAM_OUTBOX_RELAY_WAKEUPS_TOTAL: &str = "iam_outbox_relay_wakeups_total";
/// End-to-end outbox latency: `now - occurred_at` at the moment a row is successfully
/// published. **This is the only signal that proves the SMA-489 nudge is working in
/// production.** [`IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS`] cannot: it is reset to 0 on
/// every empty tick, and the nudge makes empty ticks far more frequent.
pub const IAM_OUTBOX_PUBLISH_LAG_SECONDS: &str = "iam_outbox_publish_lag_seconds";
/// Notifications the `PgOutboxListener` actually received. Distinguishes "Postgres never
/// notified us — e.g. a transaction-mode pooler silently swallowed `LISTEN`" from "the relay
/// never observed the permit", which `iam_outbox_relay_wakeups_total{source="notify"}` alone
/// cannot (SMA-489 §1.5).
pub const IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL: &str = "iam_outbox_listener_notifications_total";
/// 1 when the outbox listener holds a live `LISTEN` connection, 0 otherwise.
///
/// **Per-replica, and the replicas do NOT agree** — the same caveat [`IAM_NATS_CONNECTED`]
/// carries. `max by (job)` returns 1 while any single replica is still connected, hiding
/// exactly the partial outage worth knowing about. Use `min by (job)` to ask "are all replicas
/// listening", or keep `instance` to see which one is down. Never `sum`.
pub const IAM_OUTBOX_LISTENER_CONNECTED: &str = "iam_outbox_listener_connected";
/// Outbox-listener reconnects. Driven by sqlx's `try_recv() -> Ok(None)` signal (it reconnects
/// internally and reports possible message loss that way), NOT by a `recv()` error — `recv()`
/// loops over the internal reconnect and would leave this at 0 through a real outage.
pub const IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL: &str = "iam_outbox_listener_reconnects_total";
```

- [ ] **Step 2: Register them in `ALL`**

In the `ALL` array, after `IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL,`:

```rust
    IAM_OUTBOX_RELAY_WAKEUPS_TOTAL,
    IAM_OUTBOX_PUBLISH_LAG_SECONDS,
    IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL,
    IAM_OUTBOX_LISTENER_CONNECTED,
    IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL,
```

- [ ] **Step 3: Run the registry tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-observability 2>&1 | tail -20
```
Expected: PASS — `all_names_are_unique_and_snake_case` covers both new-name properties.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): register the outbox nudge and listener metric families (SMA-489)"
```

---

### Task 3: `TickMode`, `tick_with`, and the publish-lag histogram

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs`

**Interfaces:**
- Consumes: Task 2's `names::IAM_OUTBOX_PUBLISH_LAG_SECONDS`.
- Produces: `pub enum TickMode { All, Fresh }`; `pub async fn tick_with(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr>`; `pub async fn tick_and_record(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr>`. `tick(&self, publisher)` keeps its current signature and delegates with `TickMode::All`, so the ~8 existing test call sites are untouched. Task 4 consumes all of these.

- [ ] **Step 1: Write the failing test**

Append to `relay.rs`'s `#[cfg(test)] mod tests`:

```rust
/// `TickMode::Fresh` is the D13 retry-metering mode; the two modes must be distinguishable
/// (a `TickMode` that collapsed to one value would silently un-meter retries).
#[test]
fn tick_modes_are_distinct() {
    assert_ne!(TickMode::All, TickMode::Fresh);
}
```

The real proof of `TickMode::Fresh` is the Postgres test in Task 8 (`attempts` advances once, not N); this unit test only pins the type down so Task 3 lands compiling and tested.

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::events::relay 2>&1 | tail -20
```
Expected: FAIL — `cannot find type TickMode`.

- [ ] **Step 3: Add `TickMode`**

Above `pub struct OutboxRelay`:

```rust
/// Which rows a tick may drain (SMA-489 D13).
///
/// The distinction exists to keep retry cadence pinned to `poll_interval_secs`. `tick`
/// increments `attempts` once per tick for every row it locks, and nothing throttles how often
/// a *nudged* tick happens — so if nudged ticks drained everything, a failing row would burn
/// its retry budget at the COMMIT rate. At 2 mutations/s a row would reach the default
/// `max_attempts = 60` in ~30 s instead of ~5 min, dead-lettering the in-flight backlog on a
/// routine broker restart, and voiding `IamConfig::validate`'s
/// `duplicate_window_secs > max_attempts × poll_interval_secs` dedup floor while leaving that
/// check passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickMode {
    /// Every unpublished, unparked row — the poll tick's mode, and the pre-SMA-489 behaviour.
    All,
    /// Only never-attempted rows (`attempts = 0`) — every nudge- and backlog-driven tick.
    ///
    /// A row that has failed once is invisible to nudged ticks and is retried only by the poll.
    /// Side benefit: fresh events are no longer head-of-line blocked behind a poison row on the
    /// nudge path.
    Fresh,
}
```

- [ ] **Step 4: Split `tick` into `tick` + `tick_with`**

Replace the `tick` signature and its query prologue. `tick` becomes:

```rust
    /// Runs exactly one drain tick over EVERY eligible row and returns its [`TickReport`].
    /// Equivalent to `tick_with(publisher, TickMode::All)`; kept as-is so existing callers and
    /// tests are unaffected.
    pub async fn tick(&self, publisher: &dyn EventPublisher) -> Result<TickReport, DbErr> {
        self.tick_with(publisher, TickMode::All).await
    }

    /// [`Self::tick`], restricted to `mode`'s row set (SMA-489 D13).
    pub async fn tick_with(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr> {
        let txn = self.db.begin().await?;

        let mut query = event_outbox::Entity::find()
            .filter(event_outbox::Column::PublishedAt.is_null())
            .filter(event_outbox::Column::Parked.eq(false));
        if mode == TickMode::Fresh {
            query = query.filter(event_outbox::Column::Attempts.eq(0));
        }
        let rows = query
            .order_by_asc(event_outbox::Column::Id)
            .limit(self.batch_size)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .all(&txn)
            .await?;
```

Everything from `let mut report = TickReport {` onward stays exactly as it is.

- [ ] **Step 5: Record the publish lag on each success**

Add `histogram` to the `metrics` import at the top:

```rust
use metrics::{counter, gauge, histogram};
```

In the per-row loop, in the `Ok(())` arm, replace:

```rust
                Ok(()) => {
                    active.published_at = Set(Some(Utc::now()));
                }
```

with:

```rust
                Ok(()) => {
                    let published_at = Utc::now();
                    // SMA-489: the only end-to-end proof the nudge works in production.
                    // `oldest_unpublished_age_seconds` cannot serve — it is reset to 0 on every
                    // empty tick, and the nudge makes empty ticks common.
                    histogram!(names::IAM_OUTBOX_PUBLISH_LAG_SECONDS)
                        .record(published_at.signed_duration_since(row.occurred_at).num_milliseconds() as f64 / 1000.0);
                    active.published_at = Set(Some(published_at));
                }
```

- [ ] **Step 6: Change `tick_and_record` to take a mode and return the report**

```rust
    /// Runs one [`Self::tick_with`] and records its outcome on the `ticks_total{result}`
    /// run-loop counter. Returns `tick_with`'s own `Result` so [`Self::run`]'s backlog
    /// continuation (SMA-489 D9) can read `drained`/`failures` and so an `Err` ends a
    /// continuation run instead of hot-looping a broken database.
    pub async fn tick_and_record(&self, publisher: &dyn EventPublisher, mode: TickMode) -> Result<TickReport, DbErr> {
        match self.tick_with(publisher, mode).await {
            Ok(report) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "ok").increment(1);
                Ok(report)
            }
            Err(err) => {
                counter!(names::IAM_OUTBOX_RELAY_TICKS_TOTAL, "result" => "error").increment(1);
                tracing::warn!(error = %err, "outbox relay tick failed; retrying next interval");
                Err(err)
            }
        }
    }
```

Update its one existing caller inside `run` to `self.tick_and_record(publisher.as_ref(), TickMode::All).await;` — Task 4 rewrites `run` properly; this keeps the crate compiling now. Add `let _ = ` if the unused-result warning fires.

- [ ] **Step 7: Fix existing `tick_and_record` test callers**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && grep -rn "tick_and_record" crates/services/paigasus-iam/tests/
```
Add `, TickMode::All` to each call and import `TickMode` where needed.

- [ ] **Step 8: Build and test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam 2>&1 | tail -20 && cargo test -p paigasus-iam --lib adapters::events::relay 2>&1 | tail -20
```
Expected: builds clean, relay unit tests PASS.

- [ ] **Step 9: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/src/adapters/events/relay.rs rs/crates/services/paigasus-iam/tests/
git commit -m "feat(rs): add TickMode and the outbox publish-lag histogram (SMA-489)"
```

---

### Task 4: The `run` loop — nudge, backlog continuation, debounce

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/relay_pg.rs` (the `run` caller at line 296)

**Interfaces:**
- Consumes: Task 2's `IAM_OUTBOX_RELAY_WAKEUPS_TOTAL`; Task 3's `TickMode`/`tick_and_record`.
- Produces: `pub async fn run<S>(self, publisher: Arc<dyn EventPublisher>, wake: Arc<tokio::sync::Notify>, shutdown: S)`; `pub fn with_wake_debounce(self, d: Duration) -> Self`. Task 7 consumes both.

`with_wake_debounce` is a builder rather than a fifth `new` parameter, so the ~10 existing `OutboxRelay::new(db, poll, batch, max_attempts)` call sites across the test suite stay untouched.

- [ ] **Step 1: Write the failing tests**

Append to `relay.rs`'s test module:

```rust
/// The D9 continuation predicate, isolated. The mixed case is what discriminates
/// `drained > failures` from the naive `failures == 0`: a single poison row sits at a fixed
/// FIFO position and reappears in every batch until it parks 60 attempts later, so
/// `failures == 0` would leave the continuation dead exactly when a deep backlog needs it.
#[test]
fn continuation_predicate_requires_a_full_batch_that_made_progress() {
    let batch = 100u64;
    let should_continue = |drained: u64, failures: u64| drained == batch && drained > failures;

    assert!(should_continue(100, 0), "full batch, all published");
    assert!(should_continue(100, 99), "full batch, one row published — still progress");
    assert!(!should_continue(100, 100), "full batch, nothing published — would hot-loop");
    assert!(!should_continue(99, 0), "partial batch — queue is drained");
}

```

The `run`-loop behavioural tests (notify wakeup, debounce, shutdown) need the metrics recorder
and so live in `tests/relay_pg.rs`, not here — see Step 5 below. Keep this task's unit test to
the pure predicate, which needs nothing.

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::events::relay 2>&1 | tail -20
```
Expected: FAIL — `with_wake_debounce` not found, `run` takes 2 arguments not 3.

- [ ] **Step 3: Add the debounce field and builder**

Add `wake_debounce: Duration,` to `pub struct OutboxRelay`, initialise it in `new` with `wake_debounce: Duration::from_millis(200),`, and add:

```rust
    /// Overrides the D14 nudge debounce (default 200 ms). Builder rather than a `new` parameter
    /// so the existing four-argument call sites across the test suite stay untouched.
    #[must_use]
    pub fn with_wake_debounce(mut self, d: Duration) -> Self {
        self.wake_debounce = d;
        self
    }
```

- [ ] **Step 4: Rewrite `run`**

Replace the whole `run` method:

```rust
    /// Runs the relay loop until `shutdown` resolves.
    ///
    /// Three things can start a tick: the `poll_interval` timer (draining every eligible row,
    /// `TickMode::All`), a `wake` permit from the `PgOutboxListener` (SMA-489), or SMA-489 D9's
    /// backlog continuation. The latter two use `TickMode::Fresh` so retry cadence stays pinned
    /// to `poll_interval` (D13).
    ///
    /// **Shutdown is checked BETWEEN ticks, never raced AROUND one.** Racing `shutdown` against
    /// the tick itself would cancel it mid-flight, rolling back a transaction whose events the
    /// publisher may already have accepted — SMA-471 D3's unbounded-republish gap, on every
    /// graceful shutdown.
    ///
    /// SOUNDNESS: `S: Future` is not `FusedFuture`, and polling a completed future is a contract
    /// violation. This shape is sound only because EVERY path that observes `shutdown` ready
    /// breaks the loop immediately. Preserve that if you restructure, or switch to a
    /// `CancellationToken`/`watch::Receiver`, which are poll-after-ready safe.
    pub async fn run<S>(self, publisher: Arc<dyn EventPublisher>, wake: Arc<tokio::sync::Notify>, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);

        // SMA-489 D12: prime every label value at zero. A metrics-rs series first appears
        // already at 1, so an `increase()` rule could never fire on a label's first occurrence.
        for source in ["notify", "poll", "backlog"] {
            counter!(names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL, "source" => source).increment(0);
        }

        'outer: loop {
            // `biased` so a ready shutdown always beats a ready notify permit. Without it the
            // choice is random, an extra tick can run after shutdown, and the tests that assert
            // otherwise become flaky. It costs nothing: the tick is not inside this select.
            let mut source = tokio::select! {
                biased;
                () = &mut shutdown => break 'outer,
                () = wake.notified() => "notify",
                () = tokio::time::sleep(self.poll_interval) => "poll",
            };
            let mut mode = if source == "poll" { TickMode::All } else { TickMode::Fresh };

            loop {
                counter!(names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL, "source" => source).increment(1);
                let Ok(report) = self.tick_and_record(publisher.as_ref(), mode).await else {
                    break; // already logged and counted; never hot-loop a broken database
                };
                // D9: continue only on a FULL batch that made progress. `drained > failures`
                // rather than `failures == 0` so one poison row cannot disable the continuation.
                if report.drained < self.batch_size || report.drained <= report.failures {
                    break;
                }
                // Poll shutdown WITHOUT cancelling anything, then keep draining.
                let stopping = std::future::poll_fn(|cx| std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_ready())).await;
                if stopping {
                    break 'outer;
                }
                source = "backlog";
                mode = TickMode::Fresh;
            }

            // D14: floor the nudge-driven tick rate. The poll arm is already bounded.
            if source != "poll" {
                let jitter = 0.75 + rand::random::<f64>() * 0.5;
                let delay = self.wake_debounce.mul_f64(jitter);
                tokio::select! {
                    biased;
                    () = &mut shutdown => break 'outer,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
```

Add to the imports at the top of the file:

```rust
use std::task::Poll;
```
(only if you reference `Poll` unqualified — the code above uses `std::task::Poll` inline, so this may be unnecessary. Do not add an unused import; `warnings = deny`.)

- [ ] **Step 5: Update the existing `run` caller and add the run-loop tests in `relay_pg.rs`**

At `tests/relay_pg.rs:296`, change:

```rust
    tokio::time::timeout(Duration::from_secs(5), relay.run(publisher, std::future::ready(())))
```
to:

```rust
    tokio::time::timeout(
        Duration::from_secs(5),
        relay.run(publisher, std::sync::Arc::new(tokio::sync::Notify::new()), std::future::ready(())),
    )
```

Then append three tests. They need no Docker: `DatabaseConnection::Disconnected` faults every
tick instantly, which still proves which `select!` arm fired. Follow the file's existing
`paigasus_observability::init("<unique-name>")` + `handle.render()` idiom (see
`tick_and_record_emits_ticks_total_with_error_result_on_db_fault` at line ~270).

```rust
/// SMA-489 AC1's mechanism: a notify permit starts a tick without waiting out the poll
/// interval. The 600s interval means a `source="notify"` series can ONLY appear if the notify
/// arm fired.
#[tokio::test]
async fn a_notify_permit_wakes_the_run_loop_before_the_poll_interval() {
    let handle = paigasus_observability::init("test-iam-relay-wake-notify");
    let wake = Arc::new(tokio::sync::Notify::new());
    let relay = OutboxRelay::new(DatabaseConnection::Disconnected, Duration::from_secs(600), 10, 5)
        .with_wake_debounce(Duration::from_millis(1));
    let publisher: Arc<dyn EventPublisher> = Arc::new(CountingPublisher::default());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let w = wake.clone();
    let run = tokio::spawn(async move { relay.run(publisher, w, async move { let _ = rx.await; }).await });

    wake.notify_one();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(5), run).await.expect("run exits").expect("no panic");

    let out = handle.render();
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_relay_wakeups_total") && l.contains(r#"source="notify""#) && !l.trim_end().ends_with(" 0")),
        "expected a non-zero source=\"notify\" wakeup — the notify arm never fired:\n{out}"
    );
}

/// SMA-489 AC7/D14: the debounce floors the nudge-driven tick rate. 200 notifications delivered
/// as fast as possible must NOT produce 200 ticks.
#[tokio::test]
async fn the_debounce_bounds_the_nudge_driven_tick_rate() {
    let handle = paigasus_observability::init("test-iam-relay-wake-debounce");
    let wake = Arc::new(tokio::sync::Notify::new());
    let relay = OutboxRelay::new(DatabaseConnection::Disconnected, Duration::from_secs(600), 10, 5)
        .with_wake_debounce(Duration::from_millis(100));
    let publisher: Arc<dyn EventPublisher> = Arc::new(CountingPublisher::default());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let w = wake.clone();
    let run = tokio::spawn(async move { relay.run(publisher, w, async move { let _ = rx.await; }).await });

    let started = std::time::Instant::now();
    for _ in 0..200 {
        wake.notify_one();
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let elapsed = started.elapsed();
    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(5), run).await.expect("run exits").expect("no panic");

    // Debounce is 100ms ± 25% jitter, so the floor is 75ms; allow generous headroom.
    let ceiling = (elapsed.as_millis() / 75) as u64 + 2;
    let ticks = ticks_total_from(&handle.render());
    assert!(ticks <= ceiling, "{ticks} ticks in {elapsed:?} exceeds the debounce ceiling of {ceiling}");
    assert!(ticks >= 1, "the debounce suppressed every tick");
}

/// SMA-489 AC9/D10: with a permit already pending AND shutdown resolved, the `biased` select
/// must pick shutdown — no extra tick after shutdown.
#[tokio::test]
async fn a_pending_permit_does_not_win_a_race_against_a_resolved_shutdown() {
    let handle = paigasus_observability::init("test-iam-relay-wake-shutdown-bias");
    let wake = Arc::new(tokio::sync::Notify::new());
    wake.notify_one(); // permit stored BEFORE run starts
    let relay = OutboxRelay::new(DatabaseConnection::Disconnected, Duration::from_secs(600), 10, 5);
    let publisher: Arc<dyn EventPublisher> = Arc::new(CountingPublisher::default());

    tokio::time::timeout(Duration::from_secs(5), relay.run(publisher, wake, std::future::ready(())))
        .await
        .expect("run must return promptly even with a permit pending");

    let out = handle.render();
    assert!(
        !out.lines().any(|l| l.contains("iam_outbox_relay_ticks_total") && !l.trim_end().ends_with(" 0")),
        "a tick ran despite shutdown being ready — the outer select! is not biased:\n{out}"
    );
}
```

Write a small `fn ticks_total_from(rendered: &str) -> u64` helper that sums the
`iam_outbox_relay_ticks_total` sample values out of the Prometheus exposition text, and a
matching `fn wakeups_total_from(rendered: &str) -> u64` for
`iam_outbox_relay_wakeups_total`. Then add one more assertion to
`the_debounce_bounds_the_nudge_driven_tick_rate`, covering AC3's invariant:

```rust
    assert_eq!(
        wakeups_total_from(&handle.render()),
        ticks_total_from(&handle.render()),
        "wakeups_total must increment exactly once per tick, or no ratio query against ticks_total is valid"
    );
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::events::relay 2>&1 | tail -30
cargo nextest run --no-tests=pass -p paigasus-iam --test relay_pg 2>&1 | tail -30
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/src/adapters/events/relay.rs rs/crates/services/paigasus-iam/tests/relay_pg.rs
git commit -m "feat(rs): wake the relay loop on a notify permit and drain backlogs (SMA-489)"
```

---

### Task 5: `PgOutbox` emits the notification

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (lines 347, 460, 482, 619, 639)

**Interfaces:**
- Consumes: Task 1's `config.outbox.wake_on_commit`.
- Produces: `PgOutbox::new(notify: bool) -> PgOutbox`. Task 7's composition root passes the config value.

- [ ] **Step 1: Check for other constructors**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && grep -rn "PgOutbox::new\|PgOutbox::default\|PgOutbox {" crates/ --include=*.rs
```
Every hit must be updated in this task. `Default` is being removed deliberately — a default that silently disables the nudge is a trap — so any `PgOutbox::default()` becomes `PgOutbox::new(true)`.

- [ ] **Step 2: Change the struct and constructor**

```rust
/// `enqueue` never touches `&self` beyond reading [`Self::notify`] — all writes go to the
/// caller-supplied transaction — but the port is injected as `Arc<dyn Outbox>` (mirroring every
/// other adapter here), so a tiny value type is the simplest shape satisfying that convention.
///
/// Deliberately NOT `Default`: the only sensible default for `notify` is `true`, and a
/// `Default` that silently shipped `false` would disable SMA-489 with no diagnostic.
#[derive(Clone, Copy)]
pub struct PgOutbox {
    notify: bool,
}

impl PgOutbox {
    /// `notify` mirrors `[outbox].wake_on_commit` (SMA-489 D11).
    #[must_use]
    pub fn new(notify: bool) -> Self {
        PgOutbox { notify }
    }
}
```

- [ ] **Step 3: Emit the notification in `enqueue`**

```rust
/// The Postgres channel the relay's `PgOutboxListener` subscribes to (SMA-489 D3). Lowercase
/// on purpose: sqlx emits `LISTEN "iam_outbox_event"` (quoted, case-preserving) while
/// `pg_notify` takes the channel as a VALUE — the two agree only while the name has no
/// uppercase.
const WAKE_CHANNEL: &str = "iam_outbox_event";

#[async_trait]
impl Outbox for PgOutbox {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        event_to_model(ev).insert(txn).await.map_err(map_err)?;
        if self.notify {
            // SMA-489 D2. Emitted INSIDE the caller's transaction on purpose: Postgres buffers
            // the notification and delivers it ONLY if that transaction commits, discarding it
            // on rollback. That is what makes "signal after commit" structural here rather than
            // a rule every call site has to remember.
            //
            // The payload is empty (D3): the relay re-queries for work anyway, and an empty
            // payload means a hostile session that LISTENs on this channel — they are
            // database-wide and unprivileged — learns only that SOME mutation happened, never
            // which principal or event type.
            //
            // NOTE (D4): if Postgres's async notification queue is FULL this does not fail
            // here — the transaction fails at COMMIT instead, surfacing from
            // `SeaOrmTransaction::commit` as an opaque backend error. That is why
            // `[outbox].wake_on_commit` gates this writer and not only the listener.
            txn.execute(Statement::from_string(DbBackend::Postgres, format!("SELECT pg_notify('{WAKE_CHANNEL}', '')")))
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }
}
```

Update the imports:

```rust
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbBackend, Set, Statement};
```

- [ ] **Step 4: Update the five call sites in `adapters/http/mod.rs`**

`AppState::new` already takes `cfg: &IamConfig`. At each of lines 347, 460, 482, 619, 639 change `PgOutbox::new()` to `PgOutbox::new(cfg.outbox.wake_on_commit)`.

- [ ] **Step 5: Build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam 2>&1 | tail -20
```
Expected: clean. Fix any remaining constructor from Step 1.

- [ ] **Step 6: Run the outbox tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --test outbox_uow_pg 2>&1 | tail -20
```
Expected: PASS (or skip without Docker) — enqueue still writes the row; the notify is additive.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): notify the outbox channel from inside the mutation txn (SMA-489)"
```

---

### Task 6: The `PgOutboxListener` adapter

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox_listener.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`

**Interfaces:**
- Consumes: Task 2's four listener/notification metric names.
- Produces: `PgOutboxListener::new(url: String, wake: Arc<tokio::sync::Notify>, watchdog: Duration) -> PgOutboxListener` and `pub async fn run<S>(self, shutdown: S) where S: Future<Output = ()> + Send`. Task 7 spawns it.

- [ ] **Step 1: Create the adapter**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `PgOutboxListener` (SMA-489): turns Postgres `LISTEN` notifications into relay wakeups.
//!
//! The writer half is `PgOutbox::enqueue`, which emits `pg_notify('iam_outbox_event','')` inside
//! each mutation's own transaction — Postgres holds it until commit and drops it on rollback
//! (D2). This half owns the subscription and pokes an `Arc<Notify>` that `OutboxRelay::run`
//! races against its poll sleep (D5). The two never reference each other; the `Notify` is the
//! whole interface, which is what keeps sqlx out of the relay and the drain loop out of here.
//!
//! **Never fatal (D7).** A listener that cannot connect logs, zeroes its gauge and retries
//! forever; boot never fails and the replica never leaves rotation. Delivery simply reverts to
//! the poll interval meanwhile, which is why the poll is retained (D8).
//!
//! **Why `try_recv` and not `recv` (D15).** `PgListener` defaults to `eager_reconnect: true` and
//! reconnects INTERNALLY inside `try_recv` (`sqlx-postgres-0.8.6/src/listener.rs:285-299`),
//! re-issuing `LISTEN`; `recv()` loops over that, so it almost never returns `Err`. Driving the
//! gauge off `recv()` errors would have left `iam_outbox_listener_connected` pinned at 1 and
//! `..._reconnects_total` at 0 straight through a real outage. With `eager_reconnect(false)`,
//! `try_recv() -> Ok(None)` is the explicit "reconnected, may have missed notifications" signal.
//!
//! **Why TCP keepalives (D15).** sqlx sets none and `try_recv` has no read timeout, so a
//! silently-dropped connection leaves Postgres believing this session is alive and LISTENing —
//! the half-open case that fills the async notification queue. A full queue makes every
//! transaction calling `NOTIFY` fail AT COMMIT, i.e. every IAM mutation (D4). Keepalives surface
//! a dead peer within ~60 s and the ordinary reconnect path handles it.

use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use tokio::sync::Notify;

/// The channel `PgOutbox::enqueue` notifies. Must match `pg_outbox::WAKE_CHANNEL`.
const WAKE_CHANNEL: &str = "iam_outbox_event";

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// Subscribes to [`WAKE_CHANNEL`] and pokes `wake` on every notification.
pub struct PgOutboxListener {
    url: String,
    wake: Arc<Notify>,
    watchdog: Duration,
}

impl PgOutboxListener {
    /// `watchdog` bounds how long the listener stays silent before warning. It NEVER forces a
    /// reconnect: silence is the normal state of a quiet deployment (no mutations means no
    /// notifications), so reconnecting on it would churn a connection every period while proving
    /// nothing. Keepalives handle real death; this only gives an operator a log line to
    /// correlate with `iam_outbox_listener_notifications_total` staying flat.
    #[must_use]
    pub fn new(url: String, wake: Arc<Notify>, watchdog: Duration) -> Self {
        PgOutboxListener { url, wake, watchdog }
    }

    /// Opens a PRIVATE single-connection pool with keepalives set. Private on purpose (D6): a
    /// slot taken from SeaORM's pool would compete with request handling and with the relay's
    /// own tick, which already holds one for `batch_size × publish-latency`. Going through a
    /// pool at all is forced by `PgListener::connect(&str)` not accepting connect options.
    async fn connect(&self) -> Result<PgListener, sea_orm::sqlx::Error> {
        let opts = PgConnectOptions::from_str(&self.url)?
            .keepalives(true)
            .keepalives_idle(Duration::from_secs(30))
            .keepalives_interval(Duration::from_secs(10))
            .keepalives_retries(3);
        let pool = PgPoolOptions::new().max_connections(1).connect_with(opts).await?;
        let mut listener = PgListener::connect_with(&pool).await?;
        listener.eager_reconnect(false);
        listener.listen(WAKE_CHANNEL).await?;
        Ok(listener)
    }

    /// Runs until `shutdown` resolves. Shutdown is raced against the backoff sleep AND the
    /// connect attempt, not only against `try_recv`: with a 30 s backoff cap on top of sqlx's
    /// 30 s pool acquire timeout, a replica whose Postgres is unreachable could otherwise take
    /// ~a minute to honour SIGTERM, and SMA-471 D11 already flagged overrunning
    /// `terminationGracePeriodSeconds` as a real problem for this service.
    pub async fn run<S>(self, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
        counter!(names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL).increment(0);
        counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(0);

        let mut backoff = BACKOFF_START;
        let mut connected_before = false;

        'outer: loop {
            let listener = tokio::select! {
                biased;
                () = &mut shutdown => break 'outer,
                r = self.connect() => r,
            };

            let mut listener = match listener {
                Ok(l) => l,
                Err(e) => {
                    // NEVER log `self.url` — `IamConfig.database_url` is not redacted in the
                    // config's derived Debug/Serialize, unlike `PublisherConfig::url`.
                    tracing::warn!(error = %e, "outbox listener could not connect; delivery stays poll-only until it recovers");
                    gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
                    tokio::select! {
                        biased;
                        () = &mut shutdown => break 'outer,
                        () = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_CAP);
                    continue;
                }
            };

            backoff = BACKOFF_START;
            gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(1.0);
            if connected_before {
                counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(1);
            }
            connected_before = true;
            tracing::info!(channel = WAKE_CHANNEL, "outbox listener connected");

            loop {
                let received = tokio::select! {
                    biased;
                    () = &mut shutdown => break 'outer,
                    r = listener.try_recv() => r,
                    () = tokio::time::sleep(self.watchdog) => {
                        tracing::warn!(
                            silent_for_secs = self.watchdog.as_secs(),
                            "outbox listener has received no notification for a while — normal on a quiet deployment, but if mutations ARE committing check that the connection is not fronted by a transaction-mode pooler (LISTEN is unsupported there)"
                        );
                        continue;
                    }
                };

                match received {
                    Ok(Some(_)) => {
                        counter!(names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL).increment(1);
                        // Coalescing lives here: `notify_one` stores at most ONE permit, so a
                        // burst arriving mid-tick yields exactly one extra tick, and a
                        // notification arriving with no waiter registered is not lost.
                        self.wake.notify_one();
                    }
                    // `eager_reconnect(false)` makes this "the connection dropped and was
                    // re-established; notifications may have been missed". The poll covers the
                    // gap (D8) — Postgres does not queue for an absent listener.
                    Ok(None) => {
                        counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(1);
                        tracing::warn!("outbox listener reconnected; notifications during the gap were dropped and will be picked up by the poll");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "outbox listener connection failed; reconnecting");
                        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
                        break;
                    }
                }
            }
        }

        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
        tracing::info!("outbox listener stopped");
    }
}
```

**Note on cancellation:** the watchdog arm cancels an in-flight `try_recv`. That is safe here only because the arm `continue`s without touching the connection, and any partially-read notification is re-read on the next `try_recv`. If sqlx's `try_recv` turns out not to be cancel-safe, drop the watchdog arm entirely — keepalives are the mechanism that matters and the watchdog is observability only.

- [ ] **Step 2: Re-export**

In `adapters/persistence/mod.rs`, beside `pub use pg_outbox::PgOutbox;`:

```rust
mod pg_outbox_listener;
pub use pg_outbox_listener::PgOutboxListener;
```
Match the module-declaration style already used in that file.

- [ ] **Step 3: Verify the sqlx API surface compiles**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam 2>&1 | tail -30
```
If `keepalives_retries` or `keepalives_interval` do not exist on `PgConnectOptions` 0.8.6, check the real surface and keep whichever keepalive setters do exist:
```bash
grep -n "pub fn keepalive" ~/.cargo/registry/src/*/sqlx-postgres-0.8.6/src/options/mod.rs
```

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt && cargo clippy -p paigasus-iam -- -D warnings 2>&1 | tail -20
git add rs/crates/services/paigasus-iam/src/adapters/persistence/
git commit -m "feat(rs): add the postgres outbox listener adapter (SMA-489)"
```

---

### Task 7: Composition root

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`

**Interfaces:**
- Consumes: Tasks 1, 2, 4, 6.

- [ ] **Step 1: Wire the notify + listener into the relay block**

In the `if config.outbox.relay_enabled` block (around line 246), replace the body:

```rust
        if config.outbox.relay_enabled {
            // SMA-489: the relay and the listener share one `Arc<Notify>` — the listener pokes
            // it on every `iam_outbox_event` notification, `run` races it against the poll
            // sleep. Created here (not in `AppState`) because both consumers live in this block.
            let wake = std::sync::Arc::new(tokio::sync::Notify::new());
            let mut rx = rx.clone();
            let relay = OutboxRelay::new(
                db,
                Duration::from_secs(config.outbox.poll_interval_secs),
                config.outbox.batch_size,
                i32::try_from(config.outbox.max_attempts).unwrap_or(i32::MAX),
            )
            .with_wake_debounce(Duration::from_millis(config.outbox.wake_debounce_ms));
            let relay_wake = wake.clone();
            servers.spawn(async move {
                relay
                    .run(publisher, relay_wake, async move {
                        let _ = rx.changed().await;
                    })
                    .await;
                Ok(())
            });

            if config.outbox.wake_on_commit {
                // The listener gets its own connection string: `LISTEN` needs a direct or
                // session-mode connection, so a deployment behind a transaction-mode pooler can
                // point it elsewhere without moving the main pool (SMA-489 §1.5).
                let listen_url = config.outbox.listen_database_url.clone().unwrap_or_else(|| config.database_url.clone());
                // Watchdog is observability-only (D15): warn on silence, never reconnect on it.
                let watchdog = std::cmp::max(Duration::from_secs(60), Duration::from_secs(config.outbox.poll_interval_secs * 3));
                let listener = PgOutboxListener::new(listen_url, wake, watchdog);
                let mut rx = rx.clone();
                servers.spawn(async move {
                    listener
                        .run(async move {
                            let _ = rx.changed().await;
                        })
                        .await;
                    Ok(())
                });
            } else {
                tracing::info!("outbox.wake_on_commit = false — no commit notification is emitted and no listener runs; delivery is gated by outbox.poll_interval_secs");
            }
        } else {
```

Add `PgOutboxListener` to the `paigasus_iam::adapters::persistence::{...}` import at line 12.

- [ ] **Step 2: Describe the five new metrics**

In `describe_iam_metrics`, after the `IAM_OUTBOX_DEAD_LETTERS_*` describes:

```rust
    describe_counter!(
        names::IAM_OUTBOX_RELAY_WAKEUPS_TOTAL,
        "Relay ticks by what woke them: notify (a Postgres LISTEN notification), poll (the poll_interval_secs timer) or backlog (the continuation after a full batch that made progress). One increment per TICK, so sum without (source) equals sum without (result) of iam_outbox_relay_ticks_total."
    );
    describe_histogram!(
        names::IAM_OUTBOX_PUBLISH_LAG_SECONDS,
        "End-to-end outbox latency: now - occurred_at when a row is successfully published. The only signal that proves the commit-nudge is working; iam_outbox_oldest_unpublished_age_seconds cannot, as it resets to 0 on every empty tick."
    );
    describe_counter!(
        names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL,
        "Notifications the outbox listener received. Flat at zero while rows are being drained means LISTEN is not reaching this replica — most likely a transaction-mode connection pooler, which silently does not support it."
    );
    describe_gauge!(
        names::IAM_OUTBOX_LISTENER_CONNECTED,
        "1 when the outbox listener holds a live LISTEN connection, 0 otherwise. Per-replica and the replicas do NOT agree — aggregate with min by (job), never sum or max."
    );
    describe_counter!(
        names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL,
        "Outbox-listener reconnects. Climbing means Postgres is churning the listener connection; notifications during each gap are dropped and picked up by the poll."
    );
```

- [ ] **Step 3: Update the family count in the doc comment**

`main.rs:396`: change `the 32 metric families` to `the 37 metric families`, and extend the parenthetical with `, and the SMA-489 commit-nudge/listener families`.

- [ ] **Step 4: Build and run the whole package**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam 2>&1 | tail -20 && cargo clippy -p paigasus-iam -- -D warnings 2>&1 | tail -20
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/src/main.rs
git commit -m "feat(rs): spawn the outbox listener alongside the relay (SMA-489)"
```

---

### Task 8: Postgres integration tests

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`

**Interfaces:**
- Consumes: everything above.

These are the tests that prove the acceptance criteria. Use `support::start_migrated_postgres()` and the Docker-gated `else { return }` idiom. Copy `seed_row` from `relay_pg.rs:61` (direct entity insert — deliberately **not** through `PgOutbox::enqueue`, which would emit a notification per row and let the backlog test pass with the continuation loop deleted).

- [ ] **Step 1: Write the tests**

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-489 integration tests: the commit-nudge's Postgres semantics, D13's retry metering, and
//! D9's backlog continuation, against a real Postgres.

mod support;

// ... imports mirroring tests/relay_pg.rs, plus:
// use paigasus_iam::adapters::persistence::{PgOutbox, PgOutboxListener, SeaOrmUnitOfWork};
// use paigasus_iam::adapters::events::{OutboxRelay, TickMode};
// use sea_orm::sqlx::postgres::PgListener;

/// D2, the load-bearing claim: a notification emitted inside a transaction is delivered ONLY on
/// commit. The listening session is a DIFFERENT connection, which is also what makes this the
/// cross-replica proof — a separate replica is a separate session and the mechanism does not
/// distinguish them.
#[tokio::test]
async fn a_notification_arrives_only_after_the_enqueuing_transaction_commits() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");

    // Still uncommitted: nothing may arrive.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), listener.recv()).await.is_err(),
        "a notification arrived before commit — the whole after-commit guarantee is broken"
    );

    tx.commit().await.expect("commit");

    tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect("notification arrives promptly after commit")
        .expect("recv ok");
}

/// D2's other half: a rolled-back mutation must never nudge.
#[tokio::test]
async fn a_rolled_back_mutation_emits_no_notification() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    {
        let tx = uow.begin().await.expect("begin");
        outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
        // dropped without commit -> rollback
    }

    assert!(
        tokio::time::timeout(Duration::from_millis(500), listener.recv()).await.is_err(),
        "a rolled-back mutation nudged the relay"
    );
}

/// **AC6, the most important test in this file (D13).** A failing row's `attempts` must advance
/// at most once per poll interval no matter how many nudges arrive — otherwise the retry budget
/// burns at the commit rate and `duplicate_window_secs > max_attempts × poll_interval_secs`
/// stops describing reality while still validating.
#[tokio::test]
async fn nudged_ticks_do_not_burn_a_failing_rows_retry_budget() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(1);
    seed_row(&db, id, Utc::now()).await;

    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), 10, 60);
    let publisher = FailingPublisher;

    // One poll-mode tick takes the row to attempts = 1.
    relay.tick_with(&publisher, TickMode::All).await.expect("poll tick");

    // Ten nudge-mode ticks must all skip it.
    for _ in 0..10 {
        let report = relay.tick_with(&publisher, TickMode::Fresh).await.expect("fresh tick");
        assert_eq!(report.drained, 0, "a nudged tick must not touch an already-attempted row");
    }

    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert_eq!(row.attempts, 1, "attempts advanced more than once — D13's metering is broken");
}

/// AC4/D9: a full batch that made progress keeps draining without waiting a poll interval.
#[tokio::test]
async fn one_wakeup_drains_a_backlog_larger_than_the_batch() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let batch_size = 10u64;
    for i in 1..=25u128 {
        seed_row(&db, Uuid::from_u128(i), Utc::now()).await;
    }

    let wake = Arc::new(tokio::sync::Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), batch_size, 60)
        .with_wake_debounce(Duration::from_millis(1));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let publisher = Arc::new(CountingPublisher::default());

    let w = wake.clone();
    let p = publisher.clone();
    let handle = tokio::spawn(async move { relay.run(p, w, async move { let _ = rx.await; }).await });

    wake.notify_one();

    // 600s poll interval: only the continuation can drain past the first batch of 10.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while publisher.count() < 25 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(publisher.count(), 25, "the backlog continuation did not drain past one batch");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("run exits").expect("no panic");
}

/// AC5/D9: when NO row in a batch publishes, the continuation must stop rather than hot-loop.
#[tokio::test]
async fn a_totally_failing_publisher_stops_the_backlog_continuation() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let batch_size = 5u64;
    for i in 1..=20u128 {
        seed_row(&db, Uuid::from_u128(i), Utc::now()).await;
    }

    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), batch_size, 60)
        .with_wake_debounce(Duration::from_millis(1));
    let publisher = Arc::new(CountingAlwaysFailingPublisher::default());
    let wake = Arc::new(tokio::sync::Notify::new());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let w = wake.clone();
    let p = publisher.clone();
    let handle = tokio::spawn(async move { relay.run(p, w, async move { let _ = rx.await; }).await });

    wake.notify_one();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Exactly one batch attempted: drained == batch_size but drained == failures, so no continue.
    assert_eq!(publisher.count(), batch_size as usize, "the continuation kept going with a fully failing publisher");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("run exits").expect("no panic");
}

/// AC8/D7: a listener pointed at an unreachable database must not return, panic, or report
/// connected — it retries forever while delivery stays poll-only.
#[tokio::test]
async fn a_listener_with_an_unreachable_database_keeps_retrying_without_failing() {
    let wake = Arc::new(tokio::sync::Notify::new());
    let listener = PgOutboxListener::new(
        "postgres://nobody:nobody@127.0.0.1:1/nonexistent".to_string(),
        wake,
        Duration::from_secs(60),
    );
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move { listener.run(async move { let _ = rx.await; }).await });

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!handle.is_finished(), "the listener gave up instead of retrying (D7 says never fatal)");

    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(40), handle).await.expect("listener honours shutdown").expect("no panic");
}

/// AC10/D11: `wake_on_commit = false` must emit NO notification at all — the writer is gated,
/// not only the listener. (It does not disable D9's backlog continuation; that is deliberate
/// and documented on the config field.)
#[tokio::test]
async fn wake_on_commit_false_emits_no_notification() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(false); // the escape hatch
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    assert!(
        tokio::time::timeout(Duration::from_millis(750), listener.recv()).await.is_err(),
        "wake_on_commit = false still emitted a notification — the writer is not gated"
    );

    // ...and the row is still there for the poll to drain.
    let rows = event_outbox::Entity::find().all(&db).await.expect("query");
    assert_eq!(rows.len(), 1, "the outbox row itself must be written regardless of the flag");
}

/// AC8/D15: a killed listener backend must be noticed and reconnected, with BOTH the gauge and
/// the reconnect counter moving — the failure mode the original error-driven design would have
/// missed entirely, since sqlx reconnects internally.
#[tokio::test]
async fn a_killed_listener_backend_reconnects_and_still_delivers() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;
    let handle = paigasus_observability::init("test-iam-listener-reconnect");

    let wake = Arc::new(tokio::sync::Notify::new());
    let listener = PgOutboxListener::new(url.clone(), wake.clone(), Duration::from_secs(300));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let listen_handle = tokio::spawn(async move { listener.run(async move { let _ = rx.await; }).await });

    tokio::time::sleep(Duration::from_millis(750)).await;

    // Kill every backend that is LISTENing on our channel (not our own admin connection).
    db.execute_unprepared(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE query LIKE '%iam_outbox_event%' AND pid <> pg_backend_pid()",
    )
    .await
    .expect("terminate listener backend");

    // Give the listener time to notice and re-establish.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // A notification after the reconnect must still land.
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let t = uow.begin().await.expect("begin");
    outbox.enqueue(&*t, &sample_event()).await.expect("enqueue");
    t.commit().await.expect("commit");

    let woke = tokio::time::timeout(Duration::from_secs(10), wake.notified()).await;
    let _ = tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), listen_handle).await;

    assert!(woke.is_ok(), "the listener never delivered a notification after its backend was killed");
    let out = handle.render();
    assert!(
        out.lines().any(|l| l.contains("iam_outbox_listener_reconnects_total") && !l.trim_end().ends_with(" 0")),
        "reconnects_total never moved — liveness is not being detected:\n{out}"
    );
}

/// AC9/D10: shutdown must NEVER cancel an in-flight tick. A publisher that blocks until
/// released lets us signal shutdown mid-tick; the tick's transaction must still commit, so
/// `published_at` is stamped. A cancelled tick would roll back and leave it NULL — SMA-471 D3's
/// unbounded-republish gap on every graceful shutdown.
#[tokio::test]
async fn shutdown_during_a_tick_does_not_cancel_it() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let id = Uuid::from_u128(42);
    seed_row(&db, id, Utc::now()).await;

    let gate = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let publisher = Arc::new(BlockingPublisher { gate: gate.clone(), entered: entered.clone() });

    let wake = Arc::new(tokio::sync::Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_millis(50), 10, 5);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let run = tokio::spawn({
        let p = publisher.clone();
        let w = wake.clone();
        async move { relay.run(p, w, async move { let _ = rx.await; }).await }
    });

    entered.notified().await;          // the tick is inside publish()
    let _ = tx.send(());               // shutdown NOW, mid-tick
    tokio::time::sleep(Duration::from_millis(200)).await;
    gate.notify_one();                 // let the publish finish

    tokio::time::timeout(Duration::from_secs(10), run).await.expect("run exits").expect("no panic");

    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert!(row.published_at.is_some(), "the in-flight tick was cancelled by shutdown — its transaction rolled back");
}

/// AC1: end to end, with a live listener and a 600s poll interval, a committed mutation is
/// published in well under a second.
#[tokio::test]
async fn a_committed_mutation_is_published_without_waiting_for_the_poll() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;

    let wake = Arc::new(tokio::sync::Notify::new());
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(600), 100, 60)
        .with_wake_debounce(Duration::from_millis(1));
    let publisher = Arc::new(CountingPublisher::default());
    let (tx_relay, rx_relay) = tokio::sync::oneshot::channel::<()>();
    let (tx_listen, rx_listen) = tokio::sync::oneshot::channel::<()>();

    let w = wake.clone();
    let p = publisher.clone();
    let relay_handle = tokio::spawn(async move { relay.run(p, w, async move { let _ = rx_relay.await; }).await });
    let listener = PgOutboxListener::new(url, wake.clone(), Duration::from_secs(60));
    let listen_handle = tokio::spawn(async move { listener.run(async move { let _ = rx_listen.await; }).await });

    tokio::time::sleep(Duration::from_millis(500)).await; // let LISTEN establish

    let started = std::time::Instant::now();
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    let deadline = started + Duration::from_secs(10);
    while publisher.count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let elapsed = started.elapsed();
    assert_eq!(publisher.count(), 1, "the event was never published");
    assert!(elapsed < Duration::from_millis(1000), "published after {elapsed:?}, expected well under a poll interval");

    let _ = tx_relay.send(());
    let _ = tx_listen.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), relay_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), listen_handle).await;
}
```

Helpers this file needs, all local to it unless noted:

- `support::connection_url` may not exist — check `tests/support/mod.rs` around
  `start_migrated_postgres` (line 65) for how it builds its connection string, and add a
  `pub async fn connection_url(pg: &ContainerAsync<Postgres>) -> String` there reusing the same
  host/port logic if there is no equivalent.
- `seed_row` — copy verbatim from `relay_pg.rs:61`.
- `sample_event()` — build a `DomainEvent` the way `relay.rs`'s `base_model()` does
  (`EventType::PrincipalCreated`, `schema_version: 1`, a `prn:pgs:iam:::principal/<uuid>`
  aggregate, `payload: {"kind":"user"}`), with a fresh UUID per call.
- `CountingPublisher` with a `count()` accessor — copy from `relay_pg.rs`.
- `CountingAlwaysFailingPublisher` — counts calls, always returns `Err`.
- `BlockingPublisher { gate: Arc<Notify>, entered: Arc<Notify> }` — its `publish` signals
  `entered`, awaits `gate`, then returns `Ok(())`.

- [ ] **Step 2: Run them**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --test relay_nudge_pg 2>&1 | tail -40
```
Expected: all PASS (Docker required — they skip silently without it, which is NOT a pass; confirm Docker is running).

- [ ] **Step 3: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt
git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): cover the outbox commit-nudge end to end (SMA-489)"
```

---

### Task 9: Alert rule, promtool fixture, and runbook

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`
- Modify: `docs/ops/RUNBOOK-observability.md`

**Interfaces:**
- Consumes: Task 2's metric names (the `:observability-drift` gate fails if a rule references an unregistered family).

- [ ] **Step 1: Add the alert**

In `iam.rules.yml`, beside the other outbox alerts, matching their exact indentation and
`labels`/`annotations` shape (copy from `IamOutboxRelayStalled` at line 29):

```yaml
      - alert: IamOutboxNotificationsAbsent
        expr: (sum by (job) (increase(iam_outbox_listener_notifications_total[30m])) == 0) and (sum by (job) (increase(iam_outbox_relay_drained_total[30m])) > 0)
```

Rows are being written and drained, yet no notification has arrived in 30 minutes — the
signature of `LISTEN` not reaching this deployment at all, most often a transaction-mode pooler.
Give it `severity: warning` and a `runbook_url`/`summary` in the same style as its neighbours.

- [ ] **Step 2: Add the promtool fixture**

In `tests/iam.test.yml`, add a test group. **Include a control series** — an all-firing fixture
cannot distinguish `== 0` from `>= 0`, so assert both the firing and the non-firing case:

```yaml
  # SMA-489: notifications absent while rows drain -> fires. Notifications present -> silent.
  - interval: 1m
    input_series:
      - series: 'iam_outbox_listener_notifications_total{job="iam",instance="a"}'
        values: '0+0x40'          # flat: no notification ever arrives
      - series: 'iam_outbox_relay_drained_total{job="iam",instance="a"}'
        values: '0+5x40'          # rows ARE being drained
      - series: 'iam_outbox_listener_notifications_total{job="iam-healthy",instance="b"}'
        values: '0+5x40'          # control: notifications flowing
      - series: 'iam_outbox_relay_drained_total{job="iam-healthy",instance="b"}'
        values: '0+5x40'
    alert_rule_test:
      - eval_time: 35m
        alertname: IamOutboxNotificationsAbsent
        exp_alerts:
          - exp_labels:
              severity: warning
              job: iam
            # copy the exact annotation text from the rule
```

The `iam-healthy` job must produce **no** alert — that is the control that proves the rule is not
vacuously true.

- [ ] **Step 3: Run promtool**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool 2>&1 | tail -30
```
Expected: PASS. If the alert fires for `iam-healthy` too, the `and` clause is wrong.

- [ ] **Step 4: Update the runbook**

In `docs/ops/RUNBOOK-observability.md` §2.2, add the five families with the same one-line format
the neighbouring entries use. Then add a runbook section beside the other
`### IamOutbox*` entries:

```markdown
### `IamOutboxNotificationsAbsent` — commit nudges are not arriving (warning)

Rows are being written and drained, but the listener has received no `iam_outbox_event`
notification for 30 minutes. Delivery has silently fallen back to `[outbox].poll_interval_secs`
(~5 s), which is correct but not what this deployment is configured for.

Most likely causes, in order:

1. **A transaction- or statement-mode connection pooler** in front of Postgres. PgBouncer's
   `transaction` and `statement` modes do not support `LISTEN` — the writer's `pg_notify` still
   succeeds, so nothing else looks wrong. Point `[outbox].listen_database_url` at a direct or
   session-mode endpoint.
2. `[outbox].wake_on_commit = false` — check the effective config; the service logs an `info`
   line at boot when the nudge is disabled.
3. The listener is down: check `iam_outbox_listener_connected` with `min by (job)` (never `max`
   or `sum` — the replicas do not agree) and `iam_outbox_listener_reconnects_total`.

**If IAM mutations are also failing at commit** with an opaque backend error, suspect a full
async notification queue — a listening session that stopped consuming prevents Postgres from
truncating it. Check `SELECT pg_notification_queue_usage();` (1.0 means full). Setting
`[outbox].wake_on_commit = false` and restarting stops the writer emitting notifications and
restores mutations immediately.
```

- [ ] **Step 5: Commit**

```bash
git add ops/observability/prometheus/rules/ docs/ops/RUNBOOK-observability.md
git commit -m "feat(ops): alert when outbox commit nudges never arrive (SMA-489)"
```

---

### Task 10: Full gate run

- [ ] **Step 1: Run the complete CI graph**

Per-project tasks do NOT run the repo-level gates.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations 2>&1 | tail -40
```

- [ ] **Step 2: Diagnose any failure**

Moon reports an unattributed "N failed". Get the real task:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm no new dependency crept in**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && git diff origin/main --stat -- Cargo.toml Cargo.lock crates/services/paigasus-iam/Cargo.toml
```
Expected: **empty**. Any change here means `sqlx` was added directly, contrary to D6.

- [ ] **Step 4: Commit any fixes and push**

```bash
git push -u origin feature/sma-489-iam-wake-the-outbox-relay-on-commit-so-delivery-is-not-gated
```
