# SMA-446 (M5, sub-project 1) — IAM persistent audit log + domain-event outbox

**Status:** Design (brainstormed) · **Date:** 2026-07-12 · **Linear:** SMA-446 (part of; does
not close the epic) · **Service:** `paigasus-iam` (+ `paigasus-iam-core`, `contracts/`,
`paigasus-proto`)

---

## 1. Context

M5 (SMA-446) is the IAM v1 vertical slice. As written it bundles five deliverables and
presumes an AI Gateway that, in this repo, is a `fn main() {}` stub. During intake we
decomposed the epic (with Sven) into three follow-on cycles:

1. **IAM persistent audit log + domain-event outbox** ← *this spec*
2. Gateway service + IAM integration (authn via IAM keys, authz via `is_authorized`)
3. Prometheus dashboards + RUNBOOK

This spec covers **sub-project 1 only**. Its PR is marked *"Part of SMA-446"* and does **not**
auto-close the epic; #2 and #3 get their own Linear issues + spec/plan/PR cycles.

### What exists today (relevant substrate)

- `paigasus-iam` is a mature hexagonal service: tenancy, authn (OIDC), authz (Cedar), API
  keys, service accounts — all wired into one `AppState` over a single SeaORM
  `DatabaseConnection`. HTTP (`axum`) + gRPC (`tonic`) surfaces both served from `main.rs`.
- **Authz decisions** are recorded through the `AuditSink` port
  (`paigasus_iam_core::authz::ports::AuditSink`). The only impl is `TracingAuditSink`
  (log-only). `CedarAuthorizer::is_authorized` records **one** `AuthzDecisionEvent` per
  *computed* decision (on the decision-cache miss); a cache **hit** returns the memoized
  decision **without** re-auditing.
- **Mutations** (role grant/revoke, key issue/revoke, policy put/delete, principal create)
  are **not audited at all** today, and there is **no** persistent audit store, **no**
  outbox, and **no** message bus anywhere in the repo.
- Persistence adapters (`Pg*Repository`/`Pg*Store`) each **own their own transaction**
  (`db.begin()` … `txn.commit()`), and several bump the Redis-backed `entity_gen`/`policy_gen`
  counters (via a `Generations` handle threaded into the adapter constructor) and/or evict the
  API-key validation cache **inside** their mutation method. The application layer never
  imports `Generations`.
- `main.rs` already runs background tasks via a `spawn` + shutdown-watch pattern (the
  policy-snapshot `spawn_reload` loop) — the outbox relay will mirror it.

### M3 handoff (SMA-444 PR #78, ADR-0013) — two deferred decisions, now settled

1. **Audit granularity for denials** → **full trail** (see §6, Decision D3).
2. **Fail-closed-on-Redis-outage authz posture** → **deferred** to a separate authz-hardening
   issue (see §11). Current fail-open posture (revocation bounded by `policy_cache_ttl_secs`
   even during a Redis outage) is unchanged by this work.

---

## 2. Goals / Non-goals

### Goals (acceptance criteria for this sub-project)

- **G1.** Every **committed mutation** in the AC set — role granted/revoked, api-key
  issued/revoked, policy put/deleted — is written to a persistent, append-only **`audit_log`**,
  atomically with the mutation.
- **G2.** Every authz **denial** (including denied *mutation attempts* and repeated cache-hit
  denials) is written to `audit_log` with its **determining policy**, without ever failing or
  stalling a decision.
- **G3.** The audit log is **queryable** via HTTP `GET /v1/audit` **and** gRPC
  `AuditService.ListAuditEntries` (actor / resource / action / outcome / time-range filters,
  paginated, platform-admin authorized).
- **G4.** A **domain-event outbox** captures principal / role / api-key / policy events,
  written **transactionally** with the mutation, and a background **relay** drains unpublished
  rows through an `EventPublisher` port (tracing impl for now).

### Non-goals (explicitly out; tracked elsewhere)

- The AI Gateway and its IAM integration (sub-project #2).
- Prometheus dashboards + RUNBOOK (sub-project #3).
- A real message broker (Kafka/NATS) or any concrete non-tracing `EventPublisher`.
- Fail-closed authz posture / the "revocation-during-Redis-outage denied within TTL" test
  (separate hardening issue).
- Auditing **allows** (the AC scopes the audit log to *mutations + denials*; allows keep
  flowing to `TracingAuditSink` for logs only, never persisted).
- Auditing tenancy/membership mutations (org/team/project create/rename/archive,
  attach/detach). Not in the AC's audit set; easy future extension on the same UoW seam. (They
  *do* still surface as denials when unauthorized, via the denial path.)

---

## 3. Architecture overview

Two **separate** append-only Postgres tables, fed by two write paths, with a background relay:

```
                        ┌──────────────────────────────────────────────┐
  mutation use-cases    │  application layer (RoleService, PolicyService,│
  (grant/revoke,        │  ApiKeyService, ServiceAccountService,         │
   issue/revoke,        │  CreateUser) — builds DomainEvent + AuditEntry │
   put/delete,          └───────────────┬──────────────────────────────┘
   create/archive)                      │  UnitOfWork (one Postgres txn)
                                        ▼
                    ┌───────────────────────────────────────────┐
                    │  ONE txn:  aggregate row(s)                │
                    │            + INSERT event_outbox           │  atomic
                    │            + INSERT audit_log (mutations)  │
                    │            commit                          │
                    └───────────────┬───────────────────────────┘
                                    │ post-commit side-effects (NOT in txn):
                                    │   • bump entity_gen/policy_gen (Redis)
                                    │   • evict api-key validation cache
                                    ▼
   authz denials ─────────────────────────────────► INSERT audit_log (denied, fail-open)
   (CedarAuthorizer, incl. cache-hit Deny)

   relay task ── poll event_outbox WHERE published_at IS NULL ──► EventPublisher::publish
                 └──► set published_at (success) / attempts++ + backoff (failure)
```

**Why two tables, not one unified event log:** they have different lifecycles. `audit_log` is
permanent, compliance-retained, and includes **denials** (which are not state changes and not
domain events). `event_outbox` rows are **transient** — written, drained, then prunable. A
single table would force "never prune" (to keep audit history) and shoehorn denials-as-events.
Separate stores keep each concern's schema, retention, and consumers independent.

### Feed matrix

| Source | → `audit_log` | → `event_outbox` |
|---|:--:|:--:|
| user principal created (`CreateUser`) | — | ✓ |
| service-account created / archived | — | ✓ |
| role granted / revoked | ✓ | ✓ |
| api-key issued / revoked | ✓ | ✓ |
| policy put / deleted | ✓ | ✓ |
| authz **denial** (compute + cache-hit) | ✓ | — |

**Elegant consequence:** a *denied* mutation attempt (`authorize.check` → `Deny`) is captured by
the denial path as an `outcome=denied` row **before** the use-case ever reaches the UoW, so it
needs no special handling at mutation sites. Committed mutations are captured by the UoW path.
Together they give the full "who tried / who succeeded" picture the AC wants.

---

## 4. Data model (migration `m0006_create_audit_and_outbox`)

### `audit_log` (append-only, permanent)

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | UUIDv7, caller-supplied (kernel id-gen; ordered) |
| `occurred_at` | `timestamptz` | event time (injected clock) |
| `actor_prn` | `text` NULL | principal PRN; NULL = system/unknown |
| `action` | `text` | Cedar action name (`GrantRole`, `IssueApiKey`, `PutPolicy`, …) |
| `resource_prn` | `text` NULL | target/scope PRN |
| `outcome` | `text` | `committed` \| `denied` |
| `determining_policies` | `text[]` NULL | populated for `denied` rows |
| `detail` | `jsonb` | mutation-specific detail (e.g. `{role_key, scope}`) or `{}` |
| `correlation_id` | `uuid` NULL | request correlation (best-effort; may be NULL v1) |

Indexes: `(occurred_at DESC)`, `(actor_prn)`, `(resource_prn)`, `(action)`, `(outcome)`.
Append-only by convention (no `UPDATE`/`DELETE` in adapter code); retention/partitioning is a
future ops concern (RUNBOOK, #3).

### `event_outbox` (transient, drained)

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | UUIDv7, caller-supplied (ordered) |
| `occurred_at` | `timestamptz` | event time |
| `event_type` | `text` | stable wire string (`iam.role.granted`, …) |
| `aggregate_prn` | `text` | the entity the event is about |
| `actor_prn` | `text` NULL | who caused it |
| `payload` | `jsonb` | event body |
| `published_at` | `timestamptz` NULL | set by relay on success |
| `attempts` | `int` NOT NULL default 0 | incremented on publish failure |
| `correlation_id` | `uuid` NULL | |

Index: **partial** `(occurred_at) WHERE published_at IS NULL` (the relay's hot poll). Published
rows are retained until a future pruning job (out of scope; documented).

**Event types (this run):** `iam.principal.created`, `iam.principal.archived`,
`iam.role.granted`, `iam.role.revoked`, `iam.api_key.issued`, `iam.api_key.revoked`,
`iam.policy.put`, `iam.policy.deleted`.

---

## 5. Domain model & ports (`paigasus-iam-core`)

All new core types are **kernel-friendly**: ids and timestamps are injected (no `getrandom`,
no ambient clock), so nothing here can break the `wasm-getrandom-free` posture of shared crates.

```rust
// New value types (pure)
pub struct DomainEvent {
    pub id: Uuid,
    pub event_type: EventType,          // enum → stable wire string via as_wire()
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub correlation_id: Option<Uuid>,
}

pub struct AuditEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_prn: Option<String>,
    pub action: String,
    pub resource_prn: Option<String>,
    pub outcome: AuditOutcome,          // Committed | Denied
    pub determining_policies: Vec<String>,
    pub detail: serde_json::Value,
}

pub struct AuditFilter { /* actor, resource, action, outcome, from, to, limit, offset */ }
```

### Ports (traits)

```rust
// Transaction boundary owned by the application layer.
#[async_trait] pub trait UnitOfWork: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError>;
}
#[async_trait] pub trait Transaction: Send {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError>;
    // rollback on drop-without-commit
}

// Txn-scoped writers — take &dyn Transaction (opaque; adapter recovers the SeaORM txn).
#[async_trait] pub trait Outbox: Send + Sync {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError>;
}
#[async_trait] pub trait AuditLog: Send + Sync {
    async fn record(&self, tx: &dyn Transaction, entry: &AuditEntry) -> Result<(), RepositoryError>;
    async fn query(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError>; // read
}

// Relay egress.
#[async_trait] pub trait EventPublisher: Send + Sync {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError>;
}
```

**Existing mutation store ports** (`RoleGrantStore`, `PolicyStore`, `ApiKeyRepository`,
`ServiceAccountRepository`, `PrincipalRepository`) gain **txn-scoped variants** of their write
methods that accept `&dyn Transaction` instead of opening their own txn. The current
own-transaction methods either delegate to the txn-scoped form (wrapping a one-shot UoW) or are
migrated — decided per-method in the plan to minimise churn to read paths.

**Recommended Rust mechanism (finalise in plan):** the concrete `Transaction` wraps a SeaORM
`DatabaseTransaction`; each `Pg*` adapter downcasts `&dyn Transaction` (via `Any`) to recover
it. This keeps the per-aggregate ports **separate** (no god-object) and `dyn`-safe. Alternative
considered — `UnitOfWork::begin` yields a bundle of typed txn-bound repo facades built by the
adapter (no downcast, but a wider bundle interface + closure/lifetime ergonomics). A ~1-file
spike in the plan settles which; the spec is agnostic to the choice.

---

## 6. Write path — Unit-of-Work (Decisions D1, D2, D4)

Each instrumented use-case is rewritten from "call one own-txn store method" to "own the txn":

```rust
// RoleService::grant, after the existing authorize + resolve checks:
let event = DomainEvent::role_granted(&grant, actor, self.ids.new_event_id(), now);
let entry = AuditEntry::committed("GrantRole", actor, &scope_prn, detail_json, …);

let tx = self.uow.begin().await?;
self.grants.grant_in(&*tx, &grant).await?;      // aggregate write, no own txn
self.outbox.enqueue(&*tx, &event).await?;       // domain event, same txn
self.audit.record(&*tx, &entry).await?;         // audit entry, same txn
tx.commit().await?;

// post-commit side-effects (Redis / cache — NOT part of the Postgres txn):
self.after_commit.run(&[SideEffect::BumpPolicyGen, …]).await;  // best-effort
```

**Instrumented sites (9):** `RoleService::grant`/`revoke`, `ApiKeyService::issue`/`revoke`,
`PolicyService::put`/`delete` (audit + outbox); `CreateUser::execute`,
`ServiceAccountService::create`/`archive` (outbox only). The application layer builds the
`DomainEvent` + `AuditEntry` because only it holds the **actor** and request context.

### D2 — the generations-bump / cache-evict wrinkle (important)

Today the `Pg*` adapters bump `entity_gen`/`policy_gen` (Redis) and evict the api-key cache
**inside** their mutation method. Redis is **not** part of the Postgres transaction, and a bump
for a rolled-back mutation would be a correctness bug. Resolution: these become **post-commit
side-effects** the UoW (or the use-case, immediately after `commit`) runs **only on success**,
never inside the txn. They stay **best-effort/fail-open** (a bump/evict failure is a
cache-freshness degradation bounded by TTL, not a correctness failure — matches today's D11/D12
posture). The application layer still does not import `Generations`; the side-effect set is
expressed as an injected `AfterCommit` port (or the adapters expose post-commit hooks) —
mechanism finalised in the plan, keeping the existing hexagonal boundary intact.

---

## 7. Denial-audit path (Decision D3 — full trail)

`CedarAuthorizer` keeps recording `AuthzDecisionEvent` through the `AuditSink` port. Changes:

- A **persistent** sink writes `outcome=denied` rows (with `determining_policies`) to
  `audit_log`. It is composed **alongside** `TracingAuditSink` (a small fan-out sink, or
  `AppState` wires the persistent one and keeps tracing for logs). Allows are **never**
  persisted.
- **Fail-open (hard requirement):** the persistent write is best-effort — an error (or slow
  store) is logged and swallowed; it must **never** fail or stall `is_authorized`. v1 is a
  synchronous best-effort `INSERT`; an async-buffered writer is a noted future optimisation if
  denial volume ever pressures decision latency.
- **Cache-hit denials are recorded (full trail).** The one hot-path change in `is_authorized`:
  in the decision-cache-hit branch, when the memoized decision is a `Deny`, still record the
  denial audit entry (allows on a hit remain un-recorded). This lets the trail see repeated
  probing (`principal X hit denied resource 500×`). The existing "no double-audit for allows"
  behaviour is preserved.

Because `Authorize::check` (used by every mutation use-case) routes through this same
`CedarAuthorizer`, **denied mutation attempts are already captured** as `denied` rows — no extra
work at mutation sites.

---

## 8. Relay (transactional-outbox drain, Decision D4)

A background task spawned in `main.rs`, mirroring the existing `spawn_reload` + shutdown-watch
pattern:

- Poll `event_outbox WHERE published_at IS NULL ORDER BY id LIMIT batch_size`.
- For each row: `EventPublisher::publish(&event)`. On success → `UPDATE published_at = now()`.
  On failure → `UPDATE attempts = attempts + 1` and leave unpublished (retried next tick with
  backoff derived from `attempts`).
- **Delivery semantics: at-least-once.** A crash between `publish` and marking published
  re-delivers; **idempotency is the consumer's responsibility** (documented on the port). Global
  ordering by `id` (UUIDv7 → time-ordered); strict per-aggregate ordering is out of scope v1.
- Only impl: **`TracingEventPublisher`** (emits a structured `tracing` event per domain event).
  Real brokers land behind the same port later.

Config block `[outbox]`: `enabled` (bool), `poll_interval_secs`, `batch_size`. When disabled,
the relay task isn't spawned (rows still accrue transactionally — safe).

---

## 9. Query surface (Decision D5 — HTTP + gRPC)

- **Port:** `AuditLog::query(&AuditFilter)` (§5), impl `PgAuditLog` doing indexed reads with the
  filter columns + `LIMIT/OFFSET`.
- **New Cedar action `ListAuditLog`**, Root-scoped (platform-admin only) — added to the embedded
  authz schema (`authz/schema.rs`) and the starter roles/policies, mirroring `ListPolicies`/
  `ListRoleGrants`. Both read surfaces authorize `ListAuditLog` at `root_prn()` before querying.
- **HTTP:** `GET /v1/audit?actor=&resource=&action=&outcome=&from=&to=&limit=&offset=` returning
  a paginated DTO (reuses the existing `http::dto`/error patterns).
- **gRPC:** new `AuditService.ListAuditEntries(ListAuditEntriesRequest) → ListAuditEntriesResponse`
  in `contracts/proto/paigasus/iam/v1/iam.proto`. Requires `buf format -w` + regenerating the
  rs/py/ts bindings (known drift gotcha — a whitespace/descriptor shift means regenerating the
  embedded `FILE_DESCRIPTOR_SET`), and a `grpc/audit.rs` handler, giving parity with
  Tenancy/Authn/Authz/ServiceAccount (both surfaces).

---

## 10. Config, CI, and gate considerations

- New config: `[audit] { enabled }`, `[outbox] { enabled, poll_interval_secs, batch_size }` in
  `IamConfig` + `iam.toml.example`, with `validate()` bounds (non-zero interval/batch).
- **No new crate** — everything lives in existing `paigasus-iam-core` + `paigasus-iam` (+ proto).
  So `:affected-smoke` and the kernel→bindings strict set are **unaffected**.
- `serde_json`, `tokio`, `sea-orm`, `chrono`, `uuid` are already workspace deps → no new
  `deny.toml`/`machete` waivers expected. (Confirm during implementation; add a temporary
  `machete` `ignored` entry only if a dep is consumed a commit later than introduced.)
- **Proto drift:** editing `iam.proto` needs `buf format -w` before commit and a bindings regen,
  or `contracts:fmt`/codegen-drift reds `moon ci` (silently). Run the full gate list —
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
  :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` — before
  pushing. Adding a **new** `AuditService` RPC (not modifying an existing message) should be
  `:breaking`-clean, but verify.
- New migration `m0006` registered in `persistence/migration/mod.rs`.

---

## 11. Testing strategy

- **Unit (in-memory fakes; follows existing `application/fakes.rs` posture):**
  - UoW atomicity: a rolled-back mutation (e.g. a store error) leaves **no** outbox/audit rows;
    a committed one writes **exactly** the aggregate + one event + one entry.
  - Per-use-case event/entry construction (correct `event_type`, `action`, `aggregate_prn`,
    `detail`).
  - Denial audit: a cache-**hit** `Deny` records an entry; a cache-hit `Allow` does not; a
    persistent-sink error does **not** fail `is_authorized` (fail-open).
  - Relay: drains unpublished rows, marks `published_at`, increments `attempts` and retries on a
    failing publisher; a disabled relay spawns nothing.
- **Integration (real Postgres via `tests/support`):**
  - grant/revoke, key issue/revoke, policy put/delete each produce **atomic** audit + outbox
    rows; the query API returns them with correct filtering + pagination; authz on `/v1/audit`
    and gRPC `ListAuditEntries` denies a non-platform-admin.
  - Relay end-to-end: enqueue via a real mutation → relay publishes (tracing) → row marked
    published.
- **Post-commit side-effects:** a test asserting the generations bump does **not** happen for a
  rolled-back mutation (guards the D2 wrinkle).

---

## 12. Decision log

| # | Decision | Rationale |
|---|---|---|
| **D1** | Outbox = **transactional** (same txn as mutation) **+ drain relay** via `EventPublisher` (tracing impl); broker deferred | Durable at-least-once without inventing broker infra; proves the full pattern |
| **D2** | Mutation + outbox + audit in **one Postgres txn**; Redis gen-bump / cache-evict become **post-commit** best-effort side-effects | Redis isn't transactional with PG; a bump for a rolled-back mutation would be a bug; keeps `Generations` out of the app layer |
| **D3** | Audit **every denied attempt**, incl. decision-cache-hit denials; allows never persisted | Full security trail (probing/brute-force visible); AC scopes audit to mutations + denials |
| **D4** | **Unit-of-Work** orchestration — app layer owns the txn boundary + decides event meaning | DDD-clean; app holds actor/context; ports stay DB-agnostic |
| **D5** | Query surface = **HTTP `GET /v1/audit` + gRPC `AuditService`** | Parity with the other IAM services; honest "queryable" |
| **D6** | Two **separate** tables (`audit_log`, `event_outbox`), not one unified event log | Different lifecycles/retention; denials aren't domain events |
| **D7** | Fail-closed authz posture (M3 deferral #2) **deferred** to a separate hardening issue | Authz-posture change with availability tradeoffs, not an audit/outbox concern |

## 13. Risks / open items for the plan

- **Size.** UoW touches ~9 mutation sites + the denial path + a relay + 2 tables + a query API
  (HTTP + gRPC + proto). Large for one PR. The plan sequences commits so the **UoW + outbox
  backbone** lands first and **audit + query** build on it; split into two PRs only if the single
  review proves unwieldy. (Both PRs would still be "Part of SMA-446".)
- **UoW ergonomics.** The opaque-`Transaction`-downcast vs. typed-bundle choice (§5) — a small
  spike in the plan; low risk, well-trodden Rust pattern either way.
- **Own-txn method migration.** Some read/write store methods keep their own txn; only the
  instrumented mutations need the txn-scoped variant. Minimise churn to untouched paths.
- **Correlation id.** Best-effort v1 (may be NULL); a request-scoped correlation id is a natural
  follow-up once the gateway (#2) sets one upstream.

## 14. Follow-ups (new Linear issues to open)

- **SMA-446 #2** — Gateway service + IAM integration.
- **SMA-446 #3** — Prometheus dashboards + RUNBOOK.
- **Authz hardening** — fail-closed-on-outage option + revocation-during-outage test (D7).
- **Outbox pruning** — background prune of published `event_outbox` rows.
- **Real `EventPublisher`** — broker binding once a consumer exists.
