// SPDX-License-Identifier: Apache-2.0

//! The boot sequence of the built binary against a real Postgres (spec §6.8). The tests start the
//! binary as a child process, because the boot order lives in `main.rs`. A test that rebuilt the
//! order in the test process would not test it.
//!
//! The migration lock is held from a second session in the tests that need an open "migrating"
//! phase, so they do not race a fast migration.

mod support;

use std::time::Duration;

use paigasus_agent_host::adapters::http::error;
use paigasus_agent_host::adapters::persistence::migration_lock::MIGRATION_LOCK_KEY;
use paigasus_agent_host::adapters::persistence::{MIGRATION_TABLE, Migrator};
use paigasus_proto::paigasus::common::v1::ErrorReason;
use sea_orm::{ConnectionTrait, SqlErr};
use sea_orm_migration::MigratorTrait;

const READY_BUDGET: Duration = Duration::from_secs(90);

async fn applied(db: &sea_orm::DatabaseConnection) -> i64 {
    support::scalar_i64(db, &format!("SELECT count(*)::bigint AS n FROM {MIGRATION_TABLE}")).await
}

/// The number of migrations of this crate.
fn migrations() -> i64 {
    i64::try_from(Migrator::migrations().len()).expect("a small count")
}

#[tokio::test]
async fn a_fresh_database_boots_and_applies_every_migration_once() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    assert_eq!(applied(&db).await, migrations(), "every migration must be recorded exactly once");
    assert!(support::scalar_bool(&db, "SELECT to_regclass('inbox') IS NOT NULL AS v").await, "m0001 must create the inbox table");
}

#[tokio::test]
async fn a_second_boot_applies_nothing() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    {
        let (mut first, port) = support::spawn_service(&url);
        support::wait_for_status(&mut first, port, "/readyz", 200, READY_BUDGET).await;
    }
    assert_eq!(applied(&db).await, migrations());

    let (mut second, port) = support::spawn_service(&url);
    support::wait_for_status(&mut second, port, "/readyz", 200, READY_BUDGET).await;
    assert_eq!(applied(&db).await, migrations(), "a second boot must not apply a migration again");
}

#[tokio::test]
async fn the_inbox_unique_constraint_holds() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let insert = "INSERT INTO inbox (id, org_id, agent_id, principal_id) VALUES \
                  (gen_random_uuid(), '33333333-3333-3333-3333-333333333333', '11111111-1111-1111-1111-111111111111', \
                  '22222222-2222-2222-2222-222222222222')";
    // Without an agent row the foreign key refuses the inbox (SQLSTATE 23503).
    let err = db.execute_unprepared(insert).await.expect_err("an inbox without an agent must fail");
    assert!(matches!(err.sql_err(), Some(SqlErr::ForeignKeyConstraintViolation(_))), "expected SQLSTATE 23503, got {err:?}");
    db.execute_unprepared(
        "INSERT INTO agent (id, org_id, name, model, instructions) VALUES \
         ('11111111-1111-1111-1111-111111111111', '33333333-3333-3333-3333-333333333333', 'a', 'scripted', 'test')",
    )
    .await
    .expect("insert the agent");
    db.execute_unprepared(insert).await.expect("the first insert");
    let err = db.execute_unprepared(insert).await.expect_err("the second insert must fail");
    // `sql_err` maps SQLSTATE 23505 (unique_violation) to this variant.
    assert!(matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))), "expected SQLSTATE 23505, got {err:?}");
}

#[tokio::test]
async fn a_held_lock_reports_migrating() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let holder = support::connect_pinned(&url).await;
    // `pg_try_advisory_lock` returns a boolean, so the test can check its own set-up.
    assert!(
        support::scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must acquire the lock"
    );

    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/healthz", 200, Duration::from_secs(30)).await;

    let (status, body) = support::http_status(port, "/readyz").await.expect("readyz");
    assert_eq!(status, 503, "/readyz is 503 while migrating");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).expect("json"), serde_json::json!({ "status": "migrating" }));

    let (status, body) = support::http_status(port, "/v1/x").await.expect("app route while migrating");
    assert_eq!(status, 503, "an app route is 503 while migrating");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(parsed["error"]["code"], ErrorReason::ServiceMigrating.as_wire_reason().expect("registered reason"));

    assert!(
        support::scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must release the lock"
    );
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let (status, body) = support::http_status(port, "/v1/x").await.expect("app route when ready");
    assert_eq!(status, 404);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    let (code, _message) = error::NOT_FOUND;
    assert_eq!(parsed["error"]["code"], code);
}

#[tokio::test]
async fn sigterm_during_the_lock_wait_exits_promptly() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let holder = support::connect_pinned(&url).await;
    assert!(
        support::scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must acquire the lock"
    );

    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/healthz", 200, Duration::from_secs(30)).await;

    let pid = child.child.id();
    let status = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status().expect("run kill(1)");
    assert!(status.success(), "kill -TERM {pid} failed: {status:?}");

    // The lock wait is 60 seconds. The service must not wait for it.
    let started = std::time::Instant::now();
    let exit = loop {
        if let Some(exit) = child.child.try_wait().expect("try_wait") {
            break exit;
        }
        assert!(started.elapsed() < Duration::from_secs(15), "SIGTERM must end the lock wait; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(exit.success(), "the service must exit with status 0 after SIGTERM, got {exit}; log:\n{}", child.tail());
}

/// The agent-host sample lines of a Prometheus text exposition. Header lines (`# HELP`,
/// `# TYPE`) are skipped: a header can exist with no series, so a match on one proves nothing.
fn agent_host_samples(exposition: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = exposition.lines().filter(|l| !l.starts_with('#') && l.starts_with("agent_host_")).collect();
    lines.sort_unstable();
    lines
}

/// The sum of the values of the `agent_host_http_requests_total` sample lines.
fn request_total(exposition: &str) -> f64 {
    agent_host_samples(exposition)
        .into_iter()
        .filter(|l| l.starts_with("agent_host_http_requests_total{"))
        .map(|l| l.rsplit_once(' ').expect("a value").1.trim().parse::<f64>().expect("a number"))
        .sum()
}

#[tokio::test]
async fn metrics_count_requests_with_the_agent_host_prefix() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let (mut child, port) = support::spawn_service(&url);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    // `/metrics` is on the HTTP port, because `metrics.addr` is unset.
    let (status, before) = support::http_status(port, "/metrics").await.expect("metrics");
    assert_eq!(status, 200);

    // The probes stay outside the metrics layer: they add no agent-host sample.
    for _ in 0..3 {
        support::http_status(port, "/healthz").await.expect("healthz");
        support::http_status(port, "/readyz").await.expect("readyz");
    }
    let (_, after_probes) = support::http_status(port, "/metrics").await.expect("metrics");
    // The scheduler's first pass runs at boot and can end between the snapshots, so skip its series.
    let probe_samples = |text: &str| -> Vec<String> { agent_host_samples(text).into_iter().filter(|l| !l.starts_with("agent_host_scheduler_")).map(str::to_owned).collect() };
    assert_eq!(probe_samples(&before), probe_samples(&after_probes), "probes must not add an agent_host_* sample");

    let base = request_total(&after_probes);
    let (status, _) = support::http_status(port, "/v1/x").await.expect("app route");
    assert_eq!(status, 404);
    let (_, after_route) = support::http_status(port, "/metrics").await.expect("metrics");
    let total = request_total(&after_route);
    assert!(total >= 1.0, "one request on /v1/x must give a sample >= 1; /metrics:\n{after_route}");
    assert!((total - (base + 1.0)).abs() < f64::EPSILON, "the count must grow by exactly 1 (from {base} to {total})");
}

/// The inbox of a timer boot test, with one due follow-up. Its `next_fire_at` is a minute old.
async fn insert_due_follow_up(db: &sea_orm::DatabaseConnection) -> uuid::Uuid {
    let inbox = uuid::Uuid::new_v4();
    support::inbox::insert_inbox(db, uuid::Uuid::new_v4(), inbox, uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).await;
    support::timer::insert_follow_up_row(db, inbox, uuid::Uuid::new_v4(), "agent", "date_trunc('second', now() - interval '1 minute')", "check the build").await;
    inbox
}

#[tokio::test]
async fn a_boot_with_timers_fires_a_due_row() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    let (mut child, port) = support::spawn_service_with(&url, &[("AGENT_HOST_TIMERS__POLL_INTERVAL_SECS", "1".to_string())]);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let inbox = insert_due_follow_up(&db).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let events = support::timer::timer_events(&db, inbox).await;
        if let Some(event) = events.first() {
            assert_eq!(event.source, "urn:paigasus:agents:timer:agent");
            assert_eq!(event.status, "pending");
            // Two more poll intervals pass. The follow-up fired once, so no second event may appear.
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert_eq!(support::timer::timer_events(&db, inbox).await.len(), 1, "exactly one event for the timer");
            return;
        }
        assert!(std::time::Instant::now() < deadline, "no timer event within 10 s; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn a_boot_with_timers_off_fires_nothing() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let db = support::connect(&url).await;
    let (mut child, port) = support::spawn_service_with(
        &url,
        &[
            ("AGENT_HOST_TIMERS__POLL_INTERVAL_SECS", "1".to_string()),
            ("AGENT_HOST_TIMERS__ENABLED", "false".to_string()),
            // Keep the log short: the harness keeps only its first 16 KiB, and the SQL logs fill it.
            ("RUST_LOG", "paigasus_agent_host=info,warn".to_string()),
        ],
    );
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    // The control that does not depend on the database: the boot built no scheduler task. `serve`
    // logs "the scheduler loop started" where it spawns the task, before it logs "started". So
    // once "started" is in the log, the absence of the first line is final.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !child.tail().contains("paigasus-agent-host started") {
        assert!(std::time::Instant::now() < deadline, "the service did not log that it started; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let log = child.tail();
    assert!(
        log.contains("timers.enabled is false; the scheduler loop is off"),
        "boot must report that the scheduler is off; log:\n{log}"
    );
    assert!(!log.contains("the scheduler loop started"), "a disabled boot must build no scheduler task; log:\n{log}");

    // The second control: a due row fires no event.
    let inbox = insert_due_follow_up(&db).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        support::timer::timer_events(&db, inbox).await.is_empty(),
        "a disabled scheduler must fire nothing; log:\n{}",
        child.tail()
    );
}

/// Polls the log of `child` until it holds `line`, for at most 10 seconds.
async fn wait_for_log(child: &support::Child, line: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !child.tail().contains(line) {
        assert!(std::time::Instant::now() < deadline, "the log does not hold {line:?}; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn with_no_agents_the_pool_starts_and_takes_nothing() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    // The default `worker.enabled` is true. The database holds no agent row.
    let (mut child, port) = support::spawn_service_with(&url, &[("RUST_LOG", "paigasus_agent_host=info,warn".to_string())]);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;
    wait_for_log(&child, "paigasus-agent-host started").await;
    let log = child.tail();
    assert!(log.contains("the worker pool started"), "boot must start the pool with no agent; log:\n{log}");
    assert!(!log.contains("the worker pool does not start"), "the old warning is gone; log:\n{log}");
    let db = support::connect(&url).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM run").await, 0, "the pool took nothing");
}

/// The agent is in the database with a model that this host does not serve. The boot warns once
/// for it and goes on, and the event of its inbox waits.
#[tokio::test]
async fn an_active_agent_with_an_unknown_model_logs_a_warning_and_boots() {
    let Some((_node, url, db)) = support::inbox::migrated_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let (org, inbox, agent, event) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    support::inbox::insert_agent(&db, org, agent, "gpt-x", "Sort the request.", &[]).await;
    support::inbox::insert_inbox(&db, org, inbox, agent, uuid::Uuid::new_v4()).await;
    support::lease::insert_pending_event(&db, inbox, event, 1).await;

    let (mut child, port) = support::spawn_service_with(&url, &[("RUST_LOG", "paigasus_agent_host=info,warn".to_string())]);
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;
    wait_for_log(&child, "paigasus-agent-host started").await;
    let log = child.tail();
    let prn = paigasus_agent_host_core::ids::AgentId::new(org, agent).as_prn().canonical();
    assert!(log.contains("an active agent names a model that this host does not serve"), "boot must warn; log:\n{log}");
    assert!(log.contains("gpt-x") && log.contains(&prn), "the warning names the model and the agent; log:\n{log}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        support::scalar_bool(&db, &format!("SELECT status = 'pending' AS v FROM inbox_event WHERE id = '{event}'")).await,
        "the event of an unserved agent waits"
    );
}

/// The worker cannot commit an external memory store yet, so the S3 store with the worker on stops
/// the boot before the database is touched. No agent is needed (the agents live in the database).
#[tokio::test]
async fn the_s3_memory_store_with_the_worker_on_fails_the_boot() {
    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let dir = std::env::temp_dir().join(format!("agent-host-boot-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(dir.join("agent-host.toml"), "[memory]\nbackend = \"s3\"\n[memory.s3]\nbucket = \"paigasus-mem\"\nprefix = \"mem\"\n").expect("write agent-host.toml");

    let (mut child, _port) = support::spawn_service_in(&url, Some(&dir), &[]);
    let started = std::time::Instant::now();
    let exit = loop {
        if let Some(exit) = child.child.try_wait().expect("try_wait") {
            break exit;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the S3 store with the worker on must stop the boot; log:\n{}",
            child.tail()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!exit.success(), "the boot must fail, got {exit}; log:\n{}", child.tail());
    // The reader threads can lag the exit by a moment.
    wait_for_log(&child, "the worker supports only the Postgres memory store for now").await;
}

/// The composition root end to end: with one agent row on the file-scripted model, the pool starts
/// and a run moves a pending event to `done`.
#[cfg(feature = "test-model")]
#[tokio::test]
async fn with_an_agent_the_pool_serves_a_pending_event() {
    use paigasus_agent_host::adapters::test_model::SCRIPT_ENV;

    let Some((_node, url, db)) = support::inbox::migrated_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    let (org, inbox, agent, event) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    support::inbox::insert_inbox(&db, org, inbox, agent, uuid::Uuid::new_v4()).await;
    support::lease::insert_pending_event(&db, inbox, event, 1).await;

    let dir = std::env::temp_dir().join(format!("agent-host-boot-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create the config dir");
    // The agent is the row that `insert_inbox` made. No tool call: the model answers `done` at once.
    let script = dir.join("script.json");
    std::fs::write(&script, "[]").expect("write the script");

    let (mut child, port) = support::spawn_service_in(
        &url,
        Some(&dir),
        &[
            (SCRIPT_ENV, script.display().to_string()),
            ("AGENT_HOST_WORKER__POLL_INTERVAL_MS", "100".to_string()),
            ("RUST_LOG", "paigasus_agent_host=info,warn".to_string()),
        ],
    );
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;
    wait_for_log(&child, "the worker pool started").await;

    let done = format!("SELECT status = 'done' AS v FROM inbox_event WHERE id = '{event}'");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !support::scalar_bool(&db, &done).await {
        assert!(std::time::Instant::now() < deadline, "the pool did not finish the event; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        support::scalar_bool(&db, &format!("SELECT count(*) = 1 AS v FROM run WHERE inbox_id = '{inbox}' AND status = 'completed'")).await,
        "one completed run must serve the event; log:\n{}",
        child.tail()
    );
}

/// SMA-852: the binary binds `grpc_addr`, serves `AgentService` after the boot, and drains the
/// listener on SIGTERM within the drain timeout.
#[tokio::test]
async fn a_boot_with_grpc_addr_serves_agent_service() {
    use paigasus_proto::paigasus::agents::v1::ListAgentsRequest;
    use paigasus_proto::paigasus::agents::v1::agent_service_client::AgentServiceClient;
    use support::fake_iam::{FakeIam, serve_grpc};

    let Some((_node, url)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot test: Docker unavailable");
        return;
    };
    const TOKEN: &str = "boot-grpc-token";
    let (iam, _server) = serve_grpc(FakeIam::new(TOKEN, uuid::Uuid::from_u128(4))).await;
    let grpc_port = support::free_port();
    let (mut child, port) = support::spawn_service_with(
        &url,
        &[
            ("AGENT_HOST_IAM__GRPC_ADDR", format!("http://{iam}")),
            ("AGENT_HOST_IAM__TLS__MODE", "loopback_insecure".to_owned()),
            ("AGENT_HOST_GRPC_ADDR", format!("127.0.0.1:{grpc_port}")),
            ("AGENT_HOST_WORKER__ENABLED", "false".to_owned()),
        ],
    );
    support::wait_for_status(&mut child, port, "/readyz", 200, READY_BUDGET).await;

    let mut client = AgentServiceClient::connect(format!("http://127.0.0.1:{grpc_port}")).await.expect("connect to grpc_addr");
    let mut request = tonic::Request::new(ListAgentsRequest {
        org_prn: format!("prn:pgs:iam:::organization/{}", uuid::Uuid::from_u128(5)),
        ..Default::default()
    });
    request.metadata_mut().insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let answer = client
        .list_agents(request)
        .await
        .unwrap_or_else(|e| panic!("ListAgents must get a verdict, not {e:?}; log:\n{}", child.tail()));
    // The answer carries the correlation id: the process wires the `CorrelationLayer` on this listener.
    let correlation = answer
        .metadata()
        .get(paigasus_observability::correlation::CORRELATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    assert!(correlation.is_some_and(|v| uuid::Uuid::parse_str(&v).is_ok()), "the gRPC answer has a correlation id header");
    assert!(answer.into_inner().agents.is_empty());

    // A call without a credential is a verdict too: UNAUTHENTICATED.
    let status = client.list_agents(ListAgentsRequest::default()).await.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    // SIGTERM: the listener stops accepting and the process ends inside the drain timeout.
    let pid = child.child.id();
    let killed = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status().expect("run kill(1)");
    assert!(killed.success());
    let started = std::time::Instant::now();
    let exit = loop {
        if let Some(exit) = child.child.try_wait().expect("try_wait") {
            break exit;
        }
        // `DRAIN_TIMEOUT` is 10 s. A listener that ignores the shutdown watch is aborted then, and the
        // process still exits 0. So the bound is below it.
        assert!(started.elapsed() < Duration::from_secs(9), "the drain must end the gRPC listener; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(exit.success(), "exit after SIGTERM: {exit}; log:\n{}", child.tail());
    assert!(!child.tail().contains("drain timed out"), "the drain must not time out; log:\n{}", child.tail());
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", grpc_port)).await.is_err(),
        "the gRPC listener must be closed after the drain"
    );
}
