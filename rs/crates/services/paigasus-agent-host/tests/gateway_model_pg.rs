// SPDX-License-Identifier: Apache-2.0

//! The gateway model of the agent host against a fake AI Gateway (SMA-851). `RunWorker` runs on the
//! real adapters and a Postgres container. The model is the Helikon Anthropic provider, built by
//! `gateway_model::register`. One test for each error class of the spec (§7.5), the redirect
//! rule and the rule that the key never reaches a log line.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use paigasus_agent_host::adapters::agents::ConfigAgentResolver;
use paigasus_agent_host::adapters::helikon::gateway_model::{GatewayProbe, ProbeOutcome, register};
use paigasus_agent_host::adapters::helikon::memory_tool::run_nonce;
use paigasus_agent_host::adapters::helikon::model_registry::ModelRegistry;
use paigasus_agent_host::adapters::helikon::runner::HelikonAgentRunner;
use paigasus_agent_host::adapters::persistence::lease_store::PgLeaseStore;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::application::lease_keeper::KeeperTiming;
use paigasus_agent_host::application::worker::breaker::HostFaultSettings;
use paigasus_agent_host::application::worker::ports::RunLimits;
use paigasus_agent_host::application::worker::run::{LeaseEnd, RunWorker, WorkerPorts, WorkerSettings};
use paigasus_agent_host::config::{GatewayConfig, ModelConfig, ModelProvider};
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::fault::NoFaults;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::ports::LeaseRepository;
use paigasus_agent_host_core::retry::RetryPolicy;
use paigasus_agent_host_core::screen::AllowAll;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;
use support::commit::committer;
use support::fake_gateway::{FakeGateway, ModelsReply, PROMPT_TOO_LONG, Reply, TEXT_STREAM, counting_listener, error_body, text_sse, tool_use_sse, with_stop_reason};
use support::inbox::{insert_inbox, migrated_postgres};
use support::lease::insert_pending_event;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const KEY: &str = "sk-gw-test-0123456789";

struct Fixture {
    org: Uuid,
    inbox: Uuid,
    agent: Uuid,
    principal: Uuid,
    event: Uuid,
}

/// Everything that one test needs: the database, the inbox, the fake gateway and the registry.
struct Rig {
    db: DatabaseConnection,
    f: Fixture,
    fake: FakeGateway,
    models: ModelRegistry,
    _node: ContainerAsync<Postgres>,
    _dir: tempfile::TempDir,
}

/// A key file, a gateway config on `addr` and the registry with the model `claude`.
fn register_on(addr: std::net::SocketAddr, read_timeout_ms: u64) -> (ModelRegistry, GatewayProbe, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let key_file = dir.path().join("gateway.key");
    std::fs::write(&key_file, KEY).expect("write the key file");
    let gateway = GatewayConfig {
        base_url: format!("http://{addr}"),
        api_key_file: key_file.to_str().expect("utf-8 path").to_owned(),
        ca_file: None,
        connect_timeout_ms: 1000,
        read_timeout_ms,
    };
    let entries = [ModelConfig {
        name: "claude".to_owned(),
        provider: ModelProvider::Anthropic,
        model: "claude-sonnet-5-5".to_owned(),
        max_output_tokens: 16000,
    }];
    let mut models = ModelRegistry::default();
    let probe = register(&mut models, &gateway, &entries).expect("register the gateway model");
    (models, probe, dir)
}

async fn rig(replies: Vec<Reply>) -> Option<Rig> {
    rig_with(replies, 1000).await
}

async fn rig_with(replies: Vec<Reply>, read_timeout_ms: u64) -> Option<Rig> {
    let (node, _url, db) = migrated_postgres().await?;
    let f = Fixture {
        org: Uuid::new_v4(),
        inbox: Uuid::new_v4(),
        agent: Uuid::new_v4(),
        principal: Uuid::new_v4(),
        event: Uuid::new_v4(),
    };
    insert_inbox(&db, f.org, f.inbox, f.agent, f.principal).await;
    insert_pending_event(&db, f.inbox, f.event, 5).await;
    let fake = FakeGateway::start(replies).await;
    let (models, _probe, dir) = register_on(fake.addr, read_timeout_ms);
    Some(Rig {
        db,
        f,
        fake,
        models,
        _node: node,
        _dir: dir,
    })
}

fn worker(rig: &Rig, store: Arc<PgLeaseStore>) -> RunWorker {
    let agent = AgentDefinition {
        id: AgentId::new(rig.f.org, rig.f.agent),
        name: "worker".to_owned(),
        model_id: "claude".to_owned(),
        instructions: "Use the memory tool.".to_owned(),
        tool_names: vec!["memory".to_owned()],
    };
    let db = &rig.db;
    let ports = WorkerPorts {
        leases: store.clone(),
        claims: store,
        committer: committer(db),
        resolver: Arc::new(ConfigAgentResolver::new(db.clone(), vec![agent])),
        screen: Arc::new(AllowAll),
        runner: Arc::new(HelikonAgentRunner::new(db.clone(), Arc::new(rig.models.clone()))),
        memory: Arc::new(PgMemoryStore::new(db.clone())),
        faults: Arc::new(NoFaults),
        nonce: run_nonce,
    };
    let settings = WorkerSettings {
        claim: ClaimConfig {
            batch_limit: 20,
            max_attempts: 5,
            thread_ttl: Duration::from_secs(86400),
        },
        timing: KeeperTiming::new(Duration::from_secs(30), Duration::from_secs(10)).expect("timing"),
        limits: RunLimits {
            max_turns: 16,
            timeout: Duration::from_secs(60),
        },
        retry: RetryPolicy {
            base: Duration::from_secs(30),
            max: Duration::from_secs(1800),
        },
        max_runs_per_lease: 8,
        caps: MemoryCaps::default(),
        host_fault: HostFaultSettings {
            pause_after: 3,
            pause: Duration::from_secs(30),
        },
    };
    RunWorker::new(ports, settings)
}

/// Take the lease of the inbox and serve it once.
async fn serve_once(rig: &Rig) -> LeaseEnd {
    let store = Arc::new(PgLeaseStore::new(rig.db.clone(), Duration::from_millis(500)));
    let handle = store
        .acquire(&InboxId::new(rig.f.org, rig.f.inbox), "w1", Duration::from_secs(30))
        .await
        .expect("acquire")
        .expect("the lease is free");
    worker(rig, store).serve(handle, &CancellationToken::new()).await
}

/// The run row and the event row after a serve.
#[derive(Debug)]
struct Outcome {
    run_status: String,
    error_class: Option<String>,
    error: Option<String>,
    final_output: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    thread_id: Uuid,
    event_status: String,
    attempts: i32,
    not_before_secs: f64,
    /// The time that `serve_once` took. It is set by `class_case`.
    served_in: Duration,
}

async fn outcome(rig: &Rig) -> Outcome {
    let sql = format!(
        "SELECT r.status AS run_status, r.error_class, r.error, r.final_output, r.input_tokens, r.output_tokens, r.thread_id, \
         e.status AS event_status, e.attempts, EXTRACT(EPOCH FROM (e.not_before - now()))::float8 AS secs \
         FROM run r JOIN inbox_event e ON e.id = '{}' WHERE r.inbox_id = '{}' ORDER BY r.started_at DESC LIMIT 1",
        rig.f.event, rig.f.inbox
    );
    let row = rig
        .db
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect("query the outcome")
        .expect("the run row");
    Outcome {
        run_status: row.try_get("", "run_status").expect("run_status"),
        error_class: row.try_get("", "error_class").expect("error_class"),
        error: row.try_get("", "error").expect("error"),
        final_output: row.try_get("", "final_output").expect("final_output"),
        input_tokens: row.try_get("", "input_tokens").expect("input_tokens"),
        output_tokens: row.try_get("", "output_tokens").expect("output_tokens"),
        thread_id: row.try_get("", "thread_id").expect("thread_id"),
        event_status: row.try_get("", "event_status").expect("event_status"),
        attempts: row.try_get("", "attempts").expect("attempts"),
        not_before_secs: row.try_get("", "secs").expect("secs"),
        served_in: Duration::ZERO,
    }
}

fn error_reply(status: u16, error_type: &str, message: &str, retry_after: Option<u64>) -> Reply {
    Reply::Error {
        status,
        body: error_body(error_type, message),
        retry_after,
    }
}

/// What a class test expects of the run row, the event row and the fake gateway.
struct Want {
    run_status: &'static str,
    error_class: Option<&'static str>,
    event_status: &'static str,
    attempts: i32,
    requests: usize,
}

/// Serve one run against `replies` and check the table row of `Want`.
async fn class_case(replies: Vec<Reply>, want: Want) -> Option<(Rig, Outcome)> {
    let Some(rig) = rig(replies).await else {
        eprintln!("skipping gateway_model_pg: Docker unavailable");
        return None;
    };
    let start = Instant::now();
    serve_once(&rig).await;
    let served_in = start.elapsed();
    let mut got = outcome(&rig).await;
    got.served_in = served_in;
    assert_eq!(got.run_status, want.run_status, "run status; {got:?}");
    assert_eq!(got.error_class.as_deref(), want.error_class, "error class; {got:?}");
    assert_eq!(got.event_status, want.event_status, "event status; {got:?}");
    assert_eq!(got.attempts, want.attempts, "event attempts; {got:?}");
    assert_eq!(rig.fake.requests().len(), want.requests, "requests to the fake gateway; {got:?}");
    Some((rig, got))
}

fn host_fault_want(requests: usize) -> Want {
    Want {
        run_status: "failed",
        error_class: Some("host_fault"),
        event_status: "pending",
        attempts: 0,
        requests,
    }
}

fn deterministic_want() -> Want {
    Want {
        run_status: "failed",
        error_class: Some("deterministic"),
        event_status: "pending",
        attempts: 1,
        requests: 1,
    }
}

fn completed_want(requests: usize) -> Want {
    Want {
        run_status: "completed",
        error_class: None,
        event_status: "done",
        attempts: 1,
        requests,
    }
}

// ---- the request, the tool round, the blocks and the pings ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_request_has_the_key_the_version_the_model_and_the_limit() {
    let Some((rig, got)) = class_case(vec![Reply::Sse(TEXT_STREAM.to_owned())], completed_want(1)).await else {
        return;
    };
    let first = &rig.fake.requests()[0];
    assert_eq!(first.headers.get("x-api-key").expect("x-api-key"), KEY);
    assert!(first.headers.get("anthropic-version").is_some(), "no anthropic-version header");
    assert!(first.headers.get("authorization").is_none(), "an authorization header was sent");
    assert_eq!(first.body["model"], "claude-sonnet-5-5");
    assert_eq!(first.body["max_tokens"], 16000);
    assert!(!first.body.to_string().contains("cache_control"), "the request has cache_control");
    assert_eq!(first.body["tools"][0]["name"], "memory");
    assert!(first.body["tools"][0]["input_schema"].is_object(), "the tool has no input_schema object");
    assert_eq!(got.final_output.as_deref(), Some("Hello, it's nice to meet you!"));
    assert_eq!(got.input_tokens, Some(19));
    assert_eq!(got.output_tokens, Some(15));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_round_sends_the_tool_result_back() {
    let call = tool_use_sse("toolu_01", "memory", &json!({"command": "create", "path": "/memories/a.md", "file_text": "x"}));
    let Some((rig, _)) = class_case(vec![Reply::Sse(call), Reply::Sse(TEXT_STREAM.to_owned())], completed_want(2)).await else {
        return;
    };
    let second = &rig.fake.requests()[1];
    let has_result = second.body["messages"].as_array().expect("messages").iter().any(|m| {
        m["role"] == "user"
            && m["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "toolu_01"))
    });
    assert!(has_result, "request 2 has no tool_result block for toolu_01: {}", second.body);
    let file = support::scalar_bool(
        &rig.db,
        &format!(
            "SELECT count(*) = 1 AS v FROM memory_file WHERE org_id = '{}' AND agent_id = '{}' AND principal_id = '{}' AND path = '/a.md'",
            rig.f.org, rig.f.agent, rig.f.principal
        ),
    )
    .await;
    assert!(file, "the memory file /a.md is not stored in the scope");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_tool_block_notice_completes_the_run() {
    let notice = "[paigasus] tool call \"memory\" was blocked (policy).";
    let Some((_rig, got)) = class_case(vec![Reply::Sse(text_sse(notice))], completed_want(1)).await else {
        return;
    };
    assert_eq!(got.final_output.as_deref(), Some(notice));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pings_keep_a_slow_stream_alive() {
    let reply = Reply::Pings {
        sse: TEXT_STREAM.to_owned(),
        gap: Duration::from_millis(600),
        count: 4,
    };
    let _ = class_case(vec![reply], completed_want(1)).await;
}

// ---- one test for each error class ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_401_is_a_host_fault() {
    let Some((_rig, got)) = class_case(vec![error_reply(401, "authentication_error", "invalid x-api-key", None)], host_fault_want(1)).await else {
        return;
    };
    assert!(got.error.as_deref().is_some_and(|e| e.starts_with("the gateway rejected the agent host credential")), "{got:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_403_is_a_host_fault() {
    let Some((_rig, got)) = class_case(vec![error_reply(403, "permission_error", "not allowed", None)], host_fault_want(1)).await else {
        return;
    };
    assert!(got.error.as_deref().is_some_and(|e| e.starts_with("the gateway rejected the agent host credential")), "{got:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_404_is_a_host_fault() {
    let _ = class_case(vec![error_reply(404, "not_found_error", "no route", None)], host_fault_want(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_400_is_deterministic() {
    let _ = class_case(vec![error_reply(400, "invalid_request_error", "bad request", None)], deterministic_want()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_413_is_deterministic() {
    let _ = class_case(vec![error_reply(413, "request_too_large", "too large", None)], deterministic_want()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_prompt_is_deterministic_and_closes_the_thread() {
    let reply = Reply::Error {
        status: 400,
        body: PROMPT_TOO_LONG.to_owned(),
        retry_after: None,
    };
    let Some((rig, got)) = class_case(vec![reply], deterministic_want()).await else {
        return;
    };
    let closed = support::scalar_bool(&rig.db, &format!("SELECT closed AS v FROM thread WHERE id = '{}'", got.thread_id)).await;
    assert!(closed, "the thread is not closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_429_billing_error_is_a_host_fault_with_its_retry_after() {
    let Some((_rig, got)) = class_case(vec![error_reply(429, "billing_error", "budget used", Some(120))], host_fault_want(1)).await else {
        return;
    };
    assert!(got.not_before_secs >= 115.0, "not_before is {} s from now; {got:?}", got.not_before_secs);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_529_is_absorbed_by_the_in_run_retry() {
    let replies = vec![error_reply(529, "overloaded_error", "overloaded", None), Reply::Sse(TEXT_STREAM.to_owned())];
    let _ = class_case(replies, completed_want(2)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_503_on_every_attempt_is_a_host_fault() {
    let _ = class_case(vec![error_reply(503, "api_error", "unavailable", None)], host_fault_want(3)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_stream_is_a_host_fault() {
    // The first frame arrived, so `RetryingModel` does not retry: one request.
    let Some((_rig, got)) = class_case(vec![Reply::Stall(TEXT_STREAM.to_owned())], host_fault_want(1)).await else {
        return;
    };
    assert!(got.served_in < Duration::from_secs(10), "the run took {:?}", got.served_in);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_is_deterministic() {
    let _ = class_case(vec![Reply::Sse(with_stop_reason(TEXT_STREAM, "refusal"))], deterministic_want()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_status_is_transient() {
    let want = Want {
        run_status: "failed",
        error_class: Some("transient"),
        event_status: "pending",
        attempts: 1,
        requests: 1,
    };
    let _ = class_case(vec![error_reply(418, "api_error", "teapot", None)], want).await;
}

// ---- the redirect rule and the key ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_is_not_followed_and_is_a_host_fault() {
    let (second, accepted) = counting_listener().await;
    let reply = Reply::Redirect {
        location: format!("http://{second}/v1/messages"),
    };
    // A transport error is retried inside the run: 3 requests to the fake gateway.
    let _ = class_case(vec![reply], host_fault_want(3)).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "the agent host followed the redirect");
}

#[tokio::test]
async fn the_probe_does_not_follow_a_redirect() {
    let (second, accepted) = counting_listener().await;
    let fake = FakeGateway::start_with_models(
        vec![Reply::Sse(TEXT_STREAM.to_owned())],
        ModelsReply::Redirect {
            location: format!("http://{second}/v1/models"),
        },
    )
    .await;
    let (_models, probe, _dir) = register_on(fake.addr, 1000);
    assert_eq!(probe.run().await, ProbeOutcome::Unreachable);
    assert_eq!(fake.models_requests().len(), 1, "the probe did not reach the fake gateway");
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "the probe followed the redirect");
}

#[tokio::test]
async fn the_key_is_not_in_any_trace_line_of_a_401_run() {
    // `capture_logs` sees only this thread, so the test runs on the current-thread runtime.
    let (logs, _guard) = paigasus_logging::test_support::capture_logs();
    let Some(rig) = rig(vec![error_reply(401, "authentication_error", "invalid x-api-key", None)]).await else {
        eprintln!("skipping gateway_model_pg: Docker unavailable");
        return;
    };
    serve_once(&rig).await;
    let text = logs.text();
    assert!(text.contains("a host fault ended the run"), "positive control: no host fault line in\n{text}");
    assert!(!text.contains(KEY), "the key is in a log line:\n{text}");
}
