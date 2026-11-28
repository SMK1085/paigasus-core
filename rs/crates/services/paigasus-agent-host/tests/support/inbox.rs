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

/// Inserts one `agent` row. The name is the agent uuid. A second insert of the same id does
/// nothing.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_agent(db: &DatabaseConnection, org: Uuid, agent: Uuid, model: &str, instructions: &str, tools: &[&str]) {
    let tools = tools.iter().map(|t| format!("'{t}'")).collect::<Vec<_>>().join(", ");
    db.execute_unprepared(&format!(
        "INSERT INTO agent (id, org_id, name, model, instructions, tools) \
         VALUES ('{agent}', '{org}', '{agent}', '{model}', '{instructions}', ARRAY[{tools}]::text[]) ON CONFLICT (id) DO NOTHING"
    ))
    .await
    .expect("insert agent");
}

/// Inserts one `inbox` row. It inserts the agent row first (model `scripted`, instructions
/// `test`, no tools), and panics if the stored agent belongs to another org.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn insert_inbox(db: &DatabaseConnection, org: Uuid, id: Uuid, agent: Uuid, principal: Uuid) {
    insert_agent(db, org, agent, "scripted", "test", &[]).await;
    let same_org = super::scalar_bool(db, &format!("SELECT org_id = '{org}' AS v FROM agent WHERE id = '{agent}'")).await;
    assert!(same_org, "the agent {agent} exists in another org than {org}");
    db.execute_unprepared(&format!("INSERT INTO inbox (id, org_id, agent_id, principal_id) VALUES ('{id}', '{org}', '{agent}', '{principal}')"))
        .await
        .expect("insert inbox");
}
