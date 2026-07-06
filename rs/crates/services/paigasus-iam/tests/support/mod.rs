// SPDX-License-Identifier: Apache-2.0

//! Shared integration-test support: an ephemeral, migrated Postgres via Docker, plus the
//! axum-`oneshot` HTTP test harness (`app`/`send`).
//!
//! `start_migrated_postgres` runs against an ephemeral Postgres in Docker. In CI (`CI` env
//! set) a missing Docker daemon is a HARD FAILURE; on a Docker-less laptop the test skips
//! (returns `None`) with a note. Used by every integration test file that needs a real
//! database.
//!
//! `app`/`send` are shared by the HTTP integration suites (`http_tenancy.rs`,
//! `http_memberships.rs`); this file is compiled per test binary via `mod support;`, so
//! binaries that only need `start_migrated_postgres` (e.g. `roundtrip.rs`, `tenancy_*.rs`,
//! `grpc_tenancy.rs`) would otherwise warn on the unused HTTP helpers under
//! `clippy --all-targets -D warnings` — hence `#[allow(dead_code)]` on them.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::Migrator;
use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tower::ServiceExt;

/// Starts an ephemeral Postgres container, connects, and runs migrations.
///
/// Returns `None` when Docker is unavailable and `CI` is unset (local skip path). Panics
/// when `CI` is set and Docker is unreachable — Docker must be present in CI.
pub async fn start_migrated_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the round-trip test in CI: {e}");
            }
            eprintln!("skipping round-trip: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let db = Database::connect(&url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    Some((node, db))
}

/// Builds the real `router(AppState::new(db))` for `tower::ServiceExt::oneshot` HTTP tests.
#[allow(dead_code)]
pub fn app(db: DatabaseConnection) -> Router {
    router(AppState::new(db))
}

/// Drives one request through the router and returns `(status, json body)`. `Value::Null`
/// stands in for an empty body (e.g. the archive/restore/health endpoints and `DELETE` 204
/// responses under test don't all have one).
#[allow(dead_code)]
pub async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&b).unwrap())
        }
        None => Body::empty(),
    };
    let request = builder.body(body).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}
