// SPDX-License-Identifier: Apache-2.0

//! m0007 — create `event_outbox` (SMA-446, Slice B).
//!
//! Structured columns are serialized **TEXT**, mirroring m0006's `audit_log` convention
//! (`determining_policies`/`detail`) and m0005's `api_key.scope_actions`/`scope_roles`
//! precedent: `payload` is the domain event's `serde_json::Value` written via its plain
//! `to_string()` — no native Postgres `jsonb`, so no new SeaORM column feature.
//!
//! No foreign keys: `aggregate_prn`/`actor_prn` are free-form PRN text, not FK'd to any row —
//! an outbox row (and the audit trail it feeds once relayed) must survive its aggregate or
//! actor being deleted later, same posture as `audit_log`'s `actor_prn`/`resource_prn`.
//!
//! The partial index `ix_event_outbox_unpublished` is the relay poll's index: `published_at IS
//! NULL AND parked = false` is exactly the "still needs relaying" predicate, so every poll scan
//! stays index-only rather than scanning the whole (eventually large, append-only) table.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum EventOutbox {
    Table,
    Id,
    OccurredAt,
    EventType,
    SchemaVersion,
    AggregatePrn,
    ActorPrn,
    Payload,
    CorrelationId,
    PublishedAt,
    Attempts,
    Parked,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EventOutbox::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(EventOutbox::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(EventOutbox::OccurredAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(EventOutbox::EventType).text().not_null())
                    .col(ColumnDef::new(EventOutbox::SchemaVersion).integer().not_null().default(1))
                    .col(ColumnDef::new(EventOutbox::AggregatePrn).text().not_null())
                    .col(ColumnDef::new(EventOutbox::ActorPrn).text().null())
                    .col(ColumnDef::new(EventOutbox::Payload).text().not_null())
                    .col(ColumnDef::new(EventOutbox::CorrelationId).uuid().null())
                    .col(ColumnDef::new(EventOutbox::PublishedAt).timestamp_with_time_zone().null())
                    .col(ColumnDef::new(EventOutbox::Attempts).integer().not_null().default(0))
                    .col(ColumnDef::new(EventOutbox::Parked).boolean().not_null().default(false))
                    .to_owned(),
            )
            .await?;

        // The relay poll's index: "still needs relaying" is exactly `published_at IS NULL AND
        // parked = false` (m0005/m0002 raw-SQL precedent for a `WHERE`-qualified index — sea-query
        // has no builder support for a partial index's predicate).
        manager
            .get_connection()
            .execute_unprepared(r#"CREATE INDEX ix_event_outbox_unpublished ON "event_outbox" (id) WHERE published_at IS NULL AND parked = false;"#)
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(EventOutbox::Table).to_owned()).await?;
        Ok(())
    }
}
