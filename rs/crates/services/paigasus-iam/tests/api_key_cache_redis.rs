// SPDX-License-Identifier: Apache-2.0

//! `RedisApiKeyCache` integration test (SMA-445 Task 14, spec §9/D5): put→get round-trips
//! through a real Redis, a missing key misses cleanly (`None`, not an error), `evict` makes a
//! previously-cached entry miss again, and a stopped container demonstrates the fail-open
//! contract — `get`/`put`/`evict` all degrade gracefully, never panicking or surfacing a Redis
//! error through the cache's infallible signatures.
//!
//! Runs against an ephemeral Redis in Docker. In CI (`CI` env set) a missing Docker daemon is a
//! HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same gating
//! pattern as `tests/authz_cache_redis.rs`/`tests/redis_jwks_cache.rs`.

use chrono::Utc;
use paigasus_iam::adapters::api_keys::{ApiKeyValidationCache, CachedValidation, MemoryApiKeyCache, RedisApiKeyCache};
use paigasus_iam_core::{ApiKeyId, PrincipalId, PrincipalStatus};
use paigasus_kernel::Prn;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-521).
#[path = "support/docker.rs"]
mod docker;

/// Starts an ephemeral Redis container, returning its connection URL. Same CI-hard-fail /
/// local-skip gating as `tests/authz_cache_redis.rs`; self-contained here since this file has
/// no other Redis consumer.
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    let node = match Redis::default().start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the api key cache redis test in CI: {e}");
            }
            eprintln!("skipping api_key_cache_redis: Docker unavailable ({e})");
            return None;
        }
    };

    let port = docker::mapped_port(&node, 6379, "redis").await;
    let url = format!("redis://127.0.0.1:{port}");
    Some((node, url))
}

fn pid(n: u128) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).expect("static test prn parts are valid"))
}

fn sample_validation() -> CachedValidation {
    CachedValidation {
        principal_id: pid(1),
        sa_status: PrincipalStatus::Active,
        expires_at: Some(Utc::now()),
        key_hash: vec![0xAB, 0xCD, 0xEF, 0x01, 0x23],
        scope_prn: "prn:pgs:iam:::organization/00000000-0000-0000-0000-000000000001".to_string(),
    }
}

#[tokio::test]
async fn api_key_cache_put_then_get_round_trips() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisApiKeyCache::connect(&url, 3600).await.expect("connect to redis");
    let id = ApiKeyId::from_uuid(Uuid::from_u128(2));
    let v = sample_validation();

    cache.put(id, &v).await;
    let got = cache.get(id).await.expect("entry present after put");

    assert_eq!(got, v, "the full CachedValidation must survive the round-trip byte-for-byte");
}

#[tokio::test]
async fn api_key_cache_get_of_missing_key_is_none() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisApiKeyCache::connect(&url, 3600).await.expect("connect to redis");

    let got = cache.get(ApiKeyId::from_uuid(Uuid::from_u128(3))).await;

    assert!(got.is_none());
}

#[tokio::test]
async fn api_key_cache_evict_makes_a_cached_entry_miss_again() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisApiKeyCache::connect(&url, 3600).await.expect("connect to redis");
    let id = ApiKeyId::from_uuid(Uuid::from_u128(4));

    cache.put(id, &sample_validation()).await;
    assert!(cache.get(id).await.is_some(), "entry present after put");

    cache.evict(id).await;
    assert!(cache.get(id).await.is_none(), "entry must miss again after evict");
}

/// D5's fail-open contract: once the Redis container is stopped, `get` must degrade to a plain
/// `None` (never a panic, never an `Err` surfaced through `ApiKeyValidationCache::get`'s
/// infallible signature) and `put`/`evict` must swallow the error silently.
#[tokio::test]
async fn api_key_cache_stopped_container_fails_open() {
    let Some((node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisApiKeyCache::connect(&url, 3600).await.expect("connect to redis");
    let id = ApiKeyId::from_uuid(Uuid::from_u128(5));

    node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let got = cache.get(id).await;
    assert!(got.is_none(), "a Redis outage must degrade to a plain miss, not a panic");

    // Must not panic even though the backing container is gone (fail-open on put/evict too).
    cache.put(id, &sample_validation()).await;
    cache.evict(id).await;
}

/// Sanity check that `MemoryApiKeyCache` (no Redis involved) is usable side-by-side with the
/// Redis-backed cache in this same test binary — guards against an accidental feature flag /
/// cfg mismatch hiding it from this crate's test build.
#[tokio::test]
async fn api_key_cache_memory_cache_is_available_in_this_crate() {
    let cache = MemoryApiKeyCache::new(30);
    let id = ApiKeyId::from_uuid(Uuid::from_u128(6));
    assert!(cache.get(id).await.is_none());
    cache.put(id, &sample_validation()).await;
    assert!(cache.get(id).await.is_some());
}
