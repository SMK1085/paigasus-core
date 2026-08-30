// SPDX-License-Identifier: Apache-2.0

//! m0011 — the tenancy tables gain audit-stamp actor columns (SMA-440).
//!
//! `created_by`/`modified_by` hold a canonical PRN as free-form text with **no foreign key**,
//! following `audit_log.actor_prn` (m0006): an organization must survive the deletion of the
//! principal that created it.
//!
//! `membership` gets `created_by` only — it has no `updated_at` and `iam.proto` marks it
//! immutable.
//!
//! **No backfill.** A pre-migration row keeps NULL, and NULL is the absent `Actor` that
//! `actor.proto` already defines as unknown-or-system. A synthetic "system" PRN would be a
//! *valid* PRN and would read as a real principal, which is worse than nothing.
//!
//! Every statement is idempotent and `SET LOCAL lock_timeout` mirrors m0008/m0009/m0010:
//! SeaORM's migrator does not serialize concurrent `up()` across replicas, and `ADD COLUMN`
//! takes `ACCESS EXCLUSIVE` on tables every authorization decision reads through.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        for table in ["organization", "team", "project"] {
            conn.execute_unprepared(&format!(
                r#"ALTER TABLE "{table}"
                     ADD COLUMN IF NOT EXISTS created_by TEXT NULL,
                     ADD COLUMN IF NOT EXISTS modified_by TEXT NULL;"#
            ))
            .await?;
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP CONSTRAINT IF EXISTS ck_{table}_audit_actor_prn;"#))
                .await?;
            conn.execute_unprepared(&format!(
                r#"ALTER TABLE "{table}" ADD CONSTRAINT ck_{table}_audit_actor_prn
                     CHECK ((created_by IS NULL OR created_by LIKE 'prn:%')
                        AND (modified_by IS NULL OR modified_by LIKE 'prn:%'));"#
            ))
            .await?;
        }
        conn.execute_unprepared(r#"ALTER TABLE "membership" ADD COLUMN IF NOT EXISTS created_by TEXT NULL;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP CONSTRAINT IF EXISTS ck_membership_audit_actor_prn;"#).await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "membership" ADD CONSTRAINT ck_membership_audit_actor_prn
                 CHECK (created_by IS NULL OR created_by LIKE 'prn:%');"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        for table in ["organization", "team", "project"] {
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP CONSTRAINT IF EXISTS ck_{table}_audit_actor_prn;"#))
                .await?;
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP COLUMN IF EXISTS modified_by, DROP COLUMN IF EXISTS created_by;"#))
                .await?;
        }
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP CONSTRAINT IF EXISTS ck_membership_audit_actor_prn;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP COLUMN IF EXISTS created_by;"#).await?;
        Ok(())
    }
}
