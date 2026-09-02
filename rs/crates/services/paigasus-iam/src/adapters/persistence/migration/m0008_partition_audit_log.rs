// SPDX-License-Identifier: Apache-2.0

//! m0008 — convert `audit_log` from a plain table to a two-level partitioned table (SMA-467).
//!
//! Topology: `audit_log` PARTITION BY LIST (outcome) → `audit_log_committed`/`audit_log_denied`
//! each PARTITION BY RANGE (occurred_at) monthly, plus write-safety defaults at both levels
//! (`audit_log_<outcome>_default` at RANGE, `audit_log_other` at LIST). The denied monthly leaves
//! are the unit an outcome-aware retention job drops (SMA-467 §3.1/D14).
//!
//! **Data-preserving, serialized swap (§4/D5).** Postgres can't ALTER a plain table into a
//! partitioned one, so this creates `audit_log_new`, copies rows, drops the old table, renames,
//! and (re)creates indexes — ordered to avoid the schema-global `ix_audit_log_*` index-name
//! collision. The whole `up`/`down` body runs under `pg_advisory_xact_lock(AUDIT_PARTITION_LOCK_KEY)`
//! and is guarded by an "already partitioned?" check, so a concurrent first-boot across replicas
//! can't double-apply the destructive DROP/RENAME (SeaORM's migrator does not serialize concurrent
//! `up()`; its `seaql_migrations` bookkeeping handles the normal single-apply path — the guard is
//! defense against the simultaneous-first-boot race, whose worst case would otherwise be data loss).
//!
//! **UTC-pinned bounds (§3.5/D9).** All RANGE bounds are fully-qualified `TIMESTAMPTZ '…+00'`
//! literals and every statement runs after `SET LOCAL TimeZone='UTC'`; a bare date literal would be
//! cast in the session TZ and shift every monthly boundary (invisible in the UTC test container).

use chrono::{Datelike, Utc};
use sea_orm_migration::prelude::*;

/// One advisory-lock key namespaces ALL audit-partition DDL (this migration + the maintenance
/// task, Task 4), so the swap and a maintenance tick never run concurrently. Arbitrary fixed
/// constant chosen not to collide with anything else.
pub const AUDIT_PARTITION_LOCK_KEY: i64 = 5_580_467;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// `CREATE TABLE IF NOT EXISTS audit_log_<sub>_YYYY_MM PARTITION OF audit_log_<sub>` for the
/// month containing `year`/`month1` (`month1` is 1-based), with UTC-qualified bounds.
fn month_leaf_ddl(sub: &str, year: i32, month1: u32) -> String {
    let (ny, nm) = if month1 == 12 { (year + 1, 1) } else { (year, month1 + 1) };
    format!(
        "CREATE TABLE IF NOT EXISTS audit_log_{sub}_{year:04}_{month1:02} \
         PARTITION OF audit_log_{sub} \
         FOR VALUES FROM (TIMESTAMPTZ '{year:04}-{month1:02}-01 00:00:00+00') \
         TO (TIMESTAMPTZ '{ny:04}-{nm:02}-01 00:00:00+00');"
    )
}

impl Migration {
    /// The shared column list (order matters for the `INSERT … SELECT`).
    const COLS: &'static str = "id, occurred_at, actor_prn, action, resource_prn, outcome, determining_policies, detail, correlation_id";
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("SET LOCAL TimeZone = 'UTC';").await?;
        db.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        db.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({AUDIT_PARTITION_LOCK_KEY});")).await?;

        // Idempotency guard: if a concurrent replica already swapped, do nothing.
        if is_partitioned(db).await? {
            return Ok(());
        }

        // Determine the month span to pre-create so existing rows don't land in the RANGE default.
        // Empty table → span is just the current month; +1 month of create-ahead either way.
        let (start, end) = existing_month_span(db).await?;

        // 1. Parent + subtrees + LIST default.
        db.execute_unprepared(
            "CREATE TABLE audit_log_new (\
                id uuid NOT NULL, \
                occurred_at timestamptz NOT NULL, \
                actor_prn text, \
                action text NOT NULL, \
                resource_prn text, \
                outcome text NOT NULL, \
                determining_policies text, \
                detail text NOT NULL DEFAULT '{}', \
                correlation_id uuid, \
                CONSTRAINT audit_log_new_pkey PRIMARY KEY (id, occurred_at, outcome)\
             ) PARTITION BY LIST (outcome);",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE audit_log_committed PARTITION OF audit_log_new FOR VALUES IN ('committed') PARTITION BY RANGE (occurred_at);\
             CREATE TABLE audit_log_denied    PARTITION OF audit_log_new FOR VALUES IN ('denied')    PARTITION BY RANGE (occurred_at);\
             CREATE TABLE audit_log_other      PARTITION OF audit_log_new DEFAULT;",
        )
        .await?;

        // 2. Monthly leaves across the existing span (+ current + 1 ahead) for both subtrees, and
        //    the RANGE defaults.
        for (y, m) in months_inclusive(start, end) {
            db.execute_unprepared(&month_leaf_ddl("committed", y, m)).await?;
            db.execute_unprepared(&month_leaf_ddl("denied", y, m)).await?;
        }
        db.execute_unprepared(
            "CREATE TABLE audit_log_committed_default PARTITION OF audit_log_committed DEFAULT;\
             CREATE TABLE audit_log_denied_default    PARTITION OF audit_log_denied    DEFAULT;",
        )
        .await?;

        // 3. Copy rows (explicit column list — never SELECT *), then swap.
        db.execute_unprepared(&format!("INSERT INTO audit_log_new ({cols}) SELECT {cols} FROM audit_log;", cols = Self::COLS))
            .await?;
        db.execute_unprepared("DROP TABLE audit_log;").await?;
        db.execute_unprepared("ALTER TABLE audit_log_new RENAME TO audit_log;").await?;
        db.execute_unprepared("ALTER TABLE audit_log RENAME CONSTRAINT audit_log_new_pkey TO audit_log_pkey;").await?;

        // 4. Indexes on the parent (cascade to every leaf). `outcome` index intentionally omitted
        //    (it's the top partition key — a constant within any leaf).
        db.execute_unprepared(
            "CREATE INDEX ix_audit_log_occurred_at  ON audit_log (occurred_at);\
             CREATE INDEX ix_audit_log_actor_prn    ON audit_log (actor_prn);\
             CREATE INDEX ix_audit_log_resource_prn ON audit_log (resource_prn);\
             CREATE INDEX ix_audit_log_action       ON audit_log (action);",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("SET LOCAL TimeZone = 'UTC';").await?;
        db.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        db.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({AUDIT_PARTITION_LOCK_KEY});")).await?;

        if !is_partitioned(db).await? {
            return Ok(());
        }

        db.execute_unprepared(
            "CREATE TABLE audit_log_plain (\
                id uuid NOT NULL, \
                occurred_at timestamptz NOT NULL, \
                actor_prn text, \
                action text NOT NULL, \
                resource_prn text, \
                outcome text NOT NULL, \
                determining_policies text, \
                detail text NOT NULL DEFAULT '{}', \
                correlation_id uuid, \
                CONSTRAINT audit_log_plain_pkey PRIMARY KEY (id)\
             );",
        )
        .await?;
        // The composite partitioned PK this rolls back FROM is `(id, occurred_at, outcome)` — a
        // partitioned table's unique constraint MUST include the partition key(s), so `id`-alone
        // uniqueness can never be a table constraint on `audit_log` while it's partitioned. That
        // means this plain `SELECT` (no `DISTINCT`, no dedup) is only data-preserving for `id` if
        // `id` is ALREADY globally unique across every row the system wrote — which it is: `id` is
        // an application-minted UUIDv7 (never server-generated, never reused), so every row this
        // system produces already satisfies single-column `id` uniqueness in practice, even though
        // the partitioned schema couldn't enforce it as a constraint. Restoring a plain
        // `PRIMARY KEY (id)` is therefore safe for any state the application produces. A
        // manually-introduced duplicate-`id` row (e.g. hand-crafted via raw SQL, bypassing the
        // application's id minting) is out of scope for this rollback and would (correctly) fail
        // the subsequent `PRIMARY KEY (id)` — do NOT paper over that with `DISTINCT`/
        // `ON CONFLICT DO NOTHING` here, since either would silently drop a row instead of
        // surfacing the underlying data problem.
        db.execute_unprepared(&format!("INSERT INTO audit_log_plain ({cols}) SELECT {cols} FROM audit_log;", cols = Self::COLS))
            .await?;
        db.execute_unprepared("DROP TABLE audit_log;").await?; // cascades the whole tree
        db.execute_unprepared("ALTER TABLE audit_log_plain RENAME TO audit_log;").await?;
        db.execute_unprepared("ALTER TABLE audit_log RENAME CONSTRAINT audit_log_plain_pkey TO audit_log_pkey;").await?;
        db.execute_unprepared(
            "CREATE INDEX ix_audit_log_occurred_at  ON audit_log (occurred_at);\
             CREATE INDEX ix_audit_log_actor_prn    ON audit_log (actor_prn);\
             CREATE INDEX ix_audit_log_resource_prn ON audit_log (resource_prn);\
             CREATE INDEX ix_audit_log_action       ON audit_log (action);\
             CREATE INDEX ix_audit_log_outcome      ON audit_log (outcome);",
        )
        .await?;
        Ok(())
    }
}

async fn is_partitioned(db: &impl sea_orm::ConnectionTrait) -> Result<bool, DbErr> {
    let stmt = sea_orm::Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT 1 FROM pg_partitioned_table WHERE partrelid = 'audit_log'::regclass".to_string(),
    );
    Ok(db.query_one_raw(stmt).await?.is_some())
}

/// `(start, end)` = ((year, month) of `min(occurred_at)`, (year, month) of `max(now, now)+1mo`).
/// Empty table → both default to the current UTC month; end is always ≥ current + 1 month ahead.
async fn existing_month_span(db: &impl sea_orm::ConnectionTrait) -> Result<((i32, u32), (i32, u32)), DbErr> {
    let now = Utc::now();
    let row = db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT min(occurred_at) AS lo, max(occurred_at) AS hi FROM audit_log".to_string(),
        ))
        .await?;
    let (lo, hi) = match row {
        Some(r) => (
            r.try_get::<Option<chrono::DateTime<Utc>>>("", "lo").ok().flatten(),
            r.try_get::<Option<chrono::DateTime<Utc>>>("", "hi").ok().flatten(),
        ),
        None => (None, None),
    };
    let start_dt = lo.unwrap_or(now);
    // end = one month past the later of (max existing row, now).
    let hi_dt = Ord::max(hi.unwrap_or(now), now);
    let end = add_one_month((hi_dt.year(), hi_dt.month()));
    Ok(((start_dt.year(), start_dt.month()), end))
}

fn add_one_month((y, m): (i32, u32)) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

/// Inclusive month iterator from `start` to `end` (both `(year, month1)`).
fn months_inclusive(start: (i32, u32), end: (i32, u32)) -> Vec<(i32, u32)> {
    let mut out = Vec::new();
    let mut cur = start;
    loop {
        out.push(cur);
        if cur == end {
            break;
        }
        cur = add_one_month(cur);
        // Guard against a reversed range (shouldn't happen): stop after a sane bound.
        if out.len() > 1200 {
            break;
        }
    }
    out
}
