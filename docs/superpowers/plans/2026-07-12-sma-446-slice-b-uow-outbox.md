# SMA-446 Slice B — Unit-of-Work + Transactional Outbox + Mutation Audit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Give `paigasus-iam` an application-owned **Unit-of-Work** write path so each committed mutation atomically writes its aggregate row(s), a **domain-event outbox** row, and a **committed audit_log** row (sharing a correlation id); a background **relay** drains the outbox (multi-replica-safe) through an `EventPublisher` (tracing impl). Builds on Slice A (merged: `audit_log`, `PgAuditLog`, denial buffer, query API).

**Architecture:** The application layer opens a `UnitOfWork` (one Postgres txn), performs txn-scoped mutation + `Outbox::enqueue` + `AuditLog::record`, commits, then runs **awaited** post-commit side-effects (Redis `policy_gen`/`entity_gen` bump, api-key cache evict). The concrete `Transaction` wraps a SeaORM `DatabaseTransaction`; each `Pg*` adapter recovers it by downcast (`dyn Any`), keeping per-aggregate ports separate and `dyn`-safe. Conflict-absorbing mutations (`PolicyStore::put`) use a `Savepoint` (SeaORM nested txn) so a unique-violation rolls back only the savepoint. Spec: `docs/superpowers/specs/2026-07-12-sma-446-m5-audit-log-outbox-design.md` (D1, D2, D4, D6, D9–D14).

**Tech Stack:** Rust 2024 / 1.95, SeaORM+Postgres (txn + `begin`/nested-txn savepoints + `FOR UPDATE SKIP LOCKED`), tokio, serde_json, uuid v7, chrono. Cedar authz (unchanged).

## Global Constraints

- SPDX header + blank line + `//!` doc on every new file.
- `paigasus-iam-core` stays getrandom-free: ids/timestamps injected; new v7 ids come from `IdGenerator` (impl in the service crate). `DomainEvent`/`EventType`/`AuditEntry` are pure value types.
- **Atomicity is the point:** mutation + outbox + audit rows commit-or-roll-back together in ONE Postgres txn. The Redis gen-bump / cache-evict are **post-commit, awaited** side-effects (never inside the txn; never fire for a rolled-back mutation; best-effort/fail-open — a bump failure = TTL-bounded staleness, matching today's posture). Awaited-before-return preserves M3 **AC1** (a grant is visible to the very next decision).
- Preserve existing tested behavior: `PolicyStore::put`'s same-content-absorb / different-content-`Conflict`; `RoleGrantStore::revoke`'s idempotent no-op; `CreateUser`'s duplicate-email `Conflict`; api-key revoke/archive cache eviction; the SMA-444/445 suites stay green.
- Audit rows carry **no secrets** (api-key `detail` = id/prefix/scope/status/expiry only, never plaintext/hash). All jsonb-bound strings sanitized for NUL.
- Relay is **multi-replica-safe** (`FOR UPDATE SKIP LOCKED`), at-least-once, idempotency is the consumer's concern (documented). Only impl is `TracingEventPublisher`.
- Editing `iam.proto` is NOT expected in Slice B (no new RPC). If any proto changes, `buf format -w` + regen bindings. Before pushing run the full gate list: `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` (prefix `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`).
- Commits: conventional, scope required, subject lowercase, header ≤100, no `#NNN` in body, `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. NEVER `--no-verify` (worktree deps installed).
- Docker IS available → integration tests run real Postgres (testcontainers).

## File Structure (Slice B)

**`paigasus-iam-core`:**
- Create `src/domain_event.rs` — `DomainEvent`, `EventType` (enum → `iam.role.granted` etc.), re-export.
- Modify `src/audit.rs` — no change (types exist); `AuditEntry` already has `correlation_id`.
- Modify `src/ports.rs` — `UnitOfWork` + `Transaction` + `Savepoint` ports; `Outbox` port; extend `AuditLog` with in-txn `record(&dyn Transaction, &AuditEntry)`; `EventPublisher` port; `IdGenerator::new_event_id`/`new_correlation_id`; **txn-scoped variants** of the mutation store methods (see B1).
- Modify `src/lib.rs` — re-exports.

**`paigasus-iam`:**
- Create `src/adapters/persistence/uow.rs` — `SeaOrmUnitOfWork`, `SeaOrmTransaction` (wraps `DatabaseTransaction`, downcast entry point), `SeaOrmSavepoint`.
- Create `src/adapters/persistence/entities/event_outbox.rs` + `migration/m0007_create_event_outbox.rs`.
- Create `src/adapters/persistence/pg_outbox.rs` — `PgOutbox` (txn-scoped enqueue).
- Modify `src/adapters/persistence/pg_audit_log.rs` — add in-txn `record`.
- Modify `pg_role_grants.rs`, `pg_policies.rs`, `pg_api_keys.rs`, `pg_service_accounts.rs`, `pg_repository.rs` — add txn-scoped write variants; move gen-bump/cache-evict out to post-commit.
- Create `src/adapters/events/relay.rs` + `tracing_publisher.rs` (+ `mod.rs`) — the drain relay + `TracingEventPublisher`.
- Modify `src/application/{roles,policies,api_keys,service_accounts,create_user}.rs` — rewrite to UoW; build events + audit entries + correlation; post-commit side-effects.
- Modify `src/application/mod.rs`, `src/config.rs` (+ `iam.toml.example`), `src/adapters/http/mod.rs` (AppState wiring), `src/main.rs` (spawn relay).
- Tests: extend `tests/support`; new `tests/outbox_uow_pg.rs`, `tests/relay_pg.rs`, `tests/mutation_audit_e2e.rs`; the existing service tests must stay green.

---

## Task B1 — UoW ports + `DomainEvent` (`paigasus-iam-core`, pure)

**Files:** Create `src/domain_event.rs`; Modify `src/ports.rs`, `src/lib.rs`.

**Interfaces (Produces):**
```rust
pub struct DomainEvent { pub id: Uuid, pub event_type: EventType, pub schema_version: u16,
    pub aggregate_prn: String, pub actor_prn: Option<String>, pub occurred_at: DateTime<Utc>,
    pub payload: serde_json::Value, pub correlation_id: Option<Uuid> }
pub enum EventType { PrincipalCreated, PrincipalArchived, RoleGranted, RoleRevoked,
    ApiKeyIssued, ApiKeyRevoked, PolicyPut, PolicyDeleted }   // as_wire() -> "iam.role.granted" etc., + parse()

#[async_trait] pub trait UnitOfWork: Send + Sync { async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError>; }
#[async_trait] pub trait Transaction: Send { async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;
    async fn savepoint(&mut self) -> Result<Box<dyn Savepoint<'_>>, RepositoryError>;
    fn as_any(&self) -> &dyn std::any::Any; }              // downcast entry point for adapters
#[async_trait] pub trait Savepoint<'a>: Send { async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;
    async fn rollback(self: Box<Self>) -> Result<(), RepositoryError>; fn as_any(&self) -> &dyn std::any::Any; }
#[async_trait] pub trait Outbox: Send + Sync { async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError>; }
#[async_trait] pub trait EventPublisher: Send + Sync { async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError>; }
```
Extend `AuditLog` (Slice A) with `async fn record(&self, tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError>;` (in-txn committed entries; keep `record_out_of_band` + `query`). Add `IdGenerator::new_event_id`/`new_correlation_id`. Add **txn-scoped write variants** to the store ports, each taking `&dyn Transaction` and NOT bumping generations / evicting caches (those move post-commit):
- `RoleGrantStore::grant_in(&dyn Transaction, &RoleGrant)`, `revoke_in(&dyn Transaction, Uuid) -> bool` (returns whether a row existed, for the post-commit bump decision).
- `PolicyStore::put_in(&dyn Transaction, &PolicyDocument) -> PutOutcome` (`Inserted|Updated|AbsorbedIdempotent`; `Conflict`/`SystemImmutable` as errors — see B5 savepoint), `delete_in(&dyn Transaction, &str) -> bool`.
- `ApiKeyRepository::issue_in`/`revoke_in`; `ServiceAccountRepository::create_in`/`set_principal_status_in`; `PrincipalRepository::create_user_in`.

**Steps (TDD):** write object-safety asserts (mirror `authz/ports.rs::assert_object_safe`) for `UnitOfWork`/`Transaction`/`Outbox`/`EventPublisher`; `EventType::as_wire`/`parse` round-trip test (all 8); construct-a-`DomainEvent` test. Implement the types/ports. Update EVERY `IdGenerator` impl (real `KernelIdGenerator` + fakes `SeqIds`, `FixedIdGenerator`) with the two new id methods (grep `impl IdGenerator`). `cargo test -p paigasus-iam-core`, clippy, fmt. Commit `feat(rs): add unit-of-work + outbox + domain-event ports to iam-core (SMA-446)`.

---

## Task B2 — `event_outbox` migration + entity + `PgOutbox`; in-txn `PgAuditLog::record`

**Files:** Create `migration/m0007_create_event_outbox.rs`, `entities/event_outbox.rs`, `pg_outbox.rs`; Modify `pg_audit_log.rs`, `migration/mod.rs`, `entities/mod.rs`, `persistence/mod.rs`.

**`event_outbox` columns** (TEXT-serialized convention, like Slice A's `audit_log`): `id uuid PK`, `occurred_at timestamptz`, `event_type text`, `schema_version int NOT NULL default 1`, `aggregate_prn text`, `actor_prn text NULL`, `payload text NOT NULL` (JSON string), `correlation_id uuid NULL`, `published_at timestamptz NULL`, `attempts int NOT NULL default 0`, `parked bool NOT NULL default false`. **Partial index** `(id) WHERE published_at IS NULL AND parked = false` (the relay poll). Include `down`.

`PgOutbox::enqueue(tx, ev)` and `PgAuditLog::record(tx, entry)` recover the SeaORM txn from `tx.as_any().downcast_ref::<SeaOrmTransaction>()` (B3) and insert via `ActiveModel::insert(&seaorm_txn)`. **Interfaces (Consumes):** `SeaOrmTransaction` (B3) — so B2 depends on B3's concrete type; **implement B3 first or in the same task if the reviewer prefers** (the plan orders B3 before the adapters that downcast — see note). Integration test deferred to B3 (needs the real UoW to open a txn).

> **Ordering note:** B3 defines `SeaOrmTransaction`; B2's adapters downcast to it. Implement **B3 before B2's downcast bodies**, or fold B2+B3 into one task. The task-runner should sequence B3 → B2, or merge them.

Commit `feat(rs): add event_outbox table + PgOutbox + in-txn audit record (SMA-446)`.

---

## Task B3 — Concrete `SeaOrmUnitOfWork` / `Transaction` / `Savepoint` (the mechanism — DE-RISKING SPINE)

**Files:** Create `src/adapters/persistence/uow.rs`; Modify `persistence/mod.rs`.

This is the highest-risk task; do it early and prove it end-to-end before fanning out.

```rust
pub struct SeaOrmUnitOfWork { db: DatabaseConnection }
pub struct SeaOrmTransaction { txn: sea_orm::DatabaseTransaction }   // as_any() returns self
pub struct SeaOrmSavepoint<'a> { sp: sea_orm::DatabaseTransaction /* nested */ , _p: PhantomData<&'a ()> }
```
- `UnitOfWork::begin` → `db.begin()` wrapped in `SeaOrmTransaction`.
- `Transaction::commit` → `self.txn.commit()`. `Transaction::savepoint` → `self.txn.begin()` (SeaORM nested transaction = a Postgres SAVEPOINT) wrapped in `SeaOrmSavepoint`. `as_any` returns `&self` so adapters recover `&self.txn`.
- Adapters get the txn via `tx.as_any().downcast_ref::<SeaOrmTransaction>().ok_or(RepositoryError::…)` then use `&t.txn` as the SeaORM `ConnectionTrait`.

**Steps (TDD, real Postgres):** write `tests/outbox_uow_pg.rs`: (1) begin → insert a role_grant via `grant_in` + `outbox.enqueue` + `audit.record` → commit → all three rows present, sharing `correlation_id`; (2) begin → do the same but return an error before commit (drop the txn) → NONE of the three rows present (rollback atomicity); (3) savepoint: begin → insert → savepoint → attempt a duplicate insert (unique violation) → rollback savepoint → the first insert + a subsequent insert on the outer txn still commit (proves nested-txn isolation). If the downcast/`as_any` object-safety fights the borrow checker, that's the spike signal — report BLOCKED with the exact error and I'll switch to the typed-bundle alternative (spec §5). Commit `feat(rs): add SeaORM unit-of-work + savepoint mechanism (SMA-446)`.

---

## Task B4 — Thread `RoleService::grant`/`revoke` through the UoW (the reference pattern)

**Files:** Modify `application/roles.rs`, `pg_role_grants.rs`; extend `tests/`.

Rewrite `grant` (after the existing authorize/resolve checks): mint `corr = ids.new_correlation_id()`; build `DomainEvent::role_granted(&grant, actor, ids.new_event_id(), corr, now)` + `AuditEntry::committed("GrantRole", actor, &scope_prn, detail, ids.new_audit_id(), corr, now)`; `let tx = uow.begin().await?; grants.grant_in(&*tx,&grant).await?; outbox.enqueue(&*tx,&event).await?; audit.record(&*tx,&entry).await?; tx.commit().await?;` then **awaited** post-commit: `gens.bump_policy_gen()` (best-effort/swallow — the exact bump `PgRoleGrantStore::grant` did today, now moved up). `revoke`: same shape; only bump if `revoke_in` returned `true`. Move `RoleService`'s DI to hold `Arc<dyn UnitOfWork>`, `Arc<dyn Outbox>`, `Arc<dyn AuditLog>`, and the `Generations`-bump as an injected `AfterCommit`/closure (keep the app free of a direct `crate::adapters::authz::Generations` import — pass a `Arc<dyn PolicyGenBumper>` port, impl in the service). `PgRoleGrantStore`: add `grant_in`/`revoke_in` (txn-scoped, NO bump); keep the old `grant`/`revoke` as thin wrappers (one-shot UoW) if any non-UoW caller remains, else migrate them.

**Tests (real PG):** grant → atomic `role_grant`+`event_outbox`+`audit_log` rows sharing `correlation_id`; **AC1** (grant → next `is_authorized` Allows — the post-commit bump is awaited); a store error mid-txn → no event/audit rows AND no gen bump (guard D2); revoke idempotent no-op emits nothing. Commit `feat(rs): route role grant/revoke through the unit-of-work (SMA-446)`.

---

## Task B5 — `PolicyService::put`/`delete` through the UoW **with a savepoint** (preserve conflict-absorption)

**Files:** Modify `application/policies.rs`, `pg_policies.rs`; extend tests.

`put_in(tx, doc)`: wrap the INSERT in `tx.savepoint()`. On a `UniqueConstraintViolation`, `savepoint.rollback()` (only the savepoint aborts — the outer UoW txn stays alive), then re-read the winner **within the same UoW txn** and compare content → `AbsorbedIdempotent` (same) or `AuthzError::Conflict` (different) — preserving pg_policies.rs:159-203's exact semantics but WITHOUT a fresh connection. System-immutable + update paths as today. The application layer only enqueues the outbox event + audit entry when the outcome is `Inserted`/`Updated` (NOT on `AbsorbedIdempotent` — mirrors the "skip bump on absorb" rule). `delete` similar. Post-commit: awaited `policy_gen` bump (skip on absorb).

**Tests (real PG):** the SMA-444 conflict suite still passes through the UoW (same-content race absorbs, different-content `Conflict`); a committed `put` writes atomic policy+outbox+audit; system-immutable rejected before any write. Commit `feat(rs): route policy put/delete through the unit-of-work with savepoints (SMA-446)`.

---

## Task B6 — `ApiKeyService::issue`/`revoke` through the UoW (+ post-commit cache-evict)

**Files:** Modify `application/api_keys.rs`, `pg_api_keys.rs`; tests.

Same UoW shape. `issue`: after the D15 anti-escalation checks, mint the key, write key row (`issue_in`) + `iam.api_key.issued` outbox event (payload = id/prefix/scope/status/expiry, **no secret**) + `ApiKeyIssued` audit entry in one txn; return the plaintext once. `revoke`: `revoke_in` + events in txn; **post-commit-awaited** `cache.evict(key_id)` (the eviction MUST stay — security-critical, spec §9/D5 — just moved to after commit so a rolled-back revoke doesn't evict). **Tests:** issue writes atomic key+outbox+audit sharing corr, plaintext returned once, no secret in any row; revoke persists + evicts cache post-commit; a rolled-back revoke does NOT evict. Commit `feat(rs): route api-key issue/revoke through the unit-of-work (SMA-446)`.

---

## Task B7 — `ServiceAccountService::create`/`archive` + `CreateUser` (principal events, outbox-only)

**Files:** Modify `application/service_accounts.rs`, `application/create_user.rs`, `pg_service_accounts.rs`, `pg_repository.rs`; tests.

`CreateUser::execute`: wrap the principal+user insert (`create_user_in`) + `iam.principal.created` outbox event in one UoW txn (preserving the duplicate-email `Conflict`). No audit row (not in the AC audit set). `ServiceAccountService::create`: principal+SA insert + `iam.principal.created` event; `archive`: `set_principal_status_in(Disabled)` + `iam.principal.archived` event, **post-commit-awaited** cache-evict of the SA's keys (moved from inline). `CreateUser` currently takes no `actor` → `actor_prn = None` on its event (documented). **Tests:** each writes its outbox event atomically; duplicate-email still `Conflict` with no event; archive evicts post-commit only on success. Commit `feat(rs): emit principal outbox events via the unit-of-work (SMA-446)`.

---

## Task B8 — The relay (SKIP LOCKED drain) + `TracingEventPublisher`

**Files:** Create `src/adapters/events/{mod.rs,relay.rs,tracing_publisher.rs}`.

`TracingEventPublisher: EventPublisher` — one structured `tracing::info!` per event (type, aggregate, correlation). `OutboxRelay::run(publisher, shutdown)`: loop — poll `SELECT … FROM event_outbox WHERE published_at IS NULL AND parked=false ORDER BY id FOR UPDATE SKIP LOCKED LIMIT batch_size` (raw `Statement` or SeaORM `.lock_with_behavior`); per row `publisher.publish` → on Ok `UPDATE published_at=now()`, on Err `attempts+=1` and if `attempts>=max_attempts` set `parked=true` + `tracing::error!` (poison); emit per-tick telemetry (drained, oldest-unpublished-age, failures, parked). Exits on the shutdown-watch (mirror `spawn_reload`). **Tests (real PG):** two concurrent `run` loops don't double-publish the same row (SKIP LOCKED); a failing publisher increments `attempts` then parks at `max_attempts`; a healthy publisher drains + marks `published_at`. Commit `feat(rs): add outbox relay with FOR UPDATE SKIP LOCKED + tracing publisher (SMA-446)`.

---

## Task B9 — Config + `AppState`/`main.rs` wiring

**Files:** Modify `config.rs` (+ `iam.toml.example`), `adapters/http/mod.rs`, `main.rs`.

Config `[outbox] { relay_enabled, poll_interval_secs, batch_size, max_attempts }` with `validate()` (non-zero interval/batch/attempts; `relay_enabled` default true). Wire into `AppState::new`: construct `SeaOrmUnitOfWork`, `PgOutbox`, the `PolicyGenBumper`, pass them to the rewritten services; hold the relay handle (take-once slot like Slice A's drain, or a `spawn_relay` method — mirror the denial-drain lifecycle). `main.rs`: spawn `OutboxRelay::run` under `servers.spawn` + shutdown-watch when `relay_enabled`; when disabled, `warn!` (rows still accrue). Keep `AppState::new`'s signature stable (take-once slot) to avoid churning the 15+ callers. **Test:** boot smoke; relay disabled spawns nothing. Commit `feat(rs): wire unit-of-work + outbox relay into the iam service (SMA-446)`.

---

## Task B10 — E2E + full gate run

**Files:** Create `tests/mutation_audit_e2e.rs`.

Via the full app harness (relay spawned): perform a real authorized mutation over HTTP (e.g. `POST` a role grant) → assert (a) the `role_grant` row exists, (b) an `event_outbox` row with `event_type=iam.role.granted` sharing the mutation's `correlation_id`, (c) an `audit_log` row `outcome=committed` sharing the SAME `correlation_id` (G5 stitchability), (d) the relay marks the row `published_at` (poll-with-timeout). Then the FULL gate run (`moon ci …` — Global Constraints) + `cargo test -p paigasus-iam -p paigasus-iam-core`; report each gate. Commit `feat(rs): e2e prove mutation → outbox + audit correlation (SMA-446)`.

---

## Self-Review checklist (run after writing)

- **Spec coverage:** UoW write path (B1/B3/B4), outbox table+relay (B2/B8), mutation audit (B4–B7), savepoints (B5), post-commit-awaited side-effects + AC1 (B4), correlation id (B4–B7/B10), SKIP-LOCKED multi-replica (B8), config (B9). Denial audit + query = Slice A (done). Partitioning/retention + dead-letter + real broker = follow-ups.
- **Risk:** B3 (the downcast mechanism) is the spine — it runs early (before the mutation-site fan-out) and reports BLOCKED with the alternative if the opaque-`Transaction` pattern fights the borrow checker.
- **Type consistency:** `grant_in`/`put_in`/`enqueue`/`record`/`begin`/`commit`/`savepoint`/`as_any` names align across B1→B10; `DomainEvent`/`EventType`/`AuditEntry`(Slice A)/`correlation_id` consistent.
