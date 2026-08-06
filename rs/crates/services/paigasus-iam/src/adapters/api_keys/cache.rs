// SPDX-License-Identifier: Apache-2.0

//! API-key introspection cache (spec §9/D5): a positive-validation cache keyed by `keyid`,
//! sitting in front of the Postgres source of truth on `AuthenticateApiKey`'s hot path (parse
//! token → `cache.get(keyid)` → hit re-verifies the secret + re-checks expiry + cached
//! `sa_status`, miss falls through to a DB validate + `cache.put`). [`CachedValidation`] carries
//! only what's needed to rebuild an `AuthnPrincipal`, re-check expiry, and re-verify the
//! presented secret on read: the resolved principal id, the SA status, the key expiry, and the
//! stored peppered HMAC hash (`key_hash`) — **never** the plaintext secret. Keying by `keyid`
//! (a non-secret token segment) is what makes eviction on revoke/archive work; caching the
//! `key_hash` alongside is what stops that same non-secret key from authenticating WITHOUT the
//! secret — the caller MUST `SecretHasher::verify` the presented secret against `key_hash` on
//! every hit (that check is the credential; the `keyid` is only the lookup key).
//!
//! Two implementations, mirroring `adapters::authz::decision_cache` exactly: [`MemoryApiKeyCache`]
//! (single-replica, TTL-bounded `Mutex<HashMap<..>>`) and [`RedisApiKeyCache`] (cross-replica,
//! `ConnectionManager`, same connect/clone-per-call pattern as `RedisDecisionCache`/`SliceCache`).
//!
//! **Both fail OPEN (D5):** this cache is a pure accelerator over the Postgres-backed
//! `ApiKeyRepository` — never the system of record — so a `get` that can't be served cleanly (a
//! Redis error, or a payload that fails to deserialize) is reported as a plain cache miss,
//! `None`, never an error; a `put`/`evict` that can't be written is logged and swallowed. A
//! Redis outage bypasses the accelerator; it must never fail (or falsely succeed) a validation.
//!
//! **Revocation-vs-cache honesty (challenge M5, spec §9):** `MemoryApiKeyCache` evicts only the
//! local replica's entries — it is single-replica/dev only; multi-replica (HA) deployments must
//! use [`RedisApiKeyCache`] so `RevokeApiKey`/`ArchiveServiceAccount` evictions are global. Even
//! on Redis, a put-after-evict race (a concurrent `resolve` that DB-validated *before* a revoke
//! and `put`s *after* the evict) can re-seed a positive entry for up to one TTL — hence the
//! short default TTL (30s). This cache never re-checks `expires_at` itself on a hit; that's the
//! caller's job against the cached value (expiry has no staleness problem — it's recomputed
//! fresh from `CachedValidation.expires_at` on every read).

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{ApiKeyId, PrincipalId, PrincipalStatus};
use paigasus_kernel::Prn;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::adapters::redis_conn::{RedisHandle, RedisRole};

/// Redis/in-proc key prefix (spec §9): `iam:apikey:<keyid>`.
const KEY_PREFIX: &str = "iam:apikey:";

fn cache_key(key_id: ApiKeyId) -> String {
    format!("{KEY_PREFIX}{key_id}")
}

/// Enough of a validated API key to rebuild an `AuthnPrincipal`, re-check expiry, AND
/// re-verify the presented secret on a cache hit (spec §9). Carries the resolved principal id,
/// the service account's current status, the key's own expiry, the key's tenancy `scope_prn`,
/// and `key_hash` — the SAME peppered HMAC-SHA-256 tag stored in Postgres
/// (`ApiKeyRepository::find_by_id`'s second return), NOT the plaintext secret and NOT a new
/// secret. Caching the stored hash is what lets `AuthenticateApiKey`'s hit path skip the two DB
/// round-trips (`find_by_id` + `find_principal`) while STILL constant-time-verifying the
/// presented secret against it on EVERY hit — a hit keyed by `key_id` alone (a non-secret token
/// segment) must never authenticate on the `key_id` without also proving possession of the
/// secret. A Redis leak of the hash can't validate keys without the operator's pepper (which
/// never leaves the process), so the hash is safe to cache. `scope_prn` (the key's
/// `TenancyNodeRef::canonical` PRN, a non-secret string) rides along for the SAME reason: so
/// introspection can surface the scope on a cache HIT with NO extra DB read (D11) — the gateway
/// authorizes an `InvokeModel` against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedValidation {
    pub principal_id: PrincipalId,
    pub sa_status: PrincipalStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub key_hash: Vec<u8>,
    pub scope_prn: String,
}

/// The Redis wire payload for [`CachedValidation`]. `PrincipalId`/`PrincipalStatus` are pure
/// domain value types with no `serde` impls (core-crate purity, ADR-0005), so they're carried
/// as their string forms (`PrincipalId::canonical()` / `PrincipalStatus::as_str()`) and
/// reconstructed on read; `expires_at` is `chrono::DateTime<Utc>`, which already round-trips
/// via the workspace's `chrono/serde` feature (see `adapters::oidc::jwks::CachedJwks`); the raw
/// `key_hash` bytes are carried as standard base64 (`chrono/serde` won't serialize a bare
/// `Vec<u8>` compactly, and base64 mirrors how `pg_api_keys.rs` already stores the hash as text).
#[derive(Serialize, Deserialize)]
struct WireValidation {
    principal_prn: String,
    sa_status: String,
    expires_at: Option<DateTime<Utc>>,
    key_hash: String,
    scope_prn: String,
}

impl From<&CachedValidation> for WireValidation {
    fn from(v: &CachedValidation) -> Self {
        WireValidation {
            principal_prn: v.principal_id.canonical(),
            sa_status: v.sa_status.as_str().to_string(),
            expires_at: v.expires_at,
            key_hash: STANDARD.encode(&v.key_hash),
            scope_prn: v.scope_prn.clone(),
        }
    }
}

impl TryFrom<WireValidation> for CachedValidation {
    type Error = ();

    fn try_from(w: WireValidation) -> Result<Self, Self::Error> {
        let prn = Prn::parse(&w.principal_prn).map_err(|_| ())?;
        let sa_status = PrincipalStatus::parse(&w.sa_status).ok_or(())?;
        let key_hash = STANDARD.decode(&w.key_hash).map_err(|_| ())?;
        Ok(CachedValidation {
            principal_id: PrincipalId::from_prn(prn),
            sa_status,
            expires_at: w.expires_at,
            key_hash,
            scope_prn: w.scope_prn,
        })
    }
}

/// Per-`keyid` positive-validation cache (spec §9/D5). `get`/`put` are infallible by design
/// (fail-open, see module docs) so `AuthenticateApiKey.resolve` never has to special-case a
/// cache backend failure.
#[async_trait]
pub trait ApiKeyValidationCache: Send + Sync {
    /// Looks up `key_id`. Fail-open: any backend error (Redis unreachable, payload that fails
    /// to deserialize) degrades to `None`, indistinguishable from "never cached" — the caller
    /// always falls through to a real DB validation on a miss.
    async fn get(&self, key_id: ApiKeyId) -> Option<CachedValidation>;

    /// Caches `v` for `key_id`. Fail-open: any backend error is logged and swallowed — a
    /// failed `put` never fails the validation that produced `v`.
    async fn put(&self, key_id: ApiKeyId, v: &CachedValidation);

    /// Evicts `key_id`, e.g. on `RevokeApiKey`/`ArchiveServiceAccount`. Fail-open: any backend
    /// error is logged and swallowed.
    async fn evict(&self, key_id: ApiKeyId);
}

/// In-process `ApiKeyValidationCache`: a TTL-bounded `Mutex<HashMap<..>>` — single-replica,
/// dev/single-node posture only (spec §9 challenge M5: `RevokeApiKey`/`ArchiveServiceAccount`
/// evictions here are local-replica only; HA deployments must use [`RedisApiKeyCache`] so
/// eviction is global).
pub struct MemoryApiKeyCache {
    ttl: Duration,
    entries: Mutex<HashMap<ApiKeyId, (CachedValidation, Instant)>>,
}

impl MemoryApiKeyCache {
    /// `ttl_secs` bounds how long a `put` entry stays servable before `get` treats it as a
    /// miss and drops it — the same freshness bound `RedisApiKeyCache` gets from Redis's own
    /// `EX` expiry.
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            entries: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ApiKeyValidationCache for MemoryApiKeyCache {
    async fn get(&self, key_id: ApiKeyId) -> Option<CachedValidation> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key_id) {
            Some((value, expires_at)) if *expires_at > Instant::now() => Some(value.clone()),
            Some(_) => {
                // Expired: drop it eagerly so a stale entry never leaks past its own TTL even
                // if nothing else evicts it first.
                entries.remove(&key_id);
                None
            }
            None => None,
        }
    }

    async fn put(&self, key_id: ApiKeyId, v: &CachedValidation) {
        let expires_at = Instant::now() + self.ttl;
        self.entries.lock().unwrap().insert(key_id, (v.clone(), expires_at));
    }

    async fn evict(&self, key_id: ApiKeyId) {
        self.entries.lock().unwrap().remove(&key_id);
    }
}

/// `ApiKeyValidationCache` backed by Redis via an auto-reconnecting `ConnectionManager` (spec
/// §9), mirroring `adapters::authz::decision_cache::RedisDecisionCache`. Cheap to clone the
/// connection per call — `ConnectionManager` is itself `Arc`-backed and designed for
/// concurrent callers.
///
/// **Fail-open (D5):** every error path (connect, I/O, or (de)serialize) on `get` returns
/// `None` — a plain miss — and every error on `put`/`evict` is logged and swallowed. The
/// caller always falls through to a real DB validation on a miss, so a Redis outage only ever
/// costs the accelerator, never a validation.
pub struct RedisApiKeyCache {
    conn: RedisHandle,
    ttl_secs: u64,
}

impl RedisApiKeyCache {
    /// Opens `redis_url` and wraps it in a `ConnectionManager`. `ttl_secs` is applied to every
    /// `put` as Redis's own `EX` expiry — this cache is a fail-open accelerator, so an entry
    /// disappearing after `ttl_secs` (or on eviction) never surfaces as anything other than a
    /// subsequent miss.
    pub async fn connect(redis_url: &str, ttl_secs: u64) -> Result<Self, redis::RedisError> {
        let conn = crate::adapters::redis_conn::connect(redis_url, RedisRole::ApiKeys).await?;
        Ok(Self { conn, ttl_secs })
    }

    /// Builds a cache over an ALREADY-CONNECTED handle: mirrors
    /// `RedisDecisionCache::from_connection`/`SliceCache::from_connection` (SMA-444 Task 21) —
    /// `AppState::new` shares ONE redis connection across the redis-backed `Generations` +
    /// `RedisDecisionCache` + `SliceCache` + this cache rather than each opening its own;
    /// `connect` above stays the standalone-caller/test entry point.
    ///
    /// `pub(crate)`, not `pub` (SMA-476 D13): `adapters::redis_conn` is a `pub(crate)` module, so
    /// a `pub fn` taking a `RedisHandle` would be a private-type-in-public-interface and
    /// `cargo clippy -- -D warnings` would fail the build. Every caller is in-crate.
    #[must_use]
    pub(crate) fn from_connection(conn: RedisHandle, ttl_secs: u64) -> Self {
        Self { conn, ttl_secs }
    }
}

#[async_trait]
impl ApiKeyValidationCache for RedisApiKeyCache {
    async fn get(&self, key_id: ApiKeyId) -> Option<CachedValidation> {
        let key = cache_key(key_id);
        let mut conn = self.conn.clone();
        let raw: Result<Option<Vec<u8>>, redis::RedisError> = conn.get(&key).await;
        match raw {
            Ok(Some(bytes)) => match serde_json::from_slice::<WireValidation>(&bytes).ok().and_then(|w| CachedValidation::try_from(w).ok()) {
                Some(v) => Some(v),
                None => {
                    log_deserialize_miss();
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                log_get_miss(err.kind());
                None
            }
        }
    }

    async fn put(&self, key_id: ApiKeyId, v: &CachedValidation) {
        let payload = match serde_json::to_vec(&WireValidation::from(v)) {
            Ok(payload) => payload,
            Err(_) => {
                log_serialize_swallow();
                return;
            }
        };
        let key = cache_key(key_id);
        let mut conn = self.conn.clone();
        let result: Result<(), redis::RedisError> = conn.set_ex(&key, payload, self.ttl_secs).await;
        if let Err(err) = result {
            log_put_swallow(err.kind());
        }
    }

    async fn evict(&self, key_id: ApiKeyId) {
        let key = cache_key(key_id);
        let mut conn = self.conn.clone();
        let result: Result<(), redis::RedisError> = conn.del(&key).await;
        if let Err(err) = result {
            log_evict_swallow(err.kind());
        }
    }
}

/// Logs the Redis error's `ErrorKind` only — never `Display`/message, which can echo
/// connection details (same posture as `oidc::redis_cache::log_unavailable` /
/// `decision_cache::log_get_miss`) — then the fail-open mapping: a get error degrades to a
/// plain miss (D5).
fn log_get_miss(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis api key cache get error — treating as a miss (fail-open, D5)");
}

fn log_deserialize_miss() {
    tracing::warn!(error_kind = "serde_json", "redis api key cache deserialize error — treating as a miss (fail-open, D5)");
}

fn log_put_swallow(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis api key cache put error — swallowed (fail-open, D5)");
}

fn log_serialize_swallow() {
    tracing::warn!(error_kind = "serde_json", "redis api key cache serialize error — swallowed (fail-open, D5)");
}

fn log_evict_swallow(kind: redis::ErrorKind) {
    tracing::warn!(error_kind = ?kind, "redis api key cache evict error — swallowed (fail-open, D5)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn pid() -> PrincipalId {
        let uuid = Uuid::from_u128(100);
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).expect("static test prn parts are valid"))
    }

    fn sample_validation() -> CachedValidation {
        CachedValidation {
            principal_id: pid(),
            sa_status: PrincipalStatus::Active,
            expires_at: None,
            key_hash: vec![0xDE, 0xAD, 0xBE, 0xEF],
            scope_prn: "prn:pgs:iam:::organization/00000000-0000-0000-0000-000000000064".to_string(),
        }
    }

    #[tokio::test]
    async fn memory_cache_put_get_evict() {
        let c = MemoryApiKeyCache::new(30);
        let id = ApiKeyId::from_uuid(Uuid::from_u128(9));
        assert!(c.get(id).await.is_none());
        c.put(id, &sample_validation()).await;
        assert!(c.get(id).await.is_some());
        c.evict(id).await;
        assert!(c.get(id).await.is_none());
    }

    #[tokio::test]
    async fn memory_cache_get_round_trips_the_full_value() {
        let c = MemoryApiKeyCache::new(30);
        let id = ApiKeyId::from_uuid(Uuid::from_u128(1));
        let v = CachedValidation {
            principal_id: pid(),
            sa_status: PrincipalStatus::Disabled,
            expires_at: Some(Utc::now()),
            key_hash: vec![1, 2, 3, 4, 5],
            scope_prn: "prn:pgs:iam:::organization/00000000-0000-0000-0000-0000000000c8".to_string(),
        };
        c.put(id, &v).await;
        assert_eq!(c.get(id).await, Some(v));
    }

    #[tokio::test]
    async fn memory_cache_entry_expires_after_its_ttl() {
        let c = MemoryApiKeyCache::new(0);
        let id = ApiKeyId::from_uuid(Uuid::from_u128(11));
        c.put(id, &sample_validation()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(c.get(id).await.is_none(), "an entry past its TTL must be treated as a miss");
    }

    #[tokio::test]
    async fn memory_cache_get_of_missing_key_is_none() {
        let c = MemoryApiKeyCache::new(30);
        assert!(c.get(ApiKeyId::from_uuid(Uuid::from_u128(404))).await.is_none());
    }

    #[test]
    fn wire_validation_round_trips_through_json() {
        let v = CachedValidation {
            principal_id: pid(),
            sa_status: PrincipalStatus::Active,
            expires_at: Some(Utc::now()),
            key_hash: vec![0x00, 0xFF, 0x10, 0x20, 0x30],
            scope_prn: "prn:pgs:iam:::organization/00000000-0000-0000-0000-0000000000ff".to_string(),
        };
        let bytes = serde_json::to_vec(&WireValidation::from(&v)).unwrap();
        let wire: WireValidation = serde_json::from_slice(&bytes).unwrap();
        let round_tripped = CachedValidation::try_from(wire).unwrap();
        assert_eq!(round_tripped, v, "the stored key_hash must survive the base64 wire round-trip byte-for-byte");
        assert_eq!(
            round_tripped.scope_prn, v.scope_prn,
            "the key's tenancy scope_prn must survive the JSON wire round-trip (D11: a cache hit returns it with no DB read)"
        );
    }

    /// D5's fail-open contract, exercised without any live Redis: a `get` against an
    /// unreachable backend degrades to `None`, and `put`/`evict` never panic. Uses the
    /// production `redis_conn::connection_manager_config()` — with a stock config this test
    /// took a measured **28.4 s** (three commands × a full ~9.5 s reconnect-retry cycle),
    /// which is the cost SMA-473 removed.
    #[tokio::test]
    async fn redis_cache_fails_open_when_the_backend_is_unreachable() {
        let conn = crate::adapters::redis_conn::new_lazy_for_tests("redis://127.0.0.1:1", RedisRole::ApiKeys).expect("well-formed redis URL, never actually reachable");
        let cache = RedisApiKeyCache::from_connection(conn, 30);
        let id = ApiKeyId::from_uuid(Uuid::from_u128(12));

        assert!(cache.get(id).await.is_none(), "an unreachable redis must degrade to a plain miss, not panic/error");
        cache.put(id, &sample_validation()).await;
        cache.evict(id).await;
    }

    /// SMA-476 AC3: fail-open (D5) is preserved under an open breaker.
    ///
    /// Pointed at a BLACKHOLE, not a closed port: a closed port refuses in microseconds, which
    /// looks identical to a short-circuit. Here a command that actually dialled would cost
    /// ~2.1 s, so the elapsed assertion proves the breaker short-circuited.
    #[tokio::test]
    async fn an_open_breaker_keeps_the_api_key_cache_failing_open() {
        let blackhole = crate::adapters::redis_conn::test_support::start().await;
        let conn = crate::adapters::redis_conn::with_open_breaker_for_tests(&blackhole.url, RedisRole::ApiKeys).expect("well-formed redis URL");
        let cache = RedisApiKeyCache::from_connection(conn, 30);
        let key_id = ApiKeyId::from_uuid(Uuid::from_u128(1));

        let started = std::time::Instant::now();
        let got = cache.get(key_id).await;
        cache.evict(key_id).await;
        let elapsed = started.elapsed();

        assert!(got.is_none(), "SMA-476 AC3: an open breaker must read as a plain MISS (fail-open, D5)");
        assert!(elapsed < std::time::Duration::from_millis(100), "took {elapsed:?} — the calls dialled instead of short-circuiting");
    }
}
