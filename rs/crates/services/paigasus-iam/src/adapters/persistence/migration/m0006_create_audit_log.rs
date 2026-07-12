// SPDX-License-Identifier: Apache-2.0

//! m0006 — create `audit_log` (SMA-446 design §D14, Slice A).
//!
//! **Slice-A simplification:** ships the plain table only. Monthly range-partitioning and
//! outcome-aware retention (spec §D14) land with the retention/pruning follow-up; the schema
//! and its query shape stay partition-compatible from day one — every filter predicate
//! includes `occurred_at`/`id` (the keyset-paging cursor is `id`, a UUIDv7 and therefore
//! time-ordered).
//!
//! Structured columns are serialized **TEXT**, not native Postgres `text[]`/`jsonb` — mirrors
//! `api_key.scope_actions`/`scope_roles` (m0005): `determining_policies` is a JSON-encoded
//! `Vec<String>` when set (A5 encodes it), `detail` is a JSON string (denials default to
//! `"{}"`). This avoids introducing a new SeaORM array/json column feature for Slice A.
//!
//! No foreign keys: `actor_prn`/`resource_prn` are free-form PRN text, not FK'd to
//! `principal`/tenancy rows — an audit entry must survive its actor or resource being deleted
//! later (an audit trail outlives the subjects it describes).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum AuditLog {
    Table,
    Id,
    OccurredAt,
    ActorPrn,
    Action,
    ResourcePrn,
    Outcome,
    DeterminingPolicies,
    Detail,
    CorrelationId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(AuditLog::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(AuditLog::OccurredAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(AuditLog::ActorPrn).text().null())
                    .col(ColumnDef::new(AuditLog::Action).text().not_null())
                    .col(ColumnDef::new(AuditLog::ResourcePrn).text().null())
                    .col(ColumnDef::new(AuditLog::Outcome).text().not_null())
                    .col(ColumnDef::new(AuditLog::DeterminingPolicies).text().null())
                    .col(ColumnDef::new(AuditLog::Detail).text().not_null().default("{}"))
                    .col(ColumnDef::new(AuditLog::CorrelationId).uuid().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(Index::create().name("ix_audit_log_occurred_at").table(AuditLog::Table).col(AuditLog::OccurredAt).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_audit_log_actor_prn").table(AuditLog::Table).col(AuditLog::ActorPrn).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_audit_log_resource_prn").table(AuditLog::Table).col(AuditLog::ResourcePrn).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_audit_log_action").table(AuditLog::Table).col(AuditLog::Action).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_audit_log_outcome").table(AuditLog::Table).col(AuditLog::Outcome).to_owned())
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(AuditLog::Table).to_owned()).await?;
        Ok(())
    }
}
