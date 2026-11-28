// SPDX-License-Identifier: Apache-2.0

//! The SIGKILL test on the real binary (spec §6.11). This is the "Done when" test of SMA-790.
//!
//! Process 1 boots with one agent on the file-scripted model and the file barrier at `after_run`.
//! The test posts one CloudEvent through the HTTP ingress (with the fake IAM). Process 1 claims
//! the event, runs the model (it creates `/memories/first.md` in its workspace), creates the
//! barrier file and waits. The test sends SIGKILL. Process 2 boots on the same database with a
//! script that creates `/memories/second.md` and no barrier. After the lease ttl it takes the
//! inbox and completes the event. Process 1 never reached its commit, so it left nothing
//! committed.
//!
//! The test needs the `test-model` feature. Without it, the file holds one test that fails, so a
//! run without the feature cannot report a silent pass. The Moon `test` task passes the feature.
//!
//! Mutation note: removing the step-1 lease check of the run commit cannot make this test fail,
//! because process 1 never reaches its commit. `worker_crash_pg` proves that fence in process.
//! A barrier that does not wait makes this test fail, because process 1 then commits.

#[cfg(not(feature = "test-model"))]
#[test]
fn the_test_model_feature_is_on() {
    panic!("worker_sigkill_pg needs --features test-model; the Moon test task passes it");
}

#[cfg(feature = "test-model")]
mod support;

#[cfg(feature = "test-model")]
use std::path::{Path, PathBuf};
#[cfg(feature = "test-model")]
use std::time::{Duration, Instant};

#[cfg(feature = "test-model")]
use paigasus_agent_host::adapters::test_model::{BARRIER_FILE_ENV, BARRIER_STAGE_ENV, SCRIPT_ENV};
#[cfg(feature = "test-model")]
use sea_orm::DatabaseConnection;
#[cfg(feature = "test-model")]
use support::fake_iam::{FakeIam, serve_grpc};
#[cfg(feature = "test-model")]
use support::inbox::{insert_agent, insert_inbox};
#[cfg(feature = "test-model")]
use uuid::Uuid;

#[cfg(feature = "test-model")]
const READY_BUDGET: Duration = Duration::from_secs(90);
/// The longest wait for the barrier file of process 1.
#[cfg(feature = "test-model")]
const BARRIER_BUDGET: Duration = Duration::from_secs(30);
/// The longest wait for process 1 to exit after SIGKILL.
#[cfg(feature = "test-model")]
const EXIT_BUDGET: Duration = Duration::from_secs(10);
/// The longest wait for process 2 to complete the event.
#[cfg(feature = "test-model")]
const DONE_BUDGET: Duration = Duration::from_secs(30);
#[cfg(feature = "test-model")]
const POLL: Duration = Duration::from_millis(50);
#[cfg(feature = "test-model")]
const TOKEN: &str = "sigkill-secret";

/// A temporary directory that is removed when it drops, also after a failed assertion.
#[cfg(feature = "test-model")]
struct TempDir(PathBuf);

#[cfg(feature = "test-model")]
impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("agent-host-sigkill-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create the test dir");
        Self(dir)
    }
}

#[cfg(feature = "test-model")]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a script whose one turn creates `/memories/<file>`, and returns its path.
#[cfg(feature = "test-model")]
fn script(dir: &Path, file: &str) -> PathBuf {
    let path = dir.join(format!("script-{file}.json"));
    let turns = serde_json::json!([[["c1", "memory", {"command": "create", "path": format!("/memories/{file}"), "file_text": format!("written by {file}")}]]]);
    std::fs::write(&path, turns.to_string()).expect("write the script");
    path
}

/// The environment of one process: the fake IAM, the script, and the extra variables.
#[cfg(feature = "test-model")]
fn env(iam: std::net::SocketAddr, script: &Path, extra: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("AGENT_HOST_IAM__GRPC_ADDR", format!("http://{iam}")),
        ("AGENT_HOST_IAM__TLS__MODE", "loopback_insecure".to_owned()),
        (SCRIPT_ENV, script.display().to_string()),
        ("RUST_LOG", "paigasus_agent_host=info,warn".to_owned()),
    ];
    out.extend_from_slice(extra);
    out
}

#[cfg(feature = "test-model")]
async fn n(db: &DatabaseConnection, sql: &str) -> i64 {
    support::scalar_i64(db, sql).await
}

#[cfg(feature = "test-model")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_killed_worker_is_taken_over_and_the_event_is_done_once() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping the SIGKILL test: Docker unavailable");
        return;
    };
    let (org, inbox, agent, owner) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (iam, server) = serve_grpc(FakeIam::new(TOKEN, owner)).await;

    // Step 1: the config of a short lease and one worker.
    let dir = TempDir::new();
    std::fs::write(
        dir.0.join("agent-host.toml"),
        "[lease]\nttl_secs = 3\nheartbeat_interval_secs = 1\n\n[worker]\nconcurrency = 1\npoll_interval_ms = 200\n",
    )
    .expect("write agent-host.toml");

    // Step 2: process 1, with the barrier after the run.
    let barrier = dir.0.join("barrier");
    let first_env = env(
        iam,
        &script(&dir.0, "first.md"),
        &[(BARRIER_STAGE_ENV, "after_run".to_owned()), (BARRIER_FILE_ENV, barrier.display().to_string())],
    );
    let (mut first, port) = support::spawn_service_in(&url, Some(&dir.0), &first_env);
    support::wait_for_status(&mut first, port, "/readyz", 200, READY_BUDGET).await;
    let db = support::connect(&url).await;
    insert_agent(&db, org, agent, "scripted", "Use the memory tool.", &["memory"]).await;
    insert_inbox(&db, org, inbox, agent, owner).await;

    // Step 3: one CloudEvent through the HTTP ingress.
    // reqwest 0.13 needs an installed CryptoProvider before it builds a `Client` (SMA-809).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let body = serde_json::json!({"specversion": "1.0", "id": "evt-1", "source": "caller", "type": "user.message", "data": {"text": "remember this"}});
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/inboxes/{inbox}/events"))
        .header("content-type", "application/cloudevents+json")
        .header("authorization", format!("Bearer {TOKEN}"))
        .timeout(Duration::from_secs(10))
        .body(body.to_string())
        .send()
        .await
        .expect("post the event");
    assert_eq!(resp.status().as_u16(), 202, "the ingress must accept the event; log:\n{}", first.tail());

    // Step 4: wait for the barrier, then SIGKILL process 1 and reap it.
    let started = Instant::now();
    while !barrier.exists() {
        if let Some(exit) = first.child.try_wait().expect("try_wait") {
            panic!("process 1 exited ({exit}) before the barrier; log:\n{}", first.tail());
        }
        assert!(
            started.elapsed() < BARRIER_BUDGET,
            "process 1 did not reach the barrier within {BARRIER_BUDGET:?}; log:\n{}",
            first.tail()
        );
        tokio::time::sleep(POLL).await;
    }
    // `Child::kill` sends SIGKILL on Unix.
    first.child.kill().expect("SIGKILL process 1");
    let started = Instant::now();
    let exit = loop {
        if let Some(exit) = first.child.try_wait().expect("try_wait") {
            break exit;
        }
        assert!(started.elapsed() < EXIT_BUDGET, "process 1 did not exit within {EXIT_BUDGET:?} after SIGKILL; log:\n{}", first.tail());
        tokio::time::sleep(POLL).await;
    };
    assert!(!exit.success(), "process 1 must die by the signal, got {exit}");
    let first_log = first.tail();

    // Step 5: process 2 on the same directory and database, with no barrier.
    let second_env = env(iam, &script(&dir.0, "second.md"), &[]);
    let (mut second, port) = support::spawn_service_in(&url, Some(&dir.0), &second_env);
    support::wait_for_status(&mut second, port, "/readyz", 200, READY_BUDGET).await;

    // Step 6: wait until process 2 completed the event, after the lease of process 1 expired.
    let done = format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND status = 'done'");
    let started = Instant::now();
    while n(&db, &done).await != 1 {
        if let Some(exit) = second.child.try_wait().expect("try_wait") {
            panic!("process 2 exited ({exit}) before the event was done; log:\n{}", second.tail());
        }
        assert!(
            started.elapsed() < DONE_BUDGET,
            "process 2 did not complete the event within {DONE_BUDGET:?}; process 2 log:\n{}\nprocess 1 log:\n{first_log}",
            second.tail()
        );
        tokio::time::sleep(POLL).await;
    }

    let log = second.tail();
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}'")).await,
        1,
        "the inbox holds one event"
    );
    assert_eq!(
        n(&db, &format!("SELECT attempts::bigint AS n FROM inbox_event WHERE inbox_id = '{inbox}'")).await,
        2,
        "one attempt of process 1 and one of process 2; log:\n{log}"
    );
    assert_eq!(n(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}'")).await, 2, "one run of each process");
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}' AND status = 'completed'")).await,
        1,
        "process 2 completed one run; log:\n{log}"
    );
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM run WHERE inbox_id = '{inbox}' AND status = 'running'")).await,
        1,
        "the run of process 1 is still running"
    );
    let committed = format!("(SELECT id FROM run WHERE inbox_id = '{inbox}' AND status = 'completed')");
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM inbox_event WHERE inbox_id = '{inbox}' AND claimed_run_id = {committed}")).await,
        1,
        "the committed run of process 2 served the event"
    );
    // The run of process 1 has the older lease token.
    assert!(
        support::scalar_bool(
            &db,
            &format!("SELECT (SELECT lease_token FROM run WHERE inbox_id = '{inbox}' AND status = 'running') < (SELECT lease_token FROM run WHERE id = {committed}) AS v")
        )
        .await,
        "the running run is the run of process 1"
    );

    let scope = format!("org_id = '{org}' AND agent_id = '{agent}' AND principal_id = '{owner}'");
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM memory_file WHERE {scope} AND path = '/second.md'")).await,
        1,
        "process 2 stored /second.md once"
    );
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM memory_file WHERE {scope} AND path = '/first.md'")).await,
        0,
        "process 1 committed /first.md"
    );
    assert!(
        n(&db, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}'")).await > 0,
        "process 2 persisted no session event"
    );
    assert_eq!(
        n(&db, &format!("SELECT count(*) AS n FROM session_event WHERE inbox_id = '{inbox}' AND run_id <> {committed}")).await,
        0,
        "a session event is not of the committed run"
    );
    server.abort();
}
