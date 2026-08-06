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

/// The two authz generation counters (spec §7/D11), abstracted over an in-process
/// (`memory`) or Redis (`redis`) backend. Cheap to clone — every variant's payload is
/// `Arc`-backed — so one `Generations` can be shared across every store/loader/cache that
/// needs it (mirroring `DatabaseConnection`'s clone-a-handle posture elsewhere in this
/// crate).
#[derive(Clone)]
pub enum Generations {
    Memory(MemoryGenerations),
    Redis(RedisHandle),
}

impl Generations {
    /// In-process counters, both starting at 0. Single-replica only (spec §7).
    #[must_use]
    pub fn memory() -> Self {
        Generations::Memory(MemoryGenerations::default())
    }

    /// Opens `redis_url` and wraps it in an auto-reconnecting `RedisHandle` (mirrors
    /// `RedisJwksCache::connect`): cross-replica counters via `INCR`/`GET` on the two
    /// well-known keys.
    pub async fn redis_connect(redis_url: &str) -> Result<Self, AuthzError> {
        let conn = crate::adapters::redis_conn::connect(redis_url, RedisRole::Authz).await.map_err(redis_err)?;
        Ok(Generations::Redis(conn))
    }

    pub async fn policy_gen(&self) -> Result<u64, AuthzError> {
        self.read(POLICY_GEN_KEY, |m| &m.policy_gen).await
    }

    pub async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        self.bump(POLICY_GEN_KEY, |m| &m.policy_gen).await
    }

    pub async fn entity_gen(&self) -> Result<u64, AuthzError> {
        self.read(ENTITY_GEN_KEY, |m| &m.entity_gen).await
    }

    pub async fn bump_entity_gen(&self) -> Result<u64, AuthzError> {
        self.bump(ENTITY_GEN_KEY, |m| &m.entity_gen).await
    }

    /// Shared read path: the in-process counter's current value, or Redis `GET` (a missing
    /// key — nothing has bumped it yet — reads as `0`, never an error).
    async fn read(&self, key: &str, counter: impl FnOnce(&MemoryGenerations) -> &Arc<AtomicU64>) -> Result<u64, AuthzError> {
        match self {
            Generations::Memory(mem) => Ok(counter(mem).load(Ordering::SeqCst)),
            Generations::Redis(conn) => {
                let mut conn = conn.clone();
                let val: Option<u64> = conn.get(key).await.map_err(redis_err)?;
                Ok(val.unwrap_or(0))
            }
        }
    }

    /// Shared bump path: an atomic in-process increment, or Redis `INCR` (which also
    /// initializes a missing key at `0` before incrementing — same effective semantics as
    /// the memory backend's default-0 start). Both return the value AFTER the bump.
    async fn bump(&self, key: &str, counter: impl FnOnce(&MemoryGenerations) -> &Arc<AtomicU64>) -> Result<u64, AuthzError> {
        match self {
            Generations::Memory(mem) => Ok(counter(mem).fetch_add(1, Ordering::SeqCst) + 1),
            Generations::Redis(conn) => {
                let mut conn = conn.clone();
                let val: u64 = conn.incr(key, 1_i64).await.map_err(redis_err)?;
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
        let gens = Generations::Redis(conn);

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
}
