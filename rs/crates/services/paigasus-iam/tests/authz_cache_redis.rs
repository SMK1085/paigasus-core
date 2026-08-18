// SPDX-License-Identifier: Apache-2.0

//! `RedisDecisionCache`/`SliceCache` integration test (SMA-444 Task 14, spec §7/D11-D12):
//! both round-trip through a real Redis, a missing key misses cleanly (`None`, not an
//! error), `SliceCache` only calls its inner loader on a cache miss (and a fresh miss again
//! once `entity_gen` bumps), and a stopped container demonstrates the fail-open contract —
//! `RedisDecisionCache::get` degrades to `None` and `SliceCache::load` falls through to the
//! inner loader, neither ever panicking or surfacing a Redis error from the decision path.
//!
//! Runs against an ephemeral Redis in Docker. The Docker-unavailable policy lives once in
//! `tests/support/docker.rs` (SMA-538): a container failure with a reachable daemon is a hard
//! failure, an unreachable daemon skips locally and reds in CI.

use async_trait::async_trait;
use paigasus_iam::adapters::authz::{MemoryDecisionCache, RedisDecisionCache, SliceCache, decision_key};
use paigasus_iam_core::authz::model::{ContextValue, EntitySlice, SliceEntity};
use paigasus_iam_core::{AccessRequest, Action, AuthzError, Decision, DecisionCache, Effect, EntitySliceLoader, RequestContext};
use paigasus_kernel::{Prn, to_cedar_uid};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;

// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-521).
#[path = "support/docker.rs"]
mod docker;

/// Starts an ephemeral Redis container, returning its connection URL. The skip-versus-fail
/// decision lives once, in `support/docker.rs` (SMA-538).
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    docker::start_redis_or_skip("authz_cache_redis").await
}

fn prn(resource_type: &str, n: u128) -> Prn {
    Prn::build("iam", "", None, resource_type, uuid::Uuid::from_u128(n)).expect("static test prn parts are valid")
}

fn sample_request() -> AccessRequest {
    AccessRequest {
        principal: prn("principal", 1),
        action: Action::GetProject,
        resource: prn("project", 2),
        context: RequestContext::empty(),
    }
}

fn sample_decision() -> Decision {
    Decision {
        effect: Effect::Allow,
        determining_policies: vec!["policy-1".to_string()],
    }
}

fn slice_for(resource: &Prn, principal: &Prn) -> EntitySlice {
    let resource_uid = to_cedar_uid(resource);
    let principal_uid = to_cedar_uid(principal);
    let mut attrs = BTreeMap::new();
    attrs.insert("kind".to_string(), ContextValue::Str("test-fixture".to_string()));
    EntitySlice {
        entities: vec![
            SliceEntity {
                uid: (resource_uid.entity_type, resource_uid.entity_id),
                parents: vec![],
                attrs: attrs.clone(),
            },
            SliceEntity {
                uid: (principal_uid.entity_type, principal_uid.entity_id),
                parents: vec![],
                attrs,
            },
        ],
    }
}

/// An in-memory `EntitySliceLoader` fake that counts how many times `load` is called (so
/// tests can assert `SliceCache` only reaches it on a genuine cache miss) and exposes a
/// mutable `entity_gen` (so tests can prove a gen bump changes `SliceCache`'s key).
struct FakeEntitySliceLoader {
    calls: AtomicUsize,
    entity_gen: AtomicU64,
}

impl FakeEntitySliceLoader {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entity_gen: AtomicU64::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn bump_gen(&self) {
        self.entity_gen.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl EntitySliceLoader for FakeEntitySliceLoader {
    async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(slice_for(resource, principal))
    }

    async fn entity_gen(&self) -> Result<u64, AuthzError> {
        Ok(self.entity_gen.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn authz_cache_decision_put_then_get_round_trips() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisDecisionCache::connect(&url, 3600).await.expect("connect to redis");
    let key = decision_key("content-a", 2, &sample_request());
    let decision = sample_decision();

    cache.put(&key, &decision).await;
    let got = cache.get(&key).await.expect("entry present after put");

    assert_eq!(got, decision, "the full Decision must survive the round-trip byte-for-byte");
}

#[tokio::test]
async fn authz_cache_decision_get_of_missing_key_is_none() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisDecisionCache::connect(&url, 3600).await.expect("connect to redis");

    let got = cache.get(&decision_key("content-a", 2, &sample_request())).await;

    assert!(got.is_none());
}

/// D12's fail-open contract: once the Redis container is stopped, `get` must degrade to a
/// plain `None` (never a panic, never an `Err` surfaced through `DecisionCache::get`'s
/// infallible signature) and `put` must swallow the error silently.
#[tokio::test]
async fn authz_cache_decision_stopped_container_fails_open() {
    let Some((node, url)) = start_redis().await else {
        return;
    };
    let cache = RedisDecisionCache::connect(&url, 3600).await.expect("connect to redis");
    let key = decision_key("content-a", 2, &sample_request());

    node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let got = cache.get(&key).await;
    assert!(got.is_none(), "a Redis outage must degrade to a plain miss, not a panic");

    // Must not panic even though the backing container is gone (fail-open on put too).
    cache.put(&key, &sample_decision()).await;
}

#[tokio::test]
async fn authz_cache_slice_first_load_misses_and_caches_second_load_hits() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let inner = Arc::new(FakeEntitySliceLoader::new());
    let cache = SliceCache::connect(inner.clone(), &url, 3600).await.expect("connect to redis");
    let resource = prn("project", 10);
    let principal = prn("principal", 20);

    let first = cache.load(&resource, &principal).await.expect("first load succeeds");
    assert_eq!(inner.call_count(), 1, "a cold cache must reach the inner loader exactly once");

    let second = cache.load(&resource, &principal).await.expect("second load succeeds");
    assert_eq!(inner.call_count(), 1, "a warm cache must NOT call the inner loader again");
    assert_eq!(first, second, "the cached slice must be identical to the freshly-loaded one");
}

#[tokio::test]
async fn authz_cache_slice_bumping_entity_gen_changes_the_key_so_it_misses_again() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let inner = Arc::new(FakeEntitySliceLoader::new());
    let cache = SliceCache::connect(inner.clone(), &url, 3600).await.expect("connect to redis");
    let resource = prn("project", 11);
    let principal = prn("principal", 21);

    cache.load(&resource, &principal).await.expect("first load succeeds");
    assert_eq!(inner.call_count(), 1);
    cache.load(&resource, &principal).await.expect("second load hits the cache");
    assert_eq!(inner.call_count(), 1, "second load must be a cache hit");

    inner.bump_gen();
    cache.load(&resource, &principal).await.expect("third load succeeds after gen bump");
    assert_eq!(inner.call_count(), 2, "a bumped entity_gen must mint a different key, forcing a fresh miss");
}

/// D11's fail-open contract: once the Redis container is stopped, `load` must fall through
/// to the inner (Postgres, in production) loader and still succeed — never propagate a
/// Redis error from the decision path.
#[tokio::test]
async fn authz_cache_slice_stopped_container_falls_through_to_inner_loader() {
    let Some((node, url)) = start_redis().await else {
        return;
    };
    let inner = Arc::new(FakeEntitySliceLoader::new());
    let cache = SliceCache::connect(inner.clone(), &url, 3600).await.expect("connect to redis");
    let resource = prn("project", 12);
    let principal = prn("principal", 22);

    node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let slice = cache.load(&resource, &principal).await.expect("must fail open to the inner loader, not error");

    assert_eq!(slice, slice_for(&resource, &principal));
    assert_eq!(inner.call_count(), 1, "a Redis outage must still reach the inner loader");
}

/// Sanity check that `MemoryDecisionCache` (no Redis involved) is usable side-by-side with
/// the Redis-backed caches in this same test binary — guards against an accidental feature
/// flag / cfg mismatch hiding it from this crate's test build.
#[tokio::test]
async fn authz_cache_memory_decision_cache_is_available_in_this_crate() {
    let cache = MemoryDecisionCache::new();
    let key = decision_key("content-a", 2, &sample_request());
    assert!(cache.get(&key).await.is_none());
    cache.put(&key, &sample_decision()).await;
    assert_eq!(cache.get(&key).await, Some(sample_decision()));
}
