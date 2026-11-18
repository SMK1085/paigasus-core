// SPDX-License-Identifier: Apache-2.0

//! SMA-755 Task 13, § 5.11: the reader and the CLI binary against a real JetStream. Six events of two orgs
//! go into a stream made with `stream_config()`. The reader returns the four of one org, in stream order. The
//! real `paigasus-gateway decisions export` binary writes a trace file of `@` lines only and exits 0. The
//! reader is bounded by the stored time (`until + max_lag`) and by a raw count (`2 x max_events`).
//!
//! The reader bounds by the JetStream stored time, which is the real clock. So every range here is made
//! around "now", and `decided_at` values are seconds after the start of the test.

use std::path::Path;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use paigasus_gateway::adapters::decision_log::jetstream as dl_js;
use paigasus_gateway::adapters::decision_log::proto::{from_event, to_event};
use paigasus_gateway::adapters::decision_log::reader::{self, ReaderError};
use paigasus_gateway::application::decision_export::parse_args;
use paigasus_gateway_core::decision_log::{DecisionRecord, RecordHeader, ReportedCall, build_response};
use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
use paigasus_kernel::CedarUid;
use prost::Message;
use tempfile::TempDir;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
const ORG_B: &str = "0190a100-0000-7000-8000-0000000000b2";

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

fn rfc3339(secs: u64) -> String {
    OffsetDateTime::from_unix_timestamp(i64::try_from(secs).unwrap()).unwrap().format(&Rfc3339).unwrap()
}

/// `--since` and `--until` around `t0`: `[t0 - 10, t0 + 60)`. The stored time of an event of the test is below
/// `t0 + 60`, so the time bound `until + max_lag` does not stop the read.
fn range(t0: u64) -> Vec<String> {
    ["--since".to_owned(), rfc3339(t0 - 10), "--until".to_owned(), rfc3339(t0 + 60)].to_vec()
}

fn record(org: &str, decided: u64, seq: u64, id: u128) -> DecisionRecord {
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
    let ctx = DecisionContext::new("prn:pgs:iam:::user/u", format!("prn:pgs:iam:::organization/{org}"), Some("req-1".into()));
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(decided);
    let header = RecordHeader::new(Uuid::from_u128(id), at, Uuid::from_u128(1), seq, Some(Uuid::try_parse(org).unwrap()));
    let mut r = build_response(header, &ctx, &ReportedCall::new(entry, None, None));
    r.principal = Some(CedarUid {
        entity_type: "Pgs::Iam::User".into(),
        entity_id: "u".into(),
    });
    r
}

struct Stack {
    _node: ContainerAsync<Nats>,
    url: String,
    js: jetstream::Context,
}

async fn stack(what: &str) -> Option<Stack> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let node = paigasus_test_docker::start_or_skip(Nats::default().with_cmd(&cmd), what).await?;
    let port = paigasus_test_docker::mapped_port(&node, 4222, what).await;
    let url = format!("nats://127.0.0.1:{port}");
    let js = jetstream::new(async_nats::connect(&url).await.expect("the test client connects"));
    js.create_stream(dl_js::stream_config()).await.expect("the stream is made");
    Some(Stack { _node: node, url, js })
}

async fn publish(js: &jetstream::Context, r: &DecisionRecord) {
    let subject = dl_js::subject_for(&r.org.unwrap().to_string()).expect("a subject");
    let ack = js.publish(subject, Bytes::from(to_event(r).encode_to_vec())).await.expect("publish");
    ack.await.expect("the PubAck");
}

fn args_with(extra: &[String], url_file: &Path) -> paigasus_gateway::application::decision_export::ExportArgs {
    let mut argv: Vec<String> = vec!["--org".to_owned(), ORG_A.to_owned()];
    argv.extend(extra.iter().cloned());
    argv.extend(["--nats-url-file".to_owned(), url_file.display().to_string()]);
    parse_args(argv).expect("arguments")
}

fn run_cli(url: &str, extra: &[String], out: &Path, report: &Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-gateway"))
        .args(["decisions", "export", "--org", ORG_A])
        .args(extra)
        .arg("--out")
        .arg(out)
        .arg("--report")
        .arg(report)
        .env("PAIGASUS_NATS_URL", url)
        .output()
        .expect("run the paigasus-gateway binary")
}

fn ids(read: &[paigasus_gateway::application::decision_export::Exported]) -> Vec<u128> {
    read.iter()
        .map(|e| {
            let ev = paigasus_proto::paigasus::gateway::v1::ToolDecisionEvent::decode(e.bytes.as_ref()).expect("a ToolDecisionEvent");
            from_event(ev).expect("an event").decision_id.as_u128()
        })
        .collect()
}

#[tokio::test]
async fn the_reader_and_the_binary_export_one_org() {
    let Some(stack) = stack("decision_export_nats").await else {
        return;
    };
    let t0 = now_secs();
    // Six events, two orgs, in this stream order. The sequences of ORG_A are 1 to 4.
    for r in [
        record(ORG_A, t0 + 1, 1, 1),
        record(ORG_B, t0 + 1, 1, 2),
        record(ORG_A, t0 + 2, 2, 3),
        record(ORG_A, t0 + 3, 3, 4),
        record(ORG_B, t0 + 2, 2, 5),
        record(ORG_A, t0 + 4, 4, 6),
    ] {
        publish(&stack.js, &r).await;
    }

    let dir = TempDir::new().unwrap();
    let url_file = dir.path().join("nats-url");
    std::fs::write(&url_file, format!("{}\n", stack.url)).unwrap();
    let args = args_with(&range(t0), &url_file);
    let read = reader::read(&args).await.expect("the reader reads");
    assert_eq!(read.len(), 4, "only the events of ORG_A");
    assert_eq!(ids(&read), [1, 3, 4, 6], "in stream order");
    assert!(read.iter().all(|e| e.stored_at >= SystemTime::UNIX_EPOCH + Duration::from_secs(t0)), "stored_at is the JetStream time");

    let out = dir.path().join("trace.log");
    let report = dir.path().join("report.txt");
    let (url2, out2, report2, extra) = (stack.url.clone(), out.clone(), report.clone(), range(t0));
    let output = tokio::task::spawn_blocking(move || run_cli(&url2, &extra, &out2, &report2)).await.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    let trace = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = trace.lines().collect();
    assert_eq!(lines.len(), 4, "{trace}");
    assert!(lines.iter().all(|l| l.starts_with('@')), "the trace holds only @ lines: {trace}");
    for (line, ts) in lines.iter().zip([1, 2, 3, 4]) {
        assert!(line.starts_with(&format!("@{} ", t0 + ts)), "{line}");
    }
    assert!(output.stdout.is_empty(), "with --out the trace is not on stdout");
    let r = range(t0);
    assert!(
        stderr.starts_with(&format!("exported 4 decisions for org {ORG_A} from {} to {} (until exclusive)\n", r[1], r[3])),
        "{stderr}"
    );
    assert!(stderr.contains("sequence gaps: none"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&report).unwrap(), stderr, "the report is the stderr summary");

    // A bad argument is exit 1, and a URL on argv is refused.
    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-gateway"))
        .args(["decisions", "export", "--org", ORG_A, "--nats-url", "nats://x"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
}

#[tokio::test]
async fn the_reader_stops_at_the_first_message_stored_after_until_plus_max_lag() {
    let Some(stack) = stack("decision_export_nats").await else {
        return;
    };
    let t0 = now_secs();
    publish(&stack.js, &record(ORG_A, t0, 1, 1)).await;
    publish(&stack.js, &record(ORG_A, t0, 2, 2)).await;
    // `until` is the whole second of this instant, so `until + 1 s` is below the stored time of the next two.
    let until = now_secs();
    tokio::time::sleep(Duration::from_secs(4)).await;
    publish(&stack.js, &record(ORG_A, t0, 3, 3)).await;
    publish(&stack.js, &record(ORG_A, t0, 4, 4)).await;

    let dir = TempDir::new().unwrap();
    let url_file = dir.path().join("nats-url");
    std::fs::write(&url_file, &stack.url).unwrap();
    let extra: Vec<String> = [
        "--since".to_owned(),
        rfc3339(until - 100),
        "--until".to_owned(),
        rfc3339(until),
        "--max-lag".to_owned(),
        "1s".to_owned(),
    ]
    .to_vec();
    let read = reader::read(&args_with(&extra, &url_file)).await.expect("the reader reads and ends");
    assert_eq!(ids(&read), [1, 2], "the events stored after until + max_lag are not returned");
}

#[tokio::test]
async fn the_reader_stops_with_too_many_above_twice_max_events() {
    let Some(stack) = stack("decision_export_nats").await else {
        return;
    };
    let t0 = now_secs();
    for n in 1..=6u64 {
        publish(&stack.js, &record(ORG_A, t0 + n, n, u128::from(n))).await;
    }
    let dir = TempDir::new().unwrap();
    let url_file = dir.path().join("nats-url");
    std::fs::write(&url_file, &stack.url).unwrap();

    // The cap is 2 x max_events raw messages: 6 messages are inside the cap of --max-events 3 ...
    let mut three = range(t0);
    three.extend(["--max-events".to_owned(), "3".to_owned()]);
    let read = reader::read(&args_with(&three, &url_file)).await.expect("6 messages are not above the cap of 6");
    assert_eq!(read.len(), 6);
    // ... and above the cap of --max-events 2, which is 4. The reader stops before it holds all six.
    let mut two = range(t0);
    two.extend(["--max-events".to_owned(), "2".to_owned()]);
    let err = reader::read(&args_with(&two, &url_file)).await.expect_err("6 messages are above the cap of 4");
    assert!(matches!(err, ReaderError::TooMany), "{err}");

    // The binary exits 4 and writes no trace.
    let out = dir.path().join("trace.log");
    let report = dir.path().join("report.txt");
    let (url, out2, report2) = (stack.url.clone(), out.clone(), report.clone());
    let output = tokio::task::spawn_blocking(move || run_cli(&url, &two, &out2, &report2)).await.unwrap();
    assert_eq!(output.status.code(), Some(4), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(!out.exists(), "a failed export writes no trace");
}
