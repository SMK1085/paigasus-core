// SPDX-License-Identifier: Apache-2.0

//! `RedisSessionStore` against a real Redis (SMA-753). The whole contract suite runs once on
//! `redis:6.2-alpine` (the stated minimum) and once on `redis:7.4-alpine` (the version kind runs).
//! Each contract case also runs alone on its own 7.4 container, so a failure names its case.
//! Raw reads exec `redis-cli` in the container: `repo:redis-connect-single-site` bans a second
//! Rust Redis client.

mod support;

use std::sync::Arc;

use paigasus_gateway::adapters::history::redis::{EvictionCheck, KEY_PREFIX, RedisSessionStore, decode_event, event_key, history_key};
use paigasus_gateway::domain::tool_history::{Scope, SessionStore, StoredEvent};
use support::history_contract::{self as contract, Harness};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::core::ExecCommand;
use uuid::Uuid;

struct RedisHarness {
    node: ContainerAsync<Redis>,
    url: String,
}

/// Reads one history with its bodies in one call: member, score, field count, then the fields of
/// `HGETALL e:<member>`. One exec, not one per event: a case can hold 10,000 events.
const EVENTS_LUA: &str = "local m = redis.call('ZRANGE', KEYS[1], 0, -1, 'WITHSCORES') local out = {} \
for i = 1, #m, 2 do local f = redis.call('HGETALL', ARGV[1] .. m[i]) \
out[#out + 1] = m[i] out[#out + 1] = m[i + 1] out[#out + 1] = tostring(#f) \
for _, v in ipairs(f) do out[#out + 1] = v end end return out";

const PTTL_LUA: &str = "local o = {} for i, k in ipairs(KEYS) do o[i] = redis.call('PTTL', k) end return o";

#[async_trait::async_trait]
impl Harness for RedisHarness {
    async fn store(&self) -> Arc<dyn SessionStore> {
        // A new store per call: one more replica on the same Redis.
        Arc::new(RedisSessionStore::connect(&self.url).await.expect("connect to the test Redis"))
    }

    /// The sorted set decides presence. A body is not filtered by its own expiry.
    async fn events(&self, org: Uuid, scope: Scope, scope_id: &str, tool: &str) -> Vec<StoredEvent> {
        let key = history_key(org, scope, scope_id, tool);
        let body_prefix = event_key(org, Uuid::nil()).trim_end_matches(&Uuid::nil().to_string()).to_string();
        let out = redis_cli(&self.node, &["EVAL", EVENTS_LUA, "1", &key, &body_prefix]).await;
        let lines: Vec<&str> = if out.is_empty() { Vec::new() } else { out.lines().collect() };
        let mut events = Vec::new();
        let mut at = 0;
        while at < lines.len() {
            let score: u64 = lines[at + 1].parse().expect("an integer score");
            let n: usize = lines[at + 2].parse().expect("a field count");
            let fields: Vec<(String, String)> = lines[at + 3..at + 3 + n].chunks(2).map(|p| (p[0].to_string(), p[1].to_string())).collect();
            let event = decode_event(&fields).expect("a stored body decodes");
            assert_eq!(event.id.to_string(), lines[at], "the member is the event id");
            assert_eq!(event.ts_ms, score, "the score is the event time");
            events.push(event);
            at += 3 + n;
        }
        events
    }

    async fn ttls_ms(&self, org: Uuid, _now_ms: u64) -> Vec<Option<u64>> {
        let pattern = format!("{KEY_PREFIX}*{org}*");
        let out = redis_cli(&self.node, &["--scan", "--pattern", &pattern]).await;
        let keys: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        if keys.is_empty() {
            return Vec::new();
        }
        let count = keys.len().to_string();
        let mut args = vec!["EVAL", PTTL_LUA, &count];
        args.extend(keys.iter().copied());
        let out = redis_cli(&self.node, &args).await;
        out.lines()
            .filter_map(|l| match l.parse::<i64>().expect("an integer PTTL") {
                -2 => None,
                -1 => Some(None),
                ms => Some(Some(u64::try_from(ms).expect("a positive PTTL"))),
            })
            .collect()
    }
}

/// The last line that `redis_cli` adds: the exit status of `redis-cli`.
const RC_MARK: &str = "redis-cli-rc=";

/// Runs `redis-cli <args>` INSIDE the container and returns its trimmed stdout. The exit status
/// comes from a last line that `sh` prints, not from `ExecResult::exit_code`: that future holds a
/// `&ExecResult`, which is not `Sync`, so it cannot run inside the `Send` futures of `Harness`.
async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let script = format!("redis-cli \"$@\"; echo \"{RC_MARK}$?\"");
    let mut argv = vec!["sh", "-c", script.as_str(), "sh"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    let out = String::from_utf8(result.stdout_to_vec().await.expect("redis-cli stdout")).expect("redis-cli output is utf-8");
    let out = out.trim_end();
    let (body, rc) = out.rsplit_once(RC_MARK).expect("the exit status line");
    assert_eq!(rc, "0", "redis-cli failed (exit {rc})");
    body.trim().to_string()
}

async fn harness(tag: &str, what: &str) -> Option<RedisHarness> {
    let (node, url) = paigasus_test_docker::start_redis_image_or_skip(tag, what).await?;
    Some(RedisHarness { node, url })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_6_2() {
    let Some(h) = harness("6.2-alpine", "history_store_redis contract 6.2").await else {
        return;
    };
    contract::run_all(&h).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_7_4() {
    let Some(h) = harness("7.4-alpine", "history_store_redis contract 7.4").await else {
        return;
    };
    contract::run_all(&h).await;
}

/// One test per contract case, each on its own 7.4 container.
macro_rules! contract_cases {
    ($($case:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                let Some(h) = harness("7.4-alpine", concat!("history_store_redis ", stringify!($case))).await else {
                    return;
                };
                contract::$case(&h).await;
            }
        )*
    };
}

contract_cases!(
    a_reserve_appends_one_pending_event_per_history,
    a_caller_without_a_key_writes_the_principal_history_once,
    the_split_by_tool_keeps_other_tools,
    retention_trims_only_events_older_than_25_hours,
    the_cap_keeps_the_newest_10_000_events_per_tool,
    the_truncated_flag_needs_a_lost_event_inside_the_window,
    settle_sets_the_verdict_and_writes_the_call_link,
    settle_twice_is_idempotent,
    a_deny_settle_writes_no_call_link,
    a_lost_settle_leaves_the_event_pending_and_no_call_link,
    a_settle_for_an_unknown_event_writes_nothing,
    the_reserve_writes_decision_links,
    links_are_isolated_per_principal_and_org,
    a_result_event_is_written_once_per_call_id,
    a_write_without_results_and_without_turn_writes_nothing,
    touch_keeps_the_latest_activity_and_the_highest_turn,
    a_future_event_survives_a_reserve_with_an_older_clock,
    every_written_key_expires_after_the_retention,
    append_writes_an_allowed_event_and_its_call_link,
    a_local_shell_settle_links_the_item_id_too,
    concurrent_reserves_all_land,
    a_cap_trim_in_the_principal_history_keeps_the_event_in_its_session_history,
    a_cut_of_two_flagged_events_gives_one_truncated_entry_per_scope_and_tool,
    a_reserve_on_a_new_session_writes_its_state,
    a_reserve_with_a_lower_turn_does_not_lower_the_turn,
    an_append_on_a_new_session_writes_its_state,
    a_second_allow_settle_of_a_call_id_relinks_it_to_the_new_event,
    the_first_settle_wins,
    a_repeated_settle_of_the_same_event_keeps_result_written,
    a_result_for_a_call_id_without_a_link_writes_nothing,
);

#[tokio::test]
async fn an_evicting_policy_is_reported() {
    let Some(h) = harness("7.4-alpine", "history_store_redis evicting policy").await else {
        return;
    };
    assert_eq!(redis_cli(&h.node, &["CONFIG", "SET", "maxmemory-policy", "allkeys-lru"]).await, "OK");
    let store = RedisSessionStore::connect(&h.url).await.expect("connect");
    assert_eq!(store.read_eviction_policy().await, EvictionCheck::Evicts("allkeys-lru".to_string()));
}

#[tokio::test]
async fn noeviction_is_safe() {
    let Some(h) = harness("7.4-alpine", "history_store_redis noeviction").await else {
        return;
    };
    assert_eq!(redis_cli(&h.node, &["CONFIG", "SET", "maxmemory-policy", "noeviction"]).await, "OK");
    let store = RedisSessionStore::connect(&h.url).await.expect("connect");
    assert_eq!(store.read_eviction_policy().await, EvictionCheck::Safe);
}
