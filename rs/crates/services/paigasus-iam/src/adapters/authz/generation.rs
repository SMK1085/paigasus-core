// SPDX-License-Identifier: Apache-2.0

//! `Generations`: the two authz cache-invalidation counters (spec §7/D11) behind ONE
//! cheap-to-clone abstraction — `PgPolicyStore` (Task 10) and later `PgRoleGrantStore`
//! (Task 11)/`PgEntitySliceLoader` (Task 12) all share a single `Generations` handle rather
//! than duplicating the memory/redis backend split three times.
//!
//! - **`memory`**: two in-process `Arc<AtomicU64>` counters — single-replica, process
//!   lifetime only (a second process/replica sees its own independent counters).
//! - **`redis`**: `INCR`/`GET` against the well-known keys `iam:authz:policy_gen`/
//!   `iam:authz:entity_gen` via an auto-reconnecting, `Arc`-backed `ConnectionManager` —
//!   cross-replica, survives restarts. Mirrors `adapters::oidc::redis_cache::RedisJwksCache`'s
//!   connect/clone-per-call pattern.

use paigasus_iam_core::AuthzError;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const POLICY_GEN_KEY: &str = "iam:authz:policy_gen";
const ENTITY_GEN_KEY: &str = "iam:authz:entity_gen";

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
    Redis(ConnectionManager),
}

impl Generations {
    /// In-process counters, both starting at 0. Single-replica only (spec §7).
    #[must_use]
    pub fn memory() -> Self {
        Generations::Memory(MemoryGenerations::default())
    }

    /// Opens `redis_url` and wraps it in an auto-reconnecting `ConnectionManager` (mirrors
    /// `RedisJwksCache::connect`): cross-replica counters via `INCR`/`GET` on the two
    /// well-known keys.
    pub async fn redis_connect(redis_url: &str) -> Result<Self, AuthzError> {
        let client = Client::open(redis_url).map_err(redis_err)?;
        let conn = ConnectionManager::new(client).await.map_err(redis_err)?;
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
}
