// SPDX-License-Identifier: Apache-2.0

//! m0002 — create the `thread`, `inbox_lease`, `run`, `run_event` and `inbox_event` tables
//! (spec §7.1).
//!
//! The composite foreign keys `(thread_id, inbox_id)` and `(claimed_run_id, inbox_id)` make the
//! schema enforce that a thread, a run and an event belong to one inbox. The unique key
//! `(inbox_id, source, ce_id)` and the partial index `inbox_event_claimable` are fixed by the spec.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The statements of `up`, in order. `run_event.event_id` gets its foreign key by `ALTER`,
/// because `inbox_event` does not exist yet when `run_event` is made.
const UP: [&str; 7] = [
    "CREATE TABLE thread (
        id uuid PRIMARY KEY,
        inbox_id uuid NOT NULL REFERENCES inbox (id),
        created_at timestamptz NOT NULL DEFAULT now(),
        last_run_at timestamptz,
        closed boolean NOT NULL DEFAULT false,
        successor_id uuid,
        predecessor_id uuid,
        open_deferral boolean NOT NULL DEFAULT false,
        UNIQUE (id, inbox_id),
        FOREIGN KEY (successor_id, inbox_id) REFERENCES thread (id, inbox_id),
        FOREIGN KEY (predecessor_id, inbox_id) REFERENCES thread (id, inbox_id)
    )",
    "CREATE TABLE inbox_lease (
        inbox_id uuid PRIMARY KEY REFERENCES inbox (id),
        holder text NOT NULL,
        token bigint NOT NULL,
        expires_at timestamptz NOT NULL
    )",
    "CREATE TABLE run (
        id uuid PRIMARY KEY,
        inbox_id uuid NOT NULL REFERENCES inbox (id),
        thread_id uuid NOT NULL,
        lease_token bigint NOT NULL,
        status text NOT NULL,
        started_at timestamptz NOT NULL DEFAULT now(),
        finished_at timestamptz,
        UNIQUE (id, inbox_id),
        FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id)
    )",
    "CREATE TABLE run_event (
        run_id uuid NOT NULL REFERENCES run (id),
        event_id uuid NOT NULL,
        PRIMARY KEY (run_id, event_id)
    )",
    "CREATE TABLE inbox_event (
        id uuid PRIMARY KEY,
        inbox_id uuid NOT NULL REFERENCES inbox (id),
        source text NOT NULL,
        ce_id text NOT NULL,
        ce_type text NOT NULL,
        thread_id uuid,
        envelope jsonb NOT NULL,
        received_at timestamptz NOT NULL DEFAULT now(),
        status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'dead')),
        attempts integer NOT NULL DEFAULT 0,
        not_before timestamptz NOT NULL DEFAULT now(),
        claimed_run_id uuid,
        last_error text,
        UNIQUE (inbox_id, source, ce_id),
        FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id),
        FOREIGN KEY (claimed_run_id, inbox_id) REFERENCES run (id, inbox_id)
    )",
    "ALTER TABLE run_event ADD FOREIGN KEY (event_id) REFERENCES inbox_event (id)",
    "CREATE INDEX inbox_event_claimable ON inbox_event (inbox_id, received_at) WHERE status = 'pending'",
];

/// The statements of `down`, in reverse dependency order. `run_event` goes before `inbox_event`,
/// because `run_event.event_id` references `inbox_event`.
const DOWN: [&str; 5] = ["DROP TABLE run_event", "DROP TABLE inbox_event", "DROP TABLE run", "DROP TABLE inbox_lease", "DROP TABLE thread"];

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
