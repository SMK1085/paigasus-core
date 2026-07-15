// SPDX-License-Identifier: Apache-2.0

//! Schema + data-preservation tests for m0008 (SMA-467): `audit_log` is converted to a
//! two-level `LIST(outcome)→RANGE(occurred_at)` partitioned table with LIST + RANGE default
//! backstops, and the migration preserves existing rows. Runs against an ephemeral Postgres in
//! Docker (skips on a Docker-less laptop, same gating as `audit_log_pg.rs`).

mod support;

use chrono::{Datelike, TimeZone, Utc};
use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::adapters::persistence::entities::audit_log;
use sea_orm::{ActiveModelTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait, PaginatorTrait, Set, Statement};
use sea_orm_migration::MigratorTrait;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

/// `true` iff `audit_log` is a partitioned table (has a row in `pg_partitioned_table`).
async fn audit_log_is_partitioned(db: &impl ConnectionTrait) -> bool {
    let stmt = Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT 1 FROM pg_partitioned_table WHERE partrelid = 'audit_log'::regclass".to_string(),
    );
    db.query_one(stmt).await.unwrap().is_some()
}

/// The physical partition leaf a row with `id` ACTUALLY landed in, via `tableoid`. This is the
/// only way to catch a reversed `FOR VALUES IN` clause or a wrong month boundary: `find_by_id`/
/// `count` transparently scan every leaf and would pass even if a row silently landed in the
/// wrong partition (the gap this closes — SMA-467 Task 1 review finding).
async fn physical_leaf(db: &impl ConnectionTrait, id: Uuid) -> String {
    let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, "SELECT tableoid::regclass::text AS leaf FROM audit_log WHERE id = $1", [id.into()]);
    db.query_one(stmt)
        .await
        .unwrap()
        .expect("row must exist to read its physical leaf")
        .try_get::<String>("", "leaf")
        .unwrap()
}

/// Starts a raw, ephemeral Postgres container WITHOUT running any migrations — mirrors
/// `support::start_migrated_postgres` (same image/tag/CI-gating posture) but stops short of
/// `Migrator::up(&db, None)` so the caller can drive `Migrator::up` step by step. Needed to seed
/// the plain, pre-m0008 `audit_log` shape (m0001..m0007) with historical rows BEFORE m0008 ever
/// runs — `support::start_migrated_postgres` always runs every migration up front, so it can
/// never observe `existing_month_span`'s non-empty (pre-existing-data) branch.
async fn start_raw_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for this test in CI: {e}");
            }
            eprintln!("skipping historical-copy test: Docker unavailable ({e})");
            return None;
        }
    };
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    // Pin the pool to a SINGLE connection so per-session state — notably a `SET TimeZone` issued
    // by `routing_is_correct_under_a_non_utc_session_timezone` — is guaranteed to apply to the
    // same physical connection that the subsequent `Migrator::up` runs on (a default multi-conn
    // pool could migrate on a different, still-UTC session, making that regression test
    // non-deterministic — CodeRabbit SMA-467 round 2).
    let mut opts = ConnectOptions::new(url);
    opts.max_connections(1).min_connections(1);
    let db = Database::connect(opts).await.unwrap();
    Some((node, db))
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
    let (year, month) = (now.year(), now.month());
    let far_future = Utc.with_ymd_and_hms(2999, 1, 1, 0, 0, 0).unwrap();
    row(Uuid::from_u128(1), "committed", now).insert(&db).await.expect("committed insert routes");
    row(Uuid::from_u128(2), "denied", now).insert(&db).await.expect("denied insert routes");
    row(Uuid::from_u128(3), "denied", far_future).insert(&db).await.expect("far-future denied → RANGE default");
    row(Uuid::from_u128(4), "quarantined", now).insert(&db).await.expect("stray outcome → LIST default, must not fail");

    // Physical-routing assertions (SMA-467 Task 1 review finding): `find_by_id`/`count` scan
    // every leaf transparently, so they can't catch a reversed `FOR VALUES IN` clause or a wrong
    // month boundary — only reading back `tableoid` proves a row landed in the leaf it should
    // have, not merely SOME leaf.
    assert_eq!(
        physical_leaf(&db, Uuid::from_u128(1)).await,
        format!("audit_log_committed_{year:04}_{month:02}"),
        "committed row must physically land in its month's committed leaf"
    );
    assert_eq!(
        physical_leaf(&db, Uuid::from_u128(2)).await,
        format!("audit_log_denied_{year:04}_{month:02}"),
        "denied row must physically land in its month's denied leaf (not the committed leaf — catches a reversed LIST clause)"
    );
    assert_eq!(
        physical_leaf(&db, Uuid::from_u128(3)).await,
        "audit_log_denied_default",
        "far-future denied row (no monthly leaf pre-created) must land in the denied RANGE default"
    );
    assert_eq!(
        physical_leaf(&db, Uuid::from_u128(4)).await,
        "audit_log_other",
        "a stray outcome must land in the LIST default `audit_log_other`"
    );

    // find_by_id resolves against the partitioned parent (id is not a partition key).
    let found = audit_log::Entity::find_by_id(Uuid::from_u128(3)).one(&db).await.unwrap();
    assert!(found.is_some(), "find_by_id must resolve a row from a leaf partition");
    assert_eq!(found.unwrap().outcome, "denied");
}

/// `start_migrated_postgres` runs m0008 against an EMPTY `audit_log`, so `existing_month_span`
/// pre-creates leaves only for the container's current month (+1 ahead) — these rows are dated
/// months earlier (relative to the real clock) and therefore have no pre-created monthly leaf.
/// This asserts they land in the denied RANGE default (not silently dropped, and not misrouted
/// into some other leaf) and are still retrievable from there — the multi-month + gap-month +
/// month-end-boundary insert path doesn't ERROR even with no matching leaf. Actual per-month
/// monthly-leaf routing for pre-existing historical data is proven separately by
/// `historical_rows_seeded_before_m0008_survive_the_swap_and_route_to_their_leaf`, which seeds
/// rows before m0008 runs so `existing_month_span`'s non-empty branch spans them.
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
        let id = Uuid::from_u128(100 + i as u128);
        assert!(audit_log::Entity::find_by_id(id).one(&db).await.unwrap().is_some(), "row {i} must be retrievable after routing");
        assert_eq!(
            physical_leaf(&db, id).await,
            "audit_log_denied_default",
            "row {i} predates this test's (empty-table-bootstrap) pre-created leaves, so it must land in the denied RANGE default, not be silently misrouted"
        );
    }
}

/// Regression guard for the bare-date-literal timezone bug (§3.5/D9): under a non-UTC session TZ,
/// a boundary-adjacent row must still route to the correct UTC month. With UTC-pinned bounds this
/// passes; with bare date literals the boundary would shift and this would land the row in an
/// adjacent leaf (or the default).
///
/// Strengthened (SMA-467 CodeRabbit round 1): a prior version set the session TZ AFTER
/// `support::start_migrated_postgres()` had already run m0008 (under the default, UTC session),
/// so it never actually exercised the migration's own leaf-boundary DDL under a non-UTC session —
/// it only proved the row was readable afterward, which ANY leaf (including the RANGE default)
/// would satisfy. This now uses the same stepped-migration harness as
/// `historical_rows_seeded_before_m0008_survive_the_swap_and_route_to_their_leaf`: seed the
/// boundary row into the PLAIN pre-m0008 table, set a non-UTC session TZ, THEN run m0008 — so
/// both `existing_month_span`'s bounds query and the leaf-creating/copy DDL run while the session
/// TZ is non-UTC — and assert the PHYSICAL leaf, not just readability.
#[tokio::test]
async fn routing_is_correct_under_a_non_utc_session_timezone() {
    let Some((_pg, db)) = start_raw_postgres().await else { return };

    // Plain, pre-partition `audit_log` shape (m0001..m0007).
    Migrator::up(&db, Some(7)).await.expect("m0001..m0007 must apply");

    // Seed the boundary-adjacent row directly into the plain table BEFORE m0008 runs (mirrors
    // pre-existing historical data, and drives `existing_month_span`'s leaf pre-creation off this
    // row's actual month rather than the container's wall-clock "current month"). 2026-07-01
    // 02:00:00 UTC is 2026-06-30 22:00 local in New York (UTC-4 in July): a session-TZ-cast
    // boundary would misfile it into June's leaf (or the RANGE default); UTC-pinned bounds must
    // file it in the July leaf.
    let id = Uuid::from_u128(200);
    let ts = Utc.with_ymd_and_hms(2026, 7, 1, 2, 0, 0).unwrap();
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO audit_log (id, occurred_at, action, outcome) VALUES ($1, $2, 'GetProject', $3)",
        [id.into(), ts.into(), "denied".into()],
    );
    db.execute(stmt).await.expect("raw historical insert into the plain (pre-m0008) audit_log");

    // Apply m0008 — the leaf-creating + copy/swap migration — UNDER a non-UTC session. This is
    // the actual regression surface: both `existing_month_span`'s bounds query and the leaf DDL's
    // `FOR VALUES FROM/TO` literals must resolve in UTC, not the session TZ.
    db.execute_unprepared("SET TimeZone = 'America/New_York';").await.unwrap();
    Migrator::up(&db, None).await.expect("m0008 must apply correctly under a non-UTC session TZ");

    assert!(audit_log_is_partitioned(&db).await, "audit_log must be partitioned after m0008");
    assert_eq!(
        physical_leaf(&db, id).await,
        "audit_log_denied_2026_07",
        "boundary row must physically land in the correct UTC month's leaf under a non-UTC session, not be misfiled by a session-TZ-cast boundary"
    );
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

/// Exercises `existing_month_span`'s non-empty branch (previously untested — SMA-467 Task 1
/// review finding gap #2): seeds the PLAIN, pre-m0008 `audit_log` (m0001..m0007) with historical
/// rows spanning MULTIPLE distinct months (with a gap month) BEFORE m0008 ever runs, then applies
/// m0008 and asserts every seeded row (a) survived the copy/swap and (b) physically landed in its
/// own month's leaf — not the RANGE default — via `tableoid`.
///
/// A single `Migrator::up(&db, None)` (as `support::start_migrated_postgres` uses) always runs
/// m0008 against an empty table, so it can never observe this branch: `existing_month_span` reads
/// `min`/`max(occurred_at)` from `audit_log` at the moment m0008 runs, and an empty table always
/// collapses that span to just the current month. Driving `Migrator::up` in two steps — first
/// `Some(7)` (m0001..m0007, the plain m0006 shape), then a raw seed, then `Some(1)` (m0008) —
/// lets `existing_month_span` see the seeded historical span and pre-create leaves for it.
#[tokio::test]
async fn historical_rows_seeded_before_m0008_survive_the_swap_and_route_to_their_leaf() {
    let Some((_pg, db)) = start_raw_postgres().await else { return };

    // Apply m0001..m0007 only — the plain, pre-partition `audit_log` shape (m0006).
    Migrator::up(&db, Some(7)).await.expect("m0001..m0007 must apply");
    assert!(!audit_log_is_partitioned(&db).await, "audit_log must still be the plain m0006 table before m0008 runs");

    // Seed historical rows across multiple distinct months (with a gap month) directly into the
    // plain table via raw INSERT — mirrors real pre-existing audit data written before m0008
    // ever ran. `id`/`occurred_at`/`action`/`outcome` are the plain table's only NOT NULL columns.
    struct Seed {
        id: Uuid,
        outcome: &'static str,
        occurred_at: chrono::DateTime<Utc>,
        expected_leaf: &'static str,
    }
    let seeds = [
        Seed {
            id: Uuid::from_u128(9_001),
            outcome: "committed",
            occurred_at: Utc.with_ymd_and_hms(2026, 1, 10, 8, 0, 0).unwrap(),
            expected_leaf: "audit_log_committed_2026_01",
        },
        Seed {
            id: Uuid::from_u128(9_002),
            outcome: "denied",
            occurred_at: Utc.with_ymd_and_hms(2026, 1, 20, 9, 0, 0).unwrap(),
            expected_leaf: "audit_log_denied_2026_01",
        },
        // gap: no February row
        Seed {
            id: Uuid::from_u128(9_003),
            outcome: "denied",
            occurred_at: Utc.with_ymd_and_hms(2026, 3, 5, 10, 0, 0).unwrap(),
            expected_leaf: "audit_log_denied_2026_03",
        },
        Seed {
            id: Uuid::from_u128(9_004),
            outcome: "committed",
            occurred_at: Utc.with_ymd_and_hms(2026, 3, 28, 11, 0, 0).unwrap(),
            expected_leaf: "audit_log_committed_2026_03",
        },
    ];
    for seed in &seeds {
        let stmt = Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO audit_log (id, occurred_at, action, outcome) VALUES ($1, $2, 'GetProject', $3)",
            [seed.id.into(), seed.occurred_at.into(), seed.outcome.into()],
        );
        db.execute(stmt).await.expect("raw historical insert into the plain (pre-m0008) audit_log");
    }

    // Now apply m0008: the swap must copy every pre-existing row into the new partitioned tree,
    // and `existing_month_span` must pre-create leaves covering their months (Jan/Mar 2026).
    Migrator::up(&db, Some(1)).await.expect("m0008 must apply over pre-existing historical rows");
    assert!(audit_log_is_partitioned(&db).await, "audit_log must be partitioned after m0008");

    let after = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(after, seeds.len() as u64, "every historical row seeded before m0008 must survive the copy/swap");

    for seed in &seeds {
        assert!(
            audit_log::Entity::find_by_id(seed.id).one(&db).await.unwrap().is_some(),
            "historical row {} must be retrievable after the swap",
            seed.id
        );
        assert_eq!(
            physical_leaf(&db, seed.id).await,
            seed.expected_leaf,
            "historical row {} must physically route to its own month's leaf, not the RANGE default",
            seed.id
        );
    }
}
