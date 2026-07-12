# SMA-446 Slice A — Persistent Denial Audit + Queryable Audit Log — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every authz **denial** (incl. decision-cache-hit denials) to an append-only `audit_log` via a non-blocking buffer, and expose it over HTTP `GET /v1/audit` + gRPC `AuditService.ListAuditEntries` — the MVP slice of SMA-446 sub-project 1, independent of the Unit-of-Work refactor (that is Slice B).

**Architecture:** A bounded, in-process async buffer receives `AuthzDecisionEvent`s for denials from `CedarAuthorizer` (never awaiting a DB write on the decision path); a drain task writes them to `audit_log` (autocommit) via a new `AuditLog` port + `PgAuditLog` adapter. A new Root-scoped Cedar action `ListAuditLog` gates a query use-case exposed on both HTTP and gRPC with keyset pagination.

**Tech Stack:** Rust (edition 2024, rust 1.95), SeaORM + Postgres, axum (HTTP), tonic + prost (gRPC), Cedar (`cedar-policy`), tokio, `serde_json`, `uuid` (v7), `chrono`. Spec: `docs/superpowers/specs/2026-07-12-sma-446-m5-audit-log-outbox-design.md`.

## Global Constraints

- SPDX header on every new source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for TOML/py).
- Rust crates: edition 2024, rust-version 1.95 (workspace-inherited; don't override).
- `paigasus-iam-core` is a **pure lib**: kernel-friendly, **no `getrandom`**, no ambient clock — ids and timestamps are injected. New audit ids come from `IdGenerator` (UUIDv7 via `paigasus_kernel`, as existing id methods do); never call `Uuid::new_v4`/`now_v7` in core.
- The `Action` enum (`authz/action.rs`) is **1:1 with the embedded Cedar schema** (`authz/schema.rs`) and hand-maintained; adding an action means editing enum + `ALL` + `as_wire` + `is_write` + the exhaustiveness test's `len()` + the schema, together.
- Denial-audit is **best-effort and non-blocking**: the `is_authorized` path MUST never `.await` a Postgres write; on buffer saturation drop-oldest and bump a `dropped_denial_audits` counter (observable loss). Fail-open everywhere (a sink/DB error never fails a decision).
- Audit persistence is **always-on** (no disable flag — would violate G1/G2). Only a buffer-capacity tuning knob is configurable.
- Editing `iam.proto` requires `buf format -w` **and** regenerating the rs/py/ts bindings, or `contracts:fmt`/codegen-drift reds `moon ci` silently. Before pushing run: `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` (prefix shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`).
- Commits: conventional, scope required, subject lowercase, header ≤100 chars, no `#NNN` in the body, contiguous footer. Scope `feat(rs)`/`test(rs)`/`feat(contracts)` etc. End with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Never `--no-verify` (worktree deps are installed so the hook works).
- Audit rows carry **no secret material** (no tokens, claims, hashes) — only PRNs, action names, policy ids.

---

## File Structure (Slice A)

**`paigasus-iam-core` (pure lib):**
- Create `src/audit.rs` — `AuditEntry`, `AuditOutcome`, `AuditFilter` value types; re-export from `lib.rs`.
- Modify `src/authz/action.rs` — add `ListAuditLog` (read action).
- Modify `src/authz/schema.rs` — declare `ListAuditLog` in `SCHEMA_SRC`.
- Modify `src/ports.rs` — add `IdGenerator::new_audit_id`; add the `AuditLog` port.
- Modify `src/lib.rs` — re-export the new items.

**`paigasus-iam` (service):**
- Create `src/adapters/persistence/entities/audit_log.rs` — SeaORM entity.
- Create `src/adapters/persistence/migration/m0006_create_audit_log.rs` — table + indexes (+ down).
- Modify `.../migration/mod.rs`, `.../entities/mod.rs`, `.../persistence/mod.rs` — register.
- Create `src/adapters/persistence/pg_audit_log.rs` — `PgAuditLog` (`record_out_of_band` + `query`).
- Create `src/adapters/authz/denial_audit.rs` — bounded buffer sink + drain task.
- Modify `src/adapters/authz/audit.rs` / `mod.rs` — a fan-out `AuditSink` composing tracing + buffer.
- Modify `src/adapters/authz/cedar_authorizer.rs` — cache-hit-Deny capture.
- Create `src/application/audit.rs` — `AuditQueryService` (authorize + query).
- Create `src/adapters/http/audit.rs` — `GET /v1/audit`; modify `http/dto.rs`, `http/mod.rs`.
- Create `src/adapters/grpc/audit.rs` — `AuditService`; modify `grpc/mod.rs`.
- Modify `src/config.rs` (+ `iam.toml.example`) — `[audit].denial_buffer_capacity`.
- Modify `src/adapters/http/mod.rs` (AppState) + `src/main.rs` — wire adapter, buffer, drain task, compose sink.

**`contracts`:** Modify `proto/paigasus/iam/v1/iam.proto` — `AuditService` + messages. Regenerate `paigasus-proto` bindings.

**Tests:** unit tests inline (`#[cfg(test)]`); integration tests in `tests/audit_log_pg.rs`, `tests/http_audit.rs`, `tests/grpc_audit.rs` (mirror `tests/http_authz.rs`, `tests/grpc_authz.rs`, `tests/authz_policy_store.rs`, using `tests/support/mod.rs`).

---

## Task A1: audit domain types (`paigasus-iam-core`)

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/audit.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (add `pub mod audit;` + re-exports)

**Interfaces:**
- Produces:
  - `enum AuditOutcome { Committed, Denied }` (`Debug,Clone,Copy,PartialEq,Eq`) with `fn as_str(&self)->&'static str` (`"committed"`/`"denied"`) and `fn parse(s:&str)->Option<AuditOutcome>`.
  - `struct AuditEntry { id: Uuid, occurred_at: DateTime<Utc>, actor_prn: Option<String>, action: String, resource_prn: Option<String>, outcome: AuditOutcome, determining_policies: Vec<String>, detail: serde_json::Value, correlation_id: Option<Uuid> }` (`Debug,Clone,PartialEq`).
  - `struct AuditFilter { actor_prn: Option<String>, resource_prn: Option<String>, action: Option<String>, outcome: Option<AuditOutcome>, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>, cursor: Option<Uuid>, limit: u64 }` (`Debug,Clone`), with `const MAX_LIMIT: u64 = 200;` and `fn capped_limit(&self)->u64 { self.limit.clamp(1, Self::MAX_LIMIT) }`.

- [ ] **Step 1: Write the failing test** — append to `src/audit.rs` (create the file with the SPDX header + `use` lines first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outcome_roundtrips_through_wire_strings() {
        for o in [AuditOutcome::Committed, AuditOutcome::Denied] {
            assert_eq!(AuditOutcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(AuditOutcome::parse("nope"), None);
    }
    #[test]
    fn filter_limit_is_clamped_to_the_max_and_min() {
        let base = AuditFilter { actor_prn: None, resource_prn: None, action: None, outcome: None, from: None, to: None, cursor: None, limit: 0 };
        assert_eq!(AuditFilter { limit: 0, ..base.clone() }.capped_limit(), 1);
        assert_eq!(AuditFilter { limit: 10_000, ..base }.capped_limit(), AuditFilter::MAX_LIMIT);
    }
}
```

- [ ] **Step 2: Run test to verify it fails** — `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd rs && cargo test -p paigasus-iam-core audit:: 2>&1 | tail`. Expected: FAIL (types not defined).

- [ ] **Step 3: Write the types** in `src/audit.rs` above the test module:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Audit-log value types (SMA-446): the append-only record of security-relevant events
//! (authz denials in Slice A; committed mutations in Slice B). Pure/kernel-friendly — ids and
//! timestamps are injected by the caller (no `getrandom`, no ambient clock).
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome { Committed, Denied }
impl AuditOutcome {
    pub fn as_str(&self) -> &'static str { match self { Self::Committed => "committed", Self::Denied => "denied" } }
    pub fn parse(s: &str) -> Option<Self> { match s { "committed" => Some(Self::Committed), "denied" => Some(Self::Denied), _ => None } }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_prn: Option<String>,
    pub action: String,
    pub resource_prn: Option<String>,
    pub outcome: AuditOutcome,
    pub determining_policies: Vec<String>,
    pub detail: serde_json::Value,
    pub correlation_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct AuditFilter {
    pub actor_prn: Option<String>,
    pub resource_prn: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,
}
impl AuditFilter {
    pub const MAX_LIMIT: u64 = 200;
    pub fn capped_limit(&self) -> u64 { self.limit.clamp(1, Self::MAX_LIMIT) }
}
```

Add to `src/lib.rs`: `pub mod audit;` and to its re-export list `pub use audit::{AuditEntry, AuditFilter, AuditOutcome};` (mirror how existing modules are re-exported there).

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p paigasus-iam-core audit::` → PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit` with `feat(rs): add audit-log value types to iam-core (SMA-446)`.

---

## Task A2: `ListAuditLog` Cedar action (`paigasus-iam-core`)

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs` (variant + `ALL` + `as_wire` + `is_write` + exhaustiveness match + `len` 34→35)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs` (declare `ListAuditLog` in `SCHEMA_SRC`, applying to the Root resource — mirror `ListPolicies`/`ListRoleGrants`)

**Interfaces:**
- Produces: `Action::ListAuditLog` (a read action; `as_wire()=="ListAuditLog"`).

- [ ] **Step 1: Update the failing test** — in `action.rs` change the count assertion in `all_covers_every_variant` from `34` to `35` and its message to `"27 pre-existing + 7 M4 + 1 audit"`. Add to `wire_roundtrip_all_variants` (it already loops `ALL`, so just ensuring the variant is in `ALL` covers it). Add an explicit test:

```rust
#[test]
fn list_audit_log_is_a_read_action() {
    assert!(!Action::ListAuditLog.is_write());
    assert_eq!(Action::parse("ListAuditLog"), Some(Action::ListAuditLog));
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam-core authz::action` → FAIL (variant missing / len mismatch).

- [ ] **Step 3: Add the variant** in `action.rs` — add `ListAuditLog,` to the `enum Action`, to `ALL` (append after `ListApiKeys`), to `as_wire` (`Action::ListAuditLog => "ListAuditLog",`), to the `false` arm of `is_write` (it is read-only), and to the exhaustiveness `match` in `all_covers_every_variant`. Then in `schema.rs`, add `ListAuditLog` to the `action` declaration in `SCHEMA_SRC`, with the same `appliesTo { principal: [...], resource: [Root] }` shape as `ListPolicies`/`ListRoleGrants` (read the surrounding block first; it is Root-scoped). Do **not** touch `roles.rs`: `platform_admin`'s template is action-less (`permit(principal == ?principal, action, resource in ?resource)`) so it already covers `ListAuditLog`, and no other role should have it.

- [ ] **Step 4: Run tests** — `cargo test -p paigasus-iam-core authz::` → PASS (incl. `authz::schema` validation and `roles::every_starter_policy_passes_schema_validation`, which recompiles the schema).

- [ ] **Step 5: Commit** — `feat(rs): add root-scoped ListAuditLog cedar action (SMA-446)`.

---

## Task A3: `AuditLog` port + `IdGenerator::new_audit_id` (`paigasus-iam-core`)

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs` (add `new_audit_id` to `IdGenerator`; add `AuditLog` trait + re-export)
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (re-export `AuditLog`)

**Interfaces:**
- Consumes: `AuditEntry`, `AuditFilter` (Task A1); `RepositoryError` (existing in `ports.rs`).
- Produces:
  - `IdGenerator::new_audit_id(&self) -> Uuid` (UUIDv7).
  - `#[async_trait] trait AuditLog: Send + Sync { async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError>; async fn query(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError>; }` (results newest-first, `id`-descending for keyset paging).

- [ ] **Step 1: Write the failing test** — in `ports.rs` test module add an object-safety assertion (mirror the existing `assert_object_safe` pattern in `authz/ports.rs`):

```rust
#[allow(dead_code)]
fn audit_log_is_object_safe(_: &dyn AuditLog) {}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam-core ports` → FAIL (trait missing).

- [ ] **Step 3: Add the port + id method.** In `ports.rs`, add to the `IdGenerator` trait: `fn new_audit_id(&self) -> uuid::Uuid;` (document: UUIDv7, ordered). Every `IdGenerator` impl must add it — the real one (`adapters/id.rs`, mirror `new_api_key_id`'s v7 construction) and every test fake (`application/create_user.rs` `FixedIdGenerator`, `application/fakes.rs` `SeqIds`, and any others — grep `impl IdGenerator`). Then add the `AuditLog` trait (as above) with `use async_trait::async_trait;` and re-export it from `lib.rs`.

- [ ] **Step 4: Run tests** — `cargo test -p paigasus-iam-core` → PASS (all fakes compile with the new method).

- [ ] **Step 5: Commit** — `feat(rs): add AuditLog port + IdGenerator::new_audit_id (SMA-446)`.

---

## Task A4: `audit_log` migration + SeaORM entity (`paigasus-iam`)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0006_create_audit_log.rs`
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/entities/audit_log.rs`
- Modify: `.../migration/mod.rs` (register), `.../entities/mod.rs` (`pub mod audit_log;`), `.../persistence/mod.rs` (re-export if siblings are)

**Interfaces:**
- Produces: table `audit_log` with columns `id uuid PK`, `occurred_at timestamptz NOT NULL`, `actor_prn text NULL`, `action text NOT NULL`, `resource_prn text NULL`, `outcome text NOT NULL`, `determining_policies text[] NULL`, `detail jsonb NOT NULL DEFAULT '{}'`, `correlation_id uuid NULL`; indexes on `occurred_at DESC`, `actor_prn`, `resource_prn`, `action`, `outcome`. SeaORM `entities::audit_log::{Entity, Model, ActiveModel, Column}`.

> **Slice-A simplification:** Slice A ships the plain table. Monthly range-partitioning + outcome-aware retention (spec §D14) land with the retention/pruning follow-up; the Slice-A schema and queries are partition-compatible (all filters include `occurred_at`/`id`).

- [ ] **Step 1: Write the failing test** — create `rs/crates/services/paigasus-iam/tests/audit_log_pg.rs` (mirror `tests/authz_policy_store.rs`'s DB harness via `tests/support`):

```rust
// SPDX-License-Identifier: Apache-2.0
mod support;
#[tokio::test]
async fn migration_creates_audit_log_table() {
    let db = support::fresh_db().await; // whatever the support harness exposes; mirror authz_policy_store.rs
    // A trivial insert+count proves the table + columns exist.
    let n = support::count_rows(&db, "audit_log").await;
    assert_eq!(n, 0);
}
```

(Adapt to the actual `support` API — read `tests/support/mod.rs` and `tests/authz_policy_store.rs` first; the assertion is "table exists after migrate".)

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam --test audit_log_pg` (needs Postgres per the existing harness; if DB env is absent the test is skipped/ignored exactly like the sibling PG tests) → FAIL (relation `audit_log` does not exist).

- [ ] **Step 3: Write the migration + entity.** In `m0006_create_audit_log.rs` follow `m0005_create_service_accounts_and_api_keys.rs`'s structure (`Migration` struct, `MigrationName`, `up` builds a `Table::create()`, `down` drops it, plus `Index::create()` statements). Columns per the Interfaces block; `determining_policies` is a Postgres `text[]` (use `ColumnType::Array(...)` or a raw `Statement` if the SeaORM schema-builder lacks array sugar — check how `scope_actions`/`scope_roles` were stored in `m0005`/`entities/api_key.rs` and mirror it). `detail` is `jsonb` NOT NULL default `'{}'`. Create the SeaORM `entities/audit_log.rs` mirroring `entities/policy.rs` (Model fields matching the columns; `determining_policies: Option<Vec<String>>`, `detail: serde_json::Value` via `sea_orm::JsonValue`). Register in `migration/mod.rs` (add `mod m0006_create_audit_log;` and `Box::new(m0006_create_audit_log::Migration)` to the vec) and `entities/mod.rs`.

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p paigasus-iam --test audit_log_pg` → PASS.

- [ ] **Step 5: Commit** — `feat(rs): add audit_log table migration + entity (SMA-446)`.

---

## Task A5: `PgAuditLog` adapter (`record_out_of_band` + keyset `query`)

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_audit_log.rs`
- Modify: `.../persistence/mod.rs` (`pub use pg_audit_log::PgAuditLog;`)
- Test: extend `tests/audit_log_pg.rs`

**Interfaces:**
- Consumes: `AuditLog`, `AuditEntry`, `AuditFilter`, `AuditOutcome` (core); `entities::audit_log`.
- Produces: `struct PgAuditLog { db: DatabaseConnection }` with `PgAuditLog::new(db)`, implementing `AuditLog`. `query` returns rows `ORDER BY id DESC` with keyset `WHERE id < cursor` when `cursor` is set, `LIMIT capped_limit()`, applying each present filter (equality on `actor_prn`/`resource_prn`/`action`/`outcome`, range on `occurred_at`).

- [ ] **Step 1: Write the failing test** — add to `tests/audit_log_pg.rs`:

```rust
#[tokio::test]
async fn record_out_of_band_then_query_filters_and_paginates() {
    let db = support::fresh_db().await;
    let sink = paigasus_iam::adapters::persistence::PgAuditLog::new(db.clone());
    // insert 3 denials for two actors
    for (i, actor) in [("a", 1u128), ("a", 2), ("b", 3)] .iter().enumerate() { /* build AuditEntry with new_v7-ish ids ascending, outcome Denied, action "GetProject" */ }
    // query outcome=Denied, actor=a → 2 rows, newest-first; limit=1 + cursor paginates.
}
```

(Write it out fully against the real `AuditEntry` shape and `support` helpers; ids must be ascending UUIDv7 so `ORDER BY id DESC` is deterministic — construct them with a fixed-timestamp v7 helper or `Uuid::from_u128` monotonic values.)

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam --test audit_log_pg record_out_of_band` → FAIL.

- [ ] **Step 3: Implement `PgAuditLog`.** `record_out_of_band` builds an `entities::audit_log::ActiveModel` from the `AuditEntry` and `.insert(&self.db)` (autocommit — no txn; this is the denial path). Map `RepositoryError` via the existing `map_err` in `persistence/mod.rs`. `query` builds a `entities::audit_log::Entity::find()` with `.filter(...)` per present field (`Column::ActorPrn.eq(...)`, `Column::OccurredAt.gte(from)`, `Column::OccurredAt.lte(to)`, `Column::Outcome.eq(o.as_str())`, and `Column::Id.lt(cursor)` for keyset), `.order_by_desc(Column::Id)`, `.limit(filter.capped_limit())`, `.all(&self.db)`, then map each `Model` → `AuditEntry` (parse `outcome` via `AuditOutcome::parse`, wrapping a bad value as `RepositoryError::Backend` like `pg_repository.rs::map_principal_row` does).

- [ ] **Step 4: Run test to verify it passes** — → PASS.

- [ ] **Step 5: Commit** — `feat(rs): add PgAuditLog adapter with keyset query (SMA-446)`.

---

## Task A6: bounded denial-audit buffer + drain task

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/authz/denial_audit.rs`
- Modify: `.../authz/mod.rs` (`pub mod denial_audit;` + re-exports)

**Interfaces:**
- Consumes: `AuditLog` (core), `AuditEntry`, `AuditOutcome`; `AuthzDecisionEvent`, `AuditSink`, `Effect` (core authz).
- Produces:
  - `struct DenialAuditBuffer` (bounded, drop-oldest) with `DenialAuditBuffer::new(capacity: usize) -> (Arc<DenialAuditBuffer>, DenialAuditDrain)`; a non-blocking `push(&self, AuditEntry)` that drops the oldest + bumps `dropped()` when full; `fn dropped(&self) -> u64`.
  - `struct DenialAuditDrain` with `async fn run(self, sink: Arc<dyn AuditLog>, shutdown: impl Future<Output=()>)` — drains entries and `record_out_of_band`s them (logging + swallowing per-entry errors, fail-open).
  - `struct BufferedDenialAuditSink { buf: Arc<DenialAuditBuffer>, ids: Arc<dyn IdGenerator + …> }` implementing `AuditSink`: on `record`, if `ev.effect == Effect::Deny`, build an `AuditEntry{ outcome: Denied, action: ev.action, actor_prn: Some(ev.principal_prn), resource_prn: Some(ev.resource_prn), determining_policies: ev.determining_policies, detail: json!({}), occurred_at: ev.at, correlation_id: None, id: <new_audit_id> }` and `buf.push(entry)` (non-blocking); allows are ignored.

> Use a `Mutex<VecDeque<AuditEntry>>` ring + `tokio::sync::Notify` (no new dependency; drop-oldest = `pop_front` when at capacity). `push` holds the lock only for the enqueue; never `.await`s.

- [ ] **Step 1: Write the failing test** — in `denial_audit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // A capturing AuditLog fake collecting record_out_of_band calls.
    #[tokio::test]
    async fn buffer_drops_oldest_when_full_and_counts_drops() {
        let (buf, _drain) = DenialAuditBuffer::new(2);
        buf.push(entry("a")); buf.push(entry("b")); buf.push(entry("c")); // c evicts a
        assert_eq!(buf.dropped(), 1);
        let drained = buf.drain_for_test(); // test-only helper returning Vec
        assert_eq!(drained.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    }
    #[tokio::test]
    async fn sink_enqueues_denies_and_ignores_allows() {
        let (buf, _d) = DenialAuditBuffer::new(8);
        let sink = BufferedDenialAuditSink::new(buf.clone(), Arc::new(SeqIds::default()));
        sink.record(&event(Effect::Allow)).await;
        sink.record(&event(Effect::Deny)).await;
        assert_eq!(buf.len_for_test(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam denial_audit` → FAIL.

- [ ] **Step 3: Implement** the buffer, drain, and sink as specified (include the `#[cfg(test)]` `drain_for_test`/`len_for_test` helpers). `IdGenerator` here is the crate's `KernelIdGenerator` in production; the sink takes `Arc<dyn IdGenerator>`-equivalent (or is generic over `I: IdGenerator`) — mirror how other adapters take ids. The drain loop: `loop { notify.notified().await OR shutdown; drain all entries; for each: if let Err(e)=sink.record_out_of_band(&e).await { tracing::warn!(...) } }`, exiting on shutdown.

- [ ] **Step 4: Run test to verify it passes** — → PASS.

- [ ] **Step 5: Commit** — `feat(rs): add bounded denial-audit buffer + drain (SMA-446)`.

---

## Task A7: `CedarAuthorizer` cache-hit-Deny capture + composed sink

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs` (cache-hit branch)
- Modify: `.../authz/audit.rs` (add a `FanOutAuditSink` composing two `Arc<dyn AuditSink>`), if not composing at the wiring layer

**Interfaces:**
- Consumes: `AuditSink`, `AuthzDecisionEvent`, `Effect`, `Decision`.
- Produces: `is_authorized` records a denial audit event on a decision-cache **hit** when the cached `Decision.effect == Deny` (never for `Allow`, never double-recording the compute path).

- [ ] **Step 1: Write the failing test** — add to `cedar_authorizer.rs` tests (the `CapturingAuditSink` already exists there):

```rust
#[tokio::test]
async fn cache_hit_deny_is_re_audited_but_cache_hit_allow_is_not() {
    // Build an authorizer whose first call denies (default-deny), cache it, call again → HIT.
    // The CapturingAuditSink must now hold 2 Deny events (miss + hit), and the slice loader
    // still ran only once (proving it was a real cache hit).
    // Then a separate authorizer whose first call ALLOWS (seed a grant): second call HIT must
    // NOT add a second event (allows aren't audited on hits).
}
```

(Write it fully using the existing `fixture()`, `FakeRoleGrantStore`, `MemoryDecisionCache`, `CapturingAuditSink`, `org_admin_grant` helpers already in that test module.)

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam cedar_authorizer cache_hit_deny` → FAIL (current cache-hit returns without auditing).

- [ ] **Step 3: Modify the cache-hit branch** (cedar_authorizer.rs:157-163). When a cached decision is found, before returning: if `cached.effect == Effect::Deny`, build the same `AuthzDecisionEvent` the compute path builds (principal/action/resource from `req`, `effect`/`determining_policies` from `cached`, `at: chrono::Utc::now()`) and `self.audit.record(&event).await`. Keep the existing comment updated: hits re-audit **denials only** (full trail, D3/D8); allows on a hit remain un-audited. This `record` is the buffer's non-blocking enqueue in production, so the hot path stays cheap.

- [ ] **Step 4: Run test to verify it passes** — → PASS; also re-run the whole `cedar_authorizer` suite to confirm `cache_hit_short_circuits_the_slice_load_and_does_not_double_audit` (allow case) still passes.

- [ ] **Step 5: Commit** — `feat(rs): audit cache-hit denials for the full trail (SMA-446)`.

---

## Task A8: `AuditQueryService` application use-case

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/audit.rs`
- Modify: `src/application/mod.rs` (`pub mod audit;`)

**Interfaces:**
- Consumes: `Authorize` (application), `Action::ListAuditLog`, `root_prn()`, `AuditLog`, `AuditFilter`, `AuditEntry`, `TenancyError`.
- Produces: `struct AuditQueryService { audit: Arc<dyn AuditLog>, authorize: Authorize }` with `AuditQueryService::new(...)` and `async fn list(&self, actor: &Prn, filter: AuditFilter) -> Result<Vec<AuditEntry>, TenancyError>` — authorizes `Action::ListAuditLog` at `root_prn()` (mirror `PolicyService::list`), then `audit.query(&filter)`.

- [ ] **Step 1: Write the failing test** — in `audit.rs`, mirror `policies.rs` tests: a `FakeAuditLog` (in-memory `Mutex<Vec<AuditEntry>>` returning the filtered set) + `FakeAuthorizer`. Assert `list` denies (`Forbidden`) when `ListAuditLog` not allowed, and returns rows when allowed.

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p paigasus-iam application::audit` → FAIL.

- [ ] **Step 3: Implement** `AuditQueryService::list` (authorize-then-query, exactly like `PolicyService::list`).

- [ ] **Step 4: Run test to verify it passes** — → PASS.

- [ ] **Step 5: Commit** — `feat(rs): add AuditQueryService (list authorized by ListAuditLog) (SMA-446)`.

---

## Task A9: proto `AuditService.ListAuditEntries` + regenerate bindings

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**` (via the repo's codegen task)

**Interfaces:**
- Produces (proto): `message AuditEntry { string id; google.protobuf.Timestamp occurred_at; string actor_prn; string action; string resource_prn; string outcome; repeated string determining_policies; string detail_json; string correlation_id; }`; `message ListAuditEntriesRequest { string actor_prn; string resource_prn; string action; string outcome; google.protobuf.Timestamp from; google.protobuf.Timestamp to; string cursor; uint32 limit; }`; `message ListAuditEntriesResponse { repeated AuditEntry entries; string next_cursor; }`; `service AuditService { rpc ListAuditEntries(ListAuditEntriesRequest) returns (ListAuditEntriesResponse); }`.

- [ ] **Step 1:** Add the messages + service to `iam.proto` (place near the other services; follow the file's existing field-numbering + import style — `google/protobuf/timestamp.proto` is already imported). Optional scalar fields use empty-string/zero sentinels (mirror existing request messages).

- [ ] **Step 2: Format + regenerate** — `export PATH=...; buf format -w contracts/proto/paigasus/iam/v1/iam.proto`, then run the binding codegen the repo uses (the `contracts`/`paigasus-proto` generate task — check `moon.yml`/CONTRIBUTING; e.g. `moon run contracts:generate` or the buf generate task). This rewrites `paigasus-proto/src/generated/**` **and** the embedded `FILE_DESCRIPTOR_SET`.

- [ ] **Step 3: Verify build + drift** — `cargo build -p paigasus-proto` → PASS; `moon ci :fmt :breaking --base origin/main --include-relations` → clean (new service/messages are additive).

- [ ] **Step 4: Commit** — `feat(contracts): add AuditService.ListAuditEntries rpc (SMA-446)` (stage both the `.proto` and the regenerated bindings).

---

## Task A10: gRPC `AuditService` handler

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/grpc/audit.rs`
- Modify: `.../grpc/mod.rs` (register the service in the tonic router; mirror `grpc/authz.rs`), `.../grpc/convert.rs` if a shared mapping helps
- Test: `tests/grpc_audit.rs` (mirror `tests/grpc_authz.rs`)

**Interfaces:**
- Consumes: `AuditQueryService`, the generated `audit_service_server::{AuditService, AuditServiceServer}`, request/response messages; the authenticated caller PRN extraction used by the other gRPC services (mirror `grpc/authz.rs`).
- Produces: a `AuditService` impl whose `list_audit_entries` maps request → `AuditFilter`, calls `AuditQueryService::list`, maps `Vec<AuditEntry>` → response (`detail_json` = `entry.detail.to_string()`, timestamps via the existing convert helpers, `next_cursor` = last entry's id when a full page was returned else `""`), and maps `TenancyError` → `tonic::Status` via the existing error mapping.

- [ ] **Step 1: Write the failing test** in `tests/grpc_audit.rs` — mirror `grpc_authz.rs`: boot the gRPC server via the test harness, call `ListAuditEntries` as a non-admin → `PermissionDenied`; as a platform-admin (seed the grant the harness uses) → returns the seeded denial rows.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `grpc/audit.rs` + register `AuditServiceServer::new(...)` in `grpc/mod.rs`'s router builder (mirror how `AuthorizationServiceServer` is added). Wire `AuditQueryService` into whatever state the gRPC layer reads (see A12).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat(rs): add AuditService grpc handler (SMA-446)`.

---

## Task A11: HTTP `GET /v1/audit`

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/audit.rs`
- Modify: `.../http/dto.rs` (audit DTOs), `.../http/mod.rs` (route + module)
- Test: `tests/http_audit.rs` (mirror `tests/http_authz.rs`)

**Interfaces:**
- Consumes: `AuditQueryService`, `AppState`, the authenticated-caller extractor used by `/v1/authz` handlers (mirror `http/authz.rs`), `AuditFilter`.
- Produces: `GET /v1/audit` handler parsing query params (`actor`, `resource`, `action`, `outcome`, `from`, `to`, `cursor`, `limit`) into an `AuditFilter`, calling `AuditQueryService::list(caller, filter)`, returning `{ entries: [...], next_cursor }` JSON; `TenancyError` → HTTP status via the existing `http::error` mapping (Forbidden → 403). `detail` serialized as a JSON object (not a string) in the HTTP DTO.

- [ ] **Step 1: Write the failing test** in `tests/http_audit.rs` — mirror `http_authz.rs`: non-admin `GET /v1/audit` → 403; admin → 200 with the seeded denial rows; a `limit`/`cursor` round-trip paginates.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the handler + DTOs + register the route in `http/mod.rs` (`.route("/v1/audit", get(audit::list))` on the authenticated router, mirroring the `/v1/authz` routes and their auth middleware).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat(rs): add GET /v1/audit http endpoint (SMA-446)`.

---

## Task A12: wire into `AppState` + `main.rs` + config

**Files:**
- Modify: `src/config.rs` (+ `iam.toml.example`) — `[audit].denial_buffer_capacity: usize` (default e.g. 4096) + `validate()` (non-zero).
- Modify: `src/adapters/http/mod.rs` (`AppState`): construct `PgAuditLog`, the `DenialAuditBuffer`, the `BufferedDenialAuditSink`, and `AuditQueryService`; compose the audit sink passed to `CedarAuthorizer` as tracing **+** buffer (fan-out); hold `AuditQueryService` (+ the drain handle plumbing) for the HTTP/gRPC layers.
- Modify: `src/main.rs`: spawn the `DenialAuditDrain::run` task under the existing `servers.spawn` + shutdown-watch pattern (mirror the `spawn_reload` block).

**Interfaces:**
- Consumes: everything above.
- Produces: a running service where denials land in `audit_log` and `GET /v1/audit` + gRPC `ListAuditEntries` serve them.

- [ ] **Step 1: Write the failing test** — extend `tests/http_audit.rs` (or a new `tests/audit_e2e.rs`): via the full app harness, make an **unauthorized** request that produces a denial (e.g. a tenancy call the caller can't perform), then `GET /v1/audit` as admin and assert the denial row appears (with `outcome=denied`, the right `action`/`resource_prn`, and a non-empty `determining_policies`). This is the end-to-end proof the buffer→drain→table→query path works.
- [ ] **Step 2: Run** → FAIL (nothing wired yet / drain not spawned).
- [ ] **Step 3: Wire it.** Add the config field + validation. In `AppState::new`, build `PgAuditLog::new(db.clone())` as `Arc<dyn AuditLog>`; `DenialAuditBuffer::new(config.audit.denial_buffer_capacity)`; compose the `CedarAuthorizer`'s `audit` arg as a fan-out of `TracingAuditSink` + `BufferedDenialAuditSink`; construct `AuditQueryService`. Return the `DenialAuditDrain` (or store it) so `main.rs` can spawn it. In `main.rs`, `servers.spawn(drain.run(audit_sink_target, shutdown))` mirroring the reload block. Emit the `dropped_denial_audits` count on a periodic log/metric (a `tracing` gauge is fine for Slice A).
- [ ] **Step 4: Run** → PASS. Then the **full gate run** (Global Constraints): `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` → all green. Also `cargo test -p paigasus-iam` and `cargo test -p paigasus-iam-core`.
- [ ] **Step 5: Commit** — `feat(rs): wire persistent denial audit + query into iam service (SMA-446)`.

---

## Self-Review (run after writing)

- **Spec coverage (Slice A rows of the feed matrix + G2/G3):** denial capture (A6/A7), persistence (A4/A5), query authz (A2/A8), HTTP+gRPC surface (A9/A10/A11), non-blocking/fail-open/drop-oldest (A6/A7), no-secret rows (A6 builds `detail: {}`). Mutation-audit (G1), outbox (G4), correlation (G5), retention/partitioning → **Slice B / follow-ups** (out of scope here, per D13).
- **Placeholders:** the `tests/support` API and the exact SeaORM array-column construction are the two "read the sibling first" points — flagged explicitly with the sibling to mirror, not left blank.
- **Type consistency:** `AuditEntry`/`AuditFilter`/`AuditOutcome`/`AuditLog`/`Action::ListAuditLog`/`new_audit_id` names are used identically across A1→A12; `record_out_of_band`/`query`/`list`/`ListAuditEntries` line up across adapter → service → HTTP/gRPC.

## Execution Handoff — see the pipeline (Stage 4).
