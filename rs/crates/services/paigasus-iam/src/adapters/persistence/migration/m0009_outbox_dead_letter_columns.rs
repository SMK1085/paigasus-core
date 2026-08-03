// SPDX-License-Identifier: Apache-2.0

//! m0009 — `event_outbox` gains the dead-letter/retention columns (SMA-469).
//!
//! `parked_at` and `last_error` are both load-bearing, not conveniences:
//! - `parked_at` is what `[outbox.retention].parked_days` measures from. Measuring from
//!   `occurred_at` instead would delete a week-old event on the very tick after it parked.
//!   It is also the axis the dead-letter surface's time filters use.
//! - `last_error` is the parking reason. Before this it existed only in a `tracing::error!`
//!   line, so an operator inspecting the DLQ could not see WHY a row was dead.
//!
//! **Every statement is idempotent, deliberately.** `m0008_partition_audit_log`'s module doc
//! records that SeaORM's migrator does not serialize concurrent `up()` across replicas (m0007
//! uses `.if_not_exists()` for the same reason): a bare `ADD COLUMN` would fail the losing
//! replica of a simultaneous first boot with `column "parked_at" ... already exists`. The
//! `SET LOCAL lock_timeout` mirrors m0008 so the `ACCESS EXCLUSIVE` request backs off rather
//! than queueing ahead of in-flight `PgOutbox::enqueue` writes during a rolling deploy.
//!
//! `CREATE INDEX CONCURRENTLY` is NOT available here — SeaORM runs each migration inside a
//! transaction and `CONCURRENTLY` cannot run in one. The non-concurrent build takes `SHARE` on
//! `event_outbox`, blocking enqueues for its duration; on two partial indexes over a table
//! whose realistic size here is thousands to low millions of rows that is sub-second, and the
//! `lock_timeout` bounds the worst case.
//!
//! **The backfill is deliberate.** Leaving pre-existing parked rows at `parked_at = NULL` would
//! create a permanently uncollectable set: invisible to any time filter (NULL fails every
//! comparison, so bulk replay could never reach them) and permanently ineligible for retention
//! even if `parked_days` were raised. Stamping `now()` says exactly what is true — "we do not
//! know when this parked; it was parked as of the migration" — and starts its retention clock
//! at the migration rather than deleting it instantly.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "event_outbox"
                 ADD COLUMN IF NOT EXISTS parked_at TIMESTAMPTZ NULL,
                 ADD COLUMN IF NOT EXISTS last_error TEXT NULL;"#,
        )
        .await?;
        conn.execute_unprepared(r#"UPDATE "event_outbox" SET parked_at = now() WHERE parked = true AND parked_at IS NULL;"#)
            .await?;
        // Retention's published-sweep predicate.
        conn.execute_unprepared(r#"CREATE INDEX IF NOT EXISTS ix_event_outbox_published ON "event_outbox" (published_at) WHERE published_at IS NOT NULL;"#)
            .await?;
        // The dead-letter list's ordering + keyset paging (`ORDER BY id DESC`, `id < cursor`).
        conn.execute_unprepared(r#"CREATE INDEX IF NOT EXISTS ix_event_outbox_parked ON "event_outbox" (id) WHERE parked = true;"#)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS ix_event_outbox_parked;").await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS ix_event_outbox_published;").await?;
        conn.execute_unprepared(r#"ALTER TABLE "event_outbox" DROP COLUMN IF EXISTS last_error, DROP COLUMN IF EXISTS parked_at;"#)
            .await?;
        Ok(())
    }
}
