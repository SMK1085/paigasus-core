// SPDX-License-Identifier: Apache-2.0

//! Redis-backed `JwksCache` (spec §4.3/D15): an external, shared cache so `CachedJwks`
//! survives process restarts and is visible across replicas. Redis owns the TTL directly
//! (`SET ... EX ttl_secs`) rather than this adapter tracking freshness itself — an expired
//! key simply disappears server-side. Every Redis failure (connect, I/O, or (de)serialize)
//! maps to `AuthnError::Unavailable`: a cache outage fails closed, distinguishable from a
//! token-validity error, but never surfaces cached values or connection details in logs —
//! only the issuer and the `redis::ErrorKind`/a static tag are logged.

use async_trait::async_trait;
use paigasus_iam_core::{AuthnError, Issuer};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};

use super::jwks::{CachedJwks, JwksCache};

/// Redis key prefix for cached JWKS entries (spec §4.3): `iam:jwks:<issuer canonical
/// string>`.
const KEY_PREFIX: &str = "iam:jwks:";

fn cache_key(issuer: &Issuer) -> String {
    format!("{KEY_PREFIX}{}", issuer.as_str())
}

/// `JwksCache` backed by Redis via an auto-reconnecting `ConnectionManager` (spec §4.3/D15).
/// `ConnectionManager` is cheap to clone (an `Arc`-wrapped multiplexed connection designed
/// for concurrent callers), so `get`/`put` clone it per call rather than holding a lock.
/// `connect` is the sole constructor — Task 10's composition root calls it verbatim.
pub struct RedisJwksCache {
    conn: ConnectionManager,
    ttl_secs: u64,
}

impl RedisJwksCache {
    /// Opens `redis_url` and wraps it in a `ConnectionManager`, which transparently
    /// reconnects in the background on transient connection loss (the in-flight command
    /// that observed the drop still surfaces its error to the caller — see `get`/`put`
    /// below). `ttl_secs` is applied to every `put` as Redis's own `EX` expiry.
    pub async fn connect(redis_url: &str, ttl_secs: u64) -> Result<Self, AuthnError> {
        let client = Client::open(redis_url).map_err(|err| log_unavailable(None, err.kind()))?;
        let conn = ConnectionManager::new(client).await.map_err(|err| log_unavailable(None, err.kind()))?;
        Ok(Self { conn, ttl_secs })
    }
}

#[async_trait]
impl JwksCache for RedisJwksCache {
    async fn get(&self, issuer: &Issuer) -> Result<Option<CachedJwks>, AuthnError> {
        let key = cache_key(issuer);
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(&key).await.map_err(|err| log_unavailable(Some(issuer), err.kind()))?;
        match raw {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| log_serde_unavailable(issuer)),
        }
    }

    async fn put(&self, issuer: &Issuer, jwks: CachedJwks) -> Result<(), AuthnError> {
        let key = cache_key(issuer);
        let payload = serde_json::to_vec(&jwks).map_err(|_| log_serde_unavailable(issuer))?;
        let mut conn = self.conn.clone();
        let _: () = conn.set_ex(&key, payload, self.ttl_secs).await.map_err(|err| log_unavailable(Some(issuer), err.kind()))?;
        Ok(())
    }
}

/// Logs the issuer (when available) and the Redis error's `ErrorKind` only — never the
/// error's `Display`/message, which can echo connection details — then maps to
/// `Unavailable` (spec §4.3/D15: fail closed, but distinguishable from token invalidity).
fn log_unavailable(issuer: Option<&Issuer>, kind: redis::ErrorKind) -> AuthnError {
    match issuer {
        Some(issuer) => tracing::warn!(issuer = %issuer, error_kind = ?kind, "redis jwks cache error"),
        None => tracing::warn!(error_kind = ?kind, "redis jwks cache connect error"),
    }
    AuthnError::Unavailable
}

/// Same fail-closed mapping as `log_unavailable`, for the `serde_json` (de)serialize path,
/// which has no `redis::ErrorKind` of its own.
fn log_serde_unavailable(issuer: &Issuer) -> AuthnError {
    tracing::warn!(issuer = %issuer, error_kind = "serde_json", "redis jwks cache error");
    AuthnError::Unavailable
}
