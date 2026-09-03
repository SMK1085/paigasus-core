# audit_log Time-Partitioning + Outcome-Aware Retention Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert IAM's `audit_log` into a two-level partitioned table (`LIST(outcome)→RANGE(occurred_at)` monthly) and add an in-app maintenance task that creates month partitions ahead and drops aged-out denial partitions, so denial rows age out while committed rows are retained.

**Architecture:** A data-preserving SeaORM migration (`m0008`) swaps the plain table for the partitioned tree (advisory-lock-serialized, UTC-pinned bounds, LIST + RANGE default backstops). A `PgPartitionMaintainer` persistence adapter runs create-ahead + outcome-aware pruning on a background task spawned in `main.rs`, mirroring the existing `OutboxRelay`. Config `[audit.retention]` drives it; new metrics + a RUNBOOK/alert update make it observable.

**Tech Stack:** Rust (edition 2024, rust 1.95), SeaORM 1.1 migrations + raw `execute_unprepared` DDL, tokio background task, `metrics` facade + `paigasus-observability` name registry, figment config, Postgres 14+ (tested on 16 via testcontainers), Prometheus/promtool.

## Global Constraints

- **SPDX header** on every new source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML/TOML where used).
- **Rust edition 2024 + rust-version 1.95** (workspace-inherited; no per-crate change).
- **No new crate, no new Cargo dependency** — `sea-orm`, `chrono`, `tokio`, `metrics`, `paigasus-observability` are already deps of `paigasus-iam`. (Keeps `:affected-smoke`/`:deny`/`:machete` untouched.)
- **Postgres floor: 14** (features used exist from 11; 14 is the documented production floor). Tests run against `postgres:16-alpine` via `tests/support`.
- **All partition-bound literals are fully-qualified UTC `timestamptz`** (`TIMESTAMPTZ 'YYYY-MM-01 00:00:00+00'`), never bare date strings; every migration/maintenance transaction issues `SET LOCAL TimeZone = 'UTC'`.
- **Metric names** are `const`s in `paigasus_observability::names` and must be added to `names::ALL` (drift test).
- **Commits:** Conventional Commits, subject lowercase after the type/scope, ≤100 chars, workspace scope `feat(rs):` / `test(rs):` / `docs(rs):`. No `#NNN` in the body. End every commit body with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Worktree:** all work happens in the SMA-467 worktree (`.claude/worktrees/sma-467-audit-log-partitioning`). A subagent's FIRST action must be `EnterWorktree { path: "<worktree>" }` then confirm the branch is `feature/sma-467-…` (pinned-cwd hazard). Provision deps once before the first `cargo`/commit: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` then `proto install`, `pnpm -C ts install`, `cargo fetch` in `rs/` (commitlint is already installed).
- **Run rust commands from `rs/`** with the proto PATH exported. Integration tests need Docker (they self-skip if absent locally; CI is authoritative).

---

### Task 1: Migration `m0008_partition_audit_log` + entity doc

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0008_partition_audit_log.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs` (register `m0008`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/entities/audit_log.rs` (doc comment on the DB/entity PK divergence)
- Test: `rs/crates/services/paigasus-iam/tests/audit_log_partition_pg.rs` (new)

**Interfaces:**
- Consumes: the existing `audit_log` plain table from `m0006` (columns: `id uuid`, `occurred_at timestamptz`, `actor_prn text`, `action text`, `resource_prn text`, `outcome text`, `determining_policies text`, `detail text NOT NULL DEFAULT '{}'`, `correlation_id uuid`); `tests/support::start_migrated_postgres()`.
- Produces: a partitioned `audit_log` (parent `PARTITION BY LIST (outcome)`; subtrees `audit_log_committed`/`audit_log_denied` each `PARTITION BY RANGE (occurred_at)`; leaves `audit_log_<outcome>_YYYY_MM`; RANGE defaults `audit_log_<outcome>_default`; LIST default `audit_log_other`; DB PK `(id, occurred_at, outcome)` named `audit_log_pkey`; indexes on `occurred_at`/`actor_prn`/`resource_prn`/`action` on the parent). The month-leaf naming convention `audit_log_<outcome>_YYYY_MM` is what Task 4's maintainer parses/creates.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/audit_log_partition_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Schema + data-preservation tests for m0008 (SMA-467): `audit_log` is converted to a
//! two-level `LIST(outcome)→RANGE(occurred_at)` partitioned table with LIST + RANGE default
//! backstops, and the migration preserves existing rows. Runs against an ephemeral Postgres in
//! Docker (skips on a Docker-less laptop, same gating as `audit_log_pg.rs`).

mod support;

use chrono::{TimeZone, Utc};
use paigasus_iam::adapters::persistence::entities::audit_log;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set, Statement};
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_log_partition_pg`
Expected: FAIL — `audit_log_is_partitioned` returns false (m0008 doesn't exist yet), the `assert!` panics. (If Docker is absent it skips/returns — run on a Docker host or rely on CI.)

- [ ] **Step 3: Write the migration**

Create `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0008_partition_audit_log.rs`:

```rust
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
    const COLS: &'static str =
        "id, occurred_at, actor_prn, action, resource_prn, outcome, determining_policies, detail, correlation_id";
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("SET LOCAL TimeZone = 'UTC';").await?;
        db.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({AUDIT_PARTITION_LOCK_KEY});")).await?;

        // Idempotency guard: if a concurrent replica already swapped, do nothing.
        if is_partitioned(db).await? {
            return Ok(());
        }

        // Determine the month span to pre-create so existing rows don't land in the RANGE default.
        // Empty table → span is just the current month; +1 month of create-ahead either way.
        let (start, end) = existing_month_span(db).await?;

        // 1. Parent + subtrees + LIST default.
        db.execute_unprepared(&format!(
            "CREATE TABLE audit_log_new (\
                id uuid NOT NULL, \
                occurred_at timestamptz NOT NULL, \
                actor_prn text, \
                action text NOT NULL, \
                resource_prn text, \
                outcome text NOT NULL, \
                determining_policies text, \
                detail text NOT NULL DEFAULT '{{}}', \
                correlation_id uuid, \
                CONSTRAINT audit_log_new_pkey PRIMARY KEY (id, occurred_at, outcome)\
             ) PARTITION BY LIST (outcome);"
        ))
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
        db.execute_unprepared(&format!(
            "INSERT INTO audit_log_new ({cols}) SELECT {cols} FROM audit_log;",
            cols = Self::COLS
        ))
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
        db.execute_unprepared(&format!(
            "INSERT INTO audit_log_plain ({cols}) SELECT {cols} FROM audit_log;",
            cols = Self::COLS
        ))
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
    Ok(db.query_one(stmt).await?.is_some())
}

/// `(start, end)` = ((year, month) of `min(occurred_at)`, (year, month) of `max(now, now)+1mo`).
/// Empty table → both default to the current UTC month; end is always ≥ current + 1 month ahead.
async fn existing_month_span(db: &impl sea_orm::ConnectionTrait) -> Result<((i32, u32), (i32, u32)), DbErr> {
    let now = Utc::now();
    let row = db
        .query_one(sea_orm::Statement::from_string(
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
    let hi_dt = hi.unwrap_or(now).max(now);
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
```

Then register it in `mod.rs` — add `mod m0008_partition_audit_log;` after the `m0007` line, and `Box::new(m0008_partition_audit_log::Migration),` after the `m0007` entry in the `migrations()` vec.

Add the entity doc comment — in `entities/audit_log.rs`, above `pub struct Model`, append a paragraph to the module doc:

```rust
//! **DB vs entity primary key (SMA-467):** the physical table is partitioned
//! `LIST(outcome)→RANGE(occurred_at)`, so its Postgres PK is the composite `(id, occurred_at,
//! outcome)` (a partitioned table's PK must include every partition-key column). This entity
//! keeps `id` as its sole `primary_key`: `id` is a per-entry UUIDv7 (the logical identity), the
//! adapter only inserts (routing) and filters, and `Entity::find_by_id(id)` still resolves a row
//! from the correct leaf. The composite PK is a partitioning requirement, not a logical key.
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_log_partition_pg`
Expected: PASS (on a Docker host). Then run the existing audit regression to confirm nothing broke:
`cargo nextest run -p paigasus-iam --test audit_log_pg --test http_audit --test grpc_audit --test mutation_audit_e2e --test api_keys_pg --test outbox_uow_pg --test authz_role_grants --test authz_policy_store`
Expected: PASS (these read audit rows incl. via `find_by_id`).

- [ ] **Step 5: Add the data-preservation + timezone + down tests**

Append to `audit_log_partition_pg.rs`:

```rust
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
```

`use sea_orm::ConnectionTrait;` is already imported. Run the two new tests:
`cargo nextest run -p paigasus-iam --test audit_log_partition_pg` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/services/paigasus-iam/src/adapters/persistence/migration/m0008_partition_audit_log.rs \
        crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs \
        crates/services/paigasus-iam/src/adapters/persistence/entities/audit_log.rs \
        crates/services/paigasus-iam/tests/audit_log_partition_pg.rs
git commit -m "feat(rs): partition audit_log by outcome then month (SMA-467)"
```

---

### Task 2: `[audit.retention]` + query-window config

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (add `RetentionConfig`, extend `AuditConfig`, defaults, `validate()`, tests)
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example` (document the new block) — create if absent
- Test: inline `#[cfg(test)] mod tests` in `config.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `AuditConfig { denial_buffer_capacity, retention: RetentionConfig, query_default_window_days: u32, query_max_window_days: u32 }`; `RetentionConfig { enabled: bool, interval_secs: u64, ahead_months: u32, denied_months: u32, committed_months: u32 }`. Task 4 reads `retention`; Task 5 reads `retention` + the query-window fields; the maintainer/query consume these values.

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn retention_defaults_land_with_no_block() {
    figment::Jail::expect_with(|jail| {
        jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
        jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"", minimal_issuer_toml(), valid_pepper_b64()))?;
        let cfg: IamConfig = IamConfig::figment().extract()?;
        assert!(cfg.audit.retention.enabled);
        assert_eq!(cfg.audit.retention.interval_secs, 86_400);
        assert_eq!(cfg.audit.retention.ahead_months, 1);
        assert_eq!(cfg.audit.retention.denied_months, 3);
        assert_eq!(cfg.audit.retention.committed_months, 0);
        assert_eq!(cfg.audit.query_default_window_days, 90);
        assert_eq!(cfg.audit.query_max_window_days, 366);
        assert!(cfg.validate().is_ok(), "retention defaults must validate");
        Ok(())
    });
}

#[test]
fn validate_rejects_zero_retention_interval() {
    figment::Jail::expect_with(|jail| {
        jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
        jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"\n[audit.retention]\ninterval_secs = 0", minimal_issuer_toml(), valid_pepper_b64()))?;
        let cfg: IamConfig = IamConfig::figment().extract()?;
        assert!(cfg.validate().is_err(), "interval_secs = 0 must fail validation");
        Ok(())
    });
}

#[test]
fn validate_rejects_out_of_range_ahead_months() {
    for bad in ["ahead_months = 0", "ahead_months = 25"] {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"\n[audit.retention]\n{bad}", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "{bad} must fail validation");
            Ok(())
        });
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib config::tests::retention_defaults_land_with_no_block config::tests::validate_rejects_zero_retention_interval config::tests::validate_rejects_out_of_range_ahead_months`
Expected: compile error / FAIL — `AuditConfig` has no `retention` field.

- [ ] **Step 3: Implement the config**

In `config.rs`:

(a) Extend `AuditConfig` (the struct at line ~222) — add fields after `denial_buffer_capacity`:

```rust
    /// Partition-maintenance + outcome-aware retention (SMA-467). Absent block → all defaults.
    #[serde(default)]
    pub retention: RetentionConfig,
    /// When an audit `query` supplies neither `from` nor `to`, apply this lookback so the read
    /// prunes to recent partitions instead of MergeAppend-scanning every leaf (SMA-467 §3.6).
    pub query_default_window_days: u32,
    /// Hard cap on any `from`/`to` span; an over-wide range is clamped to this.
    pub query_max_window_days: u32,
```

(b) Add the `RetentionConfig` struct + its `Default` (place near `AuditConfig`):

```rust
/// `[audit.retention]` (SMA-467) — the in-app partition-maintenance task's knobs. Like the rest of
/// `[audit]`, every field has a default so an absent block is valid config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetentionConfig {
    /// `false` → the maintenance task is NOT spawned at all (no create-ahead, no pruning). To pause
    /// only DELETIONS while keeping create-ahead healthy, leave this `true` and set the two
    /// `*_months` to 0 — see `main.rs`'s startup `warn` for the disabled path's default-pollution
    /// consequence.
    pub enabled: bool,
    /// Seconds between maintenance ticks (create-ahead + prune). Validated non-zero.
    pub interval_secs: u64,
    /// How many months ahead to pre-create leaf partitions. Validated `1..=24`.
    pub ahead_months: u32,
    /// Drop denied monthly leaves older than this. `0` = never drop denied.
    pub denied_months: u32,
    /// Drop committed monthly leaves older than this. `0` = never auto-drop committed (default;
    /// a non-zero value auto-deletes compliance rows and triggers a startup `warn`).
    pub committed_months: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        RetentionConfig { enabled: true, interval_secs: 86_400, ahead_months: 1, denied_months: 3, committed_months: 0 }
    }
}
```

(c) Extend the `AuditDefaults` struct + its `Default` (line ~321/408) to include the new fields, and the `Default for AuditConfig` (line ~465):

```rust
// AuditDefaults — add fields:
struct AuditDefaults {
    denial_buffer_capacity: usize,
    retention: RetentionConfig,
    query_default_window_days: u32,
    query_max_window_days: u32,
}
impl Default for AuditDefaults {
    fn default() -> Self {
        AuditDefaults {
            denial_buffer_capacity: 4096,
            retention: RetentionConfig::default(),
            query_default_window_days: 90,
            query_max_window_days: 366,
        }
    }
}
// Default for AuditConfig — mirror the new fields:
impl Default for AuditConfig {
    fn default() -> Self {
        let d = AuditDefaults::default();
        AuditConfig {
            denial_buffer_capacity: d.denial_buffer_capacity,
            retention: d.retention,
            query_default_window_days: d.query_default_window_days,
            query_max_window_days: d.query_max_window_days,
        }
    }
}
```

(d) Add validation in `validate()`, in the `[audit]` block (after the `denial_buffer_capacity` check ~line 671):

```rust
        // SMA-467 [audit.retention]: the tick interval divides the maintenance loop's cadence
        // (zero would busy-loop), and ahead_months is capped — each ahead month is a
        // parent-locking CREATE, so a fat-fingered large value would hammer the parent every tick.
        if self.audit.retention.interval_secs == 0 {
            return Err("audit.retention.interval_secs must be at least 1 (0 would busy-loop the maintenance task)".to_string());
        }
        if !(1..=24).contains(&self.audit.retention.ahead_months) {
            return Err(format!("audit.retention.ahead_months ({}) must be between 1 and 24", self.audit.retention.ahead_months));
        }
        if self.audit.query_default_window_days == 0 || self.audit.query_max_window_days == 0 {
            return Err("audit.query_default_window_days and audit.query_max_window_days must be at least 1".to_string());
        }
```

(e) Because `AuditDefaults` is fed to figment's `Serialized::defaults`, and `RetentionConfig` derives `Serialize`, the nested block populates from defaults automatically; `#[serde(default)]` on the `retention` field lets a partial `[audit.retention]` block merge over them.

- [ ] **Step 4: Run to verify pass**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib config::tests`
Expected: PASS (new + existing config tests).

- [ ] **Step 5: Document in `iam.toml.example`**

Find the example config: `ls rs/crates/services/paigasus-iam/iam.toml.example` (if it exists, append; if not, skip this step and instead ensure the RUNBOOK in Task 6 documents it). If present, add:

```toml
# Audit-log partition maintenance + outcome-aware retention (SMA-467).
[audit.retention]
enabled = true          # false = don't run the task at all (see RUNBOOK: recovery-trap note)
interval_secs = 86400   # maintenance tick cadence (daily)
ahead_months = 1        # pre-create this many months of leaf partitions ahead
denied_months = 3       # drop denied monthly partitions older than this (0 = never)
committed_months = 0    # drop committed partitions older than this (0 = never; opt-in)
```

- [ ] **Step 6: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/services/paigasus-iam/src/config.rs crates/services/paigasus-iam/iam.toml.example 2>/dev/null
git commit -m "feat(rs): add [audit.retention] + audit query-window config (SMA-467)"
```

---

### Task 3: Metric name registry entries

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (4 consts + `ALL` entries)

**Interfaces:**
- Produces: `names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL`, `IAM_AUDIT_PARTITIONS_CREATED_TOTAL`, `IAM_AUDIT_PARTITIONS_DROPPED_TOTAL`, `IAM_AUDIT_DEFAULT_PARTITION_ROWS`. Task 4 emits them; Task 6 describes them; Task 6's alert references the ticks counter (drift test requires it in `ALL`).

- [ ] **Step 1: Add the consts + ALL entries**

In `names.rs`, after the outbox relay consts (line ~32) add:

```rust
pub const IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL: &str = "iam_audit_partition_maintenance_ticks_total";
pub const IAM_AUDIT_PARTITIONS_CREATED_TOTAL: &str = "iam_audit_partitions_created_total";
pub const IAM_AUDIT_PARTITIONS_DROPPED_TOTAL: &str = "iam_audit_partitions_dropped_total";
pub const IAM_AUDIT_DEFAULT_PARTITION_ROWS: &str = "iam_audit_default_partition_rows";
```

And add those four identifiers to the `ALL` slice (after `IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS`).

- [ ] **Step 2: Verify the crate + drift test compile/pass**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-observability`
Expected: PASS (drift test tolerates names in `ALL` that no dashboard references yet).

- [ ] **Step 3: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): register audit partition-maintenance metric names (SMA-467)"
```

---

### Task 4: `PgPartitionMaintainer` + background loop

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_partition_maintainer.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs` (module + re-export)
- Test: `rs/crates/services/paigasus-iam/tests/audit_partition_maintenance_pg.rs` (new)

**Interfaces:**
- Consumes: the partitioned `audit_log` (Task 1), `RetentionConfig` (Task 2), `names::*` (Task 3), `migration::m0008_partition_audit_log::AUDIT_PARTITION_LOCK_KEY`.
- Produces: `PgPartitionMaintainer::new(db: DatabaseConnection) -> Self`; `async fn tick(&self, now: DateTime<Utc>, policy: RetentionPolicy) -> MaintenanceReport`; `async fn run(self, policy: RetentionPolicy, interval: Duration, shutdown: impl Future<Output=()> + Send)`. `RetentionPolicy { ahead_months: u32, denied_months: u32, committed_months: u32 }` (a plain Copy struct built from `RetentionConfig` by `main.rs`). Task 5 does not consume this; Task 6 spawns `run`.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/audit_partition_maintenance_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgPartitionMaintainer` (SMA-467): create-ahead is idempotent, and prune
//! is outcome-aware (drops aged denied leaves, keeps recent denied + all committed, never a
//! default), and prune runs even if create-ahead can't. Real Postgres via testcontainers.

mod support;

use chrono::{TimeZone, Utc};
use paigasus_iam::adapters::persistence::{PgPartitionMaintainer, RetentionPolicy};
use sea_orm::{ConnectionTrait, Statement};

async fn leaf_exists(db: &impl ConnectionTrait, name: &str) -> bool {
    let stmt = Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!("SELECT 1 FROM pg_class WHERE relname = '{name}' AND relkind = 'r'"),
    );
    db.query_one(stmt).await.unwrap().is_some()
}

#[tokio::test]
async fn ensure_ahead_is_idempotent_and_creates_leaves() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let m = PgPartitionMaintainer::new(db.clone());
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
    let policy = RetentionPolicy { ahead_months: 2, denied_months: 3, committed_months: 0 };

    m.tick(now, policy).await;
    m.tick(now, policy).await; // second run must not error (IF NOT EXISTS)

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
    m.tick(old, RetentionPolicy { ahead_months: 1, denied_months: 0, committed_months: 0 }).await;
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
    // denied_months = 3 → Jan/Feb denied leaves are older than 3 months from July and get dropped;
    // committed_months = 0 → no committed leaf is ever dropped.
    m.tick(now, RetentionPolicy { ahead_months: 1, denied_months: 3, committed_months: 0 }).await;

    assert!(!leaf_exists(&db, "audit_log_denied_2026_01").await, "aged denied Jan leaf must be dropped");
    assert!(leaf_exists(&db, "audit_log_committed_2026_01").await, "committed leaf must NOT be dropped when committed_months = 0");
    assert!(leaf_exists(&db, "audit_log_denied_2026_07").await, "current denied leaf must be kept");
    assert!(leaf_exists(&db, "audit_log_denied_default").await, "the denied RANGE default must never be dropped");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_partition_maintenance_pg`
Expected: FAIL — `PgPartitionMaintainer`/`RetentionPolicy` don't exist (compile error).

- [ ] **Step 3: Implement the maintainer**

Create `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_partition_maintainer.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `PgPartitionMaintainer` (SMA-467): the background task that keeps `audit_log`'s monthly leaf
//! partitions created ahead of time and drops aged-out ones per an outcome-aware retention policy.
//!
//! Mirrors `OutboxRelay` (`adapters::events::relay`): a `tick` does one unit of work and returns a
//! report; `run` is the `tokio::select!` shutdown-watch loop the composition root spawns.
//!
//! **Never stalls audit inserts.** Each DDL op runs in its OWN short transaction that first takes
//! `pg_advisory_xact_lock(AUDIT_PARTITION_LOCK_KEY)` (one replica does DDL at a time; same key as
//! m0008 so the swap and a tick never overlap) and `SET LOCAL lock_timeout` so a CREATE/DROP that
//! would block behind live-insert locks on the parent BACKS OFF (errors, retried next tick) instead
//! of queueing and stalling all audit writes (hence mutations, since committed audit rows are
//! in-txn). `prune` is attempted independently of `ensure_partitions_ahead`, so a create-ahead
//! failure (e.g. a polluted default) can't wedge retention.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};

use super::migration::m0008_partition_audit_log::AUDIT_PARTITION_LOCK_KEY;

/// The retention knobs a tick needs, decoupled from `config::RetentionConfig` (`enabled`/
/// `interval_secs` live in the loop, not a tick). `Copy` so tests/`run` pass it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub ahead_months: u32,
    pub denied_months: u32,
    pub committed_months: u32,
}

/// Per-tick outcome, returned (and logged) so tests can assert without scraping logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceReport {
    pub created: u64,
    pub dropped_denied: u64,
    pub dropped_committed: u64,
    pub errored: bool,
}

const LOCK_TIMEOUT: &str = "5s";

#[derive(Clone)]
pub struct PgPartitionMaintainer {
    db: DatabaseConnection,
}

impl PgPartitionMaintainer {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgPartitionMaintainer { db }
    }

    /// One maintenance unit: create-ahead, then (independently) prune. Errors are logged + counted,
    /// never propagated — the loop keeps running.
    pub async fn tick(&self, now: DateTime<Utc>, policy: RetentionPolicy) -> MaintenanceReport {
        let mut report = MaintenanceReport::default();

        match self.ensure_partitions_ahead(now, policy.ahead_months).await {
            Ok(n) => report.created = n,
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "audit partition create-ahead failed; will retry next tick");
            }
        }
        // prune runs regardless of create-ahead's outcome (independence, §5.1).
        match self.prune(now, policy).await {
            Ok((d, c)) => {
                report.dropped_denied = d;
                report.dropped_committed = c;
            }
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "audit partition prune failed; will retry next tick");
            }
        }

        // Refresh the default-partition-rows gauge (the "create-ahead fell behind" signal).
        if let Ok(rows) = self.default_partition_rows().await {
            gauge!(names::IAM_AUDIT_DEFAULT_PARTITION_ROWS).set(rows as f64);
        }

        counter!(names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL, "result" => if report.errored { "error" } else { "ok" }).increment(1);
        counter!(names::IAM_AUDIT_PARTITIONS_CREATED_TOTAL).increment(report.created);
        counter!(names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL, "outcome" => "denied").increment(report.dropped_denied);
        counter!(names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL, "outcome" => "committed").increment(report.dropped_committed);
        tracing::info!(created = report.created, dropped_denied = report.dropped_denied, dropped_committed = report.dropped_committed, errored = report.errored, "audit partition maintenance tick");
        report
    }

    /// `CREATE TABLE IF NOT EXISTS` a monthly leaf for both outcome subtrees for each month in
    /// `[now, now + ahead_months]`. Each CREATE is its own locked, lock_timeout'd transaction.
    async fn ensure_partitions_ahead(&self, now: DateTime<Utc>, ahead_months: u32) -> Result<u64, DbErr> {
        let mut created = 0;
        let mut ym = (now.year(), now.month());
        for _ in 0..=ahead_months {
            for sub in ["committed", "denied"] {
                let ddl = month_leaf_ddl(sub, ym.0, ym.1);
                self.run_ddl(&ddl).await?;
                created += 1;
            }
            ym = add_one_month(ym);
        }
        Ok(created)
    }

    /// Drop denied leaves older than `denied_months` and (only if `committed_months > 0`) committed
    /// leaves older than `committed_months`. Enumerates ACTUAL child leaves from the catalog and
    /// drops those whose parsed `YYYY_MM` is strictly before the cutoff month. Never drops a
    /// `*_default`. Returns `(dropped_denied, dropped_committed)`.
    async fn prune(&self, now: DateTime<Utc>, policy: RetentionPolicy) -> Result<(u64, u64), DbErr> {
        let mut dropped = (0u64, 0u64);
        for (sub, months, slot) in [
            ("denied", policy.denied_months, 0usize),
            ("committed", policy.committed_months, 1usize),
        ] {
            if months == 0 {
                continue; // 0 = never drop this outcome
            }
            let cutoff = subtract_months((now.year(), now.month()), months);
            for leaf in self.child_leaves(sub).await? {
                if let Some(ym) = parse_leaf_month(&leaf, sub) {
                    if ym < cutoff {
                        self.run_ddl(&format!("DROP TABLE IF EXISTS {leaf};")).await?;
                        if slot == 0 { dropped.0 += 1 } else { dropped.1 += 1 }
                    }
                }
            }
        }
        Ok(dropped)
    }

    /// Names of the concrete leaf partitions under `audit_log_<sub>` (excludes the default, whose
    /// name is `audit_log_<sub>_default` and never parses to a month).
    async fn child_leaves(&self, sub: &str) -> Result<Vec<String>, DbErr> {
        let stmt = Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT c.relname AS name FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'audit_log_{sub}'::regclass"),
        );
        let rows = self.db.query_all(stmt).await?;
        Ok(rows.iter().filter_map(|r| r.try_get::<String>("", "name").ok()).collect())
    }

    async fn default_partition_rows(&self) -> Result<i64, DbErr> {
        let stmt = Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT (SELECT count(*) FROM audit_log_committed_default) + (SELECT count(*) FROM audit_log_denied_default) + (SELECT count(*) FROM audit_log_other) AS n".to_string(),
        );
        Ok(self.db.query_one(stmt).await?.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0))
    }

    /// Run one DDL statement in its own transaction under the advisory lock + a bounded
    /// `lock_timeout` (so it backs off rather than stalling live inserts).
    async fn run_ddl(&self, ddl: &str) -> Result<(), DbErr> {
        let txn = self.db.begin().await?;
        txn.execute_unprepared("SET LOCAL TimeZone = 'UTC';").await?;
        txn.execute_unprepared(&format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}';")).await?;
        txn.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({AUDIT_PARTITION_LOCK_KEY});")).await?;
        txn.execute_unprepared(ddl).await?;
        txn.commit().await
    }

    /// The shutdown-watch loop (mirrors `OutboxRelay::run`): sleep `interval`, tick, repeat until
    /// `shutdown` resolves.
    pub async fn run<S>(self, policy: RetentionPolicy, interval: Duration, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => { self.tick(Utc::now(), policy).await; }
                () = &mut shutdown => break,
            }
        }
    }
}

fn month_leaf_ddl(sub: &str, year: i32, month1: u32) -> String {
    let (ny, nm) = add_one_month((year, month1));
    format!(
        "CREATE TABLE IF NOT EXISTS audit_log_{sub}_{year:04}_{month1:02} \
         PARTITION OF audit_log_{sub} \
         FOR VALUES FROM (TIMESTAMPTZ '{year:04}-{month1:02}-01 00:00:00+00') \
         TO (TIMESTAMPTZ '{ny:04}-{nm:02}-01 00:00:00+00');"
    )
}

fn add_one_month((y, m): (i32, u32)) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

/// `(y, m)` shifted back by `n` months.
fn subtract_months((y, m): (i32, u32), n: u32) -> (i32, u32) {
    let total = (y * 12 + (m as i32 - 1)) - n as i32;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

/// Parse `audit_log_<sub>_YYYY_MM` → `(YYYY, MM)`; `None` for the default (`…_default`) or any
/// non-month child.
fn parse_leaf_month(name: &str, sub: &str) -> Option<(i32, u32)> {
    let suffix = name.strip_prefix(&format!("audit_log_{sub}_"))?;
    let (y, m) = suffix.split_once('_')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtract_months_crosses_year_boundary() {
        assert_eq!(subtract_months((2026, 7), 3), (2026, 4));
        assert_eq!(subtract_months((2026, 2), 3), (2025, 11));
        assert_eq!(subtract_months((2026, 1), 1), (2025, 12));
    }
    #[test]
    fn parse_leaf_month_ignores_default() {
        assert_eq!(parse_leaf_month("audit_log_denied_2026_07", "denied"), Some((2026, 7)));
        assert_eq!(parse_leaf_month("audit_log_denied_default", "denied"), None);
    }
}
```

Then in `persistence/mod.rs` add `pub mod pg_partition_maintainer;` (with the other `pub mod`s) and `pub use pg_partition_maintainer::{PgPartitionMaintainer, RetentionPolicy};` (with the other `pub use`s). Also make the migration const reachable: confirm `pub mod migration;` already re-exports the module path (it does — `m0008_partition_audit_log` is `pub` inside `migration`); if `mod m0008_partition_audit_log;` in `migration/mod.rs` is private, change it to `pub mod m0008_partition_audit_log;` so `AUDIT_PARTITION_LOCK_KEY` is importable.

- [ ] **Step 4: Run to verify pass**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_partition_maintenance_pg --lib pg_partition_maintainer`
Expected: PASS (integration + the two pure unit tests).

- [ ] **Step 5: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/services/paigasus-iam/src/adapters/persistence/pg_partition_maintainer.rs \
        crates/services/paigasus-iam/src/adapters/persistence/mod.rs \
        crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs \
        crates/services/paigasus-iam/tests/audit_partition_maintenance_pg.rs
git commit -m "feat(rs): add PgPartitionMaintainer create-ahead + outcome-aware prune (SMA-467)"
```

---

### Task 5: Bounded default/max query window in `PgAuditLog`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_audit_log.rs` (opt-in window builder + apply in `query`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:395` (chain `.with_query_window(...)` at the production construction site)
- Test: `rs/crates/services/paigasus-iam/tests/audit_log_pg.rs` (add a window test)

**Interfaces:**
- Consumes: `AuditConfig.query_default_window_days` / `query_max_window_days` (Task 2).
- Produces: `PgAuditLog::with_query_window(self, default_days: u32, max_days: u32) -> Self` (builder; `new(db)` unchanged so the ~12 existing call sites need no edit). `query` applies the window only when set.

- [ ] **Step 1: Write the failing test**

Add to `audit_log_pg.rs`:

```rust
/// With a query window configured, a filter supplying NEITHER `from` nor `to` only returns rows
/// inside the default lookback — older rows are pruned out (SMA-467 §3.6).
#[tokio::test]
async fn query_applies_default_lookback_window() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let sink = PgAuditLog::new(db.clone()).with_query_window(30, 366); // 30-day default lookback

    let recent = Utc::now() - chrono::Duration::days(1);
    let old = Utc::now() - chrono::Duration::days(120);
    for (id, ts) in [(Uuid::from_u128(900), recent), (Uuid::from_u128(901), old)] {
        let e = AuditEntry { occurred_at: ts, ..denial(id, "win-actor") };
        sink.record_out_of_band(&e).await.unwrap();
    }
    let rows = sink
        .query(&AuditFilter { actor_prn: Some("win-actor".to_string()), resource_prn: None, action: None, outcome: Some(AuditOutcome::Denied), from: None, to: None, cursor: None, limit: 10 })
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "only the row inside the 30-day default window must return");
    assert_eq!(rows[0].id, Uuid::from_u128(900));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_log_pg::query_applies_default_lookback_window`
Expected: FAIL — `with_query_window` doesn't exist (compile error).

- [ ] **Step 3: Implement the window**

In `pg_audit_log.rs`, add a field + builder and apply it in `query`:

```rust
// struct PgAuditLog — add a field:
#[derive(Clone)]
pub struct PgAuditLog {
    db: DatabaseConnection,
    /// `Some((default_days, max_days))` enables SMA-467 §3.6 window bounding; `None` = unbounded
    /// (the default from `new`, keeping every existing test's `new(db)` call unchanged).
    query_window: Option<(u32, u32)>,
}

impl PgAuditLog {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgAuditLog { db, query_window: None }
    }

    /// Enable the default-lookback + max-span window on `query` (SMA-467 §3.6). Chained at the
    /// composition root; tests that don't care leave it off via `new`.
    #[must_use]
    pub fn with_query_window(mut self, default_days: u32, max_days: u32) -> Self {
        self.query_window = Some((default_days, max_days));
        self
    }
}
```

In `query`, compute the effective `from`/`to` before building the SeaORM filter — insert this right after `let mut q = audit_log::Entity::find();`:

```rust
        // SMA-467 §3.6: bound an otherwise-unfiltered scan. `from`/`to` from the filter win; when
        // BOTH are absent apply the default lookback; clamp any span to max_days.
        let (eff_from, eff_to) = match self.query_window {
            None => (f.from, f.to),
            Some((default_days, max_days)) => {
                let to = f.to.unwrap_or_else(Utc::now);
                let from = f.from.unwrap_or_else(|| to - chrono::Duration::days(i64::from(default_days)));
                let max_from = to - chrono::Duration::days(i64::from(max_days));
                (Some(from.max(max_from)), Some(to))
            }
        };
```

Then replace the existing `if let Some(from) = f.from` / `if let Some(to) = f.to` blocks to use `eff_from`/`eff_to`:

```rust
        if let Some(from) = eff_from {
            q = q.filter(audit_log::Column::OccurredAt.gte(from));
        }
        if let Some(to) = eff_to {
            q = q.filter(audit_log::Column::OccurredAt.lte(to));
        }
```

Ensure `use chrono::Utc;` is present in the file (add it to the imports if not).

In `adapters/http/mod.rs:395`, chain the builder using the config (the `AppState::new` scope has `config: &IamConfig`):

```rust
        let audit_log: Arc<dyn AuditLog> = Arc::new(
            PgAuditLog::new(db.clone()).with_query_window(config.audit.query_default_window_days, config.audit.query_max_window_days),
        );
```

- [ ] **Step 4: Run to verify pass**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --test audit_log_pg`
Expected: PASS (new window test + the existing 3 — the existing ones use `new(db)` so are unwindowed and unaffected).

- [ ] **Step 5: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/services/paigasus-iam/src/adapters/persistence/pg_audit_log.rs \
        crates/services/paigasus-iam/src/adapters/http/mod.rs \
        crates/services/paigasus-iam/tests/audit_log_pg.rs
git commit -m "feat(rs): bound audit query with default/max time window (SMA-467)"
```

---

### Task 6: Wire the maintenance task into `main.rs`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`

**Interfaces:**
- Consumes: `PgPartitionMaintainer`, `RetentionPolicy` (Task 4); `config.audit.retention` (Task 2); `names::*` (Task 3).
- Produces: nothing (composition root).

- [ ] **Step 1: Add the imports + `db` clone**

At the top of `main.rs`, extend the persistence import to include the maintainer:

```rust
use paigasus_iam::adapters::persistence::{Migrator, PgPartitionMaintainer, RetentionPolicy};
```

(Replace the existing `use paigasus_iam::adapters::{grpc, persistence::Migrator};` with `use paigasus_iam::adapters::grpc;` + the line above, or merge appropriately.)

Because the outbox relay block **moves** `db` (line ~168), clone a handle for the maintainer BEFORE that block. Right after `let state = AppState::new(db.clone(), &config).await?;` (line ~44) add:

```rust
    // Kept for the partition-maintenance task (SMA-467), spawned below; cloned before the outbox
    // relay block consumes the original `db` handle.
    let db_for_maintenance = db.clone();
```

- [ ] **Step 2: Add the maintenance spawn block**

After the outbox-relay block (after line ~184, before `tracing::info!(… "paigasus-iam started")`), add:

```rust
    {
        // Audit-log partition maintenance (SMA-467): create month partitions ahead and drop
        // aged-out denied (and, if configured, committed) leaves — mirrors the outbox relay's
        // spawn + shutdown-watch. Gated by `[audit.retention].enabled`; a startup run creates the
        // current + ahead months before the loop (non-fatal on error — the migration + the
        // DEFAULT partitions already backstop writes).
        if config.audit.retention.enabled {
            let policy = RetentionPolicy {
                ahead_months: config.audit.retention.ahead_months,
                denied_months: config.audit.retention.denied_months,
                committed_months: config.audit.retention.committed_months,
            };
            if config.audit.retention.committed_months > 0 {
                tracing::warn!(committed_months = config.audit.retention.committed_months, "audit.retention.committed_months > 0 — committed (compliance) audit partitions will be auto-dropped at this age");
            }
            let maintainer = PgPartitionMaintainer::new(db_for_maintenance);
            let startup = maintainer.clone();
            let startup_policy = policy;
            // Awaited startup run (non-fatal).
            let report = startup.tick(chrono::Utc::now(), startup_policy).await;
            if report.errored {
                tracing::warn!("initial audit partition maintenance tick reported an error — continuing (DEFAULT partitions backstop writes)");
            }
            let interval = std::time::Duration::from_secs(config.audit.retention.interval_secs);
            let mut rx = rx.clone();
            servers.spawn(async move {
                maintainer.run(policy, interval, async move { let _ = rx.changed().await; }).await;
                Ok(())
            });
        } else {
            tracing::warn!("audit.retention.enabled = false — no partition create-ahead or pruning will run; the DEFAULT partitions will fill over time and can block create-ahead until manually reattached (see RUNBOOK)");
        }
    }
```

(`chrono` is already a dependency of the crate; if `chrono::Utc` isn't already imported in `main.rs`, use the fully-qualified `chrono::Utc::now()` as written.)

- [ ] **Step 3: Add metric descriptions**

Find the block in `main.rs` (~line 299) that calls `describe_counter!`/`describe_gauge!` for the outbox metrics and add, alongside them:

```rust
    describe_counter!(names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL, "Audit partition-maintenance ticks (create-ahead + prune); label result=ok|error.");
    describe_counter!(names::IAM_AUDIT_PARTITIONS_CREATED_TOTAL, "Audit monthly leaf partitions created by create-ahead.");
    describe_counter!(names::IAM_AUDIT_PARTITIONS_DROPPED_TOTAL, "Audit monthly leaf partitions dropped by retention; label outcome=denied|committed.");
    describe_gauge!(names::IAM_AUDIT_DEFAULT_PARTITION_ROWS, "Rows currently in the audit DEFAULT partitions — should be 0; nonzero means create-ahead fell behind.");
```

(Match the existing `describe_*`/`names` import style already present in `main.rs`.)

- [ ] **Step 4: Build + run the full iam unit/integration suite for the wiring**

Run: `cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo build -p paigasus-iam && cargo nextest run -p paigasus-iam --lib`
Expected: PASS/compile clean. (The `main.rs` wiring has no direct unit test; the build + existing suites are the gate. A `cargo run -p paigasus-iam` against a local PG is a good manual smoke — the startup tick should create leaves and log "audit partition maintenance tick".)

- [ ] **Step 5: Commit**

```bash
cd rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add crates/services/paigasus-iam/src/main.rs
git commit -m "feat(rs): spawn audit partition-maintenance task at startup (SMA-467)"
```

---

### Task 7: RUNBOOK + alert rule + promtool test

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (§2.2 catalog, §4 retention section + alert, §6 future)
- Modify: `ops/observability/prometheus/rules/iam.rules.yml` (add `IamAuditPartitionMaintenanceStalled`)
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml` (promtool test for the new alert)

**Interfaces:**
- Consumes: `names::IAM_AUDIT_PARTITION_MAINTENANCE_TICKS_TOTAL` etc. (Task 3) — the drift test requires any metric referenced in the rule YAML to be in `names::ALL` (satisfied by Task 3).

- [ ] **Step 1: Add the alert rule**

In `ops/observability/prometheus/rules/iam.rules.yml`, add a rule to the iam group (match the existing YAML shape of `IamOutboxRelayStalled`):

```yaml
      - alert: IamAuditPartitionMaintenanceStalled
        expr: rate(iam_audit_partition_maintenance_ticks_total[1h]) == 0
        for: 2h
        labels:
          severity: warning
        annotations:
          summary: "IAM audit partition-maintenance task is not ticking"
          description: "No audit partition-maintenance ticks in the last hour. Denied partitions won't be pruned (slow index/table bloat) and, after ahead_months, new-month create-ahead stops. NOTE: if audit.retention.enabled=false this is expected — silence it. See RUNBOOK §4."
```

- [ ] **Step 2: Add the promtool test**

In `ops/observability/prometheus/rules/tests/iam.test.yml`, add a test case (match the existing `IamOutboxRelayStalled` test's structure): a series `iam_audit_partition_maintenance_ticks_total` that is flat (no increase) for >2h asserts the alert fires; a rising series asserts it does not. Example (adapt field names/indentation to the file's existing cases):

```yaml
  - interval: 1m
    input_series:
      - series: 'iam_audit_partition_maintenance_ticks_total{job="iam"}'
        values: '1+0x200'   # flat for >2h → no rate → alert fires
    alert_rule_test:
      - eval_time: 2h30m
        alertname: IamAuditPartitionMaintenanceStalled
        exp_alerts:
          - exp_labels:
              severity: warning
              job: iam
            exp_annotations:
              summary: "IAM audit partition-maintenance task is not ticking"
              description: "No audit partition-maintenance ticks in the last hour. Denied partitions won't be pruned (slow index/table bloat) and, after ahead_months, new-month create-ahead stops. NOTE: if audit.retention.enabled=false this is expected — silence it. See RUNBOOK §4."
```

- [ ] **Step 3: Validate the rules with promtool**

Run (promtool is used in CI; locate it via the observability tooling — check how CI invokes it, e.g. a Moon task or a vendored binary):
`cd ops/observability && promtool check rules prometheus/rules/iam.rules.yml && promtool test rules prometheus/rules/tests/iam.test.yml`
Expected: `SUCCESS` for both. (If `promtool` isn't on PATH, note that CI runs it; ensure the YAML is syntactically valid and the test asserts the alert.)

- [ ] **Step 4: Update the RUNBOOK**

In `docs/ops/RUNBOOK-observability.md`:

(a) **§2.2 catalog** — add four rows to the `paigasus-iam` metric table:

```markdown
| `iam_audit_partition_maintenance_ticks_total` | counter | `result` | One per audit partition-maintenance tick (create-ahead + prune). `result` ∈ `ok`/`error`. Liveness signal — see §4 "Audit partition maintenance stalled". |
| `iam_audit_partitions_created_total` | counter | — | Monthly leaf partitions created by create-ahead. |
| `iam_audit_partitions_dropped_total` | counter | `outcome` | Monthly leaf partitions dropped by retention. `outcome` ∈ `denied`/`committed`. |
| `iam_audit_default_partition_rows` | gauge | — | Rows currently in the audit `DEFAULT` partitions. **Should be 0**; nonzero ⇒ create-ahead fell behind (freezes when the task is stalled/disabled — the ticks counter is the primary liveness signal). |
```

(b) **§4 "Audit retention & partitioning"** — replace the "Current implementation status: plain table" + interim batched-`DELETE` block with the real design. Cover: the partition tree (`audit_log` LIST(outcome) → RANGE(occurred_at) monthly + `*_default` + `audit_log_other`); the in-app maintenance task and `[audit.retention]` config with its semantics — including that `enabled=false` is a full off-switch that eventually strands a filling default, and the "pause deletes, keep create-ahead" mode is `denied_months=0`/`committed_months=0`; the automatic drop plus the ad-hoc manual `DROP TABLE audit_log_denied_YYYY_MM`; and the non-empty-default meaning + manual reattach. Add the new alert entry `IamAuditPartitionMaintenanceStalled` to the §4 alert table + a subsection (meaning/confirm/remediation, matching the other alert entries, incl. the `enabled=false` caveat).

(c) **§6 Future** — delete the "Monthly `audit_log` partitioning + a scheduled outcome-aware retention job" bullet (now implemented); optionally add a `DETACH … CONCURRENTLY` hardening + non-empty-default auto-remediation follow-up bullet.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-467-audit-log-partitioning
git add docs/ops/RUNBOOK-observability.md ops/observability/prometheus/rules/iam.rules.yml ops/observability/prometheus/rules/tests/iam.test.yml
git commit -m "docs(rs): document audit partitioning + retention in the RUNBOOK (SMA-467)"
```

---

## Final verification (before opening the PR — Stage 5)

Run the full gate list exactly as CI does, from the worktree with the proto PATH exported:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-467-audit-log-partitioning
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations
```

Expected: all green. Diagnose any unattributed "N failed" via `.moon/cache/ciReport.json` (`jq '.actions[]|select(.status=="failed")'`). Confirm: no new clippy warnings, `cargo fmt --check` clean, the observability drift test green (new metric names in `names::ALL`), and no `:deny`/`:machete` waiver needed (no new deps).

---

## Self-review notes (author)

- **Spec coverage:** G1 partitioned table + defaults → Task 1; G2 maintenance task → Task 4 + Task 6; G3 config → Task 2; G4 query window → Task 5; G5 RUNBOOK/alert → Task 7. D9 (UTC bounds) → Task 1 + Task 4 code + Task 1 tz test; D10 (LIST default) → Task 1; D11 (per-op txn/lock_timeout, prune independence) → Task 4; D12 (query window) → Task 5; D3 (composite PK + entity doc + find_by_id) → Task 1.
- **Type consistency:** `RetentionPolicy` fields (`ahead_months`/`denied_months`/`committed_months`) match between Task 4 (def) and Task 6 (construction); `with_query_window(default_days, max_days)` matches between Task 5 def and its http/mod.rs call + config field names (`query_default_window_days`/`query_max_window_days`); `AUDIT_PARTITION_LOCK_KEY` defined in Task 1, consumed in Task 4.
- **Known verify-against-real-PG points (surfaced by tests, not assumptions):** exact composite-PK column set (Task 1 test inserts/reads); partitioned-parent `INSERT … RETURNING` routing via SeaORM (Task 1 regression suites); `CREATE TABLE IF NOT EXISTS … PARTITION OF` syntax (Task 4 idempotency test). If PG rejects any exact DDL string, fix the string — the topology/decision doesn't change.
```

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
