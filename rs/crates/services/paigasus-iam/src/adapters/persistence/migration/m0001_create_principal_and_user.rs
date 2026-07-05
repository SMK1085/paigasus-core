// SPDX-License-Identifier: Apache-2.0

//! m0001 — create `principal` and `user` (1:1, shared PK). Text-backed enum columns.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Principal {
    Table,
    Id,
    Prn,
    Kind,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    PrincipalId,
    Email,
    DisplayName,
    Locale,
    Timezone,
    CreatedAt,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Principal::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Principal::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Principal::Prn).text().not_null().unique_key())
                    .col(ColumnDef::new(Principal::Kind).text().not_null())
                    .col(ColumnDef::new(Principal::Status).text().not_null())
                    .col(ColumnDef::new(Principal::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Principal::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(User::PrincipalId).uuid().not_null().primary_key())
                    .col(ColumnDef::new(User::Email).text().not_null().unique_key())
                    .col(ColumnDef::new(User::DisplayName).text().not_null())
                    .col(ColumnDef::new(User::Locale).text().null())
                    .col(ColumnDef::new(User::Timezone).text().null())
                    .col(ColumnDef::new(User::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(User::UpdatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_principal")
                            .from(User::Table, User::PrincipalId)
                            .to(Principal::Table, Principal::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(User::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Principal::Table).to_owned()).await?;
        Ok(())
    }
}
