// SPDX-License-Identifier: Apache-2.0

//! `Generations`'s Redis backend integration test (SMA-444 Task 10): `policy_gen`/
//! `entity_gen` are independent `INCR`-backed counters that round-trip across two clones
//! sharing one `RedisHandle` (D11's cross-replica premise — a bump made through one
//! handle must be visible through another).
//!
//! Runs against an ephemeral Redis in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same
//! gating pattern as `tests/redis_jwks_cache.rs`.

use paigasus_iam::adapters::authz::Generations;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Starts an ephemeral Redis container, returning its connection URL. Same CI-hard-fail /
/// local-skip gating as `support::start_migrated_postgres`/`tests/redis_jwks_cache.rs`;
/// self-contained here since this file has no other Redis consumer.
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    let node = match Redis::default().start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the authz generations redis test in CI: {e}");
            }
            eprintln!("skipping authz_generations_redis: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    Some((node, url))
}

#[tokio::test]
async fn redis_bump_and_read_round_trip_across_two_clones_sharing_one_connection() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");
    let clone = gens.clone();

    // Nothing bumped yet: both counters read as 0 (a missing key, not an error).
    assert_eq!(gens.policy_gen().await.unwrap(), 0);
    assert_eq!(gens.entity_gen().await.unwrap(), 0);

    assert_eq!(gens.bump_policy_gen().await.unwrap(), 1);
    assert_eq!(gens.bump_policy_gen().await.unwrap(), 2);
    // The clone shares the same `iam:authz:policy_gen` Redis key — it must observe the
    // bumps made through the original handle.
    assert_eq!(clone.policy_gen().await.unwrap(), 2);

    // `entity_gen` is a distinct Redis key: independent of `policy_gen`'s bumps above.
    assert_eq!(clone.entity_gen().await.unwrap(), 0);
    assert_eq!(clone.bump_entity_gen().await.unwrap(), 1);
    assert_eq!(gens.entity_gen().await.unwrap(), 1);
    assert_eq!(gens.policy_gen().await.unwrap(), 2, "entity_gen bump must not affect policy_gen");
}
