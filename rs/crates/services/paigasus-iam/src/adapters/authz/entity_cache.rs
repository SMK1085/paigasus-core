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
//! **Fail-open to the inner loader (D11):** `load` first calls the inner loader's own
//! `entity_gen()` to build the cache key. That call is **not guaranteed to be Redis-free**:
//! when `authz.cache.backend = redis`, the inner (`PgEntitySliceLoader`) loader's
//! `entity_gen()` delegates to the very same Redis-backed `Generations` handle this decorator
//! itself reads elsewhere in the crate — so a Redis outage can surface right here, before any
//! `GET`/`SET` in this file even runs. If `entity_gen()` errors, `load` treats it exactly like
//! a Redis `GET`/`SET` problem below: log and skip the redis slice cache ENTIRELY (no key can
//! be computed without a generation), falling straight through to `inner.load(..)` — which
//! for `PgEntitySliceLoader` only ever reads Postgres. Only once `entity_gen()` succeeds does
//! `load` attempt the Redis `GET` for the computed key; on a hit it deserializes and returns
//! straight from Redis, and on a miss OR any Redis problem (connect/I/O error, or a payload
//! that fails to deserialize), it likewise falls through to `inner.load(..)` and best-effort
//! caches that result (a `put`-time Redis error is logged and swallowed — it never turns a
//! successful inner load into a failure). A Redis outage therefore only ever costs the
//! accelerator; it can never fail a decision that the inner (Postgres) loader could otherwise
//! serve — a genuine `inner.load(..)` failure (e.g. Postgres down) still propagates, since
//! that is a real backend failure, not a cache-bypass case.

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
        // The inner loader's own generation read. Under `authz.cache.backend = redis` this is
        // ALSO a Redis round-trip (see module docs) — so an error here must fail OPEN, same
        // as the GET/SET below: skip the redis slice cache entirely (no key without a
        // generation) and fall straight through to `inner.load`, which for
        // `PgEntitySliceLoader` only ever reads Postgres. A genuine `inner.load` failure below
        // still propagates — that's a real backend failure, not a cache bypass (D11/D12).
        let entity_gen = match self.inner.entity_gen().await {
            Ok(entity_gen) => entity_gen,
            Err(err) => {
                log_entity_gen_bypass(&err);
                return self.inner.load(resource, principal).await;
            }
        };
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

/// Logs the inner loader's `entity_gen()` failure — Redis `ErrorKind` only when the
/// `AuthzError::Backend` wraps a `redis::RedisError` (the `authz.cache.backend = redis` case
/// this decorator cares about), `None` otherwise — never `Display`/message (same posture as
/// `log_get_bypass` et al.), then the fail-open mapping: skip the redis slice cache
/// altogether and bypass straight to the inner loader (D11/D12).
fn log_entity_gen_bypass(err: &AuthzError) {
    let kind = match err {
        AuthzError::Backend(source) => source.downcast_ref::<redis::RedisError>().map(redis::RedisError::kind),
        _ => None,
    };
    tracing::warn!(error_kind = ?kind, "entity generation counter unreadable — bypassing the redis slice cache entirely, falling through to the inner loader (fail-open, D11)");
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

    /// A fake `EntitySliceLoader` whose `entity_gen()` always fails (mirroring
    /// `PgEntitySliceLoader::entity_gen()` surfacing a Redis error when `authz.cache.backend =
    /// redis`, see module docs) but whose `load()` always succeeds (mirroring
    /// `PgEntitySliceLoader::load`, which never touches Redis).
    struct FailingGenLoader;

    #[async_trait]
    impl EntitySliceLoader for FailingGenLoader {
        async fn load(&self, _resource: &Prn, _principal: &Prn) -> Result<EntitySlice, AuthzError> {
            Ok(EntitySlice::default())
        }

        async fn entity_gen(&self) -> Result<u64, AuthzError> {
            Err(AuthzError::Backend(Box::new(redis::RedisError::from((redis::ErrorKind::Io, "simulated redis outage")))))
        }
    }

    /// D11/D12's core regression guard: when the inner loader's `entity_gen()` errors (a
    /// Redis-only outage under `authz.cache.backend = redis`), `SliceCache::load` must fail
    /// OPEN — returning the inner loader's `Ok` slice — never propagate the `entity_gen()`
    /// error. Uses a lazily-connecting `ConnectionManager` (never dials out) since the
    /// fail-open path returns before touching Redis at all, so this needs no live Redis
    /// server / Docker.
    #[tokio::test]
    async fn load_fails_open_to_the_inner_loader_when_entity_gen_errors() {
        let client = Client::open("redis://127.0.0.1:1").expect("well-formed redis URL, never actually dialed");
        let conn = ConnectionManager::new_lazy_with_config(client, redis::aio::ConnectionManagerConfig::new()).expect("lazy ConnectionManager construction never connects");

        let cache = SliceCache::from_connection(Arc::new(FailingGenLoader), conn, 60);
        let resource = prn("project", 1);
        let principal = prn("principal", 2);

        let slice = cache
            .load(&resource, &principal)
            .await
            .expect("an entity_gen()-only Redis failure must fail open to the inner (Postgres) loader, not error");

        assert_eq!(slice, EntitySlice::default());
    }
}
