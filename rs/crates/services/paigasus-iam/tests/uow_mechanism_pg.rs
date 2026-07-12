// SPDX-License-Identifier: Apache-2.0

//! De-risking spike for SMA-446 Slice B: proves the opaque `UnitOfWork`/`Transaction`/
//! `Savepoint` port mechanism (`as_any().downcast_ref` recovery + a SeaORM nested transaction
//! as a Postgres `SAVEPOINT`) works end-to-end against real Postgres, BEFORE B2/B4 build the
//! txn-scoped stores on top of it.
//!
//! The txn-scoped store methods (`grant_in`, `Outbox::enqueue`, `AuditLog::record`) do not exist
//! yet (they land in B2/B4), so this exercises the mechanism with RAW SeaORM inserts on the
//! downcast-recovered `DatabaseTransaction`, against the already-migrated `audit_log` table
//! (Slice A). Three scenarios: commit atomicity, rollback-on-drop atomicity, and savepoint
//! isolation.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — same gating
//! pattern as `tests/audit_log_pg.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::persistence::SeaOrmUnitOfWork;
use paigasus_iam::adapters::persistence::entities::audit_log;
use paigasus_iam::adapters::persistence::uow::{recover_savepoint_txn, recover_txn};
use paigasus_iam_core::UnitOfWork;
use sea_orm::{ActiveModelTrait, DatabaseTransaction, DbErr, EntityTrait, PaginatorTrait, QueryOrder, Set};
use uuid::Uuid;

/// Inserts one minimal `audit_log` row with the given primary key on `conn` (a
/// downcast-recovered transaction/savepoint). Returns the `DbErr` unchanged so a scenario can
/// assert on a forced unique violation.
async fn insert_row(conn: &DatabaseTransaction, id: Uuid) -> Result<audit_log::Model, DbErr> {
    audit_log::ActiveModel {
        id: Set(id),
        occurred_at: Set(Utc::now()),
        actor_prn: Set(None),
        action: Set("UowSpike".to_string()),
        resource_prn: Set(None),
        outcome: Set("committed".to_string()),
        determining_policies: Set(None),
        detail: Set("{}".to_string()),
        correlation_id: Set(None),
    }
    .insert(conn)
    .await
}

/// Every committed `audit_log` id, ascending — the fresh-connection view of what durably landed.
async fn committed_ids(db: &sea_orm::DatabaseConnection) -> Vec<Uuid> {
    audit_log::Entity::find().order_by_asc(audit_log::Column::Id).all(db).await.unwrap().into_iter().map(|m| m.id).collect()
}

/// Scenario 1 — commit atomicity: two inserts on the recovered transaction, then `commit`, and
/// both rows are durably present when read back on a fresh connection.
#[tokio::test]
async fn commit_makes_all_txn_writes_visible() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let tx = uow.begin().await.expect("begin");
    {
        let txn = recover_txn(&*tx).expect("downcast to SeaOrmTransaction");
        insert_row(txn, Uuid::from_u128(1)).await.expect("insert row 1");
        insert_row(txn, Uuid::from_u128(2)).await.expect("insert row 2");
    }
    tx.commit().await.expect("commit");

    let count = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(count, 2, "both rows written in the committed txn must be visible");
}

/// Scenario 2 — rollback atomicity: one insert on the recovered transaction, then the
/// `Box<dyn Transaction>` is dropped WITHOUT commit (SeaORM rolls a `DatabaseTransaction` back on
/// drop; MVCC also keeps the uncommitted row invisible to any other connection), so nothing
/// lands.
#[tokio::test]
async fn dropping_without_commit_rolls_everything_back() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let tx = uow.begin().await.expect("begin");
    {
        let txn = recover_txn(&*tx).expect("downcast to SeaOrmTransaction");
        insert_row(txn, Uuid::from_u128(1)).await.expect("insert row 1");
    }
    drop(tx); // no commit -> rollback

    let count = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(count, 0, "an uncommitted txn's write must never be visible");
}

/// Scenario 3 — savepoint isolation: insert A on the outer txn, open a savepoint, force a unique
/// violation inside it (duplicate PK), roll the savepoint back, then insert C on the outer txn
/// and commit. A and C persist; the failed duplicate does not — proving the failed statement
/// aborted only the savepoint, not the outer transaction.
#[tokio::test]
async fn savepoint_rollback_isolates_a_failed_write_from_the_outer_txn() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let a = Uuid::from_u128(1);
    let c = Uuid::from_u128(3);

    let mut tx = uow.begin().await.expect("begin");

    // Insert A on the outer transaction.
    {
        let outer = recover_txn(&*tx).expect("downcast outer txn");
        insert_row(outer, a).await.expect("insert row A on outer txn");
    }

    // Open a savepoint and force a unique violation inside it (duplicate PK == A). The failed
    // statement aborts the savepoint's subtransaction, not the outer one.
    let sp = tx.savepoint().await.expect("open savepoint");
    {
        let inner = recover_savepoint_txn(&*sp).expect("downcast savepoint txn");
        let dup = insert_row(inner, a).await;
        assert!(dup.is_err(), "a duplicate primary key inside the savepoint must fail");
    }
    sp.rollback().await.expect("roll the savepoint back"); // ROLLBACK TO SAVEPOINT; frees the &mut tx borrow

    // The outer txn is still usable: insert C and commit.
    {
        let outer = recover_txn(&*tx).expect("downcast outer txn after savepoint rollback");
        insert_row(outer, c).await.expect("insert row C on outer txn");
    }
    tx.commit().await.expect("commit");

    assert_eq!(committed_ids(&db).await, vec![a, c], "A and C persist; the savepoint-local duplicate does not");
}
