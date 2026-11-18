// SPDX-License-Identifier: Apache-2.0

//! SMA-755 Task 13, § 5.11: the reader and the CLI binary against a real JetStream. Six events of two orgs
//! go into a stream made with `stream_config()`. The reader returns the four of one org, in stream order. The
//! real `paigasus-gateway decisions export` binary writes a trace file of `@` lines only and exits 0.

use std::path::Path;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use paigasus_gateway::adapters::decision_log::jetstream as dl_js;
use paigasus_gateway::adapters::decision_log::proto::{from_event, to_event};
use paigasus_gateway::adapters::decision_log::reader;
use paigasus_gateway::application::decision_export::parse_args;
use paigasus_gateway_core::decision_log::{DecisionRecord, RecordHeader, ReportedCall, build_response};
use paigasus_gateway_core::{Basis, BlockCode, DecisionContext, ReportDecision, ReportEntry};
use paigasus_kernel::CedarUid;
use prost::Message;
use tempfile::TempDir;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::ImageExt;
use uuid::Uuid;

const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
const ORG_B: &str = "0190a100-0000-7000-8000-0000000000b2";
const T0: u64 = 1_700_000_000;
const RANGE: [&str; 4] = ["--since", "2023-11-14T22:13:20Z", "--until", "2023-11-14T22:13:30Z"];

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

fn run_cli(url: &str, out: &Path, report: &Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-gateway"))
        .args(["decisions", "export", "--org", ORG_A])
        .args(RANGE)
        .arg("--out")
        .arg(out)
        .arg("--report")
        .arg(report)
        .env("PAIGASUS_NATS_URL", url)
        .output()
        .expect("run the paigasus-gateway binary")
}

#[tokio::test]
async fn the_reader_and_the_binary_export_one_org() {
    let cmd = NatsServerCmd::default().with_jetstream();
    let Some(node) = paigasus_test_docker::start_or_skip(Nats::default().with_cmd(&cmd), "decision_export_nats").await else {
        return;
    };
    let port = paigasus_test_docker::mapped_port(&node, 4222, "decision_export_nats").await;
    let url = format!("nats://127.0.0.1:{port}");
    let js = jetstream::new(async_nats::connect(&url).await.expect("the test client connects"));
    js.create_stream(dl_js::stream_config()).await.expect("the stream is made");

    // Six events, two orgs, in this stream order. The sequences of ORG_A are 1 to 4.
    let events = [
        record(ORG_A, T0 + 1, 1, 1),
        record(ORG_B, T0 + 1, 1, 2),
        record(ORG_A, T0 + 2, 2, 3),
        record(ORG_A, T0 + 3, 3, 4),
        record(ORG_B, T0 + 2, 2, 5),
        record(ORG_A, T0 + 4, 4, 6),
    ];
    for r in &events {
        let subject = dl_js::subject_for(&r.org.unwrap().to_string()).expect("a subject");
        let ack = js.publish(subject, Bytes::from(to_event(r).encode_to_vec())).await.expect("publish");
        ack.await.expect("the PubAck");
    }

    let dir = TempDir::new().unwrap();
    let url_file = dir.path().join("nats-url");
    std::fs::write(&url_file, format!("{url}\n")).unwrap();
    let mut argv: Vec<String> = ["--org", ORG_A].iter().chain(RANGE.iter()).map(ToString::to_string).collect();
    argv.extend(["--nats-url-file".to_owned(), url_file.display().to_string()]);
    let args = parse_args(argv).expect("arguments");
    let read = reader::read(&args).await.expect("the reader reads");
    assert_eq!(read.len(), 4, "only the events of ORG_A");
    let ids: Vec<u128> = read.iter().map(|e| from_event(pb_decode(&e.bytes)).expect("an event").decision_id.as_u128()).collect();
    assert_eq!(ids, [1, 3, 4, 6], "in stream order");
    assert!(read.iter().all(|e| e.stored_at > SystemTime::UNIX_EPOCH + Duration::from_secs(T0)), "stored_at is the JetStream time");

    let out = dir.path().join("trace.log");
    let report = dir.path().join("report.txt");
    let (url2, out2, report2) = (url.clone(), out.clone(), report.clone());
    let output = tokio::task::spawn_blocking(move || run_cli(&url2, &out2, &report2)).await.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    let trace = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = trace.lines().collect();
    assert_eq!(lines.len(), 4, "{trace}");
    assert!(lines.iter().all(|l| l.starts_with('@')), "the trace holds only @ lines: {trace}");
    for (line, ts) in lines.iter().zip([1, 2, 3, 4]) {
        assert!(line.starts_with(&format!("@{} ", T0 + ts)), "{line}");
    }
    assert!(output.stdout.is_empty(), "with --out the trace is not on stdout");
    assert!(
        stderr.starts_with(&format!("exported 4 decisions for org {ORG_A} from 2023-11-14T22:13:20Z to 2023-11-14T22:13:30Z (until exclusive)\n")),
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

fn pb_decode(bytes: &Bytes) -> paigasus_proto::paigasus::gateway::v1::ToolDecisionEvent {
    paigasus_proto::paigasus::gateway::v1::ToolDecisionEvent::decode(bytes.as_ref()).expect("a ToolDecisionEvent")
}
