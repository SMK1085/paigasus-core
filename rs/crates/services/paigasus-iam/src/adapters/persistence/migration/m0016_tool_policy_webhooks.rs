// SPDX-License-Identifier: Apache-2.0

//! m0016 - the webhook opt-in list of a tenant (SMA-856).
//!
//! `tool_policy_settings.webhooks` is a JSON array of provider names, stored as text like
//! `role.scope_kinds`. The default `'[]'` gives every existing row an empty list.
//!
//! **Rollback hazard (spec section 6.6).** The `down` step drops the column and the lists in it.
//! After a rollback, a tenant that opted in to a webhook provider loses that opt-in. The
//! gateway then stops calling the provider for that tenant. Re-apply the list by hand after
//! the next `up`.
//!
//! `SET LOCAL lock_timeout` mirrors m0013 and m0015.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_settings" ADD COLUMN webhooks TEXT NOT NULL DEFAULT '[]';"#).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_settings" DROP COLUMN IF EXISTS webhooks;"#).await?;
        Ok(())
    }
}
