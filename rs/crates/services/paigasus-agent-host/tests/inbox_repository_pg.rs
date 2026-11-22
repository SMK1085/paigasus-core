// SPDX-License-Identifier: Apache-2.0

//! The Postgres inbox adapter against a real database (spec §7.2): the lookup of an inbox, the
//! insert of an event, the dedup on the key and the comparison of a duplicate payload.

mod support;

use paigasus_agent_host::adapters::persistence::inbox_repository::{PgInboxRepository, insert_event_in};
use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, ThreadId};
use paigasus_agent_host_core::inbox::NewInboxEvent;
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, RepositoryError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::{Value, json};
use support::inbox::{insert_inbox, migrated_postgres};
use uuid::Uuid;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// The inbox of org 1 with the UUID `inbox`.
fn inbox_id(inbox: u128) -> InboxId {
    InboxId::new(u(1), u(inbox))
}

fn new_event(event: u128, inbox: u128, ce_id: &str, ce_type: &str, envelope: Value, thread: Option<ThreadId>) -> NewInboxEvent {
    NewInboxEvent {
        id: EventId::from_uuid(u(event)),
        inbox: inbox_id(inbox),
        source: "urn:test".to_string(),
        ce_id: ce_id.to_string(),
        ce_type: ce_type.to_string(),
        thread,
        envelope,
    }
}

async fn insert_thread(db: &DatabaseConnection, thread: Uuid, inbox: Uuid) {
    db.execute_unprepared(&format!("INSERT INTO thread (id, inbox_id) VALUES ('{thread}', '{inbox}')"))
        .await
        .expect("insert thread");
}

async fn one_row(db: &DatabaseConnection, sql: &str) -> sea_orm::QueryResult {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("a row")
}

#[tokio::test]
async fn get_by_uuid_returns_the_inbox_with_its_org() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    let inbox = repo.get_by_uuid(u(2)).await.expect("get").expect("some");
    assert_eq!(inbox.org(), u(1));
    assert_eq!(inbox.id(), &inbox_id(2));
    assert_eq!(inbox.agent(), &AgentId::new(u(1), u(3)));
    assert_eq!(inbox.principal().uuid(), u(4));
}

#[tokio::test]
async fn get_by_uuid_of_an_unknown_inbox_is_none() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let repo = PgInboxRepository::new(db);
    assert!(repo.get_by_uuid(u(99)).await.expect("get").is_none());
}

#[tokio::test]
async fn an_insert_returns_the_new_id_and_the_row_is_pending() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db.clone());
    let outcome = repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("insert");
    assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(u(10)) });
    let row = one_row(&db, "SELECT status, attempts, (claimed_run_id IS NULL) AS no_run FROM inbox_event WHERE ce_id = 'c1'").await;
    assert_eq!(row.try_get::<String>("", "status").expect("status"), "pending");
    assert_eq!(row.try_get::<i32>("", "attempts").expect("attempts"), 0);
    assert!(row.try_get::<bool>("", "no_run").expect("no_run"));
}

#[tokio::test]
async fn a_duplicate_returns_the_stored_id_and_writes_no_row() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db.clone());
    repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("first");
    let outcome = repo.insert_event(new_event(11, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: true
        }
    );
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM inbox_event").await, 1);
}

#[tokio::test]
async fn a_duplicate_with_other_data_reports_a_mismatch() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("first");
    let outcome = repo.insert_event(new_event(11, 2, "c1", "agents.x", json!({"data": {"a": 2}}), None)).await.expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: false
        }
    );
}

#[tokio::test]
async fn a_duplicate_with_another_type_reports_a_mismatch() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("first");
    let outcome = repo.insert_event(new_event(11, 2, "c1", "agents.y", json!({"data": {"a": 1}}), None)).await.expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: false
        }
    );
}

// Postgres `jsonb` stores a number as a numeric: `1e18` and `1000000000000000000` are one
// value. `serde_json` keeps the written form. The compare must treat them as equal.
#[tokio::test]
async fn numerically_equal_numbers_in_another_form_match() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    let first: Value = serde_json::from_str(r#"{"data": {"n": 1000000000000000000, "m": 1.50, "k": [2.0]}}"#).expect("json");
    let second: Value = serde_json::from_str(r#"{"data": {"n": 1e18, "m": 1.5, "k": [2]}}"#).expect("json");
    repo.insert_event(new_event(10, 2, "c1", "agents.x", first, None)).await.expect("first");
    let outcome = repo.insert_event(new_event(11, 2, "c1", "agents.x", second, None)).await.expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: true
        }
    );
}

// Two integers that differ only below the f64 precision are different values.
#[tokio::test]
async fn large_integers_that_differ_beyond_f64_precision_do_not_match() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    let first: Value = serde_json::from_str(r#"{"data": {"n": 9007199254740993}}"#).expect("json");
    let second: Value = serde_json::from_str(r#"{"data": {"n": 9007199254740992}}"#).expect("json");
    repo.insert_event(new_event(10, 2, "c1", "agents.x", first, None)).await.expect("first");
    let outcome = repo.insert_event(new_event(11, 2, "c1", "agents.x", second, None)).await.expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: false
        }
    );
}

#[tokio::test]
async fn absent_and_null_data_match() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db);
    repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({"type": "agents.x"}), None)).await.expect("first");
    let outcome = repo
        .insert_event(new_event(11, 2, "c1", "agents.x", json!({"type": "agents.x", "data": null}), None))
        .await
        .expect("second");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: true
        }
    );
}

#[tokio::test]
async fn a_thread_of_another_inbox_is_stored_as_null() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    insert_inbox(&db, u(1), u(5), u(6), u(7)).await;
    insert_thread(&db, u(500), u(5)).await;
    let repo = PgInboxRepository::new(db.clone());
    let outcome = repo
        .insert_event(new_event(10, 2, "c1", "agents.x", json!({}), Some(ThreadId::from_uuid(u(500)))))
        .await
        .expect("insert");
    assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(u(10)) });
    let row = one_row(&db, "SELECT (thread_id IS NULL) AS v FROM inbox_event WHERE ce_id = 'c1'").await;
    assert!(row.try_get::<bool>("", "v").expect("v"));
}

#[tokio::test]
async fn a_thread_of_this_inbox_is_kept() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    insert_thread(&db, u(500), u(2)).await;
    let repo = PgInboxRepository::new(db.clone());
    repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({}), Some(ThreadId::from_uuid(u(500)))))
        .await
        .expect("insert");
    let row = one_row(&db, "SELECT thread_id FROM inbox_event WHERE ce_id = 'c1'").await;
    assert_eq!(row.try_get::<Uuid>("", "thread_id").expect("thread_id"), u(500));
}

#[tokio::test]
async fn a_store_error_is_a_repository_store_error() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let repo = PgInboxRepository::new(db.clone());
    db.close().await.expect("close");
    let err = repo.insert_event(new_event(10, 2, "c1", "agents.x", json!({}), None)).await.expect_err("closed pool");
    assert!(matches!(err, RepositoryError::Store(_)), "got {err:?}");
}

#[tokio::test]
async fn insert_in_a_transaction_rolls_back_with_it() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let txn = db.begin().await.expect("begin");
    let outcome = insert_event_in(&txn, &new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("insert");
    assert_eq!(outcome, InsertOutcome::Inserted { id: EventId::from_uuid(u(10)) });
    txn.rollback().await.expect("rollback");
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM inbox_event").await, 0);

    let txn = db.begin().await.expect("begin");
    insert_event_in(&txn, &new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("insert");
    txn.commit().await.expect("commit");
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM inbox_event").await, 1);
}

#[tokio::test]
async fn insert_in_a_transaction_sees_a_duplicate() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let txn = db.begin().await.expect("begin");
    insert_event_in(&txn, &new_event(10, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("first");
    txn.commit().await.expect("commit");
    let txn = db.begin().await.expect("begin");
    let outcome = insert_event_in(&txn, &new_event(11, 2, "c1", "agents.x", json!({"data": {"a": 1}}), None)).await.expect("second");
    txn.rollback().await.expect("rollback");
    assert_eq!(
        outcome,
        InsertOutcome::Duplicate {
            id: EventId::from_uuid(u(10)),
            payload_matches: true
        }
    );
}
