# SMA-446 (M5, sub-project 1) — IAM persistent audit log + domain-event outbox

**Status:** Design (brainstormed + adversarially challenged) · **Date:** 2026-07-12 ·
**Linear:** SMA-446 (part of; does not close the epic) · **Service:** `paigasus-iam`
(+ `paigasus-iam-core`, `contracts/`, `paigasus-proto`)

> Rev 2 folds in the Stage-2 spec-challenge (see §12 D8–D14 and §15 changelog). The
> architecture (transactional outbox + application-owned Unit-of-Work + two lifecycle-separated
> stores) is unchanged; the hardening is targeted.

---

## 1. Context

M5 (SMA-446) is the IAM v1 vertical slice. As written it bundles five deliverables and
presumes an AI Gateway that, in this repo, is a `fn main() {}` stub. During intake we
decomposed the epic (with Sven) into three follow-on cycles:

1. **IAM persistent audit log + domain-event outbox** ← *this spec*
2. Gateway service + IAM integration (authn via IAM keys, authz via `is_authorized`)
3. Prometheus dashboards + RUNBOOK

This spec covers **sub-project 1 only**. Its PR(s) are marked *"Part of SMA-446"* and do **not**
auto-close the epic; #2 and #3 get their own Linear issues + spec/plan/PR cycles.

### What exists today (relevant substrate)

- `paigasus-iam` is a mature hexagonal service: tenancy, authn (OIDC), authz (Cedar), API
  keys, service accounts — all wired into one `AppState` over a single SeaORM
  `DatabaseConnection`. HTTP (`axum`) + gRPC (`tonic`) both served from `main.rs`, which already
  runs multiple replicas sharing Redis (the JWKS/decision caches).
- **Authz decisions** flow through the `AuditSink` port
  (`paigasus_iam_core::authz::ports::AuditSink`); the only impl is `TracingAuditSink`
  (log-only). `CedarAuthorizer::is_authorized` records **one** `AuthzDecisionEvent` per
  *computed* decision (on the decision-cache miss); a cache **hit** (cedar_authorizer.rs:157-163)
  returns the memoized decision with **zero Postgres I/O** and no re-audit. Its `at` is stamped
  `chrono::Utc::now()` directly — the module deliberately declines a `Clock` port.
- **Mutations** are **not audited** today; there is **no** persistent audit store, **no**
  outbox, **no** message bus.
- Persistence adapters (`Pg*`) each **own their transaction** (`db.begin()` … `txn.commit()`).
  `PgRoleGrantStore::grant` (pg_role_grants.rs:124-143) bumps the Redis-backed `policy_gen`
  **synchronously before returning** — that ordering is what makes M3's AC1 hold.
  `PgPolicyStore::put` (pg_policies.rs:159-203) handles a unique-constraint race by
  `txn.rollback()` + re-reading the winner on a **fresh connection** to distinguish idempotent
  same-content (absorb as `Ok`) from a real same-id `Conflict`. `ApiKeyService`/
  `ServiceAccountService` evict the api-key validation cache after mutating. The application
  layer never imports `Generations`.
- `main.rs` runs background tasks via `spawn` + a shutdown-watch (the policy-snapshot
  `spawn_reload` loop) — the outbox relay mirrors it.

### M3 handoff (SMA-444 PR #78, ADR-0013) — two deferred decisions, now settled

1. **Audit granularity for denials** → **full trail** (D3), realised as a non-blocking
   best-effort buffer (D8) so it never stalls a decision.
2. **Fail-closed-on-Redis-outage authz posture** → **deferred** to a separate authz-hardening
   issue (§14). Current fail-open posture is unchanged.

---

## 2. Goals / Non-goals

### Goals (acceptance criteria)

- **G1.** Every **committed mutation** in the AC set — role granted/revoked, api-key
  issued/revoked, policy put/deleted — is written to a persistent, append-only **`audit_log`**,
  **atomically** with the mutation (exactly-once: rolls back with the mutation).
- **G2.** Every authz **denial** (including denied *mutation attempts* and repeated cache-hit
  denials) is captured for `audit_log` with its **determining policy**, on a path that **never
  fails or stalls a decision** — best-effort, non-blocking, and *observably* lossy only under
  saturation (D8).
- **G3.** The audit log is **queryable** via HTTP `GET /v1/audit` **and** gRPC
  `AuditService.ListAuditEntries` (actor / resource / action / outcome / time-range filters,
  bounded pagination, platform-admin authorized).
- **G4.** A **domain-event outbox** captures principal / role / api-key / policy events written
  **transactionally** with the mutation; a background **relay** drains unpublished rows through
  an `EventPublisher` port (tracing impl), safely across replicas.
- **G5.** A mutation's audit row, its outbox row(s), and any preceding denial for the same
  request share a **correlation id**, so the trail is stitchable.

### Non-goals (out; tracked elsewhere)

- The AI Gateway + IAM integration (#2); Prometheus dashboards + RUNBOOK (#3).
- A real message broker or any non-tracing `EventPublisher`.
- Fail-closed authz posture / revocation-during-outage test (§14).
- Auditing **allows** (AC scopes the log to *mutations + denials*; allows keep flowing to
  `TracingAuditSink` for logs only).
- Auditing tenancy/membership mutations (not in the AC set; they still surface as *denials*
  when unauthorized). Easy future extension on the same UoW seam.
- A full dead-letter subsystem and outbox pruning (relay caps retries + parks poison rows now;
  full DLQ + pruning are §14 follow-ups).

---

## 3. Architecture overview

```
  mutation use-cases ── UnitOfWork (ONE Postgres txn, app-owned) ──────────────┐
  (grant/revoke, issue/revoke,   build DomainEvent + AuditEntry + correlation  │
   put/delete, create/archive)   (savepoint around conflict-absorbing writes)  │
                                                                               ▼
                        ┌──────────────────────────────────────────────┐
                        │ ONE txn:  aggregate row(s)                    │
                        │          + INSERT event_outbox   (G4)         │  atomic (G1/G4)
                        │          + INSERT audit_log (committed)  (G1) │
                        │          commit                               │
                        └───────────────┬──────────────────────────────┘
                                        │ AWAITED post-commit side-effects (NOT in txn, D10):
                                        │   • bump policy_gen/entity_gen (Redis)  ← AC1 hinge
                                        │   • evict api-key validation cache
                                        ▼   (best-effort; failure = TTL-bounded staleness)

  authz denials (CedarAuthorizer, incl. cache-hit Deny)
     └─ enqueue AuditEntry to a bounded async buffer ──► drain task ──► INSERT audit_log(denied)
        (never awaited on is_authorized; drop-oldest + dropped_denial_audits metric — D8)

  relay task (per replica):  SELECT … WHERE published_at IS NULL ORDER BY id
                             FOR UPDATE SKIP LOCKED LIMIT batch   (D12, multi-replica safe)
        └─ EventPublisher::publish → set published_at (ok) │ attempts++ + backoff (fail)
           attempts ≥ max ⇒ park + log/metric (poison); relay-tick telemetry emitted
```

**Two separate tables** (`audit_log`, `event_outbox`) because they have different lifecycles:
`audit_log` is compliance-retained, `event_outbox` is transient (drained then prunable). (See
D14 for why denial rows nonetheless share `audit_log` with mutation rows, and how their volume
is bounded.)

### Feed matrix

| Source | → `audit_log` | → `event_outbox` | durability |
|---|:--:|:--:|---|
| user principal created (`CreateUser`) | — | ✓ | exactly-once (in txn) |
| service-account created / archived | — | ✓ | exactly-once (in txn) |
| role granted / revoked | ✓ | ✓ | exactly-once (in txn) |
| api-key issued / revoked | ✓ | ✓ | exactly-once (in txn) |
| policy put / deleted | ✓ | ✓ | exactly-once (in txn) |
| authz **denial** (compute + cache-hit) | ✓ | — | **best-effort** (buffer, D8) |

**Elegant consequence:** a *denied* mutation attempt (`authorize.check` → `Deny`) is captured by
the denial path **before** the use-case reaches the UoW, so mutation sites need no special
denial handling.

---

## 4. Data model (migration `m0006_create_audit_and_outbox`, with a `down`)

### `audit_log` (append-only, compliance-retained; time-partitioned by `occurred_at`, D14)

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | UUIDv7, app-supplied (`IdGenerator::new_audit_id`, D11) |
| `occurred_at` | `timestamptz` | mutation rows: injected clock. **denial rows: wall-clock** `Utc::now()` (the authz module declines a `Clock`, D-minor) |
| `actor_prn` | `text` NULL | principal PRN; NULL = system/unknown (e.g. self-registration `CreateUser`, §6 note) |
| `action` | `text` | Cedar action name (`GrantRole`, `IssueApiKey`, `PutPolicy`, …) |
| `resource_prn` | `text` NULL | target/scope PRN |
| `outcome` | `text` | `committed` \| `denied` |
| `determining_policies` | `text[]` NULL | populated for `denied` rows |
| `detail` | `jsonb` NOT NULL default `'{}'` | per-action schema (§4.1); **never** secrets |
| `correlation_id` | `uuid` NULL | G5; minted per UoW / per denial request |

Indexes: `(occurred_at DESC)`, `(actor_prn)`, `(resource_prn)`, `(action)`, `(outcome)` — all
**per-partition**. **Retention is owned by this design, not deferred:** monthly range partitions
on `occurred_at`; an outcome-aware retention policy (denials retained shorter than mutations)
documented here and enforced by a scheduled `DROP`/detach of old denial partitions (the RUNBOOK,
#3, only *operationalises* the already-specified policy).

### `event_outbox` (transient, drained)

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | UUIDv7, app-supplied (`IdGenerator::new_event_id`, D11) |
| `occurred_at` | `timestamptz` | injected clock |
| `event_type` | `text` | stable wire string (`iam.role.granted`, …) |
| `schema_version` | `int` NOT NULL default 1 | forward-compat for consumers/brokers |
| `aggregate_prn` | `text` | entity the event is about |
| `actor_prn` | `text` NULL | who caused it |
| `payload` | `jsonb` NOT NULL | event body; **never** secrets |
| `correlation_id` | `uuid` NULL | G5 |
| `published_at` | `timestamptz` NULL | set by relay on success |
| `attempts` | `int` NOT NULL default 0 | incremented on publish failure |
| `parked` | `bool` NOT NULL default false | set when `attempts ≥ max` (poison; D12) |

Relay index: **partial** `(id) WHERE published_at IS NULL AND parked = false` — matches the
`ORDER BY id … FOR UPDATE SKIP LOCKED` poll (D12). Published rows retained until the §14 pruning
job.

**Event types:** `iam.principal.created`, `iam.principal.archived`, `iam.role.granted`,
`iam.role.revoked`, `iam.api_key.issued`, `iam.api_key.revoked`, `iam.policy.put`,
`iam.policy.deleted`.

### 4.1 `detail` / `payload` schema & safety (challenge finding)

- **Secret exclusion is structural + tested:** api-key `detail`/`payload` carry only key `id`,
  `prefix`, `scope`, `status`, `expires_at` — **never** plaintext or hash (mirrors how `ApiKey`
  itself carries no secret field). A unit test asserts no secret material serializes.
- **NUL sanitisation:** any string field is rejected/escaped for ` ` before the jsonb insert
  (a NUL byte fails a Postgres jsonb write, which — under the transactional coupling (§6, D-major)
  — would fail the whole mutation). Serialization is total and cannot produce invalid jsonb.
- Each `event_type` has a documented payload shape (versioned by `schema_version`).

---

## 5. Domain model & ports (`paigasus-iam-core`)

All new core types are **kernel-friendly** (injected ids/clock; no `getrandom`), preserving the
`wasm-getrandom-free` posture of shared crates.

```rust
pub struct DomainEvent { id, event_type: EventType, schema_version: u16, aggregate_prn: String,
                         actor_prn: Option<String>, occurred_at, payload: serde_json::Value,
                         correlation_id: Option<Uuid> }
pub struct AuditEntry  { id, occurred_at, actor_prn: Option<String>, action: String,
                         resource_prn: Option<String>, outcome: AuditOutcome, // Committed|Denied
                         determining_policies: Vec<String>, detail: serde_json::Value,
                         correlation_id: Option<Uuid> }
pub struct AuditFilter { /* actor, resource, action, outcome, from, to, cursor, limit */ }
```

### Ports

```rust
#[async_trait] pub trait UnitOfWork: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError>;
}
#[async_trait] pub trait Transaction: Send {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;   // rollback on drop
    async fn savepoint(&mut self) -> Result<Box<dyn Savepoint>, RepositoryError>; // D9
}

#[async_trait] pub trait Outbox: Send + Sync {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError>;
}
#[async_trait] pub trait AuditLog: Send + Sync {
    async fn record(&self, tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError>; // in-txn, committed
    async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError>;           // autocommit, denials (D8 drain)
    async fn query(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError>;
}
#[async_trait] pub trait EventPublisher: Send + Sync {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError>;
}
```

- **`IdGenerator` gains `new_event_id()` + `new_audit_id()`** (UUIDv7, getrandom-free) — the
  `SeqIds`/fake gains deterministic counterparts for tests.
- **Existing mutation store ports** (`RoleGrantStore`, `PolicyStore`, `ApiKeyRepository`,
  `ServiceAccountRepository`, `PrincipalRepository`) gain **txn-scoped write variants** taking
  `&dyn Transaction`; the current own-txn methods delegate to them via a one-shot UoW where a
  read path still needs them.
- **Recommended Rust mechanism (finalise via a ~1-file plan spike):** the concrete `Transaction`
  wraps a SeaORM `DatabaseTransaction`; each `Pg*` adapter downcasts `&dyn Transaction` (via
  `Any`) to recover it — keeps per-aggregate ports separate + `dyn`-safe. `Savepoint` wraps a
  SeaORM nested transaction. Alternative (typed txn-bound repo bundle) noted; spec is agnostic.

---

## 6. Write path — Unit-of-Work (D1, D2, D4, D9, D10)

```rust
// RoleService::grant, after the existing authorize + resolve checks:
let corr = self.ids.new_correlation_id();                       // G5
let event = DomainEvent::role_granted(&grant, actor, self.ids.new_event_id(), corr, now);
let entry = AuditEntry::committed("GrantRole", actor, &scope_prn, detail, self.ids.new_audit_id(), corr, now);

let tx = self.uow.begin().await?;
self.grants.grant_in(&*tx, &grant).await?;                      // aggregate write, no own txn
self.outbox.enqueue(&*tx, &event).await?;                       // same txn (G4)
self.audit.record(&*tx, &entry).await?;                         // same txn (G1)
tx.commit().await?;

// AWAITED post-commit side-effects (Redis / cache — NOT in txn) — MUST complete before returning:
self.after_commit.run(&[BumpPolicyGen, /* … */]).await;        // best-effort, D2/D10
```

**Instrumented sites (9):** `RoleService::grant`/`revoke`, `ApiKeyService::issue`/`revoke`,
`PolicyService::put`/`delete` (audit + outbox); `CreateUser::execute`,
`ServiceAccountService::create`/`archive` (outbox only). The application layer builds the event +
entry (only it holds the actor + context). *Note (challenge):* `CreateUser::execute` currently
takes **no actor** — `iam.principal.created.actor_prn` is NULL for self-registration; if the
`/v1/users` route carries a caller identity, thread it, else NULL is intended and documented.

### D2/D10 — post-commit side-effects are AWAITED (the AC1 hinge)

Today `PgRoleGrantStore::grant` bumps `policy_gen` synchronously **before returning**; that is
what makes M3's **AC1** hold (the next `is_authorized` sees the bump via `reload_if_stale`,
policy_snapshot.rs:103-110). The UoW relocates the bump/evict to **post-commit** (Redis isn't in
the PG txn; a bump for a rolled-back mutation would be a bug). **Normative:** post-commit
side-effects run **synchronously (awaited)** before the use-case returns — never fire-and-forget
— so AC1 is preserved. They stay best-effort/fail-open (a bump/evict failure = TTL-bounded
staleness, matching D11/D12); a crash between commit and bump is covered by the background reload
loop (Postgres is source of truth). **New test:** grant → immediate second decision Allows, over
the UoW path.

### D9 — savepoints preserve conflict-absorption

`PolicyStore::put` (and any mutation that today absorbs a unique-constraint race by
rollback-then-re-read) cannot do that inside a caller-owned txn — a mid-txn unique violation puts
Postgres into an aborted state, so every later statement (the outbox/audit inserts, the commit)
fails and the re-read is impossible. **Fix:** `put_in`/`delete_in` wrap their write in a
**SAVEPOINT** (`Transaction::savepoint`, a SeaORM nested txn); a unique violation rolls back only
the savepoint, then the winner is re-read **within the same UoW txn**, preserving idempotent-
absorb vs. same-id-`Conflict`. The plan audits each mutation for intra-method conflict handling;
only those need a savepoint. The SMA-444 conflict-absorption test suite must stay green.

### Error taxonomy (challenge finding)

An `outbox.enqueue`/`audit.record` failure now rolls back a mutation that previously succeeded
(the correct atomicity tradeoff, but a new failure mode). It maps to the use-case's existing
internal-error variant and surfaces as a clean 5xx (never raw backend text — the D7 posture),
distinct from business `Conflict`/`Forbidden`. Covered by a test.

---

## 7. Denial-audit path (D3 full trail, realised via D8 non-blocking buffer)

`CedarAuthorizer` keeps recording `AuthzDecisionEvent` through `AuditSink`. Changes:

- A **persistent** denial sink builds an `AuditEntry{outcome: Denied, determining_policies, …}`
  and **enqueues it to a bounded in-process async buffer** (`tokio::sync::mpsc` or an
  `ArrayQueue`). A drain task `record_out_of_band`s them to `audit_log`. **The `is_authorized`
  path only ever does a non-blocking enqueue — it never `await`s a Postgres `INSERT`.** This
  satisfies G2 ("never stall") on *every* branch, including the new cache-hit-Deny branch (which
  today does zero Postgres I/O — we keep it that way).
- **Bounded + observable loss (D8):** on buffer saturation, **drop-oldest** and increment a
  `dropped_denial_audits` counter (a metric + a `warn` log) — loss is *observable*, never
  silent. This is the refinement of D3: every denied attempt is recorded under normal load;
  under an extreme denial flood the trail degrades gracefully instead of turning into a
  write-amplification/DoS vector against the shared connection pool.
- **Durability contract (stated, D8):** mutation audit rows are exactly-once; **denial audit
  rows are best-effort** (may be dropped under saturation, observably). Documented on the port
  and in the RUNBOOK.
- Allows are **never** persisted. Because `Authorize::check` routes through this `CedarAuthorizer`,
  **denied mutation attempts are already captured** — no work at mutation sites.
- `occurred_at` for denial rows is wall-clock `Utc::now()` (the authz module declines a `Clock`);
  tests don't assert on it.

---

## 8. Relay (transactional-outbox drain, D4 + D12)

A background task spawned in `main.rs` (mirrors `spawn_reload` + shutdown-watch):

- Poll `SELECT … FROM event_outbox WHERE published_at IS NULL AND parked = false ORDER BY id
  FOR UPDATE SKIP LOCKED LIMIT batch_size` — **`FOR UPDATE SKIP LOCKED` makes it multi-replica
  safe**: two replicas never grab the same rows, so no steady-state double-publish and no
  lost-update on `attempts` (D12).
- Per row: `EventPublisher::publish`. Success → `UPDATE published_at = now()`. Failure →
  `attempts += 1`; when `attempts ≥ [outbox].max_attempts`, set `parked = true` and emit a
  poison log + metric (a permanently-failing event stops retrying instead of spinning forever).
- **Delivery: at-least-once** (a crash between publish and mark redelivers); **idempotency is the
  consumer's responsibility** (documented). Global order by `id` (UUIDv7, time-ordered);
  per-aggregate causal ordering is out of scope v1.
- Only impl: **`TracingEventPublisher`**. Real brokers land behind the same port later.
- **Relay self-observability:** each tick emits structured telemetry — drained count, oldest-
  unpublished age (backlog lag), publish-failure count, parked count — so #3's dashboards have a
  substrate.

Config `[outbox]`: `poll_interval_secs`, `batch_size`, `max_attempts`, and `relay_enabled` (see
§10).

---

## 9. Query surface (D5 — HTTP + gRPC)

- **Port:** `AuditLog::query(&AuditFilter)`, impl `PgAuditLog` (indexed, partition-pruned reads).
- **New Cedar action `ListAuditLog`**, Root-scoped (platform-admin only) — added to the embedded
  schema (`authz/schema.rs`) + starter roles/policies, mirroring `ListPolicies`/`ListRoleGrants`.
  Both read surfaces authorize `ListAuditLog` at `root_prn()` before querying. *Adding an action
  is additive to the embedded schema — verify `:parity-corpus-drift`/`:breaking` stay green
  (should be additive-clean).*
- **Bounded reads (challenge):** `limit` capped by the existing `Page` cap; **keyset (cursor)
  pagination on the time-ordered `id`** (not `OFFSET`, which is O(offset) on a growing append-
  only table); a default + max time window on `from`/`to` (unbounded = full partition scan).
- **HTTP:** `GET /v1/audit?actor=&resource=&action=&outcome=&from=&to=&cursor=&limit=`
  (reuses `http::dto`/error patterns). PII: rows carry only PRNs/action/policy ids — no tokens,
  claims, or secrets — so exposure via the API is bounded by design.
- **gRPC:** new `AuditService.ListAuditEntries` in
  `contracts/proto/paigasus/iam/v1/iam.proto` (`buf format -w` + regenerate rs/py/ts bindings —
  the FILE_DESCRIPTOR_SET shifts) + a `grpc/audit.rs` handler → parity with the other services.

---

## 10. Config, CI, gate considerations

- New config: `[outbox] { relay_enabled, poll_interval_secs, batch_size, max_attempts }` in
  `IamConfig` + `iam.toml.example`, with `validate()` bounds (non-zero interval/batch/attempts).
  **Audit persistence is always-on** — there is no `[audit].enabled=false` (it would violate G1
  by skipping the in-txn mutation insert). `[outbox].relay_enabled=false` **only** skips spawning
  the relay task; **outbox rows still accrue transactionally** (safe), and a start-up `warn`
  fires when the relay is disabled (an undrained backlog is unbounded).
- **No new crate** → `:affected-smoke` + the kernel→bindings strict set are untouched.
  `serde_json`/`tokio`/`sea-orm`/`chrono`/`uuid` already deps → no new `deny`/`machete` waivers
  expected (add a temporary `machete` `ignored` only if a dep is consumed a commit later than
  introduced).
- **Proto drift:** editing `iam.proto` needs `buf format -w` + a bindings regen or
  `contracts:fmt`/codegen-drift reds `moon ci` silently. Run the full gate list —
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
  :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` — before
  pushing.
- Migration `m0006` (with `down`) registered in `persistence/migration/mod.rs`.

---

## 11. Testing strategy

- **Unit (in-memory fakes):** per-use-case event/entry construction (type/action/aggregate/
  correlation/detail); the denial buffer records under normal load, **drops-oldest + bumps the
  metric** under saturation, and **never blocks** `is_authorized`; secret material never
  serializes into `detail`/`payload`; NUL sanitisation. *Note (challenge):* true **rollback
  atomicity is NOT unit-testable** with the current independent `Mutex<Vec<_>>` fakes (no shared
  txn) — it is asserted at the Postgres layer (below); the unit fakes assert construction, not
  atomicity.
- **Integration (real Postgres via `tests/support`):**
  - **Atomicity (G1/G4):** a forced mutation failure leaves **no** audit/outbox rows; success
    writes exactly aggregate + one event + one entry sharing a `correlation_id` (G5).
  - **AC1 over the UoW path:** grant → immediate second decision Allows (D10).
  - **Savepoint (D9):** a racing `PutPolicy` still absorbs idempotent same-content and rejects
    same-id different-content — the SMA-444 suite stays green through the UoW rewrite.
  - **Query:** filters + keyset pagination + `ListAuditLog` authz (non-admin denied on both HTTP
    and gRPC).
  - **Relay (D12):** `FOR UPDATE SKIP LOCKED` — two concurrent relay loops don't double-publish
    the same row; a failing publisher increments `attempts` and parks at `max_attempts`.
  - **Post-commit ordering (D2):** the gen-bump does **not** happen for a rolled-back mutation.

---

## 12. Decision log

| # | Decision | Rationale |
|---|---|---|
| **D1** | Outbox = **transactional** + **drain relay** via `EventPublisher` (tracing impl); broker deferred | Durable at-least-once without broker infra |
| **D2** | Mutation + outbox + audit in **one Postgres txn**; Redis gen-bump/cache-evict → **post-commit** | Redis isn't transactional with PG; bump-for-rolled-back would be a bug |
| **D3** | Audit **every denied attempt** incl. cache-hit denials; allows never persisted | Full security trail; AC = mutations + denials |
| **D4** | **Unit-of-Work** — app owns the txn boundary + event meaning | DDD-clean; app holds actor/context; ports stay DB-agnostic |
| **D5** | Query = **HTTP `/v1/audit` + gRPC `AuditService`** | Parity with other IAM services |
| **D6** | Two **separate** tables (`audit_log`, `event_outbox`) | Different lifecycles/retention |
| **D7** | Fail-closed authz posture **deferred** | Posture change w/ availability tradeoffs, not audit/outbox |
| **D8** | Denial audit via a **bounded async buffer** (drop-oldest + `dropped_denial_audits` metric), never awaited on `is_authorized` | Realises D3 without stalling decisions or amplifying denial floods; denial rows are best-effort/observably-lossy |
| **D9** | Conflict-absorbing mutations (`put`/…) use a **SAVEPOINT** inside the UoW txn | Preserve tested rollback-then-re-read absorption that a shared txn would break |
| **D10** | Post-commit side-effects are **awaited** before the use-case returns | Preserve M3 AC1 (grant visible to next decision) |
| **D11** | One **correlation id** per UoW / per denial request, stamped on audit + outbox rows; `IdGenerator` gains event/audit/correlation ids | Stitchable trail (G5); no independent-UUID orphaning |
| **D12** | Relay poll uses **`FOR UPDATE SKIP LOCKED`** + `parked`/`max_attempts`; index `(id) WHERE published_at IS NULL AND NOT parked` | Multi-replica safety (no every-tick double-publish / lost-update); poison rows don't spin |
| **D13** | Implement in **two slices** (MVP-first): (A) audit_log + denial sink + query, then (B) UoW + outbox + mutation-audit + relay | Slice A ships the highest security value with the smallest change, independent of the UoW refactor |
| **D14** | Denial + mutation rows share one `audit_log` table, but volume is bounded (D8) and **retention/partitioning is owned by this design** (monthly partitions, outcome-aware retention) | One-table query simplicity for "the audit log", without unbounded denial bloat of mutation-row indexes |

## 13. Implementation slices (D13)

- **Slice A — audit + query (MVP, independent of the UoW):** `m0006` `audit_log` (+ partitions);
  `PgAuditLog` (`record_out_of_band` + `query`); the bounded denial buffer + drain task wired
  into `CedarAuthorizer`/`AppState`; cache-hit-Deny capture; `ListAuditLog` action; HTTP + gRPC
  query surface. Ships persistent denials + queryability fast. **PR 1 ("Part of SMA-446").**
- **Slice B — outbox + UoW + mutation-audit:** `event_outbox`; the `UnitOfWork`/`Transaction`/
  `Savepoint`/`Outbox` ports + `Pg*` txn-scoped variants; the 9 use-case rewrites (with
  correlation + post-commit side-effects + savepoints); the relay + `TracingEventPublisher`.
  **PR 2 ("Part of SMA-446").**

  Both slices are designed together (this spec); the plan may still land B as one PR if review
  size allows, but A-first is the sequencing.

## 14. Follow-ups (new Linear issues)

- **SMA-446 #2** Gateway + IAM integration · **#3** Prometheus dashboards + RUNBOOK.
- **Authz hardening** — fail-closed-on-outage option + revocation-during-outage test (D7).
- **Outbox pruning** — prune published rows; **full dead-letter** subsystem for parked events.
- **Real `EventPublisher`** — broker binding once a consumer exists.
- **Correlation from upstream** — the gateway (#2) sets a request correlation id the UoW adopts.

## 15. Changelog — Stage-2 challenge fold-in (rev 2)

Folded (all challenge findings were justified): denial sink → **non-blocking bounded async
buffer** with observable drop (BLOCKER 1 → D8); **savepoints** for conflict-absorbing mutations
(BLOCKER 2 → D9); post-commit side-effects **awaited** to preserve AC1 (MAJOR → D10); **correlation
id** minted now (MAJOR → D11); relay **`FOR UPDATE SKIP LOCKED`** + `parked`/`max_attempts` + fixed
index (MAJOR → D12); `AuditLog::record_out_of_band` for the denial path (MAJOR → §5); `IdGenerator`
event/audit/correlation ids (MAJOR → §5); **error taxonomy** for the audit/outbox availability
coupling (MAJOR → §6); documented **durability tiers** + `dropped_denial_audits` metric (MAJOR →
§7); one-table audit with **owned retention/partitioning** (MAJOR → D14); **MVP-first
sequencing** (MAJOR → D13); jsonb **secret-exclusion + NUL sanitisation** (§4.1); **bounded/keyset**
query (§9); **down-migration + `schema_version`** (§4); **relay telemetry + poison cap** (§8);
**config-flag semantics** (audit always-on; relay-only disable, §10); denial **`occurred_at`
wall-clock** reconciliation (§4); **`CreateUser` actor** NULL clarification (§6); atomicity is
**Postgres-tested**, not unit-tested (§11). Nothing was rejected.
