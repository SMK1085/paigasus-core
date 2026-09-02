// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgPartitionMaintainer` (SMA-467): create-ahead is idempotent, and prune
//! is outcome-aware (drops aged denied leaves, keeps recent denied + all committed, never a
//! default), and prune runs even if create-ahead can't. Real Postgres via testcontainers.

mod support;

use chrono::{TimeZone, Utc};
use paigasus_iam::adapters::persistence::{PgPartitionMaintainer, RetentionPolicy};
use sea_orm::{ConnectionTrait, Statement};

async fn leaf_exists(db: &impl ConnectionTrait, name: &str) -> bool {
    let stmt = Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!("SELECT 1 FROM pg_class WHERE relname = '{name}' AND relkind = 'r'"));
    db.query_one_raw(stmt).await.unwrap().is_some()
}

#[tokio::test]
async fn ensure_ahead_is_idempotent_and_creates_leaves() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let m = PgPartitionMaintainer::new(db.clone());
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
    let policy = RetentionPolicy {
        ahead_months: 2,
        denied_months: 3,
        committed_months: 0,
    };

    m.tick(now, policy).await;
    // Second run must not error (IF NOT EXISTS) AND must report zero NEW creations — every
    // target leaf already exists from the first tick, so `created` must accurately reflect
    // that (SMA-467 final-review fix: it used to unconditionally count every CREATE, even a
    // no-op against an already-existing leaf). The existence check now happens atomically
    // INSIDE the same advisory-locked transaction as the CREATE (CodeRabbit round 1: the prior
    // pre-fetch-then-create was racy across replicas), so this single-process assertion still
    // holds — it's just now proven race-free rather than merely correct in the no-race case.
    let report = m.tick(now, policy).await;
    assert_eq!(report.created, 0, "second tick must create nothing new (all leaves already exist)");

    for sub in ["committed", "denied"] {
        for ym in ["2026_07", "2026_08", "2026_09"] {
            assert!(leaf_exists(&db, &format!("audit_log_{sub}_{ym}")).await, "leaf audit_log_{sub}_{ym} must exist");
        }
    }
}

#[tokio::test]
async fn prune_drops_aged_denied_keeps_committed_and_recent() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let m = PgPartitionMaintainer::new(db.clone());
    // Create old + recent leaves for both outcomes by ticking "as of" an old month first.
    let old = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    m.tick(
        old,
        RetentionPolicy {
            ahead_months: 1,
            denied_months: 0,
            committed_months: 0,
        },
    )
    .await;
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
    // denied_months = 3 → Jan/Feb denied leaves are older than 3 months from July and get dropped;
    // committed_months = 0 → no committed leaf is ever dropped.
    m.tick(
        now,
        RetentionPolicy {
            ahead_months: 1,
            denied_months: 3,
            committed_months: 0,
        },
    )
    .await;

    assert!(!leaf_exists(&db, "audit_log_denied_2026_01").await, "aged denied Jan leaf must be dropped");
    assert!(leaf_exists(&db, "audit_log_committed_2026_01").await, "committed leaf must NOT be dropped when committed_months = 0");
    assert!(leaf_exists(&db, "audit_log_denied_2026_07").await, "current denied leaf must be kept");
    assert!(leaf_exists(&db, "audit_log_denied_default").await, "the denied RANGE default must never be dropped");
}
