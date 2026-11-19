// SPDX-License-Identifier: Apache-2.0

//! The decision-log writer against a real Redis (SMA-755 § 5.6, § 5.8). Redis is read only through
//! `redis-cli` in the container, as the limit-store tests do: `repo:redis-connect-single-site` bans a second
//! Rust Redis client. A test that needs only a dead or silent Redis uses the `paigasus-redis` test support
//! and needs no container.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::decision_log::jetstream as dl_js;
use paigasus_gateway::adapters::decision_log::redis_conn::breaker_metrics;
use paigasus_gateway::adapters::decision_log::relay::delete_consumer_if_empty;
use paigasus_gateway::adapters::decision_log::{RedisDecisionLog, RedisSlot, RelaySettings, WriterSettings, connect_loop, proto, run_relay, run_writer};
use paigasus_gateway::config::DecisionLogNatsConfig;
use paigasus_gateway::domain::decision_log::DecisionLog;
use paigasus_gateway_core::decision_log::{DecisionRecord, RecordHeader, ReportedCall, build_response};
use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
use prost::Message;
use secrecy::SecretString;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::core::{ExecCommand, IntoContainerPort};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

const KEY: &str = "paigasus:test:decisions";
const ORG_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";

fn record(seq: u64) -> DecisionRecord {
    let entry = ReportEntry {
        choice: 0,
        position: 0,
        name: "Get_Weather".into(),
        decision: ReportDecision::Block,
        basis: Some(Basis::Decider),
        code: Some(BlockCode::Policy),
        enforced: Some(true),
        reason: None,
    };
    let ctx = DecisionContext::new("prn:pgs:iam:::user/u", ORG_PRN, Some("req-1".into()));
    let header = RecordHeader::new(
        Uuid::from_u128(u128::from(seq) + 1),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        Uuid::from_u128(7),
        seq,
        None,
    );
    build_response(header, &ctx, &ReportedCall::new(entry, None, None))
}

fn encoded(seq: u64) -> Vec<u8> {
    proto::to_event(&record(seq)).encode_to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

type SeriesKey = (String, Vec<(String, String)>);

/// A running total of the counters and the histogram counts, and the last value of each gauge.
/// `Snapshotter::snapshot` resets each counter it reads, so a poll must add each snapshot to a total.
struct Tally {
    snapshotter: Snapshotter,
    totals: RefCell<HashMap<SeriesKey, u64>>,
    gauges: RefCell<HashMap<SeriesKey, f64>>,
}

impl Tally {
    fn new(snapshotter: Snapshotter) -> Self {
        Tally {
            snapshotter,
            totals: RefCell::new(HashMap::new()),
            gauges: RefCell::new(HashMap::new()),
        }
    }

    fn poll(&self) {
        let mut totals = self.totals.borrow_mut();
        let mut gauges = self.gauges.borrow_mut();
        for (key, _, _, value) in self.snapshotter.snapshot().into_vec() {
            let key = key.key();
            let mut have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
            have.sort();
            let id = (key.name().to_owned(), have);
            match value {
                DebugValue::Counter(n) => *totals.entry(id).or_insert(0) += n,
                DebugValue::Histogram(samples) => *totals.entry(id).or_insert(0) += samples.len() as u64,
                DebugValue::Gauge(v) => {
                    gauges.insert(id, v.into_inner());
                }
            }
        }
    }

    fn get(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        self.poll();
        let mut want: Vec<(String, String)> = labels.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        want.sort();
        self.totals.borrow().get(&(name.to_owned(), want)).copied().unwrap_or(0)
    }

    /// The last value of the gauge `name`, or `None` before its first set.
    fn gauge(&self, name: &str) -> Option<f64> {
        self.poll();
        self.gauges.borrow().get(&(name.to_owned(), Vec::new())).copied()
    }

    fn dropped(&self, reason: &str) -> u64 {
        self.get("gateway_decision_log_dropped_total", &[("reason", reason)])
    }

    fn written(&self) -> u64 {
        self.get("gateway_decision_log_written_total", &[])
    }

    fn published(&self) -> u64 {
        self.get("gateway_decision_log_published_total", &[])
    }
}

async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let mut argv = vec!["redis-cli"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    let out = result.stdout_to_vec().await.expect("redis-cli stdout");
    let code = result.exit_code().await.expect("redis-cli exit status");
    assert_eq!(code, Some(0), "redis-cli {args:?} failed (exit {code:?})");
    String::from_utf8(out).expect("redis-cli output is utf-8").trim().to_string()
}

async fn xlen(node: &ContainerAsync<Redis>) -> u64 {
    redis_cli(node, &["XLEN", KEY]).await.parse().expect("an integer XLEN")
}

/// Poll `check` until it holds, or fail after 30 s.
async fn wait_for<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn settings(max_len: u64) -> WriterSettings {
    WriterSettings { key: KEY.to_owned(), max_len }
}

async fn redis_7_4(what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", what).await
}

async fn connected_slot(url: &str) -> RedisSlot {
    let slot = RedisSlot::new();
    slot.set(paigasus_redis::connect(url, breaker_metrics()).await.expect("connect to the test Redis"));
    slot
}

struct Writer {
    stop: watch::Sender<()>,
    task: JoinHandle<()>,
}

impl Writer {
    fn start(rx: tokio::sync::mpsc::Receiver<Bytes>, slot: RedisSlot, max_len: u64, budget: Duration) -> Writer {
        let (stop, shutdown) = watch::channel(());
        let task = tokio::spawn(run_writer(rx, slot, settings(max_len), shutdown, budget));
        Writer { stop, task }
    }

    async fn shut_down(self) {
        self.stop.send(()).expect("the writer is alive");
        self.task.await.expect("the writer task ends");
    }
}

#[tokio::test]
async fn n_records_give_n_stream_entries_with_the_original_bytes() {
    let Some((node, url)) = redis_7_4("decision_log_redis entries").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (log, rx) = RedisDecisionLog::new(64);
    let writer = Writer::start(rx, connected_slot(&url).await, 1000, Duration::from_secs(2));
    for seq in 1..=5 {
        log.record(record(seq));
    }
    wait_for("five stream entries", || async { xlen(&node).await == 5 }).await;
    // Redis prints each entry as `e:<hex of the bytes>`, so that the comparison is byte exact.
    let lua = "local out = {} for i, e in ipairs(redis.call('XRANGE', KEYS[1], '-', '+')) do \
               out[i] = e[2][1] .. ':' .. (e[2][2]:gsub('.', function(c) return string.format('%02x', string.byte(c)) end)) end \
               return table.concat(out, ' ')";
    let got = redis_cli(&node, &["EVAL", lua, "1", KEY]).await;
    let want: Vec<String> = (1..=5).map(|seq| format!("e:{}", hex(&encoded(seq)))).collect();
    assert_eq!(got, want.join(" "));
    assert_eq!(snap.written(), 5);
    writer.shut_down().await;
}

#[tokio::test]
async fn redis_down_at_boot_counts_redis_unavailable_then_writes() {
    // Reserve a port that nothing listens on, point the connect loop at it, and start Redis there later.
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a probe port");
        probe.local_addr().expect("the probe address").port()
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let slot = RedisSlot::new();
    let (stop, shutdown) = watch::channel(());
    let connect = tokio::spawn(connect_loop(SecretString::from(format!("redis://127.0.0.1:{port}")), slot.clone(), shutdown.clone()));
    let (log, rx) = RedisDecisionLog::new(64);
    let writer = Writer::start(rx, slot.clone(), 1000, Duration::from_secs(2));
    for seq in 1..=3 {
        log.record(record(seq));
    }
    wait_for("three redis_unavailable drops", || async { snap.dropped("redis_unavailable") == 3 }).await;
    assert!(slot.get().is_none(), "Redis is not up yet");

    let image = Redis::default().with_tag("7.4-alpine").with_mapped_port(port, 6379.tcp());
    let Some(node) = paigasus_test_docker::start_or_skip(image, "decision_log_redis down at boot").await else {
        stop.send(()).expect("alive");
        connect.await.expect("connect loop ends");
        writer.shut_down().await;
        return;
    };
    wait_for("the connect loop connects", || async { slot.get().is_some() }).await;
    for seq in 4..=5 {
        log.record(record(seq));
    }
    wait_for("two events written", || async { snap.written() == 2 }).await;
    assert_eq!(xlen(&node).await, 2);
    assert_eq!(snap.dropped("redis_unavailable"), 3);
    stop.send(()).expect("alive");
    connect.await.expect("connect loop ends");
    writer.shut_down().await;
}

#[tokio::test]
async fn an_open_breaker_counts_breaker_open() {
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let handle = paigasus_redis::with_open_breaker_for_tests("redis://127.0.0.1:1", breaker_metrics()).expect("a lazy handle");
    let slot = RedisSlot::new();
    slot.set(handle);
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=3 {
        log.record(record(seq));
    }
    let writer = Writer::start(rx, slot, 1000, Duration::from_secs(2));
    wait_for("three breaker_open drops", || async { snap.dropped("breaker_open") == 3 }).await;
    assert_eq!(snap.written(), 0);
    assert_eq!(snap.dropped("write_error"), 0);
    writer.shut_down().await;
}

#[tokio::test]
async fn a_silent_redis_counts_write_error() {
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let blackhole = paigasus_redis::test_support::start().await;
    let slot = RedisSlot::new();
    slot.set(paigasus_redis::new_lazy_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=3 {
        log.record(record(seq));
    }
    // The three events are queued before the writer starts, so they form one batch.
    let writer = Writer::start(rx, slot, 1000, Duration::from_secs(2));
    wait_for("three write_error drops", || async { snap.dropped("write_error") == 3 }).await;
    assert_eq!(snap.written(), 0);
    writer.shut_down().await;
}

#[tokio::test]
async fn a_backlog_at_max_len_drops_each_new_event() {
    let Some((node, url)) = redis_7_4("decision_log_redis backlog").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=5 {
        log.record(record(seq));
    }
    let writer = Writer::start(rx, connected_slot(&url).await, 3, Duration::from_secs(2));
    wait_for("three events written and two dropped", || async { snap.written() == 3 && snap.dropped("backlog_full") == 2 }).await;
    assert_eq!(xlen(&node).await, 3);
    // No relay deletes entries: a later event is dropped at once, and the stream stays at 3.
    log.record(record(6));
    wait_for("a third backlog_full drop", || async { snap.dropped("backlog_full") == 3 }).await;
    assert_eq!(xlen(&node).await, 3);
    writer.shut_down().await;
}

#[tokio::test]
async fn shutdown_drains_then_counts_the_rest_as_shutdown() {
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let blackhole = paigasus_redis::test_support::start().await;
    let slot = RedisSlot::new();
    slot.set(paigasus_redis::new_lazy_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=4 {
        log.record(record(seq));
    }
    // The signal comes before the writer starts and the budget is 0: every queued event is a shutdown drop.
    let (stop, shutdown) = watch::channel(());
    stop.send(()).expect("a receiver is alive");
    run_writer(rx, slot, settings(1000), shutdown, Duration::ZERO).await;
    assert_eq!(snap.dropped("shutdown"), 4);
    assert_eq!(snap.written(), 0);
    log.record(record(5));
    assert_eq!(snap.dropped("closed"), 1, "the queue is closed after the shutdown");
}

#[tokio::test]
async fn shutdown_with_a_budget_writes_what_is_queued() {
    let Some((node, url)) = redis_7_4("decision_log_redis drain").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=3 {
        log.record(record(seq));
    }
    let (stop, shutdown) = watch::channel(());
    stop.send(()).expect("a receiver is alive");
    run_writer(rx, connected_slot(&url).await, settings(1000), shutdown, Duration::from_secs(5)).await;
    assert_eq!(xlen(&node).await, 3);
    assert_eq!(snap.written(), 3);
    assert_eq!(snap.dropped("shutdown"), 0);
}

#[tokio::test]
async fn shutdown_covers_the_in_flight_write_and_counts_it_with_the_queue() {
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // A Redis that accepts and never answers: the first write stays in flight.
    let blackhole = paigasus_redis::test_support::start().await;
    let slot = RedisSlot::new();
    slot.set(paigasus_redis::new_lazy_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
    let (log, rx) = RedisDecisionLog::new(64);
    for seq in 1..=2 {
        log.record(record(seq));
    }
    let budget = Duration::from_millis(300);
    let (stop, shutdown) = watch::channel(());
    let task = tokio::spawn(run_writer(rx, slot, settings(1000), shutdown, budget));
    wait_for("the writer dials Redis", || async { blackhole.accepted() >= 1 }).await;
    // Three more events wait in the queue behind the write in flight.
    for seq in 3..=5 {
        log.record(record(seq));
    }
    stop.send(()).expect("the writer is alive");
    // The runtime aborts the task at budget + 500 ms after the signal. The writer must return before that.
    tokio::time::timeout(budget + Duration::from_millis(500), task)
        .await
        .expect("the writer returns inside the runtime grace")
        .expect("the writer task ends");
    assert_eq!(snap.dropped("shutdown"), 5, "the batch in flight and the three queued events count as shutdown");
    assert_eq!(snap.written(), 0);
}

// ---- The relay (SMA-755 Task 11, § 5.7, § 5.8) ----

const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
const ORG_B: &str = "0190a100-0000-7000-8000-0000000000b2";

fn event_with_org(seq: u64, org: &str) -> Vec<u8> {
    let mut e = proto::to_event(&record(seq));
    e.org_id = org.to_owned();
    e.encode_to_vec()
}

fn decision_id(seq: u64) -> String {
    Uuid::from_u128(u128::from(seq) + 1).to_string()
}

/// Add `bytes` (and `pad` zero bytes after them) to the stream through a Lua script, so that binary bytes pass
/// through `redis-cli`. Returns the entry id.
async fn xadd(node: &ContainerAsync<Redis>, bytes: &[u8], pad: usize) -> String {
    let lua = "local b = ARGV[1]:gsub('..', function(c) return string.char(tonumber(c, 16)) end) .. string.rep(string.char(0), tonumber(ARGV[2])) \
               return redis.call('XADD', KEYS[1], '*', 'e', b)";
    redis_cli(node, &["EVAL", lua, "1", KEY, &hex(bytes), &pad.to_string()]).await
}

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

async fn start_nats(port: Option<u16>, what: &str) -> Option<(ContainerAsync<Nats>, String)> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let image = Nats::default().with_cmd(&cmd);
    let node = match port {
        Some(p) => paigasus_test_docker::start_or_skip(image.with_mapped_port(p, 4222.tcp()), what).await?,
        None => paigasus_test_docker::start_or_skip(image, what).await?,
    };
    let port = paigasus_test_docker::mapped_port(&node, 4222, what).await;
    Some((node, format!("nats://127.0.0.1:{port}")))
}

fn free_port() -> u16 {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a probe port");
    probe.local_addr().expect("the probe address").port()
}

async fn relay_js(url: &str, violations: Arc<AtomicU64>) -> jetstream::Context {
    let cfg = DecisionLogNatsConfig {
        url: SecretString::from(url.to_owned()),
        creds_file: None,
        ca_file: None,
    };
    dl_js::connect(&cfg, violations).await.expect("the relay NATS client is made")
}

fn fast(consumer: &str) -> RelaySettings {
    let mut s = RelaySettings::new(KEY.to_owned(), consumer.to_owned());
    s.claim_idle = Duration::from_millis(300);
    s.claim_every = Duration::from_millis(100);
    s.sample_every = Duration::from_millis(200);
    s.idle_sleep = Duration::from_millis(20);
    s.backoff_start = Duration::from_millis(200);
    s.backoff_max = Duration::from_secs(1);
    s
}

struct Relay {
    stop: watch::Sender<()>,
    task: JoinHandle<()>,
}

impl Relay {
    fn start(slot: RedisSlot, js: jetstream::Context, s: RelaySettings, violations: Arc<AtomicU64>) -> Relay {
        let (stop, shutdown) = watch::channel(());
        let task = tokio::spawn(run_relay(slot, js, s, violations, shutdown, Duration::from_secs(2)));
        Relay { stop, task }
    }

    async fn shut_down(self) {
        self.stop.send(()).expect("the relay is alive");
        self.task.await.expect("the relay task ends");
    }
}

/// One message of the stream.
struct Published {
    subject: String,
    msg_id: String,
    payload: Vec<u8>,
}

async fn stream_messages(js: &jetstream::Context) -> Vec<Published> {
    let mut stream = js.get_stream(dl_js::STREAM).await.expect("the stream exists");
    let last = stream.info().await.expect("stream info").state.last_sequence;
    let mut out = Vec::new();
    for seq in 1..=last {
        if let Ok(m) = stream.get_raw_message(seq).await {
            out.push(Published {
                subject: m.subject.to_string(),
                msg_id: m.headers.get("Nats-Msg-Id").map(|v| v.as_str().to_owned()).unwrap_or_default(),
                payload: m.payload.to_vec(),
            });
        }
    }
    out
}

/// A client of its own for the assertions.
async fn test_js(url: &str) -> jetstream::Context {
    jetstream::new(async_nats::connect(url).await.expect("the assertion client connects"))
}

#[tokio::test]
async fn n_records_give_n_messages_with_the_right_subjects_and_bytes() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay messages").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay messages").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let want = [(1, ORG_A), (2, ORG_B), (3, ""), (4, ORG_A)];
    for (seq, org) in want {
        xadd(&node, &event_with_org(seq, org), 0).await;
    }
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("four events published", || async { snap.published() == 4 && xlen(&node).await == 0 }).await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    assert_eq!(got.len(), 4);
    for (seq, org) in want {
        let m = got.iter().find(|m| m.msg_id == decision_id(seq)).unwrap_or_else(|| panic!("no message for record {seq}"));
        let subject = if org.is_empty() { "gateway.decision._none".to_owned() } else { format!("gateway.decision.{org}") };
        assert_eq!(m.subject, subject);
        assert_eq!(m.payload, event_with_org(seq, org), "the payload is the stream bytes");
    }
    assert!(snap.get("gateway_decision_log_publish_seconds", &[]) >= 4, "the publish time is recorded for each event");
    assert_eq!(snap.dropped("publish_rejected"), 0);
    relay.shut_down().await;
}

#[tokio::test]
async fn nats_down_keeps_the_events_in_redis_until_it_returns() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay nats down").await else {
        return;
    };
    let port = free_port();
    let Some((nats, nats_url)) = start_nats(Some(port), "decision_log_redis relay nats down").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    nats.stop().await.expect("stop NATS");
    for seq in 1..=4 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(xlen(&node).await, 4, "the events stay in Redis while NATS is down");
    assert_eq!(snap.published(), 0);
    assert_eq!(snap.dropped("publish_rejected"), 0, "a transport failure is not a rejection");
    nats.start().await.expect("start NATS");
    wait_for("all four events published", || async { snap.published() == 4 && xlen(&node).await == 0 }).await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    let mut ids: Vec<_> = got.iter().map(|m| m.msg_id.clone()).collect();
    ids.sort();
    let mut want: Vec<_> = (1..=4).map(decision_id).collect();
    want.sort();
    assert_eq!(ids, want, "each event once");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_deleted_stream_is_transient_and_the_relay_makes_it_again() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay stream deleted").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay stream deleted").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    // The first event makes the relay create the stream.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    wait_for("the first event is published", || async { snap.published() == 1 && xlen(&node).await == 0 }).await;
    // The stream goes away while the relay runs. A JetStream that is not ready answers the same way (R9).
    let js = test_js(&nats_url).await;
    js.delete_stream(dl_js::STREAM).await.expect("delete the stream");
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    // The relay makes the stream again and publishes the entry. It never counts the entry as rejected.
    wait_for("the second event is published after the stream is made again", || async {
        snap.published() == 2 && xlen(&node).await == 0
    })
    .await;
    assert_eq!(snap.dropped("publish_rejected"), 0, "a missing stream is not a rejection");
    let got = stream_messages(&js).await;
    assert!(got.iter().any(|m| m.msg_id == decision_id(2)), "the event that met the missing stream is on the new stream");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_poison_entry_is_rejected_and_the_next_publishes() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay poison").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay poison").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // A valid event plus an unknown length-delimited field (number 98) of 70 KiB: above the 64 KiB stream limit.
    let pad = 70 * 1024;
    let mut poison = event_with_org(1, ORG_A);
    poison.extend(varint((98 << 3) | 2));
    poison.extend(varint(pad as u64));
    xadd(&node, &poison, pad).await;
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    xadd(&node, &event_with_org(3, ORG_A), 0).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("one rejection and two publishes", || async {
        snap.dropped("publish_rejected") == 1 && snap.published() == 2 && xlen(&node).await == 0
    })
    .await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|m| m.msg_id != decision_id(1)), "the poison event is not in the stream");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_non_canonical_org_id_is_rejected_and_never_published() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay org").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay org").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    for (seq, org) in [(1, "0190A100-0000-7000-8000-0000000000A1"), (2, "{0190a100-0000-7000-8000-0000000000a1}"), (3, "a.b"), (4, "*")] {
        xadd(&node, &event_with_org(seq, org), 0).await;
    }
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("four rejections", || async { snap.dropped("publish_rejected") == 4 && xlen(&node).await == 0 }).await;
    assert_eq!(snap.published(), 0);
    assert!(stream_messages(&test_js(&nats_url).await).await.is_empty(), "no message reached gateway.decision.>");
    relay.shut_down().await;
}

#[tokio::test]
async fn an_unknown_field_survives_the_relay_byte_for_byte() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay unknown field").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay unknown field").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // Field 99, varint 1: a field that a newer gateway could add.
    let mut bytes = event_with_org(1, ORG_A);
    bytes.extend(varint(99 << 3));
    bytes.extend(varint(1));
    xadd(&node, &bytes, 0).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("one publish", || async { snap.published() == 1 }).await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].payload, bytes, "the published payload is the stream bytes, with the unknown field");
    relay.shut_down().await;
}

#[tokio::test]
async fn flushall_with_two_relays_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay flushall").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // The entries are in Redis before the relays start, so that the first XLEN sample of each relay is 3. NATS
    // is unreachable, so that no entry is published and deleted. A long sample interval keeps that sample the
    // last one until the flush.
    for seq in 1..=3 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let dead = format!("nats://127.0.0.1:{}", free_port());
    let mut relays = Vec::new();
    for name in ["i1", "i2"] {
        let v = Arc::new(AtomicU64::new(0));
        let mut s = fast(name);
        s.sample_every = Duration::from_secs(120);
        relays.push(Relay::start(connected_slot(&redis_url).await, relay_js(&dead, Arc::clone(&v)).await, s, v));
    }
    wait_for("the backlog gauge reads 3", || async { snap.gauge("gateway_decision_log_backlog") == Some(3.0) }).await;
    redis_cli(&node, &["FLUSHALL"]).await;
    wait_for("one reset", || async { snap.get("gateway_decision_log_redis_resets_total", &[]) >= 1 }).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(snap.get("gateway_decision_log_redis_resets_total", &[]), 1, "two relays count one incident");
    assert_eq!(snap.get("gateway_decision_log_lost_estimate_total", &[]), 3, "the estimate is the last XLEN sample");
    for r in relays {
        r.shut_down().await;
    }
}

/// Make the group and read `n` entries as a consumer that dies before it acks.
async fn read_as_dead_instance(node: &ContainerAsync<Redis>, n: u64) -> Vec<String> {
    redis_cli(node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    let mut ids = Vec::new();
    for seq in 1..=n {
        ids.push(xadd(node, &event_with_org(seq, ORG_A), 0).await);
    }
    // The entries hold binary bytes, so the script returns the ids only.
    let lua = "local r = redis.call('XREADGROUP', 'GROUP', 'relay', 'dead', 'COUNT', 10, 'STREAMS', KEYS[1], '>') \
               local out = {} for i, e in ipairs(r[1][2]) do out[i] = e[1] end return table.concat(out, ' ')";
    let read = redis_cli(node, &["EVAL", lua, "1", KEY]).await;
    assert_eq!(read, ids.join(" "), "the dead instance read every entry");
    ids
}

#[tokio::test]
async fn a_dead_instance_pending_entries_are_claimed_once() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay claim").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay claim").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    read_as_dead_instance(&node, 3).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("b"), v);
    wait_for("three claimed entries published", || async { snap.published() == 3 && xlen(&node).await == 0 }).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    let mut ids: Vec<_> = got.iter().map(|m| m.msg_id.clone()).collect();
    ids.sort();
    let mut want: Vec<_> = (1..=3).map(decision_id).collect();
    want.sort();
    assert_eq!(ids, want, "each entry once, by Nats-Msg-Id");
    assert_eq!(snap.published(), 3);
    relay.shut_down().await;
}

#[tokio::test]
async fn a_deleted_pending_id_counts_redis_deleted() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay deleted").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay deleted").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let ids = read_as_dead_instance(&node, 2).await;
    redis_cli(&node, &["XDEL", KEY, &ids[0]]).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("b"), v);
    wait_for("one deleted id and one publish", || async { snap.dropped("redis_deleted") == 1 && snap.published() == 1 }).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(snap.dropped("redis_deleted"), 1, "the deleted id counts once");
    assert_eq!(snap.published(), 1);
    relay.shut_down().await;
}

#[tokio::test]
async fn the_relay_creates_a_bounded_stream_and_warns_on_drift() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay stream").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay stream").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;

    // No stream: the relay makes one with the expected config.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("one publish", || async { snap.published() == 1 }).await;
    let mut stream = admin.get_stream(dl_js::STREAM).await.expect("the relay made the stream");
    let live = stream.info().await.expect("stream info").config.clone();
    let want = dl_js::stream_config();
    assert_eq!(live.max_bytes, want.max_bytes);
    assert_eq!(live.max_age, want.max_age);
    assert_eq!(live.max_message_size, want.max_message_size);
    assert_eq!(live.duplicate_window, want.duplicate_window);
    assert_eq!(live.discard, want.discard);
    assert!(live.max_bytes > 0, "the stream is bounded");
    assert!(!logs.text().contains("differs from the expected config"), "no drift warning for a matching stream");
    relay.shut_down().await;

    // A stream with no byte limit: the relay warns and does not change it.
    admin.delete_stream(dl_js::STREAM).await.expect("delete the stream");
    let mut drifted = dl_js::stream_config();
    drifted.max_bytes = -1;
    admin.create_stream(drifted).await.expect("make the drifted stream");
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i2"), v);
    wait_for("the second publish", || async { snap.published() == 2 }).await;
    let text = logs.text();
    assert!(
        text.contains("differs from the expected config") && text.contains("max_bytes"),
        "the drift warning names max_bytes: {text}"
    );
    let mut stream = admin.get_stream(dl_js::STREAM).await.expect("the stream exists");
    assert_eq!(stream.info().await.expect("stream info").config.max_bytes, -1, "the relay did not change the stream");
    relay.shut_down().await;
}

/// A broker config with two users. `admin` can do anything. `gw` is the user of a client that sends no credentials
/// (`no_auth_user`), and has the grants of the relay identity minus the
/// right to publish to `gateway.decision.>`: it can read and make the stream and read its PubAck inbox only.
const NO_PUBLISH_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js" }
authorization {
  users = [
    { user: "admin", password: "admin" }
    { user: "gw", password: "gw", permissions: {
        publish: { allow: ["$JS.API.STREAM.INFO.GATEWAY_DECISIONS", "$JS.API.STREAM.CREATE.GATEWAY_DECISIONS"] }
        subscribe: { allow: ["_INBOX_GW_DEC.>"] }
    } }
  ]
}
no_auth_user: gw
"#;

#[tokio::test]
async fn a_real_permission_refusal_is_rejected_and_a_later_outage_is_not() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay permission").await else {
        return;
    };
    let port = free_port();
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", NO_PUBLISH_CONF.as_bytes().to_vec())
        .with_mapped_port(port, 4222.tcp());
    let Some(nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay permission").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // The stream exists already, made by `admin`. The relay user can only read it.
    let admin = jetstream::new(
        async_nats::ConnectOptions::with_user_and_password("admin".into(), "admin".into())
            .connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("the admin client connects"),
    );
    admin.create_stream(dl_js::stream_config()).await.expect("the admin makes the stream");

    // Part 1: a real broker refuses the publish. The violation generation rises, and the entry is rejected and deleted.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let violations = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(
        connected_slot(&redis_url).await,
        relay_js(&format!("nats://127.0.0.1:{port}"), Arc::clone(&violations)).await,
        fast("i1"),
        Arc::clone(&violations),
    );
    wait_for("the refused entry is rejected and deleted", || async {
        snap.dropped("publish_rejected") == 1 && xlen(&node).await == 0
    })
    .await;
    assert!(violations.load(std::sync::atomic::Ordering::SeqCst) >= 1, "the event callback saw the permission violation");
    assert_eq!(snap.published(), 0);

    // Part 2: the broker goes down. A timeout with no new violation is a transport failure: the entries stay in
    // Redis and nothing counts as rejected.
    nats.stop().await.expect("stop NATS");
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    xadd(&node, &event_with_org(3, ORG_A), 0).await;
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert_eq!(xlen(&node).await, 2, "the entries stay in Redis during the outage");
    assert_eq!(snap.dropped("publish_rejected"), 1, "an outage after a violation is not a rejection");
    relay.shut_down().await;
}

/// A reset after a failed first `XGROUP CREATE` counts once, when the retry makes the group.
#[tokio::test]
async fn a_group_made_after_a_failed_first_create_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay recovery").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    // The stream is empty, so the relay polls XREADGROUP and sees NOGROUP at once after the flush.
    let dead = format!("nats://127.0.0.1:{}", free_port());
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&dead, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("the relay made the group", || async { redis_cli(&node, &["XINFO", "GROUPS", KEY]).await.contains("relay") }).await;
    // XGROUP is refused, so the first create after the flush fails.
    redis_cli(&node, &["ACL", "SETUSER", "default", "-xgroup"]).await;
    redis_cli(&node, &["FLUSHALL"]).await;
    wait_for("the first create fails", || async { logs.text().contains("cannot make the consumer group again") }).await;
    assert_eq!(snap.get("gateway_decision_log_redis_resets_total", &[]), 0, "no reset is counted while the group is missing");
    redis_cli(&node, &["ACL", "SETUSER", "default", "+xgroup"]).await;
    wait_for("one reset", || async { snap.get("gateway_decision_log_redis_resets_total", &[]) >= 1 }).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(snap.get("gateway_decision_log_redis_resets_total", &[]), 1, "the retry counts the incident once");
    relay.shut_down().await;
}

/// A broker config like `NO_PUBLISH_CONF`, but the relay user may publish events and read the stream, and may not
/// make it.
const NO_CREATE_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js" }
authorization {
  users = [
    { user: "admin", password: "admin" }
    { user: "gw", password: "gw", permissions: {
        publish: { allow: ["$JS.API.STREAM.INFO.GATEWAY_DECISIONS", "gateway.decision.>"] }
        subscribe: { allow: ["_INBOX_GW_DEC.>"] }
    } }
  ]
}
no_auth_user: gw
"#;

/// An entry that a PubAck or a rejection finished must leave Redis even when the next `ensure_stream` fails.
#[tokio::test]
async fn done_entries_are_acked_while_the_stream_cannot_be_made() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay settle").await else {
        return;
    };
    let port = free_port();
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", NO_CREATE_CONF.as_bytes().to_vec())
        .with_mapped_port(port, 4222.tcp());
    let Some(_nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay settle").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let nats_url = format!("nats://127.0.0.1:{port}");
    let admin = jetstream::new(
        async_nats::ConnectOptions::with_user_and_password("admin".into(), "admin".into())
            .connect(nats_url.clone())
            .await
            .expect("the admin client connects"),
    );
    admin.create_stream(dl_js::stream_config()).await.expect("the admin makes the stream");
    // The first event makes the relay sure of the stream.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let v = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, Arc::clone(&v)).await, fast("i1"), v);
    wait_for("the first event is published", || async { snap.published() == 1 && xlen(&node).await == 0 }).await;
    // Redis refuses XACK, so a finished entry cannot leave. The stream goes away, so the next publish says
    // StreamGone, and the relay cannot make the stream again (no grant for it).
    redis_cli(&node, &["ACL", "SETUSER", "default", "-xack", "-xreadgroup"]).await;
    admin.delete_stream(dl_js::STREAM).await.expect("delete the stream");
    xadd(&node, &event_with_org(2, "a.b"), 0).await;
    xadd(&node, &event_with_org(3, ORG_A), 0).await;
    redis_cli(&node, &["ACL", "SETUSER", "default", "+xreadgroup"]).await;
    wait_for("the rejected entry cannot be acked", || async { logs.text().contains("cannot ack and delete") }).await;
    assert_eq!(snap.dropped("publish_rejected"), 1);
    assert_eq!(xlen(&node).await, 2);
    redis_cli(&node, &["ACL", "SETUSER", "default", "+xack"]).await;
    wait_for("the rejected entry leaves while the stream is still missing", || async { xlen(&node).await == 1 }).await;
    assert_eq!(snap.dropped("publish_rejected"), 1, "the entry counts once");
    assert_eq!(snap.published(), 1, "the other entry has no PubAck and stays");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_consumer_with_a_new_pending_entry_is_not_deleted() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay prune race").await else {
        return;
    };
    let mut conn = paigasus_redis::connect(&redis_url, breaker_metrics()).await.expect("connect to the test Redis");
    redis_cli(&node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    redis_cli(&node, &["XADD", KEY, "*", "e", "x"]).await;
    // The relay read XINFO while "c1" had no pending entry. Now another relay claims an entry into "c1".
    redis_cli(&node, &["XREADGROUP", "GROUP", "relay", "c1", "COUNT", "1", "STREAMS", KEY, ">"]).await;
    let deleted = delete_consumer_if_empty(&mut conn, KEY, "relay", "c1").await.expect("the script runs");
    assert!(!deleted, "a consumer with a pending entry is not deleted");
    assert!(redis_cli(&node, &["XINFO", "CONSUMERS", KEY, "relay"]).await.contains("c1"));
    assert_eq!(redis_cli(&node, &["XPENDING", KEY, "relay"]).await.lines().next(), Some("1"), "the pending entry is still there");
    // With no pending entry the consumer goes.
    let id = redis_cli(&node, &["XRANGE", KEY, "-", "+", "COUNT", "1"]).await.lines().next().expect("an entry id").to_owned();
    redis_cli(&node, &["XACK", KEY, "relay", &id]).await;
    assert!(delete_consumer_if_empty(&mut conn, KEY, "relay", "c1").await.expect("the script runs"));
    assert!(!redis_cli(&node, &["XINFO", "CONSUMERS", KEY, "relay"]).await.contains("c1"));
}

/// Two refused publishes in one batch are AMBIGUOUS (the shared violation counter), so the relay publishes each
/// one alone, and a solo timeout with a violation is permanent.
#[tokio::test]
async fn concurrent_refusals_are_published_again_one_at_a_time_and_then_rejected() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay ambiguous").await else {
        return;
    };
    let port = free_port();
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", NO_PUBLISH_CONF.as_bytes().to_vec())
        .with_mapped_port(port, 4222.tcp());
    let Some(_nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay ambiguous").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = jetstream::new(
        async_nats::ConnectOptions::with_user_and_password("admin".into(), "admin".into())
            .connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("the admin client connects"),
    );
    admin.create_stream(dl_js::stream_config()).await.expect("the admin makes the stream");
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    let violations = Arc::new(AtomicU64::new(0));
    let relay = Relay::start(
        connected_slot(&redis_url).await,
        relay_js(&format!("nats://127.0.0.1:{port}"), Arc::clone(&violations)).await,
        fast("i1"),
        Arc::clone(&violations),
    );
    wait_for("both refused entries are rejected and deleted", || async {
        snap.dropped("publish_rejected") == 2 && xlen(&node).await == 0
    })
    .await;
    assert_eq!(snap.published(), 0);
    relay.shut_down().await;
}
