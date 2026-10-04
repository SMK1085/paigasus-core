// SPDX-License-Identifier: Apache-2.0

//! `RedisLimitStore` against a real Redis (SMA-677 spec § 5.2, § 5.3, § 5.8). The contract suite
//! runs once on `redis:6.2-alpine` (the stated minimum) and once on `redis:7.4-alpine` (the
//! version kind runs), one container per image; its cases use their own ids. The adapter cases
//! start their own 7.4 container. Raw reads exec `redis-cli` in the container, as IAM's tests do:
//! `repo:redis-connect-single-site` bans a second Rust Redis client.

mod support;

use std::sync::Arc;
use std::time::SystemTime;

use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::limits::RedisLimitStore;
use paigasus_gateway::adapters::limits::redis::{budget_key, org_rate_key, principal_rate_key};
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::domain::limits::{BudgetPeriod, LimitDecision, LimitPolicy, LimitStore, LimitTicket, WindowIndex};
use paigasus_gateway::domain::{CallerContext, Credential};
use support::limits::{FixedClock, ORG_A, ORG_A_PRN, rules};
use support::limits_contract::{self as contract, Harness, at, fresh_ids};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::core::ExecCommand;
use uuid::Uuid;

struct RedisHarness {
    url: String,
    charger: RedisLimitStore,
}

impl RedisHarness {
    async fn new(url: String) -> Self {
        let charger = RedisLimitStore::connect(&url).await.expect("connect to the test Redis");
        RedisHarness { url, charger }
    }
}

#[async_trait::async_trait]
impl Harness for RedisHarness {
    async fn store(&self) -> Arc<dyn LimitStore> {
        // A new store per call: one more replica on the same Redis.
        Arc::new(RedisLimitStore::connect(&self.url).await.expect("connect to the test Redis"))
    }

    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        self.charger.apply_charge(ticket, tokens, now).await.expect("the charge is recorded");
    }
}

/// Runs `redis-cli <args>` INSIDE the container and returns its trimmed stdout (the pattern of
/// `paigasus-iam/tests/authz_generations_redis.rs:62-82`).
async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let mut argv = vec!["redis-cli"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    // Drain stdout BEFORE the exit code, or the exit code may still be `None`.
    let out = result.stdout_to_vec().await.expect("redis-cli stdout");
    let code = result.exit_code().await.expect("redis-cli exit status");
    assert_eq!(code, Some(0), "redis-cli {args:?} failed (exit {code:?})");
    String::from_utf8(out).expect("redis-cli output is utf-8").trim().to_string()
}

fn counter(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
    snapshotter.snapshot().into_vec().into_iter().find_map(|(key, _, _, value)| {
        let key = key.key();
        let have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
        let same = key.name() == name && have.len() == labels.len() && labels.iter().all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v));
        match (same, value) {
            (true, DebugValue::Counter(n)) => Some(n),
            _ => None,
        }
    })
}

fn org_a() -> Uuid {
    Uuid::try_parse(ORG_A).expect("a fixture uuid")
}

async fn redis_7_4(what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", what).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_6_2() {
    let Some((_node, url)) = paigasus_test_docker::start_redis_image_or_skip("6.2-alpine", "limits_store_redis contract 6.2").await else {
        return;
    };
    contract::run_all(&RedisHarness::new(url).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_7_4() {
    let Some((_node, url)) = redis_7_4("limits_store_redis contract 7.4").await else {
        return;
    };
    contract::run_all(&RedisHarness::new(url).await).await;
}

/// D18: `EXPIRE 180` on every increment of both current rate keys.
#[tokio::test]
async fn an_admission_sets_a_180_second_ttl_on_both_current_rate_keys() {
    let Some((node, url)) = redis_7_4("limits_store_redis rate ttl").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = LimitPolicy {
        principal_requests_per_minute: std::num::NonZeroU64::new(5),
        org_requests_per_minute: std::num::NonZeroU64::new(5),
        budget: None,
    };
    assert_eq!(store.check_and_admit(&principal, Some(org), &policy, now).await.expect("admitted"), LimitDecision::Admit(None));
    let window = WindowIndex::at(now).index;
    for key in [principal_rate_key(&principal, window), org_rate_key(org, window)] {
        let ttl: i64 = redis_cli(&node, &["TTL", &key]).await.parse().expect("an integer TTL");
        assert!((175..=180).contains(&ttl), "{key}: TTL {ttl}");
    }
}

/// D18: the budget TTL is relative and comes from `now`. Both bounds: the lower catches a missing
/// `+ 86400` or a missing `EXPIRE` (TTL -1); the upper catches a TTL that does not come from `now`.
#[tokio::test]
async fn a_charge_sets_the_relative_budget_ttl() {
    let Some((node, url)) = redis_7_4("limits_store_redis budget ttl").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (_, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let period = BudgetPeriod::Monthly.key_at(now);
    let ttl_secs = i64::from(BudgetPeriod::charge_ttl(period, now).expect("inside the period"));
    // Written out, not derived: 2026-10-02T12:00Z to 2026-11-01T00:00Z is 29.5 days, plus the 86400 s
    // grace. A test that only compared Redis with `charge_ttl` would pass when `charge_ttl` is wrong.
    assert_eq!(ttl_secs, 29 * 86_400 + 43_200 + 86_400);
    store.apply_charge(LimitTicket { org, period }, 7, now).await.expect("charged");
    let key = budget_key(org, period);
    let ttl: i64 = redis_cli(&node, &["TTL", &key]).await.parse().expect("an integer TTL");
    assert!(ttl_secs - 5 <= ttl && ttl <= ttl_secs, "TTL {ttl}, expected about {ttl_secs}");
    assert_eq!(redis_cli(&node, &["GET", &key]).await, "7");
}

/// D18: a charge 86400 s after its period end writes no key and is counted.
#[tokio::test]
async fn a_charge_a_day_late_writes_nothing_and_is_counted() {
    let Some((node, url)) = redis_7_4("limits_store_redis period expired").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (_, org) = fresh_ids();
    let period = BudgetPeriod::Monthly.key_at(at("2026-10-15T00:00:00Z"));
    store.apply_charge(LimitTicket { org, period }, 7, at("2026-11-02T00:00:00Z")).await.expect("no store error");
    assert_eq!(redis_cli(&node, &["EXISTS", &budget_key(org, period)]).await, "0");
    assert_eq!(counter(&snapshotter, "gateway_limit_charges_dropped_total", &[("reason", "period_expired")]), Some(1));
}

/// D18: after one admission and one charge, exactly the expected keys exist.
#[tokio::test]
async fn the_keys_follow_the_v1_format() {
    let Some((node, url)) = redis_7_4("limits_store_redis keys").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = rules(Some(5), Some(5), Some(100)).policy_for(Some(org));
    let LimitDecision::Admit(Some(ticket)) = store.check_and_admit(&principal, Some(org), &policy, now).await.expect("admitted") else {
        panic!("a budget admission issues a ticket");
    };
    let period = ticket.period;
    store.apply_charge(ticket, 3, now).await.expect("charged");
    let mut keys: Vec<String> = redis_cli(&node, &["KEYS", "paigasus:gateway:limits:v1:*"]).await.lines().map(str::to_owned).collect();
    keys.sort();
    let window = WindowIndex::at(now).index;
    let mut want = vec![budget_key(org, period), org_rate_key(org, window), principal_rate_key(&principal, window)];
    want.sort();
    assert_eq!(keys, want);
}

/// D19: `SCRIPT FLUSH` (or a restart, or a failover) is recovered by `invoke_async`.
#[tokio::test]
async fn a_flushed_script_is_reloaded() {
    let Some((node, url)) = redis_7_4("limits_store_redis noscript").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, _) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = rules(Some(5), None, None).policy_for(None);
    assert_eq!(store.check_and_admit(&principal, None, &policy, now).await, Ok(LimitDecision::Admit(None)));
    assert_eq!(redis_cli(&node, &["SCRIPT", "FLUSH"]).await, "OK");
    assert_eq!(
        store.check_and_admit(&principal, None, &policy, now).await,
        Ok(LimitDecision::Admit(None)),
        "NOSCRIPT → SCRIPT LOAD → EVALSHA"
    );
}

/// D10/D21: a full `noeviction` Redis refuses the script with OOM: fail-open, `kind="server"`.
#[tokio::test]
async fn a_full_noeviction_redis_fails_open_with_kind_server() {
    let Some((node, url)) = redis_7_4("limits_store_redis oom").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    assert_eq!(redis_cli(&node, &["CONFIG", "SET", "maxmemory-policy", "noeviction"]).await, "OK");
    assert_eq!(redis_cli(&node, &["CONFIG", "SET", "maxmemory", "1"]).await, "OK");
    let limits = Arc::new(Limits::new(rules(Some(5), None, None), Arc::new(store), Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
    let caller = CallerContext {
        principal_prn: fresh_ids().0,
        scope_prn: ORG_A_PRN.to_owned(),
        credential: Credential::ApiKey { key_id: "k".to_owned() },
    };
    let guard = limits.admit(&caller).await.expect("fail-open admits");
    assert!(!guard.has_ticket());
    assert_eq!(counter(&snapshotter, "gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]), Some(1));
}

/// D16, spec § 5.8 with a runtime: a dropped guard's charge lands once the tracker drains.
#[tokio::test]
async fn a_dropped_guard_charges_through_the_tracker() {
    let Some((node, url)) = redis_7_4("limits_store_redis guard").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let tracker = store.tracker();
    let now = at("2026-10-02T12:00:00Z");
    let limits = Arc::new(Limits::new(rules(None, None, Some(100)), Arc::new(store), Arc::new(FixedClock(now))));
    let caller = CallerContext {
        principal_prn: fresh_ids().0,
        scope_prn: ORG_A_PRN.to_owned(),
        credential: Credential::ApiKey { key_id: "k".to_owned() },
    };
    let mut guard = limits.admit(&caller).await.expect("admitted");
    assert!(guard.has_ticket());
    guard.set_request_estimate(7);
    guard.mark_sent();
    drop(guard);
    tracker.close();
    tracker.wait().await;
    assert_eq!(redis_cli(&node, &["GET", &budget_key(org_a(), BudgetPeriod::Monthly.key_at(now))]).await, "7");
}

/// Task 11 review (measured on Redis 5.0): a count that is not an integer must fail the script
/// BEFORE any write, so the earlier principal key is not incremented (D4 all-or-nothing). The
/// store fails open: `Unavailable`, `kind = Server`.
#[tokio::test]
async fn a_non_integer_count_fails_before_any_write() {
    let Some((node, url)) = redis_7_4("limits_store_redis non-integer count").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let window = WindowIndex::at(now).index;
    let org_key = org_rate_key(org, window);
    assert_eq!(redis_cli(&node, &["SET", &org_key, "1.5"]).await, "OK");
    let policy = LimitPolicy {
        principal_requests_per_minute: std::num::NonZeroU64::new(5),
        org_requests_per_minute: std::num::NonZeroU64::new(5),
        budget: None,
    };
    match store.check_and_admit(&principal, Some(org), &policy, now).await {
        Err(paigasus_gateway::domain::limits::LimitStoreError::Unavailable {
            kind: paigasus_gateway::domain::limits::UnavailableKind::Server,
            ..
        }) => {}
        other => panic!("a non-integer count must fail the script with a server error, got {other:?}"),
    }
    assert_eq!(
        redis_cli(&node, &["EXISTS", &principal_rate_key(&principal, window)]).await,
        "0",
        "the principal key must not be written"
    );
    assert_eq!(redis_cli(&node, &["GET", &org_key]).await, "1.5", "the org key is untouched");
}

/// D3 / D23 boundary: limit = MAX_REQUESTS_PER_MINUTE with `current = limit - 1` admits exactly
/// once (Lua doubles hold every product below 2^53), and with `previous = limit` at elapsed
/// 59_999 it still admits (the weight is 1 ms of a window).
#[tokio::test]
async fn the_maximum_limit_is_exact_in_the_script() {
    let Some((node, url)) = redis_7_4("limits_store_redis max limit").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let max = paigasus_gateway::domain::limits::MAX_REQUESTS_PER_MINUTE;
    let policy = LimitPolicy {
        principal_requests_per_minute: std::num::NonZeroU64::new(max),
        org_requests_per_minute: None,
        budget: None,
    };
    let (principal, _) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let window = WindowIndex::at(now).index;
    let key = principal_rate_key(&principal, window);
    assert_eq!(redis_cli(&node, &["SET", &key, &(max - 1).to_string()]).await, "OK");
    assert_eq!(store.check_and_admit(&principal, None, &policy, now).await, Ok(LimitDecision::Admit(None)));
    assert_eq!(redis_cli(&node, &["GET", &key]).await, max.to_string());
    let refused = store.check_and_admit(&principal, None, &policy, now).await.expect("a decision");
    assert!(matches!(refused, LimitDecision::Refused(_)), "{refused:?}");
    // previous = limit, elapsed 59_999: 1e9 × 1 + 0 + 60000 ≤ 1e9 × 60000.
    let (second, _) = fresh_ids();
    let late = at("2026-10-02T12:00:59.999Z");
    assert_eq!(redis_cli(&node, &["SET", &principal_rate_key(&second, window - 1), &max.to_string()]).await, "OK");
    assert_eq!(store.check_and_admit(&second, None, &policy, late).await, Ok(LimitDecision::Admit(None)));
}

fn caller_for(principal: String) -> CallerContext {
    CallerContext {
        principal_prn: principal,
        scope_prn: ORG_A_PRN.to_owned(),
        credential: Credential::ApiKey { key_id: "k".to_owned() },
    }
}

/// D20: a blackholed Redis with a closed breaker. The request is admitted with no ticket, and the
/// failure is counted as `kind="io"`.
#[tokio::test]
async fn a_blackholed_redis_admits_through_limits() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let blackhole = paigasus_redis::test_support::start().await;
    let handle = paigasus_redis::new_lazy_for_tests(&blackhole.url, paigasus_gateway::adapters::limits::redis::breaker_metrics()).expect("a lazy handle");
    let store = RedisLimitStore::from_handle(handle);
    let limits = Arc::new(Limits::new(rules(Some(5), None, Some(100)), Arc::new(store), Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
    let guard = limits.admit(&caller_for(fresh_ids().0)).await.expect("fail-open admits");
    assert!(!guard.has_ticket());
    assert_eq!(counter(&snapshotter, "gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "io")]), Some(1));
}

/// D10: an open breaker admits at once, with no dial.
#[tokio::test]
async fn an_open_breaker_admits_through_limits_without_dialling() {
    let blackhole = paigasus_redis::test_support::start().await;
    let handle = paigasus_redis::with_open_breaker_for_tests(&blackhole.url, paigasus_gateway::adapters::limits::redis::breaker_metrics()).expect("a lazy handle");
    let limits = Arc::new(Limits::new(
        rules(Some(5), Some(5), None),
        Arc::new(RedisLimitStore::from_handle(handle)),
        Arc::new(FixedClock(at("2026-10-02T12:00:00Z"))),
    ));
    let guard = limits.admit(&caller_for(fresh_ids().0)).await.expect("fail-open admits");
    assert!(!guard.has_ticket());
    assert_eq!(blackhole.accepted(), 0);
}
