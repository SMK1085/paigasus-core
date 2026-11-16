// SPDX-License-Identifier: Apache-2.0

//! m0014 — the tool policy settings of each organization (SMA-775).
//!
//! One table, `tool_policy_settings`, with one row for each organization. An organization with
//! no row has the default settings: mode `off`, fail mode `closed`. The CHECK lists hold every
//! value of `ToolPolicyMode` and `FailMode`.
//!
//! `SET LOCAL lock_timeout` mirrors m0013. The statement creates a new table only, so the
//! timeout bounds the foreign-key lock on `organization`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"CREATE TABLE "tool_policy_settings" (
                 org_id UUID PRIMARY KEY REFERENCES "organization"(id) ON DELETE CASCADE,
                 mode TEXT NOT NULL CHECK (mode IN ('off','log_only','enforce')),
                 fail_mode TEXT NOT NULL CHECK (fail_mode IN ('open','closed')),
                 updated_at TIMESTAMPTZ NOT NULL,
                 updated_by TEXT NOT NULL
               );"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"DROP TABLE IF EXISTS "tool_policy_settings";"#).await?;
        Ok(())
    }
}
