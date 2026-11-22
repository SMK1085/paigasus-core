// SPDX-License-Identifier: Apache-2.0

//! Helpers for the Postgres-backed test binaries of this crate. Each binary compiles this file
//! on its own, so every item carries `#[allow(dead_code)]`: with `warnings = deny`, an item that
//! one binary does not use would fail that binary.
//!
//! The decision to skip or to fail when Docker is missing lives once, in
//! `paigasus_test_docker::start_or_skip`.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use testcontainers::ContainerAsync;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

/// How long a fresh Postgres container has to accept connections (as in `paigasus-test-docker`).
const PG_READY_BUDGET: Duration = Duration::from_secs(90);

/// How many times the container start is tried in all. A start that hits the ready budget under
/// load is tried again with a new container.
const PG_START_ATTEMPTS: usize = 3;

/// Run `attempt` until it gives `Ok`, at most `attempts` times. `Err` is a start error that may
/// pass on the next try. The last error is the answer when every try fails. `Ok(None)` (Docker
/// skipped) ends at once. The closure only starts a container, so no seed runs twice.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn retry_start<T, F, Fut>(attempts: usize, mut attempt: F) -> Result<Option<T>, String>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>, String>>,
{
    let mut last = String::new();
    for n in 1..=attempts {
        match attempt(n).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                eprintln!("container start, try {n} of {attempts}, failed: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// One start of an empty Postgres container. `Err` means the server did not accept a connection
/// in the ready budget. The container is dropped then.
async fn try_start_raw_postgres() -> Result<Option<(ContainerAsync<Postgres>, String)>, String> {
    let Some(node) = paigasus_test_docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "start_raw_postgres").await else {
        return Ok(None);
    };
    let port = paigasus_test_docker::mapped_port(&node, 5432, "postgres").await;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let deadline = std::time::Instant::now() + PG_READY_BUDGET;
    loop {
        match Database::connect(url.clone()).await {
            Ok(_) => return Ok(Some((node, url))),
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!("postgres did not accept connections within {PG_READY_BUDGET:?}: {e}"));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Starts an empty Postgres container. It runs no migration. It returns once the server accepts
/// a connection, because the image logs "ready" twice and the first log line comes before a
/// restart of the server. A start that does not get ready is tried again with a new container,
/// at most `PG_START_ATTEMPTS` times in all. The caller seeds only after this returns.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn start_raw_postgres() -> Option<(ContainerAsync<Postgres>, String)> {
    match retry_start(PG_START_ATTEMPTS, |_| try_start_raw_postgres()).await {
        Ok(v) => v,
        Err(e) => panic!("{e}"),
    }
}

/// A multi-connection pool on the database.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn connect(url: &str) -> DatabaseConnection {
    Database::connect(url.to_string()).await.expect("connect")
}

/// A pool pinned to ONE physical connection. A session-level advisory lock taken on it stays held
/// until the same connection unlocks. A default pool could send the unlock to another session.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn connect_pinned(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.max_connections(1).min_connections(1);
    Database::connect(opts).await.expect("connect pinned")
}

/// The column `v` of a query that returns one boolean.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn scalar_bool(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<bool>("", "v")
        .expect("bool column named v")
}

/// The column `n` of a query that returns one bigint.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("bigint column named n")
}

/// A free local port. Another process could take it before the service binds it. The window is
/// short, and the IAM tests accept the same risk.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

/// The status and the body of a GET, or `None` when nothing answers.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn http_status(port: u16, path: &str) -> Option<(u16, String)> {
    // reqwest 0.13 panics when it builds a `Client` with no installed CryptoProvider (SMA-809).
    // A second install returns `Err`, which is correct, so the result is ignored.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let resp = reqwest::Client::new().get(format!("http://127.0.0.1:{port}{path}")).timeout(Duration::from_secs(2)).send().await.ok()?;
    let status = resp.status().as_u16();
    Some((status, resp.text().await.ok()?))
}

/// A running service process. It is killed when it drops, so a failed assertion cannot leave a
/// service on a port.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct Child {
    pub child: std::process::Child,
    /// Stdout and stderr of the child, capped at 16 KiB. The reader threads must run: a full
    /// pipe blocks the child on its next write.
    log: Arc<Mutex<String>>,
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Child {
    /// The captured output of the child. Use it in panic messages.
    #[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
    pub fn tail(&self) -> String {
        self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// Starts the built service on a free port. The lock wait is 60 seconds.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn spawn_service(url: &str) -> (Child, u16) {
    spawn_service_with(url, &[])
}

/// Like [`spawn_service`], with extra environment variables for the child.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn spawn_service_with(url: &str, extra_env: &[(&str, String)]) -> (Child, u16) {
    let port = free_port();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-agent-host"));
    cmd.env("AGENT_HOST_DATABASE_URL", url)
        .env("AGENT_HOST_HTTP_ADDR", format!("127.0.0.1:{port}"))
        .env("AGENT_HOST_MIGRATION__LOCK_WAIT_SECS", "60");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("spawn paigasus-agent-host");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let log = Arc::new(Mutex::new(String::new()));
    for (name, reader) in [("stdout", Box::new(stdout) as Box<dyn std::io::Read + Send>), ("stderr", Box::new(stderr))] {
        let writer = log.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(reader).lines().map_while(Result::ok) {
                let mut buf = writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if buf.len() < 16 * 1024 {
                    buf.push_str(&format!("[{name}] {line}\n"));
                }
            }
        });
    }
    (Child { child, log }, port)
}

/// Polls `path` until it answers `want`. It panics with the log of the child when the child
/// exits first, or when the budget ends.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn wait_for_status(child: &mut Child, port: u16, path: &str, want: u16, budget: Duration) -> String {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if let Some((status, body)) = http_status(port, path).await
            && status == want
        {
            return body;
        }
        if let Some(exit) = child.child.try_wait().expect("try_wait") {
            panic!("the service exited ({exit}) before {path} answered {want}; log:\n{}", child.tail());
        }
        assert!(std::time::Instant::now() < deadline, "{path} did not answer {want} within {budget:?}; log:\n{}", child.tail());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub mod commit;
pub mod fake_iam;
pub mod fake_inbox;
pub mod inbox;
pub mod lease;
pub mod stub_iam;
pub mod timer;
