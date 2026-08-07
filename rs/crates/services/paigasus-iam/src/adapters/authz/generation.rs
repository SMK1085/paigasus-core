// SPDX-License-Identifier: Apache-2.0

//! `Generations`: the two authz cache-invalidation counters (spec §7/D11) behind ONE
//! cheap-to-clone abstraction — `PgPolicyStore` (Task 10) and later `PgRoleGrantStore`
//! (Task 11)/`PgEntitySliceLoader` (Task 12) all share a single `Generations` handle rather
//! than duplicating the memory/redis backend split three times.
//!
//! - **`memory`**: two in-process `Arc<AtomicU64>` counters — single-replica, process
//!   lifetime only (a second process/replica sees its own independent counters).
//! - **`redis`**: `INCR`/`GET` against the well-known keys `iam:authz:policy_gen`/
//!   `iam:authz:entity_gen` via a breaker-wrapped, auto-reconnecting `RedisHandle` —
//!   cross-replica, survives restarts. Mirrors `adapters::oidc::redis_cache::RedisJwksCache`'s
//!   connect/clone-per-call pattern; the underlying `Arc`-backed `ConnectionManager` sits
//!   behind a per-connection circuit breaker (SMA-476). Since SMA-474 it also carries a
//!   per-counter process-local high-water mark: neither key has a TTL, so an `allkeys-*`
//!   eviction (or a `FLUSHALL`, a restart without persistence, a failover to an empty replica)
//!   silently rewinds the counter and lets the fleet re-enter a cache key space that still
//!   holds live entries. A value below the mark is repaired forward with one atomic `INCRBY`.
//!   **NOTE this makes a "read" a potential Redis WRITE** on the rewind path — `INCRBY` is
//!   `denyoom` where `GET` is not, so under `maxmemory` pressure the repair can be rejected
//!   (as can an open breaker, which short-circuits it); that is why a failed repair falls back
//!   locally rather than erroring (design D4/§3.7).

use async_trait::async_trait;
use metrics::counter;
use paigasus_iam_core::{AuthzError, PolicyGenBumper};
use paigasus_observability::names;
use redis::AsyncCommands;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex as AsyncMutex;

use crate::adapters::redis_conn::{RedisHandle, RedisRole};

const POLICY_GEN_KEY: &str = "iam:authz:policy_gen";
const ENTITY_GEN_KEY: &str = "iam:authz:entity_gen";

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
    /// A rewind that cannot be repaired without overflowing Redis's i64 counter. Redis is left
    /// alone and the caller gets a process-local generation instead (see [`RedisGenerations::
    /// settle`]).
    ///
    /// **Not "unreachable in practice".** An earlier draft argued the ceiling needed ~10^10
    /// rewind events because it modelled successive events as ADDING `N × (high_water + JUMP)`
    /// evaluated at `high_water ≈ 0`. Growth is geometric, not additive: the repair delta is a
    /// function of the mark, and the mark absorbs the previous event's result, so with `m`
    /// replicas racing one rewind the counter follows `H_{k+1} = m · (H_k + JUMP)`. At `m = 10`
    /// that reaches [`REPAIR_DELTA_CEILING`] in ~13 rewind events. Rare, but reachable — which
    /// is why this arm must return a value beyond the mark rather than the mark itself. The
    /// RUNBOOK carries the manual remediation (which needs a rolling restart, because the marks
    /// that got here are process-local).
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

    /// The `counter` label on [`paigasus_observability::names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL`].
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

/// The `memory` backend's payload: two independent counters, each `Arc`-shared so cloning
/// `Generations` is cheap and every clone observes the same counters. `pub` only because
/// it's reachable through `Generations::Memory`'s public tuple field (the
/// `private_interfaces` lint) — every field stays private, so this remains
/// unconstructible/unmatchable from outside the module; callers only ever get one via
/// [`Generations::memory`].
#[derive(Clone, Default)]
pub struct MemoryGenerations {
    policy_gen: Arc<AtomicU64>,
    entity_gen: Arc<AtomicU64>,
}

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
    conn: RedisHandle,
    policy: CounterState,
    entity: CounterState,
}

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
                    reason,
                    "authz generation counter rewound but the repair would overflow redis's i64 counter — serving a process-local \
                     generation past the high-water mark; SCAN+UNLINK the iam:authz:slice:* and iam:authz:dec:* namespaces \
                     (DEL does not expand globs), SET both generation keys to 0, \
                     then ROLL-RESTART the iam replicas to reset their process-local marks (see the RUNBOOK)"
                );
                // NOT `high_water`. The mark is a generation this process observed as LIVE, so
                // returning it re-enters a key space whose entries may still be inside
                // `slice_cache_ttl_secs` — precisely the defect SMA-474 exists to prevent, and it
                // would apply to EVERY read for the rest of the process's life. Jump past it
                // instead, exactly like the local fallback in `repair`. This value never reaches
                // Redis (the arm issues no command), so the i64 bound that produced the ceiling
                // does not apply to it; `saturating_add` keeps a mark near `u64::MAX` from
                // wrapping back into the used range. The arm does not raise the mark, so
                // recomputation is stable — every subsequent call derives the same generation.
                high_water.saturating_add(REWIND_JUMP)
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
        let Ok(_gate) = state.repair_gate.try_lock() else {
            // Another task on this replica is already repairing this counter. Queueing behind it
            // would serialize every generation read — and while a repair keeps FAILING the
            // re-check below can never short-circuit, so the queue never drains and the authz
            // hot path is capped at ~1/RTT. Give up immediately with the deterministic local
            // fallback instead: `delta` is `high_water + REWIND_JUMP`, the same value a failed
            // repair returns, so it is already beyond everything this process has observed and
            // is safe to key on. The cost is that this call may use a different generation than
            // the in-flight repair lands on — transient key-space fragmentation, which is
            // exactly what D4 already accepts as safe. Mirrors `PolicySnapshot::reload_if_stale`,
            // which `try_lock`s the same gate shape for the same reason.
            return delta;
        };

        // Re-check under the gate. `delta` was computed as `high_water + REWIND_JUMP` from a
        // read taken BEFORE we queued; in the common case a repair that completed while we
        // waited raises the mark to its own `INCRBY` result, which is at least that same
        // `delta` — so this comparison is usually exactly "someone already repaired", no extra
        // round trip needed. A concurrent `Steady` observation can slip in between another
        // task's `load` and its gate entry, so it computes a delta one larger and this
        // re-check misses by one; the resulting redundant `INCRBY` is harmless (relative, so it
        // only moves the counter further forward and can never re-enter a used key space).
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

/// Every `outcome` label value [`RedisGenerations::settle`]/[`RedisGenerations::repair`] can
/// emit, and every `reason` value `Generations::read`/`Generations::bump` can derive. Adding an
/// arm to either without extending these lists ships a series that only exists after it first
/// fires — the exact defect [`prime_rewind_metric`] exists to prevent.
const REWIND_OUTCOMES: [&str; 3] = ["repaired", "repair_failed", "ceiling"];
const REWIND_REASONS: [&str; 2] = ["missing", "lower"];

/// Registers all 12 `(counter, outcome, reason)` series of
/// [`names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL`] at zero, WITHOUT incrementing any of them.
///
/// **Why this is not decoration.** `metrics-rs` creates a labelled series on its first
/// `increment`, so without this the series does not exist until a rewind happens, and its first
/// exposed sample is already `1`. Prometheus `increase()` takes the first sample in the window as
/// its baseline, so a series that appears at `1` and stays at `1` yields `increase() = 0`
/// forever: `IamAuthzGenerationRewound` could never fire on a SINGLE rewind, and the Grafana
/// `rate()` panel stayed flat through it. Priming makes the exposition start at `0`, so the first
/// increment is a visible step — which is what design §3.6's "a rewind is no longer silent"
/// actually requires. `counter!` registers the handle on its own; dropping it unincremented is
/// the whole point.
///
/// **Redis path only.** It is called from [`Generations::from_connection`], never from
/// [`Generations::memory`]: the memory backend's counters are in-process `AtomicU64`s that cannot
/// rewind, and `ops/observability/prometheus/rules/tests/iam.test.yml` pins the
/// "series absent ⇒ alert silent" contract for it. Priming there would turn the closed label set
/// into 12 permanently-zero series on every single-replica deployment and destroy that contract.
fn prime_rewind_metric() {
    for which in [Which::Policy, Which::Entity] {
        for outcome in REWIND_OUTCOMES {
            for reason in REWIND_REASONS {
                let _registered = counter!(names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL, "counter" => which.label(), "outcome" => outcome, "reason" => reason);
            }
        }
    }
}

/// The two authz generation counters (spec §7/D11), abstracted over an in-process
/// (`memory`) or Redis (`redis`) backend. Cheap to clone — every variant's payload is
/// `Arc`-backed — so one `Generations` can be shared across every store/loader/cache that
/// needs it (mirroring `DatabaseConnection`'s clone-a-handle posture elsewhere in this
/// crate).
#[derive(Clone)]
pub enum Generations {
    Memory(MemoryGenerations),
    Redis(RedisGenerations),
}

impl Generations {
    /// In-process counters, both starting at 0. Single-replica only (spec §7).
    ///
    /// Deliberately does NOT call [`prime_rewind_metric`] — see that function's "Redis path
    /// only" note.
    #[must_use]
    pub fn memory() -> Self {
        Generations::Memory(MemoryGenerations::default())
    }

    /// Wraps an ALREADY-CONNECTED [`RedisHandle`]: `AppState::new` shares ONE Redis
    /// connection across the redis-backed `Generations` + `RedisDecisionCache` + `SliceCache`,
    /// so they also share one circuit breaker (SMA-476). Matches the `from_connection` entry
    /// point `SliceCache`/`RedisDecisionCache` already expose; [`Self::redis_connect`] stays
    /// the standalone-caller/test entry point.
    #[must_use]
    pub fn from_connection(conn: RedisHandle) -> Self {
        prime_rewind_metric();
        Generations::Redis(RedisGenerations {
            conn,
            policy: CounterState::default(),
            entity: CounterState::default(),
        })
    }

    /// Opens `redis_url` and wraps it in a breaker-wrapped, auto-reconnecting `RedisHandle`
    /// (mirrors `RedisJwksCache::connect`): cross-replica counters via `INCR`/`GET` on the two
    /// well-known keys.
    pub async fn redis_connect(redis_url: &str) -> Result<Self, AuthzError> {
        let conn = crate::adapters::redis_conn::connect(redis_url, RedisRole::Authz).await.map_err(redis_err)?;
        Ok(Generations::from_connection(conn))
    }

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
                let observed: Option<u64> = conn.get(which.key()).await.map_err(redis_err)?;
                // A vanished key and a key that came back lower are different operator
                // stories: the first is eviction or data loss, the second a failover to a
                // replica holding an older value.
                let reason = if observed.is_none() { "missing" } else { "lower" };
                Ok(redis.settle(which, observed.unwrap_or(0), reason).await)
            }
        }
    }

    /// Shared bump path: an atomic in-process increment, or Redis `INCR` (which also
    /// initializes a missing key at `0` before incrementing — same effective semantics as
    /// the memory backend's default-0 start). Both return the value AFTER the bump.
    ///
    /// **SMA-474:** on the `redis` backend the returned value is the bumped counter AFTER the
    /// monotonicity guard, so a bump that landed on a rewound key returns the repaired
    /// generation rather than `previous + 1`. `INCR` against a missing key returns `1`, which
    /// is precisely the re-entry the guard exists to prevent — which is why the guard is on
    /// this path and not only on `read`. The memory backend still returns `previous + 1`
    /// exactly; the two backends differ here by design. No caller reads the value.
    async fn bump(&self, which: Which) -> Result<u64, AuthzError> {
        match self {
            Generations::Memory(mem) => Ok(which.memory(mem).fetch_add(1, Ordering::SeqCst) + 1),
            Generations::Redis(redis) => {
                let mut conn = redis.conn.clone();
                let observed: u64 = conn.incr(which.key(), 1_i64).await.map_err(redis_err)?;
                // `INCR` initializes a missing key at 0 before incrementing, so a result of
                // exactly 1 means the key was absent — a heuristic, and only ever a metric
                // label, never part of the decision.
                let reason = if observed == 1 { "missing" } else { "lower" };
                Ok(redis.settle(which, observed, reason).await)
            }
        }
    }
}

fn redis_err(e: redis::RedisError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// The [`PolicyGenBumper`] port's implementation over a shared [`Generations`] handle
/// (SMA-446, Slice B): `application::roles::RoleService`'s injected, awaited, post-commit
/// side effect (the `grant`/`revoke` reference pattern B5–B7 copy). `bump` logs and swallows
/// its own error — lifted verbatim from the pre-Slice-B `PgRoleGrantStore::
/// bump_policy_gen_best_effort` (spec §7/D11): the triggering mutation has already
/// committed by the time this runs, so a Redis-down bump must never fail it; the change
/// instead lands on the policy snapshot's TTL backstop (`policy_cache_ttl_secs +
/// refresh_interval_secs`), whose reload rotates the decision cache's `content_hash` key
/// component with it (SMA-470 D4) — the decision cache never has to expire anything of its own
/// (and on the `memory` backend it cannot: `MemoryDecisionCache` has no TTL). Keeping this
/// adapter-side (rather than a
/// direct `Generations` field on `RoleService`) is what lets the application layer depend
/// only on the `PolicyGenBumper` port, never on `crate::adapters::authz::Generations`
/// (ADR-0005).
#[derive(Clone)]
pub struct GenerationsPolicyGenBumper {
    gens: Generations,
}

impl GenerationsPolicyGenBumper {
    #[must_use]
    pub fn new(gens: Generations) -> Self {
        GenerationsPolicyGenBumper { gens }
    }
}

#[async_trait]
impl PolicyGenBumper for GenerationsPolicyGenBumper {
    async fn bump(&self) {
        if let Err(err) = self.gens.bump_policy_gen().await {
            tracing::warn!(error = %err, "GenerationsPolicyGenBumper: policy_gen bump failed after a committed write — authz decisions may be stale until the policy snapshot's TTL backstop reloads");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_starts_at_zero() {
        let gens = Generations::memory();
        assert_eq!(gens.policy_gen().await.unwrap(), 0);
        assert_eq!(gens.entity_gen().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn memory_bump_increments_and_persists_across_clones() {
        let gens = Generations::memory();
        let clone = gens.clone();

        assert_eq!(gens.bump_policy_gen().await.unwrap(), 1);
        assert_eq!(gens.bump_policy_gen().await.unwrap(), 2);
        // A clone shares the same underlying `Arc<AtomicU64>` — it observes the bumps made
        // through the original handle.
        assert_eq!(clone.policy_gen().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn memory_policy_gen_and_entity_gen_are_independent_counters() {
        let gens = Generations::memory();

        assert_eq!(gens.bump_policy_gen().await.unwrap(), 1);
        assert_eq!(gens.bump_policy_gen().await.unwrap(), 2);
        // Bumping policy_gen twice must never move entity_gen.
        assert_eq!(gens.entity_gen().await.unwrap(), 0);

        assert_eq!(gens.bump_entity_gen().await.unwrap(), 1);
        assert_eq!(gens.policy_gen().await.unwrap(), 2, "entity_gen bump must not affect policy_gen");
    }

    #[tokio::test]
    async fn policy_gen_bumper_bumps_the_shared_generations_handle() {
        let gens = Generations::memory();
        let bumper = GenerationsPolicyGenBumper::new(gens.clone());

        bumper.bump().await;
        bumper.bump().await;

        assert_eq!(
            gens.policy_gen().await.unwrap(),
            2,
            "PolicyGenBumper::bump must drive the same counter RoleService reads through Generations"
        );
    }

    /// SMA-476 AC3, the fifth posture: `Generations::Redis` fails CLOSED like `RedisJwksCache`
    /// — unlike the four fail-open caches, a generation read/bump error PROPAGATES as
    /// `AuthzError::Backend`, because a swallowed-and-defaulted generation would silently widen
    /// the decision/slice cache key space and risk serving a stale decision (SMA-470 D4). An
    /// open breaker must not change that: it still propagates, just without dialling.
    ///
    /// Pointed at a BLACKHOLE, not a closed port: a closed port refuses in microseconds, which
    /// looks identical to a short-circuit. Here a command that actually dialled would cost
    /// ~2.1 s, so the elapsed assertion proves the breaker short-circuited.
    #[tokio::test]
    async fn an_open_breaker_keeps_redis_generations_propagating_the_error() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, RedisRole::Authz).expect("well-formed redis URL");
        // `from_connection` rather than `Generations::Redis(..)`: since SMA-474 the variant
        // carries per-counter rewind state alongside the handle, so it is no longer
        // constructible from a bare `RedisHandle`.
        let gens = Generations::from_connection(conn);

        let started = std::time::Instant::now();
        let result = gens.policy_gen().await;
        let elapsed = started.elapsed();

        assert!(
            matches!(result, Err(AuthzError::Backend(_))),
            "SMA-476 AC3: an open breaker must still PROPAGATE as AuthzError::Backend — Generations::Redis is not fail-open, got {result:?}"
        );
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the read dialled instead of short-circuiting");
    }

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
        assert!(delta >= 100 + 1_000_000, "SMA-474 §3.4: the repair must jump far past the high-water mark, not by 1 — got {delta}");
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

    /// `Generations::Redis` carries per-counter state now, so it can no longer be built by
    /// wrapping a bare `RedisHandle`. `from_connection` is the replacement, and it must keep
    /// the cheap-to-clone posture the type has always had — `AppState::new` shares ONE handle
    /// across every store, loader and cache (and, since SMA-476, one circuit breaker with it).
    ///
    /// Uses a lazily-connecting manager (`127.0.0.1:1` is a closed port), so this never dials
    /// out and needs no Docker: construction and cloning touch no I/O.
    #[tokio::test]
    async fn from_connection_builds_a_redis_backend_that_is_cheap_to_clone() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");

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

    /// `Which::redis` is the Redis-side twin of `Which::memory`, pinning that it routes each
    /// counter to its OWN `CounterState` rather than a shared one — a copy-paste slip here
    /// would make a policy_gen rewind repair silently gate on (or observe the high-water mark
    /// of) entity_gen instead. It also pins the starting condition every rewind assertion in
    /// this file depends on: a fresh handle's `CounterState` is high-water 0 with an unlocked
    /// repair gate, so nothing has been "observed" until a `settle` says so.
    #[tokio::test]
    async fn which_redis_routes_each_counter_to_its_own_independent_counter_state() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
        let gens = Generations::from_connection(conn);
        let Generations::Redis(redis) = &gens else {
            panic!("from_connection must build the Redis variant");
        };

        let policy = Which::Policy.redis(redis);
        let entity = Which::Entity.redis(redis);

        assert_eq!(policy.high_water.load(Ordering::SeqCst), 0, "a fresh handle has observed nothing yet");
        assert_eq!(entity.high_water.load(Ordering::SeqCst), 0);
        assert!(!Arc::ptr_eq(&policy.high_water, &entity.high_water), "policy_gen and entity_gen must not share high-water state");
        assert!(policy.repair_gate.try_lock().is_ok(), "a fresh repair gate must start unlocked");
    }

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
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
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
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
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
    /// detectable.
    ///
    /// The steady-state `settle` calls are pure in-process — no connection is dialed. The
    /// rewind call in the middle is not: it reaches `repair`, which issues a real `INCRBY`
    /// against the closed port `127.0.0.1:1` and fails, so what the assertion after it observes
    /// is the D4 local fallback. That is fine for what this test pins (the mark rose to 9), and
    /// it needs no Docker — a connection refusal is as deterministic as a success.
    #[tokio::test]
    async fn a_steady_observation_raises_the_high_water_mark() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
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

    /// The gate is taken with `try_lock`, not `lock`: a caller that finds a repair already in
    /// flight must return the deterministic local fallback immediately rather than queueing.
    /// While a repair keeps failing, the re-check inside the gate can never short-circuit, so
    /// a blocking `lock` would serialize every generation read behind one failing Redis round
    /// trip — a throughput ceiling on the authz hot path.
    ///
    /// Holding the gate for the duration is what makes this discriminate: against a blocking
    /// `lock` this test would hang rather than fail.
    #[tokio::test]
    async fn a_repair_that_cannot_take_the_gate_returns_the_local_fallback_without_queueing() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
        let Generations::Redis(redis) = Generations::from_connection(conn) else {
            panic!("from_connection must build the Redis variant");
        };

        // Teach this process a high-water mark, then hold the entity gate so the repair below
        // cannot take it.
        redis.settle(Which::Entity, 12, "lower").await;
        let held = Which::Entity.redis(&redis).repair_gate.clone();
        let _held_guard = held.lock().await;

        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), redis.settle(Which::Entity, 0, "missing"))
            .await
            .expect("a blocked gate must not make settle queue — try_lock, not lock");

        assert_eq!(settled, 12 + REWIND_JUMP, "a gate miss must return the deterministic local fallback");
    }

    /// The `Ceiling` arm must NOT serve the high-water mark. The mark is a generation this
    /// process observed as **live**, so returning it re-enters a key space whose entries may
    /// still be inside `slice_cache_ttl_secs` — the stale-`Allow` replay this whole change
    /// exists to prevent. And unlike a one-off, a saturated counter takes this arm on EVERY
    /// subsequent read, so the re-entry would be permanent for the life of the process.
    ///
    /// The arm is reachable, contrary to an earlier "~10^10 rewind events" estimate: the repair
    /// delta is a function of the mark and the mark absorbs the previous event's result, so
    /// overshoot compounds geometrically (`H_{k+1} = m · (H_k + REWIND_JUMP)` for `m` replicas
    /// racing one rewind) — ~13 events at `m = 10`.
    ///
    /// Needs no Docker: the arm issues no Redis command, so the closed port is never dialed.
    #[tokio::test]
    async fn a_rewind_at_the_ceiling_serves_a_generation_past_the_high_water_mark() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
        let Generations::Redis(redis) = Generations::from_connection(conn) else {
            panic!("from_connection must build the Redis variant");
        };

        // Drive the mark to the ceiling with an ordinary steady observation, then rewind under
        // it. The `guard` assertion is the fixture's own control: without it a change to
        // `REPAIR_DELTA_CEILING` could quietly move this test onto the `Repair` arm, where it
        // would keep passing while asserting nothing about `Ceiling`.
        let mark = REPAIR_DELTA_CEILING;
        assert_eq!(redis.settle(Which::Entity, mark, "lower").await, mark, "a steady observation is returned unchanged");
        assert_eq!(guard(0, mark), GuardOutcome::Ceiling, "this test must actually exercise the Ceiling arm");

        let settled = redis.settle(Which::Entity, 0, "missing").await;

        assert!(
            settled > mark,
            "the Ceiling arm must not replay a generation this process observed as live — got {settled} against a high-water mark of {mark}"
        );
        assert_eq!(
            settled,
            redis.settle(Which::Entity, 0, "missing").await,
            "the Ceiling arm does not raise the mark, so repeated calls must derive the SAME generation"
        );
    }

    /// A `metrics::Recorder` that only remembers which keys were REGISTERED (the values are
    /// irrelevant here — priming registers without incrementing).
    ///
    /// Installed thread-locally through `metrics::with_local_recorder`, deliberately NOT through
    /// `paigasus_observability::init`: that installs a PROCESS-GLOBAL recorder, so an
    /// "is this series absent?" assertion against it would be coupled to every other test in the
    /// binary that happens to build a redis-backed `Generations`. A local recorder makes the
    /// assertion below true or false on its own.
    #[derive(Default)]
    struct RegisteredKeys(std::sync::Mutex<Vec<metrics::Key>>);

    impl RegisteredKeys {
        /// Every registered rewind series, rendered `counter/outcome/reason` and sorted.
        fn rewind_series(&self) -> Vec<String> {
            let mut out: Vec<String> = self
                .0
                .lock()
                .expect("test-only mutex, never poisoned")
                .iter()
                .filter(|key| key.name() == names::IAM_AUTHZ_GENERATION_REWINDS_TOTAL)
                .map(|key| key.labels().map(|label| format!("{}={}", label.key(), label.value())).collect::<Vec<_>>().join(","))
                .collect();
            out.sort();
            out
        }
    }

    impl metrics::Recorder for RegisteredKeys {
        fn describe_counter(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
        fn describe_gauge(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
        fn describe_histogram(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}

        fn register_counter(&self, key: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
            self.0.lock().expect("test-only mutex, never poisoned").push(key.clone());
            metrics::Counter::noop()
        }

        fn register_gauge(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge {
            metrics::Gauge::noop()
        }

        fn register_histogram(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Histogram {
            metrics::Histogram::noop()
        }
    }

    /// SMA-474 final review (I2). `metrics-rs` creates a labelled series on its FIRST
    /// `increment`, so without priming `iam_authz_generation_rewinds_total` would not exist until
    /// a rewind happened and its first exposed sample would already be `1`. Prometheus
    /// `increase()` baselines on the first sample in the window, so a series that appears at `1`
    /// and stays at `1` yields `increase() = 0` forever: `IamAuthzGenerationRewound` could never
    /// fire on a SINGLE rewind. Priming is what makes "a rewind is no longer silent" true.
    ///
    /// The memory half is not a nicety. Its counters are in-process `AtomicU64`s that cannot
    /// rewind, and `ops/observability/prometheus/rules/tests/iam.test.yml` pins the
    /// "series absent ⇒ alert silent" contract for that backend — priming there would break it.
    #[tokio::test]
    async fn only_the_redis_backend_primes_the_rewind_metrics_whole_label_set() {
        let recorder = RegisteredKeys::default();

        metrics::with_local_recorder(&recorder, || {
            let _memory = Generations::memory();
        });
        assert!(
            recorder.rewind_series().is_empty(),
            "the memory backend must register NO rewind series — the alert's memory-backend silence contract depends on it, got {:?}",
            recorder.rewind_series()
        );

        // Built outside the closure: constructing the handle spawns onto the runtime, while
        // `from_connection` (the thing under test) is synchronous.
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz).expect("well-formed redis URL, never actually dialed");
        metrics::with_local_recorder(&recorder, || {
            let _redis = Generations::from_connection(conn);
        });

        let expected: Vec<String> = ["policy_gen", "entity_gen"]
            .iter()
            .flat_map(|c| {
                REWIND_OUTCOMES
                    .iter()
                    .flat_map(move |o| REWIND_REASONS.iter().map(move |r| format!("counter={c},outcome={o},reason={r}")))
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        assert_eq!(expected.len(), 12, "the label set is closed at 2 counters x 3 outcomes x 2 reasons");
        assert_eq!(recorder.rewind_series(), expected, "the redis backend must register every rewind series at zero from boot");
    }
}
