// SPDX-License-Identifier: Apache-2.0

//! m0004 — create the authorization schema (ADR-0013, design §6.1): `policy` (authored Cedar
//! policies/templates), `role` (the code-defined role catalog's persisted/introspectable
//! form), and `role_grant` (a materialized grant of a role to a principal at a scope — the
//! synthetic `Root` or a concrete tenancy node).
//!
//! ServiceAccount was cut from M3 (GATE 1 decision) — this migration does **not** touch
//! `PrincipalKind` or add a `service_account` table.
//!
//! Constraint/index **names** here are load-bearing: the persistence adapter's D7 error
//! mapping (`pg_repository::conflict_kind`) matches Postgres constraint-violation messages
//! against these exact strings, so every unique/check constraint is created via raw SQL with
//! an explicit name — never sea-query's auto-named `.unique_key()`/`.check()` (m0002
//! convention).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Policy {
    Table,
    // Named `Id` (not `PolicyId`) to dodge clippy::enum_variant_names (a variant literally
    // prefixed with its enum's own name) — `#[sea_orm(iden = "...")]` still renders the real
    // `policy_id` column name (spec §6.1).
    #[sea_orm(iden = "policy_id")]
    Id,
    Kind,
    Source,
    Description,
    System,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Role {
    Table,
    Key,
    TemplateId,
    ScopeKinds,
    Description,
    System,
    CreatedAt,
}

#[derive(DeriveIden)]
enum RoleGrant {
    Table,
    Id,
    PrincipalId,
    RoleKey,
    ScopeKind,
    ScopeNodePrn,
    ScopeOrgId,
    ScopeTeamId,
    ScopeProjectId,
    LinkedPolicyId,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- policy -----------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Policy::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Policy::Id).text().not_null().primary_key())
                    .col(ColumnDef::new(Policy::Kind).text().not_null())
                    .col(ColumnDef::new(Policy::Source).text().not_null())
                    .col(ColumnDef::new(Policy::Description).text().null())
                    .col(ColumnDef::new(Policy::System).boolean().not_null().default(false))
                    .col(ColumnDef::new(Policy::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Policy::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(r#"ALTER TABLE "policy" ADD CONSTRAINT ck_policy_kind CHECK (kind IN ('static', 'template'));"#)
            .await?;

        // --- role -----------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Role::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Role::Key).text().not_null().primary_key())
                    .col(ColumnDef::new(Role::TemplateId).text().not_null())
                    // JSON array of node kinds (e.g. `["organization"]`) — encoding pinned
                    // (design §6.1); roles are code-defined, this column is only the
                    // persisted/introspectable form, never queried by structure.
                    .col(ColumnDef::new(Role::ScopeKinds).text().not_null())
                    .col(ColumnDef::new(Role::Description).text().null())
                    .col(ColumnDef::new(Role::System).boolean().not_null().default(false))
                    .col(ColumnDef::new(Role::CreatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(ForeignKey::create().name("fk_role_template").from(Role::Table, Role::TemplateId).to(Policy::Table, Policy::Id))
                    .to_owned(),
            )
            .await?;

        // --- role_grant -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(RoleGrant::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RoleGrant::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(RoleGrant::PrincipalId).uuid().not_null())
                    .col(ColumnDef::new(RoleGrant::RoleKey).text().not_null())
                    .col(ColumnDef::new(RoleGrant::ScopeKind).text().not_null())
                    .col(ColumnDef::new(RoleGrant::ScopeNodePrn).text().not_null())
                    .col(ColumnDef::new(RoleGrant::ScopeOrgId).uuid().null())
                    .col(ColumnDef::new(RoleGrant::ScopeTeamId).uuid().null())
                    .col(ColumnDef::new(RoleGrant::ScopeProjectId).uuid().null())
                    .col(ColumnDef::new(RoleGrant::LinkedPolicyId).text().not_null())
                    .col(ColumnDef::new(RoleGrant::CreatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_role_grant_principal")
                            .from(RoleGrant::Table, RoleGrant::PrincipalId)
                            // `principal` is created by m0001, whose `Principal` iden enum is
                            // private to that module — reference the existing table/column by
                            // name instead of importing it (m0002/m0003 convention).
                            .to(Alias::new("principal"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(ForeignKey::create().name("fk_role_grant_role").from(RoleGrant::Table, RoleGrant::RoleKey).to(Role::Table, Role::Key))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_role_grant_org")
                            .from(RoleGrant::Table, RoleGrant::ScopeOrgId)
                            // `organization`/`team`/`project` are created by m0002, whose iden
                            // enums are private to that module — reference by name (m0003
                            // convention).
                            .to(Alias::new("organization"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_role_grant_team")
                            .from(RoleGrant::Table, RoleGrant::ScopeTeamId)
                            .to(Alias::new("team"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_role_grant_project")
                            .from(RoleGrant::Table, RoleGrant::ScopeProjectId)
                            .to(Alias::new("project"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(Index::create().name("ix_role_grant_principal").table(RoleGrant::Table).col(RoleGrant::PrincipalId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_role_grant_org").table(RoleGrant::Table).col(RoleGrant::ScopeOrgId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_role_grant_team").table(RoleGrant::Table).col(RoleGrant::ScopeTeamId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_role_grant_project").table(RoleGrant::Table).col(RoleGrant::ScopeProjectId).to_owned())
            .await?;

        // `ck_role_grant_scope` ties `scope_kind` to exactly the matching non-null scope FK —
        // `root` means all three of scope_org_id/scope_team_id/scope_project_id are NULL (a
        // Root/platform grant is first-class, not a NULL-count hack), while
        // `organization`/`team`/`project` mean exactly the matching FK is set and the other two
        // are NULL.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "role_grant"
                    ADD CONSTRAINT ck_role_grant_scope_kind
                        CHECK (scope_kind IN ('root', 'organization', 'team', 'project')),
                    ADD CONSTRAINT uq_role_grant_principal_role_scope
                        UNIQUE (principal_id, role_key, scope_node_prn),
                    ADD CONSTRAINT uq_role_grant_linked_policy
                        UNIQUE (linked_policy_id),
                    ADD CONSTRAINT ck_role_grant_scope
                        CHECK (
                            (scope_kind = 'root' AND scope_org_id IS NULL AND scope_team_id IS NULL AND scope_project_id IS NULL)
                            OR (scope_kind = 'organization' AND scope_org_id IS NOT NULL AND scope_team_id IS NULL AND scope_project_id IS NULL)
                            OR (scope_kind = 'team' AND scope_org_id IS NULL AND scope_team_id IS NOT NULL AND scope_project_id IS NULL)
                            OR (scope_kind = 'project' AND scope_org_id IS NULL AND scope_team_id IS NULL AND scope_project_id IS NOT NULL)
                        );"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(RoleGrant::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Role::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Policy::Table).to_owned()).await?;
        Ok(())
    }
}
