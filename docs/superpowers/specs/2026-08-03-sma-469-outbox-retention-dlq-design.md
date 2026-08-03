# SMA-469 — outbox retention + a real dead-letter path for parked events

**Status:** approved (2026-08-03)
**Linear:** [SMA-469](https://linear.app/smaschek/issue/SMA-469/iam-outbox-retention-a-real-dead-letter-path-for-parked-events)
**Related:** SMA-446 (surfaced the gap), SMA-467 (`audit_log`'s equivalent treatment), SMA-471 (real broker publisher)

## 1. Problem

`event_outbox` has neither retention nor a dead-letter subsystem. Two distinct leaks:

1. **Published rows accumulate forever.** `OutboxRelay::tick` sets `published_at` and moves on.
   Nothing ever deletes them; the table grows monotonically with every audited mutation for the
   life of the deployment.
2. **Parked rows are a dead end, not a dead letter.** After `max_attempts` publish failures a row
   gets `parked = true` and is permanently excluded from the relay's poll predicate
   (`published_at IS NULL AND parked = false`). There is no way to inspect, replay, or retire it.
   `IamOutboxEventsParked` alerts an operator that a poison event exists; the runbook then offers
   only hand-written `psql` `UPDATE`/`SELECT` snippets.

`audit_log` got exactly this treatment in SMA-467 — `LIST(outcome) → RANGE(occurred_at)`
partitioning plus an in-app `PgPartitionMaintainer` doing create-ahead and outcome-aware
retention, with maintenance metrics and an alert. The outbox never got the equivalent, so the two
tables that grow together have very different operational stories.

### 1.1 A failure-model finding that shaped the design

At the shipped defaults (`poll_interval_secs = 5`, `max_attempts = 5`), a broker outage lasting
**~25 seconds** exhausts every retry for every row in the backlog. Mass parking is therefore the
*expected* outage signature, not a hypothetical — a poison-message-only mental model would be
wrong. This is why the design includes a filtered bulk replay rather than treating single-row
replay as sufficient.

## 2. Goals / non-goals

**Goals**

- Bound `event_outbox` growth with age-based retention for published rows.
- Give parked rows a real dead-letter path: inspect, replay (single + filtered bulk), discard.
- Decide explicitly whether parked rows age out. **They do not, by default.**
- Bring the outbox's operational story (metrics, alerts, runbook) up to `audit_log`'s level.

**Non-goals**

- No gRPC mirror of the dead-letter surface. HTTP-only keeps `contracts/` untouched and avoids
  the codegen-drift, `:breaking`, and `:release-parity*` gates. A follow-up can add it if a
  non-HTTP operator client ever needs it.
- No bulk **discard**. Replay is recoverable; deletion is not.
- No partitioning of `event_outbox` (see §3.1).
- SMA-471's real broker `EventPublisher` stays a separate issue.

## 3. Design decisions

### 3.1 Batched delete, not partitioning

`event_outbox` is a *drained queue*, not a durable record of truth — `audit_log` is the trail. Its
steady-state size is `retention window × mutation rate`, which is small. Partitioning would add a
`FOR UPDATE SKIP LOCKED` scan across N leaves, per-leaf partial indexes, and create-ahead
machinery to solve a size problem that does not exist. A bounded, batched age-based `DELETE` on a
maintenance tick is the right shape, and is what the issue itself proposes.

### 3.2 A dedicated maintenance task, not a fold into the relay tick

`PgOutboxMaintainer` is its own background task mirroring `PgPartitionMaintainer` — own hourly
interval, own metrics, own alert, `tick(now, policy) -> SweepReport` plus a `run` shutdown-watch
loop spawned from `main.rs`.

Folding retention into `OutboxRelay::tick` was rejected: it couples a 5-second hot loop to an
hourly bulk `DELETE`, makes tick latency lumpy, and — decisively — muddies
`iam_outbox_relay_ticks_total`, so a retention failure would red the relay's own liveness signal.

### 3.3 Parked rows never age out by default

`published_days` defaults to `7`; `parked_days` defaults to `0`. **`0` means "never" for both
windows** — one meaning for the sentinel across the whole block. A non-zero `parked_days` is
opt-in and emits a startup `warn!`, mirroring `audit.retention.committed_months > 0` exactly:
auto-deleting the very thing an operator is alerted to inspect must be a deliberate choice.

### 3.4 Parked rows stay in `event_outbox`

`parked = true` already *is* the dead-letter predicate. A dedicated `event_dead_letter` table was
considered and rejected: it costs a migration, a move-on-park inside the relay's transaction, a
move-back replay path, and renders the `parked` column vestigial — all to express a set that one
boolean already expresses cleanly.

## 4. Unit 1 — schema (m0009)

```sql
ALTER TABLE event_outbox
  ADD COLUMN parked_at  TIMESTAMPTZ NULL,
  ADD COLUMN last_error TEXT NULL;

CREATE INDEX ix_event_outbox_published ON event_outbox (published_at)
  WHERE published_at IS NOT NULL;      -- retention's published-sweep predicate
CREATE INDEX ix_event_outbox_parked ON event_outbox (id)
  WHERE parked = true;                  -- DLQ list ordering + keyset paging
```

Both columns are load-bearing, not conveniences:

- **`parked_at`** — `parked_days` must measure from *when the row parked*, not `occurred_at`.
  Without it, a week-old event that parks today would be deleted on the very next tick.
- **`last_error`** — today the parking reason exists only in a `tracing::error!` line. An
  inspection surface that cannot say *why* a row is dead is not an inspection surface.

**Pre-existing parked rows** get `parked_at = NULL`. The retention predicate requires
`parked_at IS NOT NULL`, so a dead letter that predates this migration is never silently deleted
by a window it predates — it must be retired through the API. The migration deliberately does
**not** backfill `parked_at = occurred_at`, which would make old rows immediately eligible.

`down` drops both indexes and both columns.

Follows m0007's precedent: partial-index predicates go through `execute_unprepared` raw SQL
(sea-query has no builder support for them).

## 5. Unit 2 — the relay records park metadata

In `OutboxRelay::tick`'s `Err(reason)` branch:

```rust
active.last_error = Set(Some(truncate_error(&reason)));
let attempts = row.attempts + 1;
active.attempts = Set(attempts);
if attempts >= self.max_attempts {
    active.parked = Set(true);
    active.parked_at = Set(Some(Utc::now()));
    // ...existing report.parked bookkeeping + tracing::error!
}
```

`last_error` is written on **every** failed attempt, not only at parking — an operator watching
`attempts` climb wants the current reason. Both writes ride the transaction the tick already has;
no extra round-trip.

`truncate_error` bounds the string at 1024 chars, cutting on a `char_indices` boundary and
appending `…`, so a pathological publisher error string cannot bloat the row.

`TickReport` and every existing relay behavior are unchanged.

## 6. Unit 3 — retention sweeper

### 6.1 Config

```toml
[outbox.retention]
enabled        = true    # false = don't spawn the sweeper at all
interval_secs  = 3600    # >= 1
published_days = 7       # 0 = never delete published rows
parked_days    = 0       # 0 = never delete parked rows (default); > 0 warns at startup
batch_size     = 1000    # >= 1; rows per delete pass
```

`OutboxRetentionConfig` nests under the existing `OutboxConfig`, mirroring how `RetentionConfig`
nests under `AuditConfig`. Every field has a default, so an absent block is valid config.
`IamConfig::validate` rejects `interval_secs == 0` and `batch_size == 0`; both `*_days` may be `0`.

`MAX_BATCHES_PER_TICK = 50` is a hardcoded const, not a knob. It bounds a first-run sweep over a
large backlog to `50 × batch_size` rows per tick, resuming on the next tick rather than holding
one tick open indefinitely.

### 6.2 `PgOutboxMaintainer`

New adapter `adapters/persistence/pg_outbox_maintainer.rs`, mirroring `PgPartitionMaintainer`'s
shape:

```rust
pub struct OutboxRetentionPolicy { pub published_days: u32, pub parked_days: u32, pub batch_size: u64 }  // Copy

pub struct SweepReport {
    pub deleted_published: u64,
    pub deleted_parked: u64,
    pub parked_rows: i64,
    pub errored: bool,
}

impl PgOutboxMaintainer {
    pub fn new(db: DatabaseConnection) -> Self;
    pub async fn tick(&self, now: DateTime<Utc>, policy: OutboxRetentionPolicy) -> SweepReport;
    pub async fn run<S: Future<Output = ()> + Send>(self, policy: OutboxRetentionPolicy, interval: Duration, shutdown: S);
}
```

A tick does three **independent** steps — one failing must not wedge the others, exactly as
`PgPartitionMaintainer::tick` runs `prune` regardless of create-ahead's outcome:

1. If `published_days > 0`: batched delete where
   `published_at IS NOT NULL AND published_at < now - published_days`.
2. If `parked_days > 0`: batched delete where
   `parked = true AND parked_at IS NOT NULL AND parked_at < now - parked_days`.
3. Refresh the backlog gauge: `SELECT count(*) FROM event_outbox WHERE parked = true`.

Errors from any step are logged, set `errored = true`, and are never propagated — the loop keeps
running.

### 6.3 The delete statement

```sql
DELETE FROM event_outbox WHERE id IN (
  SELECT id FROM event_outbox
  WHERE published_at IS NOT NULL AND published_at < $1
  ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
)
```

Looped until a pass affects fewer than `batch_size` rows or `MAX_BATCHES_PER_TICK` is reached.
Each pass is its own autocommit statement — never one long transaction holding locks.

**Retention and the relay cannot contend, by construction.** The relay's poll predicate is
`published_at IS NULL AND parked = false`; both delete predicates are subsets of its exact
complement. No row is ever visible to both. `FOR UPDATE SKIP LOCKED` is present for a different
reason: two *maintainer* replicas partition the work cleanly instead of one blocking on the other,
mirroring the relay's own multi-replica posture.

### 6.4 Metrics

Added to `paigasus-observability::names` (and therefore `names::ALL`), described in
`describe_iam_metrics()`:

| Metric | Type | Meaning |
|---|---|---|
| `iam_outbox_retention_ticks_total{result}` | counter | Sweep ticks; `result=ok\|error`. The liveness signal. |
| `iam_outbox_rows_deleted_total{reason}` | counter | Rows deleted; `reason=published\|parked`. |
| `iam_outbox_parked_rows` | gauge | Current parked-row count — the dead-letter backlog. |
| `iam_outbox_dead_letters_replayed_total{scope}` | counter | Replays; `scope=one\|bulk`. |
| `iam_outbox_dead_letters_discarded_total` | counter | Dead letters permanently discarded. |

`iam_outbox_parked_rows` also retires the runbook's awkward
"derive the parked count from `sum(increase(iam_outbox_relay_parked_total[…]))`" guidance.

### 6.5 Alerts

In `ops/observability/prometheus/rules/iam.rules.yml`:

- **`IamOutboxRetentionStalled`** (warning) —
  `sum without (result) (increase(iam_outbox_retention_ticks_total[2d])) == 0` for `1h`.
  Mirrors `IamAuditPartitionMaintenanceStalled`, whose long window suits a daily/hourly task.
- **`IamOutboxDeadLetterBacklog`** (warning) — `iam_outbox_parked_rows > 0` for `1h`.

The two parked-event alerts are complementary, not redundant: `IamOutboxEventsParked` is the
*something just parked* edge signal; `IamOutboxDeadLetterBacklog` is the *nobody has dealt with it*
level signal.

Rule fixtures go in `ops/observability/prometheus/rules/tests/iam.test.yml`. Each new case
includes a **control series that must not fire**, so a wrong comparison operator cannot pass an
all-firing fixture.

## 7. Unit 4 — the dead-letter surface

### 7.1 Cedar actions

Three new Root-scoped actions in `paigasus-iam-core`:

| Action | `is_write` |
|---|---|
| `ListOutboxDeadLetters` | `false` |
| `ReplayOutboxDeadLetter` | `true` |
| `DiscardOutboxDeadLetter` | `true` |

Added to the `Action` enum, `Action::ALL` (appended after `ListAuditLog`), `as_wire`, the
`is_write` arms, the `all_covers_every_variant` exhaustiveness match, and `SCHEMA_SRC`'s action
declaration. `roles.rs` is untouched: `platform_admin`'s template is action-less
(`permit(principal == ?principal, action, resource in ?resource)`) so it already covers them, and
no other role should have them.

Root-only-ness is enforced by the *service* always authorizing at `root_prn()` — the schema's
shared `appliesTo` block does not restrict it. This mirrors `AuditQueryService::list` exactly.

### 7.2 Core types and port

```rust
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,          // raw wire string — see below
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,             // raw serialized TEXT — see below
    pub correlation_id: Option<Uuid>,
    pub attempts: i32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub struct DeadLetterFilter {
    pub event_type: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,
}
```

`DeadLetterFilter::capped_limit()` clamps to `MAX_LIMIT` (mirroring `AuditFilter`);
`is_unfiltered()` is true when `event_type`, `from`, and `to` are all absent.

**`event_type` and `payload` are deliberately raw `String`s**, not `EventType` and
`serde_json::Value`. An unrecognized `event_type` wire string and invalid `payload` JSON are two
of the three reasons `row_to_domain_event` rejects a row — i.e. two of the reasons a row parks at
all. Typing them strictly would make the dead-letter surface unable to display exactly the rows it
exists to explain.

```rust
#[async_trait]
pub trait DeadLetters: Send + Sync {
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError>;
    async fn replay_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
    async fn replay_matching_in(&self, tx: &dyn Transaction, f: &DeadLetterFilter) -> Result<u64, RepositoryError>;
    async fn discard_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
}
```

The mutating methods take the caller's transaction (recovered via `uow::recover_txn`, as
`PgOutbox::enqueue` does) so the mutation and its audit entry commit atomically on one
`UnitOfWork` — the reference pattern established in `application/roles.rs`. They use
`... RETURNING *` and return `Option`, so `None` means "no parked row with that id" and the row's
contents are available for the audit detail.

### 7.3 Adapter — `PgDeadLetters`

```sql
-- replay_in
UPDATE event_outbox SET parked = false, attempts = 0, last_error = NULL, parked_at = NULL
WHERE id = $1 AND parked = true RETURNING *;

-- discard_in
DELETE FROM event_outbox WHERE id = $1 AND parked = true RETURNING *;

-- replay_matching_in (filter predicate abridged)
UPDATE event_outbox SET parked = false, attempts = 0, last_error = NULL, parked_at = NULL
WHERE id IN (SELECT id FROM event_outbox WHERE parked = true AND <filter> ORDER BY id LIMIT $n);
```

Note the ordering split: `list` sorts `id DESC` (newest first — what an operator inspecting a
backlog wants), but `replay_matching_in` selects `ORDER BY id` **ascending**. When a filter matches
more rows than the cap, replaying the *oldest* first preserves event order for the relay, which
drains `ORDER BY id` ascending, and lets repeated calls walk the backlog forward instead of
re-selecting the same newest slice.

Every mutating statement carries `AND parked = true`. A live or already-published row is therefore
untouchable through this surface — these endpoints cannot be used to mutate the live queue.

`list` orders by `id DESC` with keyset paging (`id < cursor`), mirroring `PgAuditLog::query`. IDs
are UUIDv7 (`KernelIdGenerator::mint`), so id order is time order.

`MAX_BULK_REPLAY = 10_000` caps `replay_matching_in`.

### 7.4 Application service

`application/dead_letters.rs`:

```rust
pub struct DeadLetterService {
    dead: Arc<dyn DeadLetters>,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
    authorize: Authorize,
}
```

- `list(actor, filter)` — authorize `ListOutboxDeadLetters` at `root_prn()`, then `dead.list`.
- `replay(actor, id)` — authorize `ReplayOutboxDeadLetter`; `tx = uow.begin()`;
  `dead.replay_in(&*tx, id)?`; `None` ⇒ drop the tx and return `NotFound`; else
  `audit.record(&*tx, &entry)`; `tx.commit()`.
- `replay_matching(actor, filter)` — authorize; **reject an unfiltered filter before touching the
  DB**; same transaction shape; the audit entry carries the filter and the replayed count.
- `discard(actor, id)` — authorize `DiscardOutboxDeadLetter`; same shape as `replay`.

Replay and discard record an audit entry but **do not** enqueue a domain event: these are
operational actions on the queue itself, not domain state changes, and an outbox event about
outbox operations would be circular.

**Discard's audit detail is complete on purpose** — `event_type`, `aggregate_prn`, `actor_prn`,
`correlation_id`, `occurred_at`, `attempts`, `last_error`, **and the payload**. A discarded dead
letter is gone forever, so the audit entry is its only remaining trace; `audit_log.detail` is TEXT
and has no reason to be lossy here. Replay's detail carries everything except the payload (the row
still exists).

### 7.5 HTTP adapter

`adapters/http/dead_letters.rs`, merged onto the bearer-gated `protected` sub-router. A thin
extract → service → DTO adapter with no business logic, mirroring `adapters/http/audit.rs`.

| Method | Path | Action | Notes |
|---|---|---|---|
| `GET` | `/v1/outbox/dead-letters` | `ListOutboxDeadLetters` | `event_type`/`from`/`to`/`cursor`/`limit`; `next_cursor` set only when the page came back full |
| `POST` | `/v1/outbox/dead-letters/replay` | `ReplayOutboxDeadLetter` | bulk; JSON body filter; `422` when unfiltered |
| `POST` | `/v1/outbox/dead-letters/{id}/replay` | `ReplayOutboxDeadLetter` | `404` when not a parked row |
| `POST` | `/v1/outbox/dead-letters/{id}/discard` | `DiscardOutboxDeadLetter` | `404` when not a parked row |

The literal `/replay` and the `/{id}/replay` routes differ in segment count, so axum's router has
no ambiguity between them.

DTOs live in `adapters/http/dto.rs` alongside `AuditQuery`/`AuditListResponseDto`. `payload` is
emitted as a JSON **string** (the raw stored TEXT), consistent with §7.2's rationale.

### 7.6 Composition

`AppState::new` builds `PgDeadLetters` and `DeadLetterService` (reusing the same `uow` / `audit` /
`ids` / `clock` / `authorize` handles the tenancy services already hold) and exposes it as
`state.dead_letters`. `main.rs` gains one spawn block for `PgOutboxMaintainer`, gated on
`config.outbox.retention.enabled`, mirroring the existing partition-maintenance block — including
its `warn!` on the disabled path and the `parked_days > 0` warning.

## 8. Error handling

- **Sweeper** — every step independent; errors logged, counted as `result="error"`, never
  propagated. A failed gauge refresh marks the tick errored (it signals pool/connection trouble)
  without failing the deletes that already succeeded, mirroring `PgPartitionMaintainer`.
- **Relay** — semantics unchanged; the two new writes are additive on the transaction it already
  holds.
- **API** — authorize first, always, so a denial never reveals whether an id exists. `404` for a
  non-parked or absent id. `422` for an unfiltered bulk replay, rejected before any DB access.
- **Migration** — pre-existing parked rows carry `parked_at = NULL` and are excluded from
  retention rather than being deleted on the first tick.

## 9. Testing

**Unit (no database)**

- `truncate_error`: char-boundary safety, under/over the limit, multi-byte input.
- `DeadLetterFilter::is_unfiltered` / `capped_limit`.
- HTTP query-param parsing (`to_filter`), mirroring `http::audit`'s existing test block.
- `DeadLetterService` against fakes: denies without the action; `NotFound` on a missing row;
  exactly one audit entry per mutation; the audit entry rolls back with a failed mutation;
  unfiltered bulk replay is rejected.
- `Action`: the three new variants in `ALL`, `as_wire` round-trip, correct `is_write`,
  exhaustiveness.
- Config: defaults, `0`-means-never for both windows, validation rejects zero
  `interval_secs`/`batch_size`.

**Integration (Postgres via testcontainers; `CI`-gated hard failure, local skip)**

- `outbox_retention_pg.rs` — an aged published row is deleted; a live row and a parked row are
  not; `parked_days = 0` never deletes parked; a non-zero `parked_days` deletes only aged parked
  rows; a parked row with `parked_at IS NULL` is never deleted; `batch_size` is honored across
  passes; the parked gauge is refreshed.
- `dead_letters_pg.rs` — list filtering and keyset paging; **replay followed by a relay tick
  actually publishes the row** (the end-to-end proof that replay works); discard removes the row;
  replay/discard against a live or absent id returns `None`; bulk replay honors its filter and
  cap.
- `http_dead_letters.rs` — end-to-end through the router with a platform-admin bearer, mirroring
  `http_audit.rs`: `403` for a non-admin, `404`, `422`, and a successful list/replay/discard.

**Rules** — promtool fixtures for both new alerts, each with a non-firing control series.

## 10. Documentation

- `docs/ops/RUNBOOK-observability.md` — §2.2 metric catalog rows; §4 alert table + two new
  entries; a rewrite of `IamOutboxEventsParked`'s remediation to point at the API instead of raw
  SQL (keeping the SQL as a break-glass fallback); §6 "Future" loses the delivered item.
- `rs/crates/services/paigasus-iam/iam.toml.example` — the `[outbox.retention]` block.
- `ops/observability/grafana/dashboards/iam.json` — panels for the parked-rows gauge and the
  deletion counters.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Retention deletes rows the relay still needs | Impossible by construction (§6.3) — the predicates are disjoint. Asserted directly in `outbox_retention_pg.rs`. |
| A bulk replay re-triggers the same failure loop | Bulk replay requires an explicit filter (`422` otherwise) and is capped at 10k. The runbook keeps its "confirm the root cause is fixed first" guidance. |
| Discard destroys evidence | The audit entry carries the entire event, payload included (§7.4). |
| A first tick over a huge backlog holds a long transaction | `MAX_BATCHES_PER_TICK` plus per-pass autocommit bounds it; the sweep resumes next tick. |
| New crate/dep gates red CI | No new dependencies. Still run the full `moon ci` graph (repo gates are not covered by per-project tasks) before pushing. |
