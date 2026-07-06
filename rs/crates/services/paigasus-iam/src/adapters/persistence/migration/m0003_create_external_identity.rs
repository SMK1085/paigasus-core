// SPDX-License-Identifier: Apache-2.0

//! m0003 — create `external_identity`: a persisted link between a principal and one external
//! IdP identity (issuer, subject), spec §5.1 (SMA-443, M2 authentication).
//!
//! Constraint/index **names** here are load-bearing: the persistence adapter's D7 error
//! mapping (`persistence::conflict_kind`) matches Postgres constraint-violation messages
//! against these exact strings, so the uniqueness constraint is created with an explicit
//! name — never sea-query's `.unique_key()`, whose auto-generated name would break that
//! mapping. No FK cascade on `principal_id` — principals are never hard-deleted in v1.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum ExternalIdentity {
    Table,
    Id,
    PrincipalId,
    Issuer,
    Subject,
    CreatedAt,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ExternalIdentity::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ExternalIdentity::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(ExternalIdentity::PrincipalId).uuid().not_null())
                    .col(ColumnDef::new(ExternalIdentity::Issuer).text().not_null())
                    .col(ColumnDef::new(ExternalIdentity::Subject).text().not_null())
                    .col(ColumnDef::new(ExternalIdentity::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(ExternalIdentity::UpdatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_external_identity_principal")
                            .from(ExternalIdentity::Table, ExternalIdentity::PrincipalId)
                            // `principal` is created by m0001, whose `Principal` iden enum is
                            // private to that module — reference the existing table/column by
                            // name instead of importing it. No `.on_delete(...)`: principals
                            // are never hard-deleted in v1 (status-based lifecycle).
                            .to(Alias::new("principal"), Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        // Named as a CONSTRAINT (not `Index::create().unique()`) so it shows up in
        // `pg_constraint`, matching the D7 lookup convention (m0002) and the AC's
        // `constraint_names_are_stable` test.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "external_identity"
                    ADD CONSTRAINT uq_external_identity_issuer_subject UNIQUE (issuer, subject);"#,
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("ix_external_identity_principal")
                    .table(ExternalIdentity::Table)
                    .col(ExternalIdentity::PrincipalId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(ExternalIdentity::Table).to_owned()).await?;
        Ok(())
    }
}
