// SPDX-License-Identifier: Apache-2.0

//! m0006 — create the `agent` table and the inbox foreign key (spec §6.1).
//!
//! `agent` holds the definitions of the agents of an org. The partial unique index makes a name
//! unique among the active agents of one org, so an archived agent frees its name. The foreign
//! key makes every inbox belong to an agent of the same org. The migration fails, and the Postgres
//! error names `inbox_agent_fk`, when an `inbox` row has no `agent` row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The statements of `up`, in order.
const UP: [&str; 3] = [
    "CREATE TABLE agent (
        id uuid NOT NULL,
        org_id uuid NOT NULL,
        name text NOT NULL,
        model text NOT NULL,
        instructions text NOT NULL,
        tools text[] NOT NULL DEFAULT '{}',
        version bigint NOT NULL DEFAULT 1,
        created_at timestamptz NOT NULL DEFAULT now(),
        updated_at timestamptz NOT NULL DEFAULT now(),
        archived_at timestamptz,
        PRIMARY KEY (id),
        UNIQUE (org_id, id)
    )",
    "CREATE UNIQUE INDEX agent_org_name_active ON agent (org_id, name) WHERE archived_at IS NULL",
    "ALTER TABLE inbox
        ADD CONSTRAINT inbox_agent_fk FOREIGN KEY (org_id, agent_id) REFERENCES agent (org_id, id)",
];

/// The statements of `down`, in order. The constraint goes before the table.
const DOWN: [&str; 2] = ["ALTER TABLE inbox DROP CONSTRAINT inbox_agent_fk", "DROP TABLE agent"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in UP {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in DOWN {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
