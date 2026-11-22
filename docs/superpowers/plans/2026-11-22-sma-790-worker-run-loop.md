# SMA-790 Worker Run Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the agent-host worker. A worker takes the lease of an inbox, claims a batch, makes one
Helikon run from it, and records the result in one fenced transaction that stays correct when the
worker dies at any step.

**Architecture:** The core crate gets the new value types and ports (`RunCommitter` with four
methods, `EventScreen`, `FaultPoint`, the event renderer and the retry delay). The service crate gets
the adapters (`PgRunCommitter`, `PgRunSession`, `ConfigAgentResolver`, `ModelRegistry`,
`HelikonAgentRunner`) and the application layer (`LeaseKeeper` changes, `RunWorker`,
`run_worker_pool`). The keeper stays the only owner of the `LeaseHandle`, and every write goes through it.

**Tech Stack:** Rust edition 2024, tokio, SeaORM 2.0.4 on Postgres 16, `paigasus-helikon =0.6.12`
(core 0.6.0, `runtime-tokio` 0.1.26), `paigasus-helikon-sessions-testkit =0.1.2` (dev only),
testcontainers, nextest.

**Spec:** `docs/superpowers/specs/2026-11-22-sma-790-worker-run-loop-design.md` (approved at Gate 1
on 2026-11-22). Read it with this plan. Do not re-open its decisions. §8 records an accepted risk (no
guard for a poison memory file). Do not add a guard.

**Not in this plan:** §6.13 (the ADR-0028 amendment). It is already written to Notion.

Paths in this plan use two short names:

- `core/` = `rs/crates/libs/paigasus-agent-host-core/`
- `svc/` = `rs/crates/services/paigasus-agent-host/`

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Rust edition 2024, rust-version 1.95 (workspace values; do not change them).
- The workspace sets `warnings = "deny"`. A task must not add a private or `pub(crate)` item that a
  later task first uses. A `pub` item of a library crate is not dead code. Each task must pass
  `cargo clippy --workspace --all-targets -- -D warnings` on its own.
- OOP, domain-driven design and dependency injection. `paigasus-agent-host-core` must not depend on
  Helikon, SeaORM or tokio. Its non-dev dependency set stays exactly the `ALLOWED` list in
  `core/src/lib.rs` (`the_core_crate_depends_on_no_io_crate`). Every type in a core port is a core
  type. The service converts from the Helikon types.
- Hexagonal: ports in the core (or, for a port whose types hold a tokio type, in
  `svc/src/application/worker/ports.rs`), adapters in `svc/src/adapters/`.
- Every test helper in `svc/tests/support/` carries
  `#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]`.
- No file base name is a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
  `LPT1`–`LPT9`). Do not name a file `nul.rs` for the U+0000 helper.
- Conventional commits with the workspace scope (`feat(rs): …`, `test(rs): …`,
  `refactor(rs): …`). Every commit message ends with the line
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Do not put a
  `#NNN` line in the commit body (commitlint `footer-leading-blank`).
- Shell setup for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`, then
  `cd rs`.
- Per-crate checks: `cargo nextest run -p <crate> …`, `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`. From Task 14 on, also run
  `cargo clippy -p paigasus-agent-host --all-targets --features test-model -- -D warnings`, because
  the Moon `lint` task does not enable the feature.
- From Task 14 on, run the service tests with `--features test-model`. Without the feature,
  `worker_sigkill_pg` fails on purpose (spec §6.11).
- Postgres tests start a container through `support::start_raw_postgres()` /
  `support::inbox::migrated_postgres()`, which use `paigasus_test_docker::start_or_skip`. They need
  Docker. Each new Docker-gated test binary goes into both lists of `svc/tests/docker_preflight.rs`,
  and the binary count in that file changes with it. After the SMA-788 merge the count is 17. This
  plan adds `run_session_pg`, `run_commit_pg`, `agents_pg`, `worker_pg`, `worker_pool_pg`,
  `worker_crash_pg` and `worker_sigkill_pg`, and Task 8 renames `complete_pg` to `run_commit_pg`.
  So the count is 23 at the end.
- SMA-788 (PR 525, merged into this branch as `41a2b241`) changed `config.rs`, `main.rs`,
  `adapters/persistence/{mod.rs,inbox_repository.rs,migration/mod.rs}`, `adapters/helikon/mod.rs`
  (`timer_tools`), `application/mod.rs` (`scheduler_loop`), `tests/support/mod.rs`,
  `tests/schema_pg.rs`, `tests/boot_pg.rs`, `agent-host.toml.example` and
  `paigasus-observability/src/names.rs`. Read the current file before you edit it. Do not use line
  numbers from an older copy.
- Timers are out of scope (spec N4). Do not add `timer_tools` to the agent, and do not add a timer
  step to the run commit (see the note after the Task 6 step order).
- A mutation proof must compile. Run it with `--no-fail-fast`. Restore the mutation by deleting
  the marked change, not with `git checkout --` (that also reverts the fix under test).
- New dependency: only `paigasus-helikon-sessions-testkit = "=0.1.2"` in
  `svc/Cargo.toml` `[dev-dependencies]`. Its license is `Apache-2.0 OR MIT`, already allowed by
  `rs/deny.toml`. It needs no `deny.toml` exception. The workspace has no cargo-machete metadata, and
  the crate is used by a test, so it needs no allowlist entry. The `test-model` cargo feature adds no
  dependency. The holder name uses the `HOSTNAME` environment variable (fallback `unknown-host`) so
  that no hostname crate is needed.
- All correctness times use the database clock (`clock_timestamp()` in a statement that can wait on
  a lock). Lock order in a run commit: the lease row first, then the memory scope row.
- Call `apply_in` when `MemoryCommitSet::is_empty()` is false, never on `changes.is_empty()`.
- Metric names are provisional (spec §6.14, SMA-798 owns the final names). Follow the SMA-788
  pattern: each name is a `pub const AGENT_HOST_…` in
  `rs/crates/libs/paigasus-observability/src/names.rs`, listed in `ALL`, with a doc comment that
  names its labels, and `main.rs` describes it in `describe_agent_host_metrics`.

## Review Focus

These five conditions follow from the spec, but no spec test covers them. Each line has its test in
the owning task.

1. A shutdown signal that arrives while the worker is inside a commit (not in the model loop). The
   commit must finish, the events must be `done`, and no attempt is given back. Test in Task 12
   (`a_shutdown_during_the_commit_keeps_the_commit`).
2. A shutdown signal that arrives while a worker sleeps in its poll interval. The worker must stop at
   once, not after the full interval. Test in Task 13 (`shutdown_ends_a_poll_sleep_at_once`).
3. A lease that is lost after the claim and before the model call (for example, during
   `run_start_context`). The worker must make no model call and write nothing. Test in Task 12
   (`a_lease_lost_before_the_model_call_writes_nothing`).
4. An event whose envelope `time` is not a string (a number or an object). The renderer must use
   `received_at` and not fail. Test in Task 2 (`a_time_that_is_not_a_string_uses_received_at`).
5. A committed `session_event` row whose payload does not deserialize (for example, written by a
   newer Helikon). `PgRunSession` must return a `SessionError`, and the run must be transient, not a
   panic. Test in Task 5 (`an_unreadable_row_is_a_session_error`) and Task 11
   (`a_session_read_error_is_transient`).

---

### Task 1: Core run types, the four-method `RunCommitter`, `EventScreen` and `FaultPoint`

**Files:**
- Modify: `core/src/run.rs` (replace `RunHandle` and `RunOutcome`; extend `RunStatus`, `Committed`,
  `CommitError`)
- Create: `core/src/screen.rs`
- Create: `core/src/fault.rs`
- Modify: `core/src/ports.rs:216-227` (`RunCommitter`), add `EventScreen` and `FaultPoint` re-exports
- Modify: `core/src/lib.rs` (modules, `every_port_is_object_safe_send_and_sync`)

**Interfaces:**
- Consumes: `MemoryCommitSet`, `MemoryApplyError`, `LeaseHandle`, `EventId`, `RunId`, `LeaseToken`,
  `InboxEvent` (all existing).
- Produces (all `pub`, used by every later task):

```rust
// core/src/run.rs
pub enum RunStatus { Running, Completed, Deferred, Stopped, Failed, Screened, Discarded }
impl RunStatus { pub fn as_str(&self) -> &'static str } // "running", "completed", "deferred", "stopped", "failed", "screened", "discarded"

pub struct RunUsage { pub input_tokens: u64, pub output_tokens: u64, pub cached_input_tokens: u64, pub reasoning_tokens: u64 } // Default, Copy
pub struct SessionRow { pub kind: String, pub payload: serde_json::Value }
pub struct EventDelay { pub event: EventId, pub delay: std::time::Duration }
pub enum RunClass {
    Completed { output: Option<String>, usage: RunUsage },
    Stopped { output: Option<String>, reason: String, usage: RunUsage },
    Deterministic { error: String, retry: Vec<EventDelay> },
}
pub struct RunRecord {
    pub run: RunId, pub token: LeaseToken, pub passed: Vec<EventId>,
    pub class: RunClass, pub session: Vec<SessionRow>, pub nonce: String,
    pub close_thread: bool, pub memory: MemoryCommitSet,
}
pub struct FlaggedEvent { pub event: EventId, pub detector: String, pub reason: String }
impl FlaggedEvent { pub fn last_error(&self) -> String } // "screened: <detector>: <reason>"
pub struct ScreenOut { pub run: RunId, pub token: LeaseToken, pub flagged: Vec<FlaggedEvent>, pub all: bool }
pub enum ScreenResult { AllScreened, Continue }
pub struct BackOff { pub run: RunId, pub token: LeaseToken, pub error: String, pub retry: Vec<EventDelay> }
pub struct Committed { pub run: RunId, pub done: Vec<EventId> }
pub enum CommitError { LeaseLost, OutcomeUnknown, MemoryRejected(MemoryApplyError), Store(String) }
impl CommitError { pub fn is_unencodable(&self) -> bool }
pub const UNENCODABLE_PREFIX: &str = "unencodable text: ";
pub fn replace_nul(text: &str) -> String // U+0000 -> U+FFFD

// core/src/screen.rs
pub enum ScreenVerdict { Pass, Flag { detector: String, reason: String } }
#[derive(thiserror::Error)] #[error("screen error: {0}")] pub struct ScreenError(pub String);
#[async_trait] pub trait EventScreen: Send + Sync {
    async fn screen(&self, event: &InboxEvent) -> Result<ScreenVerdict, ScreenError>;
}
pub struct AllowAll; // impl EventScreen: always Pass

// core/src/fault.rs
pub enum FaultStage { AfterRun, BeforeCommit, AfterCommit } // Debug, Clone, Copy, PartialEq, Eq
#[async_trait] pub trait FaultPoint: Send + Sync { async fn reach(&self, stage: FaultStage) { let _ = stage; } }
pub struct NoFaults; // impl FaultPoint {}

// core/src/ports.rs
#[async_trait] pub trait RunCommitter: Send + Sync {
    async fn commit(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError>;
    async fn screen_out(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError>;
    async fn back_off(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError>;
    async fn release_after_shutdown(&self, h: LeaseHandle) -> Result<(), CommitError>;
}
```

`CommitError` `Display` texts: `"the lease is lost"`, `"the commit outcome is unknown"`,
`"memory refused: {0}"` (uses the `MemoryApplyError` `Display`, which holds no path),
`"store error: {0}"`. `ClaimRepository::complete` and `CompleteError` stay in this task. Task 8
removes them.

- [ ] **Step 1: Write the failing tests** in `core/src/run.rs` (`#[cfg(test)] mod tests`),
  `core/src/screen.rs` and `core/src/lib.rs`:

```rust
#[test]
fn run_status_texts_are_the_column_values() {
    use RunStatus::*;
    let all = [Running, Completed, Deferred, Stopped, Failed, Screened, Discarded];
    let texts: Vec<_> = all.iter().map(RunStatus::as_str).collect();
    assert_eq!(texts, ["running", "completed", "deferred", "stopped", "failed", "screened", "discarded"]);
}

#[test]
fn a_memory_rejection_text_holds_no_path() {
    let path = MemoryPath::parse("/people/secret-name.md").unwrap();
    let e = CommitError::MemoryRejected(MemoryApplyError::Conflict { path });
    assert_eq!(e.to_string(), "memory refused: memory conflict");
}

#[test]
fn only_the_unencodable_prefix_is_unencodable() {
    assert!(CommitError::Store(format!("{UNENCODABLE_PREFIX}22P05")).is_unencodable());
    assert!(!CommitError::Store("connection reset".into()).is_unencodable());
    assert!(!CommitError::LeaseLost.is_unencodable());
}

#[test]
fn replace_nul_changes_only_nul() {
    assert_eq!(replace_nul("a\u{0}b"), "a\u{FFFD}b");
    assert_eq!(replace_nul("plain <x>"), "plain <x>");
}

#[test]
fn a_flagged_event_names_its_detector_in_last_error() {
    let f = FlaggedEvent { event: EventId::from_uuid(Uuid::from_u128(1)), detector: "pii".into(), reason: "an email".into() };
    assert_eq!(f.last_error(), "screened: pii: an email");
}

// core/src/screen.rs
#[tokio::test]
async fn allow_all_passes_every_event() { /* build one InboxEvent; assert_eq!(AllowAll.screen(&e).await, Ok(ScreenVerdict::Pass)) */ }
```

  In `lib.rs`, add `assert_port::<dyn EventScreen>();` and `assert_port::<dyn FaultPoint>();` to
  `every_port_is_object_safe_send_and_sync`.

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host-core`
Expected: FAIL to compile (`RunStatus::Screened`, `RunRecord`, `EventScreen` not found).

- [ ] **Step 3: Implement the types and ports above.** Delete `RunHandle` and `RunOutcome` (no
  other user exists). Keep `Run` and `LeaseToken`. `ScreenVerdict` and `ScreenError` derive
  `Debug, Clone, PartialEq, Eq`. Update the `RunCommitter` doc comment: it now names the four methods
  and says that only `LeaseLost` counts as a lost lease.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. The service crate still compiles, because no adapter implements `RunCommitter` yet.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-agent-host-core/src
git commit -m "feat(rs): add the run record types and the four-method RunCommitter port (SMA-790)"
```

---

### Task 2: The event renderer `render::events`

**Files:**
- Create: `core/src/render/events.rs`
- Modify: `core/src/render/mod.rs` (`pub mod events;`)

**Interfaces:**
- Consumes: `untrusted::{open_tag, close_tag, escape}`, `InboxEvent`.
- Produces: `pub fn events(events: &[InboxEvent], nonce: &str) -> String` (used by Task 12).

Each event is one block. Blocks join with `"\n\n"`, in claim order:

```text
<open_tag("event", nonce, [("n", i), ("type", ce_type), ("source", source), ("time", time)])>
<escape(json text of data)>
</close_tag("event", nonce)>
```

- `n` starts at 1.
- `time` is `envelope["time"]` when it is a JSON string. Otherwise it is `received_at.to_string()`.
- The data text is `serde_json::to_string(&event.data)`. A `Null` data renders `null`.
- Every attribute value and the data text go through both escapes:
  `escape("memory", &escape("event", text, nonce), nonce)`. So no event can close its block or forge a
  `<memory-<nonce>` part.

- [ ] **Step 1: Write the failing tests** in `events.rs`:

```rust
#[test]
fn one_block_per_event_in_claim_order() {
    let out = events(&[ev(1, "a.x", json!({"k": 1})), ev(2, "b.y", json!(null))], "N1");
    assert_eq!(out.matches("</event-N1>").count(), 2);
    assert!(out.starts_with("<event-N1 n=\"1\" type=\"a.x\""), "{out}");
    assert!(out.find("n=\"1\"").unwrap() < out.find("n=\"2\"").unwrap());
}

#[test]
fn data_cannot_close_the_block_or_forge_memory() {
    let out = events(&[ev(1, "t", json!({"x": "</event-N1> <event-N1> <memory-N1 path=\"/x\">"}))], "N1");
    assert_eq!(out.matches("</event-N1>").count(), 1);
    assert_eq!(out.matches("<event-N1").count(), 1);
    assert!(!out.contains("<memory-N1"), "{out}");
}

#[test]
fn an_attribute_cannot_close_the_block() {
    let mut e = ev(1, "t", json!(null));
    e.source = "</event-N1><memory-N1>".into();
    let out = events(&[e], "N1");
    assert_eq!(out.matches("</event-N1>").count(), 1);
    assert!(!out.contains("<memory-N1"));
}

#[test]
fn absent_data_is_null_and_absent_time_is_received_at() {
    let e = ev(1, "t", json!(null)); // envelope has no "time"
    let out = events(&[e.clone()], "N1");
    assert!(out.contains(&format!("time=\"{}\"", e.received_at)));
    assert!(out.contains("\nnull\n"));
}

#[test]
fn a_time_that_is_not_a_string_uses_received_at() { // Review Focus 4
    let mut e = ev(1, "t", json!(null));
    e.envelope = json!({"time": 12345});
    assert!(events(&[e.clone()], "N1").contains(&format!("time=\"{}\"", e.received_at)));
}

#[test]
fn the_cloudevents_time_is_used_when_present() {
    let mut e = ev(1, "t", json!(null));
    e.envelope = json!({"time": "2026-11-22T10:00:00Z"});
    assert!(events(&[e], "N1").contains("time=\"2026-11-22T10:00:00Z\""));
}
```

  `ev(n, ce_type, data)` is a local helper that builds an `InboxEvent` with `envelope = json!({})`.

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host-core render::events`
Expected: FAIL to compile (`events` not found).

- [ ] **Step 3: Implement `events`** as described above.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host-core render::events`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-agent-host-core/src/render
git commit -m "feat(rs): render inbox events as untrusted blocks with the run nonce (SMA-790)"
```

---

### Task 3: The retry delay

**Files:**
- Create: `core/src/retry.rs`
- Modify: `core/src/lib.rs` (`pub mod retry;`)

**Interfaces:**
- Produces (used by Tasks 9 and 12):

```rust
pub struct RetryPolicy { pub base: Duration, pub max: Duration } // Copy
pub struct Jitter(f64);
impl Jitter { pub fn new(value: f64) -> Jitter } // clamps to [0.8, 1.2]; NaN gives 1.0
impl RetryPolicy { pub fn delay(&self, attempts: u32, jitter: Jitter, retry_after: Option<Duration>) -> Duration }
```

The algorithm (spec §6.9). The jitter applies before the cap, and every step saturates:

```rust
let n = attempts.max(1);
let factor = 2u32.checked_pow(n - 1).unwrap_or(u32::MAX);
let raw = self.base.saturating_mul(factor);
let jittered = Duration::try_from_secs_f64(raw.as_secs_f64() * jitter.0).unwrap_or(self.max);
let delay = jittered.min(self.max);
match retry_after { Some(after) => delay.max(after).min(self.max), None => delay }
```

- [ ] **Step 1: Write the failing tests** (`base` 30 s, `max` 1800 s, the spec defaults):

```rust
fn p() -> RetryPolicy { RetryPolicy { base: Duration::from_secs(30), max: Duration::from_secs(1800) } }
fn one() -> Jitter { Jitter::new(1.0) }

#[test] fn the_first_attempt_waits_the_base() { assert_eq!(p().delay(1, one(), None), Duration::from_secs(30)); }
#[test] fn the_delay_doubles() { assert_eq!(p().delay(3, one(), None), Duration::from_secs(120)); }
#[test] fn the_cap_holds() { assert_eq!(p().delay(7, one(), None), Duration::from_secs(1800)); }
#[test] fn the_jitter_comes_before_the_cap() { assert_eq!(p().delay(6, Jitter::new(1.2), None), Duration::from_secs(1152)); assert_eq!(p().delay(7, Jitter::new(1.2), None), Duration::from_secs(1800)); }
#[test] fn retry_after_raises_the_delay_and_stays_capped() {
    assert_eq!(p().delay(1, one(), Some(Duration::from_secs(600))), Duration::from_secs(600));
    assert_eq!(p().delay(1, one(), Some(Duration::from_secs(99_999))), Duration::from_secs(1800));
}
#[test] fn attempt_100_does_not_overflow() { assert_eq!(p().delay(100, Jitter::new(1.2), None), Duration::from_secs(1800)); }
#[test] fn the_jitter_is_clamped() { assert_eq!(p().delay(1, Jitter::new(5.0), None), Duration::from_secs(36)); assert_eq!(p().delay(1, Jitter::new(f64::NAN), None), Duration::from_secs(30)); }
#[test] fn four_retries_cover_seven_and_a_half_minutes() {
    let total: Duration = (1..=4).map(|n| p().delay(n, one(), None)).sum();
    assert_eq!(total, Duration::from_secs(450));
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host-core retry`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `retry.rs`** with the algorithm above.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host-core retry`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-agent-host-core/src
git commit -m "feat(rs): add the capped exponential retry delay (SMA-790)"
```

---

### Task 4: Migration `m0005` — `session_event` and the new `run` columns

The spec (§6.6, §6.10) names this migration `m0004`. That number is out of date: SMA-788 (PR 525)
took `m0004_create_timer.rs`. This migration is `m0005`. Use `m0005` everywhere.

**Files:**
- Create: `svc/src/adapters/persistence/migration/m0005_create_session_event.rs`
- Modify: `svc/src/adapters/persistence/migration/mod.rs` (register m0005 after m0004)
- Modify: `svc/tests/schema_pg.rs`: `down_drops_every_table_of_m0003_in_dependency_order` goes from
  `Migrator::down(&db, Some(2))` to `Some(3)` ("down of m0005, m0004 and m0003");
  `m0004_down_drops_only_the_timer_table` goes from `Some(1)` to `Some(2)` ("down of m0005 and
  m0004"); add two tests
- Modify: `svc/tests/memory_pg.rs` `m0002_down_drops_its_objects`: `Migrator::down(&db, Some(3))`
  becomes `Some(4)`, and the doc comment and the message name m0005 too

**Interfaces:**
- Produces: the table `session_event` (spec §6.6, verbatim DDL) and the `run` columns `final_output
  text`, `error text`, `error_class text`, `input_tokens bigint`, `output_tokens bigint`,
  `cached_input_tokens bigint`, `reasoning_tokens bigint` (spec §6.10). No CHECK on `run.status`.

`UP`: `CREATE TABLE session_event (…)` exactly as spec §6.6, then one
`ALTER TABLE run ADD COLUMN …` with the seven columns. `DOWN`: `ALTER TABLE run DROP COLUMN …` (the
seven), then `DROP TABLE session_event`. The primary key `(thread_id, sequence)` is the only index
(spec §10 leaves more indexes to the builder; add none). Follow the `UP`/`DOWN` const-array form of
`m0003` and `m0004`.

- [ ] **Step 1: Write the failing tests** in `schema_pg.rs`:

```rust
#[tokio::test]
async fn m0005_adds_session_event_and_the_run_columns() {
    // after migrated_postgres():
    assert!(scalar_bool(&db, "SELECT to_regclass('session_event') IS NOT NULL AS v").await);
    for col in ["final_output", "error", "error_class", "input_tokens", "output_tokens", "cached_input_tokens", "reasoning_tokens"] {
        assert!(scalar_bool(&db, &format!("SELECT count(*) = 1 AS v FROM information_schema.columns WHERE table_name = 'run' AND column_name = '{col}'")).await, "{col}");
    }
    // the composite keys: a session row of another inbox's run is refused
    two_inboxes(&db).await; /* thread 500 and run 600 in inbox 1 */
    let err = db.execute_unprepared("INSERT INTO session_event (inbox_id, thread_id, sequence, run_id, kind, payload) VALUES (<inbox 2>, 500, 0, 600, 'user_message', '{}')").await;
    assert!(err.is_err());
    // the primary key refuses a second row with the same (thread, sequence)
}

#[tokio::test]
async fn down_of_m0005_drops_session_event_and_the_run_columns() {
    Migrator::down(&db, Some(1)).await.expect("down of m0005");
    assert!(scalar_bool(&db, "SELECT to_regclass('session_event') IS NULL AS v").await);
    assert!(scalar_bool(&db, "SELECT count(*) = 0 AS v FROM information_schema.columns WHERE table_name = 'run' AND column_name = 'final_output'").await);
    assert!(scalar_bool(&db, "SELECT to_regclass('run') IS NOT NULL AS v").await, "m0003 must stay");
    assert!(scalar_bool(&db, "SELECT to_regclass('timer') IS NOT NULL AS v").await, "m0004 must stay");
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host --test schema_pg --test memory_pg --no-fail-fast`
Expected: FAIL (`session_event` missing).

- [ ] **Step 3: Implement m0005** and register it after m0004 in `Migrator::migrations`.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host --test schema_pg --test memory_pg --test boot_pg`
Expected: PASS. `boot_pg` counts `Migrator::migrations().len()`, so it needs no change.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the session_event table and the run result columns (SMA-790)"
```

---

### Task 5: The session store and `PgRunSession`

**Files:**
- Modify: `svc/Cargo.toml` (`[dev-dependencies]` add `paigasus-helikon-sessions-testkit = "=0.1.2"`
  with a comment that names `tests/run_session_pg.rs`)
- Create: `svc/src/adapters/persistence/session_store.rs`
- Modify: `svc/src/adapters/persistence/mod.rs` (`pub mod session_store;`)
- Create: `svc/src/adapters/helikon/session.rs`
- Modify: `svc/src/adapters/helikon/mod.rs` (`pub mod session;`)
- Test: `svc/tests/run_session_pg.rs` (new, Docker-gated; add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: `SessionRow`, `replace_nul` (Task 1); the m0005 table (Task 4).
- Produces (used by Tasks 6, 11, 12):

```rust
// session_store.rs
pub fn prepare_rows(rows: &[SessionRow], nonce: &str) -> Vec<SessionRow>;
pub async fn write_session(txn: &DatabaseTransaction, inbox: Uuid, thread: Uuid, run: Uuid, nonce: &str, rows: &[SessionRow]) -> Result<usize, DbErr>;
pub async fn read_session(db: &impl ConnectionTrait, thread: Uuid) -> Result<Vec<serde_json::Value>, DbErr>; // ORDER BY sequence

// helikon/session.rs
pub struct PgRunSession { /* db, thread, fresh, buffer: std::sync::Mutex<Vec<SessionEvent>> */ }
impl PgRunSession {
    pub fn new(db: DatabaseConnection, thread: ThreadId, fresh_thread: bool) -> Self;
    pub fn take_buffer(&self) -> Vec<SessionEvent>;
}
impl Session for PgRunSession { /* append, events, snapshot */ }
pub fn to_session_row(event: &SessionEvent) -> Result<SessionRow, serde_json::Error>; // kind = event.kind(), payload = serde_json::to_value(event)
```

Rules:

- `prepare_rows` (spec §6.4, §6.6): for a payload with `"type": "user_message"`, remove every
  element of `content` that has `"type": "text"` and a `text` that starts with `<memory-{nonce}`.
  Then replace U+0000 with U+FFFD in every JSON string (keys and values), at any depth. A payload of
  another type keeps its content parts.
- `write_session` calls `prepare_rows`, reads
  `SELECT COALESCE(max(sequence), -1) + 1 AS next FROM session_event WHERE thread_id = $1`, and then
  inserts one row per prepared row with `sequence = next + i`, in order. It returns the number of
  rows. It does not lock: the primary key makes a second writer fail. It is the only function that
  inserts into `session_event`.
- `PgRunSession::events(since)` returns the committed events (from `read_session`, deserialized
  with `serde_json::from_value::<SessionEvent>`) followed by the buffer. Positions start at 0 over
  the combined list. `since` is exclusive. A fresh thread skips the read. A read or deserialize
  error is `SessionError::backend(...)`.
- `snapshot()` is `project(&self.events(None).await?)`.
- `append` extends the buffer and always returns `Ok(())` (spec §5).

- [ ] **Step 1: Write the failing unit tests** in `session_store.rs`:

```rust
#[test]
fn the_memory_part_of_this_run_is_removed() {
    let row = user_row(&["<memory-N1 path=\"/memories/a.md\">\nsecret\n</memory-N1>", "<event-N1 n=\"1\">x</event-N1>"]);
    let out = prepare_rows(&[row], "N1");
    let texts = texts_of(&out[0]);
    assert_eq!(texts, ["<event-N1 n=\"1\">x</event-N1>"]);
}

#[test]
fn a_memory_part_of_another_nonce_stays() {
    let out = prepare_rows(&[user_row(&["<memory-N2>x</memory-N2>"])], "N1");
    assert_eq!(texts_of(&out[0]).len(), 1);
}

#[test]
fn an_assistant_text_that_starts_with_the_prefix_stays() {
    let out = prepare_rows(&[assistant_row("<memory-N1 x>")], "N1");
    assert_eq!(out[0].payload, assistant_row("<memory-N1 x>").payload);
}

#[test]
fn nul_is_replaced_at_every_depth() {
    let row = SessionRow { kind: "tool_called".into(), payload: json!({"type": "tool_called", "args": {"a\u{0}": ["x\u{0}y"]}}) };
    let out = prepare_rows(&[row], "N1");
    assert_eq!(out[0].payload, json!({"type": "tool_called", "args": {"a\u{FFFD}": ["x\u{FFFD}y"]}}));
}
```

- [ ] **Step 2: Write the failing Postgres tests** in `tests/run_session_pg.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_testkit_passes_against_the_buffered_session() {
    // factory: a fresh thread uuid per call; PgRunSession::new(db, thread, true)
    paigasus_helikon_sessions_testkit::run_all(|| async { Arc::new(PgRunSession::new(db.clone(), new_thread(), true)) as Arc<dyn Session> }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_testkit_passes_against_the_write_through_session() {
    // factory: insert a fresh inbox, thread and running run; return WriteThrough { db, inbox, thread, run }.
    // WriteThrough::append: begin; SELECT pg_advisory_xact_lock(hashtextextended($thread::text, 0));
    //   write_session(&txn, inbox, thread, run, "N", &rows); commit.
    // WriteThrough::events / snapshot: PgRunSession::new(db, thread, false).events(..) / .snapshot().
    run_all(|| async { Arc::new(write_through(&db).await) as Arc<dyn Session> }).await;
}

#[tokio::test]
async fn write_session_numbers_from_the_last_sequence() {
    // two write_session calls of 2 and 3 rows on one thread
    assert_eq!(sequences(&db, thread).await, vec![0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn a_second_writer_of_one_number_fails() {
    // two open transactions both read next = 0; the second insert fails with a unique violation
    // (the first commits first); the second rolls back; exactly 1 row exists.
}

#[tokio::test]
async fn an_unreadable_row_is_a_session_error() { // Review Focus 5
    // insert a row with payload '{"type": "from_the_future"}'
    let err = PgRunSession::new(db, thread, false).events(None).await.unwrap_err();
    assert!(matches!(err, SessionError::Backend(_)), "{err:?}");
}
```

- [ ] **Step 3: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host session_store --test run_session_pg --no-fail-fast`
Expected: FAIL to compile.

- [ ] **Step 4: Implement `session_store.rs` and `session.rs`** per the rules above.

- [ ] **Step 5: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host session_store --test run_session_pg`
Expected: PASS. Both testkit setups run all five sub-tests.

- [ ] **Step 6: Mutation proof.** Change `prepare_rows` so that it does not remove the memory part
  (delete the filter call). Run `cargo nextest run -p paigasus-agent-host session_store --no-fail-fast`.
  Expected: `the_memory_part_of_this_run_is_removed` FAILS. Delete the mutation.

- [ ] **Step 7: Commit**

```bash
git add rs/Cargo.lock rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the buffered PgRunSession and the one session write function (SMA-790)"
```

---

### Task 6: `PgRunCommitter` — the full commit and the failure commit

**Files:**
- Create: `svc/src/adapters/persistence/run_commit/mod.rs` (`PgRunCommitter`, shared SQL and error
  mapping)
- Create: `svc/src/adapters/persistence/run_commit/commit.rs`
- Modify: `svc/src/adapters/persistence/mod.rs` (`pub mod run_commit;`)
- Modify: `svc/src/adapters/persistence/lease_store/mod.rs` (`commit_or_poison` and `lost` become
  `pub(crate)`)
- Create: `svc/src/application/worker/mod.rs` (`pub mod worker;` in `application/mod.rs`; it holds
  only the module lines for now)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (the eight new names, `ALL`, and a
  test `the_agent_host_worker_families_are_registered` in the form of
  `the_agent_host_timer_families_are_registered`)
- Create: `svc/tests/support/commit.rs` (`pub mod commit;` in `support/mod.rs`)
- Test: `svc/tests/run_commit_pg.rs` (new, Docker-gated; add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: Task 1 types, `write_session` (Task 5), `PgMemoryStore::apply_in`, `MemoryCommit`.
- Produces:

```rust
pub struct PgRunCommitter { /* db, memory: MemoryCommit, caps: MemoryCaps, faults: Arc<dyn FaultPoint> */ }
impl PgRunCommitter {
    pub fn new(db: DatabaseConnection, memory: MemoryCommit, caps: MemoryCaps, faults: Arc<dyn FaultPoint>) -> Self;
    pub async fn commit_record(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError>; // Task 7 wraps it as RunCommitter::commit
}
// paigasus-observability/src/names.rs (doc comment of each names its labels)
pub const AGENT_HOST_RUNS_TOTAL: &str = "agent_host_runs_total";                    // label: class
pub const AGENT_HOST_RUN_DURATION_SECONDS: &str = "agent_host_run_duration_seconds";
pub const AGENT_HOST_EVENTS_SCREENED_TOTAL: &str = "agent_host_events_screened_total";
pub const AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL: &str = "agent_host_commit_memory_conflicts_total";
pub const AGENT_HOST_COMMIT_OUTCOME_UNKNOWN_TOTAL: &str = "agent_host_commit_outcome_unknown_total";
pub const AGENT_HOST_WORKER_RESTARTS_TOTAL: &str = "agent_host_worker_restarts_total";
pub const AGENT_HOST_INBOX_LAG_SECONDS: &str = "agent_host_inbox_lag_seconds";
pub const AGENT_HOST_EVENT_ATTEMPTS: &str = "agent_host_event_attempts";       // histogram
// support/commit.rs
pub fn scope_of(org: Uuid, agent: Uuid, principal: Uuid) -> MemoryScope;
pub fn completed_record(claim: &Claim, scope: MemoryScope) -> RunRecord; // Completed, no output, empty session, empty MemoryCommitSet
pub fn committer(db: &DatabaseConnection) -> Arc<PgRunCommitter>; // NoFaults, MemoryCaps::default()
```

`commit_record` rules:

1. A poisoned handle returns `LeaseLost` with no SQL. A handle whose `open_claim()` is not
   `Some(r.run)`, or whose `token()` is not `r.token`, counts `lost("commit")` and returns
   `LeaseLost` with no SQL.
2. The adapter calls `replace_nul` on the output, the error and the reason. `write_session` does it
   for the session rows.
3. One attempt is one transaction. A `Store` error before COMMIT rolls back explicitly and retries
   the whole attempt, at most 2 more times (3 attempts in all). `LeaseLost`, `MemoryRejected`, an
   unencodable error and `OutcomeUnknown` are never retried.
4. An error whose SQLSTATE (`DbErr::{Query,Exec}(RuntimeErr::SqlxError(e))`,
   `e.as_database_error().and_then(|d| d.code())`) is `22P05` or `22021` becomes
   `CommitError::Store(format!("{UNENCODABLE_PREFIX}{code}"))`.
5. The adapter does not call `h.record_complete()`. The keeper does that (Task 8).

The step order of one attempt (spec §6.8). Write it in this order:

```text
BEGIN; SET LOCAL lock_timeout = '5s'
1  LOCK_SQL (lease row FOR UPDATE, token = $t AND expires_at > clock_timestamp())  -> no row: rollback, lost("commit"), LeaseLost
2  Completed/Stopped: RUN_DONE_SQL (status, finished_at, final_output, 4 token columns)
   Deterministic:     RUN_FAILED_SQL (status 'failed', error_class 'deterministic', error)
   both: WHERE id = $run AND status = 'running' AND lease_token = $t RETURNING thread_id
   -> no row: rollback, lost("commit"), LeaseLost
   (the thread for every later step is this RETURNING thread_id, never a field of the record)
Completed/Stopped only:
3  write_session(txn, inbox, thread, run, nonce, &r.session)
4  THREAD_SQL (last_run_at) and SUCCESSOR_SQL, as lease_store/complete.rs does today
Deterministic only:
   CLOSE_THREAD_SQL when r.close_thread
both:
5  SCOPE_SQL: the inbox row's (org_id, agent_id, principal_id) must equal the set's scope
   -> differs: rollback, Store("the memory scope is not the scope of the inbox"), retried as Store
   if !r.memory.is_empty(): apply_in(txn, &r.memory, &caps)
   -> Conflict | CapExceeded: rollback, counter AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL (Conflict only), MemoryRejected
   -> Store | Partial: rollback, Store
Completed/Stopped only:
6  DONE_SQL: status 'done' WHERE id = ANY($passed) AND claimed_run_id = $run AND status = 'pending' RETURNING id, attempts
   (observe AGENT_HOST_EVENT_ATTEMPTS per row after the COMMIT)
Deterministic only:
6' RETRY_SQL per EventDelay: not_before = clock_timestamp() + ($d::bigint * interval '1 microsecond'),
   last_error = $error WHERE id = $e AND claimed_run_id = $run AND status = 'pending'
faults.reach(FaultStage::BeforeCommit)
7  commit_or_poison(h, txn.commit()) -> Err: OutcomeUnknown (no retry)
```

`MemoryCommit` has one variant (`InTransaction(PgMemoryStore)`). Match on it, so SMA-829 adds its
arm later.

**The later timer step (not in this issue).** Timers stay out of SMA-790 (spec N4; `[[agents]].tools`
allows only `memory`). Do not add a timer tool to the agent or a timer step to this commit. A later
issue adds one step after step 5 and before step 6:
`TimerStore::apply_in(&txn, &InboxId, &TimerCaps, TimerChanges) -> Result<ApplyReport, ApplyError>`
(`svc/src/adapters/persistence/timer_store.rs:63`). It runs in its own savepoint, takes the
`inbox_lease` row lock (which step 1 already holds, so the lock order stays lease row first), and
returns `ApplyError::LeaseMissing` when no lease row exists. Keep these properties so that nothing
blocks that step: the commit runs on one `DatabaseTransaction` at READ COMMITTED; step 1 is the first
statement that locks a row; `RunRecord` is a plain struct that a later field (`timers`) can extend;
and a refusal of a savepoint step rolls back the whole transaction, as `MemoryRejected` does.

- [ ] **Step 1: Write the failing Postgres tests** in `tests/run_commit_pg.rs`. Setup: a migrated
  database, `insert_inbox(org 1, inbox 2, agent 3, principal 4)`, two pending events, a
  `PgLeaseStore` take and `claim_events`, then `committer(&db)`. Assertions:

```rust
#[tokio::test] async fn a_full_commit_writes_the_run_the_session_the_memory_and_done() {
    // record: Completed { output: Some("ok"), usage: RunUsage { input_tokens: 7, output_tokens: 3, cached_input_tokens: 1, reasoning_tokens: 2 } },
    //         two session rows, memory: put /a.md
    let c = committer.commit_record(&mut h, record).await.unwrap();
    assert_eq!(sorted(c.done), claimed_ids);
    // run: status 'completed', finished_at set, final_output 'ok', tokens 7/3/1/2
    // session_event: 2 rows, sequences 0 and 1, run_id = run, thread_id = the run's thread
    // memory_file '/a.md' exists; thread.last_run_at is set
}
#[tokio::test] async fn the_commit_uses_the_thread_of_the_run_row() { /* change claim.thread in the record source; rows go to the run row's thread */ }
#[tokio::test] async fn a_failure_commit_writes_memory_and_back_off_and_no_session() {
    // Deterministic { error: "timed out", retry: [EventDelay { e1, 60 s }, { e2, 60 s }] }, two session rows, memory: put /progress.md
    // run: 'failed', error_class 'deterministic', error 'timed out', token columns NULL
    // session_event: 0 rows; memory_file '/progress.md' exists
    // events: still 'pending', last_error 'timed out', not_before > now() + 55 s
}
#[tokio::test] async fn close_thread_closes_the_thread_in_the_failure_commit() { /* thread.closed = true */ }
#[tokio::test] async fn a_memory_conflict_rolls_back_everything() {
    // seed /a.md at version V; the record puts /a.md with Expected::Version(stale)
    assert!(matches!(res, Err(CommitError::MemoryRejected(MemoryApplyError::Conflict { .. }))));
    // run still 'running'; no session_event; events 'pending'; /a.md unchanged; counter AGENT_HOST_COMMIT_MEMORY_CONFLICTS_TOTAL == 1
}
#[tokio::test] async fn a_guard_with_no_change_still_reaches_apply_in() {
    // seed /dir/x.md; memory set: no change, one guard on /dir
    assert!(matches!(res, Err(CommitError::MemoryRejected(_))));
}
#[tokio::test] async fn a_guard_over_an_empty_dir_commits() { /* guard on /empty, no file under it: Ok */ }
#[tokio::test] async fn a_scope_of_another_principal_is_refused() {
    // memory set with scope principal 99, put /a.md
    assert!(matches!(res, Err(CommitError::Store(ref s)) if s.contains("scope")));
    // no row changed (support::lease::snapshot before == after), no memory_file row
}
#[tokio::test] async fn a_store_error_retries_the_whole_transaction() {
    // trigger on session_event: IF nextval('fail_seq') <= 2 THEN RAISE EXCEPTION 'injected'
    assert!(res.is_ok()); assert_eq!(scalar_i64("SELECT last_value AS n FROM fail_seq"), 3);
    // exactly 2 session rows (no duplicate from the failed attempts)
}
#[tokio::test] async fn a_store_error_after_two_retries_is_store() {
    // trigger always raises
    assert!(matches!(res, Err(CommitError::Store(_)))); assert_eq!(last_value, 3); // snapshot unchanged
    assert!(!h.is_poisoned());
}
#[tokio::test] async fn a_failed_commit_statement_is_outcome_unknown_and_poisons() {
    // CREATE CONSTRAINT TRIGGER … DEFERRABLE INITIALLY DEFERRED on session_event that raises
    assert_eq!(res, Err(CommitError::OutcomeUnknown)); assert!(h.is_poisoned());
}
#[tokio::test] async fn sqlstate_22p05_is_unencodable_and_not_retried() {
    // trigger: RAISE EXCEPTION 'x' USING ERRCODE = '22P05' (count calls with a sequence)
    assert!(res.as_ref().unwrap_err().is_unencodable()); assert_eq!(last_value, 1);
}
#[tokio::test] async fn nul_in_the_output_is_replaced() { /* output "a\u{0}b" -> final_output 'a\u{FFFD}b' */ }
#[tokio::test] async fn a_stale_commit_is_lease_lost_and_changes_no_row() {
    // expire the lease; commit -> LeaseLost; snapshot unchanged; counter lease_lost{operation="commit"} == 1
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host --test run_commit_pg --no-fail-fast`
Expected: FAIL to compile (`PgRunCommitter` not found).

- [ ] **Step 3: Implement `run_commit/mod.rs`, `run_commit/commit.rs`, the `names.rs` entries and
  `support/commit.rs`** per the rules and the step order above. Every error path rolls back
  explicitly (the `Drop` of a `DatabaseTransaction` can panic, see `migration_lock.rs`).

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host --test run_commit_pg`
Expected: PASS (14 tests).

- [ ] **Step 5: Mutation proofs** (each one compiles; run with `--no-fail-fast`, then delete it):
  - Replace `!r.memory.is_empty()` with `!r.memory.changes.is_empty()`. Expected:
    `a_guard_with_no_change_still_reaches_apply_in` FAILS.
  - Delete the step-1 `expires_at > clock_timestamp()` term. Expected:
    `a_stale_commit_is_lease_lost_and_changes_no_row` FAILS.
  - Delete the retry loop (one attempt only). Expected: `a_store_error_retries_the_whole_transaction`
    FAILS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-agent-host rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): add the fenced run commit and the failure commit (SMA-790)"
```

---

### Task 7: `PgRunCommitter` — screen-out, back-off, release after shutdown, and `dead` with the kept error

**Files:**
- Create: `svc/src/adapters/persistence/run_commit/screen.rs`, `back_off.rs`, `shutdown.rs`
- Modify: `svc/src/adapters/persistence/run_commit/mod.rs` (`impl RunCommitter for PgRunCommitter`)
- Modify: `svc/src/adapters/persistence/lease_store/claim.rs:44-49` (`DEAD_SQL`) and the dead move
  (observe `AGENT_HOST_EVENT_ATTEMPTS`)
- Test: `svc/tests/run_commit_pg.rs`, `svc/tests/claim_pg.rs`

**Interfaces:**
- Consumes: Task 6 (`commit_record`, the shared `LOCK_SQL`, `commit_or_poison`, `lost`).
- Produces: `impl RunCommitter for PgRunCommitter` (used by Task 8 and later). `commit` calls
  `commit_record`.

Rules (spec §6.3, §6.9):

- `screen_out`: one transaction with `lock_timeout = '5s'`; step 1 lock check (no row: `LeaseLost`,
  `lost("screen_out")`); per flagged event `UPDATE inbox_event SET status = 'dead', last_error = $3
  WHERE id = $1 AND claimed_run_id = $2 AND status = 'pending' RETURNING attempts`. When `all`:
  `UPDATE run SET status = 'screened', finished_at = now() WHERE id = $run AND status = 'running' AND
  lease_token = $t` (zero rows: rollback, `LeaseLost`). No thread update. `commit_or_poison`; an
  error is `OutcomeUnknown`. After COMMIT: `AGENT_HOST_INBOX_EVENTS_DEAD_TOTAL` and
  `AGENT_HOST_EVENTS_SCREENED_TOTAL` increase by the number of dead rows, and `AGENT_HOST_EVENT_ATTEMPTS` observes each
  `attempts`. Returns `AllScreened` when `all`, else `Continue`.
- `back_off`: one transaction; step 1 lock check (`lost("back_off")`); `UPDATE run SET status =
  'failed', finished_at = now(), error_class = 'transient', error = $3 WHERE id = $run AND status =
  'running' AND lease_token = $t` (zero rows: `LeaseLost`); per `EventDelay` the `RETRY_SQL` of
  Task 6 with `last_error = $error`; `commit_or_poison`.
- `release_after_shutdown(h)`: a poisoned handle returns `Ok(())` with no SQL. Otherwise one
  transaction: step 1 lock check; when it fails, roll back and return `LeaseLost`
  (`lost("release_after_shutdown")`), changing nothing. Otherwise, when `h.open_claim()` is `Some(run)`:
  `UPDATE run SET status = 'discarded', finished_at = now() WHERE id = $run AND status = 'running'
  AND lease_token = $t` and `UPDATE inbox_event SET attempts = attempts - 1 WHERE claimed_run_id =
  $run AND status = 'pending' AND attempts > 0`. Then the `RELEASE_SQL` of `lease_store/mod.rs`
  (make it `pub(crate)`). Then COMMIT.
- `DEAD_SQL` becomes `UPDATE inbox_event e SET status = 'dead', last_error = COALESCE(e.last_error, $4)
  … RETURNING e.id, e.attempts`. The claim observes `AGENT_HOST_EVENT_ATTEMPTS` per dead row after its COMMIT.

- [ ] **Step 1: Write the failing tests.** In `run_commit_pg.rs`:

```rust
#[tokio::test] async fn screen_out_of_some_events_keeps_the_run_running() {
    // flag e1 of [e1, e2], all = false
    assert_eq!(res, Ok(ScreenResult::Continue));
    // e1: 'dead', last_error 'screened: pii: an email'; e2: 'pending'; run 'running'; run_event keeps both rows
}
#[tokio::test] async fn screen_out_of_every_event_screens_the_run() {
    assert_eq!(res, Ok(ScreenResult::AllScreened));
    // run 'screened', finished_at set; thread.last_run_at IS NULL; thread.successor_id unchanged
    // counters: inbox_events_dead_total == 2, events_screened_total == 2
}
#[tokio::test] async fn a_failed_commit_of_screen_out_is_outcome_unknown() { /* deferred constraint trigger on inbox_event update; OutcomeUnknown; h poisoned */ }
#[tokio::test] async fn back_off_fails_the_run_and_delays_the_events() {
    // run 'failed', error_class 'transient', error 'model unavailable'; events pending, not_before > now() + 25 s, last_error set
}
#[tokio::test] async fn a_failed_commit_of_back_off_is_outcome_unknown() { /* same trigger pattern */ }
#[tokio::test] async fn back_off_then_the_claim_moves_to_dead_with_the_kept_error() {
    // max_attempts 1; back_off with error 'model unavailable' and delay 0; claim again
    // the event is 'dead' and last_error is 'model unavailable', not DEAD_REASON
}
#[tokio::test] async fn release_after_shutdown_refunds_the_attempt() {
    // after the claim attempts = 1; release_after_shutdown -> Ok
    // attempts = 0; run 'discarded'; lease not live (lease_state live == false)
}
#[tokio::test] async fn a_stale_worker_changes_no_attempt_of_the_new_holder() {
    // A claims (attempts 1); expire A; B takes and claims (attempts 2, claimed_run_id = B's run)
    let before = snapshot(&db, INBOX).await;
    assert_eq!(committer.release_after_shutdown(a_handle).await, Err(CommitError::LeaseLost));
    assert_eq!(before, snapshot(&db, INBOX).await);
}
```

  In `claim_pg.rs` keep `exhausted_events_move_to_dead` as it is (no earlier error, so
  `DEAD_REASON`), and add:

```rust
#[tokio::test] async fn a_dead_move_keeps_an_earlier_error() {
    // event with attempts = max_attempts and last_error = 'model unavailable'
    // after the claim: status 'dead' AND last_error = 'model unavailable'
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host --test run_commit_pg --test claim_pg --no-fail-fast`
Expected: FAIL to compile (no `RunCommitter` impl), then the new claim case FAILS on `last_error`.

- [ ] **Step 3: Implement the three methods, the trait impl and the `DEAD_SQL` change.**

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host --test run_commit_pg --test claim_pg`
Expected: PASS.

- [ ] **Step 5: Mutation proofs** (`--no-fail-fast`, then delete each one):
  - Delete the step-1 lock check in `release_after_shutdown`. Expected:
    `a_stale_worker_changes_no_attempt_of_the_new_holder` FAILS (the run of A becomes `discarded`).
  - Remove `COALESCE(e.last_error, $4)` (back to `$4`). Expected:
    `back_off_then_the_claim_moves_to_dead_with_the_kept_error` and `a_dead_move_keeps_an_earlier_error`
    FAIL.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add screen-out, back-off and the shutdown refund to the run committer (SMA-790)"
```

---

### Task 8: The keeper commits through `RunCommitter`; remove `complete`

**Files:**
- Modify: `svc/src/application/lease_keeper.rs` (start, new methods, remove `complete`, unit tests)
- Delete: `svc/src/adapters/persistence/lease_store/complete.rs` (move its `claim` impl and the two
  `commit_or_poison` unit tests into `lease_store/claim.rs`)
- Modify: `svc/src/adapters/persistence/lease_store/mod.rs` (drop `mod complete;`)
- Modify: `core/src/ports.rs` (remove `ClaimRepository::complete`; update the doc comment)
- Modify: `core/src/lease.rs` (remove `CompleteError`; the `ClaimError::Poisoned` doc says "a failed
  claim or commit")
- Modify: `svc/src/main.rs`, the `AGENT_HOST_LEASE_LOST_TOTAL` line of `describe_agent_host_metrics` (the help text names `heartbeat, claim, commit,
  screen_out, back_off, release_after_shutdown or deadline`)
- Delete: `svc/tests/complete_pg.rs`; move its tests into `svc/tests/run_commit_pg.rs`
- Modify: `svc/tests/lease_property_pg.rs`, `svc/tests/lease_crash_pg.rs`, `svc/tests/keeper_pg.rs`,
  `svc/tests/lease_stress_pg.rs`, `svc/tests/docker_preflight.rs` (`complete_pg` → `run_commit_pg`,
  `run_session_pg`)

**Interfaces:**
- Consumes: `RunCommitter` (Task 7), `support::commit::{completed_record, committer, scope_of}`.
- Produces (used by Tasks 12, 13, 15):

```rust
impl LeaseKeeper {
    pub fn start(handle: LeaseHandle, leases: Arc<dyn LeaseRepository>, claims: Arc<dyn ClaimRepository>, committer: Arc<dyn RunCommitter>, timing: KeeperTiming) -> LeaseKeeper;
    pub async fn claim(&self, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError>;        // unchanged
    pub async fn commit(&self, record: RunRecord) -> Result<Committed, CommitError>;
    pub async fn screen_out(&self, s: ScreenOut) -> Result<ScreenResult, CommitError>;
    pub async fn back_off(&self, b: BackOff) -> Result<(), CommitError>;
    pub async fn release(self) -> Result<(), StoreError>;                                     // unchanged
    pub async fn release_after_shutdown(self) -> Result<(), CommitError>;
    pub fn is_lost(&self) -> bool;                                                           // self.cancel.is_cancelled()
    pub fn cancellation(&self) -> CancellationToken;                                         // unchanged
}
```

Rules: `commit`, `screen_out` and `back_off` follow the old `complete` pattern: hold the mutex
through `LoseIfPoisoned`; a lost keeper returns `LeaseLost` with no store call; a `LeaseLost` result
calls `lose()`. After `Ok` of `commit`, `Ok` of `back_off` and `Ok(AllScreened)` of `screen_out`,
the keeper calls `handle.record_complete()`. `Continue` does not. `release_after_shutdown` aborts the
heartbeat, takes the handle and calls the committer.

Test migration (spec §6.8). Every old `store.complete(&mut h, claim)` becomes
`committer.commit(&mut h, completed_record(&claim, scope))`, then `h.record_complete()` on `Ok`
(a raw handle has no keeper). `Ok(done)` becomes `Ok(c)` with `c.done`. `CompleteError::LeaseLost`
becomes `CommitError::LeaseLost`, and `CompleteError::Store(_)` becomes
`CommitError::OutcomeUnknown` where the old test forced a COMMIT failure, or `Store(_)` otherwise.
The `operation` label `"complete"` becomes `"commit"`. Keep every test name, with `complete` changed
to `commit` in the name.

- [ ] **Step 1: Change the keeper unit tests first.** The `Fake` implements `RunCommitter` in place
  of `ClaimRepository::complete`: rename `complete_lost` / `complete_poisons` / `complete_hangs` to
  `commit_lost` / `commit_poisons` / `commit_hangs` (the poison case returns `OutcomeUnknown`). Rename
  the three `…_complete_…` tests to `…_commit_…`. Add:

```rust
#[tokio::test(start_paused = true)]
async fn is_lost_is_false_until_the_lease_is_lost() {
    let fake = Arc::new(Fake { commit_lost: true, ..Fake::new() });
    let keeper = start(&fake);
    assert!(!keeper.is_lost());
    assert_eq!(keeper.commit(a_record()).await, Err(CommitError::LeaseLost));
    assert!(keeper.is_lost());
}

#[tokio::test(start_paused = true)]
async fn only_a_finished_claim_clears_the_open_claim() {
    // Fake records handle.open_claim() at the start of each call.
    // claim (opens run R) -> screen_out(all: false) -> Continue -> a second claim gives ClaimOpen
    // back_off -> Ok -> the next claim reaches the store
}

#[tokio::test(start_paused = true)]
async fn release_after_shutdown_stops_the_heartbeats() { /* like release_stops_the_heartbeats */ }
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host lease_keeper --no-fail-fast`
Expected: FAIL to compile (`LeaseKeeper::start` takes four arguments; `commit` missing).

- [ ] **Step 3: Implement the keeper changes, remove `complete` everywhere, and migrate the
  integration tests** per the rules above. `lease_crash_pg.rs` keeps its two tests: B commits with a
  `Completed` record, and the late commit of A through a raw handle is `LeaseLost` with an unchanged
  snapshot.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host-core && cargo nextest run -p paigasus-agent-host && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. `grep -rn "CompleteError\|fn complete\|\.complete(" rs/crates/libs/paigasus-agent-host-core rs/crates/services/paigasus-agent-host` finds nothing.

- [ ] **Step 5: Commit**

```bash
git add -A rs/crates/libs/paigasus-agent-host-core rs/crates/services/paigasus-agent-host
git commit -m "refactor(rs): replace the claim complete with the RunCommitter commit (SMA-790)"
```

---

### Task 9: Configuration — worker, run, retry and `[[agents]]`

**Files:**
- Modify: `svc/src/config.rs`
- Modify: `svc/agent-host.toml.example` (the new keys and one commented `[[agents]]` example)

**Interfaces:**
- Consumes: `AgentId::parse`, `AgentDefinition`, `RetryPolicy` (Task 3).
- Produces (used by Tasks 10, 13, 14):

```rust
pub struct WorkerConfig { pub enabled: bool, pub concurrency: u32, pub poll_interval_ms: u64, pub max_runs_per_lease: u32 }
pub struct RunSettings { pub max_turns: u32, pub timeout_secs: u64 }               // Config.run
pub struct RetrySettings { pub base_delay_secs: u64, pub max_delay_secs: u64 }    // Config.retry
impl RetrySettings { pub fn to_policy(&self) -> RetryPolicy }
pub struct AgentConfig { pub id: String, pub name: String, pub model: String, pub instructions: String, #[serde(default)] pub tools: Vec<String> }
// Config gains: pub run: RunSettings, pub retry: RetrySettings, #[serde(default)] pub agents: Vec<AgentConfig>
impl Config { pub fn agent_definitions(&self) -> Result<Vec<AgentDefinition>, ConfigError> }
pub const KNOWN_TOOLS: &[&str] = &["memory"];
```

Defaults and ranges (spec §6.12, verbatim): `worker.concurrency` 4 (1 to 64),
`worker.poll_interval_ms` 1000 (100 to 60000), `worker.max_runs_per_lease` 8 (1 to 1000),
`run.max_turns` 16 (1 to 256), `run.timeout_secs` 300 (1 to 3600), `retry.base_delay_secs` 30 (1 to
3600), `retry.max_delay_secs` 1800 (1 to 86400, and at least the base), `[[agents]]` empty.

New `ConfigError` variants. The seven ranges use one variant in the form of the SMA-788
`TimerValueOutOfRange`, and `validate` checks them in one table loop as it does for `timers.*`:
`ValueOutOfRange { key: &'static str, value: u64, min: u64, max: u64 }` with the error text
`"{key} must be between {min} and {max} (got {value})"` and `key` the full dotted name (for example
`"worker.concurrency"`). The other new variants: `RetryMaxBelowBase { base: u64, max: u64 }`,
`AgentIdInvalid(String)`, `AgentDuplicate(String)`, `AgentUnknownTool { agent: String, tool: String }`.
`validate` calls `agent_definitions()` so that a bad entry fails the boot. The model check against
the registry is in Task 14 (the registry is a service adapter).

- [ ] **Step 1: Write the failing tests** in `config.rs`. Add seven rows to
  `each_range_is_enforced_at_both_ends` (one per range key above, with
  `err: |v| ConfigError::ValueOutOfRange { key, value: v, min, max }`; the `retry.max_delay_secs` row
  sets `base_delay_secs = 1`). Add:

```rust
#[test] fn the_worker_run_and_retry_defaults_match_the_spec() { /* 4, 1000, 8, 16, 300, 30, 1800, agents empty, validate Ok */ }
#[test] fn the_retry_max_must_be_at_least_the_base() { /* base 60, max 59 -> RetryMaxBelowBase { base: 60, max: 59 } */ }
#[test] fn an_agent_id_that_does_not_parse_fails() { /* id "triage" -> AgentIdInvalid("triage") */ }
#[test] fn a_duplicate_agent_id_fails() { /* two entries, one id -> AgentDuplicate(id) */ }
#[test] fn an_unknown_tool_fails() { /* tools ["shell"] -> AgentUnknownTool { agent: id, tool: "shell" } */ }
#[test] fn agents_are_read_from_toml() {
    // agent-host.toml with one [[agents]] table (the spec §6.2 example with a real PRN)
    let defs = cfg.agent_definitions().unwrap();
    assert_eq!(defs[0].name, "triage"); assert_eq!(defs[0].model_id, "scripted"); assert_eq!(defs[0].tool_names, ["memory"]);
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host config::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the config changes** and update `agent-host.toml.example`.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host config::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the worker, run, retry and agents configuration (SMA-790)"
```

---

### Task 10: `ConfigAgentResolver`, `ModelRegistry` and `SharedModel`

**Files:**
- Create: `svc/src/adapters/agents.rs` (`ConfigAgentResolver`; `pub mod agents;` in `adapters/mod.rs`)
- Create: `svc/src/adapters/helikon/model_registry.rs` (`pub mod model_registry;`)
- Test: `svc/tests/agents_pg.rs` (new, Docker-gated; add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: `AgentResolver` port, `AgentDefinition`, `ResolvedAgent`, the `inbox` table.
- Produces (used by Tasks 11–14):

```rust
pub struct ConfigAgentResolver { /* db, agents: HashMap<AgentId, AgentDefinition> */ }
impl ConfigAgentResolver { pub fn new(db: DatabaseConnection, agents: Vec<AgentDefinition>) -> Self; pub fn served(&self) -> Vec<AgentId>; }
impl AgentResolver for ConfigAgentResolver {} // resolve(&InboxId)

#[derive(Clone)] pub struct SharedModel(Arc<dyn Model>);
impl SharedModel { pub fn new(model: impl Model + 'static) -> Self }
impl Model for SharedModel {} // forwards invoke, capabilities, provider, model
#[derive(Clone, Default)] pub struct ModelRegistry { /* HashMap<String, SharedModel> */ }
impl ModelRegistry {
    pub fn register(&mut self, id: impl Into<String>, model: SharedModel);
    pub fn model(&self, id: &str) -> Option<SharedModel>;
    pub fn check(&self, agents: &[AgentDefinition]) -> Result<(), UnknownModel>; // UnknownModel { agent: String, model: String }, thiserror
}
```

`resolve`: `SELECT agent_id, principal_id FROM inbox WHERE id = $1 AND org_id = $2` (the inbox's
uuid and org). No row: `RepositoryError::NotFound`. Then look up `AgentId::new(inbox.org(),
agent_id)`, the full id with the org. No entry: `NotFound` ("agent not configured"). Otherwise
`ResolvedAgent { definition, principal: PrincipalId::from_uuid(principal_id) }`. A DB error is
`RepositoryError::Store`.

- [ ] **Step 1: Write the failing tests.** In `model_registry.rs`:

```rust
#[tokio::test] async fn a_shared_model_forwards_invoke() { /* register a model whose invoke counts; call through SharedModel; count == 1 */ }
#[test] fn the_production_registry_is_empty() { assert!(ModelRegistry::default().model("scripted").is_none()); }
#[test] fn check_names_the_agent_and_the_model() { /* defs with model "gpt" -> Err(UnknownModel { agent: <id>, model: "gpt" }) */ }
```

  In `tests/agents_pg.rs`:

```rust
#[tokio::test] async fn an_inbox_resolves_to_its_configured_agent() { /* definition and principal match */ }
#[tokio::test] async fn an_agent_of_another_org_with_the_same_uuid_does_not_match() {
    // inbox in org 1 with agent uuid A; config has AgentId::new(org 9, A)
    assert_eq!(resolver.resolve(&inbox).await, Err(RepositoryError::NotFound));
}
#[tokio::test] async fn an_unknown_inbox_is_not_found() {}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host model_registry --test agents_pg --no-fail-fast`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the resolver, the registry and `SharedModel`.**

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host model_registry --test agents_pg`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): resolve an inbox to a configured agent and register models (SMA-790)"
```

---

### Task 11: The `AgentRunner` port and the Helikon adapter with the failure classes

**Files:**
- Create: `svc/src/application/worker/ports.rs` (the port; its `RunSpec` holds a tokio `Mutex`, so it
  cannot live in the core)
- Create: `svc/src/adapters/helikon/runner.rs` (`HelikonAgentRunner`)
- Create: `svc/src/adapters/helikon/classify.rs`
- Modify: `svc/src/adapters/helikon/mod.rs` (`build_agent` takes the model registry type and adds the
  `memory` tool; its unit test changes)

**Interfaces:**
- Consumes: `PgRunSession`, `to_session_row` (Task 5), `ModelRegistry`, `SharedModel` (Task 10),
  `MemoryTool`.
- Produces (used by Task 12):

```rust
// application/worker/ports.rs
pub struct UserTurn { pub memory: Option<String>, pub events: String }
pub struct RunLimits { pub max_turns: u32, pub timeout: Duration }
pub struct RunSpec {
    pub agent: AgentDefinition, pub thread: ThreadId, pub fresh_thread: bool, pub turn: UserTurn,
    pub workspace: Arc<tokio::sync::Mutex<MemoryWorkspace>>, pub cancel: CancellationToken, pub limits: RunLimits,
}
pub enum RunEnd {
    Completed { output: String, usage: RunUsage },
    Stopped { output: String, reason: String, usage: RunUsage },
    Deterministic { error: String, close_thread: bool },
    Transient { error: String, retry_after: Option<Duration> },
    Cancelled,
}
pub struct RunOutput { pub end: RunEnd, pub session: Vec<SessionRow> }
#[async_trait] pub trait AgentRunner: Send + Sync { async fn run(&self, spec: RunSpec) -> RunOutput; }

// adapters/helikon/runner.rs
pub struct HelikonAgentRunner { /* db: DatabaseConnection, models: Arc<ModelRegistry> */ }
impl HelikonAgentRunner { pub fn new(db: DatabaseConnection, models: Arc<ModelRegistry>) -> Self }
impl AgentRunner for HelikonAgentRunner {}

// adapters/helikon/classify.rs
pub fn classify(result: Result<RunResult, RunError>) -> RunEnd;

// adapters/helikon/mod.rs
pub fn build_agent(def: &AgentDefinition, model: SharedModel, ws: Arc<Mutex<MemoryWorkspace>>) -> LlmAgent<(), SharedModel, String>;
```

`HelikonAgentRunner::run`:

1. `models.model(&spec.agent.model_id)`. None: `RunEnd::Transient { error: "the model of the agent is
   not registered", retry_after: None }`, empty session.
2. `build_agent` (instructions only in the system role; the `memory` tool only when `tool_names`
   holds `"memory"`).
3. `PgRunSession::new(db, thread, fresh_thread)` in an `Arc`.
4. `RunContext::new(Arc::new(()), session.clone(), HookRegistry::new(), TracerHandle::default(),
   spec.cancel).with_deferred_tool_calls(false)`.
5. `AgentInput::new()` with `messages = vec![Item::UserMessage { content }]`. `content` is
   `[Text(memory)]` (only when `Some`) followed by `[Text(events)]`.
6. `RunConfig::new().with_timeout(limits.timeout)` and then `config.max_turns = limits.max_turns`
   (`RunConfig` is `#[non_exhaustive]`).
7. `TokioRunner.run(&agent, ctx, input, config).await`, then `classify`.
8. `session` = `session.take_buffer()` mapped through `to_session_row`. A serialize error makes the
   end `Transient`.

`classify` (spec §6.7). The error text is `replace_nul(&e.to_string())`. Unwrap
`RunError::Agent(AgentError::Model(m))` to the model arm.

| Input | `RunEnd` |
|---|---|
| `Ok`, `StopReason::Completed` | `Completed { output: final_output, usage }` |
| `Ok`, `StopReason::Stopped { reason }` | `Stopped` |
| `Ok`, `StopReason::Deferred` | `Deterministic { close_thread: false }` |
| `RunError::{MaxIterations, Timeout, InvalidResume}` | `Deterministic` |
| `AgentError::{MaxTurnsExceeded, MaxIterationsExceeded, Guardrail, HookDenied, InvalidStructuredOutput, MaxAgentDepthExceeded, MaxHandoffsExceeded, Tool, InvalidInjection, NotImplemented, Incomplete}` | `Deterministic` |
| `ModelError::Refused` | `Deterministic` |
| `ModelError::ContextLengthExceeded` | `Deterministic { close_thread: true }` |
| `ModelError::RateLimited { retry_after_ms }` | `Transient { retry_after: retry_after_ms.map(Duration::from_millis) }` |
| `ModelError::{Unavailable, Transport, Other}`, `AgentError::{Session, Other}`, `RunError::{SessionWrite, Other}` | `Transient { retry_after: None }` |
| `RunError::Cancelled` | `Cancelled` |
| any other variant (wildcard arm, the enums are `#[non_exhaustive]`) | `Transient { retry_after: None }` |

`usage` converts `TokenUsage` field by field to `RunUsage`.

- [ ] **Step 1: Write the failing unit tests** in `classify.rs`:

```rust
#[test]
fn every_named_variant_has_its_class() {
    let cases: Vec<(Result<RunResult, RunError>, &str)> = vec![
        (ok(StopReason::Completed), "completed"),
        (ok(StopReason::Stopped { reason: "r".into() }), "stopped"),
        (ok(StopReason::Deferred { calls: vec![] }), "deterministic"),
        (Err(RunError::MaxIterations), "deterministic"),
        (Err(RunError::Timeout), "deterministic"),
        // InvalidResume, every AgentError of the table, ModelError::Refused …
        (Err(agent_model(ModelError::ContextLengthExceeded)), "deterministic+close"),
        (Err(agent_model(ModelError::RateLimited { retry_after_ms: Some(5000) })), "transient+5000"),
        (Err(agent_model(ModelError::Unavailable)), "transient"),
        (Err(agent_model(ModelError::Transport("x".into()))), "transient"),
        (Err(RunError::Agent(AgentError::Session(SessionError::Unavailable))), "transient"),
        (Err(RunError::SessionWrite(SessionError::Unavailable)), "transient"),
        (Err(RunError::Other(anyhow::anyhow!("x"))), "transient"),
        (Err(RunError::Cancelled), "cancelled"),
    ];
    for (input, want) in cases { assert_eq!(label(&classify(input)), want); }
}

#[test]
fn a_session_read_error_is_transient() { // Review Focus 5: Helikon reports a failed load as RunError::Other
    assert_eq!(label(&classify(Err(RunError::Other(anyhow::anyhow!("session failed"))))), "transient");
}

#[test]
fn the_usage_is_copied() { /* TokenUsage 7/3/1/2 -> RunUsage 7/3/1/2 */ }

#[test]
fn nul_in_an_error_text_is_replaced() { /* Refused { reason: "a\u{0}" } -> error contains '\u{FFFD}' */ }
```

  `label` is a test helper that prints the class, `close_thread` and `retry_after`. Construct
  `ModelError::Refused`, `AgentError::Guardrail` and the other struct variants with their fields.
  `RunResult` is `#[non_exhaustive]`: build it with `RunResult::default()` and set `stop_reason`,
  `final_output` and `usage`.

  In `runner.rs` (a tokio unit test with `support`-free fakes is not possible, because the session
  needs Postgres): add one Postgres test to `tests/run_session_pg.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scripted_run_returns_its_session_with_the_memory_part() {
    // HelikonAgentRunner with ModelRegistry { "scripted": support::ScriptedModel (one memory create, then text) }
    // RunSpec with turn.memory = Some("<memory-N …>"), fresh thread
    // out.end is Completed { output: "done", .. }
    // out.session[0].kind == "user_message" and holds 2 text parts (prepare_rows removes one only at the write)
    // the workspace holds one buffered change
}
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host classify --test run_session_pg --no-fail-fast`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the port, the runner, `classify` and the new `build_agent`.** Update the
  `build_agent` unit test: it now passes a `SharedModel` and a workspace, and it asserts that the
  agent with `tool_names = ["memory"]` has one tool named `memory`.

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host classify helikon --test run_session_pg`
Expected: PASS.

- [ ] **Step 5: Mutation proof.** Make one change: delete the `ModelError::Other` arm and change the
  wildcard arm to `Deterministic`. It compiles, because the wildcard still covers the variant. Run
  with `--no-fail-fast`. Expected: `every_named_variant_has_its_class` FAILS on `ModelError::Other`.
  Delete the mutation.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): run an agent through Helikon and classify every end of a run (SMA-790)"
```

---

### Task 12: `RunWorker` — one lease, its claims, and every outcome

**Files:**
- Create: `svc/src/application/worker/run.rs` (`RunWorker`)
- Create: `svc/src/application/worker/test_fakes.rs` (`#[cfg(test)] pub(crate) mod test_fakes;` in
  `worker/mod.rs`; Task 13 uses the same fakes, so they do not live inside `run.rs`)
- Modify: `svc/src/application/worker/mod.rs`
- Test: unit tests in `run.rs` (fakes for every port); `svc/tests/worker_pg.rs` (new, Docker-gated;
  add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: `LeaseKeeper` (Task 8), `AgentRunner` and `RunEnd` (Task 11), `AgentResolver`,
  `EventScreen`, `FaultPoint`, `RetryPolicy`, `render::events`, `run_start_context`,
  `render_run_start`, `run_nonce`, `MemoryStore`, the `names::` metric names (Task 6).
- Produces (used by Tasks 13 and 15):

```rust
pub struct WorkerSettings {
    pub claim: ClaimConfig, pub timing: KeeperTiming, pub limits: RunLimits, pub retry: RetryPolicy,
    pub max_runs_per_lease: u32, pub caps: MemoryCaps,
}
pub struct WorkerPorts {
    pub leases: Arc<dyn LeaseRepository>, pub claims: Arc<dyn ClaimRepository>, pub committer: Arc<dyn RunCommitter>,
    pub resolver: Arc<dyn AgentResolver>, pub screen: Arc<dyn EventScreen>, pub runner: Arc<dyn AgentRunner>,
    pub memory: Arc<dyn MemoryStore>, pub faults: Arc<dyn FaultPoint>,
}
pub enum LeaseEnd { Released, NothingToClaim, Dropped, Shutdown } // Debug, PartialEq
pub struct RunWorker { /* ports, settings */ }
impl RunWorker {
    pub fn new(ports: WorkerPorts, settings: WorkerSettings) -> Self;
    /// Steps 2 to 6 of spec §6.1 for one taken lease.
    pub async fn serve(&self, handle: LeaseHandle, shutdown: &CancellationToken) -> LeaseEnd;
}
```

`serve` algorithm (spec §6.1, §6.3 to §6.9):

```text
keeper = LeaseKeeper::start(handle, leases, claims, committer, timing); runs = 0
loop:
  claim = keeper.claim(&cfg)  Ok(None) -> release; return NothingToClaim (or Released when runs > 0)
                              Err(_)   -> log; drop keeper; return Dropped
  started = Instant::now()
  match self.run_claim(&keeper, claim, shutdown):   // below
    Next -> {}
    Drop -> drop keeper; return Dropped
    Shutdown -> keeper.release_after_shutdown(); return Shutdown
  runs += 1; observe AGENT_HOST_RUN_DURATION_SECONDS(started)
  if shutdown.is_cancelled() or runs >= max_runs_per_lease: keeper.release(); return Released

run_claim:
  all = claim event ids
  agent = resolver.resolve(inbox)          Err -> transient(all, text)
  verdicts = screen each event             Err -> transient(all, text)
  if any flagged: keeper.screen_out(ScreenOut { all: passed.is_empty(), .. })
      Ok(AllScreened) -> count AGENT_HOST_RUNS_TOTAL{class="screened"}; Next
      Ok(Continue)    -> continue with passed
      Err(LeaseLost | OutcomeUnknown) -> Drop      Err(other) -> transient(all, text)
  if keeper.is_lost() -> Drop
  ws = Arc::new(Mutex::new(MemoryWorkspace::new(memory, MemoryScope::new(agent id, principal), caps)))
  nonce = run_nonce()
  items = run_start_context(&mut ws.lock())      Err -> transient(passed, text)
  turn = UserTurn { memory: (!items.is_empty()).then(|| render_run_start(&items, &nonce)), events: render::events(&passed_events, &nonce) }
  if keeper.is_lost() -> Drop                     // Review Focus 3
  cancel = run token: child of keeper.cancellation(), also cancelled by `shutdown` (a linked task, aborted after the run)
  out = runner.run(spec).await
  faults.reach(AfterRun)
  if keeper.is_lost() -> count class "discarded"; Drop
  match out.end:
    Cancelled if shutdown.is_cancelled() -> count "shutdown"; Shutdown
    Cancelled -> transient(passed, "the run was cancelled")
    Transient { error, retry_after } -> transient(passed, error, retry_after)
    Completed / Stopped -> commit(RunRecord { class, session: out.session, memory: ws.take_changes(), close_thread: false, .. })
    Deterministic { error, close_thread } -> commit(RunRecord { class: Deterministic { error, retry: delays(passed) }, session: vec![], close_thread, .. })
  commit result:
    Ok -> count class; faults.reach(AfterCommit); Next
    Err(LeaseLost) -> count "discarded"; Drop
    Err(OutcomeUnknown) -> count AGENT_HOST_COMMIT_OUTCOME_UNKNOWN_TOTAL; Drop
    Err(MemoryRejected(e)) -> transient(passed, e.to_string())   // Display only, never the path
    Err(e) if e.is_unencodable() and the class was Completed/Stopped -> commit a Deterministic record (no session, no memory change: an empty set of the same scope)
    Err(Store(_)) -> transient(passed, text)

transient(targets, error, retry_after):
  keeper.back_off(BackOff { retry: delays(targets, retry_after), error, .. })
    Ok -> count "transient"; Next      Err -> Drop
delays(targets, retry_after) = per event: retry.delay(event.attempts, Jitter::new(rand 0.8..=1.2), retry_after)
```

The commit is not raced with the shutdown token, so a shutdown during a commit lets the commit
finish (Review Focus 1). `AGENT_HOST_RUNS_TOTAL` carries the label `class` with one of `completed`, `stopped`,
`deterministic`, `transient`, `screened`, `discarded`, `shutdown`.

- [ ] **Step 1: Write the failing unit tests** in `run.rs`. The fakes go in `test_fakes.rs`. Every
  fake must have a user in this task, or `warnings = deny` stops the test build. The fakes: `FakeLeases` (heartbeat Ok
  or `LeaseLost`), `FakeClaims` (a queue of claims), `FakeCommitter` (records every call; a result
  per method), `FakeResolver`, `FakeScreen` (flags chosen event ids, or errors), `FakeRunner`
  (returns a chosen `RunOutput`, counts calls, can wait for a cancel), `InMemoryMemoryStore`
  (`core` `test-support`). Use `tokio::test(start_paused = true)` where time matters.

```rust
#[tokio::test] async fn a_completed_run_commits_with_session_and_memory() { /* one commit, class Completed, session non-empty, runs_total{completed} */ }
#[tokio::test] async fn a_flagged_event_goes_dead_and_the_rest_runs() { /* screen_out(flagged [e1], all false); runner sees only e2 in turn.events */ }
#[tokio::test] async fn all_flagged_makes_no_model_call() { /* screen_out(all true) -> AllScreened; runner calls == 0; next claim */ }
#[tokio::test] async fn a_screen_error_backs_off_every_claimed_event() { /* back_off.retry covers e1 and e2 */ }
#[tokio::test] async fn a_resolver_error_is_transient() { /* back_off, no runner call */ }
#[tokio::test] async fn a_memory_store_error_at_run_start_is_transient() {}
#[tokio::test] async fn a_deterministic_end_commits_memory_and_drops_the_session() { /* commit class Deterministic, session empty, retry delays set */ }
#[tokio::test] async fn context_length_closes_the_thread() { /* record.close_thread == true */ }
#[tokio::test] async fn a_transient_end_backs_off_with_retry_after() { /* RateLimited 600 s -> every delay == 600 s */ }
#[tokio::test] async fn a_memory_rejection_backs_off_with_no_path_in_the_error() { /* back_off.error == "memory refused: memory conflict" */ }
#[tokio::test] async fn an_unencodable_completed_commit_becomes_a_failure_commit() { /* second commit is Deterministic */ }
#[tokio::test] async fn outcome_unknown_drops_the_keeper() { /* serve returns Dropped; no release call */ }
#[tokio::test] async fn a_lost_lease_after_the_run_writes_nothing() { /* heartbeat LeaseLost during run; no commit, no back_off; Dropped */ }
#[tokio::test] async fn a_lease_lost_before_the_model_call_writes_nothing() { /* Review Focus 3: lost after the claim; runner calls == 0; no commit */ }
#[tokio::test] async fn a_shutdown_in_the_model_loop_refunds_the_attempt() { /* runner waits for cancel; cancel shutdown; release_after_shutdown called; Shutdown */ }
#[tokio::test] async fn a_cancel_with_no_shutdown_and_no_lost_lease_is_transient() {}
#[tokio::test] async fn a_shutdown_during_the_commit_keeps_the_commit() { /* Review Focus 1: FakeCommitter.commit waits on a gate; cancel shutdown; open the gate; commit Ok; release (not release_after_shutdown); Released */ }
#[tokio::test] async fn max_runs_per_lease_releases_the_lease() { /* max 2, 3 claims queued; 2 commits; release called once */ }
```

- [ ] **Step 2: Write the failing Postgres tests** in `tests/worker_pg.rs` (real `PgLeaseStore`,
  `PgRunCommitter`, `ConfigAgentResolver`, `HelikonAgentRunner` with `support::ScriptedModel`,
  `PgMemoryStore`):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_text_never_reaches_session_event() {
    // seed /instructions.md = "keep-this-secret"; one event; script: one memory create, then text
    // after serve: the event is 'done'; session_event has rows
    // no session_event.payload::text contains '<memory-' or 'keep-this-secret'
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_spine_run_commits_once() { /* run 'completed', final_output 'done', memory file present, events done, attempts 1 */ }
```

- [ ] **Step 3: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host worker::run --test worker_pg --no-fail-fast`
Expected: FAIL to compile.

- [ ] **Step 4: Implement `RunWorker`** per the algorithm above.

- [ ] **Step 5: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host worker::run --test worker_pg`
Expected: PASS.

- [ ] **Step 6: Mutation proof.** In `prepare_rows` (Task 5), delete the memory filter call. Run
  `cargo nextest run -p paigasus-agent-host --test worker_pg --no-fail-fast`. Expected:
  `the_memory_text_never_reaches_session_event` FAILS. Delete the mutation.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the run worker that claims, screens, runs and commits (SMA-790)"
```

---

### Task 13: The worker pool (`run_worker_pool`), the candidate filter and the inbox lag

**Files:**
- Create: `svc/src/application/worker/pool.rs`
- Modify: `svc/src/adapters/persistence/lease_store/mod.rs` (`CANDIDATES_SQL`, `PgLeaseStore::serving`,
  the lag gauge)
- Modify: `svc/tests/lease_pg.rs` (the `acquire_next` tests call `.serving(...)`)
- Test: unit tests in `pool.rs`; `svc/tests/worker_pool_pg.rs` (new, Docker-gated; add it to
  `docker_preflight.rs`)

**Interfaces:**
- Consumes: `RunWorker::serve`, `LeaseEnd` (Task 12), `AGENT_HOST_WORKER_RESTARTS_TOTAL`, `AGENT_HOST_INBOX_LAG_SECONDS`.
- Produces (used by Tasks 14 and 16):

```rust
impl PgLeaseStore { pub fn serving(self, agents: Vec<AgentId>) -> Self } // default: serves no agent
pub struct PoolSettings { pub concurrency: u32, pub poll_interval: Duration, pub lease_ttl: Duration }
/// The pool task, in the form of `application::scheduler_loop::run_scheduler_loop` (SMA-788):
/// `main` spawns it in its `servers` JoinSet with a clone of the shutdown watch. It never returns
/// an error (an ended server task is fatal in `main`); it returns `Ok(())` only on shutdown.
pub async fn run_worker_pool(worker: Arc<RunWorker>, leases: Arc<dyn LeaseRepository>, settings: PoolSettings, holder_prefix: String, shutdown: watch::Receiver<()>) -> anyhow::Result<()>;
pub fn holder_prefix() -> String; // "<HOSTNAME or unknown-host>/<pid>"
pub fn poll_sleep(interval: Duration, jitter: f64) -> Duration; // interval × jitter, jitter clamped to [0.9, 1.1]
```

Rules:

- `CANDIDATES_SQL` gets `AND (i.org_id, i.agent_id) IN (SELECT * FROM unnest($2::uuid[], $3::uuid[]))`
  and selects the age of the oldest due event:
  `EXTRACT(EPOCH FROM now() - min(e.received_at))::float8 AS lag` from the same sub-query. The adapter
  sets the gauge `AGENT_HOST_INBOX_LAG_SECONDS` to the first row's `lag` (the oldest), or to 0 when there is no
  row. With an empty agent list, `acquire_next` returns `Ok(None)` with no SQL.
- Each task `i` of `0..concurrency` loops: `acquire_next(&format!("{prefix}/{i}"), ttl)`; `None` or
  an error (logged) sleeps `poll_sleep(...)` inside `tokio::select!` with the shutdown token, so a
  shutdown ends the sleep at once; `Some(h)` calls `worker.serve(h, &shutdown)`; a `Dropped` end
  sleeps one poll interval; a `Shutdown` end or a cancelled token ends the task.
- `run_worker_pool` makes the pool's own `CancellationToken` (spec §6.1). It waits on
  `shutdown.changed()` (or a dropped sender), then cancels the token and joins the worker tasks.
  It sets no drain timer of its own: `main`'s `drain` gives the server tasks `DRAIN_TIMEOUT` (10 s)
  and then aborts them, and dropping the pool's `JoinSet` aborts the worker tasks.
- The supervisor holds a `JoinSet` keyed by index. A task that ends with a panic restarts with the
  same index and increases `AGENT_HOST_WORKER_RESTARTS_TOTAL`. A task that ends normally while the token is
  live also restarts.

- [ ] **Step 1: Write the failing unit tests** in `pool.rs`. Use `worker::test_fakes` from Task 12:
  `FakeLeases` for `acquire_next`, and a real `RunWorker` built on the fakes, with a `FakeRunner`
  that panics once:

```rust
#[test] fn the_poll_sleep_stays_within_ten_percent() { assert_eq!(poll_sleep(Duration::from_millis(1000), 2.0), Duration::from_millis(1100)); assert_eq!(poll_sleep(Duration::from_millis(1000), 0.0), Duration::from_millis(900)); }
#[test] fn the_holder_names_host_and_pid() { assert!(holder_prefix().ends_with(&format!("/{}", std::process::id()))); }
#[tokio::test(start_paused = true)] async fn shutdown_ends_a_poll_sleep_at_once() { /* Review Focus 2: poll 60 s, no lease; send on the watch at 1 s; run_worker_pool returns Ok before 2 s */ }
#[tokio::test(start_paused = true)] async fn a_panicking_task_restarts_with_its_index() { /* holder "…/0" acquires again after the panic; restarts counter == 1 */ }
#[tokio::test(start_paused = true)] async fn the_pool_starts_concurrency_tasks() { /* 3 distinct holders */ }
```

- [ ] **Step 2: Write the failing Postgres tests** in `tests/worker_pool_pg.rs`:

```rust
#[tokio::test] async fn an_inbox_with_no_configured_agent_is_never_taken() {
    // inbox X (agent A, configured) and inbox Y (agent B, not configured), both with a due event
    // store.serving(vec![A]).acquire_next twice: only X; the second call is None; Y has no lease row
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_runs_per_lease_gives_a_second_inbox_a_turn() {
    // concurrency 1, max_runs_per_lease 1, batch_limit 1; inbox X with 3 events (oldest), inbox Y with 1 event
    // run the pool until Y's event is done; at that time X still has a pending event
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_shutdown_in_the_model_loop_refunds_the_attempt() {
    // a ScriptedModel that waits for cancel; spawn run_worker_pool with a watch receiver;
    // wait until the run is 'running'; send on the watch; join the pool task within 10 s
    // run 'discarded', event attempts 0, lease not live
}
#[tokio::test] async fn the_lag_gauge_reads_the_oldest_due_event() { /* event received 30 s ago; gauge >= 29.0 */ }
```

- [ ] **Step 3: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host worker::pool --test worker_pool_pg --test lease_pg --no-fail-fast`
Expected: FAIL to compile.

- [ ] **Step 4: Implement the pool, `serving`, the filter and the gauge.**

- [ ] **Step 5: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host worker::pool --test worker_pool_pg --test lease_pg`
Expected: PASS.

- [ ] **Step 6: Mutation proof.** Delete the `IN (SELECT * FROM unnest(...))` term. Run with
  `--no-fail-fast`. Expected: `an_inbox_with_no_configured_agent_is_never_taken` FAILS. Delete the
  mutation.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): add the worker pool and take only inboxes that an agent serves (SMA-790)"
```

---

### Task 14: The composition root and the `test-model` feature

**Files:**
- Modify: `svc/Cargo.toml` (`[features] test-model = []`)
- Modify: `svc/src/lib.rs`
  (`#[cfg(all(feature = "test-model", not(debug_assertions)))] compile_error!("the test-model feature is for debug test builds only");`)
- Create: `svc/src/adapters/test_model.rs` (`#[cfg(feature = "test-model")] pub mod test_model;` in
  `adapters/mod.rs`)
- Modify: `svc/src/main.rs`
- Modify: `svc/moon.yml` (`test` task: `args: ['--features', 'test-model']`, with a comment that
  explains `args`, not `command`, as `paigasus-iam/moon.yml` does)
- Modify: `svc/tests/support/mod.rs` (`spawn_service_in(url, dir, env)`; `spawn_service_with` calls it
  with no dir)
- Test: `svc/tests/boot_pg.rs`

**Interfaces:**
- Consumes: everything above.
- Produces (used by Task 16):

```rust
// adapters/test_model.rs (feature "test-model" only)
pub const SCRIPT_ENV: &str = "PAIGASUS_TEST_MODEL_SCRIPT";   // a JSON file: [[["call_id", "tool", {args}], …], …]
pub const BARRIER_STAGE_ENV: &str = "PAIGASUS_TEST_BARRIER_STAGE"; // "after_run" | "before_commit" | "after_commit"
pub const BARRIER_FILE_ENV: &str = "PAIGASUS_TEST_BARRIER_FILE";
pub struct FileScriptedModel { /* same behaviour as tests/support ScriptedModel */ }
impl FileScriptedModel { pub fn from_env() -> anyhow::Result<Option<FileScriptedModel>> }
pub struct FileBarrier { /* stage, path */ }
impl FileBarrier { pub fn from_env() -> Option<FileBarrier> }
impl FaultPoint for FileBarrier {} // at its stage: create the file, then std::future::pending().await
pub fn register(models: &mut ModelRegistry) -> anyhow::Result<()>; // registers "scripted" when SCRIPT_ENV is set
```

`main.rs` changes. SMA-788 changed `main.rs`: `boot` now returns `BootTasks { nats, scheduler }`,
and `serve` spawns each task of it in `servers` after the router install. Follow that pattern, and
update step 11 of the module doc ("start the NATS source, the scheduler loop and, when the pool
starts, the worker pool").

- `serve` builds two pools. The main pool:
  `ConnectOptions::new(url).max_connections(2 * concurrency + 8)`. The lease pool:
  `max_connections(concurrency + 1)`.
- After `validate`, `boot` builds `defs = config.agent_definitions()?`, the `ModelRegistry`
  (`test_model::register` under the feature) and `registry.check(&defs)?` (a boot error).
- The pool starts only when `config.worker.enabled && !defs.is_empty()`. When `[[agents]]` is empty,
  boot logs one warning: `"no [[agents]] is configured; the worker pool does not start"`. Replace the
  SMA-787 info line `"worker.enabled is read, and SMA-787 starts no worker"`.
- The `FaultPoint` is `FileBarrier::from_env()` under the feature, else `NoFaults`. It goes to
  `PgRunCommitter` and to `RunWorker`. The `EventScreen` is `AllowAll`.
- `BootTasks` gains `workers: Option<WorkerPoolTask>` (a struct with the `RunWorker`, the lease
  repository, the `PoolSettings` and the holder prefix). `serve` spawns
  `run_worker_pool(…, rx.clone())` in `servers`, as it spawns `run_scheduler_loop`, and logs
  `"the worker pool started"`. The existing `drain` with `DRAIN_TIMEOUT` ends it.
- `describe_agent_host_metrics` describes the eight new `names::AGENT_HOST_…` worker names.

- [ ] **Step 1: Write the failing tests** in `boot_pg.rs`:

```rust
#[tokio::test]
async fn with_no_agents_the_pool_does_not_start() {
    // spawn_service(url) with the default worker.enabled = true; wait for /readyz 200
    assert!(child.tail().contains("the worker pool does not start"), "{}", child.tail());
}

#[tokio::test]
async fn an_agent_with_an_unregistered_model_fails_the_boot() {
    // spawn_service_in(url, dir) with agent-host.toml: one [[agents]] with model = "gpt-x"
    // the process exits with failure; the log names "gpt-x"
}
```

  In `test_model.rs` (`#[cfg(test)]`, feature on):

```rust
#[test] fn the_script_file_parses_into_turns() { /* a temp file with one turn of one call */ }
#[test] fn an_unknown_barrier_stage_is_none() { /* stage "later" -> None */ }
```

- [ ] **Step 2: Run the tests to make sure they fail**

Run: `cargo nextest run -p paigasus-agent-host --features test-model --test boot_pg test_model --no-fail-fast`
Expected: FAIL (the old info line; no `test_model` module).

- [ ] **Step 3: Implement the feature module, the main wiring, the Moon args and
  `spawn_service_in`.**

- [ ] **Step 4: Run the tests to make sure they pass**

Run: `cargo nextest run -p paigasus-agent-host --features test-model && cargo clippy -p paigasus-agent-host --all-targets --features test-model -- -D warnings && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. Also run `cargo build --release -p paigasus-agent-host --features test-model`.
Expected: FAIL with the `compile_error!` text. Then `cargo build --release -p paigasus-agent-host`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/Cargo.lock rs/crates/services/paigasus-agent-host
git commit -m "feat(rs): start the worker pool at boot and add the test-model feature (SMA-790)"
```

---

### Task 15: The in-process crash matrix

**Files:**
- Test: `svc/tests/worker_crash_pg.rs` (new, Docker-gated; add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: `RunWorker`, `LeaseEnd` (Task 12), `PgRunCommitter` (Task 7), `FaultPoint`,
  `FaultStage` (Task 1), `support::commit`, `support::lease::snapshot`.

The test double (in the test file):

```rust
/// Reaches `stage`, tells the test, then waits. It never panics.
struct StageDouble { stage: FaultStage, reached: tokio::sync::Notify, gate: Option<tokio::sync::Semaphore> }
// mode Hang: notify, then std::future::pending().await
// mode Gate: notify, then wait for one permit from the test
```

Setup per test: one inbox (agent A configured for both workers), two pending events, a lease TTL of
3 s and a heartbeat of 1 s. Worker A: a `PgLeaseStore` and `PgRunCommitter` on pinned connections, a
`ScriptedModel` that creates `/memories/a.md`, and the double at the chosen stage. Worker B: its own
stores, `NoFaults`, a script that creates `/memories/b.md`. The double sits in the `FaultPoint` of
the worker (stages `AfterRun`, `AfterCommit`) or of A's committer (stage `BeforeCommit`).

Per stage:

| Test | Stage | Crash | Expected after B |
|---|---|---|---|
| `stage_1_after_the_tool_call` | `AfterRun`, Hang | abort A's task | no `/a.md`; `/b.md` once; every `session_event.run_id` is B's run; `attempts` 2; A's run `running`; events `done` once (by B's run) |
| `stage_2_inside_the_commit` | `BeforeCommit`, Hang | abort A's task | the same as stage 1 |
| `stage_3_after_the_commit` | `AfterCommit`, Hang | abort A's task | `/a.md` once, no `/b.md`; every `session_event.run_id` is A's run; `attempts` 1; A's run `completed`; B claims nothing |
| `stage_4_heartbeat_partition` | `AfterRun`, Gate | none | the test opens a transaction with `SELECT … FROM inbox_lease … FOR UPDATE`, opens the gate, sleeps TTL + 1 s (under the 5 s `lock_timeout`), then commits its transaction. A's commit gets `LeaseLost`. Then the same checks as stage 1 |

After each test: build A's record (A's run id and token from the `run` table, A's event ids, one
session row, a memory set that puts `/a.md`), make a raw handle with
`record_claim(a_token, Some(a_run))`, take `snapshot` plus the `session_event` and `memory_file` rows,
call `committer.commit(&mut raw, record)`, and assert `LeaseLost` and unchanged rows.

- [ ] **Step 1: Write the four tests** per the table and the stale-commit check.

- [ ] **Step 2: Run the tests**

Run: `cargo nextest run -p paigasus-agent-host --test worker_crash_pg`
Expected: PASS. (The code exists. This task proves it with mutations.)

- [ ] **Step 3: Mutation proofs** (each one compiles; `--no-fail-fast`; delete each one after):
  - Stages 1, 2 and 4: delete the `token = $2 AND expires_at > clock_timestamp()` check of the commit
    step 1 (`LOCK_SQL` matches on `inbox_id` only). Expected: `stage_1_after_the_tool_call`,
    `stage_2_inside_the_commit` and `stage_4_heartbeat_partition` FAIL. In stages 1 and 2 the stale
    commit of A succeeds (A's run row is still `running` with A's token). In stage 4 the live commit
    of A succeeds, so `/a.md` exists.
  - Stage 3: the step-1 mutation alone does not red stage 3, because A's run row is `completed` and
    step 2 refuses the stale commit. So for stage 3 also delete `AND status = 'running'` from the
    step-2 updates. Expected: `stage_3_after_the_commit` FAILS (the stale commit changes rows).
  - Record each result (test name and failing assertion) in the PR description.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-agent-host/tests
git commit -m "test(rs): add the in-process crash matrix of the run commit (SMA-790)"
```

---

### Task 16: The SIGKILL test on the real binary

**Files:**
- Test: `svc/tests/worker_sigkill_pg.rs` (new, Docker-gated; add it to `docker_preflight.rs`)

**Interfaces:**
- Consumes: `spawn_service_in` (Task 14), `support::fake_iam::{FakeIam, serve_grpc}`,
  `support::inbox::insert_inbox`, the `test_model` env names (Task 14).

```rust
#[cfg(not(feature = "test-model"))]
#[test]
fn the_test_model_feature_is_on() {
    panic!("worker_sigkill_pg needs --features test-model; the Moon test task passes it");
}

#[cfg(feature = "test-model")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_killed_worker_is_taken_over_and_the_event_is_done_once() { … }
```

Steps of the test:

1. Start Postgres and the fake IAM. Write `agent-host.toml` in a new directory under
   `std::env::temp_dir()`: one `[[agents]]` (model `scripted`, tools `["memory"]`), `[lease]
   ttl_secs = 3, heartbeat_interval_secs = 1`, `[worker] concurrency = 1, poll_interval_ms = 200`.
2. Process 1: `spawn_service_in` with the IAM env (as `ingress_latency_pg`), `PAIGASUS_TEST_MODEL_SCRIPT`
   (creates `/memories/first.md`), `PAIGASUS_TEST_BARRIER_STAGE=after_run`,
   `PAIGASUS_TEST_BARRIER_FILE=<dir>/barrier`. Wait for `/readyz` 200. Insert the inbox (agent uuid
   of the config, principal = the IAM owner).
3. POST one CloudEvent to `/v1/inboxes/{inbox}/events` (expect 202 or the existing success status).
4. Wait (budget 30 s) until the barrier file exists. Then `child.child.kill()` (SIGKILL on Unix) and
   `wait()`.
5. Process 2: the same directory, a script that creates `/memories/second.md`, no barrier env.
6. Wait (budget 30 s) until the event is `done`. Then assert: `attempts` is 2; `memory_file` has
   `/second.md` exactly once and no `/first.md`; `session_event` has rows, and every `run_id` is the
   committed run; the run of process 1 is `running`.

- [ ] **Step 1: Write the test.**

- [ ] **Step 2: Run the test**

Run: `cargo nextest run -p paigasus-agent-host --features test-model --test worker_sigkill_pg`
Expected: PASS. Without the feature: `cargo nextest run -p paigasus-agent-host --test worker_sigkill_pg`
Expected: FAIL (`the_test_model_feature_is_on`).

- [ ] **Step 3: Mutation proof.** Delete the step-1 lease check (as in Task 15). Run with
  `--no-fail-fast`. Expected: no change, because process 1 never reaches its commit. That is the
  correct result: the SIGKILL test proves the take-over, and the fence is proved in Task 15. Then
  make `FileBarrier` return without waiting (delete the `pending().await`). Expected: the test FAILS,
  because process 1 commits `/first.md` before the kill. Delete both mutations. Record both results
  in the PR description.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-agent-host/tests
git commit -m "test(rs): kill a worker process mid-run and check the take-over (SMA-790)"
```

---

### Task 17: The full gate graph

**Files:** none (only fixes that the gates find).

- [ ] **Step 1: Run the per-crate checks**

Run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p paigasus-agent-host --all-targets --features test-model -- -D warnings
cargo nextest run -p paigasus-agent-host-core
cargo nextest run -p paigasus-agent-host --features test-model
```
Expected: PASS. A Docker-gated suite that reports SKIP is not a pass: check that `docker_preflight`
passes.

- [ ] **Step 2: Run the full gate graph like CI** (from the repository root, the command between the
  `ci-targets` markers of the root `CLAUDE.md`):

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```
Expected: PASS.

  No single local bash runs every gate on this Mac (root `CLAUDE.md`, "This development Mac only").
  `repo:affected-smoke` needs system bash 3.2. `repo:ruff-ci`, `repo:next-public-free`,
  `repo:publish-metadata` and `repo:nats-permissions` need bash 4+. `repo:actionlint` needs bash 5
  and a pipe of at least 8192 bytes (read its preflight line). Pick one bash for the `moon ci` run,
  then run the gates that need the other bash directly (`<bash> ci/<gate>/run.sh`) and use those
  results. A `mapfile` or `declare -A` error, or an empty stdout with one `declare` line on stderr,
  means the wrong bash ran the gate. It is not a finding.

  Gates to watch for this change: `repo:affected-smoke` (the new Moon `args` on the test task, and
  any new `tests/` file), `repo:deny` and `repo:osv` (the testkit), `repo:machete`, and
  `repo:input-liveness`. On a failure, follow the `moon-diagnosis` procedure of the root `CLAUDE.md`
  (Step 0 first: copy `ciReport.json` and the task state directory before a re-run).

- [ ] **Step 3: Commit any gate fix** with a conventional message and the `Co-Authored-By` line.
