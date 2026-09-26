// SPDX-License-Identifier: Apache-2.0

//! m0012 — an index for the scope-filtered role grant query (SMA-699).
//!
//! `RoleGrantQuery::find` with a scope and no principal (the gateway-console org page) had no
//! index to use: no index led with `scope_node_prn`, so each call could scan the full
//! `role_grant` table. `(scope_node_prn, principal_id, id)` finds one node's rows and returns
//! them in the query's `ORDER BY g.principal_id, g.id` order, so a page stops after
//! `offset + limit` rows with no sort.
//!
//! **No `CONCURRENTLY`.** Production runs every pending migration in one outer transaction
//! (`migrate_under_lock`), and `CREATE INDEX CONCURRENTLY` cannot run in a transaction. The
//! build holds a SHARE lock on `role_grant` until that transaction commits: grants, revokes, org
//! creation and cascading deletes wait, reads continue. `SET LOCAL lock_timeout` bounds the wait,
//! as in m0008-m0011. For a large table, an operator builds the index `CONCURRENTLY` before the
//! deploy; `IF NOT EXISTS` then makes this migration only check it.
//!
//! **An INVALID index fails the migration.** A failed `CREATE INDEX CONCURRENTLY` leaves an
//! INVALID index with this name. `IF NOT EXISTS` would skip the create, and the planner ignores
//! an INVALID index, so the query would scan again with no signal.
//!
//! **`IF NOT EXISTS` also accepts a VALID index with this name but other columns.** An operator
//! build with a wrong definition passes this migration's check. The operator must build the
//! index with the exact columns `(scope_node_prn, principal_id, id)`. This migration does not
//! check the column list.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DbBackend, Statement};

const INDEX: &str = "ix_role_grant_scope_node_prn_principal_id";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(&format!(r#"CREATE INDEX IF NOT EXISTS {INDEX} ON "role_grant" (scope_node_prn, principal_id, id);"#))
            .await?;
        let row = conn
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                format!("SELECT i.indisvalid FROM pg_index i WHERE i.indexrelid = to_regclass('public.{INDEX}')"),
            ))
            .await?;
        let valid = match row {
            Some(row) => row.try_get::<bool>("", "indisvalid")?,
            None => false,
        };
        if !valid {
            return Err(DbErr::Migration(format!(
                "m0012: index {INDEX} is INVALID or missing (a failed CREATE INDEX CONCURRENTLY leaves an INVALID index). Run `DROP INDEX {INDEX};`, then run the migration again."
            )));
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(&format!("DROP INDEX IF EXISTS {INDEX};")).await?;
        Ok(())
    }
}
