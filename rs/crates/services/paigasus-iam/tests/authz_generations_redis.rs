// SPDX-License-Identifier: Apache-2.0

//! `Generations`'s Redis backend integration test (SMA-444 Task 10): `policy_gen`/
//! `entity_gen` are independent `INCR`-backed counters that round-trip across two clones
//! sharing one `RedisHandle` (D11's cross-replica premise — a bump made through one
//! handle must be visible through another).
//!
//! SMA-474 (Task 4) adds the rewind-repair properties that only a real Redis can prove: a
//! `DEL`eted key (the eviction/`FLUSHALL`/empty-failover simulation) reads back beyond the
//! high-water mark instead of as `0`, the repair is persisted so an independently-connected
//! handle converges, a bump right after a rewind cannot re-enter a used generation, repairing
//! one counter leaves the other alone, a repair Redis rejects (`CONFIG SET maxmemory 1`) falls
//! back locally instead of erroring, and the emitted `iam_authz_generation_rewinds_total`
//! carries the exact label values Grafana/alerting hard-code — for BOTH outcomes a live Redis can
//! produce, `repaired` and `repair_failed`.
//!
//! Runs against an ephemeral Redis in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same
//! gating pattern as `tests/redis_jwks_cache.rs`.

use paigasus_iam::adapters::authz::Generations;
use redis::AsyncCommands;
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

/// Deletes `key` on the test container — the cheapest faithful simulation of what an
/// `allkeys-*` eviction does to a generation key (neither key carries a TTL, so under memory
/// pressure they are ordinary eviction candidates).
async fn delete_key(url: &str, key: &str) {
    let client = redis::Client::open(url).expect("test container URL is well-formed");
    let mut conn = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    let _: () = conn.del(key).await.expect("DEL against the test container");
}

/// Reads `key` straight off the container, bypassing `Generations` entirely — so an assertion
/// about "what Redis actually holds" cannot be satisfied by process-local state.
async fn raw_get(url: &str, key: &str) -> Option<u64> {
    let client = redis::Client::open(url).expect("test container URL is well-formed");
    let mut conn = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    conn.get(key).await.expect("GET against the test container")
}

/// SMA-474's core property. Before the fix, a `DEL`ed counter read back as `0` — a successful
/// read of the wrong value — putting the fleet into a key space it had already used. It must
/// now come back BEYOND everything the process has observed.
#[tokio::test]
async fn a_deleted_entity_gen_key_reads_back_beyond_the_high_water_mark() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..5 {
        gens.bump_entity_gen().await.unwrap();
    }
    assert_eq!(gens.entity_gen().await.unwrap(), 5);

    delete_key(&url, "iam:authz:entity_gen").await;

    let after = gens.entity_gen().await.expect("a rewind must never surface as an error");
    assert!(after > 5, "a rewound counter must not read back as 0 or below the high-water mark, got {after}");
}

/// The repair has to be WRITTEN BACK, not just used locally — otherwise every other replica
/// keeps reading the rewound value and keeps writing into the old key space (design §3.3).
///
/// The second handle is a second `redis_connect`, NOT `gens.clone()`: a clone shares the same
/// `Arc<AtomicU64>` high-water marks, so it would report the repaired value from process-local
/// state even if nothing had been persisted. Same reason `authz_acceptance.rs` builds two
/// independent `AppState`s for its cross-replica test.
#[tokio::test]
async fn the_repair_is_persisted_so_an_independent_handle_converges() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..3 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;
    let repaired = gens.entity_gen().await.expect("the rewind is repaired, not an error");

    assert_eq!(raw_get(&url, "iam:authz:entity_gen").await, Some(repaired), "the repair must land in redis, not just in this process");

    let other_replica = Generations::redis_connect(&url).await.expect("a second, independent handle");
    assert_eq!(
        other_replica.entity_gen().await.unwrap(),
        repaired,
        "a handle with its own high-water mark must observe the persisted repair"
    );
}

/// The guard must be on the BUMP path too (design §3.2). `INCR` against a missing key returns
/// `1`, so a tenancy mutation landing right after an eviction would write its cache entries
/// into the gen-1 key space — where pre-mutation entries may still be live. **This is the test
/// that fails if the guard is only added to `read`.**
#[tokio::test]
async fn a_bump_immediately_after_a_rewind_cannot_re_enter_a_used_generation() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..4 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;

    let bumped = gens.bump_entity_gen().await.expect("a bump onto a rewound key must not error");
    assert!(bumped > 4, "a bump straight after a rewind must not return 1 — it would re-enter a used generation, got {bumped}");
}

/// The two counters have independent high-water marks and independent Redis keys, so
/// repairing one must not disturb the other.
#[tokio::test]
async fn repairing_one_counter_leaves_the_other_alone() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..3 {
        gens.bump_policy_gen().await.unwrap();
    }
    for _ in 0..2 {
        gens.bump_entity_gen().await.unwrap();
    }

    delete_key(&url, "iam:authz:entity_gen").await;
    let entity_after = gens.entity_gen().await.unwrap();
    assert!(entity_after > 2, "entity_gen must be repaired");

    assert_eq!(gens.policy_gen().await.unwrap(), 3, "repairing entity_gen must not move policy_gen");
    assert_eq!(raw_get(&url, "iam:authz:policy_gen").await, Some(3));
}

/// The value of the `iam_authz_generation_rewinds_total` sample carrying every one of `labels`,
/// or `None` if no such sample was exposed.
///
/// A bare `contains()` on a label pair is **not** enough on this metric: since the final SMA-474
/// review the redis backend primes its whole closed label set at zero from boot
/// (`prime_rewind_metric`, so `increase()` can see the first rewind as a step from 0 rather than
/// a series that appears already at 1), which means every `outcome`/`reason` string is present in
/// the exposition whether or not anything was ever counted. Only the VALUE distinguishes
/// "recorded" from "primed".
fn rewind_sample(rendered: &str, labels: &[&str]) -> Option<f64> {
    rendered
        .lines()
        .filter(|line| line.starts_with("iam_authz_generation_rewinds_total{"))
        .find(|line| labels.iter().all(|label| line.contains(label)))
        .and_then(|line| line.rsplit_once(' '))
        .and_then(|(_, value)| value.trim().parse().ok())
}

/// D4: a repair that Redis REJECTS must still return `Ok`, with a value beyond the high-water
/// mark, and must leave the Redis-side value alone.
///
/// `CONFIG SET maxmemory 1` is the fault injection: `INCRBY` is flagged `write denyoom` and is
/// rejected with `OOM command not allowed`, while `GET` is `readonly` and keeps succeeding.
/// That asymmetry is the same one `RUNBOOK-observability.md` documents for the pre-SMA-474
/// read path — it is what makes it possible to fail ONLY the repair.
///
/// It also owns the ONLY coverage of `outcome="repair_failed"` (design §9 AC3). That string is
/// hard-coded into the `IamAuthzGenerationRewound` annotation, the metric catalog and the
/// blast-radius table in `docs/ops/RUNBOOK-observability.md`, and it is the operationally
/// important outcome — the replica is serving a process-local generation with no cross-replica
/// cache sharing — so a typo on the emit side would ship exactly the silent gap the alert exists
/// to close. This is the only test in the suite that can produce it: it needs a Redis that
/// accepts `GET` and rejects `INCRBY`.
#[tokio::test]
async fn a_repair_rejected_by_redis_falls_back_locally_instead_of_erroring() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    // `init` is a `get_or_init` over a process-global recorder, so calling it here is safe even
    // though the sibling metric test in this binary calls it too. It must run BEFORE
    // `redis_connect`, or the priming registrations would land on the no-op recorder.
    let handle = paigasus_observability::init("test-authz-generations-redis-repair-failed-metric");
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..6 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;

    // Make every write fail, reads keep working.
    let client = redis::Client::open(url.as_str()).expect("test container URL is well-formed");
    let mut admin = client.get_multiplexed_async_connection().await.expect("connect to the test container");
    let _: () = redis::cmd("CONFIG").arg("SET").arg("maxmemory").arg("1").query_async(&mut admin).await.expect("CONFIG SET maxmemory");

    let settled = gens.entity_gen().await.expect("a failed repair must fall back locally, never error (D4)");
    assert!(settled > 6, "the local fallback must still be beyond the high-water mark, got {settled}");
    assert_eq!(raw_get(&url, "iam:authz:entity_gen").await, None, "a rejected repair must not have written anything");

    // ...and it must be COUNTED as `repair_failed`, with the labels the alert annotation and the
    // RUNBOOK's blast-radius table hard-code (design §9 AC3).
    let out = handle.render();
    let failed = rewind_sample(&out, &[r#"counter="entity_gen""#, r#"outcome="repair_failed""#, r#"reason="missing""#])
        .unwrap_or_else(|| panic!("expected an iam_authz_generation_rewinds_total sample for the rejected repair:\n{out}"));
    assert!(failed >= 1.0, "a rejected repair must be counted as outcome=\"repair_failed\", got {failed}:\n{out}");

    // The control. Only `entity_gen` rewound here, so its twin must still read zero — that is
    // what makes the `counter` label above load-bearing rather than incidental, since a
    // hard-coded or swapped label would light up both. `policy_gen`/`repair_failed` is chosen
    // deliberately: no other test in this binary can record it, so the control does not depend on
    // this file's tests each getting their own process.
    let other = rewind_sample(&out, &[r#"counter="policy_gen""#, r#"outcome="repair_failed""#]).expect("priming exposes the whole label set at zero on the redis backend");
    assert_eq!(other, 0.0, "only entity_gen rewound — policy_gen must not be counted, got {other}:\n{out}");

    // Restore, so the container is usable if this test is ever extended.
    let _: () = redis::cmd("CONFIG").arg("SET").arg("maxmemory").arg("0").query_async(&mut admin).await.expect("CONFIG SET maxmemory 0");
}

/// SMA-474 Task 3 review: nothing pinned the emitted metric's LABEL VALUES, and Tasks 5/6
/// hard-code the same strings into a Grafana panel and a Prometheus alert rule — a typo on
/// either side ships a permanently-empty panel and an alert that never fires. This drives a
/// real repair against the Docker Redis and asserts the exact label set a rewind-and-repair
/// emits on `iam_authz_generation_rewinds_total`, mirroring
/// `cedar_authorizer::is_authorized_records_iam_authz_decisions_total_on_compute_path_miss`'s
/// `paigasus_observability::init` + `PrometheusHandle::render` pattern.
#[tokio::test]
async fn a_repair_records_iam_authz_generation_rewinds_total_with_expected_labels() {
    let Some((_node, url)) = start_redis().await else {
        return;
    };
    let handle = paigasus_observability::init("test-authz-generations-redis-rewind-metric");
    let gens = Generations::redis_connect(&url).await.expect("connect to redis");

    for _ in 0..3 {
        gens.bump_entity_gen().await.unwrap();
    }
    delete_key(&url, "iam:authz:entity_gen").await;

    let _ = gens.entity_gen().await.expect("a rewind must never surface as an error");

    let out = handle.render();
    assert!(out.contains("iam_authz_generation_rewinds_total"), "expected the rewind metric to be recorded:\n{out}");
    assert!(out.contains(r#"counter="entity_gen""#), "expected a counter=\"entity_gen\" label:\n{out}");
    assert!(out.contains(r#"outcome="repaired""#), "expected an outcome=\"repaired\" label:\n{out}");
    assert!(out.contains(r#"reason="missing""#), "expected a reason=\"missing\" label:\n{out}");

    // Added by the final SMA-474 review, and load-bearing: the redis backend now PRIMES the whole
    // closed label set at zero (`prime_rewind_metric`), so all four `contains()` calls above are
    // satisfied by the primed exposition alone. Only a value separates "counted" from "primed".
    let repaired = rewind_sample(&out, &[r#"counter="entity_gen""#, r#"outcome="repaired""#, r#"reason="missing""#])
        .unwrap_or_else(|| panic!("expected an iam_authz_generation_rewinds_total sample for the repair:\n{out}"));
    assert!(repaired >= 1.0, "the repair must be COUNTED, not merely primed, got {repaired}:\n{out}");
}
