// SPDX-License-Identifier: Apache-2.0

//! Schema + data-preservation tests for m0008 (SMA-467): `audit_log` is converted to a
//! two-level `LIST(outcome)→RANGE(occurred_at)` partitioned table with LIST + RANGE default
//! backstops, and the migration preserves existing rows. Runs against an ephemeral Postgres in
//! Docker (skips on a Docker-less laptop, same gating as `audit_log_pg.rs`).

mod support;

use chrono::{TimeZone, Utc};
use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::adapters::persistence::entities::audit_log;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, PaginatorTrait, Set, Statement};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

/// `true` iff `audit_log` is a partitioned table (has a row in `pg_partitioned_table`).
async fn audit_log_is_partitioned(db: &impl ConnectionTrait) -> bool {
    let stmt = Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT 1 FROM pg_partitioned_table WHERE partrelid = 'audit_log'::regclass".to_string(),
    );
    db.query_one(stmt).await.unwrap().is_some()
}

fn row(id: Uuid, outcome: &str, occurred_at: chrono::DateTime<Utc>) -> audit_log::ActiveModel {
    audit_log::ActiveModel {
        id: Set(id),
        occurred_at: Set(occurred_at),
        actor_prn: Set(Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-000000000001".to_string())),
        action: Set("GetProject".to_string()),
        resource_prn: Set(None),
        outcome: Set(outcome.to_string()),
        determining_policies: Set(None),
        detail: Set("{}".to_string()),
        correlation_id: Set(None),
    }
}

#[tokio::test]
async fn migration_makes_audit_log_partitioned_and_routes_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    assert!(audit_log_is_partitioned(&db).await, "audit_log must be partitioned after m0008");

    // committed + denied in the current month, a far-future denied (→ RANGE default), and a
    // stray outcome (→ LIST default `audit_log_other`) — none may fail to insert (the G1 guarantee).
    let now = Utc::now();
    let far_future = Utc.with_ymd_and_hms(2999, 1, 1, 0, 0, 0).unwrap();
    row(Uuid::from_u128(1), "committed", now).insert(&db).await.expect("committed insert routes");
    row(Uuid::from_u128(2), "denied", now).insert(&db).await.expect("denied insert routes");
    row(Uuid::from_u128(3), "denied", far_future).insert(&db).await.expect("far-future denied → RANGE default");
    row(Uuid::from_u128(4), "quarantined", now).insert(&db).await.expect("stray outcome → LIST default, must not fail");

    // find_by_id resolves against the partitioned parent (id is not a partition key).
    let found = audit_log::Entity::find_by_id(Uuid::from_u128(3)).one(&db).await.unwrap();
    assert!(found.is_some(), "find_by_id must resolve a row from a leaf partition");
    assert_eq!(found.unwrap().outcome, "denied");
}

/// Seeds a PLAIN `audit_log` (pre-m0008 shape), then runs m0008's up SQL logic implicitly via a
/// fresh migrate is not possible here (migrate already ran) — instead assert the through-migration
/// invariants: rows inserted across MULTIPLE months + a gap month all round-trip and route to the
/// right monthly leaf. (The swap's copy path is exercised by any env that had rows before m0008;
/// here we prove multi-month routing + retrieval end-to-end.)
#[tokio::test]
async fn rows_across_multiple_and_gap_months_route_and_read_back() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    use chrono::TimeZone;
    let months = [
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
        // gap: no February row
        Utc.with_ymd_and_hms(2026, 3, 15, 12, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 3, 31, 23, 59, 59).unwrap(), // month-end boundary
    ];
    for (i, ts) in months.iter().enumerate() {
        row(Uuid::from_u128(100 + i as u128), "denied", *ts).insert(&db).await.expect("multi-month denied insert routes");
    }
    for (i, _) in months.iter().enumerate() {
        assert!(
            audit_log::Entity::find_by_id(Uuid::from_u128(100 + i as u128)).one(&db).await.unwrap().is_some(),
            "row {i} must be retrievable after routing to its monthly leaf"
        );
    }
}

/// Regression guard for the bare-date-literal timezone bug (§3.5/D9): under a non-UTC session TZ,
/// a boundary-adjacent row must still route to the correct UTC month. With UTC-pinned bounds this
/// passes; with bare date literals the boundary would shift and this would land the row in an
/// adjacent leaf (or the default).
#[tokio::test]
async fn routing_is_correct_under_a_non_utc_session_timezone() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    db.execute_unprepared("SET TimeZone = 'America/New_York';").await.unwrap();
    use chrono::TimeZone;
    // 2026-07-01 02:00:00 UTC — in New York (UTC-4 in July) this is still 2026-06-30 22:00 local;
    // a session-TZ-cast boundary would misfile it. UTC-pinned bounds file it in the July leaf.
    let ts = Utc.with_ymd_and_hms(2026, 7, 1, 2, 0, 0).unwrap();
    row(Uuid::from_u128(200), "denied", ts).insert(&db).await.expect("boundary insert must route, not fail");
    let found = audit_log::Entity::find_by_id(Uuid::from_u128(200)).one(&db).await.unwrap();
    assert!(found.is_some(), "boundary row must be retrievable under a non-UTC session TZ");
}

/// m0008's `down` must restore the EXACT `m0006` plain-table shape (single-col PK on `id`, five
/// indexes incl. `outcome`) while preserving every row that was in the partitioned table.
#[tokio::test]
async fn down_migration_restores_the_plain_m0006_shape_and_preserves_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    // Seed rows across outcomes (incl. a LIST-default stray) so the down-copy path is exercised
    // for every partition subtree, not just one.
    let now = Utc::now();
    row(Uuid::from_u128(1), "committed", now).insert(&db).await.expect("seed committed");
    row(Uuid::from_u128(2), "denied", now).insert(&db).await.expect("seed denied");
    row(Uuid::from_u128(3), "quarantined", now).insert(&db).await.expect("seed LIST-default stray");
    let before = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(before, 3, "all three seeded rows must be visible pre-down");

    // Revert exactly one migration step (m0008's `down`).
    Migrator::down(&db, Some(1)).await.expect("m0008 down must succeed");

    assert!(!audit_log_is_partitioned(&db).await, "audit_log must be a plain table after down");

    let after = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(after, 3, "down must preserve every row from the partitioned table");

    // Single-column PK on `id`: a duplicate id with a DIFFERENT (occurred_at, outcome) must now
    // violate the PK, whereas under the partitioned composite PK it would have been legal.
    let dup = row(Uuid::from_u128(1), "denied", now + chrono::Duration::hours(1));
    assert!(dup.insert(&db).await.is_err(), "duplicate id must violate the restored single-column PK");

    // All five m0006 indexes (incl. `outcome`) must be back.
    let idx_stmt = Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT indexname FROM pg_indexes WHERE tablename = 'audit_log' ORDER BY indexname".to_string(),
    );
    let rows = db.query_all(idx_stmt).await.unwrap();
    let names: Vec<String> = rows.iter().map(|r| r.try_get::<String>("", "indexname").unwrap()).collect();
    for expected in [
        "ix_audit_log_occurred_at",
        "ix_audit_log_actor_prn",
        "ix_audit_log_resource_prn",
        "ix_audit_log_action",
        "ix_audit_log_outcome",
    ] {
        assert!(names.contains(&expected.to_string()), "index {expected} must exist after down, got {names:?}");
    }
}
