// SPDX-License-Identifier: Apache-2.0

//! SMA-755 Task 12, § 5.13, § 6: the decision log through the real router, `build_decision_log`, the
//! writer, a real Redis and the relay, into a real JetStream. Redis is read only through `redis-cli`
//! in the container (`repo:redis-connect-single-site`). The policy source is a file directory.

mod support;

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use paigasus_gateway::adapters::decision_log::jetstream as dl_js;
use paigasus_gateway::adapters::decision_log::proto;
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::application::decision_log::DigestKey;
use paigasus_gateway::config::GatewayConfig;
use paigasus_gateway::domain::decision_log::OsEntropy;
use paigasus_gateway::domain::limits::SystemClock;
use paigasus_gateway::runtime::{self, DecisionLogRuntime};
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::BlockCode;
use paigasus_gateway_core::decision_log::trace::{Encoded, encode};
use paigasus_gateway_core::decision_log::{AppliedEffect, DecisionRecord, Prevention, RecordBasis, Side};
use paigasus_proto::paigasus::gateway::v1 as pb;
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::MockOpenAi;
use support::limits::*;
use tempfile::TempDir;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::core::ExecCommand;
use tower::ServiceExt;

const MIB: usize = 1 << 20;
const CANARY: &str = "s3cr3t-canary";
const CANARY_ARGS: &str = r#"{"secret":"s3cr3t-canary"}"#;
const DIGEST_KEY: &str = "0123456789abcdef0123456789abcdef-e2e";
const KEY: &str = "paigasus:gateway:decisions";
/// The floor forbids `floor_rm` in every mode.
const FLOOR: &str = "// SPDX-License-Identifier: Apache-2.0\n@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"floor_rm\", resource);\n";
/// The tenant set: permit every IAM principal, forbid `blocked_rm`.
const TENANT: &str = r#"@id("p") permit(principal is Pgs::Iam::Principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"blocked_rm", resource);"#;

/// A policy directory with the floor (when `floor`) and the tenant set of `ORG_A`.
fn policy_dir(floor: bool) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    if floor {
        std::fs::write(dir.path().join("floor.cedar"), FLOOR).expect("floor");
    }
    let t = dir.path().join("tenants").join(ORG_A);
    std::fs::create_dir_all(&t).expect("tenant dir");
    std::fs::write(t.join("policies.cedar"), TENANT).expect("tenant");
    dir
}

#[allow(clippy::result_large_err)]
fn config(toml: &str) -> GatewayConfig {
    let mut out = None;
    figment::Jail::expect_with(|jail| {
        jail.create_file("gateway.toml", toml)?;
        out = Some(GatewayConfig::figment().extract()?);
        Ok(())
    });
    out.expect("a config")
}

/// The gateway config of these tests: the tool policy on `dir` in `mode`, the decision log on.
fn gateway_config(dir: &TempDir, mode: &str, redis_url: &str, nats_url: &str) -> GatewayConfig {
    config(&format!(
        "[upstream.openai]\napi_key = \"sk-test-key\"\n[tool_interception]\nenabled = true\n\
         [tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\ndefault_mode = \"{mode}\"\nsource = {{ kind = \"file\", dir = \"{}\" }}\n\
         [decision_log]\nenabled = true\nredis_url = \"{redis_url}\"\ndigest_key = \"{DIGEST_KEY}\"\ndigest_key_id = \"k1\"\n\
         [decision_log.nats]\nurl = \"{nats_url}\"\n",
        dir.path().display()
    ))
}

struct Stack {
    redis: ContainerAsync<Redis>,
    _nats: ContainerAsync<Nats>,
    redis_url: String,
    nats_url: String,
    js: jetstream::Context,
}

/// A broker for the gateway binary, which makes the stream itself. JetStream reserves the full `max_bytes` of a
/// stream from the file store when it makes the stream. The binary sizes the stream from the ACCOUNT limit, which is
/// `MAX_BYTES` (18 GiB) when there is none, and a broker with no config derives its store from the free disk of the
/// host. On a small CI runner that create fails with 10047. So the store is fixed at 1 GiB, and the account `GW` has
/// a file limit of 256 MiB. `no_auth_user` gives the binary and the assertion client, which send no credentials, the
/// account `GW`.
const ACCOUNT_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1073741824 }
accounts {
  GW: {
    jetstream: { max_file: 268435456 }
    users: [ { user: "gw", password: "gw" } ]
  }
}
no_auth_user: gw
"#;

async fn stack(what: &str) -> Option<Stack> {
    let (redis, redis_url) = paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", what).await?;
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", ACCOUNT_CONF.as_bytes().to_vec());
    let nats = paigasus_test_docker::start_or_skip(image, what).await?;
    let port = paigasus_test_docker::mapped_port(&nats, 4222, what).await;
    let nats_url = format!("nats://127.0.0.1:{port}");
    let js = jetstream::new(async_nats::connect(&nats_url).await.expect("the assertion client connects"));
    Some(Stack {
        redis,
        _nats: nats,
        redis_url,
        nats_url,
        js,
    })
}

async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let mut argv = vec!["redis-cli"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    let out = result.stdout_to_vec().await.expect("redis-cli stdout");
    assert_eq!(result.exit_code().await.expect("redis-cli exit status"), Some(0), "redis-cli {args:?}");
    String::from_utf8(out).expect("utf-8").trim().to_string()
}

async fn xlen(node: &ContainerAsync<Redis>) -> u64 {
    redis_cli(node, &["XLEN", KEY]).await.parse().expect("an integer XLEN")
}

async fn wait_for<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The payloads of the stream, with their subjects. An empty list while the stream does not exist yet.
async fn published(js: &jetstream::Context) -> Vec<(String, Vec<u8>)> {
    let Ok(mut stream) = js.get_stream(dl_js::STREAM).await else {
        return Vec::new();
    };
    let last = stream.info().await.expect("stream info").state.last_sequence;
    let mut out = Vec::new();
    for seq in 1..=last {
        if let Ok(m) = stream.get_raw_message(seq).await {
            out.push((m.subject.to_string(), m.payload.to_vec()));
        }
    }
    out
}

fn decode(payload: &[u8]) -> DecisionRecord {
    proto::from_event(pb::ToolDecisionEvent::decode(payload).expect("a ToolDecisionEvent")).expect("a record")
}

// --- the router ------------------------------------------------------------------------------

/// The router over the tool policy and interception that `cfg` builds on the shared `recorder`.
fn app(base_url: &str, cfg: &GatewayConfig, recorder: &Arc<paigasus_gateway::application::decision_log::DecisionRecorder>) -> Router {
    let rt = runtime::build_tool_policy(cfg, Arc::clone(recorder), None).expect("the tool policy builds");
    let ti = runtime::tool_interception(cfg, &rt);
    let tp = rt.policy.map(|(_, tp)| tp);
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: true },
        limits: None,
        dpop_enabled: false,
        tool_interception: ti,
        tool_policy: tp,
        responses: Some(Arc::new(ResponsesSettings {
            max_request_bytes: 16 * MIB,
            max_record_bytes: 16 * MIB,
            max_held_bytes: 32 * MIB,
        })),
    })
}

async fn post_to(app: &Router, path: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(body.to_owned()))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let _ = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    status
}

fn chat_request(tools: &[&str]) -> String {
    let tools: Vec<Value> = tools.iter().map(|n| json!({"type":"function","function":{"name":n,"parameters":{}}})).collect();
    json!({"model":"gpt-4o-mini","stream":false,"messages":[{"role":"user","content":"hi"}],"tools":tools}).to_string()
}

fn chat_answer(calls: &[(&str, &str)]) -> String {
    let tool_calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (n, a))| json!({"id":format!("call_{i}"),"type":"function","function":{"name":n,"arguments":a}}))
        .collect();
    json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":tool_calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}).to_string()
}

fn responses_request(tools: &[&str]) -> String {
    let tools: Vec<Value> = tools.iter().map(|n| json!({"type":"function","name":n,"parameters":{}})).collect();
    json!({"model":"gpt-6","input":"hi","stream":false,"tools":tools}).to_string()
}

fn responses_answer(name: &str, args: &str, status: &str) -> String {
    json!({"id":"resp_1","object":"response","status":"completed","output":[
        {"type":"function_call","id":"fc_0","call_id":"c0","name":name,"arguments":args,"status":status}
    ],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}})
    .to_string()
}

async fn chat_exchange(cfg: &GatewayConfig, recorder: &Arc<paigasus_gateway::application::decision_log::DecisionRecorder>, tools: &[&str], calls: &[(&str, &str)]) {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, chat_answer(calls)).await;
    let status = post_to(&app(&mock.base_url, cfg, recorder), "/v1/chat/completions", &chat_request(tools)).await;
    assert_eq!(status, StatusCode::OK);
}

async fn boot(cfg: &GatewayConfig) -> DecisionLogRuntime {
    runtime::build_decision_log(cfg, Arc::new(SystemClock), Arc::new(OsEntropy)).await.expect("the decision log builds")
}

// --- tests -----------------------------------------------------------------------------------

#[tokio::test]
async fn a_blocked_call_through_the_router_reaches_gateway_decision_org() {
    let Some(stack) = stack("decision_log_e2e blocked call").await else {
        return;
    };
    let dir = policy_dir(true);
    let cfg = gateway_config(&dir, "enforce", &stack.redis_url, &stack.nats_url);
    cfg.validate().expect("the config validates");
    let dl = boot(&cfg).await;
    chat_exchange(&cfg, &dl.recorder, &["read_file"], &[("floor_rm", "{}")]).await;

    wait_for("one message on the stream", || async { !published(&stack.js).await.is_empty() }).await;
    wait_for("the Redis stream is empty", || async { xlen(&stack.redis).await == 0 }).await;
    let got = published(&stack.js).await;
    assert_eq!(got.len(), 1, "one decision, one message");
    assert_eq!(got[0].0, format!("gateway.decision.{ORG_A}"));
    let r = decode(&got[0].1);
    assert_eq!((r.side, r.applied, r.prevention), (Side::Response, AppliedEffect::Block, Prevention::Prevented));
    assert_eq!(r.tool.name, "floor_rm");
    let mut stream = stack.js.get_stream(dl_js::STREAM).await.expect("the stream exists");
    assert_eq!(
        stream.info().await.expect("stream info").config.max_bytes,
        dl_js::max_bytes_for(268_435_456),
        "the stream has 90 % of the account limit"
    );
    dl.tasks.expect("tasks").drain(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn the_tool_policy_off_writes_nothing() {
    let Some(stack) = stack("decision_log_e2e policy off").await else {
        return;
    };
    // This config is not validated: `validate` refuses a decision log with the tool policy off. The test
    // builds the log anyway, so that it proves that no call reaches it.
    let dir = policy_dir(true);
    let cfg = config(&format!(
        "[upstream.openai]\napi_key = \"sk-test-key\"\n[tool_interception]\nenabled = true\n\
         [decision_log]\nenabled = true\nredis_url = \"{}\"\ndigest_key = \"{DIGEST_KEY}\"\ndigest_key_id = \"k1\"\n[decision_log.nats]\nurl = \"{}\"\n",
        stack.redis_url, stack.nats_url
    ));
    let _keep = &dir;
    let dl = boot(&cfg).await;
    chat_exchange(&cfg, &dl.recorder, &["read_file"], &[("floor_rm", "{}"), ("read_file", "{}")]).await;
    // The relay makes the stream at its first event, so a stream that does not exist is also "nothing".
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(xlen(&stack.redis).await, 0, "nothing was written to Redis");
    assert!(published(&stack.js).await.is_empty(), "nothing reached gateway.decision.>");
    dl.tasks.expect("tasks").drain(Duration::from_secs(2)).await;
}

/// § 5.13. The canary is a tool-call argument on every path. It must be in no published payload, no trace
/// line, no captured log line and no metrics text. Paths through the router: allowed, blocked (enforce),
/// observed hold (log-only), unavailable (no floor file, fail closed) and truncated (a Responses call with
/// the item status `incomplete`). All five are reached through the router.
#[tokio::test]
async fn the_canary_never_leaves_the_gateway() {
    let Some(stack) = stack("decision_log_e2e canary").await else {
        return;
    };
    let metrics = paigasus_observability::init("paigasus-gateway");
    let (logs, _guard) = paigasus_logging::test_support::capture_logs();

    let good = policy_dir(true);
    let no_floor = policy_dir(false);
    let enforce = gateway_config(&good, "enforce", &stack.redis_url, &stack.nats_url);
    enforce.validate().expect("validates");
    let log_only = gateway_config(&good, "log_only", &stack.redis_url, &stack.nats_url);
    let unavailable = gateway_config(&no_floor, "enforce", &stack.redis_url, &stack.nats_url);
    let dl = boot(&enforce).await;
    let recorder = dl.recorder.clone();

    // allowed + blocked, with a floor-denied definition that the request side removes
    chat_exchange(&enforce, &recorder, &["read_file", "floor_rm"], &[("read_file", CANARY_ARGS), ("floor_rm", CANARY_ARGS)]).await;
    // an observed hold: a tenant-denied call under log-only
    chat_exchange(&log_only, &recorder, &["read_file"], &[("blocked_rm", CANARY_ARGS)]).await;
    // unavailable: no floor file, fail closed
    chat_exchange(&unavailable, &recorder, &["read_file"], &[("read_file", CANARY_ARGS)]).await;
    // truncated: a Responses call whose item is not completed
    let mock = MockOpenAi::spawn_json(StatusCode::OK, responses_answer("read_file", CANARY_ARGS, "incomplete")).await;
    let status = post_to(&app(&mock.base_url, &enforce, &recorder), "/v1/responses", &responses_request(&["read_file"])).await;
    assert_eq!(status, StatusCode::OK);

    // Every recorded decision is published.
    let counter = |name: &str| sample(&metrics.render(), name, &[]).unwrap_or(0.0);
    wait_for("every record published", || async {
        let recorded = counter("gateway_decision_log_recorded_total");
        recorded >= 6.0 && counter("gateway_decision_log_published_total") == recorded && xlen(&stack.redis).await == 0
    })
    .await;
    let recorded = counter("gateway_decision_log_recorded_total");
    let got = published(&stack.js).await;
    #[allow(clippy::cast_precision_loss)]
    let n = got.len() as f64;
    assert_eq!(n, recorded, "one message per record");

    let records: Vec<DecisionRecord> = got.iter().map(|(_, p)| decode(p)).collect();
    // each of the five paths is in the stream
    let find = |what: &str, f: &dyn Fn(&DecisionRecord) -> bool| records.iter().find(|r| f(r)).unwrap_or_else(|| panic!("no record for the {what} path: {records:#?}"));
    let allowed = find("allowed", &|r| {
        r.side == Side::Response && r.tool.name == "read_file" && r.applied == AppliedEffect::Allow && r.basis == RecordBasis::Policy
    });
    find("blocked", &|r| r.side == Side::Response && r.tool.name == "floor_rm" && r.applied == AppliedEffect::Block);
    find("observed hold", &|r| r.tool.name == "blocked_rm" && r.applied == AppliedEffect::Observe);
    find("unavailable", &|r| r.basis == RecordBasis::Unavailable);
    find("truncated", &|r| r.block_code == Some(BlockCode::Truncated));

    // The digest is keyed: it is not the plain SHA-256 of the arguments.
    let digest = allowed.arguments.as_ref().expect("the allowed record has a digest");
    let plain: [u8; 32] = Sha256::digest(CANARY_ARGS.as_bytes()).into();
    assert_ne!(digest.hmac_sha256, plain, "a keyed digest, never a plain hash");
    let key = DigestKey::new("k1".into(), DIGEST_KEY.to_owned().into()).expect("the key");
    assert_eq!(digest.hmac_sha256, key.digest(CANARY_ARGS.as_bytes()).hmac_sha256, "the HMAC of the arguments under the configured key");
    for r in &records {
        if let Some(d) = &r.arguments {
            assert_ne!(d.hmac_sha256, plain);
        }
    }

    // The canary is nowhere.
    for (_, payload) in &got {
        assert!(!payload.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()), "a published payload holds the canary");
    }
    let mut lines = 0;
    for r in &records {
        if let Encoded::Line(line) = encode(r) {
            lines += 1;
            assert!(!line.to_string().contains(CANARY), "a trace line holds the canary: {line}");
        }
    }
    assert!(lines >= 3, "the check is not vacuous: {lines} trace lines");
    let text = logs.text();
    assert!(text.contains("tool-policy decision"), "the log capture saw the decision lines: {text}");
    assert!(!text.contains(CANARY), "the tracing output holds the canary");
    let rendered = metrics.render();
    assert!(!rendered.contains(CANARY), "the metrics text holds the canary");
    assert!(sample(&rendered, "gateway_decision_log_published_total", &[]).is_some());
    dl.tasks.expect("tasks").drain(Duration::from_secs(2)).await;
}
