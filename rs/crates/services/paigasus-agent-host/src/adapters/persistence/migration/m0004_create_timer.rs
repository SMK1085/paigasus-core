// SPDX-License-Identifier: Apache-2.0

//! m0004 — create the `timer` table and its three indexes (spec §7.1).
//!
//! The composite foreign key `(thread_id, inbox_id)` makes the schema enforce that a timer
//! targets a thread of its own inbox. The CHECK constraints keep a `schedule` row and a
//! `follow_up` row in their two valid shapes. The note and the failure text have a length limit
//! in characters. The two time columns must be finite: an `infinity` value would break the due
//! query and the decode of a row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The statements of `up`, in order.
const UP: [&str; 4] = [
    "CREATE TABLE timer (
        id uuid PRIMARY KEY,
        inbox_id uuid NOT NULL REFERENCES inbox (id),
        kind text NOT NULL CHECK (kind IN ('follow_up', 'schedule')),
        origin text NOT NULL CHECK (origin IN ('agent', 'system')),
        status text NOT NULL CHECK (status IN ('active', 'done', 'cancelled', 'failed')),
        thread_id uuid,
        cron text,
        tz text,
        next_slot timestamp,
        next_slot_second boolean,
        note text NOT NULL CHECK (char_length(note) <= 1000),
        next_fire_at timestamptz NOT NULL,
        last_fired_at timestamptz,
        fire_count bigint NOT NULL DEFAULT 0,
        fire_attempts integer NOT NULL DEFAULT 0,
        failure text CHECK (char_length(failure) <= 500),
        created_at timestamptz NOT NULL DEFAULT now(),
        updated_at timestamptz NOT NULL DEFAULT now(),
        FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id),
        CHECK ((cron IS NULL) = (tz IS NULL)),
        CHECK ((cron IS NULL) = (next_slot IS NULL)),
        CHECK ((cron IS NULL) = (next_slot_second IS NULL)),
        CHECK ((kind = 'schedule') = (cron IS NOT NULL)),
        CHECK (isfinite(next_fire_at)),
        CHECK (last_fired_at IS NULL OR isfinite(last_fired_at))
    )",
    "CREATE INDEX timer_due ON timer (next_fire_at) WHERE status = 'active'",
    "CREATE INDEX timer_inbox_active ON timer (inbox_id) WHERE status = 'active'",
    "CREATE INDEX timer_inbox_agent_created ON timer (inbox_id, created_at) WHERE origin = 'agent'",
];

/// The statement of `down`. The indexes go with the table.
const DOWN: [&str; 1] = ["DROP TABLE timer"];

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
