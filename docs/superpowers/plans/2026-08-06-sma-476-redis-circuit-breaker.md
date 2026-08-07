# SMA-476 Redis Circuit Breaker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `paigasus-iam` from paying ~2.1 s per Redis command against a blackholed backend, by short-circuiting commands once the backend is known-down.

**Architecture:** A new `RedisHandle` type in `adapters::redis_conn` wraps the existing `ConnectionManager` plus an `Arc`-shared circuit breaker and implements `redis::aio::ConnectionLike`. Because redis-rs's `AsyncCommands` is a blanket impl over that trait, all eleven existing command call sites and all five error postures keep working verbatim — only the field type changes. The breaker trips after 3 consecutive connection-ish failures, short-circuits for 2 s, then admits exactly one probe.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `redis` 1.3.0, `metrics` crate, `tokio`, `cargo nextest`, Moon task runner, Prometheus/promtool, Grafana.

**Spec:** `docs/superpowers/specs/2026-08-06-sma-476-redis-circuit-breaker-design.md` — read it before starting. Decision references below (D1–D13) point at that document.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- Conventional commits with a workspace scope: `feat(rs): …`, `docs(rs): …`, `test(rs): …`.
- **Commit message body must not contain `#NNN`** — a `#` issue reference (or a stray `token: value` line) makes commitlint fail `footer-leading-blank`. Write `SMA-476` or "owner/repo PR NNN". The subject must **start lowercase** and be **≤100 chars**.
- Do **not** bypass git hooks with `--no-verify`.
- Bash PATH lacks the proto-managed CLIs. Prefix every command with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- All work happens in the worktree at
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-476-redis-circuit-breaker`
  on branch `feature/sma-476-iam-circuit-break-a-known-down-redis`. Paths below are relative to it.
- `cargo nextest` exits non-zero on a target with **no tests** — use `--no-tests=pass`.
- Clippy runs as `cargo clippy --workspace -- -D warnings`; warnings are build failures.
- New metric names MUST be added to `paigasus-observability::names::ALL` or the `:observability-drift` gate fails.

## File Structure

| File | Responsibility | Task |
| -- | -- | -- |
| `rs/crates/libs/paigasus-observability/src/names.rs` | Metric-name registry — two new consts + `ALL` entries | 1 |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_gauge!`/`describe_counter!` exposition text | 1 |
| `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` | `RedisRole`, `Breaker`, `ProbePermit`, classifier, consts, `RedisHandle`, `ConnectionLike` impl, `connect`, test constructors, `test_support` | 2, 3, 4, 7 |
| `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs` | `Generations::Redis(RedisHandle)` | 5, 6 |
| `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs` | `RedisDecisionCache { conn: RedisHandle }` | 5, 6 |
| `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs` | `SliceCache { conn: RedisHandle }` | 5, 6 |
| `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs` | `RedisApiKeyCache { conn: RedisHandle }`, `from_connection` → `pub(crate)` | 5, 6 |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs` | `RedisJwksCache { conn: RedisHandle }` | 5, 6 |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | `connect_redis(url, role)`, `AppState::new` wiring | 5 |
| `moon.yml` | `redis-connect-single-site` gate — ban the `ConnectionManager` type name | 9 |
| `ops/observability/prometheus/rules/iam.rules.yml` | Three new alert rules | 10 |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | promtool fixture with control series | 10 |
| `ops/observability/grafana/dashboards/iam.json` | Breaker-state panel | 10 |
| `docs/ops/RUNBOOK-observability.md` | §4 posture rewrite, three alert entries, §6 shrink, blackhole procedure | 11 |

---

### Task 1: Metric names and exposition text

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`
- Modify: `rs/crates/services/paigasus-iam/src/main.rs` (inside `describe_iam_metrics`, after the `IAM_AUTHZ_DECISIONS_TOTAL` block near line 379)

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus_observability::names::IAM_REDIS_BREAKER_STATE: &str` (`"iam_redis_breaker_state"`) and `IAM_REDIS_BREAKER_TRANSITIONS_TOTAL: &str` (`"iam_redis_breaker_transitions_total"`). Task 2 emits both.

- [ ] **Step 1: Run the existing registry test to confirm a green baseline**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability
```

Expected: PASS.

- [ ] **Step 2: Add the two consts to `names.rs`**

Insert after the `IAM_STARTER_POLICY_RECONCILES_TOTAL` const (currently line 52), before the `// IAM outbox relay` comment:

```rust
// IAM Redis circuit breaker (SMA-476)
/// The circuit-breaker state for one Redis connection: `0` = closed (commands pass through),
/// `1` = half_open (one probe admitted), `2` = open (every command short-circuits instantly).
///
/// `role` is a CLOSED set — `authz` | `api_keys` | `jwks` — derived from a Rust enum, never from
/// anything caller-supplied, so it cannot mint cardinality.
///
/// Three attribution caveats, all consequences of how `AppState::new` shares connections rather
/// than of the breaker itself:
/// - `role="api_keys"` exists ONLY when `authz.cache.backend = "memory"` while
///   `api_keys.introspect_cache.backend = "redis"`. Otherwise the API-key cache reuses the authz
///   connection and its commands are attributed to `role="authz"` — a missing `api_keys` series
///   does NOT mean the API-key cache is idle.
/// - Two roles may front the SAME physical Redis with independent breakers, so `authz` at 0 while
///   `jwks` is at 2 does not imply two backends.
/// - Set independently by every replica — aggregate `max by (job, role)`, never `sum`.
pub const IAM_REDIS_BREAKER_STATE: &str = "iam_redis_breaker_state";
/// One increment per circuit-breaker state transition; `to` = `closed` | `half_open` | `open`.
///
/// NOT redundant with [`IAM_REDIS_BREAKER_STATE`]. The open window is 2 s while scrapes are
/// 15–30 s apart, so a breaker that opens and re-closes between two scrapes is invisible to the
/// gauge — `changes()` over it undercounts by construction. A chronically sick backend that flaps
/// is exactly the condition worth catching early, and this counter is the only artifact that
/// survives a sub-scrape-interval state.
pub const IAM_REDIS_BREAKER_TRANSITIONS_TOTAL: &str = "iam_redis_breaker_transitions_total";
```

Then add both to `ALL`, immediately after `IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL`:

```rust
    IAM_REDIS_BREAKER_STATE,
    IAM_REDIS_BREAKER_TRANSITIONS_TOTAL,
```

- [ ] **Step 3: Run the registry test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability
```

Expected: PASS — `all_names_are_unique_and_snake_case` covers the new entries.

- [ ] **Step 4: Add the exposition text in `main.rs`**

In `describe_iam_metrics`, immediately after the `describe_counter!(names::IAM_AUTHZ_DECISIONS_TOTAL, …)` block:

```rust
    describe_gauge!(
        names::IAM_REDIS_BREAKER_STATE,
        "Redis circuit-breaker state per connection: 0=closed, 1=half_open, 2=open. Label role=authz|api_keys|jwks. Set independently by every replica — aggregate max by (job, role), never sum. role=\"api_keys\" only exists when authz.cache.backend=\"memory\" and api_keys.introspect_cache.backend=\"redis\"; otherwise those commands are attributed to role=\"authz\"."
    );
    describe_counter!(
        names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL,
        "Redis circuit-breaker state transitions, labeled by role and to=closed|half_open|open. Catches flapping the gauge cannot see: the open window is 2s while scrapes are 15-30s apart."
    );
```

Also update the doc comment above `describe_iam_metrics` — it says "the 24 metric families"; make it 26.

- [ ] **Step 5: Build and verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam && cargo clippy -p paigasus-iam -p paigasus-observability -- -D warnings
```

Expected: clean build, no warnings.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs rs/crates/services/paigasus-iam/src/main.rs
git commit -m "feat(rs): register redis circuit-breaker metric names (SMA-476)"
```

---

### Task 2: The breaker state machine

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`

**Interfaces:**
- Consumes: `names::IAM_REDIS_BREAKER_STATE`, `names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL` (Task 1).
- Produces, all `pub(crate)` unless noted:
  - `pub enum RedisRole { Authz, ApiKeys, Jwks }` (`pub` because it appears in `RedisHandle`'s constructor signature, which Task 5's adapters call; the module is `pub(crate)` so this does not leak)
  - `const FAILURE_THRESHOLD: u32 = 3`, `const OPEN_DURATION: Duration`, `const HALF_OPEN_DEADLINE: Duration`
  - `struct Breaker` with `fn new(role: RedisRole) -> Arc<Breaker>`, `fn with_durations(role, open_duration, half_open_deadline) -> Arc<Breaker>`, `fn admit(self: &Arc<Self>) -> Admission`, `fn force_open_for_tests(self: &Arc<Self>)`
  - `enum Admission { Pass(ProbePermit), ShortCircuit }`
  - `struct ProbePermit` with `fn record<T>(self, result: &redis::RedisResult<T>)`
  - `const BREAKER_OPEN_MESSAGE: &str`, `fn breaker_open_error() -> redis::RedisError`
  - Task 3 consumes all of these.

**Background the implementer needs:** the breaker's lock is a `std::sync::Mutex` and its guard must **never** be held across an `.await`. redis-rs's `AsyncCommands` blanket impl is `impl<T> AsyncCommands for T where T: aio::ConnectionLike + Send + Sync + Sized` (`redis-1.3.0/src/commands/mod.rs:3288`), so `RedisHandle` must stay `Send + Sync` and the future it returns must be `Send`. A guard held across an await breaks that and will not compile.

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `redis_conn.rs`:

```rust
    // ---- SMA-476: circuit breaker ------------------------------------------------------

    /// Test breakers use a 50ms window so the whole suite stays fast; production values are
    /// pinned separately by `the_breaker_constants_are_pinned`.
    fn test_breaker() -> std::sync::Arc<Breaker> {
        Breaker::with_durations(RedisRole::Authz, Duration::from_millis(50), Duration::from_millis(200))
    }

    fn io_err() -> redis::RedisError {
        redis::RedisError::from((redis::ErrorKind::Io, "test io failure"))
    }

    fn pass(b: &std::sync::Arc<Breaker>) -> ProbePermit {
        match b.admit() {
            Admission::Pass(permit) => permit,
            Admission::ShortCircuit => panic!("expected the breaker to admit, but it short-circuited"),
        }
    }

    fn fail_once(b: &std::sync::Arc<Breaker>) {
        pass(b).record::<()>(&Err(io_err()));
    }

    #[test]
    fn a_closed_breaker_admits_every_command() {
        let b = test_breaker();
        for _ in 0..10 {
            pass(&b).record(&Ok(()));
        }
    }

    #[test]
    fn a_success_resets_the_consecutive_failure_count() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        pass(&b).record(&Ok(()));
        // Without the reset this third failure would be the threshold-tripping one.
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::Pass(_)), "two failures after a reset must not open the breaker");
    }

    #[test]
    fn three_consecutive_failures_open_the_breaker() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::ShortCircuit), "SMA-476 D6: three consecutive failures must open the breaker");
    }

    /// Named for what it actually asserts: a long run of short-circuits must not disturb
    /// recovery. (It cannot separately prove short-circuits are "not counted" — `on_failure` is
    /// a no-op in the Open state either way — so it does not claim to.)
    #[test]
    fn an_open_breaker_still_recovers_after_many_short_circuits() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        for _ in 0..100 {
            assert!(matches!(b.admit(), Admission::ShortCircuit));
        }
        std::thread::sleep(Duration::from_millis(60));
        pass(&b).record(&Ok(()));
        assert!(matches!(b.admit(), Admission::Pass(_)), "a successful probe must close the breaker");
    }

    #[test]
    fn exactly_one_probe_is_admitted_after_the_open_window() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        let mut admitted = 0;
        let mut permits = Vec::new();
        for _ in 0..50 {
            if let Admission::Pass(p) = b.admit() {
                admitted += 1;
                permits.push(p);
            }
        }
        assert_eq!(admitted, 1, "SMA-476 D8: half-open must admit exactly one probe, not {admitted}");
        std::mem::forget(permits); // keep Drop from re-opening before the assertion above is read
    }

    #[test]
    fn a_failed_probe_reopens_the_breaker_for_another_full_window() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));
        pass(&b).record::<()>(&Err(io_err()));
        assert!(matches!(b.admit(), Admission::ShortCircuit), "a failed probe must re-open immediately");
    }

    /// SMA-476 D8, defence 1. A probe future dropped mid-await (axum drops handler futures on
    /// client disconnect) must NOT leave the breaker half-open forever.
    #[test]
    fn a_dropped_probe_permit_reopens_the_breaker() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        drop(pass(&b)); // never records an outcome

        assert!(
            matches!(b.admit(), Admission::ShortCircuit),
            "SMA-476 D8: a dropped ProbePermit must record a failure and re-open — otherwise the \
             breaker wedges half-open for the process lifetime, silently bypassing the cache forever"
        );
    }

    /// SMA-476 D8, defence 2 — belt and braces, in case defence 1 is ever refactored away.
    #[test]
    fn a_stale_half_open_admits_another_probe() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        fail_once(&b);
        std::thread::sleep(Duration::from_millis(60));

        let permit = pass(&b);
        std::mem::forget(permit); // simulate a probe that never reports AND never drops
        assert!(matches!(b.admit(), Admission::ShortCircuit), "still within the half-open deadline");

        std::thread::sleep(Duration::from_millis(210));
        assert!(
            matches!(b.admit(), Admission::Pass(_)),
            "SMA-476 D8: a half-open older than HALF_OPEN_DEADLINE must admit another probe"
        );
    }

    /// SMA-476 D5. Neither half of the classifier is sufficient alone — see the decision.
    #[test]
    fn the_failure_classifier_counts_connection_errors_only() {
        // An IO error: the ordinary dead-backend case.
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Io, "io"))));
        // A connect TIMEOUT — the blackhole case this whole issue is about. redis-rs maps it to
        // io::ErrorKind::TimedOut (aio/runtime.rs:189-193), i.e. ErrorKind::Io, whose
        // retry_method() is RetryImmediately and NOT Reconnect — so a Reconnect-only classifier
        // would never open the breaker on a blackhole.
        assert!(counts_as_failure(&redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::TimedOut))));
        // Connection-fatal to redis-rs but NOT io errors (redis_error.rs:447-448).
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Parse, "parse"))));
        assert!(counts_as_failure(&redis::RedisError::from((redis::ErrorKind::AuthenticationFailed, "auth"))));
        // The backend ANSWERED — it is healthy, the fault is ours or the data's. Counting these
        // would let a data bug disable caching fleet-wide. (Note: `TypeError` does not exist in
        // redis 1.3.0 — the variant is `UnexpectedReturnType`.)
        assert!(!counts_as_failure(&redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "type"))));
        assert!(!counts_as_failure(&redis::RedisError::from((redis::ErrorKind::Client, "client"))));
    }

    /// An error the classifier does NOT count must behave like a success: the backend answered,
    /// so the connection is healthy.
    #[test]
    fn an_uncounted_error_resets_the_failure_count() {
        let b = test_breaker();
        fail_once(&b);
        fail_once(&b);
        pass(&b).record::<()>(&Err(redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "type"))));
        fail_once(&b);
        fail_once(&b);
        assert!(matches!(b.admit(), Admission::Pass(_)), "an uncounted error must reset the consecutive-failure count");
    }

    /// SMA-476 D4. The literal reaches the logs: unlike the five adapters (which log
    /// `err.kind()` only), `cedar_authorizer.rs:167` and `generation.rs:141` log the wrapping
    /// AuthzError with `error = %err`, i.e. this Display.
    #[test]
    fn the_short_circuit_error_is_an_io_error_carrying_no_connection_details() {
        let err = breaker_open_error();
        assert!(err.is_io_error(), "SMA-476 D4: the synthetic error must be indistinguishable from a real connection failure to all five adapters");
        let rendered = err.to_string();
        assert!(rendered.contains(BREAKER_OPEN_MESSAGE), "expected the pinned literal in {rendered:?}");
        assert!(!rendered.contains("redis://") && !rendered.contains("127.0.0.1"), "the short-circuit error must never echo connection details: {rendered:?}");
    }

    #[test]
    fn the_breaker_constants_are_pinned() {
        assert_eq!(FAILURE_THRESHOLD, 3, "SMA-476 D6: three consecutive failures — one would trip on every failover gap, which is exactly what SMA-473's single retry exists to absorb");
        assert_eq!(
            OPEN_DURATION,
            Duration::from_secs(2),
            "SMA-476 D7: recovery costs TWO windows (a half-open probe consumes ConnectionManager's memoized connect future), so this bounds recovery at ~2x2s + one dial. Do not raise it without re-reading D7."
        );
        assert_eq!(HALF_OPEN_DEADLINE, Duration::from_secs(5), "SMA-476 D8: must comfortably exceed a worst-case ~2.1s dial so it never pre-empts a merely-slow probe");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::redis_conn
```

Expected: FAIL to compile — `Breaker`, `RedisRole`, `Admission`, `ProbePermit`, `counts_as_failure`, `breaker_open_error`, and the three consts do not exist.

- [ ] **Step 3: Implement the breaker**

Add to the imports at the top of `redis_conn.rs`:

```rust
use metrics::{counter, gauge};
use paigasus_observability::names;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
```

Then, after the existing `connection_manager_config` function:

```rust
/// Consecutive counted failures that trip the breaker (SMA-476 D6). Three rather than one so a
/// first connect attempt landing in a failover gap — the case SMA-473's single retry exists for —
/// does not disable the cache. Under real concurrency a failover WILL still trip this; see D6.
pub(crate) const FAILURE_THRESHOLD: u32 = 3;

/// How long an open breaker short-circuits before admitting one probe (SMA-476 D7).
///
/// Two seconds, not five, because recovery costs TWO windows: `ConnectionManager` holds an
/// `ArcSwap<Shared<..>>` connect future (`redis-1.3.0/src/aio/connection_manager.rs:335,387`) and
/// `Shared` MEMOIZES, so the first probe after a quiet window consumes an already-resolved `Err`
/// in microseconds without touching the network, and only the SECOND probe sees a fresh dial.
/// The upside of the same fact is that probes are essentially free, which is what makes a short
/// window affordable.
pub(crate) const OPEN_DURATION: Duration = Duration::from_secs(2);

/// A half-open older than this admits another probe regardless (SMA-476 D8) — the second of two
/// defences against a probe whose future was dropped before it could report.
pub(crate) const HALF_OPEN_DEADLINE: Duration = Duration::from_secs(5);

/// The short-circuit error's message. Pinned by test and deliberately free of any URL, host or
/// credential: `cedar_authorizer.rs` and `generation.rs` log the wrapping `AuthzError` with
/// `error = %err`, so this literal reaches the logs (SMA-476 D4).
pub(crate) const BREAKER_OPEN_MESSAGE: &str = "redis circuit breaker open (SMA-476)";

/// Which connection a breaker guards. A CLOSED set, so the `role` metric label is bounded by the
/// type system and cannot mint cardinality (SMA-476 D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedisRole {
    Authz,
    ApiKeys,
    Jwks,
}

impl RedisRole {
    fn as_label(self) -> &'static str {
        match self {
            RedisRole::Authz => "authz",
            RedisRole::ApiKeys => "api_keys",
            RedisRole::Jwks => "jwks",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BreakerState {
    Closed,
    HalfOpen,
    Open,
}

impl BreakerState {
    fn gauge_value(self) -> f64 {
        match self {
            BreakerState::Closed => 0.0,
            BreakerState::HalfOpen => 1.0,
            BreakerState::Open => 2.0,
        }
    }

    fn as_label(self) -> &'static str {
        match self {
            BreakerState::Closed => "closed",
            BreakerState::HalfOpen => "half_open",
            BreakerState::Open => "open",
        }
    }
}

struct Inner {
    state: BreakerState,
    consecutive_failures: u32,
    changed_at: Instant,
}

/// Per-connection circuit breaker (SMA-476 D1). `Arc`-shared, so every clone of a
/// [`RedisHandle`] observes one breaker — load-bearing, because all eleven command call sites do
/// `self.conn.clone()` per command.
pub(crate) struct Breaker {
    role: RedisRole,
    open_duration: Duration,
    half_open_deadline: Duration,
    inner: Mutex<Inner>,
}

/// What [`Breaker::admit`] decided. `Pass` carries an RAII permit whose `Drop` records a failure
/// if no outcome was reported (SMA-476 D8).
pub(crate) enum Admission {
    Pass(ProbePermit),
    ShortCircuit,
}

/// Reports one command's outcome back to the breaker. Consumed by [`ProbePermit::record`]; if it
/// is instead DROPPED without recording — an axum handler future cancelled by a client
/// disconnect — `Drop` records a failure, so a half-open breaker can never wedge (SMA-476 D8).
pub(crate) struct ProbePermit {
    breaker: Arc<Breaker>,
    reported: bool,
}

impl ProbePermit {
    pub(crate) fn record<T>(mut self, result: &redis::RedisResult<T>) {
        self.reported = true;
        let healthy = match result {
            Ok(_) => true,
            // An uncounted error means the backend ANSWERED — the connection is healthy, and the
            // fault is ours or the data's (SMA-476 D5).
            Err(err) => !counts_as_failure(err),
        };
        if healthy {
            self.breaker.on_success();
        } else {
            self.breaker.on_failure();
        }
    }
}

impl Drop for ProbePermit {
    fn drop(&mut self) {
        if !self.reported {
            self.breaker.on_failure();
        }
    }
}

impl Breaker {
    pub(crate) fn new(role: RedisRole) -> Arc<Breaker> {
        Breaker::with_durations(role, OPEN_DURATION, HALF_OPEN_DEADLINE)
    }

    pub(crate) fn with_durations(role: RedisRole, open_duration: Duration, half_open_deadline: Duration) -> Arc<Breaker> {
        // Publish the healthy state up front (SMA-476 D10): an unset gauge renders as "No data",
        // which an operator cannot distinguish from a broken scrape or an unregistered metric.
        gauge!(names::IAM_REDIS_BREAKER_STATE, "role" => role.as_label()).set(BreakerState::Closed.gauge_value());
        Arc::new(Breaker {
            role,
            open_duration,
            half_open_deadline,
            inner: Mutex::new(Inner { state: BreakerState::Closed, consecutive_failures: 0, changed_at: Instant::now() }),
        })
    }

    /// Decide whether this command may reach the backend.
    ///
    /// The mutex guard is dropped before returning and is NEVER held across an `.await` — holding
    /// it would make the returned future `!Send` and break `AsyncCommands`' blanket impl, which
    /// requires `Send + Sync` (`redis-1.3.0/src/commands/mod.rs:3288`).
    pub(crate) fn admit(self: &Arc<Self>) -> Admission {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match inner.state {
                BreakerState::Closed => {}
                BreakerState::Open => {
                    if inner.changed_at.elapsed() < self.open_duration {
                        return Admission::ShortCircuit;
                    }
                    // Exactly one caller wins this transition; everyone else arrives to find
                    // HalfOpen and short-circuits below rather than queueing (SMA-476 D8).
                    self.transition(&mut inner, BreakerState::HalfOpen);
                }
                BreakerState::HalfOpen => {
                    if inner.changed_at.elapsed() < self.half_open_deadline {
                        return Admission::ShortCircuit;
                    }
                    // Stale: the admitted probe never reported and never dropped. Re-arm.
                    self.transition(&mut inner, BreakerState::HalfOpen);
                }
            }
        }
        Admission::Pass(ProbePermit { breaker: Arc::clone(self), reported: false })
    }

    fn on_success(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.consecutive_failures = 0;
        if inner.state != BreakerState::Closed {
            self.transition(&mut inner, BreakerState::Closed);
        }
    }

    fn on_failure(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match inner.state {
            BreakerState::Closed => {
                inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                if inner.consecutive_failures >= FAILURE_THRESHOLD {
                    self.transition(&mut inner, BreakerState::Open);
                }
            }
            BreakerState::HalfOpen => self.transition(&mut inner, BreakerState::Open),
            BreakerState::Open => {}
        }
    }

    fn transition(&self, inner: &mut Inner, next: BreakerState) {
        inner.state = next;
        inner.changed_at = Instant::now();
        if next == BreakerState::Closed {
            inner.consecutive_failures = 0;
        }
        gauge!(names::IAM_REDIS_BREAKER_STATE, "role" => self.role.as_label()).set(next.gauge_value());
        counter!(names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL, "role" => self.role.as_label(), "to" => next.as_label()).increment(1);
    }

    #[cfg(test)]
    pub(crate) fn force_open_for_tests(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.transition(&mut inner, BreakerState::Open);
    }
}

/// Which redis errors mean "the connection is bad" (SMA-476 D5). NEITHER half is sufficient
/// alone:
///
/// - `retry_method()` alone misses the blackhole case. A connect timeout becomes
///   `io::ErrorKind::TimedOut` (`redis-1.3.0/src/aio/runtime.rs:189-193`), i.e. `ErrorKind::Io`
///   that is not `is_connection_dropped()`, and `retry_method` maps that to `RetryImmediately`,
///   NOT `Reconnect` (`errors/redis_error.rs:451-464`).
/// - `is_io_error()` alone misses `Parse` and `AuthenticationFailed`, which redis-rs itself
///   treats as connection-fatal (`errors/redis_error.rs:447-448`) and which would otherwise drive
///   an endless reconnect loop the breaker never opens on.
///
/// Everything else — `UnexpectedReturnType`, `Client`, `Extension`, `InvalidClientConfig`,
/// `RESP3NotSupported`, every `Server(..)` — means the backend answered and is healthy.
/// `ErrorKind` is `#[non_exhaustive]`, so this must not be written as an exhaustive match.
fn counts_as_failure(err: &redis::RedisError) -> bool {
    err.is_io_error() || matches!(err.retry_method(), redis::RetryMethod::Reconnect | redis::RetryMethod::ReconnectFromInitialConnections)
}

/// The error an open breaker returns instead of dialling. `ErrorKind::Io` so `is_io_error()`
/// holds and all five adapters' error arms fire exactly as they do against a genuinely dead
/// socket (SMA-476 D4) — they all read `err.kind()` and nothing else.
fn breaker_open_error() -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::Io, BREAKER_OPEN_MESSAGE))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::redis_conn
```

Expected: PASS, including the four pre-existing SMA-473 tests.

- [ ] **Step 5: Lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: no warnings. If clippy flags `force_open_for_tests` as dead code, leave it — Task 3 uses it. If the build breaks because of that, add `#[allow(dead_code)]` with a comment naming Task 3, and remove the allow in Task 3.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs
git commit -m "feat(rs): add the redis circuit-breaker state machine (SMA-476)"
```

---

### Task 3: `RedisHandle` and the `ConnectionLike` interception

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs`

**Interfaces:**
- Consumes: everything Task 2 produced.
- Produces:
  - `pub struct RedisHandle` — `Clone`, `Send + Sync`, implements `redis::aio::ConnectionLike`
  - `pub(crate) async fn connect(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle>` (**signature change** — the old one took only `redis_url`)
  - `#[cfg(test)] pub(crate) fn new_lazy_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle>`
  - `#[cfg(test)] pub(crate) fn with_open_breaker_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle>`
  - `#[cfg(test)] pub(crate) mod test_support` with `pub(crate) struct Blackhole { pub(crate) url: String, … }` and `pub(crate) async fn start() -> Blackhole`
  - Task 4 uses `new_lazy_for_tests` + `test_support`; Task 5 uses `connect`; Task 6 uses `with_open_breaker_for_tests` + `test_support`.

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests`:

```rust
    /// SMA-476 D1. Every one of the eleven call sites does `self.conn.clone()` per command, so a
    /// `#[derive(Clone)]` over a non-`Arc` breaker field would compile and silently give every
    /// call its own breaker — which would never open. This is that guard.
    #[tokio::test]
    async fn cloning_a_handle_shares_one_breaker() {
        use redis::AsyncCommands;

        let handle = with_open_breaker_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed url");
        let mut clone = handle.clone();

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = clone.get("sma476:probe").await;
        let elapsed = started.elapsed();

        let err = result.expect_err("an open breaker must short-circuit with an error");
        assert!(err.to_string().contains(BREAKER_OPEN_MESSAGE), "a CLONE dialled instead of short-circuiting — the breaker is not Arc-shared: {err:?}");
        assert!(elapsed < Duration::from_millis(100), "short-circuit took {elapsed:?}");
    }

    /// The interception itself: an `AsyncCommands` call on a `RedisHandle` must route through the
    /// breaker. If `ConnectionLike` is ever implemented by delegating verbatim to the inner
    /// manager, this fails.
    #[tokio::test]
    async fn an_open_breaker_short_circuits_asynccommands_without_dialling() {
        use redis::AsyncCommands;

        let mut handle = with_open_breaker_for_tests("redis://127.0.0.1:1", RedisRole::Jwks).expect("well-formed url");
        let result: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
        let err = result.expect_err("an open breaker must error");
        assert!(err.is_io_error(), "SMA-476 D4: the short-circuit error must be an IO error");
        assert!(err.to_string().contains(BREAKER_OPEN_MESSAGE));
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::redis_conn
```

Expected: FAIL to compile — `with_open_breaker_for_tests` and `RedisHandle` do not exist.

- [ ] **Step 3: Implement `RedisHandle`**

Replace the existing `connect` function with the following (keep `connection_manager_config` exactly as it is):

```rust
/// A `ConnectionManager` behind a per-connection circuit breaker (SMA-476).
///
/// Implements [`redis::aio::ConnectionLike`], which is what makes the breaker transparent:
/// redis-rs's `AsyncCommands` is a blanket impl over that trait
/// (`redis-1.3.0/src/commands/mod.rs:3288` — note it requires `Send + Sync + Sized`, which the
/// trait's own declaration does not), so every `conn.get(..)` / `conn.set_ex(..)` call site keeps
/// compiling and behaving identically while gaining the breaker.
///
/// `req_packed_command` is the correct seam rather than merely a convenient one: it is where
/// `ConnectionManager` awaits its shared connect future, i.e. where the ~2.1 s against a
/// blackholed backend is actually spent.
///
/// **Coupling to watch on a redis-rs upgrade** (SMA-476 §6 risk 5): the `AsyncCommands` blanket
/// impl over `ConnectionLike`, and the `ArcSwap<Shared<..>>` memoization that makes recovery cost
/// two open windows rather than one.
#[derive(Clone)]
pub struct RedisHandle {
    conn: ConnectionManager,
    breaker: Arc<Breaker>,
}

impl redis::aio::ConnectionLike for RedisHandle {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a redis::Cmd) -> redis::RedisFuture<'a, redis::Value> {
        Box::pin(async move {
            let permit = match self.breaker.admit() {
                Admission::Pass(permit) => permit,
                Admission::ShortCircuit => return Err(breaker_open_error()),
            };
            let result = self.conn.req_packed_command(cmd).await;
            permit.record(&result);
            result
        })
    }

    fn req_packed_commands<'a>(&'a mut self, cmd: &'a redis::Pipeline, offset: usize, count: usize) -> redis::RedisFuture<'a, Vec<redis::Value>> {
        // Implemented for real even though no call site pipelines today: `unimplemented!()`
        // behind a trait method redis-rs may call is a live panic, not a placeholder.
        Box::pin(async move {
            let permit = match self.breaker.admit() {
                Admission::Pass(permit) => permit,
                Admission::ShortCircuit => return Err(breaker_open_error()),
            };
            let result = self.conn.req_packed_commands(cmd, offset, count).await;
            permit.record(&result);
            result
        })
    }

    fn get_db(&self) -> i64 {
        self.conn.get_db()
    }
}

/// Opens `redis_url` and wraps it in a [`RedisHandle`] — a [`ConnectionManager`] built with
/// [`connection_manager_config`] behind a fresh circuit breaker. The ONLY way this crate obtains
/// a Redis connection (enforced by the `repo:redis-connect-single-site` CI gate, which since
/// SMA-476 also bans naming the `ConnectionManager` type outside this module).
///
/// **Eager**: `new_with_config` awaits the initial connection, so a Redis that is down at boot
/// still fails `AppState::new` rather than yielding a manager that fails later. That preserves
/// the pre-SMA-473 contract — but note the tolerance window shrinks from ~6-12 s to ~200 ms, so a
/// Redis slow to start now costs one crash-restart (SMA-473 D10).
///
/// The boot dial is deliberately NOT breaker-mediated (SMA-476 D11): the breaker starts Closed
/// and wraps commands only. A single boot dial has nothing to break on.
///
/// Returns a bare [`redis::RedisResult`] rather than a domain error because the callers map it
/// differently on purpose: `http::connect_redis` to `AuthnError::Backend`,
/// `RedisJwksCache::connect` to the fail-closed `AuthnError::Unavailable`.
pub(crate) async fn connect(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let client = redis::Client::open(redis_url)?;
    let conn = ConnectionManager::new_with_config(client, connection_manager_config()).await?;
    Ok(RedisHandle { conn, breaker: Breaker::new(role) })
}

/// A lazily-connecting handle with a CLOSED breaker and short test durations.
///
/// Required wherever a test must actually dial: the production [`connect`] is eager, so against a
/// dead or blackholed backend it fails before any command can be issued.
#[cfg(test)]
pub(crate) fn new_lazy_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let client = redis::Client::open(redis_url)?;
    let conn = ConnectionManager::new_lazy_with_config(client, connection_manager_config())?;
    Ok(RedisHandle { conn, breaker: Breaker::new(role) })
}

/// A lazily-connecting handle whose breaker is forced OPEN, for proving that a call site
/// short-circuits rather than dials. NOT interchangeable with [`new_lazy_for_tests`].
#[cfg(test)]
pub(crate) fn with_open_breaker_for_tests(redis_url: &str, role: RedisRole) -> redis::RedisResult<RedisHandle> {
    let handle = new_lazy_for_tests(redis_url, role)?;
    handle.breaker.force_open_for_tests();
    Ok(handle)
}
```

- [ ] **Step 4: Rewire the two pre-existing SMA-473 tests that build a manager directly**

In `#[cfg(test)] mod tests`, replace the body of `a_command_against_an_unreachable_backend_fails_fast` so it uses the handle (its assertions stay identical). It must keep issuing **exactly one** command, so the breaker is still Closed when it measures:

```rust
    #[tokio::test]
    async fn a_command_against_an_unreachable_backend_fails_fast() {
        use redis::AsyncCommands;

        // Exactly ONE command: with more, SMA-476's breaker would open at the third and the
        // later ones would short-circuit rather than measuring a real dial.
        let mut conn = new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually reachable");

        let started = std::time::Instant::now();
        let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma473:probe").await;
        let elapsed = started.elapsed();

        let err = result.expect_err("an unreachable backend must error, not return a value");
        assert!(err.is_io_error(), "expected an IO/connection error, got {err:?} — the probe never actually dialed");
        assert!(!err.to_string().contains(BREAKER_OPEN_MESSAGE), "this must measure a real dial, not a short-circuit");

        assert!(
            elapsed < Duration::from_secs(2),
            "SMA-473: a command against a dead Redis took {elapsed:?}; the tuned config must bound it \
             well under 2s (stock redis-rs is ~6.3-12.6s and cost a measured 19-28s per authz decision)"
        );
    }
```

And update `connect_is_eager_so_a_dead_backend_fails_at_construction` to pass a role:

```rust
        let result = connect("redis://127.0.0.1:1", RedisRole::Authz).await;
```

- [ ] **Step 5: Add the blackhole listener to `test_support`**

Append to `redis_conn.rs`, outside `mod tests`:

```rust
/// Shared test fixtures for the SMA-476 breaker tests. Lives here rather than in each adapter so
/// the five posture tests (Task 6) and the blackhole measurement (Task 4) use one implementation.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as AsyncMutex;

    /// A listener that reproduces a BLACKHOLED Redis: it accepts the TCP connection and then
    /// never replies, so redis-rs's dial runs to `connection_timeout`.
    ///
    /// Why this is the right shape: the dial ALWAYS awaits a server response —
    /// `connection_setup_pipeline` always appends `CLIENT SETINFO LIB-NAME`/`LIB-VER`
    /// (`redis-1.3.0/src/connection.rs:1380-1400`) — and the whole dial, resolver included, sits
    /// inside `rt.timeout(connection_timeout)` (`client.rs:495-520`). So the connect burns the
    /// full 1 s, twice (one retry = two attempts), exactly as a dropped SYN would, with no root,
    /// no iptables and no Docker.
    ///
    /// **The accepted streams MUST be retained.** Dropping a `TcpStream` — the natural way to
    /// write "ignore the socket" — makes the kernel send FIN/RST, redis-rs's setup-pipeline read
    /// returns EOF immediately, and a command costs microseconds instead of ~2.1 s. The accept
    /// task's `JoinHandle` is retained for the same reason.
    ///
    /// **Precondition for the ~1 s-per-attempt bound:** the setup pipeline must be non-empty. It
    /// is guarded by `if !connection_info.skip_set_lib_name`, and an empty pipeline short-circuits
    /// to `Ok` without I/O (`aio/mod.rs:110-112`), which would move the hang to `response_timeout`
    /// (500 ms) instead. A plain `redis://host:port` URL (RESP2, no auth, db 0) keeps it non-empty
    /// — do not add credentials or a db index to these tests' URLs.
    pub(crate) struct Blackhole {
        pub(crate) url: String,
        responding: Arc<AtomicBool>,
        _held: Arc<AsyncMutex<Vec<tokio::net::TcpStream>>>,
        _accept: tokio::task::JoinHandle<()>,
    }

    impl Blackhole {
        /// Switch the listener from blackholing to answering as a minimal RESP server. Only
        /// affects connections opened AFTER this call. Used by the recovery test (Task 7).
        pub(crate) fn start_responding(&self) {
            self.responding.store(true, Ordering::SeqCst);
        }
    }

    pub(crate) async fn start() -> Blackhole {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binding an ephemeral loopback port never fails in tests");
        let port = listener.local_addr().expect("a bound listener always has a local address").port();
        let responding = Arc::new(AtomicBool::new(false));
        let held: Arc<AsyncMutex<Vec<tokio::net::TcpStream>>> = Arc::new(AsyncMutex::new(Vec::new()));

        let accept_responding = Arc::clone(&responding);
        let accept_held = Arc::clone(&held);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                if accept_responding.load(Ordering::SeqCst) {
                    tokio::spawn(serve_minimal_resp(stream));
                } else {
                    // Hold it open forever — see the struct doc for why dropping breaks the test.
                    accept_held.lock().await.push(stream);
                }
            }
        });

        Blackhole { url: format!("redis://127.0.0.1:{port}"), responding, _held: held, _accept: accept }
    }

    /// Answers just enough RESP for redis-rs to complete a dial and one `GET`: `+OK` for each
    /// setup-pipeline command, `$-1` (null bulk string) for anything else. Deliberately dumb —
    /// it only has to let the half-open probe succeed.
    async fn serve_minimal_resp(mut stream: tokio::net::TcpStream) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut buf = vec![0_u8; 4096];
        loop {
            let Ok(n) = stream.read(&mut buf).await else { return };
            if n == 0 {
                return;
            }
            let request = String::from_utf8_lossy(&buf[..n]).to_ascii_uppercase();
            // One reply per command in the batch. The setup pipeline sends CLIENT SETINFO twice;
            // a GET arrives on its own.
            let commands = request.matches("\r\n$").count().max(1);
            let mut reply = String::new();
            for _ in 0..commands {
                if request.contains("GET") && !request.contains("SETINFO") {
                    reply.push_str("$-1\r\n");
                } else {
                    reply.push_str("+OK\r\n");
                }
            }
            if stream.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::redis_conn
```

Expected: PASS. The crate as a whole will NOT compile yet — the five adapters still expect `connect(url)` and `ConnectionManager`. That is Task 5. To keep this task independently verifiable, run only the `redis_conn` module tests above; if `cargo nextest` refuses because the lib does not build, proceed to Step 7 and verify at the end of Task 5 instead, noting it in the commit.

> **Note for the implementer:** if changing `connect`'s signature breaks the build here, do NOT add a compatibility shim. Land Tasks 3 and 5 as one commit instead — the migration is mechanical and a half-migrated tree has no value.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs
git commit -m "feat(rs): wrap the redis connection in a breaker-aware handle (SMA-476)"
```

---

### Task 4: Measure the blackholed backend (AC1)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `new_lazy_for_tests`, `test_support::start`, `BREAKER_OPEN_MESSAGE` (Task 3).
- Produces: nothing consumed later. This task delivers AC1.

**Why in-crate and not `tests/`:** an integration test under `tests/` is a separate crate linking the lib built **without** `cfg(test)`, so it cannot see `new_lazy_for_tests` at all.

- [ ] **Step 1: Write the failing test**

```rust
    /// SMA-476 AC1: the blackholed shape, MEASURED rather than calculated. Before this test the
    /// ~2.1 s figure in the issue and the RUNBOOK was arithmetic — nothing had ever been run
    /// against a backend that swallows SYNs.
    ///
    /// One test, both numbers: command #1 is the pre-breaker per-command cost (the breaker is
    /// still Closed), commands #4+ are the post-breaker cost.
    ///
    /// Bounds are deliberately loose — same posture as
    /// `a_command_against_an_unreachable_backend_fails_fast`'s 2 s. A contended CI runner adds
    /// scheduler jitter, and these assertions only have to discriminate between "dialled"
    /// (~2.1 s) and "short-circuited" (~0), not to pin exact timings.
    #[tokio::test]
    async fn a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens() {
        use redis::AsyncCommands;

        let blackhole = test_support::start().await;
        let mut conn = new_lazy_for_tests(&blackhole.url, RedisRole::Authz).expect("well-formed redis URL");

        // --- Command #1: breaker Closed, so this is a real dial. THE measured number. ---
        let started = std::time::Instant::now();
        let first: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
        let first_elapsed = started.elapsed();

        let err = first.expect_err("a blackholed backend must error");
        assert!(err.is_io_error(), "expected an IO/timeout error, got {err:?}");
        assert!(!err.to_string().contains(BREAKER_OPEN_MESSAGE), "command #1 must be a real dial, not a short-circuit");
        assert!(
            first_elapsed >= Duration::from_millis(1900),
            "a blackholed command took only {first_elapsed:?} — the listener REFUSED or reset instead of \
             blackholing (check that test_support retains the accepted TcpStream), so this test is \
             measuring the wrong thing"
        );
        assert!(first_elapsed < Duration::from_millis(3500), "a blackholed command took {first_elapsed:?}, well past the expected ~2.1s (2 x connection_timeout + one jittered min_delay)");

        // --- Commands #2, #3: still Closed, still real dials. These trip the breaker. ---
        let _: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
        let _: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;

        // --- Command #4 onwards: breaker Open, short-circuited. ---
        for i in 4..=10 {
            let started = std::time::Instant::now();
            let result: redis::RedisResult<Option<Vec<u8>>> = conn.get("sma476:probe").await;
            let elapsed = started.elapsed();
            let err = result.expect_err("an open breaker must error");
            assert!(
                err.to_string().contains(BREAKER_OPEN_MESSAGE),
                "command #{i} dialled instead of short-circuiting — the breaker never opened"
            );
            assert!(
                elapsed < Duration::from_millis(100),
                "command #{i} took {elapsed:?}; an open breaker must return without touching the network"
            );
        }

        // --- The aggregate, which is what makes the fix legible. ---
        // Ten un-broken commands cost ~21s. Three real dials plus seven short-circuits cost
        // ~6.3s. This bound sits between, with margin on both sides: it fails if the breaker
        // never opens, and passes only if it did.
        let total = started.elapsed();
        assert!(total < Duration::from_secs(14), "ten commands against a blackholed backend took {total:?}; without the breaker this is ~21s, with it ~6.3s");
    }
```

> **Implementer note:** `started` is shadowed inside the loop. Bind the outer one as `let overall = std::time::Instant::now();` at the top (before command #1) and use `overall.elapsed()` for the aggregate assertion.

- [ ] **Step 2: Run to verify it measures the right thing**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib \
  adapters::redis_conn::tests::a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens --no-capture
```

Expected: PASS. **If the lower bound (`>= 1900ms`) fails, stop and debug** — it means the listener refused or reset rather than blackholing, and the test would otherwise be measuring nothing. Do not relax the bound to make it pass.

- [ ] **Step 3: Record the real numbers**

Run the test three times and note the actual `first_elapsed` and `total` values. These are the figures Task 11 puts in the RUNBOOK, replacing the calculated ~2.1 s. Write them into the commit message body.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && for i in 1 2 3; do cargo nextest run --no-tests=pass -p paigasus-iam --lib \
  adapters::redis_conn::tests::a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens 2>&1 | grep -E "PASS|FAIL"; done
```

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs
git commit -m "test(rs): measure the blackholed redis shape end to end (SMA-476)"
```

---

### Task 5: Migrate the five adapters and the composition root

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs:18,45,58-61,84-88,98-102`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs:28,117,126-137,233`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs:40,60,68,76,233`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs:199,210-212,221,386`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs:14,31,41`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:313-324,565-577,713-723`

**Interfaces:**
- Consumes: `RedisHandle`, `RedisRole`, `connect(url, role)` (Task 3).
- Produces: a compiling crate where no module outside `redis_conn.rs` names `ConnectionManager`. Task 6 tests these adapters; Task 9 gates on the absence of the type name.

**This task is atomic.** A half-migrated tree does not compile, so there is no meaningful intermediate commit.

- [ ] **Step 1: Migrate `generation.rs`**

- Replace `use redis::aio::ConnectionManager;` with `use crate::adapters::redis_conn::{RedisHandle, RedisRole};`
- `Generations::Redis(ConnectionManager)` → `Generations::Redis(RedisHandle)`
- In `redis_connect`: `crate::adapters::redis_conn::connect(redis_url, RedisRole::Authz).await`
- Leave the `read`/`bump` bodies (`conn.get(key)`, `conn.incr(key, 1_i64)`) **untouched** — they compile unchanged via the `AsyncCommands` blanket impl.
- Update the `Generations` enum doc to mention the breaker: append to the `redis` bullet, "…, itself behind a per-connection circuit breaker (SMA-476)."

- [ ] **Step 2: Migrate `decision_cache.rs`, `entity_cache.rs`, `oidc/redis_cache.rs`**

For each: swap the `use redis::aio::ConnectionManager;` import for `use crate::adapters::redis_conn::{RedisHandle, RedisRole};`, change the `conn:` field type and every `from_connection`/`connect` signature from `ConnectionManager` to `RedisHandle`, and pass the role in `connect`:

- `RedisDecisionCache::connect` → `RedisRole::Authz`
- `SliceCache::connect` → `RedisRole::Authz`
- `RedisJwksCache::connect` → `RedisRole::Jwks`

In `entity_cache.rs` remove the now-unused `#[cfg(test)] use redis::Client;` if it becomes dead.

Replace each file's `#[cfg(test)]` lazy-manager construction with the handle constructor. In `entity_cache.rs` (currently line 233):

```rust
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", crate::adapters::redis_conn::RedisRole::Authz).expect("well-formed redis URL, never actually reachable");
```

- [ ] **Step 3: Migrate `api_keys/cache.rs`**

Same swap, plus **narrow `from_connection` to `pub(crate)`** (SMA-476 D13):

```rust
    /// Builds a cache over an ALREADY-CONNECTED handle: mirrors
    /// `RedisDecisionCache::from_connection`/`SliceCache::from_connection` (SMA-444 Task 21) —
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` + this cache rather than each opening its own;
    /// `connect` above stays the standalone-caller/test entry point.
    ///
    /// `pub(crate)`, not `pub` (SMA-476 D13): `adapters::redis_conn` is a `pub(crate)` module, so
    /// a `pub fn` taking a `RedisHandle` would be a private-type-in-public-interface and
    /// `cargo clippy -- -D warnings` would fail the build. Every caller is in-crate.
    pub(crate) fn from_connection(conn: RedisHandle, ttl_secs: u64) -> Self {
```

Drop the `#[must_use]` if clippy now objects; keep it otherwise.

`RedisApiKeyCache::connect` passes `RedisRole::ApiKeys`.

- [ ] **Step 4: Migrate `http/mod.rs`**

Remove the `ConnectionManager` import. Change `connect_redis`:

```rust
/// Opens `redis_url` and wraps it in a breaker-guarded [`RedisHandle`] — shared by every
/// redis-backed cache `AppState::new` wires (the authz `Generations`/`RedisDecisionCache`/
/// `SliceCache` trio, SMA-444 Task 21; the API-key `RedisApiKeyCache`, SMA-445 Task 19, when it
/// can't reuse the already-open `redis_conn` LOCAL BINDING in `AppState::new` — not to be
/// confused with the [`crate::adapters::redis_conn`] MODULE this delegates to), mirroring
/// `RedisJwksCache::connect`'s connect pattern.
///
/// Delegates to [`crate::adapters::redis_conn::connect`] for the tuned reconnect retry budget
/// (SMA-473) and the per-connection circuit breaker (SMA-476) — this function owns only the
/// `AuthnError` mapping. `role` labels this connection's breaker metrics; see SMA-476 D10 for why
/// a shared connection reports as `authz` even when it also serves the API-key cache.
async fn connect_redis(redis_url: &str, role: RedisRole) -> Result<RedisHandle, AuthnError> {
    crate::adapters::redis_conn::connect(redis_url, role).await.map_err(|e| AuthnError::Backend(Box::new(e)))
}
```

Update the two local bindings' types and the two call sites:

- line ~313: `let (gens, redis_conn): (Generations, Option<RedisHandle>)`
- line ~322: `let conn = connect_redis(redis_url, RedisRole::Authz).await?;`
- line ~574: `connect_redis(redis_url, RedisRole::ApiKeys).await?`

Add `use crate::adapters::redis_conn::{RedisHandle, RedisRole};` to the imports.

- [ ] **Step 5: Build the whole crate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: clean. If clippy reports `private_interfaces`, a `pub fn` still takes a `RedisHandle` — narrow it to `pub(crate)` (D13).

- [ ] **Step 6: Run the full unit suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib
```

Expected: PASS — 414 pre-existing tests plus the new breaker ones.

- [ ] **Step 7: Confirm the type name is gone outside `redis_conn.rs`**

```bash
cd rs/crates/services/paigasus-iam && grep -rn "ConnectionManager" src tests | grep -vE ':[0-9]+:[[:space:]]*//' | grep -vE 'adapters/redis_conn\.rs:'
```

Expected: **no output**. Any hit must be migrated before Task 9 can tighten the gate.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src
git commit -m "refactor(rs): route every redis command through the breaker handle (SMA-476)"
```

---

### Task 6: Prove the five error postures are unchanged (AC3)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs` (its `mod tests`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/entity_cache.rs` (its `mod tests`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/api_keys/cache.rs` (its `mod tests`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/redis_cache.rs` (add a `mod tests`)

**Interfaces:**
- Consumes: `with_open_breaker_for_tests`, `test_support::start` (Task 3); the migrated adapters (Task 5).
- Produces: AC3's evidence.

**Why the blackhole listener and not a closed port:** a closed port refuses in microseconds, which is indistinguishable from a short-circuit. Pointed at a blackhole, a command that actually dialled would cost ~2.1 s — so the elapsed-time assertion is load-bearing rather than decorative.

- [ ] **Step 1: Write the failing test in `decision_cache.rs`**

```rust
    /// SMA-476 AC3: an OPEN breaker must not change the fail-open contract (D12) — a `get`
    /// still degrades to a plain miss, a `put` is still swallowed.
    ///
    /// Pointed at a BLACKHOLE, not a closed port: a closed port refuses in microseconds, which
    /// looks identical to a short-circuit. Here a command that actually dialled would cost
    /// ~2.1 s, so the elapsed assertion proves the breaker short-circuited.
    #[tokio::test]
    async fn an_open_breaker_keeps_the_decision_cache_failing_open() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, crate::adapters::redis_conn::RedisRole::Authz).expect("well-formed redis URL");
        let cache = RedisDecisionCache::from_connection(conn, 60);
        let key = decision_key("content-a", 2, &base_request());

        let started = std::time::Instant::now();
        let got = cache.get(&key).await;
        cache.put(&key, &sample_decision()).await;
        let elapsed = started.elapsed();

        assert!(got.is_none(), "SMA-476 AC3: an open breaker must read as a plain MISS, never an error (fail-open, D12)");
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the calls dialled instead of short-circuiting");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::authz::decision_cache
```

Expected: FAIL to compile if `test_support` is not `pub(crate)`-visible from a sibling module; make it so.

- [ ] **Step 3: Make it pass**

No production change should be needed — this test asserts existing behaviour. If it fails on the assertion rather than compilation, the `ConnectionLike` impl is wrong: check that `req_packed_command` returns `Err(breaker_open_error())` rather than propagating a dial.

- [ ] **Step 4: Add the equivalent test to `entity_cache.rs`**

```rust
    /// SMA-476 AC3, D11's half: an open breaker must fall through to the inner loader, not fail.
    #[tokio::test]
    async fn an_open_breaker_falls_through_to_the_inner_slice_loader() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, crate::adapters::redis_conn::RedisRole::Authz).expect("well-formed redis URL");
        let inner = Arc::new(CountingLoader::default());
        let cache = SliceCache::from_connection(inner.clone(), conn, 60);

        let started = std::time::Instant::now();
        let slice = cache.load(&prn("project", 2), &prn("principal", 1)).await.expect("an open breaker must never fail a load — it only bypasses the cache");
        let elapsed = started.elapsed();

        assert_eq!(inner.loads(), 1, "SMA-476 AC3: the inner (Postgres) loader must be reached");
        assert!(!slice.entities.is_empty(), "the inner loader's slice must be returned verbatim");
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the cache dialled instead of short-circuiting");
    }
```

If `entity_cache.rs`'s test module has no counting loader, add one:

```rust
    #[derive(Default)]
    struct CountingLoader {
        loads: std::sync::atomic::AtomicUsize,
    }

    impl CountingLoader {
        fn loads(&self) -> usize {
            self.loads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EntitySliceLoader for CountingLoader {
        async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(slice_for(resource, principal))
        }
        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Ok(7)
        }
    }
```

Reuse the file's existing `slice_for`/`prn` helpers; if it has none, copy the shapes from `tests/authz_cache_redis.rs`.

- [ ] **Step 5: Add the equivalent test to `api_keys/cache.rs`**

```rust
    /// SMA-476 AC3: fail-open (D5) is preserved under an open breaker.
    #[tokio::test]
    async fn an_open_breaker_keeps_the_api_key_cache_failing_open() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, crate::adapters::redis_conn::RedisRole::ApiKeys).expect("well-formed redis URL");
        let cache = RedisApiKeyCache::from_connection(conn, 30);
        let key_id = ApiKeyId::from(uuid::Uuid::from_u128(1));

        let started = std::time::Instant::now();
        let got = cache.get(key_id).await;
        cache.evict(key_id).await;
        let elapsed = started.elapsed();

        assert!(got.is_none(), "SMA-476 AC3: an open breaker must read as a plain MISS (fail-open, D5)");
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the calls dialled instead of short-circuiting");
    }
```

Match `ApiKeyId`'s actual constructor to the one the file's existing tests use.

- [ ] **Step 6: Add the fail-CLOSED test to `oidc/redis_cache.rs`**

This file has no test module today. Add one:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-476 AC3, the asymmetric one. `RedisJwksCache` is the ONLY fail-CLOSED Redis consumer:
    /// an open breaker must still produce `AuthnError::Unavailable` — the posture is unchanged,
    /// it just arrives instantly instead of after ~2.1 s (SMA-476 D9).
    #[tokio::test]
    async fn an_open_breaker_keeps_the_jwks_cache_failing_closed() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, crate::adapters::redis_conn::RedisRole::Jwks).expect("well-formed redis URL");
        let cache = RedisJwksCache { conn, ttl_secs: 300 };
        let issuer = Issuer::parse("https://idp.example.com").expect("a well-formed issuer");

        let started = std::time::Instant::now();
        let got = cache.get(&issuer).await;
        let elapsed = started.elapsed();

        assert!(
            matches!(got, Err(AuthnError::Unavailable)),
            "SMA-476 AC3: the JWKS cache must stay fail-CLOSED under an open breaker, got {got:?}"
        );
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the get dialled instead of short-circuiting");
    }
}
```

`RedisJwksCache`'s fields are private but the test module is a child of the same module, so direct construction works. If `CachedJwks`/`AuthnError` do not derive `Debug`, replace `{got:?}` with a static message.

- [ ] **Step 7: Run all four**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters
git commit -m "test(rs): pin the five redis error postures under an open breaker (SMA-476)"
```

---

### Task 7: Recovery test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` (its `mod tests`)

**Interfaces:**
- Consumes: `test_support::start`, `Blackhole::start_responding`, `new_lazy_for_tests` (Task 3).
- Produces: nothing consumed later.

- [ ] **Step 1: Write the failing test**

```rust
    /// SMA-476 D7. The breaker must re-close once the backend answers again.
    ///
    /// Asserts a BOUND, not an exact window count, on purpose. Recovery costs two windows today
    /// because `ConnectionManager` memoizes its connect future in an `ArcSwap<Shared<..>>`
    /// (`connection_manager.rs:335,387,681`), so the first probe after a quiet window consumes an
    /// already-resolved `Err` without dialling. That is a redis-rs internal: a future version
    /// that recovers in ONE window is an improvement and must not red this build.
    #[tokio::test]
    async fn the_breaker_recloses_once_the_backend_answers_again() {
        use redis::AsyncCommands;

        let blackhole = test_support::start().await;
        // A 50ms window keeps this test fast; the production value is pinned separately.
        let client = redis::Client::open(blackhole.url.as_str()).expect("well-formed redis URL");
        let conn = ConnectionManager::new_lazy_with_config(client, connection_manager_config()).expect("lazy construction never connects");
        let mut handle = RedisHandle {
            conn,
            breaker: Breaker::with_durations(RedisRole::Authz, Duration::from_millis(50), Duration::from_millis(500)),
        };

        for _ in 0..3 {
            let _: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
        }
        let short_circuited: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
        assert!(
            short_circuited.expect_err("expected an error").to_string().contains(BREAKER_OPEN_MESSAGE),
            "the breaker must be open before recovery can be tested"
        );

        blackhole.start_responding();

        // Probe repeatedly; each attempt is one window. Generous cap: two windows is the
        // expectation, ten is "it never recovers".
        let mut recovered = false;
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let result: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
            if result.is_ok() {
                recovered = true;
                break;
            }
        }

        assert!(recovered, "SMA-476 D7: the breaker never re-closed after the backend started answering — recovery is wedged");

        // And it stays closed: a command right after recovery must not short-circuit.
        let after: redis::RedisResult<Option<Vec<u8>>> = handle.get("sma476:probe").await;
        assert!(after.is_ok(), "the breaker re-opened immediately after a successful probe: {after:?}");
    }
```

> **Implementer note:** this test constructs `RedisHandle` by struct literal, which requires the test module to be a child of `redis_conn` (it is). It also names `ConnectionManager::new_lazy_with_config` — that is fine because the file is `redis_conn.rs`, the one place the CI gate allows it.

- [ ] **Step 2: Run to verify it fails, then passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib \
  adapters::redis_conn::tests::the_breaker_recloses_once_the_backend_answers_again --no-capture
```

Expected: PASS. If `recovered` stays false, the minimal RESP responder is not satisfying redis-rs's setup pipeline — print the bytes it receives and adjust the reply count. **Do not** weaken the assertion to make it pass.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs
git commit -m "test(rs): pin breaker recovery once redis answers again (SMA-476)"
```

---

### Task 8: Prove the metrics are emitted (AC5)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/redis_conn.rs` (its `mod tests`)
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml` if a test recorder dependency is needed

**Interfaces:**
- Consumes: Task 1's names, Task 2's `Breaker`.
- Produces: AC5's evidence.

**Why this exists:** `paigasus-observability`'s `tests/drift.rs` only proves that dashboard/rule expressions reference registered names. It never proves anything *emits* a family. Without this test, AC5 rests on construction claims alone.

- [ ] **Step 1: Check what recorder the repo already uses**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-476-redis-circuit-breaker
grep -rn "metrics_util\|DebuggingRecorder\|Snapshotter\|set_global_recorder" rs/crates --include=*.rs --include=*.toml | head -20
```

If `metrics-util`'s `DebuggingRecorder` is already used somewhere, copy that pattern. If nothing exists, add `metrics-util` as a `[dev-dependencies]` entry of `paigasus-iam` pinned to the version already in `rs/Cargo.lock`, and check whether `rs/deny.toml` needs a `[licenses] exceptions` entry (run the gate in Step 4 to find out).

- [ ] **Step 2: Write the failing test**

```rust
    /// SMA-476 AC5. `tests/drift.rs` proves rules reference registered names; nothing proves a
    /// name is ever EMITTED. This does.
    ///
    /// Uses a local recorder rather than the global one so it cannot race other tests.
    #[test]
    fn breaker_transitions_emit_the_gauge_and_the_counter() {
        use metrics_util::debugging::{DebuggingRecorder, DebugValue};

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();

        metrics::with_local_recorder(&recorder, || {
            let b = Breaker::with_durations(RedisRole::Jwks, Duration::from_millis(50), Duration::from_millis(200));
            // Three failures: Closed -> Open.
            for _ in 0..3 {
                match b.admit() {
                    Admission::Pass(permit) => permit.record::<()>(&Err(io_err())),
                    Admission::ShortCircuit => panic!("the breaker opened early"),
                }
            }
        });

        let snapshot = snapshotter.snapshot().into_vec();
        let gauge = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == names::IAM_REDIS_BREAKER_STATE)
            .expect("SMA-476 AC5: iam_redis_breaker_state was never emitted");
        assert!(
            gauge.0.key().labels().any(|l| l.key() == "role" && l.value() == "jwks"),
            "the gauge must carry the role label"
        );
        assert!(matches!(gauge.3, DebugValue::Gauge(v) if (v.into_inner() - 2.0).abs() < f64::EPSILON), "an open breaker must report 2, got {:?}", gauge.3);

        let transitions = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL)
            .expect("SMA-476 AC5: iam_redis_breaker_transitions_total was never emitted");
        assert!(matches!(transitions.3, DebugValue::Counter(n) if n >= 1), "expected at least one transition, got {:?}", transitions.3);
    }
```

> **Implementer note:** `metrics-util`'s snapshot tuple shape and `DebugValue` variants differ between versions. Adjust the destructuring to whatever the pinned version exposes; the assertions (name emitted, `role` label present, gauge == 2, counter >= 1) are what matter. If `with_local_recorder` is unavailable, use `metrics::set_global_recorder` once behind a `std::sync::OnceLock` and make this the only test that reads the snapshot.

- [ ] **Step 3: Run**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib adapters::redis_conn
```

Expected: PASS.

- [ ] **Step 4: Run the dependency gates (a new dev-dependency may need a waiver)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny repo:machete
```

Expected: PASS. If `deny` fails on a license, add an entry to `rs/deny.toml`'s `[licenses] exceptions` with a comment naming SMA-476. If `machete` reports `metrics-util` unused, it is because it is dev-only — confirm it is under `[dev-dependencies]`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "test(rs): assert the breaker emits its gauge and transition counter (SMA-476)"
```

---

### Task 9: Strengthen the CI gate

**Files:**
- Modify: `moon.yml:145-215` (the `redis-connect-single-site` task)

**Interfaces:**
- Consumes: Task 5's guarantee that no module outside `redis_conn.rs` names `ConnectionManager`.
- Produces: a gate that makes the breaker structurally unbypassable.

- [ ] **Step 1: Verify the precondition**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-476-redis-circuit-breaker/rs/crates/services/paigasus-iam
grep -rn "ConnectionManager" src tests | grep -vE ':[0-9]+:[[:space:]]*//' | grep -vE 'adapters/redis_conn\.rs:'
```

Expected: no output. If there is output, finish Task 5 first.

- [ ] **Step 2: Widen the gated pattern**

In `moon.yml`, change the `hits` line from:

```bash
      hits="$(grep -rnE 'ConnectionManager::new\(|ConnectionManager::new_with_config\(|ConnectionManagerConfig|\.get_connection_manager' src tests | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
```

to:

```bash
      hits="$(grep -rnE 'ConnectionManager|\.get_connection_manager' src tests | grep -vE ':[0-9]+:[[:space:]]*//' || true)"
```

Then **delete** the entire second check (the `lazy=` block through its closing `fi`) — the type-name ban subsumes it, since `new_lazy_with_config` cannot be called without naming the type.

- [ ] **Step 3: Update the gate's comment block**

Replace the "WHAT IS GATED" comment with:

```yaml
    # WHAT IS GATED: the `ConnectionManager` TYPE NAME itself, anywhere outside redis_conn.rs.
    #
    # SMA-473 gated only the constructors (`::new(`, `::new_with_config(`,
    # `ConnectionManagerConfig`, `.get_connection_manager`), because adapters legitimately HELD a
    # `ConnectionManager` in a field. Since SMA-476 they hold a `RedisHandle` instead, so the type
    # name has no business appearing anywhere else — and banning the name outright is strictly
    # stronger: you cannot bypass the circuit breaker without naming a connection, and you cannot
    # name one.
    #
    # This subsumes SMA-473's separate `new_lazy_with_config` rule (which required naming
    # `connection_manager_config()` on the same line): that call cannot be made without the type
    # name either, so the rule was deleted rather than kept as dead weight.
    #
    # `.get_connection_manager` is kept as its own term because the `redis::Client` convenience
    # constructors return a `ConnectionManager` without the caller ever naming the type
    # (`redis-1.3.0/src/client.rs:453`) — exactly the accidental/copy-paste bypass.
    #
    # SCOPE is `src/` AND `tests/`: a Docker-gated integration test is just as able to construct a
    # stock manager as production code.
    #
    # Comment lines are excluded so prose may still name the API — several module docs do. NOTE
    # the filter anchors on `//` only, so a `/* ... */` BLOCK comment naming ConnectionManager
    # WILL trip this gate. Use `//` in this crate when writing about the type.
    #
    # Two portability notes, both learned the hard way:
    #   - Do NOT anchor paths on `^\./`. GNU grep (CI, Linux) emits the `./` prefix; ugrep
    #     (some dev shells) strips it. An `^\./` anchor silently matches nothing on one of
    #     them, which would make this gate pass while guarding nothing. Passing `src tests`
    #     explicitly (rather than `.`) sidesteps that divergence entirely.
    #   - The comment filter anchors on `:[0-9]+:[[:space:]]*//` so it tests the CONTENT,
    #     not any `://` that happens to appear inside a redis URL on a code line.
    #
    # The control (`expected` must be non-empty) is what catches a pattern typo or an upstream
    # rename: without it, BOTH greps could go empty and the gate would pass while guarding nothing.
```

Also update the task `description` to mention SMA-476.

- [ ] **Step 4: Run the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:redis-connect-single-site
```

Expected: PASS.

- [ ] **Step 5: Prove the gate actually catches a violation**

```bash
cd rs/crates/services/paigasus-iam
printf '\n// scratch\nfn _sma476_gate_probe(_c: redis::aio::ConnectionManager) {}\n' >> src/adapters/authz/generation.rs
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:redis-connect-single-site; echo "exit=$?"
git checkout src/adapters/authz/generation.rs
```

Expected: `exit=1` with the offender printed. Then the `git checkout` restores the file. **If it exits 0, the gate is not guarding anything — fix the pattern before continuing.**

- [ ] **Step 6: Commit**

```bash
git add moon.yml
git commit -m "ci: ban the redis ConnectionManager type outside redis_conn (SMA-476)"
```

---

### Task 10: Alert rules, fixture, and dashboard panel

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`
- Modify: `ops/observability/grafana/dashboards/iam.json`

**Interfaces:**
- Consumes: Task 1's metric names.
- Produces: the alerting surface Task 11 documents.

- [ ] **Step 1: Add the three rules**

Append to the `iam` group in `iam.rules.yml`, after `IamAuthzRedisCacheBypassed`:

```yaml
      - alert: IamRedisBreakerOpen
        # SMA-476. `!= 0` rather than `== 2` on purpose: the gauge legitimately sits at 1
        # (half_open) while a probe is in flight, and an `== 2` expression with a `for:` clause
        # could reset every time a scrape lands on a probe — i.e. never fire during exactly the
        # outage it exists for.
        #
        # `max by (job, role)` because the gauge is per-replica: every replica sets its own, so
        # sum() would report nonsense.
        #
        # role="jwks" is EXCLUDED here and paged separately below at critical severity — an open
        # JWKS breaker is a total token-auth outage, not a degradation.
        expr: max by (job, role) (iam_redis_breaker_state{role!="jwks"}) != 0
        for: 2m
        labels: { severity: warning }
        annotations: { summary: "IAM Redis circuit breaker is not closed (role {{ $labels.role }})", description: "The Redis circuit breaker for role {{ $labels.role }} has been open or half-open for 2m, so authz/API-key cache commands are short-circuiting instantly instead of dialling. Decisions remain CORRECT (these caches fail open onto Postgres) but uncached. Recovery lags Redis by up to ~6s after it returns. See RUNBOOK's \"Authz availability posture\"." }
      - alert: IamJwksRedisBreakerOpen
        # SMA-476 D9. RedisJwksCache is the ONLY fail-CLOSED Redis consumer: its error propagates
        # through JwksProvider::key_for as AuthnError::Unavailable, so while this fires EVERY
        # token-authenticated request 503s. That is an outage, not a degradation.
        #
        # It cannot be left to IamAuthzRedisCacheBypassed as a "critical companion": that rule is
        # structurally SILENT under authz.cache.backend="memory", which is exactly the split
        # configuration where a JWKS Redis may be the only Redis in play.
        expr: max by (job, role) (iam_redis_breaker_state{role="jwks"}) != 0
        for: 1m
        labels: { severity: critical }
        annotations: { summary: "IAM JWKS Redis circuit breaker is not closed — token auth is failing closed", description: "The Redis circuit breaker for the JWKS cache has been open or half-open for 1m. RedisJwksCache fails CLOSED, so every token-authenticated request is returning 503 (API-key auth is unaffected — it fails open onto Postgres). Recovery lags Redis by up to ~6s after it returns. See RUNBOOK's \"Authz availability posture\"." }
      - alert: IamRedisBreakerFlapping
        # SMA-476 D10. Neither rule above can fire on a breaker that opens and re-closes INSIDE
        # one scrape interval: the open window is 2s while scrapes are 15-30s apart, so the gauge
        # is sampled at 0 most of the time and the `for:` clause never holds. A chronically sick
        # backend looks exactly like that, and this counter is the only artifact that survives a
        # sub-scrape-interval state.
        expr: sum by (job, role) (increase(iam_redis_breaker_transitions_total{to="open"}[10m])) > 5
        for: 0m
        labels: { severity: warning }
        annotations: { summary: "IAM Redis circuit breaker is flapping (role {{ $labels.role }})", description: "The Redis circuit breaker for role {{ $labels.role }} opened more than 5 times in 10m. Each open costs a cache-bypass window; repeated opens suggest an intermittently unhealthy backend rather than a clean outage. NOTE the state gauge may read 0 at scrape time — that is why this rule watches transitions, not state." }
```

- [ ] **Step 2: Add the promtool fixture**

Append to `tests/iam.test.yml`:

```yaml
  # SMA-476 breaker alerts. The `authz` series sits at 2 (open) and `api_keys` at 0 throughout:
  # that second series is the CONTROL. Without it both inputs would be nonzero and a broken expr
  # (`>= 0`, or a missing role filter) would still produce exactly the expected alerts and pass.
  - interval: 1m
    input_series:
      - series: 'iam_redis_breaker_state{job="iam", role="authz"}'
        values: '2+0x5'
      - series: 'iam_redis_breaker_state{job="iam", role="api_keys"}'
        values: '0+0x5'
      - series: 'iam_redis_breaker_state{job="iam", role="jwks"}'
        values: '2+0x5'
    alert_rule_test:
      - eval_time: 1m
        alertname: IamRedisBreakerOpen
        exp_alerts: []
      - eval_time: 3m
        alertname: IamRedisBreakerOpen
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, role: authz }
            exp_annotations:
              summary: "IAM Redis circuit breaker is not closed (role authz)"
              description: "The Redis circuit breaker for role authz has been open or half-open for 2m, so authz/API-key cache commands are short-circuiting instantly instead of dialling. Decisions remain CORRECT (these caches fail open onto Postgres) but uncached. Recovery lags Redis by up to ~6s after it returns. See RUNBOOK's \"Authz availability posture\"."
      # The jwks series is at 2 as well, but IamRedisBreakerOpen must NOT match it — proving the
      # role!="jwks" filter works rather than being decorative.
      - eval_time: 3m
        alertname: IamJwksRedisBreakerOpen
        exp_alerts:
          - exp_labels: { severity: critical, job: iam, role: jwks }
            exp_annotations:
              summary: "IAM JWKS Redis circuit breaker is not closed — token auth is failing closed"
              description: "The Redis circuit breaker for the JWKS cache has been open or half-open for 1m. RedisJwksCache fails CLOSED, so every token-authenticated request is returning 503 (API-key auth is unaffected — it fails open onto Postgres). Recovery lags Redis by up to ~6s after it returns. See RUNBOOK's \"Authz availability posture\"."

  # Flapping: eight opens in 10m against a control that never opens.
  - interval: 1m
    input_series:
      - series: 'iam_redis_breaker_transitions_total{job="iam", role="authz", to="open"}'
        values: '0+1x10'
      - series: 'iam_redis_breaker_transitions_total{job="iam", role="jwks", to="open"}'
        values: '0+0x10'
    alert_rule_test:
      - eval_time: 10m
        alertname: IamRedisBreakerFlapping
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, role: authz }
            exp_annotations:
              summary: "IAM Redis circuit breaker is flapping (role authz)"
              description: "The Redis circuit breaker for role authz opened more than 5 times in 10m. Each open costs a cache-bypass window; repeated opens suggest an intermittently unhealthy backend rather than a clean outage. NOTE the state gauge may read 0 at scrape time — that is why this rule watches transitions, not state."
```

- [ ] **Step 3: Run promtool**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool
```

Expected: PASS. `exp_annotations` must match the rendered text **exactly**, including the `{{ $labels.role }}` substitution — if it fails, copy the rendered string promtool prints rather than hand-editing.

- [ ] **Step 4: Add the Grafana panel**

Open `ops/observability/grafana/dashboards/iam.json`, copy the panel object for an existing gauge (search for `iam_outbox_parked_rows`), and add one alongside it:

- `title`: `"Redis circuit breaker state (0=closed 1=half-open 2=open)"`
- `targets[0].expr`: `max by (role) (iam_redis_breaker_state)`
- `targets[0].legendFormat`: `{{role}}`
- Give it a unique `id` (one higher than the current maximum) and place its `gridPos` below the existing panels.

- [ ] **Step 5: Run the drift gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:observability-drift repo:promtool
```

Expected: PASS. A failure here means a metric name in the dashboard or rules is not in `names::ALL` — check Task 1 landed.

- [ ] **Step 6: Commit**

```bash
git add ops/observability
git commit -m "feat(ops): alert and dashboard the redis circuit breaker (SMA-476)"
```

---

### Task 11: RUNBOOK, and the manual blackhole procedure

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` §4 "Authz availability posture" (~line 1005), §4 alert list, §4 line ~1055, §6 (~line 1674)

**Interfaces:**
- Consumes: Task 4's measured numbers, Task 10's alert names.
- Produces: AC4 and the documentation half of AC5.

- [ ] **Step 1: Run the manual blackhole procedure and record real numbers**

```bash
# Start a Redis, note the published port.
docker run -d --name sma476-redis -p 6399:6379 redis:7-alpine
# Confirm it answers.
redis-cli -p 6399 ping    # PONG
# Freeze it: the process stops but the listening socket stays in the kernel, so connections
# are accepted into the backlog and then never answered — the same shape as the hermetic test.
docker pause sma476-redis
# Time a dial. Expect ~1s (one connection_timeout) for a single attempt.
time redis-cli -p 6399 -t 30 get sma476:probe
docker unpause sma476-redis
time redis-cli -p 6399 get sma476:probe
docker rm -f sma476-redis
```

Record the paused and unpaused timings. If `redis-cli` is unavailable, use the numbers from Task 4's test run instead and say so in the RUNBOOK.

- [ ] **Step 2: Rewrite §4's "A blackholed Redis is the residual" paragraph**

Replace it with text that:
- States the ~2.1 s per-command figure as **measured**, citing
  `adapters::redis_conn::tests::a_blackholed_backend_costs_seconds_per_command_until_the_breaker_opens`
  and the numbers from Step 1 / Task 4.
- Reframes that cost: it now applies only to the failures that open the breaker (3 consecutive) and to the request cohort already in flight when the outage starts — **not** to every command.
- Documents the breaker: `FAILURE_THRESHOLD = 3`, `OPEN_DURATION = 2s`, `HALF_OPEN_DEADLINE = 5s`, one breaker per connection.
- Documents **why recovery costs two windows** (`ConnectionManager` memoizes its connect future in an `ArcSwap<Shared<..>>`, so the first probe consumes a stale resolved `Err` without dialling) and the resulting ~6 s bound.
- Documents the JWKS asymmetry: `RedisJwksCache` fails closed, so while its breaker is open **every token-authenticated request 503s**, including for up to ~6 s after Redis recovers. A routine failover under load now trips it.
- Documents how to read `iam_redis_breaker_state` and `iam_redis_breaker_transitions_total`, including the three attribution caveats (`api_keys` only exists in the split config; two roles may front one physical Redis; aggregate `max by (job, role)`).

- [ ] **Step 3: Correct the now-conditional claim at ~line 1055**

The existing sentence — "a `ConnectionManager` burns a **full cycle per failed command**, because the failing command only kicks off a background reconnect and the *next* command awaits a brand-new cycle" — is true only when commands are issued faster than a dial completes. Add a clause saying so and pointing at the breaker, which deliberately introduces a longer gap. Left uncorrected it contradicts the recovery model above it.

- [ ] **Step 4: Add three alert entries to §4**

`IamRedisBreakerOpen`, `IamJwksRedisBreakerOpen`, `IamRedisBreakerFlapping`, in the house format used by the surrounding entries (what fired, what it means, what to check, what to do). For each, the "what to do" must distinguish "Redis is genuinely down — fix Redis" from "the breaker is flapping — the backend is intermittently unhealthy".

- [ ] **Step 5: Add the manual blackhole procedure as a §4 subsection**

Document both mechanisms **accurately**, because the obvious descriptions are wrong:

- `docker pause` does **not** drop SYNs. The cgroup freezer stops the *process*; the listening socket stays in the kernel, which completes handshakes into the accept backlog (redis's `tcp-backlog` defaults to 511). So it reproduces the accept-and-never-reply shape — connect succeeds, the read hangs — until the backlog fills. That is the right shape and the easiest to run; it is just not a SYN drop. `docker unpause` makes it a recovery test.
- `iptables -I INPUT -p tcp --dport 6379 -j DROP` will **not** catch host traffic to a Docker-published port: that path is DNAT'd in `nat PREROUTING`/`OUTPUT` and traverses `FORWARD`/`DOCKER-USER`, not `INPUT`. The rule must go in `DOCKER-USER`, or target the container's own network namespace.

- [ ] **Step 6: Shrink §6's Redis-breaker bullet**

Replace the existing bullet with one that says the breaker **shipped** in SMA-476, and lists only what remains open: `connection_timeout` still at redis-rs's 1 s (SMA-476 D2 — a remote/managed Redis makes a global tightening a false-trip risk), and SMA-473 D10's boot-tolerance residual, noting that a retry-loop-at-boot would interact with the breaker's D11 "boot dial is not breaker-mediated" decision and must be revisited with it.

- [ ] **Step 7: Verify no metric-name drift**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:observability-drift
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(ops): document the redis circuit breaker and its measured cost (SMA-476)"
```

---

### Task 12: Full CI graph

**Files:** none — verification only.

- [ ] **Step 1: Run the whole gate set exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-476-redis-circuit-breaker
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. Per-project tasks do **not** run the repo-level gates, so this is the only command that proves CI will pass.

- [ ] **Step 2: Diagnose any unattributed failure**

Moon reports "N failed" without naming the task. Find it:

```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 3: Run the Docker-gated integration suites**

The spec (§4.6) flags these as needing verification rather than assumption — they stop a Redis container mid-test and keep issuing commands, so the breaker may now open mid-run. Their assertions are about `None` / fall-through / `Unavailable`, all of which an open breaker produces identically, but confirm it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam \
  --test authz_cache_redis --test authz_generations_redis --test api_key_cache_redis \
  --test redis_jwks_cache --test authz_acceptance
```

Expected: PASS (requires Docker). If one fails, do **not** relax its assertion — work out whether the breaker changed real behaviour, and report it before proceeding.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(rs): reconcile the docker redis suites with the breaker (SMA-476)"
```

Skip if nothing changed.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
| -- | -- |
| D1 per-connection scope + Arc-shared | 2, 3 (`cloning_a_handle_shares_one_breaker`) |
| D2 timeouts unchanged, no config knob | 3 (SMA-473 tests stay unedited), 11 (§6) |
| D3 `ConnectionLike` interception, Send/Sync | 3 |
| D4 synthetic `Io` error, pinned literal | 2 |
| D5 classifier | 2 |
| D6 threshold 3 | 2 |
| D7 `OPEN_DURATION = 2s`, two-window recovery | 2 (const pin), 7 (recovery), 11 (docs) |
| D8 one probe, RAII guard, staleness deadline | 2 |
| D9 uniform coverage incl. JWKS | 5, 6, 10 (critical alert) |
| D10 two metrics, closed role set | 1, 2, 8 |
| D11 boot dial not breaker-mediated | 3 (`connect` doc), 11 (§6) |
| D12 gate bans the type name | 9 |
| D13 `from_connection` → `pub(crate)` | 5 |
| §4.1 breaker state machine tests | 2 |
| §4.2 blackhole measurement (AC1) | 4 |
| §4.3 five posture tests (AC3) | 6 |
| §4.4 recovery test | 7 |
| §4.5 emission test (AC5) | 8 |
| §4.6 existing suites | 12 |
| §5 documentation (AC4) | 11 |
| §3.5 rules + fixture + dashboard | 10 |

No gaps.

**Type consistency:** `RedisHandle`, `RedisRole`, `Breaker`, `Admission`, `ProbePermit`, `counts_as_failure`, `breaker_open_error`, `BREAKER_OPEN_MESSAGE`, `new_lazy_for_tests`, `with_open_breaker_for_tests`, `force_open_for_tests`, `test_support::start`, `Blackhole::start_responding` are each defined once (Tasks 2–3) and used with the same names and signatures in Tasks 4–8. `connect(url, role)` has one signature throughout. Metric consts are defined in Task 1 and referenced as `names::IAM_REDIS_BREAKER_STATE` / `names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL` everywhere after.

**Known soft spots flagged inline rather than left to be discovered:**
- Task 3 Step 6 — the crate will not compile between Tasks 3 and 5; the note tells the implementer to combine the commits rather than build a shim.
- Task 4 Step 1 — the `started` shadowing is called out with the fix.
- Task 8 Step 2 — `metrics-util`'s snapshot API varies by version; the assertions that matter are stated separately from the destructuring.
- Task 10 Step 3 — `exp_annotations` must match rendered output exactly.
