// SPDX-License-Identifier: Apache-2.0

//! m0015 — the version counter and the held versions (SMA-781, plan H1 to H4).
//!
//! `tool_policy_state.last_version` is the highest version number that any row of a set holds,
//! held and discarded rows included. A new version takes `GREATEST(live_version, last_version) + 1`,
//! so a held row and the next save never share a number.
//!
//! A held row of a failed compile has no compiled bytes, so `compiled` allows NULL. Two CHECKs
//! keep that safe: only a held or discarded row may have no bytes, and a held row has a
//! `base_version` and a `report`.
//!
//! `SET LOCAL lock_timeout` mirrors m0013. The ALTER statements need a short lock on two small
//! tables, and the timeout bounds the wait.

use sea_orm_migration::prelude::*;

/// Sets `last_version` of every state row to the highest stored version number of its set.
/// The migration and the backfill test both run this statement.
pub const M0015_BACKFILL_SQL: &str = "UPDATE tool_policy_state s SET last_version = COALESCE((SELECT MAX(v.version) FROM tool_policy_set_version v \
     WHERE v.kind = s.kind AND v.org_id IS NOT DISTINCT FROM s.org_id), 0);";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_state" ADD COLUMN last_version BIGINT NOT NULL DEFAULT 0;"#).await?;
        conn.execute_unprepared(M0015_BACKFILL_SQL).await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_set_version" ALTER COLUMN compiled DROP NOT NULL;"#).await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "tool_policy_set_version" ADD CONSTRAINT ck_tool_policy_version_compiled
                 CHECK (compiled IS NOT NULL OR status IN ('held','discarded'));"#,
        )
        .await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "tool_policy_set_version" ADD CONSTRAINT ck_tool_policy_version_held_fields
                 CHECK (status <> 'held' OR (base_version IS NOT NULL AND report IS NOT NULL));"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_set_version" DROP CONSTRAINT IF EXISTS ck_tool_policy_version_held_fields;"#)
            .await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_set_version" DROP CONSTRAINT IF EXISTS ck_tool_policy_version_compiled;"#)
            .await?;
        // PR 2 code numbers a save `live_version + 1`. A held or discarded row above the live
        // version would hold that number, so every save of the set would hit the unique index.
        conn.execute_unprepared(
            r#"DELETE FROM "tool_policy_set_version" v USING "tool_policy_state" s
                 WHERE v.status IN ('held','discarded') AND v.version > s.live_version
                   AND v.kind = s.kind AND v.org_id IS NOT DISTINCT FROM s.org_id;"#,
        )
        .await?;
        conn.execute_unprepared(r#"DELETE FROM "tool_policy_set_version" WHERE compiled IS NULL;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_set_version" ALTER COLUMN compiled SET NOT NULL;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "tool_policy_state" DROP COLUMN IF EXISTS last_version;"#).await?;
        Ok(())
    }
}
