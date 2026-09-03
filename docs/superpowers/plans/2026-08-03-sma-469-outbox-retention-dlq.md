# SMA-469 — outbox retention + dead-letter path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound `event_outbox` growth with age-based retention, and give parked rows a real dead-letter path (inspect / replay / discard) instead of a permanent dead end.

**Architecture:** Four units. (1) An m0009 migration adds `parked_at` + `last_error` and two partial indexes. (2) `OutboxRelay::tick` records both, with a source-chain-walking error description. (3) A new `PgOutboxMaintainer` background task mirrors `PgPartitionMaintainer`: batched age-based deletes plus a dead-letter backlog gauge. (4) A Root-scoped HTTP surface (`/v1/outbox/dead-letters`) over a new `DeadLetters` core port, with replay/discard committing atomically alongside their audit entry on one `UnitOfWork`.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM 1.1.x + sea-orm-migration, axum, Cedar, `metrics` facade + Prometheus, testcontainers (Postgres 16-alpine), promtool.

**Spec:** `docs/superpowers/specs/2026-08-03-sma-469-outbox-retention-dlq-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python/YAML-adjacent tooling files).
- Rust crates are **edition 2024 + rust-version 1.95**. Do not add `edition = "2021"`.
- Conventional commits with a workspace scope: `feat(rs): …`, `fix(rs): …`, `docs(repo): …`. Subject must **start lowercase** and be **≤100 chars**.
- **Never write `#NNN` in a commit body** — it makes commitlint fail `footer-leading-blank`. Write "owner/repo PR NNN". Keep one contiguous footer block.
- Do **not** bypass git hooks with `--no-verify`. The worktree is already provisioned.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/nextest/buf resolve to repo-pinned versions (shims first).
- Work in the existing worktree at `.claude/worktrees/sma-469-outbox-retention-dlq` on branch `feature/sma-469-iam-outbox-retention-a-real-dead-letter-path-for-parked`. Do **not** `cd` to the main checkout.
- `cargo nextest` exits non-zero on a workspace with no tests — use `--no-tests=pass` where relevant.
- No new crate dependencies are needed by any task. If you think you need one, stop and ask.
- Postgres integration tests gate on Docker: hard failure when `CI` is set, `return` (skip) locally. Copy the exact pattern from `rs/crates/services/paigasus-iam/tests/relay_pg.rs`.
- Run the per-crate check after each task: `cd rs && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run -p paigasus-iam -p paigasus-iam-core -p paigasus-observability --no-tests=pass`. The **full** `moon ci` graph runs once, in Task 19.

---

## File Structure

**Created**

| File | Responsibility |
|---|---|
| `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0009_outbox_dead_letter_columns.rs` | m0009: `parked_at`/`last_error` columns, backfill, two partial indexes |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox_maintainer.rs` | `PgOutboxMaintainer`: batched retention sweep + backlog gauge |
| `rs/crates/libs/paigasus-iam-core/src/dead_letter.rs` | `DeadLetterEntry`, `DeadLetterFilter`, `BulkReplayRequest`, `DeadLetters` port |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_dead_letters.rs` | `PgDeadLetters`: the `DeadLetters` adapter |
| `rs/crates/services/paigasus-iam/src/application/dead_letters.rs` | `DeadLetterService`: authorize → mutate + audit on one UoW |
| `rs/crates/services/paigasus-iam/src/adapters/http/dead_letters.rs` | `/v1/outbox/dead-letters` handlers |
| `rs/crates/services/paigasus-iam/tests/outbox_retention_pg.rs` | Sweep integration tests |
| `rs/crates/services/paigasus-iam/tests/outbox_retention_concurrency_pg.rs` | Concurrent relay-vs-sweep disjointness proof |
| `rs/crates/services/paigasus-iam/tests/dead_letters_pg.rs` | Adapter integration tests incl. replay → relay publishes |
| `rs/crates/services/paigasus-iam/tests/http_dead_letters.rs` | End-to-end HTTP tests |

**Modified**

| File | Change |
|---|---|
| `.../adapters/events/relay.rs` | `describe_error`, `truncate_error`, write `parked_at`/`last_error` |
| `.../adapters/persistence/entities/event_outbox.rs` | two new model fields |
| `.../adapters/persistence/migration/mod.rs` | register m0009 |
| `.../adapters/persistence/mod.rs` | export the two new adapters |
| `.../src/config.rs` | `OutboxRetentionConfig` + defaults + validation |
| `.../src/application/error.rs` | `TenancyError::InvalidBulkReplay` |
| `.../src/application/fakes.rs` | `FakeDeadLetters` |
| `.../src/application/mod.rs`, `.../src/adapters/http/mod.rs` | module registration + `AppState` wiring + router merge |
| `.../src/adapters/http/dto.rs` | dead-letter DTOs |
| `.../src/main.rs` | maintainer spawn block + `describe_iam_metrics` entries |
| `rs/crates/libs/paigasus-iam-core/src/lib.rs` | export `dead_letter` |
| `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`, `.../authz/schema.rs` | three new Cedar actions |
| `rs/crates/libs/paigasus-observability/src/names.rs` | five new metric names |
| `ops/observability/prometheus/rules/iam.rules.yml` + `rules/tests/iam.test.yml` | two new alerts + fixtures |
| `ops/observability/grafana/dashboards/iam.json` | two new panels |
| `docs/ops/RUNBOOK-observability.md`, `rs/crates/services/paigasus-iam/iam.toml.example` | docs |

---

## Task 1: Descriptive publish-failure reasons

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs`

**Interfaces:**
- Produces: `fn describe_error(err: &(dyn std::error::Error + 'static)) -> String` (module-private in `relay.rs`).

**Why:** `PublishError::Backend` is `#[error("backend error")]` (`rs/crates/libs/paigasus-iam-core/src/ports.rs:373-375`) — its `Display` never renders the boxed source. `relay.rs:124` does `.map_err(|e| e.to_string())`, so today every real publish failure produces the literal string `"backend error"`. Task 3 stores that string in `last_error`, so it must be fixed first.

Do **not** change `PublishError`'s `#[error]` attribute. thiserror's `#[from]` already makes the boxed error the variant's `source()`; adding `{0}` would make every chain walk emit each level twice.

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests` block at the bottom of `relay.rs`:

```rust
    /// A publish failure must carry its whole `source()` chain into the reason string —
    /// `PublishError::Backend`'s own `Display` is the static "backend error" and renders
    /// nothing about what actually failed (`ports.rs`).
    #[test]
    fn describe_error_walks_the_full_source_chain_without_duplicating_levels() {
        #[derive(Debug, thiserror::Error)]
        #[error("transport closed")]
        struct Inner;

        #[derive(Debug, thiserror::Error)]
        #[error("publish failed")]
        struct Outer(#[source] Inner);

        let err = PublishError::from(Box::new(Outer(Inner)) as Box<dyn std::error::Error + Send + Sync>);
        assert_eq!(describe_error(&err), "backend error: publish failed: transport closed");
    }

    #[test]
    fn describe_error_of_a_sourceless_error_is_just_its_display() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope")]
        struct Bare;
        assert_eq!(describe_error(&Bare), "nope");
    }
```

Add `use paigasus_iam_core::PublishError;` to the test module's imports if `super::*` does not already bring it in (the file's top-level `use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};` does not include it — extend that top-level import instead).

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam describe_error
```
Expected: FAIL — `cannot find function describe_error in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add above `row_to_domain_event` in `relay.rs`:

```rust
/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`.
///
/// `PublishError::Backend`'s `Display` is the static string `"backend error"` — thiserror's
/// `#[from]` makes the boxed cause the variant's `source()` rather than part of its message
/// (`paigasus_iam_core::ports`), so `to_string()` alone tells an operator nothing about WHY a
/// publish failed. Since the parked row's `last_error` (SMA-469) and the `error!`/`warn!` lines
/// below all render this string, the chain walk is what makes any of them informative.
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}
```

Then change the publish call at `relay.rs:124` from:

```rust
                Ok(ev) => publisher.publish(&ev).await.map_err(|e| e.to_string()),
```
to:
```rust
                Ok(ev) => publisher.publish(&ev).await.map_err(|e| describe_error(&e)),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/events/relay.rs
git commit -m "fix(rs): render the full publish-error source chain in relay reasons (SMA-469)"
```

---

## Task 2: m0009 — `parked_at` + `last_error` columns

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0009_outbox_dead_letter_columns.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/entities/event_outbox.rs`

**Interfaces:**
- Produces: `event_outbox.parked_at: Option<DateTimeUtc>` and `event_outbox.last_error: Option<String>` on the SeaORM model; indexes `ix_event_outbox_published` and `ix_event_outbox_parked`.

**Why idempotent DDL:** `m0008_partition_audit_log.rs:13-17` documents that SeaORM's migrator does **not** serialize concurrent `up()` across replicas, and m0007 uses `.if_not_exists()` for the same reason. A bare `ADD COLUMN` fails the losing replica's boot.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/outbox_dead_letter_columns_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0009 (SMA-469): `event_outbox` gains `parked_at` + `last_error`, the two retention/DLQ
//! partial indexes exist, and every row already parked at migration time is backfilled with a
//! non-NULL `parked_at` (so it is reachable by both time filters and retention rather than
//! being permanently uncollectable).

mod support;

use sea_orm::{ConnectionTrait, DbBackend, Statement};

#[tokio::test]
async fn m0009_adds_columns_and_partial_indexes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping m0009 test: Docker unavailable");
        return;
    };

    let cols = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'event_outbox'".to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "column_name").unwrap())
        .collect::<Vec<_>>();
    assert!(cols.contains(&"parked_at".to_string()), "missing parked_at: {cols:?}");
    assert!(cols.contains(&"last_error".to_string()), "missing last_error: {cols:?}");

    let idx = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes WHERE tablename = 'event_outbox'".to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String>("", "indexname").unwrap())
        .collect::<Vec<_>>();
    assert!(idx.contains(&"ix_event_outbox_published".to_string()), "missing published index: {idx:?}");
    assert!(idx.contains(&"ix_event_outbox_parked".to_string()), "missing parked index: {idx:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test outbox_dead_letter_columns_pg
```
Expected: FAIL on `missing parked_at` (or skip locally if Docker is down — in that case verify by inspection and rely on CI).

- [ ] **Step 3: Write the migration**

Create `m0009_outbox_dead_letter_columns.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0009 — `event_outbox` gains the dead-letter/retention columns (SMA-469).
//!
//! `parked_at` and `last_error` are both load-bearing, not conveniences:
//! - `parked_at` is what `[outbox.retention].parked_days` measures from. Measuring from
//!   `occurred_at` instead would delete a week-old event on the very tick after it parked.
//!   It is also the axis the dead-letter surface's time filters use.
//! - `last_error` is the parking reason. Before this it existed only in a `tracing::error!`
//!   line, so an operator inspecting the DLQ could not see WHY a row was dead.
//!
//! **Every statement is idempotent, deliberately.** `m0008_partition_audit_log`'s module doc
//! records that SeaORM's migrator does not serialize concurrent `up()` across replicas (m0007
//! uses `.if_not_exists()` for the same reason): a bare `ADD COLUMN` would fail the losing
//! replica of a simultaneous first boot with `column "parked_at" ... already exists`. The
//! `SET LOCAL lock_timeout` mirrors m0008 so the `ACCESS EXCLUSIVE` request backs off rather
//! than queueing ahead of in-flight `PgOutbox::enqueue` writes during a rolling deploy.
//!
//! `CREATE INDEX CONCURRENTLY` is NOT available here — SeaORM runs each migration inside a
//! transaction and `CONCURRENTLY` cannot run in one. The non-concurrent build takes `SHARE` on
//! `event_outbox`, blocking enqueues for its duration; on two partial indexes over a table
//! whose realistic size here is thousands to low millions of rows that is sub-second, and the
//! `lock_timeout` bounds the worst case.
//!
//! **The backfill is deliberate.** Leaving pre-existing parked rows at `parked_at = NULL` would
//! create a permanently uncollectable set: invisible to any time filter (NULL fails every
//! comparison, so bulk replay could never reach them) and permanently ineligible for retention
//! even if `parked_days` were raised. Stamping `now()` says exactly what is true — "we do not
//! know when this parked; it was parked as of the migration" — and starts its retention clock
//! at the migration rather than deleting it instantly.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "event_outbox"
                 ADD COLUMN IF NOT EXISTS parked_at TIMESTAMPTZ NULL,
                 ADD COLUMN IF NOT EXISTS last_error TEXT NULL;"#,
        )
        .await?;
        conn.execute_unprepared(r#"UPDATE "event_outbox" SET parked_at = now() WHERE parked = true AND parked_at IS NULL;"#)
            .await?;
        // Retention's published-sweep predicate.
        conn.execute_unprepared(r#"CREATE INDEX IF NOT EXISTS ix_event_outbox_published ON "event_outbox" (published_at) WHERE published_at IS NOT NULL;"#)
            .await?;
        // The dead-letter list's ordering + keyset paging (`ORDER BY id DESC`, `id < cursor`).
        conn.execute_unprepared(r#"CREATE INDEX IF NOT EXISTS ix_event_outbox_parked ON "event_outbox" (id) WHERE parked = true;"#)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS ix_event_outbox_parked;").await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS ix_event_outbox_published;").await?;
        conn.execute_unprepared(r#"ALTER TABLE "event_outbox" DROP COLUMN IF EXISTS last_error, DROP COLUMN IF EXISTS parked_at;"#)
            .await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Register it and extend the entity**

In `migration/mod.rs`, add `mod m0009_outbox_dead_letter_columns;` after the `pub mod m0008_partition_audit_log;` line, and append `Box::new(m0009_outbox_dead_letter_columns::Migration),` as the last element of the `migrations()` vec.

In `entities/event_outbox.rs`, add two fields at the end of `struct Model` (after `pub parked: bool,`):

```rust
    /// When the relay flipped `parked` to true (SMA-469). `[outbox.retention].parked_days`
    /// measures from this, never from `occurred_at`. NULL only for a row that is not parked.
    pub parked_at: Option<DateTimeUtc>,
    /// The most recent publish-failure reason, rewritten on EVERY failed attempt (not only at
    /// parking) so an operator watching `attempts` climb sees the current cause. Deliberately
    /// preserved across a replay, so a re-parked row keeps the original evidence.
    pub last_error: Option<String>,
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS. Existing tests that build an `event_outbox::Model` literal (`relay.rs`'s `base_model`, `tests/relay_pg.rs`, `tests/outbox_uow_pg.rs`) will fail to compile until the two new fields are added to those literals — add `parked_at: None, last_error: None,` to each.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence rs/crates/services/paigasus-iam/tests
git commit -m "feat(rs): add event_outbox parked_at and last_error columns (SMA-469)"
```

---

## Task 3: The relay records park metadata

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs`

**Interfaces:**
- Consumes: `describe_error` (Task 1), the two new entity fields (Task 2).
- Produces: `fn truncate_error(s: &str) -> String` (module-private).

- [ ] **Step 1: Write the failing test**

Append to `relay.rs`'s `mod tests`:

```rust
    #[test]
    fn truncate_error_leaves_a_short_string_untouched() {
        assert_eq!(truncate_error("boom"), "boom");
    }

    #[test]
    fn truncate_error_bounds_a_long_string_by_bytes_on_a_char_boundary() {
        // 700 four-byte chars = 2800 bytes, comfortably over the 1024-byte bound and past
        // Postgres's ~2KB TOAST threshold — the reason the bound is in BYTES, not chars.
        let long = "😀".repeat(700);
        let out = truncate_error(&long);
        assert!(out.len() <= MAX_ERROR_BYTES + '…'.len_utf8(), "not bounded: {} bytes", out.len());
        assert!(out.ends_with('…'), "expected an elision marker");
        // The prefix must still be valid UTF-8 made of whole chars (String guarantees this;
        // a naive byte slice would have panicked before we got here).
        assert!(out.trim_end_matches('…').chars().all(|c| c == '😀'));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam truncate_error
```
Expected: FAIL — `cannot find function truncate_error`.

- [ ] **Step 3: Write the implementation**

Add near `describe_error` in `relay.rs`:

```rust
/// Byte bound on a stored `last_error` (SMA-469). Deliberately a BYTE bound, not a char count:
/// 1024 four-byte chars would be 4KB, past Postgres's ~2KB TOAST threshold, so a pathological
/// publisher error string could bloat the row it is meant to describe.
const MAX_ERROR_BYTES: usize = 1024;

/// Bounds `s` to [`MAX_ERROR_BYTES`], cutting on a char boundary and marking the elision.
fn truncate_error(s: &str) -> String {
    if s.len() <= MAX_ERROR_BYTES {
        return s.to_string();
    }
    let end = s.char_indices().map(|(i, _)| i).take_while(|i| *i <= MAX_ERROR_BYTES).last().unwrap_or(0);
    format!("{}…", &s[..end])
}
```

Then in `tick`'s `Err(reason)` arm, add the two writes. The arm becomes:

```rust
                Err(reason) => {
                    report.failures += 1;
                    let attempts = row.attempts + 1;
                    active.attempts = Set(attempts);
                    // SMA-469: recorded on EVERY failed attempt, not only at parking — an
                    // operator watching `attempts` climb wants the current reason, and the
                    // dead-letter surface reads this column.
                    active.last_error = Set(Some(truncate_error(&reason)));
                    if attempts >= self.max_attempts {
                        active.parked = Set(true);
                        // `[outbox.retention].parked_days` measures from HERE, never from
                        // `occurred_at` (m0009's module doc).
                        active.parked_at = Set(Some(Utc::now()));
                        report.parked += 1;
                        tracing::error!(id = %row.id, event_type = %row.event_type, attempts, reason = %reason, "outbox event parked after max attempts (poison)");
                    } else {
                        tracing::warn!(id = %row.id, event_type = %row.event_type, attempts, reason = %reason, "outbox event publish failed; will retry");
                    }
                }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS.

- [ ] **Step 5: Extend the existing relay integration test**

In `rs/crates/services/paigasus-iam/tests/relay_pg.rs`, find the scenario-2 test (the `FailingPublisher` poison-parking case) and add assertions after it observes the row parked:

```rust
    // SMA-469: a parked row must carry BOTH the park time (what parked_days measures from)
    // and a descriptive reason (what the dead-letter surface shows).
    assert!(parked_row.parked_at.is_some(), "a parked row must record parked_at");
    let err = parked_row.last_error.clone().expect("a parked row must record last_error");
    assert!(err.contains("always fails"), "last_error must name the real cause, got: {err}");
    assert!(err.starts_with("backend error: "), "expected the source chain to be walked, got: {err}");
```

Adapt the variable name to whatever the existing test binds the re-fetched model to.

- [ ] **Step 6: Run the integration test**

```bash
cd rs && cargo nextest run -p paigasus-iam --test relay_pg
```
Expected: PASS (or skip if Docker is unavailable locally).

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/events/relay.rs rs/crates/services/paigasus-iam/tests/relay_pg.rs
git commit -m "feat(rs): record parked_at and last_error when the relay parks a row (SMA-469)"
```

---

## Task 4: Metric names

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Produces: `names::IAM_OUTBOX_RETENTION_TICKS_TOTAL`, `IAM_OUTBOX_ROWS_DELETED_TOTAL`, `IAM_OUTBOX_PARKED_ROWS`, `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL`, `IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL`.

**Why first:** Tasks 6, 13 and 17 all reference these consts, and `tests/drift.rs` asserts every metric named in a committed dashboard/rule expression appears in `names::ALL`.

- [ ] **Step 1: Add the constants**

After the `IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS` line, add:

```rust
// IAM outbox retention + dead letters (SMA-469)
pub const IAM_OUTBOX_RETENTION_TICKS_TOTAL: &str = "iam_outbox_retention_ticks_total";
pub const IAM_OUTBOX_ROWS_DELETED_TOTAL: &str = "iam_outbox_rows_deleted_total";
/// Current parked-row count — the dead-letter backlog. Refreshed by every
/// `PgOutboxMaintainer` tick, INCLUDING when `[outbox.retention].enabled = false` (the tick
/// still runs for this gauge), so disabling deletion never blinds the backlog alert.
/// Every replica sets the same global count, so this is PER-REPLICA: aggregate it
/// `max by (job)` in alerts and dashboards, never `sum`.
pub const IAM_OUTBOX_PARKED_ROWS: &str = "iam_outbox_parked_rows";
pub const IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL: &str = "iam_outbox_dead_letters_replayed_total";
pub const IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL: &str = "iam_outbox_dead_letters_discarded_total";
```

Then append all five to `ALL`, immediately after `IAM_OUTBOX_OLDEST_UNPUBLISHED_AGE_SECONDS,`.

- [ ] **Step 2: Run the registry tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability && cargo fmt --check
```
Expected: PASS — `all_names_are_unique_and_snake_case` covers the new entries.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): register the outbox retention and dead-letter metric names (SMA-469)"
```

---

## Task 5: `[outbox.retention]` config

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs`

**Interfaces:**
- Produces: `pub struct OutboxRetentionConfig { enabled: bool, interval_secs: u64, published_days: u32, parked_days: u32, batch_size: u64, max_batches_per_tick: u32 }`, reachable as `config.outbox.retention`.

**Semantics to preserve exactly:** `0` means **never** for *both* `published_days` and `parked_days` — one meaning for the sentinel across the block. `enabled = false` disables **deletion only**; the maintainer still ticks to refresh the backlog gauge.

- [ ] **Step 1: Write the failing tests**

In `config.rs`'s `mod tests`, add (place them beside the existing outbox default/validation tests around lines 1824-1942):

```rust
        #[test]
        fn outbox_retention_defaults() {
            let cfg = load_minimal_config();
            assert!(cfg.outbox.retention.enabled, "retention must default to enabled");
            assert_eq!(cfg.outbox.retention.interval_secs, 3600);
            assert_eq!(cfg.outbox.retention.published_days, 7);
            assert_eq!(cfg.outbox.retention.parked_days, 0, "parked rows must NOT age out by default");
            assert_eq!(cfg.outbox.retention.batch_size, 1000);
            assert_eq!(cfg.outbox.retention.max_batches_per_tick, 50);
        }

        #[test]
        fn outbox_retention_rejects_zero_interval_batch_and_max_batches() {
            for mutate in [
                (|c: &mut IamConfig| c.outbox.retention.interval_secs = 0) as fn(&mut IamConfig),
                |c: &mut IamConfig| c.outbox.retention.batch_size = 0,
                |c: &mut IamConfig| c.outbox.retention.max_batches_per_tick = 0,
            ] {
                let mut cfg = load_minimal_config();
                mutate(&mut cfg);
                assert!(cfg.validate().is_err(), "expected a zero retention knob to fail validation");
            }
        }

        #[test]
        fn outbox_retention_allows_zero_day_windows_meaning_never() {
            let mut cfg = load_minimal_config();
            cfg.outbox.retention.published_days = 0;
            cfg.outbox.retention.parked_days = 0;
            assert!(cfg.validate().is_ok(), "0 days must be valid — it is the 'never delete' sentinel");
        }
```

Reuse whatever helper the surrounding tests already use to build a valid `IamConfig` (they use a figment `Jail` with a minimal TOML — copy that shape and name the helper accordingly rather than inventing `load_minimal_config` if a differently-named one already exists).

- [ ] **Step 2: Run tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam outbox_retention
```
Expected: FAIL — `no field retention on type OutboxConfig`.

- [ ] **Step 3: Add the config type**

Add after `OutboxConfig`'s definition:

```rust
/// `event_outbox` retention (SMA-469) — the knobs for
/// [`PgOutboxMaintainer`](crate::adapters::persistence::PgOutboxMaintainer), the background
/// sweep that bounds the outbox's growth. Nests under `[outbox]` exactly as
/// [`RetentionConfig`] nests under `[audit]`; every field has a default, so an absent
/// `[outbox.retention]` block is valid config.
///
/// **`0` means "never" for BOTH day windows** — one meaning for the sentinel across the whole
/// block, deliberately: two different readings of `0` inside one table would be a trap.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OutboxRetentionConfig {
    /// When `false`, the maintainer performs NO deletions — but it is still spawned and still
    /// ticks, because the tick is what refreshes `iam_outbox_parked_rows`. Gating the SPAWN on
    /// this would mean an operator who sets `enabled = false` (a plausible "stop deleting
    /// things" reaction during an incident) silently loses the dead-letter backlog signal
    /// while the relay keeps parking rows.
    pub enabled: bool,
    /// Seconds between sweep ticks. Validated non-zero.
    pub interval_secs: u64,
    /// Delete published rows older than this many days. `0` = never delete published rows.
    pub published_days: u32,
    /// Delete parked rows whose `parked_at` is older than this many days. `0` = never (the
    /// default) — auto-deleting the very thing an operator is alerted to inspect must be a
    /// deliberate choice, mirroring `audit.retention.committed_months`. A non-zero value
    /// triggers a startup `warn!`.
    pub parked_days: u32,
    /// Rows deleted per pass. Validated non-zero.
    pub batch_size: u64,
    /// Passes per tick, so one tick retires at most `batch_size * this` rows and a huge first
    /// sweep resumes next tick instead of holding one tick open. Config rather than a constant
    /// because it is exactly as much an operational knob as `batch_size`: at the defaults a
    /// deployment draining a 10M-row backlog needs ~8 days, and the operator doing that
    /// drain must be able to raise it. Validated non-zero.
    pub max_batches_per_tick: u32,
}

impl Default for OutboxRetentionConfig {
    fn default() -> Self {
        OutboxRetentionConfig {
            enabled: true,
            interval_secs: 3_600,
            published_days: 7,
            parked_days: 0,
            batch_size: 1_000,
            max_batches_per_tick: 50,
        }
    }
}
```

Add the field to `OutboxConfig`:

```rust
    /// Retention for the table the relay drains — see [`OutboxRetentionConfig`].
    #[serde(default)]
    pub retention: OutboxRetentionConfig,
```

Add it to `OutboxDefaults` (mirroring how `AuditDefaults` carries `retention: RetentionConfig`):

```rust
    retention: OutboxRetentionConfig,
```

and set `retention: OutboxRetentionConfig::default(),` in whatever constructs `OutboxDefaults` (mirror the `audit` block's construction exactly).

In `IamConfig::validate`, after the existing `outbox.max_attempts` check:

```rust
        if self.outbox.retention.interval_secs == 0 {
            return Err("outbox.retention.interval_secs must be at least 1 (0 would busy-loop the sweep)".to_string());
        }
        if self.outbox.retention.batch_size == 0 {
            return Err("outbox.retention.batch_size must be at least 1 (0 would make every sweep pass delete nothing)".to_string());
        }
        if self.outbox.retention.max_batches_per_tick == 0 {
            return Err("outbox.retention.max_batches_per_tick must be at least 1 (0 would make every sweep tick do no passes)".to_string());
        }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS. `tests/support/mod.rs` builds an `OutboxConfig` literal — add `retention: OutboxRetentionConfig::default(),` there and import the type.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs rs/crates/services/paigasus-iam/tests/support/mod.rs
git commit -m "feat(rs): add the outbox.retention config block (SMA-469)"
```

---

## Task 6: `PgOutboxMaintainer`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox_maintainer.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`

**Interfaces:**
- Consumes: `names::IAM_OUTBOX_RETENTION_TICKS_TOTAL`, `IAM_OUTBOX_ROWS_DELETED_TOTAL`, `IAM_OUTBOX_PARKED_ROWS` (Task 4); the `parked_at` column (Task 2).
- Produces:
  - `pub struct OutboxRetentionPolicy { pub enabled: bool, pub published_days: u32, pub parked_days: u32, pub batch_size: u64, pub max_batches_per_tick: u32 }` (`Copy`)
  - `pub struct SweepReport { pub deleted_published: u64, pub deleted_parked: u64, pub passes_published: u32, pub passes_parked: u32, pub parked_rows: u64, pub errored: bool }`
  - `pub struct PgOutboxMaintainer` with `new(db) -> Self`, `async fn tick(&self, now: DateTime<Utc>, policy: OutboxRetentionPolicy) -> SweepReport`, `async fn run<S: Future<Output = ()> + Send>(self, policy, interval: Duration, shutdown: S)`
  - Re-exported from `persistence::{PgOutboxMaintainer, OutboxRetentionPolicy, SweepReport}`

- [ ] **Step 1: Write the failing unit test**

Create the file with only its `mod tests` populated first — but the two pure helpers under test are `cutoff` and the SQL builders, which need no DB. Write:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_sweep_sql_excludes_parked_rows_and_bounds_the_batch() {
        let sql = published_sweep_sql();
        assert!(sql.contains("published_at IS NOT NULL"), "{sql}");
        // Redundant TODAY (the relay sets published_at or parked, never both) but nothing
        // ENFORCES that, and `replay_in` is now a second writer of these columns. §3.3's
        // promise that parked rows never age out by default must hold structurally.
        assert!(sql.contains("parked = false"), "published sweep must never touch a parked row: {sql}");
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
        assert!(sql.contains("LIMIT $2"), "{sql}");
    }

    #[test]
    fn parked_sweep_sql_requires_a_known_park_time() {
        let sql = parked_sweep_sql();
        assert!(sql.contains("parked = true"), "{sql}");
        assert!(sql.contains("parked_at IS NOT NULL"), "a row with an unknown park time must never be swept: {sql}");
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
    }

    #[test]
    fn cutoff_subtracts_whole_days() {
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(cutoff(now, 7), DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z").unwrap().with_timezone(&Utc));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam sweep_sql
```
Expected: FAIL — module does not exist / functions not found.

- [ ] **Step 3: Write the implementation**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `PgOutboxMaintainer` (SMA-469): the background sweep that bounds `event_outbox`'s growth.
//!
//! Mirrors `PgPartitionMaintainer` (`pg_partition_maintainer`, SMA-467): a `tick` does one unit
//! of work and returns a report; `run` is the `tokio::select!` shutdown-watch loop the
//! composition root spawns. It is deliberately NOT folded into `OutboxRelay::tick` — that
//! would couple a 5-second hot loop to an hourly bulk `DELETE`, make tick latency lumpy, and
//! (decisively) muddy `iam_outbox_relay_ticks_total`, so a retention failure would red the
//! relay's own liveness signal.
//!
//! **Retention and the relay cannot contend, by construction.** The relay's poll predicate is
//! `published_at IS NULL AND parked = false`; both sweep predicates are subsets of its exact
//! complement, so no row is ever visible to both. That claim covers relay-vs-retention ONLY —
//! it says nothing about replay vs. retention or replay vs. replay, which `PgDeadLetters`
//! handles with its own `FOR UPDATE SKIP LOCKED`.
//!
//! `SKIP LOCKED` here lets two maintainer replicas partition the work rather than block. Note
//! the consequence: a pass can return fewer than `batch_size` rows because a PEER replica
//! holds them, so `passes < max_batches_per_tick` does NOT prove the backlog is drained. The
//! next tick resumes.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, Value};

/// The sweep knobs a tick needs, decoupled from `config::OutboxRetentionConfig`
/// (`interval_secs` lives in the loop, not a tick). `Copy` so tests/`run` pass it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxRetentionPolicy {
    /// `false` = perform no deletions. The tick still runs, because it is what refreshes the
    /// `iam_outbox_parked_rows` backlog gauge.
    pub enabled: bool,
    /// Delete published rows older than this. `0` = never.
    pub published_days: u32,
    /// Delete parked rows whose `parked_at` is older than this. `0` = never.
    pub parked_days: u32,
    pub batch_size: u64,
    pub max_batches_per_tick: u32,
}

/// Per-tick outcome, returned (and logged) so tests can assert without scraping logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted_published: u64,
    pub deleted_parked: u64,
    /// Delete passes actually issued. Exists so a test can prove BATCHING happened — with
    /// totals alone, one pass of 2000 and two passes of 1000 are indistinguishable.
    pub passes_published: u32,
    pub passes_parked: u32,
    pub parked_rows: u64,
    pub errored: bool,
}

/// `now` shifted back by `days`.
fn cutoff(now: DateTime<Utc>, days: u32) -> DateTime<Utc> {
    now - chrono::Duration::days(i64::from(days))
}

/// `$1` = cutoff timestamp, `$2` = batch size.
fn published_sweep_sql() -> &'static str {
    r#"DELETE FROM "event_outbox" WHERE id IN (
         SELECT id FROM "event_outbox"
         WHERE published_at IS NOT NULL AND published_at < $1 AND parked = false
         ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
       )"#
}

/// `$1` = cutoff timestamp, `$2` = batch size.
fn parked_sweep_sql() -> &'static str {
    r#"DELETE FROM "event_outbox" WHERE id IN (
         SELECT id FROM "event_outbox"
         WHERE parked = true AND parked_at IS NOT NULL AND parked_at < $1
         ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
       )"#
}

#[derive(Clone)]
pub struct PgOutboxMaintainer {
    db: DatabaseConnection,
}

impl PgOutboxMaintainer {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgOutboxMaintainer { db }
    }

    /// One maintenance unit: sweep published, then (independently) sweep parked, then always
    /// refresh the backlog gauge. Errors are logged + counted, never propagated — the loop
    /// keeps running, exactly like `PgPartitionMaintainer::tick`.
    pub async fn tick(&self, now: DateTime<Utc>, policy: OutboxRetentionPolicy) -> SweepReport {
        let mut report = SweepReport::default();

        if policy.enabled && policy.published_days > 0 {
            match self.sweep(published_sweep_sql(), cutoff(now, policy.published_days), policy).await {
                Ok((n, passes)) => {
                    report.deleted_published = n;
                    report.passes_published = passes;
                }
                Err((n, passes, e)) => {
                    // A pass that errors aborts only ITS OWN step's loop; the rows already
                    // deleted are still reported.
                    report.deleted_published = n;
                    report.passes_published = passes;
                    report.errored = true;
                    tracing::warn!(error = %e, "outbox published-row sweep failed; will retry next tick");
                }
            }
        }

        // Runs regardless of the published sweep's outcome (independence — one step failing
        // must not wedge the other, mirroring `PgPartitionMaintainer`'s prune-after-create).
        if policy.enabled && policy.parked_days > 0 {
            match self.sweep(parked_sweep_sql(), cutoff(now, policy.parked_days), policy).await {
                Ok((n, passes)) => {
                    report.deleted_parked = n;
                    report.passes_parked = passes;
                }
                Err((n, passes, e)) => {
                    report.deleted_parked = n;
                    report.passes_parked = passes;
                    report.errored = true;
                    tracing::warn!(error = %e, "outbox parked-row sweep failed; will retry next tick");
                }
            }
        }

        // ALWAYS — including when `enabled = false`. This gauge is the dead-letter backlog
        // signal, and losing it because deletion was paused would blind the operator exactly
        // when they are most likely to have paused it.
        match self.parked_row_count().await {
            Ok(n) => {
                report.parked_rows = n;
                gauge!(names::IAM_OUTBOX_PARKED_ROWS).set(n as f64);
            }
            Err(e) => {
                report.errored = true;
                tracing::warn!(error = %e, "outbox parked-row gauge query failed");
            }
        }

        counter!(names::IAM_OUTBOX_RETENTION_TICKS_TOTAL, "result" => if report.errored { "error" } else { "ok" }).increment(1);
        counter!(names::IAM_OUTBOX_ROWS_DELETED_TOTAL, "reason" => "published").increment(report.deleted_published);
        counter!(names::IAM_OUTBOX_ROWS_DELETED_TOTAL, "reason" => "parked").increment(report.deleted_parked);
        tracing::info!(
            deleted_published = report.deleted_published,
            deleted_parked = report.deleted_parked,
            passes_published = report.passes_published,
            passes_parked = report.passes_parked,
            parked_rows = report.parked_rows,
            errored = report.errored,
            "outbox retention tick"
        );
        report
    }

    /// Batched delete: repeat `sql` until a pass affects fewer than `batch_size` rows or
    /// `max_batches_per_tick` is reached. Each pass is its OWN autocommit statement — never one
    /// long transaction holding locks across the whole sweep. On error, returns what was
    /// deleted before it, so a partial sweep is still reported honestly.
    async fn sweep(&self, sql: &str, cutoff: DateTime<Utc>, policy: OutboxRetentionPolicy) -> Result<(u64, u32), (u64, u32, DbErr)> {
        let mut total = 0u64;
        let mut passes = 0u32;
        while passes < policy.max_batches_per_tick {
            let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, [Value::from(cutoff), Value::from(policy.batch_size as i64)]);
            let affected = match self.db.execute(stmt).await {
                Ok(r) => r.rows_affected(),
                Err(e) => return Err((total, passes, e)),
            };
            passes += 1;
            total += affected;
            if affected < policy.batch_size {
                break;
            }
        }
        Ok((total, passes))
    }

    async fn parked_row_count(&self) -> Result<u64, DbErr> {
        let stmt = Statement::from_string(DbBackend::Postgres, r#"SELECT count(*) AS n FROM "event_outbox" WHERE parked = true"#.to_string());
        let n = self.db.query_one(stmt).await?.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0);
        Ok(n.max(0) as u64)
    }

    /// The shutdown-watch loop (mirrors `PgPartitionMaintainer::run`/`OutboxRelay::run`):
    /// sleep `interval`, tick, repeat until `shutdown` resolves.
    pub async fn run<S>(self, policy: OutboxRetentionPolicy, interval: Duration, shutdown: S)
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
```

Then append the `mod tests` block from Step 1.

- [ ] **Step 4: Export it**

In `persistence/mod.rs`, add `pub mod pg_outbox_maintainer;` (alphabetically, after `pg_outbox`) and `pub use pg_outbox_maintainer::{OutboxRetentionPolicy, PgOutboxMaintainer, SweepReport};`.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence
git commit -m "feat(rs): add PgOutboxMaintainer for batched outbox retention (SMA-469)"
```

---

## Task 7: Sweep integration tests

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/outbox_retention_pg.rs`

**Interfaces:**
- Consumes: `PgOutboxMaintainer`, `OutboxRetentionPolicy`, `SweepReport` (Task 6); `support::start_migrated_postgres`.

- [ ] **Step 1: Write the tests**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgOutboxMaintainer` (SMA-469) against real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — the same
//! gating pattern as `tests/relay_pg.rs`.

mod support;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{OutboxRetentionPolicy, PgOutboxMaintainer};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, Set};
use uuid::Uuid;

fn policy(published_days: u32, parked_days: u32) -> OutboxRetentionPolicy {
    OutboxRetentionPolicy {
        enabled: true,
        published_days,
        parked_days,
        batch_size: 1000,
        max_batches_per_tick: 50,
    }
}

/// Seeds one row in a chosen lifecycle state. `published_at`/`parked_at` are set explicitly so
/// a test can age a row without waiting.
async fn seed(db: &DatabaseConnection, id: u128, published_at: Option<DateTime<Utc>>, parked: bool, parked_at: Option<DateTime<Utc>>) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(published_at),
        attempts: Set(0),
        parked: Set(parked),
        parked_at: Set(parked_at),
        last_error: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

async fn exists(db: &DatabaseConnection, id: Uuid) -> bool {
    event_outbox::Entity::find_by_id(id).one(db).await.unwrap().is_some()
}

#[tokio::test]
async fn sweeps_aged_published_rows_and_leaves_live_and_parked_rows_alone() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    let aged = seed(&db, 1, Some(now - ChronoDuration::days(30)), false, None).await;
    let fresh = seed(&db, 2, Some(now - ChronoDuration::hours(1)), false, None).await;
    let live = seed(&db, 3, None, false, None).await;
    let parked = seed(&db, 4, None, true, Some(now - ChronoDuration::days(30))).await;

    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(7, 0)).await;

    assert!(!report.errored, "tick reported an error");
    assert_eq!(report.deleted_published, 1);
    assert_eq!(report.deleted_parked, 0, "parked_days = 0 must never delete a parked row");
    assert!(!exists(&db, aged).await, "the aged published row should have been swept");
    assert!(exists(&db, fresh).await, "a published row inside the window must survive");
    assert!(exists(&db, live).await, "an undrained row must never be swept");
    assert!(exists(&db, parked).await, "a parked row must survive parked_days = 0");
    assert_eq!(report.parked_rows, 1, "the backlog gauge must count the parked row");
}

#[tokio::test]
async fn sweeps_aged_parked_rows_only_when_parked_days_is_set_and_park_time_is_known() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    let aged_parked = seed(&db, 10, None, true, Some(now - ChronoDuration::days(60))).await;
    let fresh_parked = seed(&db, 11, None, true, Some(now - ChronoDuration::days(1))).await;
    // A row parked with an UNKNOWN park time must never be swept — m0009 backfills these, so
    // this state should be unreachable in production, which is exactly why the guard is tested.
    let unknown_parked = seed(&db, 12, None, true, None).await;

    let report = PgOutboxMaintainer::new(db.clone()).tick(now, policy(0, 30)).await;

    assert!(!report.errored);
    assert_eq!(report.deleted_parked, 1);
    assert_eq!(report.deleted_published, 0, "published_days = 0 must never delete a published row");
    assert!(!exists(&db, aged_parked).await);
    assert!(exists(&db, fresh_parked).await);
    assert!(exists(&db, unknown_parked).await, "a parked row with NULL parked_at must never be swept");
}

#[tokio::test]
async fn disabled_retention_deletes_nothing_but_still_refreshes_the_backlog_gauge() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    let aged = seed(&db, 20, Some(now - ChronoDuration::days(30)), false, None).await;
    seed(&db, 21, None, true, Some(now - ChronoDuration::days(90))).await;

    let mut p = policy(7, 30);
    p.enabled = false;
    let report = PgOutboxMaintainer::new(db.clone()).tick(now, p).await;

    assert!(!report.errored);
    assert_eq!(report.deleted_published, 0);
    assert_eq!(report.deleted_parked, 0);
    assert!(exists(&db, aged).await, "enabled = false must delete nothing");
    assert_eq!(report.parked_rows, 1, "the gauge must still be refreshed when deletion is disabled");
}

#[tokio::test]
async fn honors_batch_size_and_max_batches_per_tick_across_passes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox retention test: Docker unavailable");
        return;
    };
    let now = Utc::now();
    for i in 0..10u128 {
        seed(&db, 100 + i, Some(now - ChronoDuration::days(30)), false, None).await;
    }

    let maintainer = PgOutboxMaintainer::new(db.clone());

    // batch_size 3, capped at 2 passes => exactly 6 rows this tick, in 2 passes.
    let capped = OutboxRetentionPolicy {
        enabled: true,
        published_days: 7,
        parked_days: 0,
        batch_size: 3,
        max_batches_per_tick: 2,
    };
    let first = maintainer.tick(now, capped).await;
    assert_eq!(first.deleted_published, 6, "cap must bound a tick to batch_size * max_batches");
    assert_eq!(first.passes_published, 2, "batching must actually happen, not one big delete");
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), 4);

    // A subsequent tick resumes and drains the rest.
    let second = maintainer.tick(now, capped).await;
    assert_eq!(second.deleted_published, 4);
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), 0);
}
```

- [ ] **Step 2: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test outbox_retention_pg
```
Expected: PASS (or a clean skip if Docker is unavailable — in that case start Docker and re-run before committing, since these are the primary proof for Task 6).

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/outbox_retention_pg.rs
git commit -m "test(rs): cover the outbox retention sweep against postgres (SMA-469)"
```

---

## Task 8: Concurrent relay-vs-sweep disjointness proof

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/outbox_retention_concurrency_pg.rs`

**Why a separate test file:** the risk table claims retention "cannot contend with the relay by construction". Task 7's assertions are all against statically seeded rows, which cannot prove a *concurrency* claim. This proves it by holding a relay-style `FOR UPDATE` lock open across a real sweep — the same hold-open technique `tests/relay_pg.rs` already uses for its `SKIP LOCKED` scenario.

- [ ] **Step 1: Read the existing hold-open pattern**

```bash
grep -n -B 20 -A 40 "SKIP LOCKED" rs/crates/services/paigasus-iam/tests/relay_pg.rs
```
Mirror whatever it does to open a second connection and hold a transaction open (it begins a transaction on a separate `DatabaseConnection` and issues a `SELECT ... FOR UPDATE` without committing until after the relay tick).

- [ ] **Step 2: Write the test**

```rust
// SPDX-License-Identifier: Apache-2.0

//! The §6.3 disjointness claim, proven CONCURRENTLY (SMA-469).
//!
//! `PgOutboxMaintainer`'s sweep predicates are subsets of the exact complement of the relay's
//! poll predicate (`published_at IS NULL AND parked = false`), so no row is ever visible to
//! both. `tests/outbox_retention_pg.rs` asserts that against statically seeded rows, which
//! cannot prove a claim about concurrency. This holds a relay-style `FOR UPDATE` lock open
//! across a real sweep tick and asserts the sweep neither blocks on it nor deletes the row —
//! the same hold-open technique `tests/relay_pg.rs` uses for its own `SKIP LOCKED` scenario.

mod support;

use chrono::{Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{OutboxRetentionPolicy, PgOutboxMaintainer};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, DbBackend, EntityTrait, Set, Statement, TransactionTrait};
use uuid::Uuid;

#[tokio::test]
async fn a_sweep_neither_blocks_on_nor_deletes_a_row_the_relay_holds_locked() {
    let Some((node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping outbox concurrency test: Docker unavailable");
        return;
    };
    let now = Utc::now();

    // The row the "relay" is mid-tick on: unpublished, unparked — invisible to both sweeps.
    let live = Uuid::from_u128(1);
    // The row retention is entitled to delete, seeded aged-published.
    let aged = Uuid::from_u128(2);
    for (id, published) in [(live, None), (aged, Some(now - ChronoDuration::days(30)))] {
        event_outbox::ActiveModel {
            id: Set(id),
            occurred_at: Set(now),
            event_type: Set("iam.principal.created".to_string()),
            schema_version: Set(1),
            aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
            actor_prn: Set(None),
            payload: Set(serde_json::json!({}).to_string()),
            correlation_id: Set(None),
            published_at: Set(published),
            attempts: Set(0),
            parked: Set(false),
            parked_at: Set(None),
            last_error: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    // A SECOND connection holds the relay's row lock open for the whole sweep.
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let holder = Database::connect(format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let held = holder.begin().await.unwrap();
    held.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"SELECT id FROM "event_outbox" WHERE id = '{live}' FOR UPDATE"#),
    ))
    .await
    .unwrap();

    // The sweep must complete promptly — if it blocked on the held lock this would hang until
    // the test timeout rather than returning.
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        PgOutboxMaintainer::new(db.clone()).tick(
            now,
            OutboxRetentionPolicy {
                enabled: true,
                published_days: 7,
                parked_days: 0,
                batch_size: 100,
                max_batches_per_tick: 10,
            },
        ),
    )
    .await
    .expect("the sweep blocked on a row the relay holds locked — the predicates are not disjoint");

    assert!(!report.errored);
    assert_eq!(report.deleted_published, 1, "the aged published row is still swept while the relay holds another row");

    held.rollback().await.unwrap();

    assert!(event_outbox::Entity::find_by_id(live).one(&db).await.unwrap().is_some(), "the relay's in-flight row must never be swept");
    assert!(event_outbox::Entity::find_by_id(aged).one(&db).await.unwrap().is_none());
}
```

- [ ] **Step 3: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test outbox_retention_concurrency_pg
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/outbox_retention_concurrency_pg.rs
git commit -m "test(rs): prove the retention sweep never contends with the relay (SMA-469)"
```

---

## Task 9: Cedar actions

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs`

**Interfaces:**
- Produces: `Action::ListOutboxDeadLetters` (read), `Action::ReplayOutboxDeadLetter` (write), `Action::DiscardOutboxDeadLetter` (write), each with `as_wire()` equal to its Rust name.

**Ordering matters:** `Action::ALL`'s doc says "in schema-declaration order". Insert all three in the **same position in both** files — after `ListAuditLog`, before `InvokeModel`.

**Known consequence, deliberately accepted:** `forbid_archived_writes_source()` (`roles.rs:262-266`) generates its action list from `Action::ALL.filter(is_write && !is_restore)`, so two new write actions change that generated starter-policy source. `reconcile_starter` (`bootstrap.rs:79-84`) compares and warns without overwriting, so pre-existing databases will log `"starter policy drift"` at every boot. This is tracked as SMA-477 and does **not** block this work; Task 18 documents the operator remediation. Do **not** misclassify replay/discard as reads to dodge it.

- [ ] **Step 1: Write the failing test**

In `action.rs`'s `mod tests`, add:

```rust
    #[test]
    fn outbox_dead_letter_actions_are_classified_and_round_trip() {
        assert!(!Action::ListOutboxDeadLetters.is_write(), "listing dead letters is a read");
        assert!(Action::ReplayOutboxDeadLetter.is_write(), "replay mutates the outbox");
        assert!(Action::DiscardOutboxDeadLetter.is_write(), "discard deletes a row");
        for a in [Action::ListOutboxDeadLetters, Action::ReplayOutboxDeadLetter, Action::DiscardOutboxDeadLetter] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
        // None of the three is a restore, so all three land in the generated
        // `forbid_archived_writes` list (harmless: they are Root-scoped and `Root` has no
        // `effective_status` attribute, so the clause can never match them).
        assert!(!Action::ReplayOutboxDeadLetter.is_restore());
    }
```

Update the existing count assertion in `all_covers_every_variant`:

```rust
        assert_eq!(Action::ALL.len(), 39, "27 pre-existing + 7 M4 + 1 audit + 1 invoke-model + 3 outbox dead-letter");
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core outbox_dead_letter_actions
```
Expected: FAIL — no variant `ListOutboxDeadLetters`.

- [ ] **Step 3: Add the variants**

In `action.rs`, make four edits, each inserting after the `ListAuditLog` entry and before `InvokeModel`:

1. `enum Action`: add `ListOutboxDeadLetters,`, `ReplayOutboxDeadLetter,`, `DiscardOutboxDeadLetter,`.
2. `ALL`: add `Action::ListOutboxDeadLetters,`, `Action::ReplayOutboxDeadLetter,`, `Action::DiscardOutboxDeadLetter,`.
3. `as_wire`: add the three `Action::X => "X",` arms.
4. The `is_write` `false` arm: add `| Action::ListOutboxDeadLetters` to the existing read-action chain that already ends `| Action::ListAuditLog => false,`. The two write actions need no edit if `is_write` has a catch-all `_ => true`; if it enumerates writes explicitly, add them to that arm instead — read the function before editing.
5. The exhaustiveness `match` inside `all_covers_every_variant`: add all three to the `=> {}` arm.

Also confirm `Action::parse` is derived from `as_wire` (it is — check before assuming); if it has its own match, add three arms there too.

In `schema.rs`, extend `SCHEMA_SRC`'s action declaration so the tail reads:

```
         IssueApiKey, RevokeApiKey, ListApiKeys, ListAuditLog, ListOutboxDeadLetters,
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, InvokeModel
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam-core -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam-core --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS. If a starter-policy snapshot test in `roles.rs` asserts the exact generated `forbid` source, update its expected string — that change is the intended consequence documented above.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz
git commit -m "feat(rs): add root-scoped outbox dead-letter cedar actions (SMA-469)"
```

---

## Task 10: Core dead-letter types + `DeadLetters` port

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/dead_letter.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs`

**Interfaces:**
- Produces: `DeadLetterEntry`, `DeadLetterFilter` (with `MAX_LIMIT: u64 = 200`, `capped_limit()`), `BulkReplayRequest` (with `MAX_BULK_REPLAY: u64 = 10_000`, `capped_max_rows()`, `is_valid()`), and the `DeadLetters` port. All re-exported from the crate root.

- [ ] **Step 1: Write the failing test**

Put this at the bottom of the new file (write the test first, then the types above it):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_limit_is_clamped_like_the_audit_filter() {
        let f = |limit| DeadLetterFilter {
            event_type: None,
            parked_from: None,
            parked_to: None,
            cursor: None,
            limit,
        };
        assert_eq!(f(0).capped_limit(), 1, "a zero limit floors at 1");
        assert_eq!(f(50).capped_limit(), 50);
        assert_eq!(f(10_000).capped_limit(), DeadLetterFilter::MAX_LIMIT);
    }

    #[test]
    fn bulk_replay_requires_an_explicit_max_rows_and_is_capped() {
        let r = |max_rows| BulkReplayRequest {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows,
        };
        // A missing/zero max_rows is invalid: the required, explicit blast radius IS the guard.
        assert!(!r(0).is_valid(), "max_rows = 0 must be rejected, not treated as unlimited");
        assert!(r(1).is_valid());
        assert_eq!(r(1_000_000).capped_max_rows(), BulkReplayRequest::MAX_BULK_REPLAY);
        assert_eq!(r(500).capped_max_rows(), 500);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core bulk_replay
```
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the types**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Dead-letter value types and the `DeadLetters` port (SMA-469): the operator-facing view of
//! `event_outbox` rows the relay parked (`parked = true`), plus the operations that retire
//! them. Pure/kernel-friendly — ids and timestamps are injected by the caller.

use crate::ports::{RepositoryError, Transaction};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A parked `event_outbox` row, projected for inspection.
///
/// `event_type`, `payload`, and `schema_version` are deliberately RAW — a wire `String`, a
/// serialized-TEXT `String`, and the stored `i32` — rather than `EventType`,
/// `serde_json::Value`, and `u16`. All three of the relay's malformed-row rejection reasons
/// are an unrecognized `event_type` wire string, invalid `payload` JSON, and an out-of-range
/// `schema_version`; i.e. all three are reasons a row PARKS. Typing any of them strictly would
/// make the dead-letter surface unable to display exactly the rows it exists to explain. This
/// is a diagnostic projection of a persisted row, not a domain type.
///
/// `attempts` is a plain `u32` by contrast — it is a count, never negative, and never a park
/// reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,
    pub correlation_id: Option<Uuid>,
    pub attempts: u32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Listing + keyset paging. `parked_from`/`parked_to` filter `parked_at` — NOT `occurred_at`.
/// The operationally meaningful question is "what parked during last night's outage", which
/// `occurred_at` cannot answer; the fields are named for the column so no call site can be
/// ambiguous about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterFilter {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,
}

impl DeadLetterFilter {
    /// Mirrors `AuditFilter::MAX_LIMIT`.
    pub const MAX_LIMIT: u64 = 200;
    pub fn capped_limit(&self) -> u64 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

/// Bulk replay. A SEPARATE type from [`DeadLetterFilter`], deliberately: reusing the paging
/// filter would put its `MAX_LIMIT` (200) in direct contradiction with `MAX_BULK_REPLAY`
/// (10_000) and leave `cursor` meaningless on a path that does not page.
///
/// `max_rows` is REQUIRED and is the guard. An "at least one filter field must be present"
/// check was considered and rejected: `parked_from = 1970-01-01T00:00:00Z` satisfies it while
/// matching every row, and that is the most natural way an operator writes "replay
/// everything". An explicit row budget cannot be satisfied by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkReplayRequest {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub max_rows: u64,
}

impl BulkReplayRequest {
    pub const MAX_BULK_REPLAY: u64 = 10_000;
    /// `false` when `max_rows` is absent/zero — the caller must state its blast radius.
    pub fn is_valid(&self) -> bool {
        self.max_rows > 0
    }
    pub fn capped_max_rows(&self) -> u64 {
        self.max_rows.min(Self::MAX_BULK_REPLAY)
    }
}

/// Inspect and retire parked outbox rows.
///
/// The three mutating methods take the CALLER's transaction (like `Outbox::enqueue`) so the
/// mutation and its audit entry commit atomically on one `UnitOfWork`. They return the affected
/// row (or a count) rather than a bare bool so the caller can build a complete audit entry —
/// for `discard_in` that entry is the discarded event's ONLY remaining trace.
#[async_trait]
pub trait DeadLetters: Send + Sync {
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError>;
    /// `None` when no PARKED row has that id (absent, live, or already published/discarded).
    async fn replay_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
    /// Returns how many rows were un-parked.
    async fn replay_matching_in(&self, tx: &dyn Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError>;
    /// `None` when no PARKED row has that id.
    async fn discard_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
}
```

In `lib.rs`, add `pub mod dead_letter;` (alphabetically, after `pub mod authz;`) and
`pub use dead_letter::{BulkReplayRequest, DeadLetterEntry, DeadLetterFilter, DeadLetters};`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam-core && cargo clippy -p paigasus-iam-core --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src
git commit -m "feat(rs): add the dead-letter core types and DeadLetters port (SMA-469)"
```

---

## Task 11: `PgDeadLetters` adapter

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_dead_letters.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`

**Interfaces:**
- Consumes: `DeadLetters`, `DeadLetterEntry`, `DeadLetterFilter`, `BulkReplayRequest` (Task 10); `uow::recover_txn`; `persistence::map_err`.
- Produces: `pub struct PgDeadLetters { db: DatabaseConnection }` with `new(db) -> Self`, implementing `DeadLetters`. Re-exported as `persistence::PgDeadLetters`.

- [ ] **Step 1: Write the failing test**

The DB-touching behavior is covered by Task 12; what is unit-testable here is the dynamic filter SQL. At the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_sql_always_scopes_to_parked_rows() {
        let mut params: Vec<Value> = Vec::new();
        let sql = filter_clauses(&None, &None, &None, &mut params);
        assert_eq!(sql, "parked = true", "an unfiltered request must still be scoped to parked rows");
        assert!(params.is_empty());
    }

    #[test]
    fn filter_sql_binds_each_present_field_positionally() {
        let mut params: Vec<Value> = Vec::new();
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let sql = filter_clauses(&Some("iam.principal.created".to_string()), &Some(from), &Some(to), &mut params);
        assert_eq!(sql, "parked = true AND event_type = $1 AND parked_at >= $2 AND parked_at <= $3");
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn filter_sql_numbers_params_from_the_existing_offset() {
        // The caller may have already bound values (the bulk UPDATE binds none, but `list`
        // binds its cursor first) — placeholders must continue the sequence, not restart it.
        let mut params: Vec<Value> = vec![Value::from(1i64)];
        let sql = filter_clauses(&Some("x".to_string()), &None, &None, &mut params);
        assert_eq!(sql, "parked = true AND event_type = $2");
    }

    #[test]
    fn every_mutating_statement_is_scoped_to_parked_rows() {
        // A live or already-published row must be untouchable through this surface.
        assert!(REPLAY_ONE_SQL.contains("parked = true"), "{REPLAY_ONE_SQL}");
        assert!(DISCARD_ONE_SQL.contains("parked = true"), "{DISCARD_ONE_SQL}");
        // Replay must NOT clear last_error: a re-parked row would otherwise lose the original
        // evidence and show only the second failure.
        assert!(!REPLAY_ONE_SQL.contains("last_error = NULL"), "replay must preserve last_error");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam filter_sql
```
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed [`DeadLetters`] (SMA-469): inspect and retire `event_outbox` rows the relay
//! parked. `parked = true` IS the dead-letter predicate — there is no separate table (a
//! dedicated one would cost a move-on-park inside the relay's transaction, a move-back replay
//! path, and would render the `parked` column vestigial, all to express a set one boolean
//! already expresses).
//!
//! The three mutating methods write on the CALLER's transaction (recovered via
//! [`recover_txn`], exactly like `PgOutbox::enqueue`) so the mutation and its audit entry
//! commit atomically on one `UnitOfWork`.
//!
//! **Every mutating statement carries `AND parked = true`**, so a live or already-published row
//! is untouchable through this surface — these endpoints can never be used to mutate the live
//! queue.
//!
//! They use `RETURNING *` and go through `Statement` + `query_one` (`execute` discards the
//! returned row), so the caller gets the affected row's contents for its audit entry. For
//! `discard_in` that audit entry is the discarded event's ONLY remaining trace.

use super::entities::event_outbox;
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterEntry, DeadLetterFilter, DeadLetters, RepositoryError};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter, QueryOrder, QueryResult, QuerySelect, Statement, Value};
use uuid::Uuid;

/// `$1` = id. Note `last_error` is deliberately NOT cleared: clearing it would destroy the
/// evidence chain when a replayed row re-parks for a different reason — the operator would then
/// see only the second failure. `parked_at`/`attempts` DO reset, because they describe the
/// row's current state; the error string is history.
const REPLAY_ONE_SQL: &str = r#"UPDATE "event_outbox" SET parked = false, attempts = 0, parked_at = NULL
                                WHERE id = $1 AND parked = true RETURNING *"#;

/// `$1` = id.
const DISCARD_ONE_SQL: &str = r#"DELETE FROM "event_outbox" WHERE id = $1 AND parked = true RETURNING *"#;

/// Builds the shared `parked = true [AND …]` predicate, appending each present filter value to
/// `params` and numbering its placeholder from the vec's running length (so a caller that has
/// already bound values gets a correct continuation, not a restarted sequence).
fn filter_clauses(event_type: &Option<String>, parked_from: &Option<DateTime<Utc>>, parked_to: &Option<DateTime<Utc>>, params: &mut Vec<Value>) -> String {
    let mut sql = "parked = true".to_string();
    if let Some(t) = event_type {
        params.push(Value::from(t.clone()));
        sql.push_str(&format!(" AND event_type = ${}", params.len()));
    }
    if let Some(f) = parked_from {
        params.push(Value::from(*f));
        sql.push_str(&format!(" AND parked_at >= ${}", params.len()));
    }
    if let Some(t) = parked_to {
        params.push(Value::from(*t));
        sql.push_str(&format!(" AND parked_at <= ${}", params.len()));
    }
    sql
}

fn model_to_entry(m: event_outbox::Model) -> DeadLetterEntry {
    DeadLetterEntry {
        id: m.id,
        occurred_at: m.occurred_at,
        event_type: m.event_type,
        schema_version: m.schema_version,
        aggregate_prn: m.aggregate_prn,
        actor_prn: m.actor_prn,
        payload: m.payload,
        correlation_id: m.correlation_id,
        attempts: m.attempts.max(0) as u32,
        parked_at: m.parked_at,
        last_error: m.last_error,
    }
}

/// Projects a `RETURNING *` row. Column names mirror `event_outbox`'s schema exactly.
fn row_to_entry(r: &QueryResult) -> Result<DeadLetterEntry, RepositoryError> {
    let backend = |e: sea_orm::DbErr| map_err(e);
    Ok(DeadLetterEntry {
        id: r.try_get("", "id").map_err(backend)?,
        occurred_at: r.try_get("", "occurred_at").map_err(backend)?,
        event_type: r.try_get("", "event_type").map_err(backend)?,
        schema_version: r.try_get("", "schema_version").map_err(backend)?,
        aggregate_prn: r.try_get("", "aggregate_prn").map_err(backend)?,
        actor_prn: r.try_get("", "actor_prn").map_err(backend)?,
        payload: r.try_get("", "payload").map_err(backend)?,
        correlation_id: r.try_get("", "correlation_id").map_err(backend)?,
        attempts: r.try_get::<i32>("", "attempts").map_err(backend)?.max(0) as u32,
        parked_at: r.try_get("", "parked_at").map_err(backend)?,
        last_error: r.try_get("", "last_error").map_err(backend)?,
    })
}

/// `Clone`: `DatabaseConnection` is an `Arc`-backed pool handle, mirroring every other adapter
/// in this module.
#[derive(Clone)]
pub struct PgDeadLetters {
    db: DatabaseConnection,
}

impl PgDeadLetters {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgDeadLetters { db }
    }
}

#[async_trait]
impl DeadLetters for PgDeadLetters {
    /// Keyset paging by `id DESC` (`id < cursor`), mirroring `PgAuditLog::query`. Outbox ids
    /// are UUIDv7 (`KernelIdGenerator::mint`), so id order IS time order — newest first, which
    /// is what an operator inspecting a backlog wants.
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError> {
        let mut q = event_outbox::Entity::find().filter(event_outbox::Column::Parked.eq(true));
        if let Some(t) = &f.event_type {
            q = q.filter(event_outbox::Column::EventType.eq(t.clone()));
        }
        if let Some(from) = f.parked_from {
            q = q.filter(event_outbox::Column::ParkedAt.gte(from));
        }
        if let Some(to) = f.parked_to {
            q = q.filter(event_outbox::Column::ParkedAt.lte(to));
        }
        if let Some(cursor) = f.cursor {
            q = q.filter(event_outbox::Column::Id.lt(cursor));
        }
        let models = q
            .order_by_desc(event_outbox::Column::Id)
            .limit(f.capped_limit())
            .all(&self.db)
            .await
            .map_err(map_err)?;
        Ok(models.into_iter().map(model_to_entry).collect())
    }

    async fn replay_in(&self, tx: &dyn paigasus_iam_core::Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, REPLAY_ONE_SQL, [Value::from(id)]);
        match txn.query_one(stmt).await.map_err(map_err)? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }

    /// **`FOR UPDATE SKIP LOCKED` on the subquery is required, not an optimization.** Postgres
    /// does not guarantee an `UPDATE ... WHERE id IN (SELECT ... ORDER BY ...)` takes row locks
    /// in the subquery's order, so two concurrent bulk replays with overlapping filters can
    /// deadlock; a non-deadlocking overlap instead blocks the second operator for the whole of
    /// the first's transaction — which includes its audit write and commit. `SKIP LOCKED` makes
    /// concurrent replays partition rather than collide, and an operator responding to an
    /// outage is precisely the person most likely to fire two of these.
    ///
    /// The subquery selects `ORDER BY id` ASCENDING (unlike `list`'s `DESC`): when a filter
    /// matches more rows than `max_rows`, repeated calls then walk the backlog forward instead
    /// of re-selecting the same newest slice.
    async fn replay_matching_in(&self, tx: &dyn paigasus_iam_core::Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError> {
        let txn = recover_txn(tx)?;
        let mut params: Vec<Value> = Vec::new();
        let predicate = filter_clauses(&r.event_type, &r.parked_from, &r.parked_to, &mut params);
        params.push(Value::from(r.capped_max_rows() as i64));
        let limit_placeholder = params.len();
        let sql = format!(
            r#"UPDATE "event_outbox" SET parked = false, attempts = 0, parked_at = NULL
               WHERE id IN (
                 SELECT id FROM "event_outbox" WHERE {predicate}
                 ORDER BY id LIMIT ${limit_placeholder} FOR UPDATE SKIP LOCKED
               )"#
        );
        let res = txn.execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, params)).await.map_err(map_err)?;
        Ok(res.rows_affected())
    }

    async fn discard_in(&self, tx: &dyn paigasus_iam_core::Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, DISCARD_ONE_SQL, [Value::from(id)]);
        match txn.query_one(stmt).await.map_err(map_err)? {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }
}
```

Then append the `mod tests` block from Step 1.

- [ ] **Step 4: Export it**

In `persistence/mod.rs`: add `pub mod pg_dead_letters;` (alphabetically, before `pg_entity_slice`) and `pub use pg_dead_letters::PgDeadLetters;`.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS. If `map_err` is `pub(crate)` and the closure shape trips clippy's `redundant_closure`, use `map_err` directly rather than the `backend` alias.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence
git commit -m "feat(rs): add the PgDeadLetters adapter over parked outbox rows (SMA-469)"
```

---

## Task 12: Dead-letter adapter integration tests

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/dead_letters_pg.rs`

**Interfaces:**
- Consumes: `PgDeadLetters` (Task 11), `SeaOrmUnitOfWork`, `OutboxRelay`, `support::start_migrated_postgres`.

**The centerpiece** is the replay round-trip: replay must not merely flip a column, it must make the relay's very next tick publish the row. Everything else in the surface is worthless if that link is broken.

- [ ] **Step 1: Write the tests**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `PgDeadLetters` (SMA-469) against real Postgres. Docker gating
//! mirrors `tests/relay_pg.rs`: hard failure in CI, skip on a Docker-less laptop.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use paigasus_iam::adapters::events::OutboxRelay;
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_iam::adapters::persistence::{PgDeadLetters, SeaOrmUnitOfWork};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter, DeadLetters, DomainEvent, EventPublisher, EventType, PublishError, UnitOfWork};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

#[derive(Default)]
struct CountingPublisher {
    count: AtomicUsize,
}

#[async_trait]
impl EventPublisher for CountingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Seeds a parked row (the dead-letter state) with a chosen event type and park time.
async fn seed_parked(db: &DatabaseConnection, id: u128, event_type: &str, parked_ago_days: i64) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set(event_type.to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(5),
        parked: Set(true),
        parked_at: Set(Some(Utc::now() - ChronoDuration::days(parked_ago_days))),
        last_error: Set(Some("backend error: transport closed".to_string())),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

fn filter() -> DeadLetterFilter {
    DeadLetterFilter {
        event_type: None,
        parked_from: None,
        parked_to: None,
        cursor: None,
        limit: 50,
    }
}

#[tokio::test]
async fn lists_only_parked_rows_newest_first_and_pages_by_keyset() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());

    let a = seed_parked(&db, 1, "iam.principal.created", 3).await;
    let b = seed_parked(&db, 2, "iam.role.granted", 2).await;
    let c = seed_parked(&db, 3, "iam.principal.created", 1).await;
    // A live row must never appear in the dead-letter list.
    event_outbox::ActiveModel {
        id: Set(Uuid::from_u128(4)),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set("{}".to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
        parked_at: Set(None),
        last_error: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let all = dead.list(&filter()).await.unwrap();
    assert_eq!(all.len(), 3, "only parked rows are dead letters");
    assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b, a], "newest (highest v7 id) first");
    assert_eq!(all[0].last_error.as_deref(), Some("backend error: transport closed"));
    assert_eq!(all[0].attempts, 5);

    // event_type filter
    let typed = dead
        .list(&DeadLetterFilter {
            event_type: Some("iam.role.granted".to_string()),
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(typed.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b]);

    // parked_at range filter — the axis that answers "what parked during last night's outage".
    let recent = dead
        .list(&DeadLetterFilter {
            parked_from: Some(Utc::now() - ChronoDuration::days(2) - ChronoDuration::hours(1)),
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(recent.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b]);

    // keyset paging
    let page1 = dead.list(&DeadLetterFilter { limit: 2, ..filter() }).await.unwrap();
    assert_eq!(page1.len(), 2);
    let page2 = dead
        .list(&DeadLetterFilter {
            cursor: Some(page1.last().unwrap().id),
            limit: 2,
            ..filter()
        })
        .await
        .unwrap();
    assert_eq!(page2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![a]);
}

#[tokio::test]
async fn replay_returns_the_row_to_the_live_queue_and_the_relay_publishes_it() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let id = seed_parked(&db, 10, "iam.principal.created", 1).await;

    let tx = uow.begin().await.unwrap();
    let replayed = dead.replay_in(&*tx, id).await.unwrap().expect("replay must return the affected row");
    tx.commit().await.unwrap();
    assert_eq!(replayed.id, id);

    let row = event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert!(!row.parked, "replay must un-park the row");
    assert_eq!(row.attempts, 0, "replay must reset the attempt count");
    assert!(row.parked_at.is_none(), "replay must clear the park time");
    assert_eq!(
        row.last_error.as_deref(),
        Some("backend error: transport closed"),
        "replay must PRESERVE last_error so a re-parked row keeps its original evidence"
    );

    // The whole point: the relay's very next tick actually publishes it.
    let publisher = Arc::new(CountingPublisher::default());
    let report = OutboxRelay::new(db.clone(), Duration::from_secs(60), 100, 5).tick(publisher.as_ref()).await.unwrap();
    assert_eq!(report.drained, 1, "a replayed row must be visible to the relay's poll");
    assert_eq!(report.failures, 0);
    assert_eq!(publisher.count.load(Ordering::SeqCst), 1);
    assert!(event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().unwrap().published_at.is_some());
}

#[tokio::test]
async fn discard_removes_the_row_and_returns_its_full_contents() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let id = seed_parked(&db, 20, "iam.principal.created", 1).await;

    let tx = uow.begin().await.unwrap();
    let discarded = dead.discard_in(&*tx, id).await.unwrap().expect("discard must return the deleted row");
    tx.commit().await.unwrap();

    // The returned contents ARE the discarded event's only remaining trace — the service
    // copies them into an audit entry, so they must be complete.
    assert_eq!(discarded.id, id);
    assert_eq!(discarded.event_type, "iam.principal.created");
    assert_eq!(discarded.payload, serde_json::json!({"kind": "user"}).to_string());
    assert!(discarded.last_error.is_some());
    assert!(event_outbox::Entity::find_by_id(id).one(&db).await.unwrap().is_none());
}

#[tokio::test]
async fn replay_and_discard_of_a_non_parked_id_return_none() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    // A live row: reachable by id, but NOT a dead letter — this surface must not touch it.
    let live = Uuid::from_u128(30);
    event_outbox::ActiveModel {
        id: Set(live),
        occurred_at: Set(Utc::now()),
        event_type: Set("iam.principal.created".to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(None),
        payload: Set("{}".to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
        parked_at: Set(None),
        last_error: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let tx = uow.begin().await.unwrap();
    assert!(dead.replay_in(&*tx, live).await.unwrap().is_none(), "a live row is not a dead letter");
    assert!(dead.discard_in(&*tx, live).await.unwrap().is_none(), "a live row is not a dead letter");
    assert!(dead.replay_in(&*tx, Uuid::from_u128(999)).await.unwrap().is_none(), "an absent id yields None");
    tx.commit().await.unwrap();

    assert!(event_outbox::Entity::find_by_id(live).one(&db).await.unwrap().is_some(), "the live row must survive untouched");
}

#[tokio::test]
async fn bulk_replay_honors_its_filter_cap_and_ascending_selection_order() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        eprintln!("skipping dead-letter test: Docker unavailable");
        return;
    };
    let dead = PgDeadLetters::new(db.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());

    let mut created = Vec::new();
    for i in 0..5u128 {
        created.push(seed_parked(&db, 100 + i, "iam.principal.created", 1).await);
    }
    let other = seed_parked(&db, 200, "iam.role.granted", 1).await;

    // Capped at 2: must replay the two OLDEST (lowest v7 ids) of the matching set, so repeated
    // calls walk the backlog forward instead of re-selecting the same newest slice.
    let tx = uow.begin().await.unwrap();
    let n = dead
        .replay_matching_in(
            &*tx,
            &BulkReplayRequest {
                event_type: Some("iam.principal.created".to_string()),
                parked_from: None,
                parked_to: None,
                max_rows: 2,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(n, 2);

    for (i, id) in created.iter().enumerate() {
        let parked = event_outbox::Entity::find_by_id(*id).one(&db).await.unwrap().unwrap().parked;
        assert_eq!(parked, i >= 2, "the two oldest matching rows must be the ones replayed (index {i})");
    }
    assert!(
        event_outbox::Entity::find_by_id(other).one(&db).await.unwrap().unwrap().parked,
        "a row outside the event_type filter must not be replayed"
    );
}
```

- [ ] **Step 2: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test dead_letters_pg
```
Expected: PASS. If `OutboxRelay::tick`'s signature differs from the call above, read `tests/relay_pg.rs` and mirror its exact construction.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/dead_letters_pg.rs
git commit -m "test(rs): cover the dead-letter adapter incl. replay then relay publish (SMA-469)"
```

---

## Task 13: `DeadLetterService` + `InvalidBulkReplay`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs`
- Create: `rs/crates/services/paigasus-iam/src/application/dead_letters.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs`

**Interfaces:**
- Consumes: `Action::{ListOutboxDeadLetters, ReplayOutboxDeadLetter, DiscardOutboxDeadLetter}` (Task 9); the `DeadLetters` port (Task 10); `Authorize`, `UnitOfWork`, `AuditLog`, `IdGenerator`, `Clock`, `root_prn()`.
- Produces:
  - `TenancyError::InvalidBulkReplay` — `ErrorClass::Validation`, `code()` = `"invalid-bulk-replay"`, so it renders **400**.
  - `pub struct DeadLetterService` with `new(deps)`, `async fn list(&self, actor: &Prn, filter: DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, TenancyError>`, `async fn replay(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError>`, `async fn replay_matching(&self, actor: &Prn, req: BulkReplayRequest) -> Result<u64, TenancyError>`, `async fn discard(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError>`.
  - `FakeDeadLetters` in `fakes.rs`.

**Why 400, not 422:** `ErrorClass` has exactly six variants and `Validation → 400 BAD_REQUEST` (`adapters/http/error.rs:20-26`); `status_to_grpc` matches all six exhaustively (`adapters/grpc/convert.rs:32-42`); there is no `422` anywhere in `rs/`. Adding one would mean a new `ErrorClass` arm and a broken gRPC exhaustive match for no benefit.

- [ ] **Step 1: Add the error variant**

In `application/error.rs`:
- Add to `enum TenancyError`, after `InvalidAction(String)`:
  ```rust
      /// A bulk dead-letter replay arrived without an explicit, non-zero `max_rows`
      /// (SMA-469). The required row budget IS the guard on blast radius — an "at least one
      /// filter must be present" check was rejected because `parked_from = 1970-01-01`
      /// satisfies it while matching everything, which is how an operator naturally writes
      /// "replay everything".
      #[error("bulk replay requires an explicit non-zero max_rows")]
      InvalidBulkReplay,
  ```
- Add to `code()`: `Self::InvalidBulkReplay => "invalid-bulk-replay",`
- Add to `class()`'s `ErrorClass::Validation` chain: `| Self::InvalidBulkReplay`

- [ ] **Step 2: Write the failing service tests**

Create `application/dead_letters.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeAuthorizer, FakeDeadLetters, FakeUnitOfWork, FixedClock, SeqIds};
    use paigasus_iam_core::AuditOutcome;

    fn actor() -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
    }

    fn entry(id: u128) -> DeadLetterEntry {
        DeadLetterEntry {
            id: Uuid::from_u128(id),
            occurred_at: Utc::now(),
            event_type: "iam.principal.created".to_string(),
            schema_version: 1,
            aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
            actor_prn: None,
            payload: serde_json::json!({"kind": "user"}).to_string(),
            correlation_id: None,
            attempts: 5,
            parked_at: Some(Utc::now()),
            last_error: Some("backend error: transport closed".to_string()),
        }
    }

    fn bulk(max_rows: u64) -> BulkReplayRequest {
        BulkReplayRequest {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows,
        }
    }

    struct Fixture {
        svc: DeadLetterService,
        audit: FakeAuditLog,
        dead: FakeDeadLetters,
    }

    fn fixture(allow: &[Action]) -> Fixture {
        let mut authz = FakeAuthorizer::default();
        for a in allow {
            authz.allow(*a, &root_prn());
        }
        let dead = FakeDeadLetters::default();
        let audit = FakeAuditLog::default();
        let svc = DeadLetterService::new(DeadLetterDeps {
            dead: Arc::new(dead.clone()),
            uow: Arc::new(FakeUnitOfWork),
            audit: Arc::new(audit.clone()),
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(FixedClock::default()),
            authorize: Authorize::new(Arc::new(authz)),
        });
        Fixture { svc, audit, dead }
    }

    #[tokio::test]
    async fn every_operation_denies_an_unauthorized_actor() {
        let f = fixture(&[]);
        f.dead.seed(entry(1));
        assert!(matches!(f.svc.list(&actor(), filter()).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.replay(&actor(), Uuid::from_u128(1)).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.discard(&actor(), Uuid::from_u128(1)).await, Err(TenancyError::Forbidden)));
        assert!(matches!(f.svc.replay_matching(&actor(), bulk(10)).await, Err(TenancyError::Forbidden)));
        assert_eq!(f.audit.0.lock().unwrap().len(), 0, "a denied call must never write an audit entry");
    }

    #[tokio::test]
    async fn replay_records_exactly_one_audit_entry_naming_the_event() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        let replayed = f.svc.replay(&actor(), Uuid::from_u128(1)).await.unwrap();
        assert_eq!(replayed.id, Uuid::from_u128(1));

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "replay must record exactly one audit entry");
        assert_eq!(entries[0].action, "ReplayOutboxDeadLetter");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);
        assert_eq!(entries[0].detail["event_id"], serde_json::json!(Uuid::from_u128(1).to_string()));
        assert_eq!(entries[0].detail["event_type"], serde_json::json!("iam.principal.created"));
        // The row still exists after a replay, so its payload is not copied.
        assert!(entries[0].detail.get("payload").is_none(), "replay must not duplicate the payload");
    }

    #[tokio::test]
    async fn discard_audit_detail_carries_the_whole_event_including_the_payload() {
        let f = fixture(&[Action::DiscardOutboxDeadLetter]);
        f.dead.seed(entry(1));
        f.svc.discard(&actor(), Uuid::from_u128(1)).await.unwrap();

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "DiscardOutboxDeadLetter");
        // A discarded dead letter is gone forever — this entry is its ONLY remaining trace,
        // so it must be lossless.
        assert_eq!(entries[0].detail["payload"], serde_json::json!(serde_json::json!({"kind": "user"}).to_string()));
        assert_eq!(entries[0].detail["aggregate_prn"], serde_json::json!("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa"));
        assert_eq!(entries[0].detail["attempts"], serde_json::json!(5));
        assert_eq!(entries[0].detail["last_error"], serde_json::json!("backend error: transport closed"));
    }

    #[tokio::test]
    async fn replay_and_discard_of_an_unknown_id_are_not_found_and_write_no_audit_entry() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter, Action::DiscardOutboxDeadLetter]);
        assert!(matches!(f.svc.replay(&actor(), Uuid::from_u128(9)).await, Err(TenancyError::NotFound)));
        assert!(matches!(f.svc.discard(&actor(), Uuid::from_u128(9)).await, Err(TenancyError::NotFound)));
        assert_eq!(f.audit.0.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn bulk_replay_rejects_a_missing_max_rows_before_touching_the_store() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        assert!(matches!(f.svc.replay_matching(&actor(), bulk(0)).await, Err(TenancyError::InvalidBulkReplay)));
        assert_eq!(f.dead.replay_matching_calls(), 0, "validation must happen before any store access");
        assert_eq!(f.audit.0.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn bulk_replay_audits_the_request_and_the_count() {
        let f = fixture(&[Action::ReplayOutboxDeadLetter]);
        f.dead.seed(entry(1));
        f.dead.seed(entry(2));
        let n = f.svc.replay_matching(&actor(), bulk(10)).await.unwrap();
        assert_eq!(n, 2);

        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "one bulk call is one audit entry");
        assert_eq!(entries[0].action, "ReplayOutboxDeadLetter");
        assert_eq!(entries[0].detail["replayed"], serde_json::json!(2));
        assert_eq!(entries[0].detail["max_rows"], serde_json::json!(10));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam dead_letter
```
Expected: FAIL — `DeadLetterService` not found.

- [ ] **Step 4: Write the service**

Above the test module in `application/dead_letters.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `DeadLetterService` (SMA-469): the Root-only use case over parked `event_outbox` rows.
//!
//! Root-only-ness lives HERE, not in the Cedar schema — the shared `appliesTo` block does not
//! restrict the three actions, so this service enforces it by always authorizing at
//! `root_prn()`, exactly like `AuditQueryService::list` and `PolicyService::list`.
//!
//! `replay`/`replay_matching`/`discard` drive the mutation and its audit entry through ONE
//! `UnitOfWork` transaction (the `application::roles` reference pattern), so a mid-transaction
//! failure leaves neither. They deliberately do NOT enqueue a domain event: these are
//! operational actions on the queue itself, and an outbox event about outbox operations would
//! be circular.

use std::sync::Arc;

use chrono::Utc;
use metrics::counter;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{Action, AuditEntry, AuditLog, AuditOutcome, BulkReplayRequest, Clock, DeadLetterEntry, DeadLetterFilter, DeadLetters, IdGenerator, UnitOfWork};
use paigasus_kernel::Prn;
use paigasus_observability::names;
use uuid::Uuid;

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;

/// Constructor bag, mirroring `RoleServiceDeps` — keeps `new` from growing a six-argument
/// positional signature.
pub struct DeadLetterDeps {
    pub dead: Arc<dyn DeadLetters>,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
    pub authorize: Authorize,
}

#[derive(Clone)]
pub struct DeadLetterService {
    dead: Arc<dyn DeadLetters>,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
    authorize: Authorize,
}

impl DeadLetterService {
    #[must_use]
    pub fn new(deps: DeadLetterDeps) -> Self {
        DeadLetterService {
            dead: deps.dead,
            uow: deps.uow,
            audit: deps.audit,
            ids: deps.ids,
            clock: deps.clock,
            authorize: deps.authorize,
        }
    }

    /// Builds the committed-outcome audit entry every mutating operation records.
    fn audit_entry(&self, actor: &Prn, action: Action, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: self.clock.now(),
            actor_prn: Some(actor.canonical()),
            action: action.as_wire().to_string(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: Vec::new(),
            detail,
            correlation_id: Some(self.ids.new_correlation_id()),
        }
    }

    pub async fn list(&self, actor: &Prn, filter: DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, TenancyError> {
        self.authorize.check(actor, Action::ListOutboxDeadLetters, &root_prn()).await?;
        Ok(self.dead.list(&filter).await?)
    }

    pub async fn replay(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError> {
        self.authorize.check(actor, Action::ReplayOutboxDeadLetter, &root_prn()).await?;
        let tx = self.uow.begin().await?;
        let Some(entry) = self.dead.replay_in(&*tx, id).await? else {
            // Dropping `tx` without committing rolls it back. `None` covers an absent id, a
            // live row, and a row another actor just replayed or discarded — all 404 to the
            // caller (documented in the runbook so an operator does not chase a phantom).
            return Err(TenancyError::NotFound);
        };
        let detail = serde_json::json!({
            "event_id": entry.id.to_string(),
            "event_type": entry.event_type,
            "aggregate_prn": entry.aggregate_prn,
            "attempts": entry.attempts,
            "last_error": entry.last_error,
        });
        let audit = self.audit_entry(actor, Action::ReplayOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        // Counted AFTER the commit, so a rolled-back replay is never counted. (This differs
        // from `PgAuditLog`'s counter, which deliberately fires at insert — see its doc.)
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "one").increment(1);
        Ok(entry)
    }

    pub async fn replay_matching(&self, actor: &Prn, req: BulkReplayRequest) -> Result<u64, TenancyError> {
        self.authorize.check(actor, Action::ReplayOutboxDeadLetter, &root_prn()).await?;
        // Validated BEFORE any store access — the explicit row budget is the guard on blast
        // radius, so a request without one must never reach the database.
        if !req.is_valid() {
            return Err(TenancyError::InvalidBulkReplay);
        }
        let tx = self.uow.begin().await?;
        let replayed = self.dead.replay_matching_in(&*tx, &req).await?;
        let detail = serde_json::json!({
            "event_type": req.event_type,
            "parked_from": req.parked_from.map(|t| t.to_rfc3339()),
            "parked_to": req.parked_to.map(|t| t.to_rfc3339()),
            "max_rows": req.max_rows,
            "replayed": replayed,
        });
        let audit = self.audit_entry(actor, Action::ReplayOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        // Increments by ROWS, not calls — mixing units within one metric family would make
        // `rate()` meaningless.
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "bulk").increment(replayed);
        Ok(replayed)
    }

    pub async fn discard(&self, actor: &Prn, id: Uuid) -> Result<DeadLetterEntry, TenancyError> {
        self.authorize.check(actor, Action::DiscardOutboxDeadLetter, &root_prn()).await?;
        let tx = self.uow.begin().await?;
        let Some(entry) = self.dead.discard_in(&*tx, id).await? else {
            return Err(TenancyError::NotFound);
        };
        // Deliberately LOSSLESS, payload included: a discarded dead letter is gone forever, so
        // this entry is its only remaining trace and the documented reconciliation input for
        // the downstream delivery that will now never happen.
        let detail = serde_json::json!({
            "event_id": entry.id.to_string(),
            "event_type": entry.event_type,
            "schema_version": entry.schema_version,
            "aggregate_prn": entry.aggregate_prn,
            "actor_prn": entry.actor_prn,
            "correlation_id": entry.correlation_id.map(|c| c.to_string()),
            "occurred_at": entry.occurred_at.to_rfc3339(),
            "attempts": entry.attempts,
            "last_error": entry.last_error,
            "payload": entry.payload,
        });
        let audit = self.audit_entry(actor, Action::DiscardOutboxDeadLetter, detail);
        self.audit.record(&*tx, &audit).await?;
        tx.commit().await?;
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL).increment(1);
        Ok(entry)
    }
}
```

Add `pub mod dead_letters;` to `application/mod.rs`.

Unused-import note: the test module references `filter()` — add the same `fn filter()` helper used in Task 12 to the test module.

- [ ] **Step 5: Add `FakeDeadLetters`**

In `application/fakes.rs`, mirroring `FakeOutbox`/`FakeAuditLog`'s existing shape (read them first — they wrap a `Mutex<Vec<_>>` in a `#[derive(Clone, Default)]` newtype):

```rust
/// In-memory [`DeadLetters`] for service tests (SMA-469). `replay_in`/`discard_in` REMOVE the
/// entry, so a second call on the same id returns `None` — matching the adapter, whose
/// `AND parked = true` predicate makes a replayed or discarded row unmatchable.
#[derive(Clone, Default)]
pub struct FakeDeadLetters {
    entries: Arc<Mutex<Vec<DeadLetterEntry>>>,
    replay_matching_calls: Arc<AtomicUsize>,
}

impl FakeDeadLetters {
    pub fn seed(&self, e: DeadLetterEntry) {
        self.entries.lock().unwrap().push(e);
    }
    pub fn replay_matching_calls(&self) -> usize {
        self.replay_matching_calls.load(Ordering::SeqCst)
    }
    fn take(&self, id: Uuid) -> Option<DeadLetterEntry> {
        let mut g = self.entries.lock().unwrap();
        let idx = g.iter().position(|e| e.id == id)?;
        Some(g.remove(idx))
    }
}

#[async_trait]
impl DeadLetters for FakeDeadLetters {
    async fn list(&self, _f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError> {
        Ok(self.entries.lock().unwrap().clone())
    }
    async fn replay_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Ok(self.take(id))
    }
    async fn replay_matching_in(&self, _tx: &dyn Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError> {
        self.replay_matching_calls.fetch_add(1, Ordering::SeqCst);
        let mut g = self.entries.lock().unwrap();
        let n = g.len().min(r.capped_max_rows() as usize);
        g.drain(..n);
        Ok(n as u64)
    }
    async fn discard_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError> {
        Ok(self.take(id))
    }
}
```

Add whatever imports are missing at the top of `fakes.rs` (`std::sync::atomic::{AtomicUsize, Ordering}`, and the four dead-letter types).

- [ ] **Step 6: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS. If `FakeAuthorizer` has no `allow(action, prn)` method with that signature, read its definition and adapt the fixture — `application/audit.rs`'s test module already calls `fake_authz.allow(Action::ListAuditLog, &root_prn())`, so mirror that exactly.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application
git commit -m "feat(rs): add DeadLetterService with audited replay and discard (SMA-469)"
```

---

## Task 14: HTTP surface

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/dead_letters.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`

**Interfaces:**
- Consumes: `DeadLetterService` (Task 13).
- Produces: `pub fn router() -> Router<AppState>` mounting the four routes; `AppState.dead_letters: DeadLetterService`.

| Method | Path |
|---|---|
| `GET` | `/v1/outbox/dead-letters` |
| `POST` | `/v1/outbox/dead-letters/replay` |
| `POST` | `/v1/outbox/dead-letters/{id}/replay` |
| `POST` | `/v1/outbox/dead-letters/{id}/discard` |

The literal `/replay` and `/{id}/replay` routes differ in segment count, so axum's router has no ambiguity between them.

- [ ] **Step 1: Write the failing test**

At the bottom of the new `dead_letters.rs`, mirroring `http/audit.rs`'s existing `to_filter` test block:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> DeadLetterQuery {
        DeadLetterQuery {
            event_type: None,
            parked_from: None,
            parked_to: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn to_filter_treats_absent_and_empty_fields_as_unfiltered() {
        let f = to_filter(DeadLetterQuery {
            event_type: Some(String::new()),
            ..q()
        })
        .unwrap();
        assert_eq!(f.event_type, None);
        assert_eq!(f.parked_from, None);
        assert_eq!(f.parked_to, None);
        assert_eq!(f.cursor, None);
    }

    #[test]
    fn to_filter_maps_an_absent_or_zero_limit_to_the_server_default() {
        assert_eq!(to_filter(q()).unwrap().limit, DEFAULT_LIMIT);
        assert_eq!(to_filter(DeadLetterQuery { limit: Some(0), ..q() }).unwrap().limit, DEFAULT_LIMIT);
        // Sanity: capped_limit's own floor for a literal 0 is 1, not DEFAULT_LIMIT — this test
        // is only meaningful because to_filter intercepts the sentinel first.
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    #[test]
    fn to_filter_parses_rfc3339_park_times_and_a_uuid_cursor() {
        let f = to_filter(DeadLetterQuery {
            parked_from: Some("2026-08-01T00:00:00Z".to_string()),
            parked_to: Some("2026-08-02T00:00:00Z".to_string()),
            cursor: Some(Uuid::from_u128(7).to_string()),
            ..q()
        })
        .unwrap();
        assert!(f.parked_from.is_some());
        assert!(f.parked_to.is_some());
        assert_eq!(f.cursor, Some(Uuid::from_u128(7)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_timestamp_and_cursor() {
        assert!(matches!(
            to_filter(DeadLetterQuery { parked_from: Some("nope".to_string()), ..q() }),
            Err(TenancyError::InvalidPrn(_))
        ));
        assert!(matches!(to_filter(DeadLetterQuery { cursor: Some("nope".to_string()), ..q() }), Err(TenancyError::InvalidPrn(_))));
    }

    #[test]
    fn bulk_body_without_max_rows_becomes_an_invalid_bulk_replay_request() {
        let req = BulkReplayBody {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows: None,
        }
        .into_request()
        .unwrap();
        assert!(!req.is_valid(), "an absent max_rows must produce an invalid request, not a default");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam to_filter_parses_rfc3339_park_times
```
Expected: FAIL — module does not exist.

- [ ] **Step 3: Add the DTOs**

In `http/dto.rs`, after the audit DTOs:

```rust
/// A parked outbox row over HTTP (SMA-469). `payload` is emitted as a JSON **string** — it is
/// the raw serialized TEXT exactly as stored, deliberately NOT re-parsed into a
/// `serde_json::Value`: invalid payload JSON is one of the reasons a row parks, so a surface
/// that could only render valid JSON could not display the rows it exists to explain.
#[derive(Debug, Clone, Serialize)]
pub struct DeadLetterEntryDto {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,
    pub correlation_id: Option<Uuid>,
    pub attempts: u32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl From<DeadLetterEntry> for DeadLetterEntryDto {
    fn from(e: DeadLetterEntry) -> Self {
        DeadLetterEntryDto {
            id: e.id,
            occurred_at: e.occurred_at,
            event_type: e.event_type,
            schema_version: e.schema_version,
            aggregate_prn: e.aggregate_prn,
            actor_prn: e.actor_prn,
            payload: e.payload,
            correlation_id: e.correlation_id,
            attempts: e.attempts,
            parked_at: e.parked_at,
            last_error: e.last_error,
        }
    }
}

/// `next_cursor` is present only when the page came back FULL, mirroring `AuditListResponseDto`.
#[derive(Debug, Clone, Serialize)]
pub struct DeadLetterListResponseDto {
    pub entries: Vec<DeadLetterEntryDto>,
    pub next_cursor: Option<String>,
}

/// Query params for `GET /v1/outbox/dead-letters`. Timestamps stay raw `Option<String>` so a
/// parse failure funnels through the handler's `{"error":{code,message}}` envelope, mirroring
/// `AuditQuery`'s identical posture.
#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterQuery {
    pub event_type: Option<String>,
    pub parked_from: Option<String>,
    pub parked_to: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// Body for the bulk `POST /v1/outbox/dead-letters/replay`. `max_rows` is `Option` on the wire
/// so an omitted field is distinguishable from an explicit `0` — both are rejected, but the
/// type must be able to represent "absent" to reject it deliberately rather than defaulting.
#[derive(Debug, Clone, Deserialize)]
pub struct BulkReplayBody {
    pub event_type: Option<String>,
    pub parked_from: Option<String>,
    pub parked_to: Option<String>,
    pub max_rows: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkReplayResponseDto {
    pub replayed: u64,
}
```

Add `use paigasus_iam_core::{BulkReplayRequest, DeadLetterEntry};` to `dto.rs`'s imports as needed.

- [ ] **Step 4: Write the handlers**

```rust
// SPDX-License-Identifier: Apache-2.0

//! `/v1/outbox/dead-letters` handlers (SMA-469): a thin adapter over `AppState.dead_letters` —
//! parse -> `DeadLetterService` -> DTO, no business logic here (mirrors `http::audit`).
//!
//! All three Cedar actions are Root-only, enforced INSIDE `DeadLetterService` itself, so a
//! non-Root caller gets `403` with nothing about the dead-letter contents reaching the
//! response. Sits on the bearer-gated `protected` sub-router; the caller's PRN comes from the
//! auth middleware's `AuthContext`, never a client-supplied value.
//!
//! This is an operator-only break-glass surface and is deliberately HTTP-only: unlike the
//! audit read API it has no gRPC mirror, which keeps `contracts/` untouched. That is a scope
//! decision, not an API-boundary principle.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{BulkReplayBody, BulkReplayResponseDto, DeadLetterEntryDto, DeadLetterListResponseDto, DeadLetterQuery};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/outbox/dead-letters", get(list))
        // The literal `/replay` and `/{id}/replay` below differ in segment count, so axum's
        // router has no ambiguity between them.
        .route("/v1/outbox/dead-letters/replay", post(replay_matching))
        .route("/v1/outbox/dead-letters/{id}/replay", post(replay_one))
        .route("/v1/outbox/dead-letters/{id}/discard", post(discard_one))
}

fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

fn opt_non_empty(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.is_empty())
}

/// Absent/empty means unfiltered; a present value must parse as RFC3339.
/// `InvalidPrn`-as-sentinel, mirroring `http::audit::parse_ts` exactly.
fn parse_ts(raw: Option<String>) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| TenancyError::InvalidPrn(format!("invalid RFC3339 timestamp: {s}"))),
    }
}

fn parse_cursor(raw: Option<String>) -> Result<Option<Uuid>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(&s).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string())),
    }
}

/// `limit` absent or `0` maps to [`DEFAULT_LIMIT`] HERE — passing a bare `0` through would hit
/// `DeadLetterFilter::capped_limit`'s own floor of 1 instead, so a default request would return
/// a single row (the same trap `http::audit::to_filter` documents).
fn to_filter(q: DeadLetterQuery) -> Result<DeadLetterFilter, TenancyError> {
    Ok(DeadLetterFilter {
        event_type: opt_non_empty(q.event_type),
        parked_from: parse_ts(q.parked_from)?,
        parked_to: parse_ts(q.parked_to)?,
        cursor: parse_cursor(q.cursor)?,
        limit: match q.limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(l) => l,
        },
    })
}

impl BulkReplayBody {
    /// An absent `max_rows` becomes `0`, which `BulkReplayRequest::is_valid` rejects — the
    /// service turns that into `TenancyError::InvalidBulkReplay` (a 400). It is deliberately
    /// NOT defaulted to anything usable: the explicit row budget is the guard.
    pub fn into_request(self) -> Result<BulkReplayRequest, TenancyError> {
        Ok(BulkReplayRequest {
            event_type: opt_non_empty(self.event_type),
            parked_from: parse_ts(self.parked_from)?,
            parked_to: parse_ts(self.parked_to)?,
            max_rows: self.max_rows.unwrap_or(0),
        })
    }
}

async fn list(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<DeadLetterQuery>) -> Result<Json<DeadLetterListResponseDto>, ApiError> {
    let filter = to_filter(q)?;
    let limit = filter.capped_limit();
    let entries = s.dead_letters.list(&actor_prn(&ctx), filter).await?;
    let next_cursor = if entries.len() as u64 == limit { entries.last().map(|e| e.id.to_string()) } else { None };
    Ok(Json(DeadLetterListResponseDto {
        entries: entries.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

async fn replay_one(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<DeadLetterEntryDto>, ApiError> {
    Ok(Json(s.dead_letters.replay(&actor_prn(&ctx), id).await?.into()))
}

async fn discard_one(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<DeadLetterEntryDto>, ApiError> {
    Ok(Json(s.dead_letters.discard(&actor_prn(&ctx), id).await?.into()))
}

async fn replay_matching(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(body): Json<BulkReplayBody>) -> Result<Json<BulkReplayResponseDto>, ApiError> {
    let req = body.into_request()?;
    let replayed = s.dead_letters.replay_matching(&actor_prn(&ctx), req).await?;
    Ok(Json(BulkReplayResponseDto { replayed }))
}
```

Then append the `mod tests` block from Step 1.

- [ ] **Step 5: Wire it into `AppState` and the router**

In `http/mod.rs`:
1. Add `mod dead_letters;` beside `mod audit;`.
2. Add the field to `AppState`:
   ```rust
       /// The dead-letter operator use case (SMA-469) — `GET/POST /v1/outbox/dead-letters*`
       /// read through this. Root-only, enforced inside the service itself.
       pub dead_letters: DeadLetterService,
   ```
3. In `AppState::new`, beside the other service constructions, build it with its **own** fresh
   `SeaOrmUnitOfWork` — every other service in this function does the same (`role_uow`,
   `policy_uow`, `api_key_uow`, `service_account_uow` are each separate instances over the same
   `Arc`-backed pool):
   ```rust
           // SMA-469: the dead-letter surface over parked `event_outbox` rows. Its own
           // `SeaOrmUnitOfWork` (a fresh instance is fine — `db.clone()` is a cheap
           // `Arc`-backed pool handle, mirroring `role_uow`/`policy_uow`), so replay/discard
           // and their audit entry commit atomically on one transaction.
           let dead_letter_uow: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(db.clone()));
           let dead_letters = DeadLetterService::new(DeadLetterDeps {
               dead: Arc::new(PgDeadLetters::new(db.clone())),
               uow: dead_letter_uow,
               audit: audit_log.clone(),
               ids: ids.clone(),
               clock: clock.clone(),
               authorize: authorize.clone(),
           });
   ```
   Use whatever local bindings already exist for the shared `audit_log`, `ids`, `clock`, and
   `authorize` — read the surrounding code and match its names rather than inventing new ones.
4. Add `dead_letters,` to the returned `AppState { … }` literal.
5. Merge the router into the **protected** (bearer-gated) sub-router, beside where
   `audit::router()` is merged: `.merge(dead_letters::router())`.

- [ ] **Step 6: Run tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http
git commit -m "feat(rs): expose the outbox dead-letter surface over http (SMA-469)"
```

---

## Task 15: HTTP end-to-end tests

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/http_dead_letters.rs`

**Interfaces:**
- Consumes: the router from Task 14; `support::{start_migrated_postgres, app, send}`.

- [ ] **Step 1: Read the existing pattern**

```bash
sed -n '1,80p' rs/crates/services/paigasus-iam/tests/http_audit.rs
```
`http_audit.rs` already stands up a platform admin, mints a bearer, and asserts `403` for a non-admin. Mirror its setup helpers exactly rather than reinventing them — including how it seeds the `platform_admin` grant.

- [ ] **Step 2: Write the tests**

Create `http_dead_letters.rs` following `http_audit.rs`'s structure, with these cases:

```rust
// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `/v1/outbox/dead-letters` (SMA-469): the three Cedar actions
//! are Root-only, so a non-admin bearer gets 403 with nothing about the dead-letter contents
//! in the response. Docker gating mirrors `tests/http_audit.rs`.
```

Cases to implement (each seeds a parked `event_outbox` row directly, as `dead_letters_pg.rs` does):

1. `list_requires_platform_admin` — a non-admin bearer on `GET /v1/outbox/dead-letters` returns `403`, and the body contains no `entries` key.
2. `list_returns_parked_rows_for_a_platform_admin` — `200`, one entry, whose `last_error` and `attempts` are echoed and whose `payload` is a JSON **string** (assert `body["entries"][0]["payload"].is_string()`).
3. `replay_one_returns_the_row_and_a_second_call_is_404` — first `POST /v1/outbox/dead-letters/{id}/replay` is `200`; the same call again is `404`, because the row is no longer parked. Assert the `404` body's `error.code` is `"not-found"`. **This is the documented success-after-timeout signal** — a client that timed out and retried sees exactly this.
4. `discard_one_removes_the_row` — `200`, then `GET` returns an empty `entries` array.
5. `bulk_replay_without_max_rows_is_400_invalid_bulk_replay` — `POST /v1/outbox/dead-letters/replay` with body `{}` returns `400` and `error.code == "invalid-bulk-replay"`, and the seeded row is **still parked** (validation happened before any store access).
6. `bulk_replay_with_max_rows_replays_and_reports_the_count` — body `{"max_rows": 10}` returns `200` with `{"replayed": N}` matching the seeded parked count.

Use the same `send(app, request)` helper `http_audit.rs` uses, and `serde_json::from_slice` on the response body.

- [ ] **Step 3: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test http_dead_letters
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/http_dead_letters.rs
git commit -m "test(rs): cover the dead-letter http surface end to end (SMA-469)"
```

---

## Task 16: Composition root wiring

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`

**Interfaces:**
- Consumes: `PgOutboxMaintainer`, `OutboxRetentionPolicy` (Task 6); `config.outbox.retention` (Task 5); the five metric names (Task 4).

**The maintainer is ALWAYS spawned**, unlike the audit partition maintainer. `enabled = false` is passed
through in the policy and disables the delete steps only — the tick still runs, because it is what
refreshes `iam_outbox_parked_rows`. Gating the spawn would mean an operator who disables retention during
an incident silently loses the dead-letter backlog signal while the relay keeps parking rows.

- [ ] **Step 1: Keep a db handle for the maintainer**

`main.rs` currently clones `db` into `db_for_maintenance` before the outbox-relay block consumes the
original. Add a second clone beside it:

```rust
    // Kept for the outbox retention sweep (SMA-469), spawned below — cloned here for the same
    // reason `db_for_maintenance` is: the outbox-relay block consumes the original `db` handle.
    let db_for_outbox_retention = db.clone();
```

- [ ] **Step 2: Add the spawn block**

Add a new block after the audit partition-maintenance block:

```rust
    {
        // Outbox retention (SMA-469): bounded, batched deletes of aged published rows and —
        // only when explicitly opted in — aged parked ones, plus the dead-letter backlog gauge.
        // Mirrors the audit partition-maintenance block above, with one deliberate difference:
        // this task is spawned UNCONDITIONALLY. `[outbox.retention].enabled = false` disables
        // the DELETES (it rides along in the policy) but the tick still runs, because the tick
        // is what refreshes `iam_outbox_parked_rows`. Gating the spawn on `enabled` would mean
        // an operator who pauses deletion during an incident — a plausible reaction — silently
        // loses the dead-letter backlog signal while the relay keeps parking rows.
        let policy = OutboxRetentionPolicy {
            enabled: config.outbox.retention.enabled,
            published_days: config.outbox.retention.published_days,
            parked_days: config.outbox.retention.parked_days,
            batch_size: config.outbox.retention.batch_size,
            max_batches_per_tick: config.outbox.retention.max_batches_per_tick,
        };
        if !config.outbox.retention.enabled {
            tracing::warn!("outbox.retention.enabled = false — event_outbox rows will never be deleted and the table will grow without bound; the dead-letter backlog gauge still updates");
        }
        if config.outbox.retention.parked_days > 0 {
            tracing::warn!(
                parked_days = config.outbox.retention.parked_days,
                "outbox.retention.parked_days > 0 — parked (dead-letter) rows will be auto-deleted at this age, whether or not an operator has inspected them"
            );
        }
        let maintainer = PgOutboxMaintainer::new(db_for_outbox_retention);
        // An awaited startup sweep (non-fatal), mirroring the partition maintainer's: without
        // it nothing happens for the first `interval_secs`, which on a deployment being rescued
        // from an unbounded table is the wrong first impression.
        let report = maintainer.clone().tick(chrono::Utc::now(), policy).await;
        if report.errored {
            tracing::warn!("initial outbox retention tick reported an error — continuing");
        }
        let interval = Duration::from_secs(config.outbox.retention.interval_secs);
        let mut rx = rx.clone();
        servers.spawn(async move {
            maintainer
                .run(policy, interval, async move {
                    let _ = rx.changed().await;
                })
                .await;
            Ok(())
        });
    }
```

Extend the `use` line to `use paigasus_iam::adapters::persistence::{Migrator, OutboxRetentionPolicy, PgOutboxMaintainer, PgPartitionMaintainer, RetentionPolicy};`.

- [ ] **Step 3: Describe the new metrics**

In `describe_iam_metrics()`, after the existing outbox-relay describes:

```rust
    describe_counter!(
        names::IAM_OUTBOX_RETENTION_TICKS_TOTAL,
        "Outbox retention sweep ticks, labeled by result (ok/error) — the sweep's liveness signal. Ticks even when [outbox.retention].enabled = false, because the tick also refreshes the dead-letter backlog gauge."
    );
    describe_counter!(
        names::IAM_OUTBOX_ROWS_DELETED_TOTAL,
        "event_outbox rows deleted by retention; label reason=published|parked."
    );
    describe_gauge!(
        names::IAM_OUTBOX_PARKED_ROWS,
        "Parked (dead-letter) event_outbox rows awaiting an operator. Set independently by every replica — aggregate max by (job), never sum."
    );
    describe_counter!(
        names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL,
        "Dead-letter ROWS returned to the live queue; label scope=one|bulk. Counts rows, not calls, so rate() is meaningful across both scopes."
    );
    describe_counter!(
        names::IAM_OUTBOX_DEAD_LETTERS_DISCARDED_TOTAL,
        "Dead letters permanently discarded by an operator — each one is an event that committed in IAM and will never reach any consumer."
    );
```

Update the doc comment above `describe_iam_metrics` — it currently says "the 17 metric families"; make it 22 and mention the SMA-469 retention/dead-letter families.

- [ ] **Step 4: Verify it builds and the binary boots its config path**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```
Expected: clean build, no warnings.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/main.rs
git commit -m "feat(rs): spawn the outbox retention sweep from the composition root (SMA-469)"
```

---

## Task 17: Alerts and promtool fixtures

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`

**Interfaces:**
- Consumes: `iam_outbox_retention_ticks_total`, `iam_outbox_parked_rows` (Task 4). `tests/drift.rs` asserts every `iam_`-prefixed name in a rule expression appears in `names::ALL`, so Task 4 must be done first.

**Gotchas this task must respect:**
- `promtool check config` reads **zero** rule files (its glob is container-absolute), so it is no proof the rules load. The fixture run is the real gate.
- Fixture `rule_files` **globs across files**, so a duplicate alert name anywhere collides. Neither new name may already exist.
- An all-firing fixture cannot distinguish `> 0` from `>= 0`. Every new case needs a **control series that must not fire**.

- [ ] **Step 1: Add the alerts**

In `iam.rules.yml`, after the `IamAuditPartitionMaintenanceStalled` entry:

```yaml
      # The window is scaled to THIS task's hourly default (interval_secs = 3600), NOT copied
      # from IamAuditPartitionMaintenanceStalled's [2d] — that alert's window matches the audit
      # maintainer's DAILY interval, and reusing it here would tolerate ~48 consecutive missed
      # ticks. The `or (up{job="iam"} == 1) * 0` fallback mirrors IamPolicySnapshotReloadsStalled:
      # without it, a replica that spawned the maintainer but never completed a single tick emits
      # no series at all, and `empty == 0` is empty — the alert would go silent exactly when
      # things are worst.
      - alert: IamOutboxRetentionStalled
        expr: (sum by (job, instance) (increase(iam_outbox_retention_ticks_total[6h])) or (up{job="iam"} == 1) * 0) == 0
        for: 1h
        labels: { severity: warning }
        annotations: { summary: "IAM outbox retention sweep is not ticking", description: "No outbox retention tick in ~6 hours on {{ $labels.job }}/{{ $labels.instance }}. Published event_outbox rows are not being deleted, so the table grows without bound, and the iam_outbox_parked_rows dead-letter gauge is stale. The sweep ticks on outbox.retention.interval_secs (default 3600 = hourly), so the 6h window assumes the default — widen it if interval_secs is increased. Unlike the audit equivalent, outbox.retention.enabled=false does NOT silence this: the maintainer is spawned unconditionally and still ticks to refresh the gauge, so silence here always means something is wrong. See RUNBOOK section 4." }
      # `max by (job)` is REQUIRED, not cosmetic: every replica runs a maintainer and each sets
      # the same global count, so N replicas emit N identical series. A bare
      # `iam_outbox_parked_rows > 0` would page N times for one condition, and a `sum()` panel
      # would report N times the real backlog.
      - alert: IamOutboxDeadLetterBacklog
        expr: max by (job) (iam_outbox_parked_rows) > 0
        for: 1h
        labels: { severity: warning }
        annotations: { summary: "IAM outbox dead letters are awaiting an operator", description: "At least one event_outbox row has been parked for over an hour on {{ $labels.job }} and nobody has retired it. Parked rows are excluded from the relay's poll forever, so those events will never be delivered until an operator replays or discards them via /v1/outbox/dead-letters. This complements IamOutboxEventsParked: that one fires when something JUST parked, this one fires when nobody has dealt with it. See RUNBOOK section 4." }
```

- [ ] **Step 2: Add the fixtures**

In `rules/tests/iam.test.yml`, append:

```yaml
  # IamOutboxRetentionStalled: a live target whose sweep counter never moves must fire; a target
  # whose counter IS advancing is the control that proves the rule is not simply always-firing.
  - interval: 1m
    input_series:
      - series: 'up{job="iam", instance="stalled:8080"}'
        values: '1+0x400'
      - series: 'iam_outbox_retention_ticks_total{job="iam", instance="stalled:8080", result="ok"}'
        values: '5+0x400'
      - series: 'up{job="iam", instance="healthy:8080"}'
        values: '1+0x400'
      - series: 'iam_outbox_retention_ticks_total{job="iam", instance="healthy:8080", result="ok"}'
        values: '5+1x400'
    alert_rule_test:
      - eval_time: 7h
        alertname: IamOutboxRetentionStalled
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, instance: "stalled:8080" }
            exp_annotations: { summary: "IAM outbox retention sweep is not ticking", description: "No outbox retention tick in ~6 hours on iam/stalled:8080. Published event_outbox rows are not being deleted, so the table grows without bound, and the iam_outbox_parked_rows dead-letter gauge is stale. The sweep ticks on outbox.retention.interval_secs (default 3600 = hourly), so the 6h window assumes the default — widen it if interval_secs is increased. Unlike the audit equivalent, outbox.retention.enabled=false does NOT silence this: the maintainer is spawned unconditionally and still ticks to refresh the gauge, so silence here always means something is wrong. See RUNBOOK section 4." }

  # IamOutboxRetentionStalled: a LIVE target that has never emitted the series at all must still
  # fire — that is the `or (up == 1) * 0` fallback, and without it this case would be silent.
  - interval: 1m
    input_series:
      - series: 'up{job="iam", instance="never-ticked:8080"}'
        values: '1+0x400'
    alert_rule_test:
      - eval_time: 7h
        alertname: IamOutboxRetentionStalled
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, instance: "never-ticked:8080" }
            exp_annotations: { summary: "IAM outbox retention sweep is not ticking", description: "No outbox retention tick in ~6 hours on iam/never-ticked:8080. Published event_outbox rows are not being deleted, so the table grows without bound, and the iam_outbox_parked_rows dead-letter gauge is stale. The sweep ticks on outbox.retention.interval_secs (default 3600 = hourly), so the 6h window assumes the default — widen it if interval_secs is increased. Unlike the audit equivalent, outbox.retention.enabled=false does NOT silence this: the maintainer is spawned unconditionally and still ticks to refresh the gauge, so silence here always means something is wrong. See RUNBOOK section 4." }

  # IamOutboxDeadLetterBacklog: a nonzero backlog held for an hour fires ONCE for the job even
  # though two replicas each report it (proving the `max by (job)` aggregation). The zero-valued
  # job is the control: a `>= 0` comparison would fire for it, `> 0` must not.
  - interval: 1m
    input_series:
      - series: 'iam_outbox_parked_rows{job="iam", instance="a:8080"}'
        values: '3+0x120'
      - series: 'iam_outbox_parked_rows{job="iam", instance="b:8080"}'
        values: '3+0x120'
      - series: 'iam_outbox_parked_rows{job="iam-empty", instance="c:8080"}'
        values: '0+0x120'
    alert_rule_test:
      - eval_time: 30m
        alertname: IamOutboxDeadLetterBacklog
        exp_alerts: []
      - eval_time: 61m
        alertname: IamOutboxDeadLetterBacklog
        exp_alerts:
          - exp_labels: { severity: warning, job: iam }
            exp_annotations: { summary: "IAM outbox dead letters are awaiting an operator", description: "At least one event_outbox row has been parked for over an hour on iam and nobody has retired it. Parked rows are excluded from the relay's poll forever, so those events will never be delivered until an operator replays or discards them via /v1/outbox/dead-letters. This complements IamOutboxEventsParked: that one fires when something JUST parked, this one fires when nobody has dealt with it. See RUNBOOK section 4." }
```

The `description` strings in `exp_annotations` must match the rendered template **exactly**, including
the `{{ $labels.* }}` substitutions. If promtool reports a mismatch, copy the "got" value it prints
rather than hand-editing.

- [ ] **Step 3: Run the promtool gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool
```
Expected: PASS. If the task name differs, find it with `moon query tasks --affected` or
`grep -n "promtool" .moon/tasks.yml moon.yml`.

- [ ] **Step 4: Run the metric-name drift gate**

```bash
cd rs && cargo nextest run -p paigasus-observability
```
Expected: PASS — `tests/drift.rs` extracts every `iam_`-prefixed identifier from the rule expressions
and asserts each is in `names::ALL`.

- [ ] **Step 5: Commit**

```bash
git add ops/observability/prometheus
git commit -m "feat(ops): alert on a stalled outbox sweep and an unattended dlq backlog (SMA-469)"
```

---

## Task 18: Documentation

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md`
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example`
- Modify: `ops/observability/grafana/dashboards/iam.json`

- [ ] **Step 1: Add the config block to `iam.toml.example`**

After the existing `[outbox]` block:

```toml
# --- Outbox retention + dead letters (SMA-469) ---

# [outbox.retention]
# enabled              = true   # default shown. false = perform NO deletions. The maintainer is
#                                # still spawned and still ticks, because the tick is what
#                                # refreshes the iam_outbox_parked_rows dead-letter gauge — so
#                                # pausing deletion never blinds the backlog alert.
# interval_secs        = 3600   # seconds between sweep ticks — default shown (hourly); must be >= 1.
#                                # Raising this requires widening the IamOutboxRetentionStalled
#                                # alert's [6h] window, which assumes the default.
# published_days       = 7      # delete rows whose published_at is older than this — default
#                                # shown; 0 = never. The outbox is a drained QUEUE, not a record
#                                # of truth (audit_log is the durable trail), so a short window
#                                # is the intended posture.
# parked_days          = 0      # delete parked (dead-letter) rows whose parked_at is older than
#                                # this — default shown (never). A non-zero value auto-deletes
#                                # events an operator was alerted to inspect, so it is opt-in and
#                                # logs a startup warn. It is ALSO the supported bulk-retirement
#                                # path: set a deliberate window, let the sweep retire a mass-
#                                # parked backlog on a schedule, then set it back to 0.
# batch_size           = 1000   # rows per delete pass — default shown; must be >= 1
# max_batches_per_tick = 50     # passes per tick, so one tick retires at most batch_size * this
#                                # and a huge first sweep resumes next tick instead of holding one
#                                # tick open — default shown; must be >= 1. Raise it when draining
#                                # a large accumulated backlog (at the defaults, 50k rows/hour).
```

- [ ] **Step 2: Extend the RUNBOOK metric catalog (§2.2)**

Add rows for the five new families. For `iam_outbox_parked_rows`, state explicitly that it is
**per-replica** and must be aggregated `max by (job)`, never `sum` — a `sum` panel reports N× the real
backlog.

- [ ] **Step 3: Add the two alerts to the §4 table**

```
| `IamOutboxRetentionStalled` | `(sum by (job, instance) (increase(iam_outbox_retention_ticks_total[6h])) or (up{job="iam"} == 1) * 0) == 0` for 1h | warning |
| `IamOutboxDeadLetterBacklog` | `max by (job) (iam_outbox_parked_rows) > 0` for 1h | warning |
```

- [ ] **Step 4: Rewrite the `IamOutboxEventsParked` remediation section**

Replace the current "Remediation (interim — manual, no automated replay tool exists yet)" block. The
new text must cover:

- **The API is now the primary path.** `GET /v1/outbox/dead-letters?event_type=&parked_from=&parked_to=&cursor=&limit=`
  to inspect (each entry carries `last_error`, `attempts`, `parked_at`, and the raw `payload`);
  `POST /v1/outbox/dead-letters/{id}/replay` to return one row to the live queue;
  `POST /v1/outbox/dead-letters/{id}/discard` to retire it permanently;
  `POST /v1/outbox/dead-letters/replay` with `{"event_type": …, "parked_from": …, "parked_to": …, "max_rows": N}`
  for bulk recovery. All four require `platform_admin` (Root-scoped Cedar actions).
- **`max_rows` is required** on the bulk path and returns `400 invalid-bulk-replay` if absent or zero.
  It is the guard on blast radius, deliberately chosen over an "at least one filter" check, which
  `parked_from = 1970-01-01` would defeat.
- **Mass parking is the expected outage signature**, not a poison-message-only symptom: at the defaults
  (`poll_interval_secs = 5`, `max_attempts = 5`) a ~25-second broker outage exhausts every retry for the
  whole backlog. Confirm the root cause is fixed before bulk replay — otherwise the rows just re-park.
- **A 10k-row replay delays live traffic by roughly 8 minutes.** Replayed rows carry lower ids than fresh
  ones and the relay drains `ORDER BY id` ascending at `batch_size = 100` every 5 seconds.
- **`404` conflates several states**: no such id, a row that was never parked, a row another actor just
  replayed or discarded, and a row the relay is mid-tick on and about to park. Do not chase a phantom.
- **Replay is not idempotent.** A client that times out and retries gets `404`, and that `404` **is** the
  expected success-after-timeout signal. A retried *bulk* replay replays a different row set.
- **Replay exercises the at-least-once contract.** The relay is already at-least-once (a publish that
  succeeds followed by a failed commit re-publishes), so consumers must already be idempotent; replay
  makes an operator exercise that deliberately.
- **Discard destroys delivery, not just evidence.** The event committed in IAM and will now never reach
  any consumer — with a real broker publisher (SMA-471) that is permanent, silent divergence. Its audit
  entry carries the complete event, payload included, and is the documented reconciliation input. Record
  a reconciliation plan before discarding.
- **Bulk retirement uses `parked_days`, not bulk discard.** There is deliberately no bulk discard: set
  `[outbox.retention].parked_days` to a deliberate window, let the sweep retire the backlog, then set it
  back to `0`. Unlike a bulk `DELETE` call this is reversible right up until the sweep runs.
- Keep the existing raw SQL as an explicitly-labelled **break-glass fallback** for when the API is
  unreachable.

- [ ] **Step 5: Add the two new alert sections**

Write `### IamOutboxRetentionStalled` and `### IamOutboxDeadLetterBacklog` sections in the same
Meaning / Likely causes / Confirm / Remediation shape the neighbouring entries use. `IamOutboxRetentionStalled`'s
remediation must mention that `DELETE` alone does **not** shrink the table's disk footprint — autovacuum
reclaims space to the free space map, and a large first drain may warrant a manual `VACUUM (or
`VACUUM FULL` during a maintenance window, which takes an exclusive lock).

- [ ] **Step 6: Add the starter-policy drift note**

Add a short subsection (near the audit-retention operational notes) covering the boot-time warning
`"starter policy drift: the stored source differs from the code-defined source"`. Explain that SMA-469
added two write actions, which changes the generated `forbid_archived_writes` starter policy, and that
`reconcile_starter` warns without overwriting. Remediation: update the system-owned `policy` row's
`source` to match the code-defined value, or delete that row and let `reconcile_starter` re-`put` it on
the next boot. Link SMA-477 as the tracked fix.

- [ ] **Step 7: Update §6 "Future"**

Remove the delivered bullet ("Outbox pruning + a full dead-letter-queue subsystem for parked events") and
replace it with what genuinely remains: a gRPC mirror of the dead-letter surface if a non-HTTP operator
client ever needs one, and bulk discard if `parked_days` proves insufficient in practice.

- [ ] **Step 8: Add the Grafana panels**

In `ops/observability/grafana/dashboards/iam.json`, add two panels to the outbox row:

1. **"Outbox dead-letter backlog"** — `max by (job) (iam_outbox_parked_rows)`. **Must** be `max by (job)`,
   not `sum`: every replica sets the same global count, so `sum` reports N× the truth.
2. **"Outbox rows deleted"** — `sum by (reason) (rate(iam_outbox_rows_deleted_total[5m]))`.

Copy an existing panel object's structure verbatim and change only `title`, `targets[].expr`, `gridPos`,
and `id` — ids must stay unique within the dashboard.

- [ ] **Step 9: Verify the docs and dashboard gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability
cd .. && moon run repo:observability-drift
python3 -c "import json;json.load(open('ops/observability/grafana/dashboards/iam.json'));print('dashboard json ok')"
```
Expected: PASS — the drift test asserts every metric named in the dashboard is in `names::ALL`.

- [ ] **Step 10: Commit**

```bash
git add docs/ops ops/observability/grafana rs/crates/services/paigasus-iam/iam.toml.example
git commit -m "docs(repo): document outbox retention and the dead-letter runbook path (SMA-469)"
```

---

## Task 19: Full CI gate run

**Files:** none — this task only verifies.

Per-project Moon tasks do **not** run the repo-level gates (`:deny`, `:machete`, `:affected-smoke`,
codegen-drift, CODEOWNERS). This task runs the graph the way CI does, before pushing.

- [ ] **Step 1: Run the full affected graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```
Expected: all green.

- [ ] **Step 2: Diagnose any unattributed failure**

Moon reports "N failed" without naming the task. Resolve it with:

```bash
jq '.actions[] | select(.status == "failed") | {target, status}' .moon/cache/ciReport.json
```

Known traps for this change set:
- **No new dependencies were added**, so `:deny` and `:machete` should be unaffected. If either reds,
  something outside this plan's scope changed — read the error rather than adding a waiver.
- **No new crate** was created, so the `:affected-smoke` `kernel->bindings` expected set in
  `ci/affected-graph/run.sh` needs no edit.
- **No `.proto` file was touched**, so `contracts:fmt` and `:breaking` should be unaffected.
- `:promtool` failing after Task 17 usually means an `exp_annotations` string does not match the
  rendered template exactly — copy the "got" value promtool prints.

- [ ] **Step 3: Verify the commit-message parity gate specifically**

The local `commit-msg` hook can pass while CI fails, because trailers are appended after the hook runs.
Check the whole branch:

```bash
git log origin/main..HEAD --format='%s' | while read -r s; do
  printf '%s\n' "$s" | grep -qE '^[a-z]+(\([a-z-]+\))?: [a-z]' || echo "BAD SUBJECT: $s"
  [ "${#s}" -le 100 ] || echo "TOO LONG (${#s}): $s"
done
git log origin/main..HEAD --format='%B' | grep -n '#[0-9]' && echo "BAD: a #NNN ref in a commit body breaks footer-leading-blank"
```
Expected: no output from either check.

- [ ] **Step 4: Push**

```bash
git push -u origin feature/sma-469-iam-outbox-retention-a-real-dead-letter-path-for-parked
```

---

## Self-review notes

**Spec coverage.** Every numbered spec section maps to a task: §4 → Task 2; §5/§5.1 → Tasks 1 and 3;
§6.1 → Task 5; §6.2/§6.3 → Tasks 6, 7, 8; §6.4 → Tasks 4 and 16; §6.5 → Task 17; §7.1 → Task 9;
§7.2 → Task 10; §7.3 → Tasks 11 and 12; §7.4 → Task 13; §7.5 → Tasks 14 and 15; §7.6 → Tasks 14 and 16;
§7.7 → Task 18 (documented, not mechanised, as the spec specifies); §8 → distributed across the tasks
that own each surface; §9 → Tasks 7, 8, 12, 15 plus the per-task unit tests; §10 → Task 18;
§11 → mitigations are implemented in Tasks 6, 11, 13, and documented in 18.

**Type consistency.** `DeadLetterEntry` / `DeadLetterFilter` / `BulkReplayRequest` / `DeadLetters` are
defined once in Task 10 and used with identical field names in Tasks 11, 13, and 14.
`OutboxRetentionPolicy` / `SweepReport` are defined in Task 6 and used unchanged in Tasks 7, 8, and 16.
`filter_clauses` / `REPLAY_ONE_SQL` / `DISCARD_ONE_SQL` appear only in Task 11.
`TenancyError::InvalidBulkReplay` is introduced in Task 13 and consumed in Tasks 14 and 15.
`names::IAM_OUTBOX_*` are defined in Task 4 and consumed in Tasks 6, 13, 16, and 17.

**Known follow-up, not a gap.** The starter-policy drift Task 9 triggers is tracked as SMA-477 and
documented in Task 18 Step 6 — it is deliberately not fixed here.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
