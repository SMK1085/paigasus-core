// SPDX-License-Identifier: Apache-2.0

//! m0002 — create the tenancy hierarchy: `organization` → `team` → `project`, plus
//! `membership` (a principal's belongs-to link into exactly one of the three).
//!
//! Constraint/index **names** here are load-bearing: the persistence adapter's D7 error
//! mapping (`pg_repository::conflict_kind`) matches Postgres constraint-violation messages
//! against these exact strings, so every unique/check constraint is created via raw SQL
//! with an explicit name — never sea-query's `.unique_key()`, whose auto-generated name
//! would break that mapping.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Organization {
    Table,
    Id,
    Prn,
    Slug,
    Name,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Team {
    Table,
    Id,
    OrgId,
    Prn,
    Slug,
    Name,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Project {
    Table,
    Id,
    TeamId,
    OrgId,
    Prn,
    Slug,
    Name,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Membership {
    Table,
    Id,
    PrincipalId,
    OrgId,
    TeamId,
    ProjectId,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- organization ---------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Organization::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Organization::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Organization::Prn).text().not_null())
                    .col(ColumnDef::new(Organization::Slug).text().not_null())
                    .col(ColumnDef::new(Organization::Name).text().not_null())
                    .col(ColumnDef::new(Organization::Status).text().not_null())
                    .col(ColumnDef::new(Organization::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Organization::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "organization"
                    ADD CONSTRAINT uq_organization_prn UNIQUE (prn),
                    ADD CONSTRAINT uq_organization_slug UNIQUE (slug);"#,
            )
            .await?;

        // --- team -------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Team::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Team::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Team::OrgId).uuid().not_null())
                    .col(ColumnDef::new(Team::Prn).text().not_null())
                    .col(ColumnDef::new(Team::Slug).text().not_null())
                    .col(ColumnDef::new(Team::Name).text().not_null())
                    .col(ColumnDef::new(Team::Status).text().not_null())
                    .col(ColumnDef::new(Team::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Team::UpdatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_team_org")
                            .from(Team::Table, Team::OrgId)
                            .to(Organization::Table, Organization::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager.create_index(Index::create().name("ix_team_org").table(Team::Table).col(Team::OrgId).to_owned()).await?;
        // `uq_team_id_org` backs the composite `fk_project_team` FK created below — a
        // Postgres FK to a non-PK column tuple requires a unique constraint on that tuple.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "team"
                    ADD CONSTRAINT uq_team_prn UNIQUE (prn),
                    ADD CONSTRAINT uq_team_org_slug UNIQUE (org_id, slug),
                    ADD CONSTRAINT uq_team_id_org UNIQUE (id, org_id);"#,
            )
            .await?;

        // --- project ------------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Project::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Project::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Project::TeamId).uuid().not_null())
                    .col(ColumnDef::new(Project::OrgId).uuid().not_null())
                    .col(ColumnDef::new(Project::Prn).text().not_null())
                    .col(ColumnDef::new(Project::Slug).text().not_null())
                    .col(ColumnDef::new(Project::Name).text().not_null())
                    .col(ColumnDef::new(Project::Status).text().not_null())
                    .col(ColumnDef::new(Project::CreatedAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Project::UpdatedAt).timestamp_with_time_zone().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(Index::create().name("ix_project_team").table(Project::Table).col(Project::TeamId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_project_org").table(Project::Table).col(Project::OrgId).to_owned())
            .await?;
        // No single-column FK to `team` — only the composite (team_id, org_id) → team
        // (id, org_id) below, so a project can never reference a team from another org.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "project"
                    ADD CONSTRAINT uq_project_prn UNIQUE (prn),
                    ADD CONSTRAINT uq_project_team_slug UNIQUE (team_id, slug),
                    ADD CONSTRAINT fk_project_team FOREIGN KEY (team_id, org_id)
                        REFERENCES "team" (id, org_id) ON DELETE CASCADE;"#,
            )
            .await?;

        // --- membership -----------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Membership::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Membership::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Membership::PrincipalId).uuid().not_null())
                    .col(ColumnDef::new(Membership::OrgId).uuid().null())
                    .col(ColumnDef::new(Membership::TeamId).uuid().null())
                    .col(ColumnDef::new(Membership::ProjectId).uuid().null())
                    .col(ColumnDef::new(Membership::CreatedAt).timestamp_with_time_zone().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_membership_principal")
                            .from(Membership::Table, Membership::PrincipalId)
                            // `principal` is created by m0001, whose `Principal` iden enum is
                            // private to that module — reference the existing table/column by
                            // name instead of importing it.
                            .to(Alias::new("principal"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_membership_org")
                            .from(Membership::Table, Membership::OrgId)
                            .to(Organization::Table, Organization::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_membership_team")
                            .from(Membership::Table, Membership::TeamId)
                            .to(Team::Table, Team::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_membership_project")
                            .from(Membership::Table, Membership::ProjectId)
                            .to(Project::Table, Project::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(Index::create().name("ix_membership_principal").table(Membership::Table).col(Membership::PrincipalId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_membership_org").table(Membership::Table).col(Membership::OrgId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_membership_team").table(Membership::Table).col(Membership::TeamId).to_owned())
            .await?;
        manager
            .create_index(Index::create().name("ix_membership_project").table(Membership::Table).col(Membership::ProjectId).to_owned())
            .await?;
        // Exactly one of org_id/team_id/project_id may be set (spec D-membership), plus a
        // partial unique index per target kind so a principal can't join the same node twice.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "membership"
                    ADD CONSTRAINT ck_membership_one_target
                        CHECK (num_nonnulls(org_id, team_id, project_id) = 1);
                   CREATE UNIQUE INDEX uq_membership_principal_org ON "membership" (principal_id, org_id) WHERE org_id IS NOT NULL;
                   CREATE UNIQUE INDEX uq_membership_principal_team ON "membership" (principal_id, team_id) WHERE team_id IS NOT NULL;
                   CREATE UNIQUE INDEX uq_membership_principal_project ON "membership" (principal_id, project_id) WHERE project_id IS NOT NULL;"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Membership::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Project::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Team::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Organization::Table).to_owned()).await?;
        Ok(())
    }
}
