# SMA-469 — outbox retention + a real dead-letter path for parked events

**Status:** APPROVED (2026-08-03), after adversarial challenge and revision
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
   only hand-written `psql` snippets.

`audit_log` got exactly this treatment in SMA-467 — `LIST(outcome) → RANGE(occurred_at)`
partitioning plus an in-app `PgPartitionMaintainer` doing create-ahead and outcome-aware
retention, with maintenance metrics and an alert. The outbox never got the equivalent.

### 1.1 A failure-model finding that shaped the design

At the shipped defaults (`poll_interval_secs = 5`, `max_attempts = 5`), a broker outage lasting
**~25 seconds** exhausts every retry for every row in the backlog. Mass parking is therefore the
*expected* outage signature, not a hypothetical — a poison-message-only mental model would be
wrong. This drives three decisions: filtered bulk replay (§7.4), `parked_days` as the bulk
*retirement* path (§3.3), and a `last_error` that is actually descriptive under broker failure
(§5.1).

## 2. Goals / non-goals

**Goals**

- Bound `event_outbox` growth with age-based retention for published rows.
- Give parked rows a real dead-letter path: inspect, replay (single + filtered bulk), discard.
- Decide explicitly whether parked rows age out. **They do not, by default** (§3.3).
- Bring the outbox's operational story (metrics, alerts, runbook) up to `audit_log`'s level.

**Non-goals**

- **No gRPC mirror of the dead-letter surface.** Stated plainly: this is a scope decision, not an
  API-boundary principle. The audit surface has both transports because it is a product read API;
  this is an operator-only break-glass surface, and HTTP-only keeps `contracts/` untouched and
  avoids the codegen-drift, `:breaking`, and `:release-parity*` gates. A follow-up can add gRPC if
  a non-HTTP operator client ever needs it.
- **No bulk discard.** Replay is recoverable; deletion is not. `parked_days > 0` (§3.3) is the
  supported bulk-retirement path, and it is reversible right up until the sweep runs.
- No partitioning of `event_outbox` (§3.1).
- No idempotency-key mechanism on the POST endpoints (§7.7 documents the semantics instead).
- SMA-471's real broker `EventPublisher` stays a separate issue.

## 3. Design decisions

### 3.1 Batched delete, not partitioning

`event_outbox` is a *drained queue*, not a durable record of truth — `audit_log` is the trail. Its
steady-state size is `retention window × mutation rate`, which is small. Partitioning would add a
`FOR UPDATE SKIP LOCKED` scan across N leaves, per-leaf partial indexes, and create-ahead
machinery to solve a size problem that does not exist.

### 3.2 A dedicated maintenance task, not a fold into the relay tick

`PgOutboxMaintainer` is its own background task mirroring `PgPartitionMaintainer` — own hourly
interval, own metrics, own alert, `tick(now, policy) -> SweepReport` plus a `run` shutdown-watch
loop spawned from `main.rs`.

Folding retention into `OutboxRelay::tick` was rejected: it couples a 5-second hot loop to an
hourly bulk `DELETE`, makes tick latency lumpy, and — decisively — muddies
`iam_outbox_relay_ticks_total`, so a retention failure would red the relay's own liveness signal.

### 3.3 Parked rows never age out by default

`published_days` defaults to `7`; `parked_days` defaults to `0`. **`0` means "never" for both
windows** — one meaning for the sentinel across the block. A non-zero `parked_days` is opt-in and
emits a startup `warn!`, mirroring `audit.retention.committed_months > 0`.

`parked_days` is also the answer to §1.1's mass-parking scenario. An operator facing 50k genuinely
poison dead letters does not replay them (that burns `50k × max_attempts` publish attempts and
re-parks them all) and does not discard them one HTTP call at a time. They set `parked_days` to a
deliberate window, let the sweep retire them on a schedule, and set it back to `0`. This is why
bulk discard is not needed.

### 3.4 Parked rows stay in `event_outbox`

`parked = true` already *is* the dead-letter predicate. A dedicated `event_dead_letter` table was
rejected: it costs a migration, a move-on-park inside the relay's transaction, a move-back replay
path, and renders the `parked` column vestigial — all to express a set one boolean expresses.

### 3.5 The maintainer always spawns; `enabled` gates only deletion

`iam_outbox_parked_rows` (the dead-letter backlog gauge) is refreshed by the maintainer's tick. If
the maintainer's *spawn* were gated on `retention.enabled`, an operator setting
`enabled = false` — a plausible "stop deleting things" reaction during an incident — would
silently lose the backlog signal while the relay kept parking rows.

So `main.rs` always spawns the maintainer. `enabled = false` skips both delete steps; the gauge
refresh still runs. The task is renamed in the config docs accordingly: `enabled` gates
*deletion*, not *maintenance*.

## 4. Unit 1 — schema (m0009)

```sql
SET LOCAL lock_timeout = '5s';

ALTER TABLE event_outbox
  ADD COLUMN IF NOT EXISTS parked_at  TIMESTAMPTZ NULL,
  ADD COLUMN IF NOT EXISTS last_error TEXT NULL;

-- Every row already parked when this runs gets a well-defined park time: the migration moment.
UPDATE event_outbox SET parked_at = now() WHERE parked = true AND parked_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_event_outbox_published ON event_outbox (published_at)
  WHERE published_at IS NOT NULL;      -- retention's published-sweep predicate
CREATE INDEX IF NOT EXISTS ix_event_outbox_parked ON event_outbox (id)
  WHERE parked = true;                  -- DLQ list ordering + keyset paging
```

Both columns are load-bearing:

- **`parked_at`** — `parked_days` must measure from *when the row parked*, not `occurred_at`.
  Without it, a week-old event that parks today would be deleted on the very next tick. It is also
  the axis the DLQ's time filters use (§7.2).
- **`last_error`** — today the parking reason exists only in a `tracing::error!` line.

**Idempotence is required, not optional.** `m0008_partition_audit_log.rs:13-17` documents that
SeaORM's migrator does not serialize concurrent `up()` across replicas; `m0007` uses
`.if_not_exists()` for the same reason. A bare `ADD COLUMN` would fail the losing replica's boot
with `column "parked_at" ... already exists`. Hence `IF NOT EXISTS` on every DDL statement, and
`SET LOCAL lock_timeout = '5s'` (mirroring m0008) so the `ACCESS EXCLUSIVE` request backs off
rather than queueing ahead of in-flight `PgOutbox::enqueue` writes during a rolling deploy.

`CREATE INDEX CONCURRENTLY` is **not available** here — SeaORM runs each migration inside a
transaction, and `CONCURRENTLY` cannot run in one. The non-concurrent build takes `SHARE` on
`event_outbox`, blocking enqueues for its duration. On the two partial indexes over a table whose
realistic size at migration time is thousands to low millions of rows this is sub-second; the
`lock_timeout` bounds the worst case.

**The backfill is deliberate and replaces the earlier "leave it NULL" plan.** Leaving pre-existing
parked rows at `parked_at = NULL` created a permanently un-collectable set: invisible to any time
filter (NULL fails every comparison, so bulk replay could never reach them) and permanently
ineligible for retention *even if* `parked_days` were raised. Stamping `now()` says exactly what is
true — "we do not know when this parked; it was parked as of the migration" — starts its retention
clock at the migration rather than deleting it instantly, and makes it reachable by both filters
and retention. The retention predicate still carries `parked_at IS NOT NULL` as defense in depth.

`down` drops both indexes and both columns (`IF EXISTS`), mirroring m0007's `down`.

## 5. Unit 2 — the relay records park metadata

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

`last_error` is written on **every** failed attempt, not only at parking. Both writes ride the
transaction the tick already has; no extra round-trip. `TickReport` is unchanged.

`truncate_error` bounds the string at **1024 bytes** (not chars — 1024 four-byte chars is 4KB, past
Postgres's ~2KB TOAST threshold), cutting on a `char_indices` boundary and appending `…`.

### 5.1 The reason string must actually name the failure

`relay.rs:124` builds `reason` via `publisher.publish(&ev).await.map_err(|e| e.to_string())`.
`PublishError` has one variant, `#[error("backend error")] Backend(#[from] Box<dyn Error + Send +
Sync>)` (`ports.rs:373-375`) — its `Display` is a **static string that never renders the boxed
source**. As written, every real publish failure would store `last_error = "backend error"`, and
§1.1 says broker failure is the dominant parking mode. The column would ship looking informative
and be useless.

The fix is **one** change, not two: the relay builds `reason` by walking the full `source()` chain
— a `describe_error` helper joining each level's `Display` with `": "` — so
`PublishError::Backend(inner)` renders as `"backend error: <inner>: <inner's source>: …"` to
arbitrary depth.

`PublishError::Backend`'s attribute is deliberately **left as** `#[error("backend error")]`.
Changing it to `#[error("backend error: {0}")]` was considered and rejected: thiserror's `#[from]`
already makes the boxed error the variant's `source()`, so rendering it in `Display` *as well*
would make every chain walk emit each inner message twice. The chain walk subsumes the format
change, and confining the fix to the relay keeps the blast radius to this crate.

This also repairs the existing `tracing::error!`/`warn!` lines at `relay.rs:140,142`, which log the
same `reason` and are equally uninformative today.

A unit test asserts a two-level nested source chain survives into the produced reason, and that no
level is duplicated.

## 6. Unit 3 — retention sweeper

### 6.1 Config

```toml
[outbox.retention]
enabled              = true    # false = perform NO deletions (the gauge refresh still runs)
interval_secs        = 3600    # >= 1
published_days       = 7       # 0 = never delete published rows
parked_days          = 0       # 0 = never delete parked rows (default); > 0 warns at startup
batch_size           = 1000    # >= 1; rows per delete pass
max_batches_per_tick = 50      # >= 1; caps rows/tick at batch_size * this
```

`OutboxRetentionConfig` nests under `OutboxConfig`, mirroring `RetentionConfig` under
`AuditConfig`. Every field has a default, so an absent block is valid config. `IamConfig::validate`
rejects `interval_secs == 0`, `batch_size == 0`, and `max_batches_per_tick == 0`; both `*_days` may
be `0`.

`max_batches_per_tick` is **config, not a constant.** It is exactly as much an operational knob as
`batch_size`: at the defaults a tick retires at most 50k rows, so a deployment that has been
accumulating published rows "for the life of the deployment" (§1) needs ~8 days to drain 10M rows.
An operator draining that backlog must be able to raise it.

### 6.2 `PgOutboxMaintainer`

New adapter `adapters/persistence/pg_outbox_maintainer.rs`:

```rust
pub struct OutboxRetentionPolicy {           // Copy
    pub enabled: bool,
    pub published_days: u32,
    pub parked_days: u32,
    pub batch_size: u64,
    pub max_batches_per_tick: u32,
}

pub struct SweepReport {
    pub deleted_published: u64,
    pub deleted_parked: u64,
    pub passes_published: u32,
    pub passes_parked: u32,
    pub parked_rows: u64,
    pub errored: bool,
}
```

A tick does three **independent** steps — one failing must not wedge the others, exactly as
`PgPartitionMaintainer::tick` runs `prune` regardless of create-ahead's outcome:

1. If `enabled && published_days > 0`: batched delete of aged published rows.
2. If `enabled && parked_days > 0`: batched delete of aged parked rows.
3. Always: refresh the backlog gauge (`SELECT count(*) FROM event_outbox WHERE parked = true`).

Errors from any step are logged, set `errored = true`, and are never propagated. **A pass that
errors aborts only its own step's loop**; `deleted_*` then reports the partial count alongside
`errored = true`.

`passes_*` exist so §9 can assert batching actually happened — with totals alone a test cannot
distinguish one pass of 2000 from two passes of 1000.

`main.rs` runs an **awaited startup tick** before spawning the loop, mirroring the
`PgPartitionMaintainer` block at `main.rs:249` (non-fatal on error). Without it nothing happens for
the first `interval_secs`, which on a deployment being rescued from an unbounded table is the wrong
first impression.

### 6.3 The delete statements

```sql
-- published sweep
DELETE FROM event_outbox WHERE id IN (
  SELECT id FROM event_outbox
  WHERE published_at IS NOT NULL AND published_at < $1 AND parked = false
  ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
)

-- parked sweep
DELETE FROM event_outbox WHERE id IN (
  SELECT id FROM event_outbox
  WHERE parked = true AND parked_at IS NOT NULL AND parked_at < $1
  ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED
)
```

Looped until a pass affects fewer than `batch_size` rows or `max_batches_per_tick` is reached. Each
pass is its own autocommit statement — never one long transaction holding locks.

The published sweep carries a redundant-today `AND parked = false`. Today `parked ⇒ published_at IS
NULL` holds because `relay.rs:130-144` sets one or the other and never both, but nothing
*enforces* it — no CHECK constraint, and `replay_in` is now a second writer of these columns.
§3.3's headline promise is that parked rows never age out by default; a free predicate that makes
that true structurally rather than by convention is worth having.

**Scope of the disjointness claim.** The relay's poll predicate is `published_at IS NULL AND parked
= false`, and both delete predicates are subsets of its exact complement — so *retention and the
relay* can never contend. This claim covers relay-vs-retention **only**. It says nothing about
replay vs. retention or replay vs. replay; those are handled by `FOR UPDATE SKIP LOCKED` in §7.3.

`SKIP LOCKED` here also lets two maintainer replicas partition the work rather than block. Note the
consequence: a pass can return fewer than `batch_size` rows because a *peer replica* holds them, so
a replica may stop sweeping early for the interval with work remaining. This is benign — the next
tick resumes — but it means `passes_* < max_batches_per_tick` does not prove the backlog is drained.

### 6.4 Metrics

Added to `paigasus-observability::names` (and `names::ALL`), described in `describe_iam_metrics()`:

| Metric | Type | Meaning |
|---|---|---|
| `iam_outbox_retention_ticks_total{result}` | counter | Sweep ticks; `result=ok\|error`. The liveness signal. |
| `iam_outbox_rows_deleted_total{reason}` | counter | Rows deleted; `reason=published\|parked`. |
| `iam_outbox_parked_rows` | gauge | Current parked-row count — the dead-letter backlog. **Per-replica**; see §6.5. |
| `iam_outbox_dead_letters_replayed_total{scope}` | counter | Rows replayed; `scope=one\|bulk`. |
| `iam_outbox_dead_letters_discarded_total` | counter | Dead letters permanently discarded. |

Both dead-letter counters increment by **rows affected**, not by calls, for every `scope` — mixing
units within one family makes `rate()` meaningless. They increment **after** the transaction
commits, so a rolled-back replay is never counted (this differs from `pg_audit_log.rs:125-131`,
whose counter deliberately fires at insert; the difference is documented at both sites).

### 6.5 Alerts

- **`IamOutboxRetentionStalled`** (warning) —
  ```
  (sum by (job, instance) (increase(iam_outbox_retention_ticks_total[6h])) or (up{job="iam"} == 1) * 0) == 0
  for: 1h
  ```
  The `[6h]` window is scaled to this task's **hourly** default, not copied from
  `IamAuditPartitionMaintenanceStalled`'s `[2d]` — that alert's window matches
  `audit.retention.interval_secs = 86_400` (daily), and reusing it here would tolerate ~48
  consecutive missed ticks. The `or (up{job="iam"} == 1) * 0` fallback follows
  `IamPolicySnapshotReloadsStalled` (`iam.rules.yml:53`): without it, a replica that spawned the
  maintainer but never completed a single tick emits no series at all, and `empty == 0` is empty —
  the alert goes silent exactly when things are worst. The `description` states that the window
  assumes the default hourly interval and must be widened if `interval_secs` is raised.

  Because the maintainer now always spawns (§3.5), `enabled = false` does **not** silence this
  alert — the tick still runs for the gauge refresh. That is a deliberate improvement over the
  audit alert's documented `enabled=false ⇒ silent` caveat.

- **`IamOutboxDeadLetterBacklog`** (warning) — `max by (job) (iam_outbox_parked_rows) > 0` for `1h`.

  The `max by (job)` aggregation is required, not cosmetic: every replica runs a maintainer and
  each sets the same global count, so N replicas emit N identical series. A bare
  `iam_outbox_parked_rows > 0` pages N times for one condition, and a `sum()` dashboard panel would
  report N× the real backlog. §10's Grafana panel uses the same aggregation.

`IamOutboxEventsParked` (existing) is the *something just parked* edge signal;
`IamOutboxDeadLetterBacklog` is the *nobody has dealt with it* level signal. Both new alerts carry
a substantial `description`, matching the three most recent alerts in the file.

Rule fixtures go in `ops/observability/prometheus/rules/tests/iam.test.yml`. Each new case includes
a **control series that must not fire**, so a wrong comparison operator cannot pass an all-firing
fixture (the SMA-466 lesson). Note that fixture `rule_files` globs across files, so the two new
alert names must not collide with any existing name.

## 7. Unit 4 — the dead-letter surface

### 7.1 Cedar actions

| Action | `is_write` |
|---|---|
| `ListOutboxDeadLetters` | `false` |
| `ReplayOutboxDeadLetter` | `true` |
| `DiscardOutboxDeadLetter` | `true` |

Added to the `Action` enum, `Action::ALL`, `as_wire`, the `is_write` arms, the
`all_covers_every_variant` exhaustiveness match, and `SCHEMA_SRC`. `Action::ALL`'s doc says "in
schema-declaration order", so the three go in the **same position in both** — appended after
`ListAuditLog` and before `InvokeModel` in `ALL` *and* in `SCHEMA_SRC`. The length assertion at
`action.rs:275` (`assert_eq!(Action::ALL.len(), 36, "27 pre-existing + 7 M4 + 1 audit + 1
invoke-model")`) becomes `39` with its message extended.

Root-only-ness is enforced by the *service* always authorizing at `root_prn()`; the schema's shared
`appliesTo` block does not restrict it. Mirrors `AuditQueryService::list`.

**Correcting the earlier claim that `roles.rs` is untouched.** It is untouched as a *file*, but
`forbid_archived_writes_source()` (`roles.rs:262-266`) generates its action list from
`Action::ALL.iter().filter(|a| a.is_write() && !a.is_restore())`. Two new write actions therefore
change that generated policy source, and `reconcile_starter` (`bootstrap.rs:79-84`) compares and
**warns without overwriting** — so every database seeded before this change logs
`"starter policy drift: the stored source differs from the code-defined source"` at **every boot,
forever**.

This is a pre-existing property of compare-and-warn reconciliation that any future action addition
also triggers; it is not created by this issue, but this issue is what fires it. Handling:

- The spec does **not** silently reclassify replay/discard as reads to dodge it. They mutate; the
  catalog must say so.
- Including them in the archived-writes forbid is harmless-but-noisy: both are Root-scoped, and
  `Root` has no `effective_status` attribute (`schema.rs:13`), so the forbid clause can never match
  them.
- §10's runbook gains the operator remediation (update the system-owned `policy` row's `source` to
  match, or delete it and let `reconcile_starter` re-`put` it on next boot).
- **[SMA-477](https://linear.app/smaschek/issue/SMA-477/iam-starter-policies-reconcile-by-compare-and-warn-so-any-action)**
  tracks giving system-owned starter policies real reconciliation rather than compare-and-warn.
  Filed 2026-08-03; it does **not** block this issue, and this issue does not wait on it — the
  runbook remediation covers the interim.

### 7.2 Core types and ports

```rust
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,          // raw wire string — see below
    pub schema_version: i32,         // raw column value — see below
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,             // raw serialized TEXT — see below
    pub correlation_id: Option<Uuid>,
    pub attempts: u32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Listing + keyset paging. `parked_from`/`parked_to` filter `parked_at`.
pub struct DeadLetterFilter {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,                  // capped_limit() clamps to MAX_LIMIT = 200
}

/// Bulk replay. A SEPARATE type — not `DeadLetterFilter` with a bigger limit.
pub struct BulkReplayRequest {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub max_rows: u64,               // REQUIRED; clamped to MAX_BULK_REPLAY = 10_000
}
```

**Time filters name `parked_at`, explicitly.** The operationally meaningful question is "what
parked during last night's outage", which `occurred_at` cannot answer. The fields are named
`parked_from`/`parked_to` rather than `from`/`to` so the column is unambiguous at every call site.
The §4 backfill guarantees every parked row has a non-NULL `parked_at`, so no parked row is ever
invisible to a time filter.

**`event_type` and `payload` are raw `String`s, and `schema_version` is a raw `i32`** — not
`EventType`, `serde_json::Value`, and `u16`. All three of `row_to_domain_event`'s rejection reasons
(`relay.rs:63-65`) are an unrecognized `event_type` wire string, an out-of-range `schema_version`,
and invalid `payload` JSON — i.e. all three are reasons a row parks. Typing any of them strictly
would make the dead-letter surface unable to display exactly the rows it exists to explain. This is
a diagnostic projection of a persisted row, not a domain type, and the field docs say so.
`attempts` is `u32` — it is a plain count, never negative, and not a park reason.

**Two caps, cleanly separated.** The earlier design reused `DeadLetterFilter` for bulk replay,
which put `capped_limit()`'s `MAX_LIMIT = 200` (`audit.rs:55-58`) in direct contradiction with
`MAX_BULK_REPLAY = 10_000` and left `cursor` meaningless on the bulk path. `BulkReplayRequest` has
no `cursor`, and its `max_rows` is required.

```rust
#[async_trait]
pub trait DeadLetters: Send + Sync {
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError>;
    async fn replay_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
    async fn replay_matching_in(&self, tx: &dyn Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError>;
    async fn discard_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
}
```

The mutating methods take the caller's transaction (recovered via `uow::recover_txn`, as
`PgOutbox::enqueue` does) so the mutation and its audit entry commit atomically on one
`UnitOfWork` — the reference pattern in `application/roles.rs`. `None` means "no parked row with
that id".

### 7.3 Adapter — `PgDeadLetters`

```sql
-- replay_in: note last_error is NOT cleared
UPDATE event_outbox SET parked = false, attempts = 0, parked_at = NULL
WHERE id = $1 AND parked = true RETURNING *;

-- discard_in
DELETE FROM event_outbox WHERE id = $1 AND parked = true RETURNING *;

-- replay_matching_in (filter predicate abridged)
UPDATE event_outbox SET parked = false, attempts = 0, parked_at = NULL
WHERE id IN (
  SELECT id FROM event_outbox WHERE parked = true AND <filter>
  ORDER BY id LIMIT $n FOR UPDATE SKIP LOCKED
);
```

`RETURNING *` goes through SeaORM's `Statement` + `query_one` (`execute` discards the returned
row), matching how `PgPartitionMaintainer` issues raw statements. The recovered
`DatabaseTransaction` supports this via `ConnectionTrait::query_one`.

Every mutating statement carries `AND parked = true`, so a live or already-published row is
untouchable through this surface.

**`FOR UPDATE SKIP LOCKED` on the bulk subquery is required.** Postgres does not guarantee an
`UPDATE ... WHERE id IN (SELECT ... ORDER BY ...)` takes row locks in the subquery's order, so two
concurrent bulk replays with overlapping filters can deadlock; and a non-deadlocking overlap blocks
the second operator for the whole of the first operator's transaction — which includes
`audit.record` and `tx.commit()`, an application-side hold. `SKIP LOCKED` makes concurrent replays
partition instead of collide. An operator responding to an outage is precisely the person most
likely to fire two of these.

**`last_error` is deliberately preserved across replay.** Clearing it would destroy the evidence
chain when a replayed row re-parks for a different reason — the operator would see only the second
failure. `parked_at` and `attempts` reset because they describe the row's *current* state; the
error string is history.

`list` orders by `id DESC` with keyset paging (`id < cursor`), mirroring `PgAuditLog::query`. IDs
are UUIDv7 (`KernelIdGenerator::mint`), so id order is time order.

**Ordering split, precisely.** `list` sorts `id DESC` (newest first — what an inspecting operator
wants); `replay_matching_in` selects `ORDER BY id` **ascending**, so that when a filter matches more
rows than `max_rows`, repeated calls walk the backlog forward instead of re-selecting the same
newest slice. It does **not** "preserve event order" in any global sense — per-aggregate ordering
was already broken the moment one row parked while later rows published, and replay cannot restore
it. One operational consequence to document: replayed rows carry lower ids than fresh traffic, and
the relay drains `ORDER BY id` ascending at `batch_size = 100` every 5s, so a 10k-row replay delays
live event delivery by roughly 8 minutes.

**Index coverage is deliberately partial.** `ix_event_outbox_parked ON (id) WHERE parked = true`
serves ordering and keyset paging; an `event_type` or `parked_at` filter is applied on the heap
fetch. For a parked set in the tens of thousands this is acceptable, and adding speculative
composite indexes to a table whose healthy state is *zero* parked rows is not worth the write cost.

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
  `dead.replay_in(&*tx, id)?`; `None` ⇒ drop the tx, return `NotFound`; else
  `audit.record(&*tx, &entry)`; `tx.commit()`; then increment the counter.
- `replay_matching(actor, req)` — authorize; validate `max_rows` (see §7.5) **before any DB
  access**; same transaction shape; the audit entry carries the request and the replayed count.
- `discard(actor, id)` — authorize `DiscardOutboxDeadLetter`; same shape as `replay`.

Replay and discard record an audit entry but **do not** enqueue a domain event: these are
operational actions on the queue itself, and an outbox event about outbox operations would be
circular.

**Discard's audit detail is complete on purpose** — `event_type`, `aggregate_prn`, `actor_prn`,
`correlation_id`, `occurred_at`, `attempts`, `last_error`, **and the payload**. A discarded dead
letter is gone forever, so the audit entry is its only remaining trace. Replay's detail carries
everything except the payload (the row still exists).

*Privacy note:* this makes "copy the full event payload into `audit_log.detail`" a durable
contract, and `audit.retention.committed_months` defaults to `0` (never dropped). Every current
event payload carries only ids, keys, and names — no PII — but this decision must be revisited if
that ever changes.

### 7.5 HTTP adapter

`adapters/http/dead_letters.rs`, merged onto the bearer-gated `protected` sub-router. A thin
extract → service → DTO adapter, mirroring `adapters/http/audit.rs`.

| Method | Path | Action | Notes |
|---|---|---|---|
| `GET` | `/v1/outbox/dead-letters` | `ListOutboxDeadLetters` | `event_type`/`parked_from`/`parked_to`/`cursor`/`limit`; `next_cursor` set only when the page came back full |
| `POST` | `/v1/outbox/dead-letters/replay` | `ReplayOutboxDeadLetter` | bulk; JSON body; responds `{"replayed": n}` |
| `POST` | `/v1/outbox/dead-letters/{id}/replay` | `ReplayOutboxDeadLetter` | `404` when not a parked row |
| `POST` | `/v1/outbox/dead-letters/{id}/discard` | `DiscardOutboxDeadLetter` | `404` when not a parked row |

The literal `/replay` and `/{id}/replay` routes differ in segment count, so axum's router has no
ambiguity.

**The bulk guard is a required `max_rows`, not an inferred-intent check, and it returns `400`.**
Two corrections to the earlier design:

- *`422` has no home in this codebase.* `ErrorClass` has exactly six variants and
  `Validation → 400 BAD_REQUEST` (`http/error.rs:20-26`); `status_to_grpc` matches all six
  exhaustively (`grpc/convert.rs:32-42`); there is no `422` anywhere in `rs/`. Introducing one
  would mean a new `ErrorClass` variant, a broken gRPC exhaustive match, and a new stable code —
  for no benefit. A missing/zero `max_rows` is `TenancyError::InvalidBulkReplay` (new variant,
  `ErrorClass::Validation`, stable kebab-case `code()` = `invalid-bulk-replay`), rendered `400`.
- *An "at least one filter field" check was a speed bump, not a guard.* `parked_from =
  1970-01-01T00:00:00Z` would satisfy it while matching everything — and that is the most natural
  way an operator writes "replay everything". A required explicit `max_rows`, clamped to
  `MAX_BULK_REPLAY`, is a positive affirmation of blast radius that cannot be satisfied by
  accident. Filters remain optional.

DTOs live in `adapters/http/dto.rs`. `payload` is emitted as a JSON **string** (the raw stored
TEXT), per §7.2.

### 7.6 Composition

`AppState::new` builds `PgDeadLetters` and `DeadLetterService` with its own fresh
`SeaOrmUnitOfWork` (`Arc::new(SeaOrmUnitOfWork::new(db.clone()))`), matching how every other
service in `AppState::new` constructs its own — `role_uow`, `policy_uow`, `api_key_uow`,
`service_account_uow` are each separate instances over the same `Arc`-backed pool. It is exposed as
`state.dead_letters`. `main.rs` gains one spawn block for `PgOutboxMaintainer` — always spawned
(§3.5), with an awaited startup tick and the `parked_days > 0` warning.

### 7.7 Concurrency and retry semantics (documented, not mechanised)

- **`404` covers three distinct states**: no such id, a row that was never parked, and a row
  another actor just replayed or discarded. It also covers a row the relay is mid-tick on and about
  to park — the `AND parked = true` predicate blocks on the relay's `FOR UPDATE` lock, then fails
  to match. The runbook says this so an operator does not chase a phantom.
- **Replay is not idempotent.** A client that times out on `POST /{id}/replay` and retries gets
  `404`, which is indistinguishable from "wrong id" — and that `404` is the expected
  success-after-timeout signal. A retried *bulk* replay replays a different row set. Idempotency
  keys are not worth the machinery for a Root-only break-glass surface; the semantics are
  documented in the runbook instead.
- **Replay exercises the at-least-once contract.** The relay is already at-least-once (a publish
  that succeeds followed by a failed commit re-publishes), so consumers must already be idempotent.
  Replay makes operators exercise that contract deliberately, so the runbook states it.
- **Availability before SMA-471.** With `TracingEventPublisher` as the only production publisher,
  broker-driven parking cannot happen yet — but malformed-row parking (`row_to_domain_event`'s
  three rejection paths) can, and the tests inject a failing publisher. Shipping the surface
  unflagged is what makes SMA-471's rollout safe, so it is not gated behind a feature flag.

## 8. Error handling

- **Sweeper** — every step independent; errors logged, counted as `result="error"`, never
  propagated. A failed gauge refresh marks the tick errored (it signals pool trouble) without
  failing the deletes that already succeeded, mirroring `PgPartitionMaintainer`.
- **Relay** — semantics unchanged; the two new writes are additive on the transaction it holds.
- **API** — authorize first, always, so a denial never reveals whether an id exists. `404` for a
  non-parked or absent id. `400` (`invalid-bulk-replay`) for a missing/zero `max_rows`, rejected
  before any DB access.
- **Migration** — idempotent DDL under a bounded `lock_timeout`; the backfill gives every
  pre-existing parked row a defined `parked_at`.

## 9. Testing

**Unit (no database)**

- `truncate_error`: byte bound, char-boundary safety, multi-byte input.
- `describe_error`: a two-level nested source chain reaches the reason string (§5.1).
- `BulkReplayRequest` validation: missing/zero `max_rows` rejected; clamping to `MAX_BULK_REPLAY`.
- HTTP query-param parsing, mirroring `http::audit`'s existing test block.
- `DeadLetterService` against fakes: denies without the action; `NotFound` on a missing row;
  exactly one audit entry per mutation; the audit entry rolls back with a failed mutation; the
  counter does **not** increment on a rolled-back replay.
- `Action`: the three new variants in `ALL` in schema order, `as_wire` round-trip, correct
  `is_write`, exhaustiveness, and the updated `ALL.len() == 39` assertion.
- Config: defaults, `0`-means-never for both windows, validation rejects zero `interval_secs` /
  `batch_size` / `max_batches_per_tick`.

**Integration (Postgres via testcontainers; `CI`-gated hard failure, local skip)**

- `outbox_retention_pg.rs` — an aged published row is deleted; a live row and a parked row are not;
  `parked_days = 0` never deletes parked; a non-zero `parked_days` deletes only aged parked rows;
  `enabled = false` deletes nothing but still refreshes the gauge; `batch_size` /
  `max_batches_per_tick` are honored (asserted via `passes_*`, which is why they exist); the parked
  gauge is refreshed.
- **`outbox_retention_concurrency_pg.rs`** — the disjointness claim of §6.3 asserted *concurrently*,
  not just against statically seeded rows: hold a relay-selected row's `FOR UPDATE` lock open
  across a maintainer tick using `relay_pg.rs:157`'s existing hold-open pattern, and assert the
  sweep neither blocks nor deletes it.
- `dead_letters_pg.rs` — list filtering (including `parked_at` range) and keyset paging; **replay
  followed by a relay tick actually publishes the row**; discard removes it; replay/discard against
  a live or absent id returns `None`; a pre-migration-shaped row (parked, `parked_at` backfilled)
  is reachable by a time filter; bulk replay honors its filter, its `max_rows` cap, and its
  ascending selection order.
- `http_dead_letters.rs` — end-to-end through the router with a platform-admin bearer, mirroring
  `http_audit.rs`: `403` for a non-admin, `404`, `400` for a missing `max_rows`, and a successful
  list/replay/discard.

**Rules** — promtool fixtures for both new alerts, each with a non-firing control series, plus a
case proving `IamOutboxRetentionStalled`'s `up`-fallback fires for a live target that has never
emitted a tick.

## 10. Documentation

- `docs/ops/RUNBOOK-observability.md` — §2.2 metric catalog rows (noting `iam_outbox_parked_rows`
  is per-replica and must be aggregated `max by (job)`); §4 alert table + two new entries; a rewrite
  of `IamOutboxEventsParked`'s remediation to point at the API (keeping the SQL as break-glass);
  §6 "Future" loses the delivered item. New guidance covering: the `404`-conflation and
  non-idempotency semantics of §7.7; the at-least-once/consumer-idempotency contract that replay
  exercises; the starter-policy drift remediation of §7.1; that `DELETE` alone does not shrink the
  table's disk footprint (autovacuum reclaims to the free space map, and a large first drain may
  warrant a manual `VACUUM`); and the consumer-divergence consequence of discard (§11).
- `rs/crates/services/paigasus-iam/iam.toml.example` — the `[outbox.retention]` block.
- `ops/observability/grafana/dashboards/iam.json` — panels for the parked-rows gauge (aggregated
  `max by (job)`) and the deletion counters.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Retention deletes rows the relay still needs | Disjoint predicates (§6.3) plus a redundant `AND parked = false`; asserted concurrently in `outbox_retention_concurrency_pg.rs`, not only against seeded rows. |
| Concurrent bulk replays deadlock or block each other | `FOR UPDATE SKIP LOCKED` on the bulk subquery (§7.3) makes them partition rather than collide. |
| A bulk replay re-triggers the same failure loop | A required, capped `max_rows` (§7.5) is an explicit affirmation of blast radius; the runbook keeps its "confirm the root cause is fixed first" guidance. |
| **Discard destroys delivery, not just evidence** | A discarded event represents a state change that *did* commit in IAM and will now never reach any consumer — with SMA-471's real publisher that is permanent, silent divergence with no reconciliation path. The complete audit entry (§7.4) is the documented reconciliation input, and the runbook requires the operator to record a plan before discarding. |
| A first tick over a huge backlog stalls | `max_batches_per_tick` (config) plus per-pass autocommit bounds it; the sweep resumes next tick; an awaited startup tick means it begins immediately. |
| Migration fails a concurrent multi-replica boot | Idempotent `IF NOT EXISTS` DDL plus `SET LOCAL lock_timeout` (§4), following m0007/m0008 precedent. |
| Permanent starter-policy drift warning on existing databases | Documented, with operator remediation in the runbook; the underlying reconciliation gap is tracked as SMA-477 (§7.1), which does not block this issue. |
| New crate/dep gates red CI | No new dependencies. Still run the full `moon ci` graph before pushing — repo gates are not covered by per-project tasks. |

## 12. Rejected challenge findings

Recorded so the reasoning is visible rather than silently dropped:

- **A `published_rows_eligible` progress gauge.** Would add a second `COUNT(*)` per tick against the
  largest table for marginal value; `passes_*` reaching `max_batches_per_tick` plus the
  `iam_outbox_rows_deleted_total` rate already tells an operator the sweep is saturated and how
  fast it is draining.
- **A two-step `dry_run` + confirm token, or client idempotency keys, for bulk replay.** Real
  machinery for a Root-only break-glass surface used by operators who already have `psql`. The
  required `max_rows` provides the affirmation; §7.7 documents the retry semantics instead.
- **Gating Unit 4 behind a feature flag until SMA-471 lands.** Parking already occurs today via
  malformed rows, and having the surface in place *before* a real publisher exists is what makes
  that rollout safe.
- **Filtered bulk discard.** §3.3's `parked_days` is the supported bulk-retirement path, and unlike
  a bulk `DELETE` call it is reversible right up until the sweep runs.
- **Adding composite indexes for filtered DLQ queries.** The healthy state of the parked set is
  zero rows; speculative indexes cost writes on the hot enqueue path forever to speed up a query
  that only matters during an incident (§7.3).
