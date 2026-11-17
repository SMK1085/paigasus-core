// SPDX-License-Identifier: Apache-2.0
//! SMA-752 § 7.4, G8, D18: the tool-policy overhead per request, measured in release mode.
//!
//! Both tests are `#[ignore]`, because a debug build says nothing about G8. Run them with:
//! `cargo nextest run --release -p paigasus-gateway --test perf --run-ignored only --no-capture`.
//! The overhead is the `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS` histogram. The gateway records one
//! sample per request: the request-side time plus the decider time (preflight F14).

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, Lookup, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
use paigasus_gateway_core::{Limits as CoreLimits, OnUnavailable, TenantMode};
use paigasus_observability::names;
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::{ORG_A, ORG_A_PRN, PRINCIPAL_1, ScopedIam, app_with_tool_policy, send};
use uuid::Uuid;

const POLICIES: usize = 50;
const DEFINITIONS: usize = 30;
const REQUESTS: usize = 2000;
const CONCURRENCY: usize = 8;
const ARGUMENT_BYTES: usize = 64 * 1024;

const FLOOR: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-name-collision\") forbid(principal, action, resource) when { context.findings.contains(\"name_collision\") };\n";

/// 50 tenant policies. 48 permit `read_file` with a condition on the argument; one permits the rest; one forbids `blocked_rm`.
fn tenant_policies() -> String {
    let mut text = String::new();
    for i in 0..POLICIES - 2 {
        let prefix = if i == 0 { "/workspace/".to_owned() } else { format!("/zone-{i}/") };
        text.push_str(&format!(
            "@id(\"p{i}\") permit(principal is Pgs::Iam::Principal, action == Pgs::Gateway::Action::\"read_file\", resource) when {{ context.input has path && context.input.path like \"{prefix}*\" }};\n"
        ));
    }
    text.push_str("@id(\"rest\") permit(principal is Pgs::Iam::Principal, action, resource);\n");
    text.push_str("@id(\"no-rm\") forbid(principal, action == Pgs::Gateway::Action::\"blocked_rm\", resource);\n");
    text
}

fn write_policies(root: &std::path::Path) {
    std::fs::write(root.join("floor.cedar"), FLOOR).expect("floor");
    let t = root.join("tenants").join(ORG_A);
    std::fs::create_dir_all(&t).expect("tenant dir");
    std::fs::write(t.join("policies.cedar"), tenant_policies()).expect("tenant");
}

fn settings() -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode: TenantMode::Enforce,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(60),
        orgs: HashMap::new(),
        settings_from_source: false,
    }
}

/// 30 definitions (`read_file`, the removed `blocked_rm`, 28 more), tool results in the history,
/// and one earlier call with a 64 KiB argument.
fn request_body() -> String {
    let mut names: Vec<String> = vec!["read_file".to_owned(), "blocked_rm".to_owned()];
    names.extend((0..DEFINITIONS - 2).map(|i| format!("tool_{i}")));
    let tools: Vec<Value> = names.iter().map(|n| json!({"type":"function","function":{"name":n,"parameters":{}}})).collect();
    let big = format!("{{\"path\":\"/workspace/{}\"}}", "a".repeat(ARGUMENT_BYTES - 32));
    let messages = json!([
        {"role":"user","content":"hi"},
        {"role":"assistant","content":null,"tool_calls":[
            {"id":"h1","type":"function","function":{"name":"read_file","arguments":big}},
            {"id":"h2","type":"function","function":{"name":"tool_0","arguments":"{\"q\":1}"}}
        ]},
        {"role":"tool","tool_call_id":"h1","content":"file text"},
        {"role":"tool","tool_call_id":"h2","content":"result"},
        {"role":"user","content":"go on"}
    ]);
    json!({"model":"gpt-4o-mini","stream":false,"messages":messages,"tools":tools}).to_string()
}

/// A non-stream answer that calls `read_file` and `tool_1`.
fn response_body() -> String {
    let calls = json!([
        {"id":"call_0","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"/workspace/a.txt\"}"}},
        {"id":"call_1","type":"function","function":{"name":"tool_1","arguments":"{}"}}
    ]);
    json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}).to_string()
}

fn take_overhead(snapshotter: &Snapshotter) -> Vec<f64> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(k, _, _, _)| k.key().name() == names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS)
        .flat_map(|(_, _, _, v)| match v {
            DebugValue::Histogram(h) => h.into_iter().map(f64::from).collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement in release mode"]
async fn p99_policy_overhead_with_50_policies() {
    // Global, because the requests run on other worker threads. Nextest runs each test in its own process.
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install().expect("the one global recorder");

    let dir = tempfile::tempdir().expect("tempdir");
    write_policies(dir.path());
    let settings = settings();
    let cache = PolicyCache::new(Arc::new(FilePolicySource::new(dir.path())), CacheSettings::from_settings(&settings));
    let tp = Some(Arc::new(RequestToolPolicy::new(cache, settings)));
    let ti = Some(Arc::new(ToolInterception {
        limits: CoreLimits::defaults(),
        decider: Arc::new(CedarDecider),
    }));
    let mock = MockOpenAi::spawn_json(StatusCode::OK, response_body()).await;
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, ti, tp);
    let body = Arc::new(request_body());

    // Warm the cache, and check the fixture does what it should: one definition removed.
    for _ in 0..20 {
        let sent = send(&app, &body).await;
        assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    }
    let upstream: Value = serde_json::from_slice(&mock.recorded().expect("a request").body).expect("json");
    let sent_tools = upstream["tools"].as_array().expect("tools");
    assert_eq!(sent_tools.len(), DEFINITIONS - 1, "blocked_rm is removed");
    assert!(sent_tools.iter().all(|t| t["function"]["name"] != "blocked_rm"));
    let _warm = take_overhead(&snapshotter);

    let gate = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    let started = Instant::now();
    for _ in 0..REQUESTS {
        let permit = Arc::clone(&gate).acquire_owned().await.expect("semaphore");
        let (app, body) = (app.clone(), Arc::clone(&body));
        set.spawn(async move {
            let sent = send(&app, &body).await;
            drop(permit);
            sent.status
        });
    }
    while let Some(status) = set.join_next().await {
        assert_eq!(status.expect("task"), StatusCode::OK);
    }
    let wall = started.elapsed();

    let mut samples = take_overhead(&snapshotter);
    assert_eq!(samples.len(), REQUESTS, "one overhead sample per request");
    samples.sort_by(f64::total_cmp);
    let p99 = percentile(&samples, 0.99);
    println!(
        "p50 = {:.3} ms, p99 = {:.3} ms, max = {:.3} ms over {} samples ({} policies, {} definitions, {:.1} s wall, {}, profile {})",
        percentile(&samples, 0.5) * 1e3,
        p99 * 1e3,
        samples[samples.len() - 1] * 1e3,
        samples.len(),
        POLICIES,
        DEFINITIONS,
        wall.as_secs_f64(),
        std::env::consts::ARCH,
        if cfg!(debug_assertions) { "debug" } else { "release" },
    );
    assert!(p99 < 0.015, "p99 {p99}");
}

#[tokio::test]
#[ignore = "a measurement in release mode"]
async fn cold_load_and_compile_of_50_policies() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_policies(dir.path());
    let settings = settings();
    let cache = PolicyCache::new(Arc::new(FilePolicySource::new(dir.path())), CacheSettings::from_settings(&settings));
    let org = Uuid::try_parse(ORG_A).expect("uuid");
    let started = Instant::now();
    let Lookup::Ready(floor) = cache.floor(OnUnavailable::Closed).await else {
        panic!("the floor loads");
    };
    let Lookup::Ready(_tenant) = cache.tenant(org, &floor, OnUnavailable::Closed).await else {
        panic!("the tenant loads");
    };
    let took = started.elapsed();
    println!("cold load and compile of the floor and {POLICIES} policies = {:.3} ms", took.as_secs_f64() * 1e3);
    assert!(took < Duration::from_millis(50), "{took:?}");
}
