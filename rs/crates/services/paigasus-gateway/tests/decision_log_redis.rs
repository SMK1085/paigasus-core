// SPDX-License-Identifier: Apache-2.0

//! The decision-log writer against a real Redis (SMA-755 § 5.6, § 5.8). Redis is read only through
//! `redis-cli` in the container, as the limit-store tests do: `repo:redis-connect-single-site` bans a second
//! Rust Redis client. A test that needs only a dead or silent Redis uses the `paigasus-redis` test support
//! and needs no container.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::decision_log::gen_kv::NatsGenKv;
use paigasus_gateway::adapters::decision_log::gen_redis::{GroupOutcome, Sample, detect, ensure_group, epoch_key, gen_key, sample};
use paigasus_gateway::adapters::decision_log::generation::{GenKv, GenRecord, KvError, RedisView, entry_key, new_id};
use paigasus_gateway::adapters::decision_log::jetstream as dl_js;
use paigasus_gateway::adapters::decision_log::redis_conn::breaker_metrics;
use paigasus_gateway::adapters::decision_log::relay::delete_consumer_if_empty;
use paigasus_gateway::adapters::decision_log::{CHECK_ERROR_KINDS, RedisDecisionLog, RedisSlot, RelaySettings, WriterSettings, connect_loop, proto, run_relay, run_writer};
use paigasus_gateway::config::DecisionLogNatsConfig;
use paigasus_gateway::domain::decision_log::DecisionLog;
use paigasus_gateway::runtime;
use paigasus_gateway_core::decision_log::{DecisionRecord, RecordHeader, ReportedCall, build_response};
use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
use prost::Message;
use secrecy::SecretString;
use testcontainers_modules::nats::Nats;
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
    // `XLEN` can read 5 before the writer has counted the pipeline reply, so wait for both.
    wait_for("five stream entries and five writes", || async { xlen(&node).await == 5 && snap.written() == 5 }).await;
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

/// M3 (review round 1): a batch with an over-the-cap part, cut off by the shutdown deadline while the `XADD`
/// is pending, counts each event once. `CLIENT PAUSE ... WRITE` holds the `XADD` and still answers `XLEN`.
#[tokio::test]
async fn a_shutdown_during_a_partial_backlog_write_counts_each_event_once() {
    let Some((node, url)) = redis_7_4("decision_log_redis partial backlog shutdown").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let slot = connected_slot(&url).await;
    redis_cli(&node, &["CLIENT", "PAUSE", "10000", "WRITE"]).await;
    let (log, rx) = RedisDecisionLog::new(64);
    // Room for one of the two events: `XLEN` is 0 and the cap is 1.
    for seq in 1..=2 {
        log.record(record(seq));
    }
    // The budget ends before the 500 ms response timeout of the Redis handle, so the deadline cuts the write.
    let writer = Writer::start(rx, slot, 1, Duration::from_millis(100));
    // Wait until the writer has read the batch, passed the `XLEN` check and sent the `XADD` that the pause holds.
    // `CLIENT LIST` shows that client with `cmd=xadd`. Without this, the shutdown could cut a batch that the
    // writer never dequeued, and the test would not cover a write cancel.
    wait_for("the paused XADD is in flight", || async { redis_cli(&node, &["CLIENT", "LIST"]).await.contains("cmd=xadd") }).await;
    writer.shut_down().await;
    redis_cli(&node, &["CLIENT", "UNPAUSE"]).await;
    let counted = snap.dropped("backlog_full") + snap.dropped("shutdown");
    assert_eq!(
        counted,
        2,
        "each of the two events counts once: backlog_full {} shutdown {}",
        snap.dropped("backlog_full"),
        snap.dropped("shutdown")
    );
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

/// A broker for the relay tests that let the relay make the stream. JetStream reserves the full `max_bytes` of a
/// stream from the file store when it makes the stream. A broker with no config derives its store from the free disk
/// of the host, and the relay sizes the stream from the ACCOUNT limit, which is `MAX_BYTES` (18 GiB) when there is
/// none. On a small CI runner that create fails with 10047. So the store is fixed at 1 GiB, and the account `GW` has
/// a file limit of 256 MiB. The relay then makes a stream of 90 % of that limit. `no_auth_user` gives the clients
/// that send no credentials the account `GW`.
const RELAY_ACCOUNT_CONF: &str = r#"
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
/// The file limit of the account `GW` in `RELAY_ACCOUNT_CONF`.
const RELAY_ACCOUNT_BYTES: i64 = 268_435_456;

async fn start_nats(port: Option<u16>, what: &str) -> Option<(ContainerAsync<Nats>, String)> {
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", RELAY_ACCOUNT_CONF.as_bytes().to_vec());
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

async fn relay_js(url: &str, violations: dl_js::Violations) -> jetstream::Context {
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
    s.kv_timeout = Duration::from_secs(1);
    s
}

struct Relay {
    stop: watch::Sender<()>,
    task: JoinHandle<()>,
}

impl Relay {
    fn start(slot: RedisSlot, js: jetstream::Context, s: RelaySettings, violations: dl_js::Violations) -> Relay {
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

/// Make the generation bucket as the provisioner does (`ops/nats/provision-decisions.sh`). The relay never makes
/// it (D20): its identity has no `STREAM.CREATE` grant on the bucket.
async fn provision_bucket(url: &str) {
    test_js(url).await.create_key_value(dl_js::gen_kv_config()).await.expect("the provisioner makes the bucket");
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for("one publish", || async { snap.published() == 1 }).await;
    let got = stream_messages(&test_js(&nats_url).await).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].payload, bytes, "the published payload is the stream bytes, with the unknown field");
    relay.shut_down().await;
}

/// The delivery count of each pending entry of the group, from `XPENDING <key> relay - + 10`.
async fn delivery_counts(node: &ContainerAsync<Redis>) -> Vec<String> {
    let out = redis_cli(node, &["XPENDING", KEY, "relay", "-", "+", "10"]).await;
    let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Each entry is four lines: the id, the consumer, the idle time and the delivery count.
    lines.chunks(4).filter(|c| c.len() == 4).map(|c| c[3].to_owned()).collect()
}

/// The generation record in the KV bucket, or `None` while the bucket or a live entry is missing.
async fn kv_record(admin: &jetstream::Context) -> Option<GenRecord> {
    let store = admin.get_key_value(dl_js::GEN_BUCKET).await.ok()?;
    let entry = store.entry(entry_key(KEY)).await.ok()??;
    if entry.operation != jetstream::kv::Operation::Put {
        return None;
    }
    GenRecord::parse(&entry.value)
}

/// Wait until the stored epoch changed `n` times from the value at the call. After a Redis reset the epoch can
/// start again at 1, so the wait counts changes and does not compare the value.
async fn wait_for_kv_steps(admin: &jetstream::Context, n: u64) {
    let mut last = kv_record(admin).await.map(|r| r.epoch);
    let mut changes = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while changes < n {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {n} KV steps (saw {changes})");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = kv_record(admin).await.map(|r| r.epoch);
        // A failed read must not count as a step and must not reset `last`.
        if now.is_some() {
            if now != last {
                changes += 1;
            }
            last = now;
        }
    }
}

/// Make a stream `BLOCK` on `gateway.decision.>`. While it exists, `get_or_create_stream` for `GATEWAY_DECISIONS`
/// fails (overlapping subjects), so each relay keeps its batch in Redis, and the KV step still runs.
async fn block_publishes(admin: &jetstream::Context) {
    admin
        .create_stream(jetstream::stream::Config {
            name: "BLOCK".into(),
            subjects: vec!["gateway.decision.>".into()],
            max_bytes: 64 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .expect("the admin makes the BLOCK stream");
}

async fn unblock_publishes(admin: &jetstream::Context) {
    admin.delete_stream("BLOCK").await.expect("the admin deletes the BLOCK stream");
}

fn resets(snap: &Tally) -> u64 {
    snap.get("gateway_decision_log_redis_resets_total", &[])
}

fn lost(snap: &Tally) -> u64 {
    snap.get("gateway_decision_log_lost_estimate_total", &[])
}

fn check_errors(snap: &Tally, kind: &str) -> u64 {
    snap.get("gateway_decision_log_generation_check_errors_total", &[("kind", kind)])
}

/// Call after a NATS pause. Wait until a KV step fails on its `kv_timeout`: this proves that a step hung on a
/// KV request. The single-flight flag is then clear, so the next 200 ms tick starts a new step, and that step
/// hangs for a full `kv_timeout`. The wait of 400 ms puts the caller inside that window, with a KV request in
/// flight.
async fn wait_for_a_hung_kv_step(snap: &Tally) {
    let before = check_errors(snap, "timeout");
    wait_for("a KV step timed out on the paused NATS", || async { check_errors(snap, "timeout") > before }).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[tokio::test]
async fn flushall_with_two_relays_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay flushall").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay flushall").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    // NATS stays up. The BLOCK stream stops each publish, so no entry leaves Redis before the flush, and the KV
    // step still works.
    block_publishes(&admin).await;
    for seq in 1..=3 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let mut relays = Vec::new();
    for name in ["i1", "i2"] {
        let v = dl_js::Violations::default();
        relays.push(Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast(name), v));
    }
    wait_for("the backlog gauge reads 3", || async { snap.gauge("gateway_decision_log_backlog") == Some(3.0) }).await;
    // One relay reads the three entries. The other claims them after 300 ms of idle time, so each entry has two
    // deliveries. Then BOTH relays hold a batch, and neither reads Redis until it can publish.
    wait_for("both relays hold the three entries", || async {
        let counts = delivery_counts(&node).await;
        counts.len() == 3 && counts.iter().all(|c| c == "2")
    })
    .await;
    wait_for("the KV record holds the sample 3", || async { kv_record(&admin).await.is_some_and(|r| r.sample == Some(3)) }).await;
    redis_cli(&node, &["FLUSHALL"]).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(resets(&snap), 0, "a relay that holds a batch does not make the group again, and a stream with no group counts nothing");
    unblock_publishes(&admin).await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "two relays count one incident");
    assert_eq!(lost(&snap), 3, "the estimate is the KV sample from before the loss");
    // Each relay settled its batch after the flush, so XACK found no group and acked 0 of 3.
    let text = logs.text();
    assert!(
        text.contains("the relay acked fewer entries than it settled") && text.contains("acked=0") && text.contains("settled=3"),
        "a settle after the loss logs the two counts: {text}"
    );
    for r in relays {
        r.shut_down().await;
    }
}

#[tokio::test]
async fn a_restart_with_no_loss_counts_nothing() {
    let Some((_node, redis_url)) = redis_7_4("decision_log_redis restart no loss").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis restart no loss").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let v = dl_js::Violations::default();
    let first = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    let generation = kv_record(&admin).await.expect("a record").generation;
    first.shut_down().await;
    let v = dl_js::Violations::default();
    let second = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 0, "a restart with no loss counts no reset");
    assert_eq!(kv_record(&admin).await.expect("a record").generation, generation, "the generation is the same after the restart");
    for kind in CHECK_ERROR_KINDS {
        assert_eq!(check_errors(&snap, kind), 0, "no check error of kind {kind}");
    }
    second.shut_down().await;
}

#[tokio::test]
async fn the_upgrade_from_sma_755_counts_nothing() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis upgrade").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis upgrade").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    // The state of SMA-755: a group, no generation key, no epoch key, and an empty KV.
    redis_cli(&node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    let admin = test_js(&nats_url).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 0, "the upgrade is not a loss");
    let generation = gen_value(&node).await;
    assert!(Uuid::parse_str(&generation).is_ok(), "the generation key holds a UUID: {generation}");
    let epoch: u64 = redis_cli(&node, &["GET", &epoch_key(KEY)]).await.parse().expect("the epoch is a number");
    assert!(epoch >= 3, "the epoch grew with each step: {epoch}");
    assert_eq!(kv_record(&admin).await.expect("a record").generation, generation, "the KV record holds the generation of Redis");
    relay.shut_down().await;
}

#[tokio::test]
async fn the_backlog_gauge_updates_while_the_kv_step_hangs() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis gauge during kv hang").await else {
        return;
    };
    let Some((nats, nats_url)) = start_nats(None, "decision_log_redis gauge during kv hang").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let v = dl_js::Violations::default();
    // A long KV timeout: a KV step that blocked the sampler would hold the gauge for 30 s.
    let mut s = fast("i1");
    s.kv_timeout = Duration::from_secs(30);
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, s, v);
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    block_publishes(&admin).await;
    nats.pause().await.expect("pause NATS");
    for want in 1..=3u64 {
        xadd(&node, &event_with_org(want, ORG_A), 0).await;
        let reached = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for("the gauge follows XLEN", || async { snap.gauge("gateway_decision_log_backlog") == Some(want as f64) }),
        )
        .await;
        assert!(reached.is_ok(), "the gauge did not reach {want} within 2 s while the KV step hangs");
    }
    nats.unpause().await.expect("unpause NATS");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_shutdown_during_a_hung_kv_step_ends_in_time() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis shutdown during kv hang").await else {
        return;
    };
    let Some((nats, nats_url)) = start_nats(None, "decision_log_redis shutdown during kv hang").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let v = dl_js::Violations::default();
    let mut s = fast("i1");
    s.kv_timeout = Duration::from_secs(3);
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, s, v);
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    nats.pause().await.expect("pause NATS");
    wait_for_a_hung_kv_step(&snap).await;
    // The drain budget is 2 s and `kv_timeout` is 3 s.
    let ended = tokio::time::timeout(Duration::from_secs(2 + 3 + 2), relay.shut_down()).await;
    assert!(ended.is_ok(), "the relay did not end within the budget plus the KV timeout");
    // No KV step task outlives the relay. The detection of a step that ran after the shutdown would add 1 to the
    // Redis epoch, and a step runs `detect` only after its KV read, which needs NATS. So read the epoch while NATS
    // is still paused, then unpause, and check that it does not move.
    let before = redis_cli(&node, &["GET", &epoch_key(KEY)]).await;
    nats.unpause().await.expect("unpause NATS");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(redis_cli(&node, &["GET", &epoch_key(KEY)]).await, before, "a KV step ran after the relay ended");
}

/// I1 (final review): the same hung KV step, with the PRODUCTION values: `kv_timeout` 5 s from
/// `RelaySettings::new`, and the gateway's own `DecisionLogTasks::drain` with `DECISION_LOG_DRAIN_BUDGET`. The
/// relay must end inside the drain budget, so drain logs no "aborted" warning, and no KV step outlives it.
#[tokio::test]
async fn a_production_drain_during_a_hung_kv_step_aborts_nothing_and_leaves_no_step() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis production drain during kv hang").await else {
        return;
    };
    let Some((nats, nats_url)) = start_nats(None, "decision_log_redis production drain during kv hang").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    let v = dl_js::Violations::default();
    let mut s = fast("i1");
    s.kv_timeout = RelaySettings::new(KEY.to_owned(), "i1".to_owned()).kv_timeout;
    assert_eq!(s.kv_timeout, Duration::from_secs(5), "the production KV timeout");
    let (stop, shutdown) = watch::channel(());
    let relay = tokio::spawn(run_relay(
        connected_slot(&redis_url).await,
        relay_js(&nats_url, v.clone()).await,
        s,
        v,
        shutdown,
        runtime::DECISION_LOG_DRAIN_BUDGET,
    ));
    let tasks = runtime::DecisionLogTasks::new(stop, tokio::spawn(async {}), relay, tokio::spawn(async {}));
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    nats.pause().await.expect("pause NATS");
    wait_for_a_hung_kv_step(&snap).await;
    tasks.drain(runtime::DECISION_LOG_DRAIN_BUDGET).await;
    assert!(!logs.text().contains("did not stop in time and was aborted"), "the relay outlived the drain budget: {}", logs.text());
    // `drain` logs a task panic and returns normally, so a relay that panicked after its KV cleanup would pass the
    // line above. Check for the panic diagnostic too.
    assert!(!logs.text().contains("ended with a panic"), "the relay panicked during the drain: {}", logs.text());
    // The same epoch check as above: a step that outlived the relay would run `detect` after NATS comes back.
    let before = redis_cli(&node, &["GET", &epoch_key(KEY)]).await;
    nats.unpause().await.expect("unpause NATS");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(redis_cli(&node, &["GET", &epoch_key(KEY)]).await, before, "a KV step ran after the relay ended");
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("b"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("b"), v);
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
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for("one publish", || async { snap.published() == 1 }).await;
    let mut stream = admin.get_stream(dl_js::STREAM).await.expect("the relay made the stream");
    let live = stream.info().await.expect("stream info").config.clone();
    let want = dl_js::stream_config(dl_js::max_bytes_for(RELAY_ACCOUNT_BYTES));
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
    let mut drifted = dl_js::stream_config(dl_js::MAX_BYTES);
    drifted.max_bytes = -1;
    admin.create_stream(drifted).await.expect("make the drifted stream");
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i2"), v);
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

/// The `max_bytes` of a stream that a test makes itself on a broker with a fixed store. JetStream reserves `max_bytes`
/// of the file store for each stream. Streams of `MAX_BYTES` (18 GiB each) do not fit a store that the server derives
/// from the free disk of a small CI runner, and the create fails with 10047. So the streams are small, and the store
/// is fixed (see `MISROUTE_CONF`, `NO_PUBLISH_CONF` and `NO_CREATE_CONF`).
const MISROUTE_STREAM_BYTES: i64 = 67_108_864;
/// A broker with a fixed JetStream file store of 1 GiB, so that the test does not depend on the free disk of the host.
const MISROUTE_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1073741824 }
"#;

/// A drifted `GATEWAY_DECISIONS` lost the decision subjects, and another stream holds them. The relay sends
/// `Nats-Expected-Stream`, so the broker refuses the publish (10060) and stores nothing. The relay keeps the event in
/// Redis and does not count it as rejected. When the operator repairs the streams, the event goes out.
#[tokio::test]
async fn a_publish_that_another_stream_would_take_stays_in_redis() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay misroute").await else {
        return;
    };
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", MISROUTE_CONF.as_bytes().to_vec());
    let Some(nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay misroute").await else {
        return;
    };
    let nats_url = format!("nats://127.0.0.1:{}", paigasus_test_docker::mapped_port(&nats, 4222, "decision_log_redis relay misroute").await);
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    let mut drifted = dl_js::stream_config(MISROUTE_STREAM_BYTES);
    drifted.subjects = vec!["gateway.decision-old.>".to_owned()];
    admin.create_stream(drifted).await.expect("make the drifted stream");
    let mut other = dl_js::stream_config(MISROUTE_STREAM_BYTES);
    other.name = "OTHER".to_owned();
    admin.create_stream(other).await.expect("make the other stream");

    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    // Wait for the misroute report, or for the end of the entry (the fault that this test guards against).
    wait_for("the relay reports the misroute or deletes the entry", || async {
        logs.text().contains("another JetStream stream holds the decision subject") || xlen(&node).await == 0
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(xlen(&node).await, 1, "the event stays in Redis");
    assert_eq!(snap.published(), 0);
    assert_eq!(snap.dropped("publish_rejected"), 0, "a misroute is not a rejection");
    let mut stream = admin.get_stream("OTHER").await.expect("the other stream exists");
    assert_eq!(stream.info().await.expect("stream info").state.messages, 0, "the other stream stored nothing");

    // The operator repairs the streams. The relay publishes the kept event to GATEWAY_DECISIONS.
    admin.delete_stream("OTHER").await.expect("delete the other stream");
    admin.update_stream(dl_js::stream_config(MISROUTE_STREAM_BYTES)).await.expect("repair the subjects");
    wait_for("the kept event is published", || async { snap.published() == 1 && xlen(&node).await == 0 }).await;
    let got = stream_messages(&admin).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].msg_id, decision_id(1));
    relay.shut_down().await;
}

/// A broker config with two users. `admin` can do anything. `gw` is the user of a client that sends no credentials
/// (`no_auth_user`), and has the grants of the relay identity minus the
/// right to publish to `gateway.decision.>`: it can read the account limits, read and make the stream, and read
/// its PubAck inbox only.
const NO_PUBLISH_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1073741824 }
authorization {
  users = [
    { user: "admin", password: "admin" }
    { user: "gw", password: "gw", permissions: {
        publish: { allow: ["$JS.API.INFO", "$JS.API.STREAM.INFO.GATEWAY_DECISIONS", "$JS.API.STREAM.CREATE.GATEWAY_DECISIONS"] }
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
    admin.create_stream(dl_js::stream_config(MISROUTE_STREAM_BYTES)).await.expect("the admin makes the stream");

    // Part 1: a real broker refuses the publish. The violation generation rises, and the entry is rejected and deleted.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let violations = dl_js::Violations::default();
    let relay = Relay::start(
        connected_slot(&redis_url).await,
        relay_js(&format!("nats://127.0.0.1:{port}"), violations.clone()).await,
        fast("i1"),
        violations.clone(),
    );
    wait_for("the refused entry is rejected and deleted", || async {
        snap.dropped("publish_rejected") == 1 && xlen(&node).await == 0
    })
    .await;
    assert!(violations.publish.load(std::sync::atomic::Ordering::SeqCst) >= 1, "the event callback saw the permission violation");
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
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis relay recovery").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    // The stream is empty, so the relay polls XREADGROUP and sees NOGROUP at once after the flush.
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    // XGROUP is refused, so the first create after the flush fails.
    redis_cli(&node, &["ACL", "SETUSER", "default", "-xgroup"]).await;
    redis_cli(&node, &["FLUSHALL"]).await;
    wait_for("the first create fails", || async { logs.text().contains("cannot make the consumer group again") }).await;
    // Let several sampler ticks (200 ms) and KV steps run while the group is missing.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(resets(&snap), 0, "no reset is counted while the group is missing");
    redis_cli(&node, &["ACL", "SETUSER", "default", "+xgroup"]).await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "the retry counts the incident once");
    relay.shut_down().await;
}

/// A broker config like `NO_PUBLISH_CONF`, but the relay user may publish events and read the stream, and may not
/// make it.
const NO_CREATE_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1073741824 }
authorization {
  users = [
    { user: "admin", password: "admin" }
    { user: "gw", password: "gw", permissions: {
        publish: { allow: ["$JS.API.INFO", "$JS.API.STREAM.INFO.GATEWAY_DECISIONS", "gateway.decision.>"] }
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
    admin.create_stream(dl_js::stream_config(MISROUTE_STREAM_BYTES)).await.expect("the admin makes the stream");
    // The first event makes the relay sure of the stream.
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&nats_url, v.clone()).await, fast("i1"), v);
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
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::DEBUG);
    let admin = jetstream::new(
        async_nats::ConnectOptions::with_user_and_password("admin".into(), "admin".into())
            .connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("the admin client connects"),
    );
    admin.create_stream(dl_js::stream_config(MISROUTE_STREAM_BYTES)).await.expect("the admin makes the stream");
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    let violations = dl_js::Violations::default();
    let relay = Relay::start(
        connected_slot(&redis_url).await,
        relay_js(&format!("nats://127.0.0.1:{port}"), violations.clone()).await,
        fast("i1"),
        violations.clone(),
    );
    wait_for("both refused entries are rejected and deleted", || async {
        snap.dropped("publish_rejected") == 2 && xlen(&node).await == 0
    })
    .await;
    assert_eq!(snap.published(), 0);
    relay.shut_down().await;
    // Each entry has one attempt in the concurrent batch, and one solo attempt after the AMBIGUOUS result. A
    // relay that rejected a concurrent refusal at once would show no solo attempt.
    let text = logs.text();
    let attempts = |concurrent: bool| {
        text.lines()
            .filter(|l| l.contains("the relay publishes an entry") && l.contains(&format!("concurrent={concurrent}")))
            .count()
    };
    assert_eq!(attempts(true), 2, "two attempts in the concurrent batch:\n{text}");
    assert_eq!(attempts(false), 2, "one solo attempt for each refused entry:\n{text}");
}

/// A broker config whose JetStream holds 1 MiB of file storage for the whole server.
const FULL_DISK_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1048576 }
"#;

/// A full JetStream disk is a resource condition, not a rejection of the event. The PubAck has error code 10023
/// (`JSInsufficientResourcesErr`), and `async-nats` reports it as `Other`. The relay keeps the entries in Redis and
/// publishes them when there is free disk again.
#[tokio::test]
async fn a_full_jetstream_disk_keeps_the_events_until_there_is_free_disk() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay full disk").await else {
        return;
    };
    let port = free_port();
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", FULL_DISK_CONF.as_bytes().to_vec())
        .with_mapped_port(port, 4222.tcp());
    let Some(_nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay full disk").await else {
        return;
    };
    let url = format!("nats://127.0.0.1:{port}");
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&url).await;
    // A stream with no byte limit, so that the server limit, not the stream limit, is the first limit to hit.
    let mut cfg = dl_js::stream_config(dl_js::MAX_BYTES);
    cfg.max_bytes = -1;
    admin.create_stream(cfg).await.expect("the admin makes the stream");
    // Fill the disk until the broker refuses a message.
    let mut code = None;
    for n in 0..64 {
        let fill = jetstream::message::PublishMessage::build().payload(Bytes::from(vec![b'x'; 60 * 1024])).message_id(format!("fill-{n}"));
        if let Err(err) = admin.send_publish("gateway.decision._none", fill).await.expect("the fill publish is sent").await {
            code = dl_js::api_error_of(&err).map(|a| a.code);
            break;
        }
    }
    assert_eq!(code, Some(10023), "the broker refuses a message when the disk is full");

    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    xadd(&node, &event_with_org(2, ORG_A), 0).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&url, v.clone()).await, fast("i1"), v);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(snap.dropped("publish_rejected"), 0, "a full disk is not a rejection");
    assert_eq!(snap.published(), 0);
    assert_eq!(xlen(&node).await, 2, "the entries stay in Redis while the disk is full");

    // Free the disk. The relay publishes the kept entries.
    admin.get_stream(dl_js::STREAM).await.expect("the stream exists").purge().await.expect("purge the stream");
    wait_for("both kept entries are published", || async { snap.published() == 2 && xlen(&node).await == 0 }).await;
    assert_eq!(snap.dropped("publish_rejected"), 0);
    let ids: Vec<_> = stream_messages(&admin).await.into_iter().map(|m| m.msg_id).collect();
    assert!(ids.contains(&decision_id(1)) && ids.contains(&decision_id(2)), "both events are on the stream: {ids:?}");
    relay.shut_down().await;
}

/// A broker config with an account that holds 256 MiB of file storage, less than the default 20 GiB. `gw` has the
/// grants of the relay identity in `ops/nats/subjects.env`. `admin` can do anything in the same account.
const SMALL_ACCOUNT_CONF: &str = r#"
port: 4222
jetstream: { store_dir: "/tmp/js", max_file_store: 1073741824 }
accounts {
  GW: {
    jetstream: { max_mem: 0, max_file: 268435456, max_streams: 2, max_consumers: 16 }
    users: [
      { user: "admin", password: "admin" }
      { user: "gw", password: "gw", permissions: {
          publish: { allow: ["gateway.decision.>", "$JS.API.INFO", "$JS.API.STREAM.INFO.GATEWAY_DECISIONS", "$JS.API.STREAM.CREATE.GATEWAY_DECISIONS"] }
          subscribe: { allow: ["_INBOX_GW_DEC.>"] }
      } }
    ]
  }
}
no_auth_user: gw
"#;

/// On an account with a smaller disk limit, the relay makes the stream with 90 % of that limit, not with the fixed
/// default (which the broker refuses). A stream that has that value is not a drift.
#[tokio::test]
async fn the_relay_sizes_the_stream_to_a_smaller_account_limit() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis relay small account").await else {
        return;
    };
    let port = free_port();
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", SMALL_ACCOUNT_CONF.as_bytes().to_vec())
        .with_mapped_port(port, 4222.tcp());
    let Some(_nats) = paigasus_test_docker::start_or_skip(image, "decision_log_redis relay small account").await else {
        return;
    };
    let url = format!("nats://127.0.0.1:{port}");
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = jetstream::new(
        async_nats::ConnectOptions::with_user_and_password("admin".into(), "admin".into())
            .connect(&url)
            .await
            .expect("the admin client connects"),
    );
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&url, v.clone()).await, fast("i1"), v.clone());
    wait_for("the relay makes the stream and publishes", || async { snap.published() == 1 && xlen(&node).await == 0 }).await;
    let mut stream = admin.get_stream(dl_js::STREAM).await.expect("the relay made the stream");
    let live = stream.info().await.expect("stream info").config.clone();
    assert_eq!(live.max_bytes, 268_435_456 * 9 / 10, "90 % of the account limit");
    assert_eq!(live.max_bytes, dl_js::max_bytes_for(268_435_456));
    let text = logs.text();
    assert!(!text.contains("differs from the expected config"), "no drift warning: {text}");
    assert!(!text.contains("cannot read the account limits"), "the relay grants cover $JS.API.INFO: {text}");
    assert_eq!(v.publish.load(std::sync::atomic::Ordering::SeqCst), 0, "no permission violation");
    relay.shut_down().await;
}

#[tokio::test]
async fn the_kv_adapter_does_a_cas_on_the_provisioned_bucket() {
    let Some((_nats, url)) = start_nats(None, "decision_log_redis kv adapter").await else {
        return;
    };
    provision_bucket(&url).await;
    let key = entry_key(KEY);
    let kv = NatsGenKv::new(relay_js(&url, dl_js::Violations::default()).await, key.clone());
    assert!(kv.get().await.expect("get on a new bucket").is_none(), "no entry before the first create");
    let admin = test_js(&url).await;
    let store = admin.get_key_value(dl_js::GEN_BUCKET).await.expect("the provisioned bucket");
    let info = admin.get_stream(format!("KV_{}", dl_js::GEN_BUCKET)).await.expect("the bucket stream").cached_info().clone();
    assert_eq!(info.config.max_bytes, dl_js::GEN_MAX_BYTES);
    assert_eq!(info.config.max_messages_per_subject, 1, "history 1");
    assert_eq!(info.config.storage, jetstream::stream::StorageType::File);
    assert!(info.config.allow_direct, "the direct get is on");
    let r1 = kv.create(Bytes::from_static(b"a")).await.expect("first create");
    assert_eq!(kv.create(Bytes::from_static(b"b")).await, Err(KvError::WrongRevision), "a create on a live key");
    let r2 = kv.update(Bytes::from_static(b"c"), r1).await.expect("update with the right revision");
    assert!(r2 > r1);
    assert_eq!(kv.update(Bytes::from_static(b"d"), r1).await, Err(KvError::WrongRevision), "a stale revision");
    let got = kv.get().await.expect("get").expect("an entry");
    assert_eq!((got.value.as_ref(), got.revision, got.live), (&b"c"[..], r2, true));
    store.delete(&key).await.expect("admin delete");
    let marker = kv.get().await.expect("get after delete").expect("a marker entry");
    assert!(!marker.live, "a delete marker is not live");
    kv.create(Bytes::from_static(b"e")).await.expect("a create over a marker");
}

#[tokio::test]
async fn the_kv_adapter_recovers_after_an_outage() {
    let Some((nats, url)) = start_nats(None, "decision_log_redis kv outage").await else {
        return;
    };
    provision_bucket(&url).await;
    let kv = NatsGenKv::new(relay_js(&url, dl_js::Violations::default()).await, entry_key(KEY));
    kv.get().await.expect("get while NATS is up");
    // A pause keeps the connection and the server state. A stop does not: a reconnecting client can still answer.
    nats.pause().await.expect("pause NATS");
    let down = kv.get().await;
    assert!(matches!(down, Err(KvError::Timeout | KvError::Other)), "get while NATS is paused: {down:?}");
    nats.unpause().await.expect("unpause NATS");
    wait_for("get works again", || async { kv.get().await.is_ok() }).await;
}

#[tokio::test]
async fn a_deleted_bucket_counts_no_bucket_until_the_provisioner_makes_it_again() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis deleted bucket").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis deleted bucket").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    provision_bucket(&nats_url).await;
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    assert_eq!(check_errors(&snap, "no_bucket"), 0, "no check error while the bucket exists");
    admin.delete_stream(format!("KV_{}", dl_js::GEN_BUCKET)).await.expect("admin deletes the bucket stream");
    wait_for("the KV step counts no_bucket", || async { check_errors(&snap, "no_bucket") > 0 }).await;
    // A publish still goes through while the bucket is missing.
    let published = snap.published();
    xadd(&node, &event_with_org(1, ORG_A), 0).await;
    wait_for("the event is published", || async { snap.published() == published + 1 && xlen(&node).await == 0 }).await;
    provision_bucket(&nats_url).await;
    wait_for("the KV step works again", || async { kv_record(&admin).await.is_some() }).await;
    let counted = check_errors(&snap, "no_bucket");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(check_errors(&snap, "no_bucket"), counted, "no more no_bucket after the bucket is back");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_relay_with_no_bucket_counts_no_bucket_and_publishes_every_event() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis no bucket").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis no bucket").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    // Block the publishes, so that the events stay in Redis and the backlog gauge has a value to show.
    block_publishes(&admin).await;
    for seq in 1..=10 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    wait_for("the KV step counts no_bucket", || async { check_errors(&snap, "no_bucket") > 0 }).await;
    wait_for("the backlog gauge reads 10 while the bucket is missing", || async {
        snap.gauge("gateway_decision_log_backlog") == Some(10.0)
    })
    .await;
    unblock_publishes(&admin).await;
    wait_for("10 events are published and Redis is empty", || async { snap.published() == 10 && xlen(&node).await == 0 }).await;
    wait_for("the backlog gauge reads 0 again", || async { snap.gauge("gateway_decision_log_backlog") == Some(0.0) }).await;
    assert_eq!(snap.dropped("publish_rejected"), 0, "a missing bucket rejects no event");
    assert!(admin.get_key_value(dl_js::GEN_BUCKET).await.is_err(), "the relay did not make the bucket");
    relay.shut_down().await;
}

/// D9: a call that the step timeout cancels counts as a failure, so it leaves no cached handle.
#[tokio::test]
async fn the_kv_adapter_makes_the_handle_again_after_a_cancelled_call() {
    let Some((nats, url)) = start_nats(None, "decision_log_redis kv adapter cancelled call").await else {
        return;
    };
    provision_bucket(&url).await;
    let kv = NatsGenKv::new(relay_js(&url, dl_js::Violations::default()).await, entry_key(KEY));
    kv.create(Bytes::from_static(b"a")).await.expect("create on the provisioned bucket");
    let admin = test_js(&url).await;
    admin.delete_stream(format!("KV_{}", dl_js::GEN_BUCKET)).await.expect("admin deletes the bucket stream");
    nats.pause().await.expect("pause NATS");
    let cancelled = tokio::time::timeout(Duration::from_millis(300), kv.get()).await;
    nats.unpause().await.expect("unpause NATS");
    assert!(cancelled.is_err(), "the get did not hang on a paused NATS: {cancelled:?}");
    // A kept handle would fail here with another error. A new handle gets the bucket, finds none, and reports it.
    let missing = kv.get().await;
    assert!(matches!(missing, Err(KvError::NoBucket)), "the cancelled call left no handle, so the get finds no bucket: {missing:?}");
    provision_bucket(&url).await;
    let again = kv.get().await.expect("the next get works after the provisioner made the bucket");
    assert!(again.is_none(), "the new bucket is empty");
}

// ---- The generation record Redis scripts (SMA-821 Task 7) ----

async fn script_conn(url: &str) -> paigasus_redis::RedisHandle {
    paigasus_redis::connect(url, breaker_metrics()).await.expect("connect to the test Redis")
}

async fn gen_value(node: &ContainerAsync<Redis>) -> String {
    redis_cli(node, &["GET", &gen_key(KEY)]).await
}

#[tokio::test]
async fn the_group_script_makes_a_new_generation_on_create() {
    let Some((node, url)) = redis_7_4("decision_log_redis group script create").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    assert_eq!(ensure_group(&mut conn, KEY, "relay", "id-1").await.expect("first create"), GroupOutcome::Created);
    assert_eq!(gen_value(&node).await, "id-1");
    redis_cli(&node, &["DEL", KEY]).await;
    assert_eq!(ensure_group(&mut conn, KEY, "relay", "id-2").await.expect("create after a stream loss"), GroupOutcome::Created);
    assert_eq!(gen_value(&node).await, "id-2", "a new group means a new generation");
}

#[tokio::test]
async fn the_group_script_keeps_the_generation_on_busygroup_and_repairs_a_missing_one() {
    let Some((node, url)) = redis_7_4("decision_log_redis group script busygroup").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    ensure_group(&mut conn, KEY, "relay", "id-1").await.expect("create");
    assert_eq!(ensure_group(&mut conn, KEY, "relay", "id-2").await.expect("busygroup"), GroupOutcome::Exists);
    assert_eq!(gen_value(&node).await, "id-1", "BUSYGROUP keeps the generation");
    redis_cli(&node, &["DEL", &gen_key(KEY)]).await;
    assert_eq!(ensure_group(&mut conn, KEY, "relay", "id-3").await.expect("busygroup, no gen"), GroupOutcome::Exists);
    assert_eq!(gen_value(&node).await, "id-3", "a missing generation is repaired with NX");
}

#[tokio::test]
async fn the_group_script_returns_another_error() {
    let Some((node, url)) = redis_7_4("decision_log_redis group script error").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    redis_cli(&node, &["ACL", "SETUSER", "default", "-xgroup"]).await;
    let result = ensure_group(&mut conn, KEY, "relay", "id-1").await;
    redis_cli(&node, &["ACL", "SETUSER", "default", "+xgroup"]).await;
    let err = result.expect_err("XGROUP is denied");
    assert!(!err.to_string().contains("BUSYGROUP"), "not a BUSYGROUP: {err}");
    assert_eq!(gen_value(&node).await, "", "the failed script wrote no generation");
}

#[tokio::test]
async fn the_detection_script_repairs_the_generation_and_increments_the_epoch() {
    let Some((node, url)) = redis_7_4("decision_log_redis detect script").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    redis_cli(&node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    assert_eq!(
        detect(&mut conn, KEY, "relay", "id-1").await.expect("detect 1"),
        RedisView::Live {
            generation: "id-1".into(),
            epoch: 1,
            xlen: 0
        }
    );
    assert_eq!(
        detect(&mut conn, KEY, "relay", "id-2").await.expect("detect 2"),
        RedisView::Live {
            generation: "id-1".into(),
            epoch: 2,
            xlen: 0
        },
        "the generation is kept and the epoch grows"
    );
    assert_eq!(
        detect(&mut conn, KEY, "relay", "id-3").await.expect("detect 3"),
        RedisView::Live {
            generation: "id-1".into(),
            epoch: 3,
            xlen: 0
        },
        "the epoch counts every call"
    );
}

#[tokio::test]
async fn the_detection_script_writes_nothing_without_a_group() {
    let Some((node, url)) = redis_7_4("decision_log_redis detect no group").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    assert_eq!(detect(&mut conn, KEY, "relay", "id-1").await.expect("empty Redis"), RedisView::NoGroup);
    assert_eq!(redis_cli(&node, &["EXISTS", &gen_key(KEY), &epoch_key(KEY)]).await, "0");
    redis_cli(&node, &["XADD", KEY, "*", "e", "x"]).await;
    assert_eq!(detect(&mut conn, KEY, "relay", "id-1").await.expect("stream, no group"), RedisView::NoGroup);
    assert_eq!(redis_cli(&node, &["EXISTS", &gen_key(KEY), &epoch_key(KEY)]).await, "0", "the detection wrote no key");
}

#[tokio::test]
async fn the_sample_script_reads_the_keys_and_writes_nothing() {
    let Some((node, url)) = redis_7_4("decision_log_redis sample script").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    assert_eq!(
        sample(&mut conn, KEY, "relay").await.expect("sample on empty Redis"),
        Sample {
            xlen: 0,
            has_group: false,
            generation: None,
            epoch: None
        }
    );
    redis_cli(&node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    redis_cli(&node, &["XADD", KEY, "*", "e", "x"]).await;
    redis_cli(&node, &["SET", &gen_key(KEY), "id-9"]).await;
    redis_cli(&node, &["SET", &epoch_key(KEY), "7"]).await;
    let before = state_of(&node).await;
    let got = sample(&mut conn, KEY, "relay").await.expect("sample");
    assert_eq!(
        got,
        Sample {
            xlen: 1,
            has_group: true,
            generation: Some("id-9".into()),
            epoch: Some(7)
        }
    );
    assert_eq!(state_of(&node).await, before, "the sample changed gen, epoch or the stream");
    assert_eq!(before, ("id-9".to_owned(), "7".to_owned(), "1".to_owned()));
}

/// `(GET gen, GET epoch, XLEN)` as `redis-cli` prints them.
async fn state_of(node: &ContainerAsync<Redis>) -> (String, String, String) {
    (
        redis_cli(node, &["GET", &gen_key(KEY)]).await,
        redis_cli(node, &["GET", &epoch_key(KEY)]).await,
        redis_cli(node, &["XLEN", KEY]).await,
    )
}

/// Run `detect` while the ACL `deny` is set on the default user (for example `-set`). The ACL is restored before
/// the result is returned.
async fn detect_with_denied(node: &ContainerAsync<Redis>, conn: &mut paigasus_redis::RedisHandle, deny: &str, allow: &str) -> Result<RedisView, redis::RedisError> {
    redis_cli(node, &["ACL", "SETUSER", "default", deny]).await;
    let result = detect(conn, KEY, "relay", "id-2").await;
    redis_cli(node, &["ACL", "SETUSER", "default", allow]).await;
    result
}

#[tokio::test]
async fn the_detection_script_returns_a_write_error_and_changes_no_epoch() {
    let Some((node, url)) = redis_7_4("decision_log_redis detect write error").await else {
        return;
    };
    let mut conn = script_conn(&url).await;
    redis_cli(&node, &["XGROUP", "CREATE", KEY, "relay", "0", "MKSTREAM"]).await;
    // SET denied: the first write fails. Nothing is written.
    let err = detect_with_denied(&node, &mut conn, "-set", "+set").await.expect_err("SET is denied");
    assert!(err.to_string().contains("no permissions to run the 'set' command"), "the ACL error text is returned: {err}");
    assert_eq!(redis_cli(&node, &["EXISTS", &gen_key(KEY), &epoch_key(KEY)]).await, "0", "no key was written");
    // INCR denied: the generation is set, the epoch is not.
    let err = detect_with_denied(&node, &mut conn, "-incr", "+incr").await.expect_err("INCR is denied");
    assert!(err.to_string().contains("no permissions to run the 'incr' command"), "the ACL error text is returned: {err}");
    assert_eq!(redis_cli(&node, &["EXISTS", &epoch_key(KEY)]).await, "0", "no epoch change");
}

// ---- The scenario tests of the generation record (SMA-821 Task 9) ----

async fn start_relay(redis_url: &str, nats_url: &str, name: &str) -> Relay {
    let v = dl_js::Violations::default();
    Relay::start(connected_slot(redis_url).await, relay_js(nats_url, v.clone()).await, fast(name), v)
}

/// Run one Lua script with the given keys through `redis-cli`.
async fn eval(node: &ContainerAsync<Redis>, lua: &str, keys: &[String]) -> String {
    let n = keys.len().to_string();
    let mut args = vec!["EVAL", lua, n.as_str()];
    args.extend(keys.iter().map(String::as_str));
    redis_cli(node, &args).await
}

#[tokio::test]
async fn a_fresh_relay_after_flushall_counts_one_reset_with_the_stored_sample() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis fresh relay after flushall").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis fresh relay after flushall").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    block_publishes(&admin).await;
    for seq in 1..=3 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let first = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the KV record holds the sample 3", || async { kv_record(&admin).await.is_some_and(|r| r.sample == Some(3)) }).await;
    first.shut_down().await;
    redis_cli(&node, &["FLUSHALL"]).await;
    let second = start_relay(&redis_url, &nats_url, "i2").await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "a new relay with no memory counts the loss once");
    assert_eq!(lost(&snap), 3, "the estimate is the KV sample from before the loss");
    second.shut_down().await;
}

#[tokio::test]
async fn a_restore_of_an_older_generation_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis restore older generation").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis restore older generation").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    let old = kv_record(&admin).await.expect("a record").generation;
    let new = new_id();
    redis_cli(&node, &["SET", &gen_key(KEY), &new]).await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "a changed generation counts one reset");
    assert_eq!(kv_record(&admin).await.expect("a record").generation, new, "the KV record holds the new generation, not {old}");
    // Spec T5: a restore to the generation that Redis held before. It counts one more reset.
    redis_cli(&node, &["SET", &gen_key(KEY), &old]).await;
    wait_for("a second reset", || async { resets(&snap) >= 2 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 2, "a restore of the older generation counts exactly one more reset");
    assert_eq!(kv_record(&admin).await.expect("a record").generation, old, "the KV record holds the restored generation");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_reset_during_a_nats_outage_counts_after_nats_returns() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis reset during nats outage").await else {
        return;
    };
    // A pause keeps the port and the store; a stop and start on a fixed port can lose the port to another container.
    let Some((nats, nats_url)) = start_nats(None, "decision_log_redis reset during nats outage").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    let before = kv_record(&admin).await.expect("a record").generation;
    let errors = || CHECK_ERROR_KINDS.iter().map(|k| check_errors(&snap, k)).sum::<u64>();
    let errors_before = errors();
    nats.pause().await.expect("pause NATS");
    redis_cli(&node, &["FLUSHALL"]).await;
    wait_for("a new check error while NATS is down", || async { errors() > errors_before }).await;
    assert_eq!(resets(&snap), 0, "no reset is counted while the KV cannot be read");
    nats.unpause().await.expect("unpause NATS");
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "the loss during the outage counts once");
    let after = kv_record(&admin).await.expect("a record").generation;
    assert_ne!(after, before, "the KV record moved to the new generation");
    assert_eq!(after, gen_value(&node).await, "the KV record holds the generation of Redis");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_publisher_without_kv_grants_still_relays_every_event() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis publisher without kv grants").await else {
        return;
    };
    // The broker is paused, never restarted, so the port that Docker maps is enough.
    let what = "decision_log_redis publisher without kv grants";
    let image = Nats::default()
        .with_cmd(["-c", "/etc/nats-test.conf"])
        .with_copy_to("/etc/nats-test.conf", SMALL_ACCOUNT_CONF.as_bytes().to_vec());
    let Some(nats) = paigasus_test_docker::start_or_skip(image, what).await else {
        return;
    };
    let url = format!("nats://127.0.0.1:{}", paigasus_test_docker::mapped_port(&nats, 4222, what).await);
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::DEBUG);
    let v = dl_js::Violations::default();
    let relay = Relay::start(connected_slot(&redis_url).await, relay_js(&url, v.clone()).await, fast("i1"), v.clone());
    let mut seq = 0;
    for _ in 0..4 {
        for _ in 0..5 {
            seq += 1;
            xadd(&node, &event_with_org(seq, ORG_A), 0).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    wait_for("20 events are published and Redis is empty", || async { snap.published() == 20 && xlen(&node).await == 0 }).await;
    assert_eq!(snap.dropped("publish_rejected"), 0, "a KV refusal does not reject an event");
    assert_eq!(v.publish.load(std::sync::atomic::Ordering::SeqCst), 0, "no publish violation");
    assert!(v.kv.load(std::sync::atomic::Ordering::SeqCst) > 0, "the KV step saw permission violations");
    assert!(check_errors(&snap, "permission") > 0, "the KV step counts permission errors");
    // The context of `dl_js::connect` is `jetstream::new(client)`: async-nats 0.53 waits 5 s for a publish ack
    // (`DEFAULT_TIMEOUT`). The relay logs nothing and counts nothing on a timeout, but each publish attempt logs
    // "the relay publishes an entry" at DEBUG. With NATS paused for 8 s, the batch is tried again after the first
    // timeout (measured: one attempt each 5 s, at about 0 s, 5 s and 10 s), so there are at least 2 attempts for one entry.
    // Without a timeout there would be 1.
    let attempts = |text: &str| text.matches("the relay publishes an entry").count();
    nats.pause().await.expect("pause NATS");
    // Count only the attempts after the pause starts. ONE event goes in: the batch has one entry, so a second
    // attempt can only be the same entry tried again after its 5 s ack timeout.
    let attempts_before = attempts(&logs.text());
    seq += 1;
    xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let during = attempts(&logs.text()) - attempts_before;
    assert!(during >= 2, "the one event timed out after 5 s and was tried again: {during} attempts");
    assert_eq!(snap.published(), 20, "nothing is published while NATS is paused");
    assert_eq!(snap.dropped("publish_rejected"), 0, "a publish timeout is not a rejection");
    assert_eq!(xlen(&node).await, 1, "the event stays in Redis");
    nats.unpause().await.expect("unpause NATS");
    wait_for("21 events are published", || async { snap.published() == 21 && xlen(&node).await == 0 }).await;
    assert_eq!(
        snap.dropped("publish_rejected"),
        0,
        "the publish timeouts of the pause rejected no event (a paused broker sends no -ERR)"
    );
    assert_eq!(v.publish.load(std::sync::atomic::Ordering::SeqCst), 0, "still no publish violation");
    assert!(check_errors(&snap, "permission") > 0, "the KV step still counts permission errors");
    relay.shut_down().await;
}

#[tokio::test]
async fn an_xack_of_zero_and_a_group_made_by_another_relay_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis xack zero").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis xack zero").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    block_publishes(&admin).await;
    for seq in 1..=3 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let a = start_relay(&redis_url, &nats_url, "a").await;
    wait_for("the KV record holds the sample 3", || async { kv_record(&admin).await.is_some_and(|r| r.sample == Some(3)) }).await;
    wait_for("relay A holds three pending deliveries", || async { delivery_counts(&node).await.len() == 3 }).await;
    redis_cli(&node, &["FLUSHALL"]).await;
    let b = start_relay(&redis_url, &nats_url, "b").await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "relay B made the group and the loss counts once");
    assert_eq!(lost(&snap), 3, "the estimate is the KV sample from before the loss");
    unblock_publishes(&admin).await;
    wait_for("relay A settled its batch with an XACK of 0", || async { logs.text().contains("acked=0") }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "the settle of relay A adds no reset");
    a.shut_down().await;
    b.shut_down().await;
}

#[tokio::test]
async fn a_group_made_by_an_sma_755_relay_is_repaired_and_counts_one_reset() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis group by sma 755 relay").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis group by sma 755 relay").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    let old = kv_record(&admin).await.expect("a record").generation;
    // One script: the old relay's group is made at once, so the relay never reads NOGROUP.
    let lua = "redis.call('DEL', KEYS[1], KEYS[2], KEYS[3]) return redis.call('XGROUP', 'CREATE', KEYS[1], 'relay', '0', 'MKSTREAM')";
    eval(&node, lua, &[KEY.to_owned(), gen_key(KEY), epoch_key(KEY)]).await;
    // One KV step that began before the script can still end after it. So the second record change after the script
    // is the end of the first step that saw the old group, and the detection script must have set the key by then.
    // `wait_for_kv_steps` has its own 30 s limit, so the test cannot hang.
    wait_for_kv_steps(&admin, 2).await;
    assert!(!gen_value(&node).await.is_empty(), "the first KV step that saw the old group did not set the generation key");
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "the repaired generation counts one reset");
    let new = gen_value(&node).await;
    assert_ne!(new, old, "the relay set a new generation");
    assert_eq!(kv_record(&admin).await.expect("a record").generation, new, "the KV record follows the new generation");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_return_to_an_older_state_of_the_same_generation_counts_one_loss() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis return to older state").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis return to older state").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    block_publishes(&admin).await;
    for seq in 1..=2 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the KV record holds the sample 2", || async { kv_record(&admin).await.is_some_and(|r| r.sample == Some(2)) }).await;
    let keys = [KEY.to_owned(), gen_key(KEY), epoch_key(KEY), "side:stream".to_owned(), "side:gen".to_owned(), "side:epoch".to_owned()];
    eval(&node, "for i = 1, 3 do redis.call('SET', KEYS[i + 3], redis.call('DUMP', KEYS[i])) end return 1", &keys).await;
    let generation = gen_value(&node).await;
    // An entry after the snapshot: the restore removes it, and the KV holds the larger sample before the restore.
    xadd(&node, &event_with_org(3, ORG_A), 0).await;
    wait_for("the KV record holds the sample 3", || async { kv_record(&admin).await.is_some_and(|r| r.sample == Some(3)) }).await;
    wait_for_kv_steps(&admin, 3).await;
    eval(
        &node,
        "for i = 1, 3 do redis.call('RESTORE', KEYS[i], 0, redis.call('GET', KEYS[i + 3]), 'REPLACE') end return 1",
        &keys,
    )
    .await;
    wait_for("one reset", || async { resets(&snap) >= 1 }).await;
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 1, "a return to an older state counts one loss");
    assert_eq!(lost(&snap), 3, "the estimate is the KV sample from before the restore");
    assert_eq!(gen_value(&node).await, generation, "the generation is unchanged");
    assert_eq!(xlen(&node).await, 2, "the stream is the restored one: the entry after the snapshot is gone");
    assert!(redis_cli(&node, &["XINFO", "GROUPS", KEY]).await.contains("relay"), "the relay group still exists after the restore");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_foreign_writer_counts_errors_and_no_reset() {
    let Some((_node, redis_url)) = redis_7_4("decision_log_redis foreign writer").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis foreign writer").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
    let admin = test_js(&nats_url).await;
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    let store = admin.get_key_value(dl_js::GEN_BUCKET).await.expect("the provisioner made the bucket");
    let foreign = GenRecord {
        generation: new_id(),
        epoch: 1,
        sample: Some(0),
        sampled_at: 1,
        claim: None,
    };
    // The relay writes too, so a write with a stale revision is refused. Read the revision, write, and retry.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let foreign_revision = loop {
        let entry = store.entry(entry_key(KEY)).await.expect("read the entry").expect("the entry exists");
        if let Ok(revision) = store.update(entry_key(KEY), foreign.to_bytes(), entry.revision).await {
            break revision;
        }
        assert!(tokio::time::Instant::now() < deadline, "the foreign write never stuck within 5 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    wait_for("three foreign writer errors", || async { check_errors(&snap, "foreign_writer") >= 3 }).await;
    assert_eq!(resets(&snap), 0, "a foreign writer is an error and no reset");
    assert!(logs.text().contains("another writer set"), "the error log names the foreign writer: {}", logs.text());
    assert_eq!(
        kv_record(&admin).await.expect("a record").generation,
        foreign.generation,
        "the relay writes nothing over a foreign entry"
    );
    // A rewrite with the same bytes keeps the generation, but it moves the revision.
    let entry = store.entry(entry_key(KEY)).await.expect("read the entry").expect("the entry exists");
    assert_eq!(entry.revision, foreign_revision, "the relay did not write the foreign entry again");
    relay.shut_down().await;
}

#[tokio::test]
async fn a_redis_at_its_memory_limit_counts_a_redis_error_and_keeps_the_gauge() {
    let Some((node, redis_url)) = redis_7_4("decision_log_redis memory limit").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis memory limit").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    block_publishes(&admin).await;
    for seq in 1..=3 {
        xadd(&node, &event_with_org(seq, ORG_A), 0).await;
    }
    let relay = start_relay(&redis_url, &nats_url, "i1").await;
    wait_for("the gauge reads 3", || async { snap.gauge("gateway_decision_log_backlog") == Some(3.0) }).await;
    wait_for("the first KV record", || async { kv_record(&admin).await.is_some() }).await;
    redis_cli(&node, &["CONFIG", "SET", "maxmemory-policy", "noeviction"]).await;
    redis_cli(&node, &["CONFIG", "SET", "maxmemory", "1"]).await;
    wait_for("a redis check error", || async { check_errors(&snap, "redis") > 0 }).await;
    assert_eq!(resets(&snap), 0, "a refused write is no reset");
    // XADD is refused at the limit, so XTRIM (which frees memory and is allowed) moves XLEN between the ticks.
    redis_cli(&node, &["XTRIM", KEY, "MAXLEN", "1"]).await;
    assert_eq!(xlen(&node).await, 1, "the trim took effect");
    wait_for("the gauge follows XLEN to 1", || async { snap.gauge("gateway_decision_log_backlog") == Some(1.0) }).await;
    assert_eq!(resets(&snap), 0, "no reset at the limit");
    redis_cli(&node, &["CONFIG", "SET", "maxmemory", "0"]).await;
    wait_for_kv_steps(&admin, 1).await;
    let baseline = check_errors(&snap, "redis");
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(check_errors(&snap, "redis"), baseline, "no new redis error after the limit is lifted");
    assert_eq!(resets(&snap), 0, "no reset after the limit is lifted");
    relay.shut_down().await;
}

#[tokio::test]
async fn two_fresh_relays_on_a_first_deploy_count_nothing() {
    let Some((_node, redis_url)) = redis_7_4("decision_log_redis first deploy").await else {
        return;
    };
    let Some((_nats, nats_url)) = start_nats(None, "decision_log_redis first deploy").await else {
        return;
    };
    provision_bucket(&nats_url).await;
    let recorder = DebuggingRecorder::new();
    let snap = Tally::new(recorder.snapshotter());
    let _local = metrics::set_default_local_recorder(&recorder);
    let admin = test_js(&nats_url).await;
    let (a, b) = tokio::join!(start_relay(&redis_url, &nats_url, "a"), start_relay(&redis_url, &nats_url, "b"));
    wait_for_kv_steps(&admin, 3).await;
    assert_eq!(resets(&snap), 0, "a first deploy is not a loss");
    for kind in CHECK_ERROR_KINDS {
        assert_eq!(check_errors(&snap, kind), 0, "no check error of kind {kind}");
    }
    a.shut_down().await;
    b.shut_down().await;
}
