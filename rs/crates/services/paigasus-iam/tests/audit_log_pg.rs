// SPDX-License-Identifier: Apache-2.0

//! Schema-level test for m0006 (SMA-446 Task A4): asserts the `audit_log` table + its SeaORM
//! entity exist with the expected columns. The `PgAuditLog` port adapter itself is a later
//! task (A5) — this only proves the migration + entity line up, via a trivial insert+count
//! through the entity (a missing/mistyped column would fail the INSERT/SELECT this issues,
//! not just panic on a `.unwrap()` of absent data).
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same
//! gating pattern as `tests/authz_policy_store.rs`/`tests/service_accounts.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::PgAuditLog;
use paigasus_iam::adapters::persistence::entities::audit_log;
use paigasus_iam_core::{AuditEntry, AuditFilter, AuditLog, AuditOutcome, IdGenerator};
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};
use uuid::Uuid;

#[tokio::test]
async fn migration_creates_audit_log_table_and_accepts_a_row() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let before = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(before, 0, "freshly migrated audit_log must start empty");

    let id = KernelIdGenerator.new_audit_id();
    audit_log::ActiveModel {
        id: Set(id),
        occurred_at: Set(Utc::now()),
        actor_prn: Set(Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-000000000001".to_string())),
        action: Set("CreateOrganization".to_string()),
        resource_prn: Set(None),
        outcome: Set("committed".to_string()),
        determining_policies: Set(None),
        detail: Set("{}".to_string()),
        correlation_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let after = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(after, 1, "the inserted row must be counted");
}

/// A denial `AuditEntry` for `actor` at `id`, all sharing the same denied `GetProject`
/// shape — the only axis under test is `id`/`actor_prn`.
fn denial(id: Uuid, actor: &str) -> AuditEntry {
    AuditEntry {
        id,
        occurred_at: Utc::now(),
        actor_prn: Some(actor.to_string()),
        action: "GetProject".to_string(),
        resource_prn: None,
        outcome: AuditOutcome::Denied,
        determining_policies: vec!["policy-forbid-1".to_string()],
        detail: serde_json::json!({"reason": "no matching allow"}),
        correlation_id: None,
    }
}

#[tokio::test]
async fn record_out_of_band_then_query_filters_and_paginates() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sink = PgAuditLog::new(db.clone());

    // Ascending `Uuid::from_u128` ids (byte-big-endian, so larger u128 = larger uuid) so
    // `ORDER BY id DESC` is deterministic: id=1 and id=2 belong to actor "a", id=3 to "b".
    for (id, actor) in [(Uuid::from_u128(1), "a"), (Uuid::from_u128(2), "a"), (Uuid::from_u128(3), "b")] {
        sink.record_out_of_band(&denial(id, actor)).await.expect("record_out_of_band must succeed");
    }

    // Round-trip check on one row: TEXT-column (de)serialization of `determining_policies`/
    // `detail` must survive the write+read, not just the filter/outcome fields.
    let base_filter = AuditFilter {
        actor_prn: Some("a".to_string()),
        resource_prn: None,
        action: None,
        outcome: Some(AuditOutcome::Denied),
        from: None,
        to: None,
        cursor: None,
        limit: 10,
    };
    let rows = sink.query(&base_filter).await.expect("query must succeed");
    assert_eq!(rows.len(), 2, "only actor a's 2 denials must match, newest-first");
    assert_eq!(rows[0].id, Uuid::from_u128(2), "id DESC: the higher id comes first");
    assert_eq!(rows[1].id, Uuid::from_u128(1));
    assert_eq!(rows[0].determining_policies, vec!["policy-forbid-1".to_string()]);
    assert_eq!(rows[0].detail, serde_json::json!({"reason": "no matching allow"}));
    assert_eq!(rows[0].outcome, AuditOutcome::Denied);

    // Keyset pagination: limit=1 returns only the newest match; passing that row's id as the
    // next `cursor` returns the next-newest, not the same row again.
    let page1 = sink.query(&AuditFilter { limit: 1, ..base_filter.clone() }).await.expect("page1 query must succeed");
    assert_eq!(page1.len(), 1);
    assert_eq!(page1[0].id, Uuid::from_u128(2));

    let page2 = sink
        .query(&AuditFilter {
            limit: 1,
            cursor: Some(page1[0].id),
            ..base_filter.clone()
        })
        .await
        .expect("page2 query must succeed");
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].id, Uuid::from_u128(1));

    // Actor b's denial must never surface under the actor=a filter.
    assert!(rows.iter().all(|e| e.actor_prn.as_deref() == Some("a")));
}

/// `query`'s `from`/`to` `occurred_at` filters (both `gte`/`lte`, i.e. inclusive on both
/// ends) must select only the in-range rows, not silently ignore the bounds.
#[tokio::test]
async fn query_filters_by_occurred_at_from_and_to() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sink = PgAuditLog::new(db.clone());

    let base = Utc::now();
    let t0 = base - chrono::Duration::hours(2); // before the range
    let t1 = base - chrono::Duration::hours(1); // range start (`from`)
    let t2 = base; // inside the range
    let t3 = base + chrono::Duration::hours(1); // range end (`to`)
    let t4 = base + chrono::Duration::hours(2); // after the range

    for (id, occurred_at) in [
        (Uuid::from_u128(200), t0),
        (Uuid::from_u128(201), t1),
        (Uuid::from_u128(202), t2),
        (Uuid::from_u128(203), t3),
        (Uuid::from_u128(204), t4),
    ] {
        let entry = AuditEntry {
            occurred_at,
            ..denial(id, "range-actor")
        };
        sink.record_out_of_band(&entry).await.expect("record_out_of_band must succeed");
    }

    let rows = sink
        .query(&AuditFilter {
            actor_prn: Some("range-actor".to_string()),
            resource_prn: None,
            action: None,
            outcome: Some(AuditOutcome::Denied),
            from: Some(t1),
            to: Some(t3),
            cursor: None,
            limit: 10,
        })
        .await
        .expect("range query must succeed");

    let mut ids: Vec<Uuid> = rows.iter().map(|e| e.id).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![Uuid::from_u128(201), Uuid::from_u128(202), Uuid::from_u128(203)],
        "only rows with occurred_at in [from, to] must be returned, both bounds inclusive"
    );
}

/// With a query window configured, a filter supplying NEITHER `from` nor `to` only returns rows
/// inside the default lookback — older rows are pruned out (SMA-467 §3.6).
#[tokio::test]
async fn query_applies_default_lookback_window() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sink = PgAuditLog::new(db.clone()).with_query_window(30, 366); // 30-day default lookback

    let recent = Utc::now() - chrono::Duration::days(1);
    let old = Utc::now() - chrono::Duration::days(120);
    for (id, ts) in [(Uuid::from_u128(900), recent), (Uuid::from_u128(901), old)] {
        let e = AuditEntry {
            occurred_at: ts,
            ..denial(id, "win-actor")
        };
        sink.record_out_of_band(&e).await.unwrap();
    }
    let rows = sink
        .query(&AuditFilter {
            actor_prn: Some("win-actor".to_string()),
            resource_prn: None,
            action: None,
            outcome: Some(AuditOutcome::Denied),
            from: None,
            to: None,
            cursor: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "only the row inside the 30-day default window must return");
    assert_eq!(rows[0].id, Uuid::from_u128(900));
}
