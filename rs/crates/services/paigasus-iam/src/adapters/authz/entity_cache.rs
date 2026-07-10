// SPDX-License-Identifier: Apache-2.0

//! [`SliceCache`]: a Redis-backed [`EntitySliceLoader`] decorator (spec §7/D11) wrapping an
//! inner loader (typically the Postgres-backed `PgEntitySliceLoader`, Task 12) with a
//! Redis-cached fast path, keyed `iam:authz:slice:<entity_gen>:<resource-prn>:<principal-prn>`.
//!
//! **Why the principal is IN the key, not just the resource:** an [`EntitySlice`] is the
//! minimal set of Cedar entities needed to decide one `(principal, resource)` pair — it
//! embeds the PRINCIPAL entity (and its attributes/parents) alongside the resource's
//! ancestor chain. Keying the cache by `entity_gen:<resource-prn>` alone would serve
//! whichever principal's slice happened to be cached first back to every OTHER principal
//! asking about the same resource — a correctness bug (wrong principal entity/attributes
//! feeding the Cedar decision), not just a cache-efficiency one. The key therefore folds in
//! both PRNs.
//!
//! **Fail-open to the inner loader (D11):** `load` always calls the inner loader's own
//! `entity_gen()` first (not a Redis read), then tries a Redis `GET` for the computed key.
//! On a hit it deserializes and returns straight from Redis. On a miss OR any Redis problem
//! (connect/I/O error, or a payload that fails to deserialize), it falls through to
//! `inner.load(..)` and best-effort caches that result (a `put`-time Redis error is logged
//! and swallowed — it never turns a successful inner load into a failure). A Redis outage
//! therefore only ever costs the accelerator; it can never fail a decision that the inner
//! (Postgres) loader could otherwise serve.

use async_trait::async_trait;
use paigasus_iam_core::authz::model::EntitySlice;
use paigasus_iam_core::{AuthzError, EntitySliceLoader};
use paigasus_kernel::Prn;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use std::sync::Arc;

/// Redis key prefix (spec §7): `iam:authz:slice:<entity_gen>:<resource-prn>:<principal-prn>`.
const KEY_PREFIX: &str = "iam:authz:slice:";

/// The cache key for one `(resource, principal)` pair at a given `entity_gen`. Includes
/// BOTH PRNs (see the module docs) — omitting the principal would serve the wrong
/// principal's slice for the same resource.
fn slice_key(entity_gen: u64, resource: &Prn, principal: &Prn) -> String {
    format!("{KEY_PREFIX}{entity_gen}:{}:{}", resource.canonical(), principal.canonical())
}

/// Wraps an inner [`EntitySliceLoader`] with a Redis-cached fast path (spec §7/D11, see
/// module docs for the fail-open + key-shape rationale). `entity_gen` delegates straight to
/// the inner loader — this decorator never tracks its own generation state.
pub struct SliceCache {
    inner: Arc<dyn EntitySliceLoader>,
    conn: ConnectionManager,
    ttl_secs: u64,
}

impl SliceCache {
    /// Opens `redis_url` and wraps it in a `ConnectionManager` (mirrors
    /// `RedisJwksCache::connect`/`RedisDecisionCache::connect`). `ttl_secs` is applied to
    /// every cache write as Redis's own `EX` expiry.
    pub async fn connect(inner: Arc<dyn EntitySliceLoader>, redis_url: &str, ttl_secs: u64) -> Result<Self, AuthzError> {
        let client = Client::open(redis_url).map_err(redis_connect_err)?;
        let conn = ConnectionManager::new(client).await.map_err(redis_connect_err)?;
        Ok(Self { inner, conn, ttl_secs })
    }

    /// Wraps `inner` over an ALREADY-CONNECTED `ConnectionManager` (SMA-444 Task 21):
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` rather than each opening its own — `connect` above
    /// stays the standalone-caller/test entry point.
    pub(crate) fn from_connection(inner: Arc<dyn EntitySliceLoader>, conn: ConnectionManager, ttl_secs: u64) -> Self {
        Self { inner, conn, ttl_secs }
    }
}

#[async_trait]
impl EntitySliceLoader for SliceCache {
    async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError> {
        // The inner loader's own generation read — not a Redis round-trip. If this errors,
        // it's an inner-loader (e.g. Postgres) failure, not a cache problem, so it
        // propagates like any other inner-loader error would.
        let entity_gen = self.inner.entity_gen().await?;
        let key = slice_key(entity_gen, resource, principal);

        let mut conn = self.conn.clone();
        let cached: Result<Option<Vec<u8>>, redis::RedisError> = conn.get(&key).await;
        match cached {
            Ok(Some(bytes)) => match serde_json::from_slice::<EntitySlice>(&bytes) {
                Ok(slice) => return Ok(slice),
                Err(_) => log_deserialize_bypass(),
            },
            Ok(None) => {}
            Err(err) => log_get_bypass(err.kind()),
        }

        // Miss, or any Redis problem above: fall through to the inner (Postgres) loader —
        // fail-open (D11). A Redis outage never fails a decision, only bypasses the cache.
        let slice = self.inner.load(resource, principal).await?;

        match serde_json::to_vec(&slice) {
            Ok(payload) => {
                let mut conn = self.conn.clone();
                let result: Result<(), redis::RedisError> = conn.set_ex(&key, payload, self.ttl_secs).await;
                if let Err(err) = result {
                    log_put_swallow(err.kind());
                }
            }
            Err(_) => log_serialize_swallow(),
        }

        Ok(slice)
    }

    async fn entity_gen(&self) -> Result<u64, AuthzError> {
        self.inner.entity_gen().await
    }
}

fn redis_connect_err(e: redis::RedisError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// Logs the Redis error's `ErrorKind` only — never `Display`/message (same posture as
/// `oidc::redis_cache::log_unavailable` / `decision_cache::log_get_miss`) — then the
/// fail-open mapping: bypass to the inner loader (D11).
fn log_get_bypass(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis slice cache get error — bypassing to the inner loader (fail-open, D11)");
}

fn log_deserialize_bypass() {
    tracing::warn!(error_kind = "serde_json", "redis slice cache deserialize error — bypassing to the inner loader (fail-open, D11)");
}

fn log_put_swallow(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis slice cache put error — swallowed (fail-open, D11)");
}

fn log_serialize_swallow() {
    tracing::warn!(error_kind = "serde_json", "redis slice cache serialize error — swallowed (fail-open, D11)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn prn(resource_type: &str, n: u128) -> Prn {
        Prn::build("iam", "", None, resource_type, Uuid::from_u128(n)).expect("static test prn parts are valid")
    }

    #[test]
    fn slice_key_is_stable_for_identical_inputs() {
        let resource = prn("project", 1);
        let principal = prn("principal", 2);
        assert_eq!(slice_key(5, &resource, &principal), slice_key(5, &resource, &principal));
    }

    #[test]
    fn slice_key_changes_when_entity_gen_changes() {
        let resource = prn("project", 1);
        let principal = prn("principal", 2);
        assert_ne!(slice_key(5, &resource, &principal), slice_key(6, &resource, &principal));
    }

    #[test]
    fn slice_key_changes_when_resource_changes() {
        let principal = prn("principal", 2);
        assert_ne!(slice_key(5, &prn("project", 1), &principal), slice_key(5, &prn("project", 99), &principal));
    }

    /// The key MUST fold in the principal (module docs): two different principals asking
    /// about the SAME resource at the SAME entity_gen must get different keys, or one
    /// principal's cached slice would be served back to the other.
    #[test]
    fn slice_key_changes_when_principal_changes_even_for_the_same_resource_and_gen() {
        let resource = prn("project", 1);
        assert_ne!(slice_key(5, &resource, &prn("principal", 2)), slice_key(5, &resource, &prn("principal", 3)));
    }
}
