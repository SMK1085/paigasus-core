// SPDX-License-Identifier: Apache-2.0

//! Helpers for the tests of the inbox tables. A test starts a migrated Postgres and inserts rows
//! with plain SQL.

use paigasus_agent_host::adapters::persistence::Migrator;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

/// Starts Postgres, connects and runs every migration. It returns `None` when Docker is
/// unavailable and the skip policy allows it.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn migrated_postgres() -> Option<(ContainerAsync<Postgres>, String, DatabaseConnection)> {
    let (node, url) = super::start_raw_postgres().await?;
    let db = super::connect(&url).await;
    Migrator::up(&db, None).await.expect("Migrator::up");
    Some((node, url, db))
}

/// Inserts one `inbox` row.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_inbox(db: &DatabaseConnection, org: Uuid, id: Uuid, agent: Uuid, principal: Uuid) {
    db.execute_unprepared(&format!("INSERT INTO inbox (id, org_id, agent_id, principal_id) VALUES ('{id}', '{org}', '{agent}', '{principal}')"))
        .await
        .expect("insert inbox");
}
