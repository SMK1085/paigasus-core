// SPDX-License-Identifier: Apache-2.0

//! SMA-753 design D8, spec P15: the history sum cap holds across two gateway replicas on one real
//! Redis. Each replica has its own `RedisSessionStore` client, `ToolHistory`, request side and
//! `CedarDecider`. Three concurrent holds of one `send` of 2,000 each run against
//! `sum_within(input.amount, 24h, Tool(send)) <= 5000` in the principal scope. The reserves run in
//! some order, so exactly two are allowed and one is blocked. The denied 2,000 also counts, so a
//! fourth call of 100 is blocked too (6,100). The second test runs the same case with one replica
//! clock 50 ms ahead. Raw reads exec `redis-cli` in the container: `repo:redis-connect-single-site`
//! bans a second Rust Redis client.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use paigasus_gateway::adapters::history::redis::history_key;
use paigasus_gateway::adapters::history::{HistoryKey, HmacHistoryDigester, RedisSessionStore};
use paigasus_gateway::application::decision_log::DecisionRecorder;
use paigasus_gateway::application::tool_history::ToolHistory;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::ToolPolicySettings;
use paigasus_gateway::domain::decision_log::OsEntropy;
use paigasus_gateway::domain::limits::Clock;
use paigasus_gateway::domain::tool_history::Scope;
use paigasus_gateway::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies};
use paigasus_gateway_core::{BlockCode, Decision, DecisionContext, Format, OnUnavailable, TenantMode, ToolCall, ToolCallDecider, ToolTurn, parse_body, parse_request};
use paigasus_proto::paigasus::iam::v1 as pb;
use secrecy::SecretString;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::core::ExecCommand;
use tokio::sync::Barrier;
use uuid::Uuid;

const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000c1";
const ORG: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000c1);
const ORG_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000c1";
/// The one tenant set: `send` is permitted only while the slot `p_cap` is true. The brief's text,
/// with the `@id` that the kernel compiler requires of every policy.
const TENANT: &str = r#"@id("cap") permit(principal, action == Pgs::Gateway::Action::"send", resource) when { context.p_cap };"#;
/// 24 hours, the window of `p_cap` and the window limit of both sets.
const DAY_MS: u64 = 86_400_000;
const BLOCK_POLICY: Decision = Decision::Block { code: BlockCode::Policy };

/// `p_cap`: the sum of `input.amount` over the calls of `send` of the principal, within 24 hours,
/// is at most 5000.
fn p_cap() -> pb::HistoryCondition {
    use pb::{history_condition, history_predicate};
    let predicate = pb::HistoryPredicate {
        node: Some(history_predicate::Node::Tool(pb::ToolMatch {
            name_key: "send".to_owned(),
            any_catalog_tool: false,
        })),
    };
    pb::HistoryCondition {
        slot: "p_cap".to_owned(),
        policy_id: "cap".to_owned(),
        scope: pb::HistoryScope::Principal as i32,
        window_ms: DAY_MS,
        shape: Some(history_condition::Shape::SumWithin(pb::SumWithinShape {
            predicate: Some(predicate),
            field_path: vec!["amount".to_owned()],
            op: pb::ComparisonOp::Le as i32,
            threshold: 5000,
        })),
    }
}

/// An empty floor, and `TENANT` with the condition `p_cap` for every org.
struct CapSource;

#[async_trait::async_trait]
impl PolicySource for CapSource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        Ok(PolicyText {
            text: String::new(),
            version: "f1".to_owned(),
            conditions: Vec::new(),
            max_window_ms: DAY_MS,
        })
    }

    async fn tenant(&self, _org: Uuid) -> Result<TenantPolicies, SourceError> {
        Ok(TenantPolicies {
            text: TENANT.to_owned(),
            catalog: vec!["send".to_owned()],
            version: "t1".to_owned(),
            settings: None,
            conditions: vec![p_cap()],
            max_window_ms: DAY_MS,
        })
    }
}

/// The system clock, moved by a fixed offset: the clock of one replica.
struct OffsetClock(Duration);

impl Clock for OffsetClock {
    fn now(&self) -> SystemTime {
        SystemTime::now() + self.0
    }
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

/// One gateway replica: its own Redis client, history, request side and decider.
struct Replica {
    history: Arc<ToolHistory>,
    policy: RequestToolPolicy,
    decider: CedarDecider,
}

impl Replica {
    async fn new(redis_url: &str, skew: Duration) -> Self {
        let store = Arc::new(RedisSessionStore::connect(redis_url).await.expect("connect to the test Redis"));
        let key = HistoryKey::new(SecretString::from("history-cap-test-key-of-32-bytes!".to_owned())).expect("a 32-byte key");
        let digester = HmacHistoryDigester::new(Some(key), None).expect("a digester");
        // A deadline far above any test load: a missed budget would make the slot missing.
        let history = Arc::new(ToolHistory::new(store, Arc::new(OffsetClock(skew)), Arc::new(OsEntropy), Duration::from_secs(10), Arc::new(digester)));
        let s = settings();
        let recorder = Arc::new(DecisionRecorder::tracing());
        let policy = RequestToolPolicy::new(
            PolicyCache::new(Arc::new(CapSource), CacheSettings::from_settings(&s), Arc::new(HmacHistoryDigester::keyless())),
            s,
            Arc::clone(&recorder),
        );
        let decider = CedarDecider::new(recorder).with_history(Arc::clone(&history));
        Self { history, policy, decider }
    }

    /// The request side of one chat request of the principal, linked to a new session.
    async fn request(&self) -> (DecisionContext, ToolTurn) {
        let body = bytes::Bytes::from(r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"send","parameters":{}}}]}"#);
        let turn = parse_request(Format::OpenAiChat, &body);
        let linked = self.history.link(PRINCIPAL, ORG, None, &turn).await.expect("a session");
        let ctx = DecisionContext::new(PRINCIPAL, ORG_PRN, None);
        let out = self.policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        assert!(out.snapshot.history.is_some(), "the snapshot carries the condition");
        (ctx.with_policy(out.snapshot).with_session(linked.session), turn)
    }

    /// The decision of one hold of one `send` of `amount`.
    async fn send(&self, ctx: DecisionContext, turn: ToolTurn, amount: i64) -> Decision {
        let decided = self.decider.decide(Arc::new(ctx), Arc::new(turn), send_call(amount)).await.expect("decide");
        assert_eq!(decided.len(), 1, "one decision per call");
        decided[0].decision
    }
}

/// The calls of a chat completion that calls `send` once with `amount`.
fn send_call(amount: i64) -> Vec<ToolCall> {
    let args = format!(r#"{{"amount":{amount}}}"#);
    let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": [
        {"id": "c-1", "type": "function", "function": {"name": "send", "arguments": args}}
    ]}, "finish_reason": "tool_calls"}]});
    parse_body(Format::OpenAiChat, body.to_string().as_bytes(), 1 << 20).expect("body").calls
}

/// Runs `redis-cli <args>` inside the container and returns its trimmed stdout.
async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let mut argv = vec!["redis-cli"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    let out = String::from_utf8(result.stdout_to_vec().await.expect("redis-cli stdout")).expect("redis-cli output is utf-8");
    assert_eq!(result.exit_code().await.expect("the exit code"), Some(0), "redis-cli failed: {out}");
    out.trim().to_string()
}

/// The case of design D8, with the clock of replica B `skew` ahead of the clock of replica A.
async fn the_cap_holds_on_two_replicas(skew: Duration, what: &str) {
    let Some((node, url)) = paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", what).await else {
        return;
    };
    let a = Arc::new(Replica::new(&url, Duration::ZERO).await);
    let b = Arc::new(Replica::new(&url, skew).await);
    // Three holds, spread over the two replicas: A, B, A. Each is its own request and session, so
    // only the principal history joins them. The request sides run first, so the holds start
    // together at the barrier.
    let mut holds = Vec::new();
    for replica in [&a, &b, &a] {
        let (ctx, turn) = replica.request().await;
        holds.push((Arc::clone(replica), ctx, turn));
    }
    let barrier = Arc::new(Barrier::new(holds.len()));
    let tasks: Vec<_> = holds
        .into_iter()
        .map(|(replica, ctx, turn)| {
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                replica.send(ctx, turn, 2000).await
            })
        })
        .collect();
    let mut decisions = Vec::new();
    for t in tasks {
        decisions.push(t.await.expect("the hold task"));
    }
    let allowed = decisions.iter().filter(|d| **d == Decision::Allow).count();
    let blocked = decisions.iter().filter(|d| **d == BLOCK_POLICY).count();
    assert_eq!((allowed, blocked), (2, 1), "exactly two holds pass the cap: {decisions:?}");

    // The denied 2,000 counts: 6,000 + 100 is over the cap, on the replica with the slower clock.
    let (ctx, turn) = a.request().await;
    assert_eq!(a.send(ctx, turn, 100).await, BLOCK_POLICY, "the denied call counts");

    // The principal history of `send` holds the four events, once each.
    let key = history_key(ORG, Scope::Principal, PRINCIPAL, "send");
    assert_eq!(redis_cli(&node, &["ZCARD", &key]).await, "4", "one event per call in {key}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_concurrent_sends_pass_the_cap_only_twice() {
    the_cap_holds_on_two_replicas(Duration::ZERO, "history_cap_redis no skew").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cap_holds_with_a_50_ms_clock_skew() {
    the_cap_holds_on_two_replicas(Duration::from_millis(50), "history_cap_redis 50 ms skew").await;
}
