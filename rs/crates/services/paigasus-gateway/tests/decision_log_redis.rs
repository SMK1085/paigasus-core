// SPDX-License-Identifier: Apache-2.0

//! The decision-log writer against a real Redis (SMA-755 § 5.6, § 5.8). Redis is read only through
//! `redis-cli` in the container, as the limit-store tests do: `repo:redis-connect-single-site` bans a second
//! Rust Redis client. A test that needs only a dead or silent Redis uses the `paigasus-redis` test support
//! and needs no container.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::decision_log::redis_conn::breaker_metrics;
use paigasus_gateway::adapters::decision_log::{RedisDecisionLog, RedisSlot, WriterSettings, connect_loop, proto, run_writer};
use paigasus_gateway::domain::decision_log::DecisionLog;
use paigasus_gateway_core::decision_log::{DecisionRecord, RecordHeader, ReportedCall, build_response};
use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
use prost::Message;
use secrecy::SecretString;
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

/// A running total of the counters. `Snapshotter::snapshot` resets each counter it reads, so a poll must add
/// each snapshot to a total.
struct Tally {
    snapshotter: Snapshotter,
    totals: RefCell<HashMap<SeriesKey, u64>>,
}

impl Tally {
    fn new(snapshotter: Snapshotter) -> Self {
        Tally {
            snapshotter,
            totals: RefCell::new(HashMap::new()),
        }
    }

    fn get(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
        let mut totals = self.totals.borrow_mut();
        for (key, _, _, value) in self.snapshotter.snapshot().into_vec() {
            if let DebugValue::Counter(n) = value {
                let key = key.key();
                let mut have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
                have.sort();
                *totals.entry((key.name().to_owned(), have)).or_insert(0) += n;
            }
        }
        let mut want: Vec<(String, String)> = labels.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        want.sort();
        totals.get(&(name.to_owned(), want)).copied().unwrap_or(0)
    }

    fn dropped(&self, reason: &str) -> u64 {
        self.get("gateway_decision_log_dropped_total", &[("reason", reason)])
    }

    fn written(&self) -> u64 {
        self.get("gateway_decision_log_written_total", &[])
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
