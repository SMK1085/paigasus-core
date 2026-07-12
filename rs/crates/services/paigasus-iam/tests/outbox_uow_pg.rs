// SPDX-License-Identifier: Apache-2.0

//! Integration test for Task B2 (SMA-446, Slice B): `PgOutbox::enqueue` + `PgAuditLog::record`
//! driven through the real B3 `SeaOrmUnitOfWork`, against real Postgres. Proves the atomicity
//! the transactional-outbox pattern exists for: an `event_outbox` row and an `audit_log` row
//! written on the SAME transaction either BOTH land (commit) or NEITHER lands (drop without
//! commit) — exactly `tests/uow_mechanism_pg.rs`'s scenarios 1/2, but through the real B2
//! adapters instead of raw SeaORM inserts.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — same
//! gating pattern as `tests/audit_log_pg.rs`/`tests/uow_mechanism_pg.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam::adapters::persistence::uow::recover_txn;
use paigasus_iam::adapters::persistence::{PgAuditLog, PgOutbox, SeaOrmUnitOfWork};
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, DomainEvent, EventType, IdGenerator, Outbox, UnitOfWork};
use sea_orm::{EntityTrait, PaginatorTrait};

/// One minimal `DomainEvent`, distinct correlation id per call so callers can assert on it if
/// needed — mirrors `tests/audit_log_pg.rs`'s `denial` builder's role for `AuditEntry`.
fn event() -> DomainEvent {
    DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::PrincipalCreated,
        schema_version: 1,
        aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
        actor_prn: Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb".to_string()),
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: Some(KernelIdGenerator.new_correlation_id()),
    }
}

/// One minimal committed `AuditEntry` (mirrors `tests/audit_log_pg.rs::denial`, but a
/// `Committed` outcome — this test is about a successful in-txn mutation, not a denial).
fn entry() -> AuditEntry {
    AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: Utc::now(),
        actor_prn: Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb".to_string()),
        action: "CreatePrincipal".to_string(),
        resource_prn: Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: None,
    }
}

/// Scenario 1 — commit atomicity: `PgOutbox::enqueue` + `PgAuditLog::record` on the same
/// transaction, then `commit`; both rows are durably present on a fresh read.
#[tokio::test]
async fn commit_makes_both_outbox_and_audit_rows_visible() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new();
    let audit = PgAuditLog::new(db.clone());

    let ev = event();
    let ae = entry();

    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &ev).await.expect("enqueue outbox row");
    audit.record(&*tx, &ae).await.expect("record audit row");
    tx.commit().await.expect("commit");

    let outbox_count = event_outbox::Entity::find().count(&db).await.unwrap();
    let audit_count = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(outbox_count, 1, "the committed outbox row must be visible");
    assert_eq!(audit_count, 1, "the committed audit row must be visible");

    let outbox_row = event_outbox::Entity::find_by_id(ev.id).one(&db).await.unwrap().expect("outbox row present");
    assert_eq!(outbox_row.event_type, EventType::PrincipalCreated.as_wire());
    assert_eq!(outbox_row.aggregate_prn, ev.aggregate_prn);
    assert_eq!(outbox_row.payload, ev.payload.to_string());
    assert!(outbox_row.published_at.is_none(), "a freshly enqueued row is unpublished");
    assert!(!outbox_row.parked);
    assert_eq!(outbox_row.attempts, 0);

    let audit_row = audit_log::Entity::find_by_id(ae.id).one(&db).await.unwrap().expect("audit row present");
    assert_eq!(audit_row.action, "CreatePrincipal");
    assert_eq!(audit_row.outcome, AuditOutcome::Committed.as_str());
}

/// Scenario 2 — rollback atomicity: the same two writes, but the `Box<dyn Transaction>` is
/// dropped WITHOUT commit — neither row is ever visible (SeaORM rolls the `DatabaseTransaction`
/// back on drop; MVCC also keeps the uncommitted rows invisible to any other connection).
#[tokio::test]
async fn dropping_without_commit_rolls_back_both_outbox_and_audit_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new();
    let audit = PgAuditLog::new(db.clone());

    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &event()).await.expect("enqueue outbox row");
    audit.record(&*tx, &entry()).await.expect("record audit row");
    drop(tx); // no commit -> rollback

    let outbox_count = event_outbox::Entity::find().count(&db).await.unwrap();
    let audit_count = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(outbox_count, 0, "an uncommitted txn's outbox write must never be visible");
    assert_eq!(audit_count, 0, "an uncommitted txn's audit write must never be visible");
}

/// Sanity check: in-txn `AuditLog::record` and out-of-band `AuditLog::record_out_of_band` don't
/// interfere — a `record_out_of_band` call durably lands even though it runs interleaved with
/// an as-yet-uncommitted `record` on an unrelated transaction (they write through different
/// connections: `record_out_of_band` autocommits on `&self.db`, `record` runs on the caller's
/// txn recovered via [`recover_txn`]).
#[tokio::test]
async fn record_in_txn_and_record_out_of_band_do_not_interfere() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let audit = PgAuditLog::new(db.clone());

    let tx = uow.begin().await.expect("begin");
    let in_txn_entry = entry();
    audit.record(&*tx, &in_txn_entry).await.expect("record audit row in-txn");

    // Recovering the SAME transaction a second time still finds it live (not yet committed).
    assert!(recover_txn(&*tx).is_ok());

    let out_of_band_entry = entry();
    audit.record_out_of_band(&out_of_band_entry).await.expect("record_out_of_band on an unrelated connection");

    // The out-of-band row is visible immediately (autocommit); the in-txn row is not, until
    // its own transaction commits.
    assert!(audit_log::Entity::find_by_id(out_of_band_entry.id).one(&db).await.unwrap().is_some());
    assert!(audit_log::Entity::find_by_id(in_txn_entry.id).one(&db).await.unwrap().is_none());

    tx.commit().await.expect("commit");
    assert!(audit_log::Entity::find_by_id(in_txn_entry.id).one(&db).await.unwrap().is_some());
}
