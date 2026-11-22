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
                  (gen_random_uuid(), gen_random_uuid(), '11111111-1111-1111-1111-111111111111', \
                  '22222222-2222-2222-2222-222222222222')";
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
    assert_eq!(agent_host_samples(&before), agent_host_samples(&after_probes), "probes must not add an agent_host_* sample");

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
