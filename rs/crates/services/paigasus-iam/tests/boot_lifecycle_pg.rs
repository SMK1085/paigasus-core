// SPDX-License-Identifier: Apache-2.0

//! SMA-571 AC 1 + AC 2 end to end, through the real composition root.
//!
//! `serve()` lives in `src/main.rs` and integration tests link the LIB, so this spawns the built
//! binary as a subprocess — the only suite in the crate that does. The alternative, re-creating
//! the boot ordering in-process, would be vacuous: the ordering IS what is under test.
//!
//! The deferred phase is pinned open by holding SMA-559's migration advisory lock from a second
//! session, so this does not race a fast migration.
//!
//! This is also where SMA-571 Task 3's review gap gets closed: `boot_deferred.rs`'s Docker-free
//! `grpc_health_answers_not_serving_while_the_slot_is_empty` only asserts `grpc-status != "14"`,
//! which proves health isn't swallowed by the migrating fallback but never decodes
//! `HealthCheckResponse.status` to confirm it is actually `NOT_SERVING`. This suite decodes it on
//! both sides of the migrate -> ready transition, via a real `tonic_health` client against the
//! spawned process, and additionally proves the UNAVAILABLE -> UNAUTHENTICATED flip on a real
//! `TenancyService` RPC that is the delegation evidence Task 4's implementer only checked by hand.

mod support;

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paigasus_iam::adapters::persistence::migration_lock::MIGRATION_LOCK_KEY;
use paigasus_proto::paigasus::iam::v1::CreateOrganizationRequest;
use paigasus_proto::paigasus::iam::v1::tenancy_service_client::TenancyServiceClient;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use tonic::Code;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

/// Kills the child on drop, so a failing assertion cannot leave a service holding a port.
struct Child {
    child: std::process::Child,
    /// The child's stderr, drained on a background thread and capped at 16KiB — surfaced in
    /// panic messages below. `Stdio::piped()` with nobody reading it is a real deadlock risk
    /// (a full OS pipe buffer blocks the child's next write, which during the migration wait
    /// would silently hang the very boot sequence this test is timing), not merely a missed
    /// diagnostic — draining it is load-bearing, the capture is the bonus.
    log: Arc<Mutex<String>>,
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Child {
    fn tail(&self) -> String {
        self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

async fn scalar_bool(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<bool>("", "v")
        .expect("bool column")
}

/// A pool pinned to ONE physical connection, so the session-level advisory lock taken on it
/// stays held across the later `pg_advisory_unlock` call. Mirrors `migration_lock_pg.rs`'s
/// `connect_pinned` (`tests/migration_lock_pg.rs:109-114`) — a plain `Database::connect` (a
/// multi-connection pool by default) hands out connections round-robin, so the lock and the
/// unlock could land on two DIFFERENT physical sessions. Postgres session-level advisory locks
/// are per-session, so that unlock would return `false` (an assertable failure, not a silent
/// one) while the lock stayed held on the abandoned session — wedging the migrator for the rest
/// of the test run instead of merely failing loudly.
async fn connect_pinned(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.max_connections(1).min_connections(1);
    Database::connect(opts).await.expect("connect pinned")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

async fn http_status(port: u16, path: &str) -> Option<(u16, String)> {
    let resp = reqwest::Client::new().get(format!("http://127.0.0.1:{port}{path}")).timeout(Duration::from_secs(2)).send().await.ok()?;
    let status = resp.status().as_u16();
    Some((status, resp.text().await.ok()?))
}

/// Dials the gRPC port, retrying briefly. The HTTP and gRPC listeners are bound back-to-back in
/// `main.rs` but not atomically, so a caller that has only confirmed `/healthz` might still beat
/// the gRPC `bind` by a few microseconds.
async fn connect_grpc(port: u16) -> tonic::transport::Channel {
    let mut last_err = None;
    for _ in 0..50 {
        match tonic::transport::Endpoint::new(format!("http://127.0.0.1:{port}")).expect("endpoint").connect().await {
            Ok(channel) => return channel,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    panic!("gRPC port {port} never accepted a connection: {last_err:?}");
}

fn spawn_iam(db_url: &str, http_port: u16, grpc_port: u16) -> Child {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-iam"));
    cmd.env("IAM_DATABASE_URL", db_url)
        .env("IAM_HTTP_ADDR", format!("127.0.0.1:{http_port}"))
        .env("IAM_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
        .env("IAM_AUTHN__ISSUERS", r#"[{issuer="https://idp.example.com",audiences=["paigasus"]}]"#)
        .env("IAM_API_KEYS__PEPPER", "cGFpZ2FzdXMtc21va2UtcGVwcGVyLW5vdC1hLXJlYWwtc2VjcmV0LTAwMA==")
        .env("IAM_MIGRATION__LOCK_WAIT_SECS", "60")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn paigasus-iam");
    let stderr = child.stderr.take().expect("piped stderr");
    let log = Arc::new(Mutex::new(String::new()));
    let writer = log.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut buf = writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if buf.len() < 16 * 1024 {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
    });
    Child { child, log }
}

/// AC 1 and AC 2: while the migration lock is held elsewhere, the replica is BOUND and visibly
/// unready — not absent. Before SMA-571 both HTTP requests below would be connection-refused.
///
/// Also proves the D7 gRPC contract end to end: `HealthCheckResponse.status` is decoded (not
/// merely "not UNAVAILABLE") on both sides of the transition, and a real, unauthenticated
/// `TenancyService` RPC over the same connection flips from UNAVAILABLE (14, the boot slot's
/// catch-all) to UNAUTHENTICATED (16, `AuthLayer` rejecting the missing bearer) once the slot
/// installs — that flip is what proves the real router took over, not merely that SOME response
/// came back.
#[tokio::test]
async fn a_lock_blocked_replica_is_bound_and_reports_migrating() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot lifecycle test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;

    // `pg_try_advisory_lock`, not the blocking form: the latter returns void and cannot assert
    // its own setup, so a holder that silently failed would make this whole test vacuous.
    let holder = connect_pinned(&url).await;
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let (http_port, grpc_port) = (free_port(), free_port());
    let child = spawn_iam(&url, http_port, grpc_port);

    // Poll for the bind — the process still has to load config and connect to Postgres.
    let mut healthz = None;
    for _ in 0..100 {
        if let Some(r) = http_status(http_port, "/healthz").await {
            healthz = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (status, _) = healthz.unwrap_or_else(|| panic!("the listener must bind while the migration lock is held; stderr:\n{}", child.tail()));
    assert_eq!(status, 200, "AC 1: /healthz answers 200 while migrating");

    let (status, body) = http_status(http_port, "/readyz").await.expect("readyz");
    assert_eq!(status, 503, "AC 1: /readyz is 503 while migrating");
    assert!(body.contains("migrating"), "AC 1: the body distinguishes migrating from a failed ping, got {body}");

    // One channel, reused across the whole migrate -> ready transition below — the same
    // connection experiencing both halves, mirroring the manual run in the task-4 report.
    let channel = connect_grpc(grpc_port).await;
    let mut health_client = HealthClient::new(channel.clone());
    let mut tenancy_client = TenancyServiceClient::new(channel);

    let resp = health_client
        .check(HealthCheckRequest { service: String::new() })
        .await
        .expect("gRPC health check while migrating")
        .into_inner();
    assert_eq!(
        resp.status,
        ServingStatus::NotServing as i32,
        "AC 2: gRPC health must report NOT_SERVING while migrating — not merely avoid UNAVAILABLE"
    );

    let err = tenancy_client
        .create_organization(CreateOrganizationRequest {
            slug: "sma-571-migrating".to_string(),
            name: "SMA-571".to_string(),
        })
        .await
        .expect_err("the deferred slot must reject every app RPC while migrating");
    assert_eq!(err.code(), Code::Unavailable, "AC 2: a live app RPC must be UNAVAILABLE, not absent or UNIMPLEMENTED, while migrating");

    assert!(
        scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually release the lock"
    );

    // Once the lock is free the replica migrates and flips to ready. Budget is 90s (900 x
    // 100ms), not the bind-wait loop's tight 10s: production's own documented worst case is
    // `MIGRATION_BUDGET_SECS = 60` (`migration_lock.rs`), so a slow-but-CORRECT run under loaded
    // CI must not exhaust this loop and report a false failure — unlike the bind-wait and SIGTERM
    // budgets, which stay tight deliberately because that tightness is what makes their
    // respective regressions detectable.
    let mut ready = false;
    for _ in 0..900 {
        if let Some((200, _)) = http_status(http_port, "/readyz").await {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "the replica must become ready once the migration lock is released; stderr:\n{}", child.tail());

    let resp = health_client
        .check(HealthCheckRequest { service: String::new() })
        .await
        .expect("gRPC health check once installed")
        .into_inner();
    assert_eq!(resp.status, ServingStatus::Serving as i32, "gRPC health must flip to SERVING once the boot slot is installed");

    let err = tenancy_client
        .create_organization(CreateOrganizationRequest {
            slug: "sma-571-ready".to_string(),
            name: "SMA-571".to_string(),
        })
        .await
        .expect_err("an unauthenticated RPC must still be rejected once installed — by AuthLayer, not the boot slot");
    assert_eq!(
        err.code(),
        Code::Unauthenticated,
        "the UNAVAILABLE -> UNAUTHENTICATED transition IS the proof that the real, AuthLayer-wrapped router took over"
    );
}

/// §4.6: SIGTERM during the deferred phase must drain promptly, not be ignored until the lock
/// wait expires and SIGKILL arrives — which is the stranded-backend scenario in the RUNBOOK.
///
/// Sent via the `kill(1)` binary rather than `libc::kill`: POSIX and present on both macOS and
/// Linux CI, and avoids adding `libc` as a new workspace dependency for one assertion.
#[cfg(unix)]
#[tokio::test]
async fn sigterm_during_the_deferred_phase_exits_promptly() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot lifecycle test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let (http_port, grpc_port) = (free_port(), free_port());
    let mut child = spawn_iam(&url, http_port, grpc_port);
    for _ in 0..100 {
        if http_status(http_port, "/healthz").await.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let pid = child.child.id();
    let status = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status().expect("run kill(1)");
    assert!(status.success(), "kill -TERM {pid} failed: {status:?}");

    let started = std::time::Instant::now();
    loop {
        if child.child.try_wait().expect("try_wait").is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "SIGTERM must be honoured during the deferred phase, not after lock_wait_secs; stderr:\n{}",
            child.tail()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
