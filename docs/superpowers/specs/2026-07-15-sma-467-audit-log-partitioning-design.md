# SMA-467 — IAM `audit_log` time-partitioning + outcome-aware retention

**Status:** Design (brainstormed + adversarially challenged, rev 2) · **Date:** 2026-07-15 ·
**Linear:** SMA-467 (closes) · **Service:** `paigasus-iam` · **Related:** SMA-446 (M5 audit/outbox —
this implements its §4/D14 deferred retention design)

> **Rev 2** folds in the Stage-2 spec-challenge. The architecture (two-level
> `LIST(outcome)→RANGE(occurred_at)` partitioning + an in-app maintenance task) is unchanged; the
> hardening is targeted — UTC-pinned partition bounds, a top-level `LIST` default, cross-replica
> serialization of the destructive swap, per-op transactions with `lock_timeout` for retention,
> a bounded default query window, and corrected claims/test scope. See §12 changelog.

---

## 1. Context

The M5 audit/outbox design (`docs/superpowers/specs/2026-07-12-sma-446-m5-audit-log-outbox-design.md`,
§4 / **D14**) specified `audit_log` as **monthly range-partitioned on `occurred_at`** with an
**outcome-aware retention policy** — denial rows retained for a shorter window than mutation rows,
enforced by a scheduled `DROP`/detach of aged-out denial partitions. The shipped migration
(`m0006_create_audit_log`, PRs #80/#81) created `audit_log` as a **plain, non-partitioned table**
(a deliberate "Slice-A simplification" — see the m0006 module doc), so the retention design was
never implemented. The observability RUNBOOK (`docs/ops/RUNBOOK-observability.md`, §4 "Audit
retention & partitioning") documents an **interim batched-`DELETE`** procedure as a stopgap and
lists the partitioning work as a §6 follow-up.

This spec implements that follow-up: convert `audit_log` to the D14 partitioned design, add the
outcome-aware retention policy + a scheduled partition-drop, and replace the RUNBOOK's interim
procedure with the real operation.

**Not a live incident.** Denial-row *write rate* is bounded by the D8 drop-oldest buffer, so this
is about long-term index/table bloat (denials accumulate over time and dominate the audit-row
volume), not an active outage.

### Substrate that already exists (relevant)

- **`audit_log`** (`m0006`): plain table, PK `id` (UUIDv7). Columns `id`, `occurred_at
  (timestamptz)`, `actor_prn`, `action`, `resource_prn`, `outcome (text: committed|denied)`,
  `determining_policies (text, JSON-encoded Vec<String>)`, `detail (text, JSON, NOT NULL DEFAULT
  '{}')`, `correlation_id`. Five indexes: `occurred_at`, `actor_prn`, `resource_prn`, `action`,
  `outcome`. `outcome` is **unconstrained TEXT** (no CHECK — `pg_audit_log.rs:96` treats a stray
  value as corruption to surface, i.e. it is *possible*, not impossible). `determining_policies`/
  `detail`/`outcome` are serialized **TEXT** (Slice-A convention; no native `jsonb`/`text[]`).
- **`PgAuditLog`** (`adapters/persistence/pg_audit_log.rs`): `record` (in-txn, for committed
  mutation rows — the audit insert is atomic with the mutation, **G1**), `record_out_of_band`
  (autocommit, for denial rows drained from the D8 buffer), and `query` (keyset pagination:
  `ORDER BY id DESC`, `WHERE id < cursor`, equality filters per present `AuditFilter` field,
  `LIMIT capped_limit()`; `from`/`to` applied only when present — see §3.6).
- **SeaORM entity** `entities/audit_log.rs`: `id` is the sole `#[sea_orm(primary_key,
  auto_increment = false)]`.
- **Migration harness**: `adapters/persistence/migration/` with `Migrator` in `mod.rs`;
  `main.rs:32` calls `Migrator::up(&db, None)` on **every process start**, but SeaORM's
  `seaql_migrations` bookkeeping means each migration's `up` **runs exactly once** (the first boot
  that finds it pending) — *not* on every boot. Concurrent first-boot across replicas is the race
  §4 guards. Ample `execute_unprepared` raw-SQL precedent for DDL sea-query can't express (partial
  indexes in `m0007`, CHECK constraints in `m0004`, functions/triggers in `m0002`).
- **Background-task pattern**: the outbox relay (`spawn`ed in `main.rs`, mirrors the
  policy-snapshot `spawn_reload` loop — a `tokio::select!` on an interval vs. a shutdown-watch,
  config-gated by `[outbox].relay_enabled`, with a startup `warn` when disabled). This maintenance
  task mirrors it.
- **Observability**: metric names are `const`s in `paigasus_observability::names` (+ a `names::ALL`
  slice); a drift test (`paigasus-observability/tests/drift.rs`) asserts every `iam_`/`gateway_`
  identifier referenced in committed dashboard JSON / rule YAML is in `names::ALL`. RUNBOOK **prose**
  tables are explicitly *not* drift-tested.
- **Tests**: integration tests run against an ephemeral Postgres via `tests/support`
  (testcontainers, `postgres:16-alpine`, **defaults to UTC session TZ** — see the §3.5 hazard);
  Docker-less laptops skip, CI treats a missing daemon as a hard failure. Several suites read audit
  rows via `audit_log::Entity::find_by_id(...)` (§3.2).

### Production Postgres floor

Developed and tested against **PG 16** (testcontainer). The partitioning features used
(multi-level partitioning, `DEFAULT` partitions, indexes on a partitioned parent cascading to
leaves) exist from **PG 11+**; the **documented production floor is PG ≥ 14** (aligns with common
managed-PG baselines and leaves room for `DETACH … CONCURRENTLY` as a future retention hardening,
§10). CI/tests validate against 16.

---

## 2. Goals / Non-goals

### Goals (acceptance criteria)

- **G1.** `audit_log` is a **two-level partitioned table**: `PARTITION BY LIST (outcome)` →
  each outcome subtree `PARTITION BY RANGE (occurred_at)` monthly, plus a top-level `LIST` default
  (§3.3). A new migration (`m0008_partition_audit_log`) performs a **data-preserving,
  cross-replica-serialized** conversion and has a working `down` that restores the exact `m0006`
  plain-table shape. **No committed-audit insert (which is in the mutation's txn) may fail for lack
  of a partition** — enforced by the LIST + RANGE defaults.
- **G2.** An **in-app maintenance task** (a) creates upcoming monthly leaf partitions ahead of time,
  and (b) enforces **outcome-aware retention** by dropping aged-out denied (and optionally
  committed) monthly leaves, on a schedule, multi-replica safe, and **without stalling live audit
  inserts** (per-op `lock_timeout` back-off, §5.1).
- **G3.** Retention windows and the schedule are validated **config** (`[audit.retention]` in
  `IamConfig`): denied dropped at 3 months, committed never auto-dropped (opt-in).
- **G4.** The change is **transparent to the write/adapter API and callers** for writes and
  filtered reads. **Reads are not free of cost:** an unfiltered `query` becomes a `MergeAppend`
  across all leaves, so this spec also lands the M5 §9 **bounded default/max time window** so
  unbounded scans prune to a bounded leaf set (§3.6).
- **G5.** The **RUNBOOK** replaces its interim batched-`DELETE` retention procedure with the real
  partition-tree + maintenance-task + partition-drop documentation, and the now-done follow-up is
  retired from §6.

### Non-goals (out; tracked elsewhere or deliberately deferred)

- Auto-remediating a **non-empty `DEFAULT` partition** (moving leaked rows into a freshly created
  month leaf). v1 surfaces it as a metric + a manual RUNBOOK reattach step; it only occurs if the
  maintenance task is disabled/down for ≈ a full `ahead_months` window (D8).
- A `pg_cron`/`pg_partman` extension-based scheduler (rejected — adds an infra dependency the test
  Postgres image and CI don't have; see D2).
- `DETACH … CONCURRENTLY`-based retention (a PG14+ hardening noted as a follow-up, §10; v1 uses
  `DROP TABLE IF EXISTS <leaf>` under a `lock_timeout`, which is adequate for a non-live-incident
  background op).
- Changing the audit **column types** (`detail`/`determining_policies`/`outcome` stay TEXT).
- Auditing new events or touching the outbox/relay.
- A Grafana **dashboard** panel for the new metrics (metric consts + a RUNBOOK prose entry + one
  alert rule ship; a dashboard panel is optional polish, §10).

---

## 3. Architecture

### 3.1 Partition topology (D1)

```
audit_log                         PARTITION BY LIST (outcome)
├─ audit_log_committed            PARTITION BY RANGE (occurred_at)
│   ├─ audit_log_committed_2026_07   FROM (TIMESTAMPTZ '2026-07-01 00:00:00+00')
│   │                                  TO (TIMESTAMPTZ '2026-08-01 00:00:00+00')
│   ├─ audit_log_committed_2026_08   …
│   └─ audit_log_committed_default    DEFAULT            ← RANGE write-safety backstop
├─ audit_log_denied               PARTITION BY RANGE (occurred_at)
│   ├─ audit_log_denied_2026_07      FROM (…'2026-07-01…+00') TO (…'2026-08-01…+00')
│   ├─ audit_log_denied_2026_08      …
│   └─ audit_log_denied_default       DEFAULT            ← RANGE write-safety backstop
└─ audit_log_other                DEFAULT (plain leaf)   ← LIST catch-all for stray outcomes (§3.3)
```

Retention drops whole aged-out **denied** month leaves (`audit_log_denied_YYYY_MM`) on the short
window; committed leaves are kept indefinitely by default. The denied subtree *is* the droppable
unit — the faithful realisation of D14's "drop old denial partitions."

### 3.2 Composite primary key (D3)

Postgres requires a partitioned table's PK/unique constraints to include **every partitioning
level's key column**. With `LIST (outcome)` at the top and `RANGE (occurred_at)` beneath, the DB
PK is **`(id, occurred_at, outcome)`** — this is a definite Postgres rule, and the migration test
asserts it against real PG.

Consequences:
- `id` is no longer DB-unique on its own (only the triple is). In practice `id` is a per-entry
  UUIDv7 — collisions astronomically improbable — a standard, acceptable partitioning trade-off.
- **The SeaORM entity keeps `id` as its sole `primary_key`.** The adapter only *inserts* (routing)
  and *filters* (`query`), so the single-column entity PK is safe there. **Correction to rev 1:**
  the claim that reads "never use `find_by_id`" was false — several integration suites read audit
  rows via `audit_log::Entity::find_by_id(entry.id)`: `api_keys_pg.rs:100,191`,
  `outbox_uow_pg.rs:85,134,135,138`, `authz_role_grants.rs:440,523`, `authz_policy_store.rs:561`.
  On a sole-`id` entity `find_by_id` emits `WHERE id = $1`, which resolves correctly against the
  partitioned parent (a full-tree scan — `id` isn't a partition key, so no pruning, but the row is
  returned). §7 lists these suites in the regression set and adds an explicit assertion that
  `find_by_id` still resolves. A doc comment on the entity records the DB/entity PK divergence.

### 3.3 `DEFAULT` partitions — LIST and RANGE (D4, D10)

Two default layers, both to protect the **G1 in-transaction committed insert** (a committed insert
that finds no partition fails and **rolls back the whole mutation**):

- **RANGE default per outcome subtree** (`audit_log_committed_default`, `audit_log_denied_default`)
  — catches an `occurred_at` with no matching month leaf if create-ahead ever lags. Also de-flakes
  the `query_filters_by_occurred_at_from_and_to` test (writes `now ± 2h`) across month boundaries.
- **LIST default `audit_log_other`** (D10, new in rev 2) — `outcome` is unconstrained TEXT; the top
  `LIST` has partitions only for `'committed'`/`'denied'`. Without a LIST default, a stray outcome
  (a bug, tampering, or a future third value) has **no home** and hard-fails the insert — for the
  in-txn committed path, a G1 regression the current plain table does not have. `audit_log_other`
  is a plain (non-sub-partitioned) catch-all that must stay empty and is monitored (§6.1). The
  domain only ever writes the two-variant `AuditOutcome`, so it is defense-in-depth, not an
  expected path.

**Default-pollution trade-off:** `CREATE TABLE … PARTITION OF … FOR VALUES FROM … TO …` while a
matching default holds rows makes PG scan the default and error. Kept a non-issue by staying
`ahead_months ≥ 1` ahead so defaults stay empty in steady state; a non-empty default is surfaced by
`iam_audit_default_partition_rows` and a manual RUNBOOK reattach step (auto-remediation is a
non-goal, D8).

### 3.4 Indexes

The four useful indexes (`occurred_at`, `actor_prn`, `resource_prn`, `action`) are created **on the
partitioned parents**, so Postgres cascades them to every present and future leaf. The **`outcome`
index is dropped** — `outcome` is now the top LIST partition key, so within any leaf it is a
constant and the index is dead weight (outcome filters are served by partition pruning; a query
filtering *only* by outcome prunes to the whole subtree and gains nothing from a per-leaf index on a
constant column). `down` restores the original five indexes (incl. `outcome`) on the plain table.

### 3.5 Timezone posture (D9) — UTC-pinned bounds

**All partition-bound literals are fully-qualified UTC `timestamptz`** (`TIMESTAMPTZ '2026-08-01
00:00:00+00'`), never bare date strings. A bare `'2026-08-01'` compared against a `timestamptz`
column is cast using the session's `TimeZone` GUC (which SeaORM/sqlx do **not** force to UTC), so in
any non-UTC session the leaf boundaries — and therefore routing *and* the monthly retention window —
shift by the offset. The testcontainer defaults to UTC, so this would be **green in tests and broken
in production**. Every migration and maintenance transaction additionally issues `SET LOCAL
TimeZone = 'UTC'` as belt-and-suspenders, and §7 adds a test that runs the migration + maintainer
under a **non-UTC** session TZ.

### 3.6 Bounded query window (D12) — the read-cost mitigation

Partitioning turns the adapter's unfiltered `ORDER BY id DESC LIMIT n` into a `MergeAppend` over
every leaf's PK index (leaf count grows monthly), so the M5 §9 "default + max time window" — never
implemented in Slice A — lands here:

- When **both** `from` and `to` are absent, `query` applies a **default lookback** (`from = now −
  default_window`) so the scan prunes to a bounded set of recent leaves.
- A **max window** caps any explicit `from`/`to` span (an over-wide range is clamped, not a
  full-tree scan).
- Both bounds are config (`[audit].query_default_window_days`, `query_max_window_days`) with
  sensible defaults. Existing tests are unaffected (their rows are all recent / use explicit narrow
  ranges). This is a deliberate, documented behavior change to the query surface, not "transparent"
  (G4 corrected).

---

## 4. Migration `m0008_partition_audit_log` (D5 — serialized, data-preserving swap)

All steps via `execute_unprepared` raw SQL (sea-query can't express `PARTITION BY`). The `up` body
runs under a **`pg_advisory_xact_lock(<const key>)`** and is **guarded by an "already partitioned?"
check** (`SELECT 1 FROM pg_partitioned_table WHERE partrelid = 'audit_log'::regclass`) so a
concurrent first-boot across replicas cannot double-apply the destructive `DROP`/`RENAME` — the
second replica takes the lock after the first commits, sees the table already partitioned, and
no-ops. (SeaORM's migrator does **not** serialize concurrent `up()` by default; every prior
migration was additive `CREATE … IF NOT EXISTS` and tolerated the race — this destructive one must
not, so it self-serializes. The advisory key is an app-specific constant chosen to not collide with
the migrator.) `SET LOCAL TimeZone='UTC'` and `SET LOCAL lock_timeout` are set at the top.

**Expected scale:** this is a newly-built service; `audit_log` is empty or near-empty in every real
target environment at m0008 time, so the copy is trivial. The data-preserving path is chosen for
correctness hygiene (a dev/staging env *may* hold rows), not for scale; the `lock_timeout` bounds
the worst case.

**`up`** (ordered to avoid schema-global `ix_audit_log_*` index-name collisions):
1. `CREATE TABLE audit_log_new (…columns, PRIMARY KEY (id, occurred_at, outcome)) PARTITION BY LIST
   (outcome);` (columns listed explicitly with exact types/nullability/`DEFAULT '{}'`).
2. Create `committed`/`denied` subtrees (`PARTITION OF … FOR VALUES IN (…) PARTITION BY RANGE
   (occurred_at)`) and the `audit_log_other` LIST default.
3. Create month leaves for both subtrees spanning `min(occurred_at)…max(occurred_at)` of existing
   rows plus current + `ahead_months`, and the two `*_default` RANGE leaves (UTC-qualified bounds).
   Empty table → just current + ahead + defaults.
4. `INSERT INTO audit_log_new (id, occurred_at, actor_prn, action, resource_prn, outcome,
   determining_policies, detail, correlation_id) SELECT <same columns> FROM audit_log;` — **explicit
   column list** (never `SELECT *`), so a future column reorder can't silently misalign.
5. `DROP TABLE audit_log;` (frees the `ix_audit_log_*` names) → `ALTER TABLE audit_log_new RENAME TO
   audit_log;`.
6. Create the four indexes on the parent `audit_log` (post-load = faster + no name clash), and
   `ALTER TABLE audit_log RENAME CONSTRAINT audit_log_new_pkey TO audit_log_pkey;` (Postgres does not
   rename constraints on table rename — keeps parity with `m0006`).

**`down`:** reverse to the exact `m0006` shape (under the same advisory lock + UTC) — `CREATE TABLE
audit_log_plain (…, CONSTRAINT audit_log_pkey PRIMARY KEY (id))`, `INSERT … SELECT <cols> FROM
audit_log`, `DROP TABLE audit_log` (cascades the tree), `RENAME audit_log_plain → audit_log`,
recreate the original **five** indexes (incl. `outcome`).

Registered in `migration/mod.rs` after `m0007`.

> **Rollback tolerance:** rolling a binary back to a migrator that lacks `m0008` is safe — SeaORM's
> `up` only applies *pending* migrations and ignores an unknown *applied* row in `seaql_migrations`.
> A schema rollback uses the `down`.

---

## 5. Maintenance task (D2)

### 5.1 `PgPartitionMaintainer` (persistence adapter)

Partition management is pure Postgres DDL → it lives in `adapters/persistence/` (infrastructure),
**not** in `paigasus-iam-core`. `PgPartitionMaintainer { db }` with:

- `ensure_partitions_ahead(now, ahead_months)` — for each outcome subtree, `CREATE TABLE IF NOT
  EXISTS audit_log_<outcome>_YYYY_MM PARTITION OF audit_log_<outcome> FOR VALUES FROM (UTC bound) TO
  (UTC bound)` for `now … now + ahead_months`. Idempotent.
- `prune(now, policy)` — `DROP TABLE IF EXISTS audit_log_denied_YYYY_MM` for months strictly older
  than `denied_months`; the same for `audit_log_committed_YYYY_MM` **only when `committed_months >
  0`**. Never touches any `*_default` / `audit_log_other`.

**Concurrency & liveness posture (D11, hardened in rev 2):**
- **Each DDL op runs in its own short transaction**, each acquiring `pg_advisory_xact_lock(<const
  key>)` (one replica does DDL at a time) and `SET LOCAL lock_timeout` so a `CREATE`/`DROP` that
  would block behind live-insert locks on the parent **backs off** (errors → retried next tick)
  rather than queueing and stalling all audit inserts (hence mutations). Idempotent `IF [NOT]
  EXISTS` is the second safety layer.
- **`prune` is independent of `ensure_partitions_ahead`:** a create-ahead failure (e.g. a polluted
  default) must **not** skip pruning, or a single polluted default would wedge retention and cause
  unbounded growth in exactly the state that needs it. They are separate ops with separate
  transactions; each is attempted every tick regardless of the other's outcome.
- Month arithmetic uses `chrono` (already a dep) in **UTC** (`now` injected for testability);
  boundaries are computed as first-of-month UTC instants.

### 5.2 `spawn_partition_maintenance(...)` (service wiring in `main.rs`)

Mirrors the outbox relay:
- **Startup:** run `ensure_partitions_ahead` once, awaited. Failure is **non-fatal** (log a `warn`
  and continue) — the migration already created current+ahead leaves and the `*_default` backstops
  cover any gap, so a transient startup DDL error must not block the service from serving.
- **Loop:** `tokio::select!` between an `interval(interval_secs)` tick — which runs
  `ensure_partitions_ahead` then (independently) `prune` — and the shutdown-watch.
- **Gated** by `[audit.retention].enabled`.

### 5.3 Config `[audit.retention]` and its semantics (D6, D7)

```toml
[audit.retention]
enabled = true          # false → don't spawn the task AT ALL (see recovery-trap note)
interval_secs = 86400   # daily
ahead_months = 1        # create-ahead horizon
denied_months = 3       # DROP denied leaves older than this; 0 = never drop denied
committed_months = 0    # 0 = never auto-drop committed (opt-in)
```

- Added to `IamConfig` under the existing `[audit]` table + `iam.toml.example`. Env override
  `IAM_AUDIT__RETENTION__*`.
- `validate()`: `interval_secs > 0`, `1 ≤ ahead_months ≤ 24` (an upper cap — each create-ahead
  month is a parent-locking `CREATE`; a fat-fingered large value would hammer the parent every
  tick). `denied_months`/`committed_months` `= 0` means "don't prune that outcome" — a valid,
  documented value.
- **`committed_months` guardrail (D6):** a non-zero `committed_months` auto-deletes compliance data;
  startup logs a prominent `warn` stating the effective committed-drop window when it is non-zero.
- **`enabled = false` is a full off-switch, not a "pause deletes" mode (recovery-trap fix, D7):**
  disabling stops create-ahead *and* pruning; after `ahead_months` the defaults fill, and a polluted
  default then blocks create-ahead even after re-enabling (needs the §6.2 manual reattach). To
  **pause deletions while keeping create-ahead healthy**, set `denied_months = 0` and
  `committed_months = 0` (task still runs, creates leaves, drops nothing). The startup `warn` when
  `enabled = false` states this consequence explicitly.

---

## 6. Observability + RUNBOOK (G5)

### 6.1 Metrics

New consts in `paigasus_observability::names` (+ `names::ALL`, per the drift-test discipline):

| metric | type | labels | meaning |
|---|---|---|---|
| `iam_audit_partition_maintenance_ticks_total` | counter | `result` (`ok`/`error`) | one per maintenance tick — the task's **liveness** signal (mirrors `iam_outbox_relay_ticks_total`) |
| `iam_audit_partitions_created_total` | counter | — | leaves created by create-ahead |
| `iam_audit_partitions_dropped_total` | counter | `outcome` | leaves dropped by retention |
| `iam_audit_default_partition_rows` | gauge | — | rows in the `*_default` + `audit_log_other` partitions — **should be 0**; non-zero ⇒ create-ahead fell behind |

`iam_audit_default_partition_rows` is refreshed **once per successful maintenance tick** (a
`count(*)` over the default leaves). **Known blind spot:** it freezes if the task is stalled or
disabled — exactly when a default is most likely filling — so the `…_ticks_total` liveness alert
(below) is the primary "task not running" signal and this gauge is the secondary "already behind"
signal; the RUNBOOK states both.

### 6.2 RUNBOOK edits (`docs/ops/RUNBOOK-observability.md`)

- **§4 "Audit retention & partitioning":** replace the plain-table + interim batched-`DELETE` block
  with: the partition tree, the maintenance task + `[audit.retention]` config and its semantics
  (incl. the `enabled=false` recovery-trap vs `denied_months=0` pause mode), the real drop operation
  (automatic, plus ad-hoc `DROP TABLE audit_log_denied_YYYY_MM`), and the `*_default`/`audit_log_other`
  meaning + the manual reattach procedure for a non-empty default.
- **§2.2 catalog:** add the four metric rows.
- **§4 alert table + a new entry:** add `IamAuditPartitionMaintenanceStalled`
  (`rate(iam_audit_partition_maintenance_ticks_total[…]) == 0`, **warning** — slow bloat, not a live
  incident, unlike the relay's `critical`; the entry documents the `enabled=false` caveat that it
  fires forever when legitimately disabled) to `ops/observability/prometheus/rules/iam.rules.yml` +
  a `promtool test rules` case in `rules/tests/iam.test.yml`.
- **§6 Future:** remove the now-done "Monthly `audit_log` partitioning + scheduled retention job"
  bullet.

---

## 7. Testing strategy

Real PG via `tests/support` (testcontainers, PG 16); unit tests for config.

- **Schema (post-migrate):** assert `audit_log` is partitioned and the tree exists
  (`pg_partition_tree('audit_log')` / `pg_class` + `pg_inherits`) with committed/denied subtrees, the
  `audit_log_other` LIST default, current-month leaves, and both RANGE defaults. Insert committed +
  denied rows at current / next-month / far-future `occurred_at` **and a row with a stray outcome**
  (→ `audit_log_other`, must not fail the insert — the G1/D10 guarantee) → all route and query back.
- **Composite-PK resolution:** assert the DB PK is `(id, occurred_at, outcome)`; assert
  `audit_log::Entity::find_by_id(id)` still resolves a row against the partitioned parent.
- **Migration swap (richer, D5):** seed a **plain** `audit_log` with rows spanning **multiple
  months, a gap month, and min/max boundary rows** (incl. a row whose span-computed month must exist
  so nothing leaks to a default at migration time), run `m0008` `up`, assert **every** row survived,
  routed to the right leaf, and the table is now partitioned. Then run `down` and assert the plain
  shape + rows are restored.
- **Timezone (D9):** run the migration + `ensure_partitions_ahead` under a **non-UTC** session TZ
  (e.g. `America/New_York`) and assert a boundary-adjacent row routes to the correct UTC month
  (guards the bare-literal regression).
- **Maintainer:** `ensure_partitions_ahead` idempotent (run twice, no error); `prune` with
  `denied_months=3, committed_months=0` drops aged denied leaves, keeps recent denied **and all
  committed** (even old) and never a default; a run with `committed_months>0` drops aged committed;
  `prune` still runs when `ensure_partitions_ahead` errors (independence, D11).
- **Bounded query window (D12):** a query with no `from`/`to` applies the default lookback; an
  over-wide explicit range is clamped to the max window.
- **Config:** `validate()` rejects `interval_secs=0`, `ahead_months=0`, `ahead_months=25`;
  `enabled=false` doesn't spawn the task; a non-zero `committed_months` emits the guardrail warn.
- **Regression (expanded, D3):** `audit_log_pg.rs`, `http_audit.rs`, `grpc_audit.rs`,
  `mutation_audit_e2e.rs`, **and the `find_by_id` suites** `api_keys_pg.rs`, `outbox_uow_pg.rs`,
  `authz_role_grants.rs`, `authz_policy_store.rs` all stay green with no adapter/caller changes.

---

## 8. CI / gates

- **No new crate, no new deps** (`sea-orm`, `chrono`, `tokio`, `metrics` already deps) →
  `:affected-smoke`, `:deny`, `:machete` untouched; no waivers expected.
- Raw-SQL migration + a metric referenced in a new alert rule → run the full gate list before push:
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
  :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.
  `:parity-corpus-drift`/`:breaking` unaffected (no proto/Cedar-schema change); the observability
  **drift test** stays green (every new metric name added to `names::ALL`).
- One cohesive PR (**"Closes SMA-467"**) — the maintenance task depends on the partitioned table and
  the new port, so the pieces don't split cleanly.

---

## 9. Decision log

| # | Decision | Rationale |
|---|---|---|
| **D1** | `PARTITION BY LIST (outcome)` → each subtree `PARTITION BY RANGE (occurred_at)` monthly | The denied subtree becomes the droppable unit → faithful to D14; a single-level `RANGE(occurred_at)` table mixes outcomes in one leaf and can't outcome-selectively drop denials |
| **D2** | Retention/create-ahead via an **in-app background task** (relay pattern), not `pg_cron`/`pg_partman` | No Postgres-extension dependency (test image + CI lack it); testable in the existing harness; multi-replica-safe via advisory lock + idempotent DDL |
| **D3** | DB PK `(id, occurred_at, outcome)`; SeaORM entity keeps sole `id` PK; `find_by_id` still resolves | PG requires every partition-level key in the PK; `id` (UUIDv7) stays the logical identity; adapter inserts/filters + existing `find_by_id` reads all work against the partitioned parent (verified) |
| **D4** | Per-subtree RANGE `*_default` partitions | Committed inserts are in-txn (G1) — a missing month leaf would roll back the mutation; also de-flakes the `occurred_at` range test |
| **D5** | Migration is a **cross-replica-serialized, data-preserving swap** (advisory lock + already-partitioned guard; create-new → copy(explicit cols) → drop-old → rename → index; rename PK constraint), with a real `down` | Correct hygiene even if a dev/staging env holds rows; guard/ordering prevent double-apply and index-name collisions; table is empty/tiny in practice so the copy is trivial |
| **D6** | Defaults: denied auto-drop at 3mo; committed never (opt-in); non-zero `committed_months` warns | Never accidentally delete compliance data; matches the RUNBOOK's 90-day denial example |
| **D7** | Retention windows + schedule are validated `[audit.retention]` config; `enabled=false` fully off, `denied_months=0` is the pause-deletes mode | Compliance owns the actual windows; a clear off-switch that doesn't strand the table with a filling default |
| **D8** | Non-empty-`DEFAULT` auto-remediation deferred (metric + manual RUNBOOK step) | Only occurs if the task is disabled/down ≈ a full `ahead_months`; auto-moving leaked rows is pg_partman-class work → YAGNI for v1, made observable |
| **D9** | **UTC-pinned** partition bounds (fully-qualified `timestamptz` literals) + `SET LOCAL TimeZone='UTC'` in migration/maintenance txns | Bare date literals cast in the session TZ → wrong routing + misaligned retention in non-UTC sessions, invisible in the UTC test container |
| **D10** | **Top-level `LIST` default** `audit_log_other` | `outcome` is unconstrained TEXT with no LIST default → a stray value hard-fails the in-txn committed insert (G1 regression the plain table lacks) |
| **D11** | Retention runs **each DDL op in its own txn under `lock_timeout`**, prune **independent** of create-ahead | Avoid stalling live audit inserts (hence mutations) behind a queued `CREATE`/`DROP`; a polluted default must not wedge pruning |
| **D12** | Land the M5 §9 **bounded default/max query window** | Partitioning turns an unfiltered read into a growing `MergeAppend` over all leaves; a default lookback prunes it — reads are *not* transparently free (G4) |

## 10. Follow-ups (not in scope)

- `DETACH … CONCURRENTLY`-then-`DROP` retention (PG14+) to fully eliminate the brief parent lock a
  `DROP TABLE <leaf>` still takes.
- Auto-remediate a non-empty `DEFAULT` partition (reattach leaked rows into a fresh month leaf).
- A Grafana dashboard panel for the maintenance metrics (metric consts + alert ship now).
- Generalise the maintainer if `event_outbox` (or another table) later needs the same
  create-ahead/retention treatment (the outbox pruning follow-up from the M5 §14 list).

## 11. Implementation slices

One cohesive PR. Suggested internal ordering for the plan (all in one branch): migration `m0008` +
entity doc → `PgPartitionMaintainer` + config → `spawn_partition_maintenance` wiring → bounded query
window → metrics → tests → RUNBOOK.

## 12. Changelog — Stage-2 challenge fold-in (rev 2)

All challenge findings were justified (verified against the live adapter/entity/tests). Folded:
**UTC-pinned partition bounds** (BLOCKER → D9/§3.5); **top-level `LIST` default** to preserve G1
(MAJOR → D10/§3.3); **cross-replica-serialized destructive swap** via advisory lock + already-
partitioned guard (MAJOR → D5/§4); **swap expected-empty + `lock_timeout` + migrator-runs-once
correction** (MAJOR → §4); **retention per-op txn + `lock_timeout` back-off + prune independent of
create-ahead** (MAJOR → D11/§5.1); **`enabled=false` recovery-trap clarified**, `denied_months=0`
pause mode (MAJOR → D7/§5.3); **corrected D3 `find_by_id` claim + expanded regression list** (MAJOR →
§3.2/§7); **bounded default/max query window** implementing M5 §9 (MAJOR → D12/§3.6); explicit
`INSERT` column list, PK-constraint rename, `ahead_months ≤ 24` cap, `committed_months` guardrail
warn, PG floor stated, default-rows gauge population + blind spot, richer multi-month/boundary swap
test, non-fatal startup, alert disabled caveat (MINORs → §4/§5/§6/§7). Nothing was rejected.
