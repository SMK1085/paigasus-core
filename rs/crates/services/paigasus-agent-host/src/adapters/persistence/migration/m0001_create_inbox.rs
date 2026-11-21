// SPDX-License-Identifier: Apache-2.0

//! m0001 — create the `inbox` table (spec §6.8).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE inbox (
                    id uuid PRIMARY KEY,
                    org_id uuid NOT NULL,
                    agent_id uuid NOT NULL,
                    principal_id uuid NOT NULL,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (agent_id, principal_id)
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared("DROP TABLE inbox").await?;
        Ok(())
    }
}
