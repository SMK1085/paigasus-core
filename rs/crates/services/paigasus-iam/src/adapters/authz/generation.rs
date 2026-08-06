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
//!   behind a per-connection circuit breaker (SMA-476).

use async_trait::async_trait;
use paigasus_iam_core::{AuthzError, PolicyGenBumper};
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
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz)
            .expect("well-formed redis URL, never actually dialed");

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
    /// of) entity_gen instead. Nothing consumes `CounterState` yet (Task 3 wires the guard
    /// in); this only pins the plumbing this task adds: a fresh state starts at high-water 0
    /// with an unlocked repair gate.
    #[tokio::test]
    async fn which_redis_routes_each_counter_to_its_own_independent_counter_state() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::Authz)
            .expect("well-formed redis URL, never actually dialed");
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
}
