// SPDX-License-Identifier: Apache-2.0

//! m0005 — create the `session_event` table and add the result columns to `run` (spec §6.6 and
//! §6.10).
//!
//! `session_event` holds the committed events of a thread, numbered by `sequence`. The primary
//! key `(thread_id, sequence)` makes a second writer fail instead of writing a duplicate number.
//! The two composite foreign keys make the schema enforce that a row belongs to a thread and a
//! run of its own inbox. The new `run` columns hold the final output, the error and the token
//! usage of a finished run. There is no CHECK on `run.status`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The statements of `up`, in order.
const UP: [&str; 2] = [
    "CREATE TABLE session_event (
        inbox_id uuid NOT NULL,
        thread_id uuid NOT NULL,
        sequence bigint NOT NULL,
        run_id uuid NOT NULL,
        kind text NOT NULL,
        payload jsonb NOT NULL,
        PRIMARY KEY (thread_id, sequence),
        FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id),
        FOREIGN KEY (run_id, inbox_id) REFERENCES run (id, inbox_id)
    )",
    "ALTER TABLE run
        ADD COLUMN final_output text,
        ADD COLUMN error text,
        ADD COLUMN error_class text,
        ADD COLUMN input_tokens bigint,
        ADD COLUMN output_tokens bigint,
        ADD COLUMN cached_input_tokens bigint,
        ADD COLUMN reasoning_tokens bigint",
];

/// The statements of `down`, in order. The table goes after the columns.
const DOWN: [&str; 2] = [
    "ALTER TABLE run
        DROP COLUMN final_output,
        DROP COLUMN error,
        DROP COLUMN error_class,
        DROP COLUMN input_tokens,
        DROP COLUMN output_tokens,
        DROP COLUMN cached_input_tokens,
        DROP COLUMN reasoning_tokens",
    "DROP TABLE session_event",
];

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
