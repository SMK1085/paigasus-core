// SPDX-License-Identifier: Apache-2.0

//! SMA-752 § 6.3.5, § 7.3: the tool-policy NATS subscription against a real `nats:2.10.14`.
//!
//! The broker runs with JetStream and no auth: the permission set is proven by IAM's
//! `nats_permissions.rs` against the committed `ops/nats` fixture. These tests prove the client:
//! each replica gets each JetStream publish on `iam.policy.put` (Q1-A), a reconnect invalidates,
//! another subject does not, and a NATS outage never blocks a request.
//!
//! The counters are a positive signal. A test waits for a count to arrive and never passes by
//! waiting out a timeout.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use axum::http::StatusCode;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::nats::{PolicyEvents, connect};
use paigasus_gateway::adapters::policy_source::file::FilePolicySource;
use paigasus_gateway::application::tool_policy::{CacheSettings, CedarDecider, Invalidate, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::{ToolPolicyNatsConfig, ToolPolicySettings};
use paigasus_gateway_core::{Limits as CoreLimits, OnUnavailable, TenantMode};
use paigasus_observability::names;
use secrecy::SecretString;
use support::MockOpenAi;
use support::limits::{NON_STREAM_BODY, ORG_A, ORG_A_PRN, PRINCIPAL_1, ScopedIam, USAGE_BODY, app_with_tool_policy, sample, send};
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::watch;

/// How long a test waits for a count to arrive. The happy path returns as soon as it does.
const BUDGET: Duration = Duration::from_secs(10);

/// A `nats:2.10.14` broker (the module's pinned tag) with JetStream and the `IAM_EVENTS` stream
/// on `iam.>`, as `provision.sh` makes it. `None` when Docker is unavailable outside CI.
async fn start_nats() -> Option<(ContainerAsync<Nats>, String)> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let node = paigasus_test_docker::start_or_skip(Nats::default().with_cmd(&cmd), "tool_policy_nats").await?;
    let port = paigasus_test_docker::mapped_port(&node, 4222, "tool_policy_nats").await;
    let url = format!("nats://127.0.0.1:{port}");
    let js = jetstream::new(async_nats::connect(&url).await.expect("the test client connects"));
    js.create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("the stream is created");
    Some((node, url))
}

/// Counts `invalidate_all` calls.
#[derive(Default)]
struct Counting(AtomicUsize);

impl Counting {
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl Invalidate for Counting {
    fn invalidate_all(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn cfg(url: &str) -> ToolPolicyNatsConfig {
    ToolPolicyNatsConfig {
        url: SecretString::from(url.to_string()),
        creds_file: None,
        ca_file: None,
    }
}

/// Wait until `counter` holds at least `want`, then return its value.
async fn reach(counter: &Counting, want: usize, what: &str) -> usize {
    let deadline = tokio::time::Instant::now() + BUDGET;
    loop {
        let n = counter.get();
        if n >= want {
            return n;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}: the counter is {n}, want {want} within {BUDGET:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Connect a subscriber, start its `run` loop, and wait until the server has its subscription.
///
/// The first invalidation is the one of the first connect. `connect` registers the subscription
/// before it returns, so the flush (a PING/PONG round trip queued after the SUB) returns only when
/// the server has the SUB.
async fn subscriber(url: &str, shutdown: watch::Receiver<()>) -> (Arc<Counting>, async_nats::Client, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let counter = Arc::new(Counting::default());
    let events: PolicyEvents = connect(&cfg(url), Arc::clone(&counter) as Arc<dyn Invalidate>).await.expect("connect");
    let client = events.client().clone();
    let task = tokio::spawn(events.run(shutdown));
    reach(&counter, 1, "the first connect invalidates").await;
    client.flush().await.expect("flush");
    (counter, client, task)
}

/// One JetStream publish, acked by the stream.
async fn js_publish(url: &str, subject: &'static str) {
    let js = jetstream::new(async_nats::connect(url).await.expect("the publisher connects"));
    js.publish(subject, "{}".into()).await.expect("publish").await.expect("the stream acks the publish");
}

fn metric(name: &str, labels: &[(&str, &str)]) -> f64 {
    sample(&paigasus_observability::init("test-gateway-tool-policy-nats").render(), name, labels).unwrap_or(0.0)
}

/// § 7.3, Q1-A, D16: two replicas, one JetStream publish on `iam.policy.put`, and each replica
/// invalidates once more than its connect did. A shared durable would give the event to one only.
#[tokio::test]
async fn two_replicas_both_invalidate_on_one_jetstream_publish() {
    metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[]);
    let Some((_node, url)) = start_nats().await else { return };
    let (_tx, rx) = watch::channel(());
    let (a, _ca, _ta) = subscriber(&url, rx.clone()).await;
    let (b, _cb, _tb) = subscriber(&url, rx).await;
    let before = metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "message")]);

    js_publish(&url, "iam.policy.put").await;

    assert_eq!(reach(&a, 2, "replica a").await, 2);
    assert_eq!(reach(&b, 2, "replica b").await, 2);
    assert_eq!(metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "message")]) - before, 2.0);
}

/// § 6.3.5: a reconnect may have missed messages, so it invalidates, with `cause="connect"`.
#[tokio::test]
async fn a_reconnect_invalidates() {
    metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[]);
    let Some((_node, url)) = start_nats().await else { return };
    let (_tx, rx) = watch::channel(());
    let (counter, client, _task) = subscriber(&url, rx).await;
    let base = counter.get();
    let before = metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "connect")]);

    client.force_reconnect().await.expect("the reconnect is accepted");

    assert_eq!(reach(&counter, base + 1, "the reconnect").await, base + 1);
    assert_eq!(metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "connect")]) - before, 1.0);
    assert_eq!(metric(names::GATEWAY_TOOL_POLICY_NATS_CONNECTED, &[]), 1.0, "the gauge is up after the reconnect");

    // The subscription survives the reconnect: one publish after it is delivered.
    client.flush().await.expect("flush after the reconnect");
    js_publish(&url, "iam.policy.put").await;
    assert_eq!(reach(&counter, base + 2, "a publish after the reconnect").await, base + 2);
}

/// § 6.3.5: when the subscription ends while the client stays open, `run` subscribes again. Events
/// may have been missed in the gap, so the new subscription invalidates, with `cause="resubscribe"`.
/// Then one publish proves that the new subscription delivers.
#[tokio::test]
async fn a_resubscribe_invalidates_and_delivers() {
    metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[]);
    let Some((_node, url)) = start_nats().await else { return };
    let (_tx, rx) = watch::channel(());
    let counter = Arc::new(Counting::default());
    let mut events: PolicyEvents = connect(&cfg(&url), Arc::clone(&counter) as Arc<dyn Invalidate>).await.expect("connect");
    let client = events.client().clone();
    reach(&counter, 1, "the first connect invalidates").await;
    let base = counter.get();
    let before = metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "resubscribe")]);

    events.end_subscription().await.expect("the unsubscribe is sent");
    let _task = tokio::spawn(events.run(rx));

    // The loop waits RESUBSCRIBE_DELAY (5 s) before it subscribes again; BUDGET is 10 s.
    assert_eq!(reach(&counter, base + 1, "the resubscribe").await, base + 1);
    assert_eq!(metric(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, &[("cause", "resubscribe")]) - before, 1.0);

    client.flush().await.expect("flush after the resubscribe");
    js_publish(&url, "iam.policy.put").await;
    assert_eq!(reach(&counter, base + 2, "a publish after the resubscribe").await, base + 2);
}

/// D16 reads only `iam.policy.>`. A role grant is published first and a policy event after it, on
/// the same connection. The policy event arrives, so the earlier role grant had its chance; the
/// count grew by exactly one.
#[tokio::test]
async fn a_non_policy_subject_does_not_invalidate() {
    let Some((_node, url)) = start_nats().await else { return };
    let (_tx, rx) = watch::channel(());
    let (counter, _client, _task) = subscriber(&url, rx).await;
    let base = counter.get();

    let js = jetstream::new(async_nats::connect(&url).await.expect("the publisher connects"));
    js.publish("iam.role.granted", "{}".into()).await.expect("publish").await.expect("ack");
    js.publish("iam.policy.put", "{}".into()).await.expect("publish").await.expect("ack");

    assert_eq!(reach(&counter, base + 1, "the policy event").await, base + 1);
    // A second message would land in the same order it was sent. Give the loop a moment anyway.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.get(), base + 1, "iam.role.granted must not invalidate");
}

/// § 6.3.5: "A NATS outage never blocks a request." `connect` to a closed port returns at once
/// (`retry_on_initial_connect`), and a request with the tool policy on is answered. No Docker.
#[tokio::test]
async fn nats_down_never_blocks_a_request() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    };
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("floor.cedar"), "// SPDX-License-Identifier: Apache-2.0\n").expect("floor");
    let tenant = dir.path().join("tenants").join(ORG_A);
    std::fs::create_dir_all(&tenant).expect("tenant dir");
    std::fs::write(tenant.join("policies.cedar"), "@id(\"p\") permit(principal, action, resource);").expect("tenant");
    let settings = ToolPolicySettings {
        enabled: true,
        default_mode: TenantMode::Enforce,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(60),
        orgs: std::collections::HashMap::new(),
        settings_from_source: false,
    };
    let cache = PolicyCache::new(Arc::new(FilePolicySource::new(dir.path())), CacheSettings::from_settings(&settings));

    let events = tokio::time::timeout(Duration::from_secs(5), connect(&cfg(&format!("nats://127.0.0.1:{closed}")), Arc::clone(&cache) as Arc<dyn Invalidate>))
        .await
        .expect("connect does not wait for the broker")
        .expect("connect returns a client while the broker is down");
    let (_tx, rx) = watch::channel(());
    let _task = tokio::spawn(events.run(rx));

    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let ti = Some(Arc::new(ToolInterception {
        limits: CoreLimits::defaults(),
        decider: Arc::new(CedarDecider),
    }));
    let app = app_with_tool_policy(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, ti, Some(Arc::new(RequestToolPolicy::new(cache, settings))));
    let sent = tokio::time::timeout(Duration::from_secs(10), send(&app, NON_STREAM_BODY)).await.expect("the request is answered");
    assert_eq!(sent.status, StatusCode::OK);
}
