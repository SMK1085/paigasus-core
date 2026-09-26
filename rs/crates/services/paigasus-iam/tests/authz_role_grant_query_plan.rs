// SPDX-License-Identifier: Apache-2.0

//! SMA-699: the scope-filtered `RoleGrantQuery::find` and its index
//! (`ix_role_grant_scope_node_prn_principal_id`, m0012). Three parts: m0012's migration round
//! trip; the results of every filter shape against the domain oracle
//! `RoleGrantFilter::matches`; and the plans of the three real query shapes (spec "The real
//! query shapes"), with a custom and a generic plan.

mod support;

use paigasus_iam::adapters::persistence::Migrator;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

const INDEX: &str = "ix_role_grant_scope_node_prn_principal_id";

/// m0012 is the 12th migration. Pinning the `up` count (not counting `down` steps from the tip)
/// keeps `down(Some(1))` equal to m0012's `down` when later migrations land (the
/// `audit_log_partition_pg.rs` precedent).
const M0012: u32 = 12;

/// `None`: no such index. `Some(valid)`: the index exists, with `pg_index.indisvalid`.
async fn index_validity(db: &DatabaseConnection) -> Option<bool> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = '{INDEX}'"),
        ))
        .await
        .unwrap()?;
    Some(row.try_get::<bool>("", "indisvalid").unwrap())
}

#[tokio::test]
async fn m0012_up_down_up_keeps_a_valid_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012)).await.expect("m0001..m0012 must apply");
    assert_eq!(index_validity(&db).await, Some(true), "after up");

    Migrator::down(&db, Some(1)).await.expect("m0012 down must succeed");
    assert_eq!(index_validity(&db).await, None, "after down");

    Migrator::up(&db, Some(1)).await.expect("m0012 must apply again");
    assert_eq!(index_validity(&db).await, Some(true), "after the second up");
}

/// E5: `CREATE INDEX IF NOT EXISTS` skips an INVALID index with the same name (what a failed
/// out-of-band `CREATE INDEX CONCURRENTLY` leaves), and the planner ignores an INVALID index.
/// m0012 must fail with an operator message, not pass.
#[tokio::test]
async fn m0012_refuses_an_invalid_leftover_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012 - 1)).await.expect("m0001..m0011 must apply");
    db.execute_unprepared(&format!(r#"CREATE INDEX {INDEX} ON "role_grant" (scope_node_prn, principal_id, id);"#))
        .await
        .unwrap();
    db.execute_unprepared(&format!("UPDATE pg_index SET indisvalid = false WHERE indexrelid = '{INDEX}'::regclass;"))
        .await
        .unwrap();
    assert_eq!(index_validity(&db).await, Some(false), "sanity: the leftover index is INVALID");

    let err = Migrator::up(&db, Some(1)).await.expect_err("m0012 must refuse an INVALID index");
    let msg = err.to_string();
    assert!(msg.contains(INDEX) && msg.contains("INVALID"), "the error must name the index and the cause: {msg}");
}
