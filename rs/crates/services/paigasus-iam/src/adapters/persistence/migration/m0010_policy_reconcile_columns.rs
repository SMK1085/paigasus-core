// SPDX-License-Identifier: Apache-2.0

//! m0010 — `policy` gains the boot-reconciliation columns (SMA-477).
//!
//! - `content_fingerprint` is a blake3 hex of the `(kind, source, description)` triple this
//!   service last wrote for the row. It exists to tell a routine code change (silent) from an
//!   out-of-band edit (WARN + audit) — see `authz::reconcile`'s module docs for why it is a
//!   provenance hint and not a security control.
//! - `starter_revision` is the `authz::roles::STARTER_POLICY_REVISION` of the binary that last
//!   wrote the row. Reconcile refuses to write when the stored revision is HIGHER than its own,
//!   which is what stops an older replica pushing its policy set onto the fleet through this
//!   shared table.
//!
//! **No backfill.** blake3 is not computable in Postgres (`pgcrypto` does not offer it), so
//! both columns start NULL and the first `reconcile_starter` after this migration stamps every
//! system row. A NULL fingerprint reads as "provenance unknown" (adopt, do not warn) and a NULL
//! revision reads as `0`.
//!
//! **Every statement is idempotent, deliberately** — m0007/m0008/m0009 record that SeaORM's
//! migrator does not serialize concurrent `up()` across replicas, so a bare `ADD COLUMN` would
//! fail the loser of a simultaneous first boot. `SET LOCAL lock_timeout` mirrors m0008/m0009 so
//! the `ACCESS EXCLUSIVE` request backs off rather than queueing ahead of in-flight
//! `PolicyService::put` writes during a rolling deploy.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "policy"
                 ADD COLUMN IF NOT EXISTS content_fingerprint TEXT NULL,
                 ADD COLUMN IF NOT EXISTS starter_revision INTEGER NULL;"#,
        )
        .await?;
        // Pins the encoding `authz::reconcile::content_fingerprint` promises: lowercase hex,
        // 64 chars. Dropped first so a re-run replaces rather than errors.
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP CONSTRAINT IF EXISTS ck_policy_fingerprint;"#).await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "policy" ADD CONSTRAINT ck_policy_fingerprint
                 CHECK (content_fingerprint IS NULL OR content_fingerprint ~ '^[0-9a-f]{64}$');"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP CONSTRAINT IF EXISTS ck_policy_fingerprint;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "policy" DROP COLUMN IF EXISTS starter_revision, DROP COLUMN IF EXISTS content_fingerprint;"#)
            .await?;
        Ok(())
    }
}
