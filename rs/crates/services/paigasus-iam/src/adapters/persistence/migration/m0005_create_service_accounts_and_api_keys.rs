// SPDX-License-Identifier: Apache-2.0

//! m0005 — create `service_account` and `api_key` (SMA-445 design §5.1).
//!
//! `service_account` shares its PK with `principal` (mirrors `user`, m0001) and carries **no
//! `status` column** — SA lifecycle status lives on the `principal` row (D16). Exactly one of
//! `owner_org_id`/`owner_team_id`/`owner_project_id` is set (`ck_service_account_owner`); a
//! name is unique per owner via three partial unique indexes (the m0002 NULL-aware precedent).
//!
//! `api_key` has its own UUID PK. Exactly one of `scope_org_id`/`scope_team_id`/
//! `scope_project_id` is set (`ck_api_key_scope`), and each is FK'd to
//! `organization`/`team`/`project` `ON DELETE CASCADE` — mirroring `role_grant`'s scope trio
//! (m0004), so a key can never reference a node that doesn't exist.
//!
//! Constraint/index **names** here are load-bearing: the persistence adapter's D7 error
//! mapping (`pg_repository::conflict_kind`) matches Postgres constraint-violation messages
//! against these exact strings, so every unique/check constraint is created via raw SQL with
//! an explicit name — never sea-query's auto-named `.unique_key()`/`.check()` (m0002/m0004
//! convention).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum ServiceAccount {
    Table,
    PrincipalId,
    OwnerOrgId,
    OwnerTeamId,
    OwnerProjectId,
    Name,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ApiKey {
    Table,
    Id,
    ServiceAccountId,
    ScopeOrgId,
    ScopeTeamId,
    ScopeProjectId,
    Prefix,
    KeyHash,
    Status,
    ExpiresAt,
    LastUsedAt,
    CreatedAt,
    RevokedAt,
    ScopeActions,
    ScopeRoles,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- service_account --------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(ServiceAccount::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ServiceAccount::PrincipalId).uuid().not_null().primary_key())
                    .col(ColumnDef::new(ServiceAccount::OwnerOrgId).uuid().null())
                    .col(ColumnDef::new(ServiceAccount::OwnerTeamId).uuid().null())
                    .col(ColumnDef::new(ServiceAccount::OwnerProjectId).uuid().null())
                    .col(ColumnDef::new(ServiceAccount::Name).text().not_null())
                    .col(ColumnDef::new(ServiceAccount::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(ServiceAccount::UpdatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_account_principal")
                            .from(ServiceAccount::Table, ServiceAccount::PrincipalId)
                            // `principal` is created by m0001, whose `Principal` iden enum is
                            // private to that module — reference the existing table/column by
                            // name instead of importing it (m0002/m0003/m0004 convention).
                            .to(Alias::new("principal"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_account_org")
                            .from(ServiceAccount::Table, ServiceAccount::OwnerOrgId)
                            // `organization`/`team`/`project` are created by m0002, whose iden
                            // enums are private to that module — reference by name.
                            .to(Alias::new("organization"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_account_team")
                            .from(ServiceAccount::Table, ServiceAccount::OwnerTeamId)
                            .to(Alias::new("team"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_service_account_project")
                            .from(ServiceAccount::Table, ServiceAccount::OwnerProjectId)
                            .to(Alias::new("project"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // Exactly one owner target (spec D-service_account, mirrors `ck_membership_one_target`),
        // plus a partial unique index per owner kind so a name is unique within its owner scope.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "service_account"
                    ADD CONSTRAINT ck_service_account_owner
                        CHECK (num_nonnulls(owner_org_id, owner_team_id, owner_project_id) = 1);
                   CREATE UNIQUE INDEX uq_service_account_org_name ON "service_account" (owner_org_id, name) WHERE owner_org_id IS NOT NULL;
                   CREATE UNIQUE INDEX uq_service_account_team_name ON "service_account" (owner_team_id, name) WHERE owner_team_id IS NOT NULL;
                   CREATE UNIQUE INDEX uq_service_account_project_name ON "service_account" (owner_project_id, name) WHERE owner_project_id IS NOT NULL;"#,
            )
            .await?;

        // --- api_key ------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(ApiKey::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ApiKey::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(ApiKey::ServiceAccountId).uuid().not_null())
                    .col(ColumnDef::new(ApiKey::ScopeOrgId).uuid().null())
                    .col(ColumnDef::new(ApiKey::ScopeTeamId).uuid().null())
                    .col(ColumnDef::new(ApiKey::ScopeProjectId).uuid().null())
                    .col(ColumnDef::new(ApiKey::Prefix).text().not_null())
                    .col(ColumnDef::new(ApiKey::KeyHash).text().not_null())
                    .col(ColumnDef::new(ApiKey::Status).text().not_null())
                    .col(ColumnDef::new(ApiKey::ExpiresAt).timestamp_with_time_zone().null())
                    .col(ColumnDef::new(ApiKey::LastUsedAt).timestamp_with_time_zone().null())
                    .col(ColumnDef::new(ApiKey::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(ApiKey::RevokedAt).timestamp_with_time_zone().null())
                    .col(ColumnDef::new(ApiKey::ScopeActions).text().null())
                    .col(ColumnDef::new(ApiKey::ScopeRoles).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_service_account")
                            .from(ApiKey::Table, ApiKey::ServiceAccountId)
                            .to(ServiceAccount::Table, ServiceAccount::PrincipalId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_scope_org")
                            .from(ApiKey::Table, ApiKey::ScopeOrgId)
                            // `organization`/`team`/`project` are created by m0002, whose iden
                            // enums are private to that module — reference by name.
                            .to(Alias::new("organization"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_scope_team")
                            .from(ApiKey::Table, ApiKey::ScopeTeamId)
                            .to(Alias::new("team"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_scope_project")
                            .from(ApiKey::Table, ApiKey::ScopeProjectId)
                            .to(Alias::new("project"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(Index::create().name("ix_api_key_service_account").table(ApiKey::Table).col(ApiKey::ServiceAccountId).to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "api_key"
                    ADD CONSTRAINT uq_api_key_hash UNIQUE (key_hash),
                    ADD CONSTRAINT ck_api_key_scope
                        CHECK (num_nonnulls(scope_org_id, scope_team_id, scope_project_id) = 1);"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(ApiKey::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(ServiceAccount::Table).to_owned()).await?;
        Ok(())
    }
}
