// SPDX-License-Identifier: Apache-2.0

//! m0013 — the tool catalog and the tool policy set versions (SMA-774, spec sections 4.2, 4.4,
//! 4.5 and 6).
//!
//! Three tables. `tool_catalog_entry` holds one row per tool of an organization.
//! `tool_policy_state` holds one row per policy set (`floor` or `tenant:<uuid>`). It carries the
//! catalog revision and the live version, and it is the row that writers lock with
//! `SELECT … FOR UPDATE`. `tool_policy_set_version` holds every stored version of a set.
//!
//! The CHECK lists for `status` and `origin` and the `held` index hold every value that later
//! releases use. The Rust enums hold only the values that this release writes.
//!
//! A `custom` tool has no schema. A `function` tool may have none: a built-in tool with an
//! empty record. So the CHECK is `kind <> 'custom' OR input_schema IS NULL`.
//!
//! `SET LOCAL lock_timeout` mirrors m0008-m0012. These statements create new tables only, so
//! the timeout bounds the foreign-key lock on `organization`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"CREATE TABLE "tool_catalog_entry" (
                 org_id UUID NOT NULL REFERENCES "organization"(id) ON DELETE CASCADE,
                 name_key TEXT NOT NULL,
                 display_name TEXT NOT NULL,
                 kind TEXT NOT NULL CHECK (kind IN ('function','custom')),
                 input_schema JSONB NULL,
                 cedar_input_type JSONB NOT NULL,
                 dropped_fields JSONB NOT NULL DEFAULT '[]',
                 source TEXT NOT NULL CHECK (source IN ('manual','openai','mcp')),
                 created_at TIMESTAMPTZ NOT NULL,
                 updated_at TIMESTAMPTZ NOT NULL,
                 PRIMARY KEY (org_id, name_key),
                 CHECK (kind <> 'custom' OR input_schema IS NULL)
               );"#,
        )
        .await?;
        conn.execute_unprepared(
            r#"CREATE TABLE "tool_policy_state" (
                 set_key TEXT PRIMARY KEY,
                 kind TEXT NOT NULL CHECK (kind IN ('tenant','floor')),
                 org_id UUID NULL REFERENCES "organization"(id) ON DELETE CASCADE,
                 catalog_revision BIGINT NOT NULL DEFAULT 0,
                 live_version BIGINT NOT NULL DEFAULT 0,
                 CHECK ((kind = 'floor') = (org_id IS NULL)),
                 CHECK (set_key = CASE WHEN kind = 'floor' THEN 'floor' ELSE 'tenant:' || org_id::text END)
               );"#,
        )
        .await?;
        conn.execute_unprepared(r#"INSERT INTO "tool_policy_state" (set_key, kind) VALUES ('floor','floor');"#).await?;
        conn.execute_unprepared(
            r#"CREATE TABLE "tool_policy_set_version" (
                 kind TEXT NOT NULL CHECK (kind IN ('tenant','floor')),
                 org_id UUID NULL REFERENCES "organization"(id) ON DELETE CASCADE,
                 version BIGINT NOT NULL CHECK (version >= 1),
                 source TEXT NOT NULL,
                 compiled BYTEA NOT NULL,
                 compiler_generation INTEGER NOT NULL,
                 schema_revision INTEGER NOT NULL,
                 catalog_revision BIGINT NULL,
                 status TEXT NOT NULL CHECK (status IN ('live','superseded','held','discarded')),
                 origin TEXT NOT NULL CHECK (origin IN ('save','starter','catalog_change','recompile','accept')),
                 base_version BIGINT NULL,
                 report JSONB NULL,
                 created_by TEXT NOT NULL,
                 created_at TIMESTAMPTZ NOT NULL,
                 CHECK ((kind = 'floor') = (org_id IS NULL)),
                 CHECK ((kind = 'floor') = (catalog_revision IS NULL))
               );"#,
        )
        .await?;
        conn.execute_unprepared(
            r#"CREATE UNIQUE INDEX uq_tool_policy_set_version ON "tool_policy_set_version"
                 (kind, COALESCE(org_id, '00000000-0000-0000-0000-000000000000'::uuid), version);"#,
        )
        .await?;
        conn.execute_unprepared(
            r#"CREATE UNIQUE INDEX uq_tool_policy_set_version_live ON "tool_policy_set_version"
                 (kind, COALESCE(org_id, '00000000-0000-0000-0000-000000000000'::uuid)) WHERE status = 'live';"#,
        )
        .await?;
        conn.execute_unprepared(
            r#"CREATE UNIQUE INDEX uq_tool_policy_set_version_held ON "tool_policy_set_version"
                 (kind, COALESCE(org_id, '00000000-0000-0000-0000-000000000000'::uuid)) WHERE status = 'held';"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"DROP TABLE IF EXISTS "tool_policy_set_version";"#).await?;
        conn.execute_unprepared(r#"DROP TABLE IF EXISTS "tool_policy_state";"#).await?;
        conn.execute_unprepared(r#"DROP TABLE IF EXISTS "tool_catalog_entry";"#).await?;
        Ok(())
    }
}
