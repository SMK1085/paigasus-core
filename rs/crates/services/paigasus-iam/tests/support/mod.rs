// SPDX-License-Identifier: Apache-2.0

//! Shared integration-test support: an ephemeral, migrated Postgres via Docker.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns `None`) with
//! a note. Used by every integration test file that needs a real database.

use paigasus_iam::adapters::persistence::Migrator;
use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

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
