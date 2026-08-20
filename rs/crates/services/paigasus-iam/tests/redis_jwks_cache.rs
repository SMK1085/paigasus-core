// SPDX-License-Identifier: Apache-2.0

//! `RedisJwksCache` integration test (SMA-443 Task 8, spec §4.3/D15): put/get round-trips
//! through a real Redis, an unknown issuer misses cleanly (`None`, not an error), and a
//! stopped container surfaces `AuthnError::Unavailable` rather than hanging or panicking.
//!
//! Runs against an ephemeral Redis in Docker. The Docker-unavailable policy lives once in
//! `tests/support/docker.rs` (SMA-538): a container failure with a reachable daemon is a hard
//! failure, an unreachable daemon skips locally and reds in CI.

use chrono::{SubsecRound, Utc};
use jsonwebtoken::jwk::JwkSet;
use paigasus_iam::adapters::oidc::jwks::{CachedJwks, JwksCache};
use paigasus_iam::adapters::oidc::redis_cache::RedisJwksCache;
use paigasus_iam_core::{AuthnError, Issuer};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;

// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-521).
#[path = "support/docker.rs"]
mod docker;

/// Starts an ephemeral Redis container, returning its connection URL. The skip-versus-fail
/// decision lives once, in `support/docker.rs` (SMA-538).
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    docker::start_redis_or_skip("redis_jwks_cache").await
}

fn sample_jwks(fetched_at: chrono::DateTime<Utc>) -> CachedJwks {
    let jwks: JwkSet = serde_json::from_value(serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "kid": "kid-a",
            "use": "sig",
            "alg": "RS256",
            "n": "test-modulus",
            "e": "AQAB",
        }]
    }))
    .expect("valid jwk set fixture");
    CachedJwks {
        jwks,
        jwks_uri: "https://idp.example.com/jwks".to_string(),
        fetched_at,
    }
}

#[tokio::test]
async fn put_then_get_round_trips() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisJwksCache::connect(&url, 3600).await.expect("connect to redis");
    let issuer = Issuer::parse("https://idp.example.com").unwrap();
    let jwks = sample_jwks(Utc::now().trunc_subsecs(6));

    cache.put(&issuer, jwks.clone()).await.unwrap();
    let got = cache.get(&issuer).await.unwrap().expect("entry present after put");

    assert_eq!(got.jwks, jwks.jwks, "the full JwkSet must survive the round-trip byte-for-byte");
    assert_eq!(got.jwks_uri, jwks.jwks_uri);
    assert_eq!(got.fetched_at, jwks.fetched_at);
}

#[tokio::test]
async fn get_of_unknown_issuer_is_none() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisJwksCache::connect(&url, 3600).await.expect("connect to redis");
    let issuer = Issuer::parse("https://never-cached.example.com").unwrap();

    let got = cache.get(&issuer).await.unwrap();

    assert!(got.is_none());
}

#[tokio::test]
async fn stopped_container_is_unavailable() {
    let Some((node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisJwksCache::connect(&url, 3600).await.expect("connect to redis");
    let issuer = Issuer::parse("https://idp.example.com").unwrap();

    node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let err = cache.get(&issuer).await.unwrap_err();

    assert!(matches!(err, AuthnError::Unavailable));
}
