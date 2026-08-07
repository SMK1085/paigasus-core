# Generation-counter rewind guard (SMA-474) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the two Redis authz generation counters detect and self-heal a rewind, so a lost `iam:authz:entity_gen` key can no longer put the fleet back into a cache key space that still holds live, pre-change entries.

**Architecture:** All logic lands in one file, `adapters/authz/generation.rs`. `Generations::Redis` grows a per-counter process-local high-water mark plus a single-flight repair gate. Every `read`/`bump` runs a **pure** guard function against the observed value; a value below the high-water is a rewind, repaired with one atomic `INCRBY key (high_water + 1_000_000)`. A failed repair never returns a new error — it falls back to a disjoint local key space. Neither cache key shape changes, and the memory backend is untouched.

**Tech Stack:** Rust (edition 2024, 1.95), `redis` crate v1 with `connection-manager` (no `script` feature), `tokio::sync::Mutex`, `metrics` crate, Prometheus/promtool, Grafana JSON.

## Global Constraints

- **Design doc:** `docs/superpowers/specs/2026-08-06-sma-474-generation-counter-rewind-design.md`. Every decision reference (D1–D5, §3.x) points there.
- **`REWIND_JUMP = 1_000_000`** — never `+1`. A minimum jump is *actively harmful* (design §3.4).
- **A failed repair returns `Ok`, never `Err`** (D4). `Err` from `read`/`bump` continues to mean only "the Redis command failed".
- **Redis counters are i64**, not u64. The repair delta is capped; see `REPAIR_DELTA_CEILING`.
- **Never log a Redis `Display`/message** — `ErrorKind` only. Existing posture across this crate (`decision_cache::log_get_miss`, `entity_cache::log_get_bypass`).
- **An error is never a metric label.** Closed label sets only.
- **Commit messages:** conventional commits, scope from `[rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace]`, subject starts lowercase, ≤100 chars. No `#NNN` anywhere in the body — it breaks `footer-leading-blank`. Write "SMA-474", never "#474".
- **Do NOT use `git stash`** — the stash stack is shared across worktrees.
- Prefix all `moon`/`cargo` shell invocations with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Tests run under `cargo nextest`, from the `rs/` directory.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs` | The whole mechanism: `Which`, `guard`, `RedisGenerations`, repair | 1, 2, 3 |
| `rs/crates/libs/paigasus-observability/src/names.rs` | Metric name const + `ALL` registration (drift-gated) | 2 |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!` + the family-count doc comment | 2 |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | One construction site moves to `from_connection` | 2 |
| `rs/crates/services/paigasus-iam/tests/authz_generations_redis.rs` | Docker-gated Redis behaviour | 4 |
| `ops/observability/grafana/dashboards/iam.json` | Dashboard panel | 5 |
| `ops/observability/prometheus/rules/iam.rules.yml` | `IamAuthzGenerationRewound` | 5 |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | promtool fixture | 5 |
| `docs/ops/RUNBOOK-observability.md` | Five superseded paragraphs + new alert entry | 6 |

---

### Task 1: The pure guard

The decision logic, with no Redis and no state — so it can be tested exhaustively in-process. Nothing calls it yet; Task 3 wires it in.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `const REWIND_JUMP: u64`, `const REPAIR_DELTA_CEILING: u64`, `enum GuardOutcome { Steady, Repair { delta: u64 }, Ceiling }`, `fn guard(observed: u64, high_water: u64) -> GuardOutcome`. All private to the module.

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `generation.rs`:

```rust
    /// Steady state: an observation at or beyond everything this process has seen is
    /// returned untouched. `observed == high_water` is the ordinary case (a counter that
    /// hasn't moved since the last read), and a fresh handle is `0/0`.
    #[test]
    fn guard_is_steady_when_the_observation_is_at_or_beyond_the_high_water_mark() {
        assert_eq!(guard(0, 0), GuardOutcome::Steady, "a fresh handle must not repair");
        assert_eq!(guard(7, 7), GuardOutcome::Steady);
        assert_eq!(guard(9, 7), GuardOutcome::Steady, "a counter that advanced is not a rewind");
    }

    /// The defect this whole design exists for: a counter that came back BELOW what this
    /// process already observed.
    #[test]
    fn guard_repairs_a_rewind_to_zero() {
        assert_eq!(guard(0, 7), GuardOutcome::Repair { delta: 7 + REWIND_JUMP });
    }

    /// A partial rewind — a failover to a replica holding an older value — is the same
    /// defect, not a special case.
    #[test]
    fn guard_repairs_a_partial_rewind_to_a_nonzero_lower_value() {
        assert_eq!(guard(3, 7), GuardOutcome::Repair { delta: 7 + REWIND_JUMP });
    }

    /// Design §3.4: the jump is deliberately LARGE. A `+1` repair lands a lagging replica
    /// inside a generation that may still hold live cache entries — worse than doing
    /// nothing. If someone "simplifies" this to `high_water + 1`, this test must fail.
    #[test]
    fn the_repair_delta_clears_the_high_water_mark_by_a_wide_margin() {
        let GuardOutcome::Repair { delta } = guard(0, 100) else {
            panic!("a rewind from 100 to 0 must repair");
        };
        assert!(
            delta >= 100 + 1_000_000,
            "SMA-474 §3.4: the repair must jump far past the high-water mark, not by 1 — got {delta}"
        );
    }

    /// Redis counters are i64 and `INCRBY` past `i64::MAX` errors. A high-water mark close
    /// to the ceiling must degrade to `Ceiling`, never produce a delta that would overflow
    /// or wrap.
    #[test]
    fn guard_reports_ceiling_rather_than_overflowing_the_i64_counter() {
        assert_eq!(guard(0, REPAIR_DELTA_CEILING), GuardOutcome::Ceiling);
        assert_eq!(guard(0, u64::MAX), GuardOutcome::Ceiling, "must saturate, not panic or wrap");
    }

    /// The boundary itself: one below the ceiling still repairs, so `Ceiling` is not
    /// triggered early.
    #[test]
    fn guard_still_repairs_just_below_the_ceiling() {
        let high_water = REPAIR_DELTA_CEILING - REWIND_JUMP;
        assert_eq!(guard(0, high_water), GuardOutcome::Repair { delta: REPAIR_DELTA_CEILING });
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::authz::generation
```

Expected: FAIL — `cannot find function guard in this scope`, `cannot find type GuardOutcome`.

- [ ] **Step 3: Write the implementation**

Add near the top of `generation.rs`, directly after the two `*_GEN_KEY` consts:

```rust
/// How far past the observing process's high-water mark a rewind repair jumps.
///
/// **Deliberately not `+1`.** A minimum jump is actively harmful (design §3.4): a replica
/// whose high-water lags the fleet repairs straight into a generation that still holds live
/// cache entries, replaying pre-change state that doing nothing would have avoided. The jump
/// has to clear every generation used within the longest cache TTL. One million generations
/// is one million tenancy mutations — over 16,000 per second sustained against the 60 s
/// `authz.slice_cache_ttl_secs` default.
const REWIND_JUMP: u64 = 1_000_000;

/// Ceiling on the repair delta. Redis counters are **i64**: `INCRBY` past `i64::MAX` returns
/// `ERR increment or decrement would overflow`, and a delta above `i64::MAX` is rejected as
/// out of range. Halved because the delta is ADDED to whatever Redis currently stores, which
/// is read in a separate round trip and so is not exactly the value the guard saw — the
/// halving is headroom for that stored value.
const REPAIR_DELTA_CEILING: u64 = (i64::MAX as u64) / 2;

/// What [`guard`] decided about one freshly-observed counter value.
#[derive(Debug, PartialEq, Eq)]
enum GuardOutcome {
    /// At or beyond everything this process has observed — return it unchanged. The steady
    /// state: one atomic compare, no extra round trip.
    Steady,
    /// A rewind. Repair with `INCRBY key delta`, which lands at `stored + delta`, hence at
    /// least `high_water + REWIND_JUMP` for any stored value (D5's invariant).
    Repair { delta: u64 },
    /// A rewind that cannot be repaired without overflowing Redis's i64 counter. Unreachable
    /// in practice (~10^10 rewind events); the RUNBOOK carries the manual remediation.
    Ceiling,
}

/// The monotonicity decision, as a pure function of two numbers — no connection, no state,
/// so it is exhaustively unit-testable. See [`GuardOutcome`] for what each arm means.
fn guard(observed: u64, high_water: u64) -> GuardOutcome {
    if observed >= high_water {
        return GuardOutcome::Steady;
    }
    match high_water.checked_add(REWIND_JUMP) {
        Some(delta) if delta <= REPAIR_DELTA_CEILING => GuardOutcome::Repair { delta },
        _ => GuardOutcome::Ceiling,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::authz::generation
```

Expected: PASS — the 6 new tests plus the 4 pre-existing memory-backend tests.

Note: `guard`, `GuardOutcome`, `REWIND_JUMP` and `REPAIR_DELTA_CEILING` are unused outside tests until Task 3. If `cargo clippy` (Task 3's lint step) flags dead code before then, that is expected; do not add `#[allow(dead_code)]` — Task 3 removes the condition.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs
git commit -F - <<'EOF'
feat(rs): pure monotonicity guard for the authz generation counters (SMA-474)

The decision logic for detecting a rewound Redis generation counter, as a
pure function of the observed value and the process-local high-water mark.
No connection and no state, so every branch is unit-testable in process.

REWIND_JUMP is deliberately 1_000_000 rather than 1. A minimum jump is
actively harmful per design section 3.4: a replica whose high-water lags the
fleet repairs straight into a generation that still holds live cache
entries, which is worse than not repairing at all.

Not wired into read/bump yet.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 2: State, plumbing, and metric registration

Everything the repair needs to exist, with **no behaviour change**. After this task the service compiles, every existing test passes, and no counter is ever emitted — Task 3 turns it on.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:323`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:23` and `:114`
- Modify: `rs/crates/services/paigasus-iam/src/main.rs:361` and `:380-383`

**Interfaces:**
- Consumes: `guard`, `GuardOutcome`, `REWIND_JUMP`, `REPAIR_DELTA_CEILING` (Task 1).
- Produces: `pub struct RedisGenerations` (all fields private), `Generations::Redis(RedisGenerations)`, `pub fn Generations::from_connection(conn: ConnectionManager) -> Generations`, private `enum Which { Policy, Entity }` with `key()`/`label()`/`memory()`/`redis()`, and `paigasus_observability::names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL`.

- [ ] **Step 1: Write the failing test**

Append to `generation.rs`'s test module:

```rust
    /// `Generations::Redis` carries per-counter state now, so it can no longer be built by
    /// wrapping a bare `ConnectionManager`. `from_connection` is the replacement, and it must
    /// keep the cheap-to-clone posture the type has always had — `AppState::new` shares ONE
    /// handle across every store, loader and cache.
    ///
    /// Uses a lazily-connecting manager (`127.0.0.1:1` is a closed port), so this never dials
    /// out and needs no Docker: construction and cloning touch no I/O.
    #[tokio::test]
    async fn from_connection_builds_a_redis_backend_that_is_cheap_to_clone() {
        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never actually dialed");
        let conn = ConnectionManager::new_lazy_with_config(client, crate::adapters::redis_conn::connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");

        let gens = Generations::from_connection(conn);
        let clone = gens.clone();

        assert!(matches!(gens, Generations::Redis(_)));
        assert!(matches!(clone, Generations::Redis(_)));
    }

    /// `Which` is what removed the duplicated per-counter plumbing. Pin the two mappings that
    /// a copy-paste slip would silently invert — a swapped key would make `policy_gen` and
    /// `entity_gen` share one Redis key, which no other test in this file would catch.
    #[test]
    fn which_maps_each_counter_to_its_own_redis_key_and_metric_label() {
        assert_eq!(Which::Policy.key(), POLICY_GEN_KEY);
        assert_eq!(Which::Entity.key(), ENTITY_GEN_KEY);
        assert_ne!(Which::Policy.key(), Which::Entity.key());

        assert_eq!(Which::Policy.label(), "policy_gen");
        assert_eq!(Which::Entity.label(), "entity_gen");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::authz::generation
```

Expected: FAIL — `no function or associated item named from_connection`, `cannot find type Which`.

- [ ] **Step 3: Write the implementation**

**3a.** In `generation.rs`, replace the imports block and add the new types. The `use` block becomes:

```rust
use async_trait::async_trait;
use metrics::counter;
use paigasus_iam_core::{AuthzError, PolicyGenBumper};
use paigasus_observability::names;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
#[cfg(test)]
use redis::aio::ConnectionManagerConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex as AsyncMutex;
```

(Drop the `#[cfg(test)] use redis::aio::ConnectionManagerConfig;` line if the test above compiles without it — `ConnectionManager::new_lazy_with_config` is reachable via the already-imported `ConnectionManager`.)

**3b.** Add `Which`, directly after `guard`:

```rust
/// Which of the two counters an operation is about. Replaces the pair of accessor closures
/// the read/bump helpers used to take: each counter now needs FOUR things (a Redis key, a
/// metric label, an in-process `AtomicU64`, and a Redis-side `CounterState`), and threading
/// four closures through would be worse than one dispatch enum.
#[derive(Clone, Copy)]
enum Which {
    Policy,
    Entity,
}

impl Which {
    fn key(self) -> &'static str {
        match self {
            Which::Policy => POLICY_GEN_KEY,
            Which::Entity => ENTITY_GEN_KEY,
        }
    }

    /// The `counter` label on [`names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL`].
    fn label(self) -> &'static str {
        match self {
            Which::Policy => "policy_gen",
            Which::Entity => "entity_gen",
        }
    }

    fn memory(self, mem: &MemoryGenerations) -> &Arc<AtomicU64> {
        match self {
            Which::Policy => &mem.policy_gen,
            Which::Entity => &mem.entity_gen,
        }
    }

    fn redis(self, redis: &RedisGenerations) -> &CounterState {
        match self {
            Which::Policy => &redis.policy,
            Which::Entity => &redis.entity,
        }
    }
}
```

**3c.** Add the Redis payload, directly after `MemoryGenerations`:

```rust
/// One Redis counter's process-local state (SMA-474).
///
/// `high_water` is the largest value this PROCESS has ever observed for the counter. It only
/// ever rises, and it is what makes a rewind detectable: Redis mapping a missing key to `0`
/// is indistinguishable from a genuine `0` without it.
///
/// `repair_gate` single-flights the repair. Every replica reads a generation on essentially
/// every authz decision, so at the instant of a rewind many in-flight requests observe it at
/// once; without the gate each would issue its own `INCRBY`. Mirrors
/// `PolicySnapshot::reload_gate`'s use of a `tokio::sync::Mutex` for the same herd.
#[derive(Clone, Default)]
struct CounterState {
    high_water: Arc<AtomicU64>,
    repair_gate: Arc<AsyncMutex<()>>,
}

/// The `redis` backend's payload: the shared connection plus per-counter rewind state.
///
/// `pub` for the same reason as [`MemoryGenerations`] — it is reachable through
/// `Generations::Redis`'s public tuple field (the `private_interfaces` lint) — with every
/// field private, so it stays unconstructible and unmatchable from outside this module.
/// Callers only ever get one via [`Generations::redis_connect`] or
/// [`Generations::from_connection`].
#[derive(Clone)]
pub struct RedisGenerations {
    conn: ConnectionManager,
    policy: CounterState,
    entity: CounterState,
}
```

**3d.** Change the variant and add the constructor. `Generations::Redis(ConnectionManager)` becomes `Generations::Redis(RedisGenerations)`, and:

```rust
    /// Wraps an ALREADY-CONNECTED `ConnectionManager`: `AppState::new` shares ONE Redis
    /// connection across the redis-backed `Generations` + `RedisDecisionCache` + `SliceCache`.
    /// Matches the `from_connection` entry point `SliceCache`/`RedisDecisionCache` already
    /// expose; [`Self::redis_connect`] stays the standalone-caller/test entry point.
    #[must_use]
    pub fn from_connection(conn: ConnectionManager) -> Self {
        Generations::Redis(RedisGenerations { conn, policy: CounterState::default(), entity: CounterState::default() })
    }
```

and `redis_connect`'s body becomes:

```rust
    pub async fn redis_connect(redis_url: &str) -> Result<Self, AuthzError> {
        let conn = crate::adapters::redis_conn::connect(redis_url).await.map_err(redis_err)?;
        Ok(Generations::from_connection(conn))
    }
```

**3e.** Re-point the four public accessors and the two helpers at `Which`. Behaviour is unchanged in this task — only the dispatch mechanism:

```rust
    pub async fn policy_gen(&self) -> Result<u64, AuthzError> {
        self.read(Which::Policy).await
    }

    pub async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        self.bump(Which::Policy).await
    }

    pub async fn entity_gen(&self) -> Result<u64, AuthzError> {
        self.read(Which::Entity).await
    }

    pub async fn bump_entity_gen(&self) -> Result<u64, AuthzError> {
        self.bump(Which::Entity).await
    }

    /// Shared read path: the in-process counter's current value, or Redis `GET` (a missing
    /// key — nothing has bumped it yet — reads as `0`, never an error).
    async fn read(&self, which: Which) -> Result<u64, AuthzError> {
        match self {
            Generations::Memory(mem) => Ok(which.memory(mem).load(Ordering::SeqCst)),
            Generations::Redis(redis) => {
                let mut conn = redis.conn.clone();
                let val: Option<u64> = conn.get(which.key()).await.map_err(redis_err)?;
                Ok(val.unwrap_or(0))
            }
        }
    }

    /// Shared bump path: an atomic in-process increment, or Redis `INCR` (which also
    /// initializes a missing key at `0` before incrementing — same effective semantics as
    /// the memory backend's default-0 start). Both return the value AFTER the bump.
    async fn bump(&self, which: Which) -> Result<u64, AuthzError> {
        match self {
            Generations::Memory(mem) => Ok(which.memory(mem).fetch_add(1, Ordering::SeqCst) + 1),
            Generations::Redis(redis) => {
                let mut conn = redis.conn.clone();
                let val: u64 = conn.incr(which.key(), 1_i64).await.map_err(redis_err)?;
                Ok(val)
            }
        }
    }
```

**3f.** `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:323` — replace

```rust
                (Generations::Redis(conn.clone()), Some(conn))
```

with

```rust
                (Generations::from_connection(conn.clone()), Some(conn))
```

**3g.** `rs/crates/libs/paigasus-observability/src/names.rs` — add the const immediately after `IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL` (line 23):

```rust
/// Rewinds of a Redis authz generation counter (`iam:authz:policy_gen`/`iam:authz:entity_gen`),
/// by `counter`, `outcome` (`repaired`/`repair_failed`/`ceiling`) and `reason`
/// (`missing`/`lower`). Non-zero means a generation key was lost — most often `allkeys-*`
/// eviction, which the RUNBOOK's `maxmemory-policy` mandate exists to prevent (SMA-474).
pub const IAM_AUTHZ_GENERATION_REWINDS_TOTAL: &str = "iam_authz_generation_rewinds_total";
```

and register it in `ALL`, immediately after `IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL` (line 114):

```rust
    IAM_AUTHZ_GENERATION_REWINDS_TOTAL,
```

**3h.** `rs/crates/services/paigasus-iam/src/main.rs` — add a `describe_counter!` immediately after the `IAM_AUTHZ_DECISIONS_TOTAL` block (ends line 383):

```rust
    describe_counter!(
        names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL,
        "Rewinds of a Redis authz generation counter, labeled by counter (policy_gen/entity_gen), outcome (repaired/repair_failed/ceiling) and reason (missing/lower)."
    );
```

and change the family count in the doc comment at line 361 from `24` to `25`:

```rust
/// Registers `# HELP`/`# TYPE` exposition text for the 25 metric families `paigasus-iam` emits
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -p paigasus-observability
```

Expected: PASS. This is the no-behaviour-change checkpoint — **every pre-existing test must still pass**, including `authz_generations_redis.rs` if Docker is available.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs \
        rs/crates/services/paigasus-iam/src/adapters/http/mod.rs \
        rs/crates/libs/paigasus-observability/src/names.rs \
        rs/crates/services/paigasus-iam/src/main.rs
git commit -F - <<'EOF'
refactor(rs): per-counter state on the redis Generations backend (SMA-474)

Carries the process-local high-water mark and the single-flight repair gate
each Redis generation counter needs, plus the metric family the repair will
emit. No behaviour change - read and bump still return exactly what Redis
says.

Generations::Redis now holds a RedisGenerations struct rather than a bare
ConnectionManager, so the one external construction site moves to a new
from_connection constructor, matching what SliceCache and RedisDecisionCache
already expose. A Which enum replaces the accessor closures the read and
bump helpers took, since each counter now needs four things rather than one.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 3: Wire the guard into `read` and `bump`

> **Amendment — this task's code below is superseded; the plan is left as the dated record.**
> Two things in the `repair`/`settle` sketch below did not survive implementation and review:
> the gate is taken with `try_lock`, not the blocking `state.repair_gate.lock().await` shown here
> (and the in-gate re-check comment's claim that a completed repair's result is "**necessarily**
> `>=` that same `delta`" is off by one against a concurrent `Steady` observation); and the
> `GuardOutcome::Ceiling` arm returns `high_water + REWIND_JUMP`, not `high_water`. Read
> `## 3a. Post-implementation amendments` in
> `docs/superpowers/specs/2026-08-06-sma-474-generation-counter-rewind-design.md` — §3a.1 and
> §3a.2 — for what shipped and why. The same section's §3a.3 adds the metric priming this task
> did not have.

The behaviour change. After this task a rewind is detected, repaired, counted and logged.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs`

**Interfaces:**
- Consumes: `guard`, `GuardOutcome`, `REWIND_JUMP` (Task 1); `Which`, `CounterState`, `RedisGenerations`, `names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL` (Task 2).
- Produces: `RedisGenerations::settle(&self, which: Which, observed: u64, reason: &'static str) -> u64` and `RedisGenerations::repair(&self, which: Which, delta: u64, reason: &'static str) -> u64`. Both **infallible** — they return `u64`, not `Result`.

- [ ] **Step 1: Write the failing test**

Append to `generation.rs`'s test module. These pin the two properties that do not need a live Redis; the Redis round-trip behaviour is Task 4.

```rust
    /// The memory backend must be completely untouched by SMA-474 — its counters are
    /// in-process `AtomicU64`s and cannot rewind, so it must never pay the guard's cost or
    /// emit the rewind metric. A regression here would mean `settle` leaked onto the memory
    /// arm of `read`/`bump`.
    #[tokio::test]
    async fn the_memory_backend_never_repairs_and_stays_strictly_incremental() {
        let gens = Generations::memory();

        for expected in 1..=5_u64 {
            assert_eq!(gens.bump_entity_gen().await.unwrap(), expected, "memory bumps must stay +1 exactly");
        }
        assert_eq!(gens.entity_gen().await.unwrap(), 5);
        assert_eq!(gens.policy_gen().await.unwrap(), 0, "entity_gen activity must not move policy_gen");
    }

    /// `settle` is infallible by design (D4): a failed repair must NEVER become an error,
    /// because every caller of `read` treats an error as "bypass the caches entirely", and
    /// the high-water mark never decreases — so one failed repair would make that bypass
    /// permanent. This drives `settle` against a manager pointed at a closed port, so the
    /// `INCRBY` inside `repair` is guaranteed to fail.
    ///
    /// The returned value must still be beyond the high-water mark: a disjoint local key
    /// space is safe, re-entering a used one is not.
    #[tokio::test]
    async fn a_failed_repair_falls_back_locally_instead_of_erroring() {
        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never reachable");
        let conn = ConnectionManager::new_lazy_with_config(client, crate::adapters::redis_conn::connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");
        let Generations::Redis(redis) = Generations::from_connection(conn) else {
            panic!("from_connection must build the Redis variant");
        };

        // Teach this process that the counter has been at 42, then hand it a rewound 0.
        redis.settle(Which::Entity, 42, "lower").await;
        let settled = redis.settle(Which::Entity, 0, "missing").await;

        assert!(
            settled >= 42 + REWIND_JUMP,
            "a failed repair must still return a value beyond everything this process observed, got {settled}"
        );
    }

    /// The fallback must be STABLE across repeated failures. If a failed repair raised the
    /// high-water mark to its own fallback value, the next call would compute
    /// `high_water + REWIND_JUMP` again and the delta would grow by a million per read,
    /// reaching the i64 ceiling in short order.
    #[tokio::test]
    async fn repeated_failed_repairs_do_not_ratchet_the_fallback_upward() {
        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never reachable");
        let conn = ConnectionManager::new_lazy_with_config(client, crate::adapters::redis_conn::connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");
        let Generations::Redis(redis) = Generations::from_connection(conn) else {
            panic!("from_connection must build the Redis variant");
        };

        redis.settle(Which::Entity, 42, "lower").await;

        let first = redis.settle(Which::Entity, 0, "missing").await;
        let second = redis.settle(Which::Entity, 0, "missing").await;
        let third = redis.settle(Which::Entity, 0, "missing").await;

        assert_eq!(first, second, "a failed repair must not move the high-water mark");
        assert_eq!(second, third);
    }

    /// A successful observation raises the mark, which is what makes the NEXT rewind
    /// detectable. Pure-in-process: `settle` on the steady-state path touches no I/O.
    #[tokio::test]
    async fn a_steady_observation_raises_the_high_water_mark() {
        let client = redis::Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never reachable");
        let conn = ConnectionManager::new_lazy_with_config(client, crate::adapters::redis_conn::connection_manager_config())
            .expect("lazy ConnectionManager construction never connects");
        let Generations::Redis(redis) = Generations::from_connection(conn) else {
            panic!("from_connection must build the Redis variant");
        };

        assert_eq!(redis.settle(Which::Policy, 3, "lower").await, 3, "a steady observation is returned unchanged");
        assert_eq!(redis.settle(Which::Policy, 9, "lower").await, 9);

        // Rewinding below 9 must now be detected — proving the mark rose to 9.
        let settled = redis.settle(Which::Policy, 8, "lower").await;
        assert!(settled >= 9 + REWIND_JUMP, "8 is below the mark of 9 and must be treated as a rewind, got {settled}");

        // ...and the OTHER counter's mark must be untouched.
        assert_eq!(redis.settle(Which::Entity, 1, "lower").await, 1, "policy_gen's mark must not leak into entity_gen");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib adapters::authz::generation
```

Expected: FAIL — `no method named settle found for struct RedisGenerations`. (`the_memory_backend_never_repairs_and_stays_strictly_incremental` passes already; that is intended — it is a regression guard, not a driver.)

- [ ] **Step 3: Write the implementation**

**3a.** Add the `settle`/`repair` pair as an `impl RedisGenerations` block, placed after the `RedisGenerations` struct definition:

```rust
impl RedisGenerations {
    /// Applies the monotonicity guard to one freshly-observed counter value and returns the
    /// value the caller should actually use (SMA-474 §3.2).
    ///
    /// **Infallible on purpose (D4).** A failed repair must never become an error: every
    /// caller of `Generations::read` treats an error as "bypass the caches entirely"
    /// (`CedarAuthorizer::cache_key`, `SliceCache::load`), and because the high-water mark
    /// never decreases, one failed repair would make that bypass PERMANENT — a raw Postgres
    /// entity-slice load per decision until the process restarts. On `policy_gen` it would
    /// cost more than speed: an error drives `load_and_compile` to a provisional stamp, and
    /// `reload_if_stale` then suppresses request-driven reloads entirely, costing
    /// same-decision revocation visibility. `Err` from `read`/`bump` therefore keeps its
    /// existing, narrower meaning — the Redis command itself failed.
    ///
    /// `reason` labels the metric only; it never affects the decision.
    async fn settle(&self, which: Which, observed: u64, reason: &'static str) -> u64 {
        let state = which.redis(self);
        let high_water = state.high_water.load(Ordering::SeqCst);
        match guard(observed, high_water) {
            GuardOutcome::Steady => {
                state.high_water.fetch_max(observed, Ordering::SeqCst);
                observed
            }
            GuardOutcome::Repair { delta } => self.repair(which, delta, reason).await,
            GuardOutcome::Ceiling => {
                counter!(names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL, "counter" => which.label(), "outcome" => "ceiling", "reason" => reason).increment(1);
                tracing::error!(
                    counter = which.label(),
                    "authz generation counter rewound but the repair would overflow redis's i64 counter — serving the high-water mark; \
                     flush iam:authz:slice:* and iam:authz:dec:*, then SET both generation keys to 0 (see the RUNBOOK)"
                );
                high_water
            }
        }
    }

    /// The repair itself: one atomic `INCRBY key delta`, single-flighted per counter.
    ///
    /// The gate is what stops a herd. Every replica reads a generation on essentially every
    /// authz decision, so at the instant of a rewind many in-flight tasks reach here at once;
    /// without it each would issue its own `INCRBY` and the counter would advance by
    /// `delta × in-flight` instead of `delta`.
    ///
    /// On failure the high-water mark is deliberately **left alone**. Raising it to the
    /// fallback would make the next call compute `high_water + REWIND_JUMP` all over again,
    /// growing the delta by a million per read and reaching [`REPAIR_DELTA_CEILING`] in short
    /// order. Leaving it stable means every subsequent call derives the SAME fallback — a
    /// stable, disjoint local key space — and retries the `INCRBY`, so the fleet re-converges
    /// the moment Redis accepts writes again.
    async fn repair(&self, which: Which, delta: u64, reason: &'static str) -> u64 {
        let state = which.redis(self);
        let _gate = state.repair_gate.lock().await;

        // Re-check under the gate. `delta` was computed as `high_water + REWIND_JUMP` from a
        // read taken BEFORE we queued; a repair that completed while we waited raises the mark
        // to its own `INCRBY` result, which is necessarily >= that same `delta`. So this
        // comparison is exactly "someone already repaired" — no extra round trip needed.
        let high_water = state.high_water.load(Ordering::SeqCst);
        if high_water >= delta {
            return high_water;
        }

        let mut conn = self.conn.clone();
        let repaired: Result<u64, redis::RedisError> = conn.incr(which.key(), delta as i64).await;
        match repaired {
            Ok(value) => {
                state.high_water.fetch_max(value, Ordering::SeqCst);
                counter!(names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL, "counter" => which.label(), "outcome" => "repaired", "reason" => reason).increment(1);
                tracing::warn!(
                    counter = which.label(),
                    reason,
                    "authz generation counter rewound — repaired forward in redis; check redis maxmemory-policy (must be volatile-*, never allkeys-*)"
                );
                value
            }
            Err(err) => {
                counter!(names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL, "counter" => which.label(), "outcome" => "repair_failed", "reason" => reason).increment(1);
                tracing::warn!(
                    counter = which.label(),
                    reason,
                    error_kind = ?err.kind(),
                    "authz generation counter rewound and the repair write failed — serving a process-local generation instead \
                     (disjoint key space, no cross-replica cache sharing until redis accepts writes again)"
                );
                delta
            }
        }
    }
}
```

**3b.** Wire `settle` into both Redis arms. In `read`:

```rust
            Generations::Redis(redis) => {
                let mut conn = redis.conn.clone();
                let observed: Option<u64> = conn.get(which.key()).await.map_err(redis_err)?;
                // A vanished key and a key that came back lower are different operator
                // stories: the first is eviction or data loss, the second a failover to a
                // replica holding an older value.
                let reason = if observed.is_none() { "missing" } else { "lower" };
                Ok(redis.settle(which, observed.unwrap_or(0), reason).await)
            }
```

In `bump`:

```rust
            Generations::Redis(redis) => {
                let mut conn = redis.conn.clone();
                let observed: u64 = conn.incr(which.key(), 1_i64).await.map_err(redis_err)?;
                // `INCR` initializes a missing key at 0 before incrementing, so a result of
                // exactly 1 means the key was absent — a heuristic, and only ever a metric
                // label, never part of the decision.
                let reason = if observed == 1 { "missing" } else { "lower" };
                Ok(redis.settle(which, observed, reason).await)
            }
```

**3c.** Update the two doc comments the change invalidates.

On `bump` (the "Both return the value AFTER the bump" line), append:

```rust
    /// **SMA-474:** on the `redis` backend the returned value is the bumped counter AFTER the
    /// monotonicity guard, so a bump that landed on a rewound key returns the repaired
    /// generation rather than `previous + 1`. `INCR` against a missing key returns `1`, which
    /// is precisely the re-entry the guard exists to prevent — which is why the guard is on
    /// this path and not only on `read`. The memory backend still returns `previous + 1`
    /// exactly; the two backends differ here by design. No caller reads the value.
```

On the module header, extend the `redis` bullet:

```rust
//! - **`redis`**: `INCR`/`GET` against the well-known keys `iam:authz:policy_gen`/
//!   `iam:authz:entity_gen` via an auto-reconnecting, `Arc`-backed `ConnectionManager` —
//!   cross-replica, survives restarts. Mirrors `adapters::oidc::redis_cache::RedisJwksCache`'s
//!   connect/clone-per-call pattern. Since SMA-474 it also carries a per-counter process-local
//!   high-water mark: neither key has a TTL, so an `allkeys-*` eviction (or a `FLUSHALL`, a
//!   restart without persistence, a failover to an empty replica) silently rewinds the counter
//!   and lets the fleet re-enter a cache key space that still holds live entries. A value below
//!   the mark is repaired forward with one atomic `INCRBY`. **NOTE this makes a "read" a
//!   potential Redis WRITE** on the rewind path — `INCRBY` is `denyoom` where `GET` is not, so
//!   under `maxmemory` pressure the repair can be rejected; that is why a failed repair falls
//!   back locally rather than erroring (design D4/§3.7).
```

Also update the `GenerationsReader` port doc at `cedar_authorizer.rs:106-115` to note that a read can now perform a write on the rewind path.

- [ ] **Step 4: Run tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: PASS, and clippy clean (the Task 1 dead-code condition is gone now).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs \
        rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs
git commit -F - <<'EOF'
fix(rs): detect and repair a rewound authz generation counter (SMA-474)

Neither generation key carries a TTL, so an allkeys-star eviction silently
rewinds the counter and lets the fleet re-enter a cache key space that still
holds live, pre-change entries. read and bump now run the monotonicity guard
against the process-local high-water mark and repair a rewind forward with
one atomic INCRBY, single-flighted per counter so a herd of in-flight
requests issues one write rather than one each.

The guard is on bump as well as read: INCR against a missing key returns 1,
which is exactly the dangerous re-entry.

A failed repair returns Ok with a process-local generation, never Err. The
high-water mark never decreases, so an Err would make the resulting cache
bypass permanent, and on policy_gen it would additionally suppress
request-driven snapshot reloads and cost same-decision revocation
visibility. Err keeps its existing narrower meaning.

Note this makes a read a potential Redis write on the rewind path. INCRBY is
denyoom where GET is not, so the repair can be rejected under memory
pressure - which is the same pressure that causes the eviction being
repaired, and the reason the fallback is local rather than an error.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 4: Docker-gated Redis integration tests

The properties that need a real Redis: the repair round-trips, it is *persisted* so other processes converge, and it holds on the bump path.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_generations_redis.rs`

**Interfaces:**
- Consumes: `Generations::redis_connect` (unchanged public API), the repair behaviour from Task 3.
- Produces: nothing consumed downstream.

**Critical:** the "another process observes it" test **must** build a second handle via a second `Generations::redis_connect(&url)`. A `.clone()` shares the same `Arc<AtomicU64>` high-water marks and would prove nothing.

- [ ] **Step 1: Write the failing tests**

Append to `authz_generations_redis.rs`. Add `use redis::AsyncCommands;` to the imports.

```rust
/// Deletes `key` on the test container — the cheapest faithful simulation of what an
/// `allkeys-*` eviction does to a generation key (neither key carries a TTL, so under memory
/// pressure they are ordinary eviction candidates).
async fn delete_key(url: &str, key: &str) {
    let client = redis::Client::open(url).expect("test container URL is well-formed");
    let mut conn = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    let _: () = conn.del(key).await.expect("DEL against the test container");
}

/// Reads `key` straight off the container, bypassing `Generations` entirely — so an assertion
/// about "what Redis actually holds" cannot be satisfied by process-local state.
async fn raw_get(url: &str, key: &str) -> Option<u64> {
    let client = redis::Client::open(url).expect("test container URL is well-formed");
    let mut conn = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    conn.get(key).await.expect("GET against the test container")
}

/// SMA-474's core property. Before the fix, a `DEL`ed counter read back as `0` — a successful
/// read of the wrong value — putting the fleet into a key space it had already used. It must
/// now come back BEYOND everything the process has observed.
#[tokio::test]
async fn a_deleted_entity_gen_key_reads_back_beyond_the_high_water_mark() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..5 {
        gens.bump_entity_gen().await.unwrap();
    }
    assert_eq!(gens.entity_gen().await.unwrap(), 5);

    delete_key(&url, "iam:authz:entity_gen").await;

    let after = gens.entity_gen().await.expect("a rewind must never surface as an error");
    assert!(after > 5, "a rewound counter must not read back as 0 or below the high-water mark, got {after}");
}

/// The repair has to be WRITTEN BACK, not just used locally — otherwise every other replica
/// keeps reading the rewound value and keeps writing into the old key space (design §3.3).
///
/// The second handle is a second `redis_connect`, NOT `gens.clone()`: a clone shares the same
/// `Arc<AtomicU64>` high-water marks, so it would report the repaired value from process-local
/// state even if nothing had been persisted. Same reason `authz_acceptance.rs` builds two
/// independent `AppState`s for its cross-replica test.
#[tokio::test]
async fn the_repair_is_persisted_so_an_independent_handle_converges() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..3 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;
    let repaired = gens.entity_gen().await.expect("the rewind is repaired, not an error");

    assert_eq!(raw_get(&url, "iam:authz:entity_gen").await, Some(repaired), "the repair must land in redis, not just in this process");

    let other_replica = Generations::redis_connect(&url).await.expect("a second, independent handle");
    assert_eq!(
        other_replica.entity_gen().await.unwrap(),
        repaired,
        "a handle with its own high-water mark must observe the persisted repair"
    );
}

/// The guard must be on the BUMP path too (design §3.2). `INCR` against a missing key returns
/// `1`, so a tenancy mutation landing right after an eviction would write its cache entries
/// into the gen-1 key space — where pre-mutation entries may still be live. **This is the test
/// that fails if the guard is only added to `read`.**
#[tokio::test]
async fn a_bump_immediately_after_a_rewind_cannot_re_enter_a_used_generation() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..4 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;

    let bumped = gens.bump_entity_gen().await.expect("a bump onto a rewound key must not error");
    assert!(bumped > 4, "a bump straight after a rewind must not return 1 — it would re-enter a used generation, got {bumped}");
}

/// The two counters have independent high-water marks and independent Redis keys, so
/// repairing one must not disturb the other.
#[tokio::test]
async fn repairing_one_counter_leaves_the_other_alone() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..3 {
        gens.bump_policy_gen().await.unwrap();
    }
    for _ in 0..2 {
        gens.bump_entity_gen().await.unwrap();
    }

    delete_key(&url, "iam:authz:entity_gen").await;
    let entity_after = gens.entity_gen().await.unwrap();
    assert!(entity_after > 2, "entity_gen must be repaired");

    assert_eq!(gens.policy_gen().await.unwrap(), 3, "repairing entity_gen must not move policy_gen");
    assert_eq!(raw_get(&url, "iam:authz:policy_gen").await, Some(3));
}

/// D4: a repair that Redis REJECTS must still return `Ok`, with a value beyond the high-water
/// mark, and must leave the Redis-side value alone.
///
/// `CONFIG SET maxmemory 1` is the fault injection: `INCRBY` is flagged `write denyoom` and is
/// rejected with `OOM command not allowed`, while `GET` is `readonly` and keeps succeeding.
/// That asymmetry is the same one `RUNBOOK-observability.md` documents for the pre-SMA-474
/// read path — it is what makes it possible to fail ONLY the repair.
#[tokio::test]
async fn a_repair_rejected_by_redis_falls_back_locally_instead_of_erroring() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..6 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;

    // Make every write fail, reads keep working.
    let client = redis::Client::open(url.as_str()).expect("test container URL is well-formed");
    let mut admin = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    let _: () = redis::cmd("CONFIG").arg("SET").arg("maxmemory").arg("1").query_async(&mut admin).await.expect("CONFIG SET maxmemory");

    let settled = gens.entity_gen().await.expect("a failed repair must fall back locally, never error (D4)");
    assert!(settled > 6, "the local fallback must still be beyond the high-water mark, got {settled}");
    assert_eq!(raw_get(&url, "iam:authz:entity_gen").await, None, "a rejected repair must not have written anything");

    // Restore, so the container is usable if this test is ever extended.
    let _: () = redis::cmd("CONFIG").arg("SET").arg("maxmemory").arg("0").query_async(&mut admin).await.expect("CONFIG SET maxmemory 0");
}
```

- [ ] **Step 2: Run tests to verify they fail against pre-fix code**

Mutation-test each one, matching the bar SMA-470 set. Stash nothing — use a scratch checkout of the pre-fix file:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs /tmp/gen-fixed.rs
git show HEAD~1:rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs > /tmp/gen-prefix.rs
```

Then, for the mutation proof, temporarily neuter the guard by making `settle` return `observed`
unchanged, run the five new tests, and confirm each FAILS:

```bash
cd rs && cargo nextest run -p paigasus-iam --test authz_generations_redis
```

Expected: all five new tests FAIL (the four repair tests see `0`/`1` where they demand a
repaired value; the `maxmemory` test sees an unchanged `0`). Restore the real implementation
afterwards and re-verify. **Record which assertion failed for each test** — a test that passes
against the neutered guard proves nothing and must be rewritten.

- [ ] **Step 3: No new implementation**

Task 3 already implements everything these tests exercise. If a test fails against the real
implementation, fix `generation.rs` — do not weaken the test.

- [ ] **Step 4: Run tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_generations_redis
```

Expected: PASS (all 4 pre-existing + 5 new). Docker must be running; without it the tests skip
with a note, which is **not** a pass — re-run with Docker before committing.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/tests/authz_generations_redis.rs
git commit -F - <<'EOF'
test(rs): cover generation-counter rewind repair against real redis (SMA-474)

Five Docker-gated tests: a deleted key reads back beyond the high-water mark
rather than as 0, the repair is persisted so an independently-connected
handle converges, a bump straight after a rewind cannot return 1 and
re-enter a used generation, repairing one counter leaves the other alone,
and a repair Redis rejects falls back locally instead of erroring.

The persistence test connects a second handle rather than cloning: a clone
shares the same high-water Arc and would report the repaired value from
process-local state even if nothing had been written.

The rejected-repair test uses CONFIG SET maxmemory 1, which fails INCRBY
(denyoom) while GET keeps succeeding (readonly) - the same asymmetry the
RUNBOOK documents, and what makes it possible to fail only the repair.

Every test was verified to fail against a neutered guard.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 5: Dashboard panel, alert rule, promtool fixture

**Files:**
- Modify: `ops/observability/grafana/dashboards/iam.json`
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`

**Interfaces:**
- Consumes: `names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL` registered in `ALL` (Task 2) — the `observability-drift` gate fails without it.
- Produces: alert `IamAuthzGenerationRewound` with labels `severity`, `counter`, `outcome`.

- [ ] **Step 1: Write the failing promtool fixture**

Append to `ops/observability/prometheus/rules/tests/iam.test.yml`:

```yaml
  # IamAuthzGenerationRewound (SMA-474): sum by (counter, outcome) (increase(...[15m])) > 0,
  # for: 5m.
  #
  # `sum by (counter, outcome)`, never a bare `sum()`. A bare sum has no label selector to
  # drop, so no added series can act as a control — every series folds into the same total and
  # necessarily fires. Grouping is what makes the flat `policy_gen` series below discriminate:
  # under the correct rule its group's `increase()` is 0 and stays silent while the moving
  # `entity_gen` group fires, so a `>= 0` mutant is caught by the flat group firing too.
  # Grouping also separates benign `repaired` from page-worthy `repair_failed` in the alert
  # itself.
  - interval: 1m
    input_series:
      # The control: flat at zero throughout. `increase()` over it is 0 for every window, so
      # only a `>= 0` mutant produces an alert for this group.
      - series: 'iam_authz_generation_rewinds_total{counter="policy_gen",outcome="repaired",reason="missing"}'
        values: '0+0x30'
      # The signal: nothing until t=10m, then a genuine rewind stream.
      - series: 'iam_authz_generation_rewinds_total{counter="entity_gen",outcome="repaired",reason="missing"}'
        values: '0+0x9 1+1x21'
    alert_rule_test:
      # Healthy window: nothing has moved anywhere. A `>= 0` mutant fires here on BOTH groups.
      - eval_time: 5m
        alertname: IamAuthzGenerationRewound
        exp_alerts: []
      # The signal has been true since ~t=11m, so `for: 5m` has elapsed. Exactly ONE alert —
      # the flat policy_gen group must still be silent, which is what pins `> 0` over `>= 0`.
      - eval_time: 20m
        alertname: IamAuthzGenerationRewound
        exp_alerts:
          - exp_labels: { severity: warning, counter: entity_gen, outcome: repaired }
            exp_annotations: { summary: "IAM authz generation counter rewound", description: "A Redis authz generation counter (iam:authz:policy_gen / iam:authz:entity_gen) came back lower than this process had already observed, and was repaired forward. Neither key carries a TTL, so the usual cause is allkeys-* eviction — check CONFIG GET maxmemory-policy, which must be volatile-*. A FLUSHALL, a restart without persistence, or a failover to an empty replica also produce this, and those are benign: they destroy the slice/decision caches along with the counter, leaving a cold cache rather than a stale one. Check whether iam:authz:slice:* and iam:authz:dec:* are also empty to tell them apart. outcome=repair_failed means Redis rejected the repair write (INCRBY is denyoom) and the replica is serving a process-local generation with no cross-replica cache sharing. See RUNBOOK's \"Authz availability posture\"." }

  # The `memory`-backend contract, pinned (SMA-474; same shape as the SMA-473 block above).
  # `authz.cache.backend = "memory"` uses in-process AtomicU64 counters that cannot rewind, so
  # this series is never emitted at all. `sum by (...)` over an empty vector is EMPTY, not 0,
  # so the `> 0` comparison has nothing to evaluate and the alert stays SILENT. Someone
  # "hardening" the rule with `or vector(0)` would turn every memory-backend deployment into a
  # permanent page. Evaluated at 25m, well past `for: 5m`.
  - interval: 1m
    input_series:
      - series: 'iam_authz_decisions_total{cache="hit",decision="allow"}'
        values: '0+5x25'
    alert_rule_test:
      - eval_time: 25m
        alertname: IamAuthzGenerationRewound
        exp_alerts: []
```

- [ ] **Step 2: Run promtool to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool test rules ops/observability/prometheus/rules/tests/iam.test.yml
```

Expected: FAIL — the fixture's `20m` case expects an alert from a rule that does not exist yet.

- [ ] **Step 3: Add the alert rule**

Append to the `iam.rules.yml` rule list, immediately after `IamAuthzRedisCacheBypassed`:

```yaml
      - alert: IamAuthzGenerationRewound
        # SMA-474. A Redis generation counter came back BELOW what a process had already
        # observed, and `Generations` repaired it forward. Neither `iam:authz:policy_gen` nor
        # `iam:authz:entity_gen` carries a TTL, so under `allkeys-*` they are ordinary eviction
        # candidates — and an evicted key reads back as `0`, which before SMA-474 was a
        # successful read of the wrong value that nothing detected.
        #
        # WARNING, not critical, and deliberately so: the mechanism self-heals, and three of
        # the four causes are BENIGN. A FLUSHALL, a restart without persistence, and a failover
        # to an empty replica all destroy the slice/decision caches along with the counter,
        # leaving a cold cache rather than a stale one (the generations, slice cache and
        # decision cache share ONE connection — `AppState::new`). Only SELECTIVE eviction is
        # hazardous, and this signal cannot tell them apart on its own; the RUNBOOK entry
        # carries the triage step. Do NOT reword the annotation to claim this is conclusive
        # evidence of `allkeys-*`.
        #
        # `sum by (counter, outcome)`, never a bare `sum()`: the grouping is what lets the
        # promtool fixture use a flat series as a control (a bare sum folds every series into
        # one total, so no control is possible), and it puts the counter and outcome on the
        # alert where triage needs them. `outcome="repair_failed"` is the one that matters
        # operationally — Redis rejected the repair write, so that replica is serving a
        # process-local generation with no cross-replica cache sharing.
        expr: sum by (counter, outcome) (increase(iam_authz_generation_rewinds_total[15m])) > 0
        for: 5m
        labels: { severity: warning }
        annotations: { summary: "IAM authz generation counter rewound", description: "A Redis authz generation counter (iam:authz:policy_gen / iam:authz:entity_gen) came back lower than this process had already observed, and was repaired forward. Neither key carries a TTL, so the usual cause is allkeys-* eviction — check CONFIG GET maxmemory-policy, which must be volatile-*. A FLUSHALL, a restart without persistence, or a failover to an empty replica also produce this, and those are benign: they destroy the slice/decision caches along with the counter, leaving a cold cache rather than a stale one. Check whether iam:authz:slice:* and iam:authz:dec:* are also empty to tell them apart. outcome=repair_failed means Redis rejected the repair write (INCRBY is denyoom) and the replica is serving a process-local generation with no cross-replica cache sharing. See RUNBOOK's \"Authz availability posture\"." }
```

- [ ] **Step 4: Add the dashboard panel and verify**

In `ops/observability/grafana/dashboards/iam.json`, append to the `panels` array (the highest
existing `id` is 18; the last panel sits at `y: 64, x: 0, w: 12`, so this fills that row):

```json
    {
      "id": 19,
      "type": "timeseries",
      "title": "Authz generation rewinds",
      "description": "iam_authz_generation_rewinds_total by counter/outcome — non-zero means a Redis generation key was lost (SMA-474); check CONFIG GET maxmemory-policy",
      "gridPos": { "h": 8, "w": 12, "x": 12, "y": 64 },
      "datasource": { "type": "prometheus", "uid": "prometheus" },
      "fieldConfig": { "defaults": { "unit": "ops" }, "overrides": [] },
      "targets": [
        {
          "refId": "A",
          "datasource": { "type": "prometheus", "uid": "prometheus" },
          "expr": "sum(rate(iam_authz_generation_rewinds_total[$__rate_interval])) by (counter, outcome)",
          "legendFormat": "{{counter}} {{outcome}}"
        }
      ]
    }
```

Then run both gates:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "import json; json.load(open('ops/observability/grafana/dashboards/iam.json')); print('dashboard JSON parses')"
promtool check rules ops/observability/prometheus/rules/*.rules.yml
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
cd rs && cargo nextest run -p paigasus-observability --test drift
```

Expected: all PASS. If drift fails, `IAM_AUTHZ_GENERATION_REWINDS_TOTAL` is missing from
`names::ALL` (Task 2 step 3g).

**Then prove the fixture discriminates:** temporarily change the rule's `> 0` to `>= 0`, re-run
`promtool test rules`, and confirm it FAILS on the `5m` case. Restore `> 0`.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add ops/observability/
git commit -F - <<'EOF'
feat(ci): alert and dashboard for authz generation-counter rewinds (SMA-474)

IamAuthzGenerationRewound at warning severity, plus a dashboard panel.

Grouped with sum by (counter, outcome) rather than a bare sum(). A bare sum
has no selector to drop, so no control series is possible - every series
folds into the same total and necessarily fires. The grouping is what lets
the fixture use a flat-at-zero policy_gen series to catch a >= 0 mutant
while the moving entity_gen series fires, and it puts the labels triage
needs on the alert.

Warning rather than critical because three of the four rewind causes are
benign: a FLUSHALL, a restart without persistence and a failover to an empty
replica all destroy the slice and decision caches along with the counter,
leaving a cold cache rather than a stale one. Only selective eviction is
hazardous, and the signal cannot tell them apart, so the annotation sends
the operator to a triage step rather than claiming a diagnosis.

Also pins the memory-backend silence contract, matching the SMA-473 block.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 6: RUNBOOK corrections and the full gate run

Five paragraphs in the RUNBOOK are now factually wrong. This is the task that satisfies AC6.

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md`

**Interfaces:**
- Consumes: everything from Tasks 1–5.
- Produces: nothing.

- [ ] **Step 1: Make the five edits plus the new alert entry**

**1 — metric catalog (§2.2 table).** Add a row after `iam_authz_policy_snapshot_reloads_total`:

```markdown
| `iam_authz_generation_rewinds_total` | counter | `counter`, `outcome`, `reason` | A Redis authz generation counter read back **below** what the process had already observed (SMA-474). `counter` ∈ `policy_gen`/`entity_gen`. `outcome` ∈ `repaired` (jumped forward with an atomic `INCRBY`, persisted so other replicas converge) / `repair_failed` (Redis rejected the write — `INCRBY` is `denyoom`, so `maxmemory` pressure does this; the replica falls back to a process-local generation, which is safe but stops cross-replica cache sharing) / `ceiling` (the repair would overflow Redis's i64 counter — see the remediation below). `reason` ∈ `missing` (the key was gone) / `lower` (it came back at a smaller value, e.g. a failover to a stale replica). **Only ever emitted on the `redis` backend** — the `memory` backend's in-process counters cannot rewind. |
```

**2 — the `maxmemory-policy` mandate paragraph.** It currently says `Generations::read` maps a
missing key to `0` so evicting one "**silently rewinds** that counter". Replace that sentence
with:

```markdown
`Generations::read` maps a missing key to `0`, so evicting one rewinds that counter. Since
SMA-474 this is no longer *silent*: each process keeps a high-water mark per counter, a value
below it is repaired forward with an atomic `INCRBY` (persisted, so other replicas converge),
and every occurrence increments `iam_authz_generation_rewinds_total` and fires
`IamAuthzGenerationRewound`. **The mandate still stands** — the repair reduces the exposure by
roughly six orders of magnitude but does not eliminate it (a replica that has not read the
counter in a very long time can still, in principle, repair into a live generation), and an
`allkeys-*` policy turns a routine memory-pressure event into an authz-freshness event for no
benefit. Verify with `CONFIG GET maxmemory-policy`.
```

**3 — the `IamAuthzRedisCacheBypassed` cause list (`:869-877`).** The claim that the generation
read is `readonly fast` and not `denyoom` is now conditional. Replace the sentence beginning
"The read behind `cache="bypass"` is a plain `GET`" with:

```markdown
The read behind `cache="bypass"` is normally a plain `GET` (`Generations::read`), which is
`readonly fast` and **not** `denyoom` — it keeps succeeding at `maxmemory` even under
`noeviction`, where it is the `INCR` that bumps the counter which gets `OOM command not
allowed`, and a failed bump is swallowed (see "Revocation freshness" below), never bypassed.
**Since SMA-474 there is one exception:** when the read detects a rewind it issues a repairing
`INCRBY`, which *is* `denyoom` — so under `maxmemory` pressure the repair can be rejected. That
does not bypass either: a rejected repair falls back to a process-local generation and is
counted as `iam_authz_generation_rewinds_total{outcome="repair_failed"}`. An *evicted* counter
is likewise a **missing** key, which no longer reads back as a silently wrong `0` — it is
detected and repaired, and `IamAuthzGenerationRewound` fires. Both are real failure modes, just
not this one; see the `maxmemory-policy` mandate below.
```

**4 — the remediation paragraph (`:912-919`).** Replace "an `allkeys-*` policy will have been
rewinding the counters silently the whole time" and "Nothing about the decision path needs
repair afterwards" with:

```markdown
If the cause is memory pressure rather than an outage, relieve it *and* check
`maxmemory-policy` per the mandate below — under `allkeys-*` the counters will have been
rewinding, which since SMA-474 shows up as `IamAuthzGenerationRewound` rather than passing
silently. Note that memory pressure also rejects the repairing `INCRBY` itself
(`outcome="repair_failed"`), so a fleet under sustained pressure loses cross-replica cache
sharing until it is relieved. Nothing about the decision path needs manual repair afterwards:
the caches repopulate on their own, the snapshot recovers on generation *inequality*, and a
rewound counter is repaired forward automatically. The one exception is
`iam_authz_generation_rewinds_total{outcome="ceiling"}` — a counter within a factor of two of
`i64::MAX`, which cannot be repaired further. Remediate by hand: `DEL` the
`iam:authz:slice:*` and `iam:authz:dec:*` key spaces, then `SET iam:authz:policy_gen 0` and
`SET iam:authz:entity_gen 0`.
```

**5 — the `entity_gen` bound paragraph.** Append to "This bound covers policy and role-grant
revocation only":

```markdown
Since SMA-474 that 90 s figure is the **residual** exposure after rewind repair, not the raw
one. The entity path did **not** get the policy path's content-addressed key, and could not:
`CompiledPolicies::content_hash` works because the compiled policy set is one global object
already in memory when the key is built, whereas an entity slice is per-`(resource, principal)`
and only exists *after* the Postgres load the slice cache exists to avoid — so hashing it to
derive the key would require performing that load on every lookup. See
`docs/superpowers/specs/2026-08-06-sma-474-generation-counter-rewind-design.md` D1. Eliminating
the window structurally rather than bounding it is SMA-475.
```

**6 — new alert entry.** Add a `### IamAuthzGenerationRewound — a Redis authz generation
counter rewound (warning)` section alongside the other alert entries, covering: **Meaning** (the
expression and what a rewind is), **Confirm** (`CONFIG GET maxmemory-policy`; break down by
`sum by (counter, outcome, reason)`), **Triage the benign vs. hazardous split** (check whether
`iam:authz:slice:*` and `iam:authz:dec:*` are also empty — if they are, this was whole-Redis
loss and the caches are cold, not stale), **Blast radius** (`repaired` self-heals;
`repair_failed` means no cross-replica cache sharing; `ceiling` needs the manual remediation
above), and **Remediation** (set `maxmemory-policy` to a `volatile-*` value).

- [ ] **Step 2: Verify no superseded claims remain**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "silently rewinds\|rewinding the counters silently\|readonly fast" docs/ops/RUNBOOK-observability.md
```

Expected: the only `readonly fast` hit is inside the rewritten paragraph from edit 3; **zero**
hits for the two "silently" phrases.

- [ ] **Step 3: Run the full CI gate set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. If moon reports an unattributed failure, diagnose with:

```bash
jq '.actions[] | select(.status=="failed")' .moon/cache/ciReport.json
```

- [ ] **Step 4: Confirm the design doc's acceptance criteria**

Re-read §9 of `docs/superpowers/specs/2026-08-06-sma-474-generation-counter-rewind-design.md`
and confirm each of AC1–AC6 against the actual diff. Any AC you cannot point at a change for is
a gap — fix it before committing.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add docs/ops/RUNBOOK-observability.md
git commit -F - <<'EOF'
docs(repo): correct the RUNBOOK paragraphs SMA-474 supersedes

Five paragraphs became wrong when the generation counters gained a rewind
guard.

The maxmemory-policy mandate said an evicted counter rewinds silently - it
no longer does, though the mandate itself stands, since the repair reduces
the exposure rather than eliminating it. The IamAuthzRedisCacheBypassed
cause list guaranteed the generation read is readonly and not denyoom, which
is now conditional: a read that detects a rewind issues a repairing INCRBY,
which can be rejected under memory pressure. The remediation paragraph
claimed nothing needs repair afterwards and that allkeys-star rewinds pass
silently. The entity_gen bound paragraph now records that 90s is the
residual exposure after repair, and why content-addressing did not transfer
to the entity path.

Adds the metric catalog row and the IamAuthzGenerationRewound alert entry,
including the triage step that separates a benign whole-Redis loss from a
hazardous selective eviction.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §2.1 D1 / §2.2 D2 (recorded decisions) | 6 (RUNBOOK edit 5) — the spec itself is already committed |
| §2.3 D3 (both counters) | 1–3 (`Which` covers both uniformly) |
| §2.4 D4 (local fallback, never `Err`) | 3 (`settle`/`repair` return `u64`), 3 & 4 tests |
| §2.5 D5 (`INCRBY`, i64, overshoot) | 1 (`REPAIR_DELTA_CEILING`), 3 (single-flight) |
| §3.1 (state, `from_connection`, lint) | 2 |
| §3.2 (pure guard, both paths) | 1, 3; bump path pinned by Task 4 |
| §3.3 (write-back) | 4 (`the_repair_is_persisted_...`) |
| §3.4 (large `JUMP`) | 1 (`the_repair_delta_clears_..._wide_margin`) |
| §3.5 (orphaned entries age out) | 6 (RUNBOOK) |
| §3.6 (guarantees) | 4 |
| §3.7 (`denyoom` exposure) | 3 (module doc), 4 (test), 6 (RUNBOOK edit 3) |
| §4 (metric, `ALL`, describe, panel, rule, fixture) | 2, 5 |
| §5 (five RUNBOOK edits) | 6 |
| §7.1 / §7.2 (tests) | 1, 3 / 4 |
| §9 AC1–AC6 | 6 step 4 |

No gaps.

**Placeholder scan:** every code step carries real code; no "TBD", no "add error handling", no "similar to Task N".

**Type consistency:** `guard(observed: u64, high_water: u64) -> GuardOutcome` (Task 1) is called only in `settle` (Task 3). `GuardOutcome::Repair { delta }` (Task 1) is destructured as `delta` in Task 3. `Which::{key,label,memory,redis}` (Task 2) are used with those exact names in Tasks 2 and 3. `settle(&self, which, observed, reason) -> u64` and `repair(&self, which, delta, reason) -> u64` are consistent between their definition and both call sites. `Generations::from_connection` is defined in Task 2 step 3d and used in 3f, plus Tasks 3 and 4 tests. `names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL` is spelled identically in Tasks 2, 3, 5 and 6.

**One adjacent gap found, deliberately NOT in scope.** `IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL` (SMA-470) is in `names::ALL` but has **no** `describe_counter!` in `describe_iam_metrics` — it ships with no `# HELP`/`# TYPE` text. That is a pre-existing SMA-470 oversight in the exact function Task 2 edits. Fixing it is a 4-line addition (and would make the doc-comment count 26, not 25), but it is not SMA-474's defect. Flagged for a separate decision rather than folded in silently.
