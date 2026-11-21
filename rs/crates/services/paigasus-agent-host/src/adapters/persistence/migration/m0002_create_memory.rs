// SPDX-License-Identifier: Apache-2.0

//! m0002 — create the memory store tables (SMA-789 spec §6.8).
//!
//! The `CHECK` on `path` is a second guard for the path rules of the core (`MemoryPath`). The
//! first term allows only the store charset, a first character that is not `.`, at most 8
//! segments and 64 bytes for each segment. That term already refuses a `.` or `..` segment,
//! because a segment cannot start with `.`. The `!~` term refuses those segments explicitly, as
//! spec §6.8 asks, so a later edit of the first term cannot open them. `COLLATE "C"` makes
//! `ORDER BY path` byte order.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("CREATE SEQUENCE memory_file_version_seq").await?;
        db.execute_unprepared(
            "CREATE TABLE memory_scope (
                org_id       uuid   NOT NULL,
                agent_id     uuid   NOT NULL,
                principal_id uuid   NOT NULL,
                files        bigint NOT NULL DEFAULT 0,
                bytes        bigint NOT NULL DEFAULT 0,
                PRIMARY KEY (org_id, agent_id, principal_id)
            )",
        )
        .await?;
        db.execute_unprepared(
            r#"CREATE TABLE memory_file (
                org_id       uuid        NOT NULL,
                agent_id     uuid        NOT NULL,
                principal_id uuid        NOT NULL,
                path         text        COLLATE "C" NOT NULL
                             CHECK (path ~ '^(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,63}){1,8}$'
                                    AND path !~ '/\.\.?(/|$)'
                                    AND octet_length(path) <= 256),
                content      text        NOT NULL,
                size_bytes   integer     GENERATED ALWAYS AS (octet_length(content)) STORED,
                version      bigint      NOT NULL DEFAULT nextval('memory_file_version_seq'),
                updated_at   timestamptz NOT NULL DEFAULT now(),
                PRIMARY KEY (org_id, agent_id, principal_id, path),
                FOREIGN KEY (org_id, agent_id, principal_id) REFERENCES memory_scope
            )"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE memory_file").await?;
        db.execute_unprepared("DROP TABLE memory_scope").await?;
        db.execute_unprepared("DROP SEQUENCE memory_file_version_seq").await?;
        Ok(())
    }
}
