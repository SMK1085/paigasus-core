# SMA-467 — IAM `audit_log` time-partitioning + outcome-aware retention

**Status:** Design (brainstormed) · **Date:** 2026-07-15 · **Linear:** SMA-467 (closes) ·
**Service:** `paigasus-iam` · **Related:** SMA-446 (M5 audit/outbox — this implements its §4/D14
deferred retention design)

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
  `determining_policies (text, JSON-encoded Vec<String>)`, `detail (text, JSON)`, `correlation_id`.
  Five indexes: `occurred_at`, `actor_prn`, `resource_prn`, `action`, `outcome`. The
  `determining_policies`/`detail`/`outcome` columns are serialized **TEXT** (Slice-A convention; no
  native `jsonb`/`text[]`).
- **`PgAuditLog`** (`adapters/persistence/pg_audit_log.rs`): `record` (in-txn, for committed
  mutation rows — the audit insert is atomic with the mutation, G1), `record_out_of_band`
  (autocommit, for denial rows drained from the D8 buffer), and `query` (keyset pagination:
  `ORDER BY id DESC` — UUIDv7 ids double as occurred-at-descending — `WHERE id < cursor`, equality
  filters per present `AuditFilter` field, `LIMIT capped_limit()`).
- **SeaORM entity** `entities/audit_log.rs`: `id` is the sole `#[sea_orm(primary_key,
  auto_increment = false)]`.
- **Migration harness**: `adapters/persistence/migration/` with `Migrator` in `mod.rs`; migrations
  `m0001`–`m0007`. Ample `execute_unprepared` raw-SQL precedent for DDL sea-query can't express
  (partial indexes in `m0007`, CHECK constraints in `m0004`, functions/triggers in `m0002`).
- **Background-task pattern**: the outbox relay (`spawn`ed in `main.rs`, mirrors the policy-snapshot
  `spawn_reload` loop — a `tokio::select!` on an interval vs. a shutdown-watch, config-gated by
  `[outbox].relay_enabled`, with a startup `warn` when disabled). This maintenance task mirrors it.
- **Observability**: metric names are `const`s in `paigasus_observability::names` (+ a `names::ALL`
  slice); a drift test (`paigasus-observability/tests/drift.rs`) asserts every `iam_`/`gateway_`
  identifier referenced in committed dashboard JSON / rule YAML is in `names::ALL`. RUNBOOK **prose**
  tables are explicitly *not* drift-tested (§2 of the RUNBOOK).
- **Integration tests** run against an ephemeral Postgres via `tests/support` (testcontainers);
  Docker-less laptops skip, CI treats a missing daemon as a hard failure.

---

## 2. Goals / Non-goals

### Goals (acceptance criteria)

- **G1.** `audit_log` is a **two-level partitioned table**: `PARTITION BY LIST (outcome)` →
  each outcome subtree `PARTITION BY RANGE (occurred_at)` monthly. A new migration
  (`m0008_partition_audit_log`) performs a **data-preserving** conversion and has a working `down`
  that restores the exact `m0006` plain-table shape.
- **G2.** An **in-app maintenance task** (a) creates upcoming monthly leaf partitions ahead of time
  so inserts never fail for lack of a partition, and (b) enforces **outcome-aware retention** by
  dropping aged-out denied (and optionally committed) monthly leaves, on a schedule, multi-replica
  safe.
- **G3.** Retention windows and the schedule are **config** (`[audit.retention]` in `IamConfig`)
  with validated defaults: denied dropped at 3 months, committed never auto-dropped (opt-in).
- **G4.** The change is **transparent to the read/write adapter and callers**: `PgAuditLog::record`
  / `record_out_of_band` / `query` and all existing audit tests keep working unchanged (partition
  routing + pruning are handled by Postgres).
- **G5.** The **RUNBOOK** replaces its interim batched-`DELETE` retention procedure with the real
  partition-tree + maintenance-task + partition-drop documentation, and the now-done follow-up is
  retired from §6.

### Non-goals (out; tracked elsewhere or deliberately deferred)

- Auto-remediating a **non-empty `DEFAULT` partition** (moving leaked rows into a freshly created
  month leaf). v1 surfaces it as a metric + a manual RUNBOOK reattach step; it only occurs if the
  maintenance task is down for ≈ a full `ahead_months` window.
- A `pg_cron`/`pg_partman` extension-based scheduler (rejected — adds an infra dependency the test
  Postgres image and CI don't have; see D2).
- Changing the audit **column types** (`detail`/`determining_policies`/`outcome` stay TEXT — the
  Slice-A convention is unchanged; this migration only touches partitioning, PK, indexes).
- Auditing new events, changing the query surface, or touching the outbox/relay.
- A Grafana **dashboard** panel for the new metrics (metric consts + a RUNBOOK prose entry + one
  alert rule ship; a dashboard panel is optional polish, not required by the issue).

---

## 3. Architecture

### 3.1 Partition topology (D1)

```
audit_log                         PARTITION BY LIST (outcome)
├─ audit_log_committed            PARTITION BY RANGE (occurred_at)
│   ├─ audit_log_committed_2026_07   FOR VALUES FROM ('2026-07-01') TO ('2026-08-01')
│   ├─ audit_log_committed_2026_08   …
│   └─ audit_log_committed_default    DEFAULT            ← write-safety backstop
└─ audit_log_denied               PARTITION BY RANGE (occurred_at)
    ├─ audit_log_denied_2026_07      FOR VALUES FROM ('2026-07-01') TO ('2026-08-01')
    ├─ audit_log_denied_2026_08      …
    └─ audit_log_denied_default       DEFAULT            ← write-safety backstop
```

Retention drops whole aged-out **denied** month leaves (`audit_log_denied_YYYY_MM`) on the short
window; committed leaves are kept indefinitely by default. The denied subtree *is* the droppable
unit — this is the faithful realisation of D14's "drop old denial partitions."

### 3.2 Composite primary key (D3)

Postgres requires a partitioned table's PK/unique constraints to include **every partitioning
level's key column**. With `LIST (outcome)` at the top and `RANGE (occurred_at)` beneath, the DB
PK becomes **`(id, occurred_at, outcome)`**.

> **Verify-at-implementation:** the *exact minimal* column set Postgres demands for a two-level
> hierarchy is asserted by the migration integration test against real PG, not trusted from memory.
> If PG accepts a smaller set, use it; the design assumes the full `(id, occurred_at, outcome)`.

Consequences:
- `id` is no longer DB-unique on its own (only the triple is). In practice `id` is a per-entry
  UUIDv7 — collisions are astronomically improbable — so this is a standard, acceptable partitioning
  trade-off, not a real weakening.
- **The SeaORM entity keeps `id` as its sole `primary_key`.** Inserts set every column and reads use
  `.filter(Column::…)` (never `find_by_id`), so the single-column entity PK stays functionally
  correct; a doc comment on the entity records the DB/entity PK divergence and why.

### 3.3 `DEFAULT` partition backstop (D4)

Each outcome subtree gets a `*_default` partition. Rationale:
- **Committed audit rows are inserted inside the mutation's transaction (G1).** A committed insert
  that found no matching month leaf would fail and **roll back the whole mutation** — an
  availability landmine. The default guarantees every row has a home even if create-ahead lags.
- It makes the existing `query_filters_by_occurred_at_from_and_to` test (which writes `now ± 2h`)
  robust across month boundaries instead of flaky.

Trade-off: `CREATE TABLE … PARTITION OF … FOR VALUES FROM … TO …` while the default holds rows in
that range makes PG scan the default and error. Kept a non-issue by staying `ahead_months ≥ 1` ahead
so the default stays empty in steady state; a non-empty default is surfaced by the
`iam_audit_default_partition_rows` gauge and a manual RUNBOOK reattach step (auto-remediation is a
non-goal, §2).

### 3.4 Indexes

The four useful indexes (`occurred_at`, `actor_prn`, `resource_prn`, `action`) are created **on the
partitioned parents**, so Postgres cascades them to every present and future leaf. The **`outcome`
index is dropped** — `outcome` is now the top LIST partition key, so within any leaf it is a
constant and the index is dead weight (outcome filters are served by partition pruning). `down`
restores the original five indexes (incl. `outcome`) on the plain table.

---

## 4. Migration `m0008_partition_audit_log` (D5 — data-preserving swap)

All steps via `execute_unprepared` raw SQL (sea-query can't express `PARTITION BY`). Ordered to
avoid `ix_audit_log_*` index-name collisions between the old and new tables (index names are
schema-global in Postgres).

**`up`:**
1. `CREATE TABLE audit_log_new (…same columns…, PRIMARY KEY (id, occurred_at, outcome)) PARTITION BY
   LIST (outcome);`
2. `CREATE TABLE audit_log_committed PARTITION OF audit_log_new FOR VALUES IN ('committed') PARTITION
   BY RANGE (occurred_at);` and the `denied` counterpart.
3. Create month leaves for both subtrees spanning **`min(occurred_at)`…`max(occurred_at)` of existing
   rows** plus **current + `ahead_months`** months, and the two `*_default` leaves. (Existing-row
   span computed from `audit_log`; empty table → just current + ahead + defaults.)
4. `INSERT INTO audit_log_new SELECT * FROM audit_log;` (routes each row to its leaf).
5. `DROP TABLE audit_log;` (frees the `ix_audit_log_*` names) → `ALTER TABLE audit_log_new RENAME TO
   audit_log;` (child/leaf names are independent of the parent name — only the top rename matters).
6. Create the four indexes on the parent `audit_log` (post-load = faster + no name clash).

**`down`:** reverse to the exact `m0006` shape — `CREATE TABLE audit_log_plain (…, PRIMARY KEY (id))`,
`INSERT … SELECT * FROM audit_log`, `DROP TABLE audit_log` (cascades the tree), `RENAME
audit_log_plain → audit_log`, recreate the original **five** indexes (incl. `outcome`).

Registered in `migration/mod.rs` after `m0007`.

> **Verify-at-implementation:** insert-with-`RETURNING` routing into a partitioned parent via SeaORM
> `ActiveModel::insert` (used by `record`/`record_out_of_band`) works on PG 11+; confirmed by the
> existing `audit_log_pg` tests passing unchanged against the partitioned table.

---

## 5. Maintenance task (D2)

### 5.1 `PgPartitionMaintainer` (persistence adapter)

Partition management is pure Postgres DDL → it lives in `adapters/persistence/` (infrastructure),
**not** in `paigasus-iam-core`. A `PgPartitionMaintainer { db: DatabaseConnection }` with:

- `ensure_partitions_ahead(now, ahead_months)` — for each outcome subtree, `CREATE TABLE IF NOT
  EXISTS audit_log_<outcome>_YYYY_MM PARTITION OF audit_log_<outcome> FOR VALUES FROM
  ('YYYY-MM-01') TO (first-of-next-month)` for `now …= now + ahead_months`. Idempotent.
- `prune(now, policy)` — `DROP TABLE IF EXISTS audit_log_denied_YYYY_MM` for months strictly older
  than `denied_months`; the same for `audit_log_committed_YYYY_MM` **only when `committed_months >
  0`**. Never touches `*_default`.
- Both wrapped in `SELECT pg_advisory_xact_lock(<const key>)` inside a short transaction so **one
  replica performs DDL per tick** — concurrent `CREATE`/`DROP` from multiple replicas can't race.
  Idempotent `IF [NOT] EXISTS` DDL is the second layer of safety.

Month arithmetic uses `chrono` (already a dep); the task takes the clock as an injected `now` for
testability (mirrors the codebase's clock-injection posture).

### 5.2 `spawn_partition_maintenance(...)` (service wiring in `main.rs`)

Mirrors the outbox relay:
- **Startup:** run `ensure_partitions_ahead` **once, awaited, before/at service start** so the first
  writes have a home even across a month boundary (the migration already created current+ahead, so
  this is belt-and-suspenders; the `*_default` backstop covers the gap regardless).
- **Loop:** `tokio::select!` between an `interval(interval_secs)` tick — which runs
  `ensure_partitions_ahead` then `prune` — and the shutdown-watch.
- **Gated** by `[audit.retention].enabled`; when `false`, the task is not spawned and a startup
  `warn` fires (an un-pruned table grows unbounded and create-ahead won't run — the `*_default`
  backstop still prevents write failures).

### 5.3 Config `[audit.retention]`

```toml
[audit.retention]
enabled = true          # false → don't spawn the task (startup warn)
interval_secs = 86400   # daily
ahead_months = 1        # create-ahead horizon
denied_months = 3       # DROP denied leaves older than this
committed_months = 0    # 0 = never auto-drop committed (opt-in)
```

Added to `IamConfig` (nested under the existing `[audit]` table) + `iam.toml.example`. `validate()`:
`interval_secs > 0`, `ahead_months ≥ 1`. `0` on either window = "don't prune that outcome" (a valid,
documented value, not an error). Env override via figment `IAM_AUDIT__RETENTION__*`.

---

## 6. Observability + RUNBOOK (G5)

### 6.1 Metrics

New consts in `paigasus_observability::names` (+ `names::ALL`, per the drift-test discipline):

| metric | type | labels | meaning |
|---|---|---|---|
| `iam_audit_partition_maintenance_ticks_total` | counter | `result` (`ok`/`error`) | one per maintenance tick — the task's **liveness** signal (mirrors `iam_outbox_relay_ticks_total`) |
| `iam_audit_partitions_created_total` | counter | — | leaves created by create-ahead |
| `iam_audit_partitions_dropped_total` | counter | `outcome` | leaves dropped by retention |
| `iam_audit_default_partition_rows` | gauge | — | rows currently in the `*_default` partitions — **should be 0**; non-zero ⇒ create-ahead fell behind |

### 6.2 RUNBOOK edits (`docs/ops/RUNBOOK-observability.md`)

- **§4 "Audit retention & partitioning":** replace the "Current implementation status: plain table"
  + interim batched-`DELETE` block with: the partition tree, the maintenance task + `[audit.retention]`
  config, the real drop operation (automatic, plus ad-hoc `DROP TABLE audit_log_denied_YYYY_MM`), and
  the `*_default` partition meaning + manual reattach procedure for a non-empty default.
- **§2.2 catalog:** add the four metric rows above.
- **§4 alert table + a new entry:** add `IamAuditPartitionMaintenanceStalled`
  (`rate(iam_audit_partition_maintenance_ticks_total[…]) == 0`, warning) to
  `ops/observability/prometheus/rules/iam.rules.yml` + a `promtool test rules` case in
  `rules/tests/iam.test.yml` (matching the existing alert discipline).
- **§6 Future:** remove the now-done "Monthly `audit_log` partitioning + scheduled retention job"
  bullet.

---

## 7. Testing strategy

Real PG via `tests/support` (testcontainers); unit tests for config.

- **Schema (post-migrate):** assert `audit_log` is partitioned and the tree exists
  (`pg_partition_tree('audit_log')` / `pg_class` + `pg_inherits`) with committed/denied subtrees,
  current-month leaves, and both defaults. Insert committed + denied rows at current / next-month /
  far-future `occurred_at` → all route and query back (routing + default backstop).
- **Migration swap:** a focused test that seeds a **plain** `audit_log` with a row, runs the `m0008`
  `up` SQL, and asserts the row survived and the table is now partitioned (validates data-preservation
  + the composite-PK column set — this is where the D3 verify-at-implementation resolves).
- **Maintainer:** `ensure_partitions_ahead` is idempotent (run twice, no error, expected leaves
  exist); `prune` with `denied_months=3, committed_months=0` drops aged denied leaves, keeps recent
  denied **and all committed** (even old), and never a `*_default`; a second run with
  `committed_months>0` drops aged committed too.
- **Config:** `validate()` rejects `interval_secs=0` / `ahead_months=0`; `enabled=false` doesn't
  spawn the task.
- **Regression:** the existing `audit_log_pg.rs` (3 tests), `http_audit.rs`, `grpc_audit.rs`,
  `mutation_audit_e2e.rs` stay green with no adapter/caller changes.

---

## 8. CI / gates

- **No new crate, no new deps** (`sea-orm`, `chrono`, `tokio`, `metrics` are already deps) →
  `:affected-smoke`, `:deny`, `:machete` untouched; no waivers expected.
- Raw-SQL migration + a metric referenced in a new alert rule → run the full gate list before push:
  `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
  :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.
  `:parity-corpus-drift`/`:breaking` are unaffected (no proto / Cedar-schema change); the
  observability **drift test** stays green because every new metric name is added to `names::ALL`.
- One cohesive PR (**"Closes SMA-467"**) — the maintenance task depends on the partitioned table and
  the new port, so the pieces don't split cleanly.

---

## 9. Decision log

| # | Decision | Rationale |
|---|---|---|
| **D1** | `PARTITION BY LIST (outcome)` → each subtree `PARTITION BY RANGE (occurred_at)` monthly | The denied subtree becomes the droppable unit → faithful to D14 "drop old denial partitions"; a pure `RANGE(occurred_at)` monthly partition mixes committed+denied and can't be outcome-selectively dropped |
| **D2** | Retention/create-ahead via an **in-app background task** (relay pattern), not `pg_cron`/`pg_partman` | No Postgres-extension dependency (test image + CI lack it); unit/integration-testable in the existing harness; consistent with how the service already does background work; multi-replica-safe via advisory lock + idempotent DDL |
| **D3** | DB PK becomes `(id, occurred_at, outcome)`; SeaORM entity keeps sole `id` PK | PG requires every partition-level key in the PK; `id` (UUIDv7) stays the logical identity; inserts/filters don't need the DB PK reflected in the entity → zero adapter churn |
| **D4** | Per-subtree `*_default` partitions as a write-safety backstop | Committed inserts are in-txn (G1) — a missing month leaf would roll back the mutation; the default guarantees a home and de-flakes the `occurred_at` range test across month boundaries |
| **D5** | Migration is a **data-preserving swap** (create-new → copy → drop-old → rename → index), with a real `down` | Correct migration hygiene even if a dev/staging env already holds rows; ordering dodges schema-global index-name collisions |
| **D6** | Defaults: denied auto-drop at 3mo; committed never (opt-in `committed_months`) | Never accidentally delete compliance/mutation data; "denials shorter than mutations" holds trivially; matches the RUNBOOK's existing 90-day denial example |
| **D7** | Retention windows + schedule are validated `[audit.retention]` config | Compliance owns the actual windows (the RUNBOOK explicitly declines to prescribe day-counts) → operators tune without a code change |
| **D8** | Non-empty-`DEFAULT` auto-remediation deferred (metric + manual RUNBOOK step) | Only occurs if the task is down ≈ a full `ahead_months`; auto-moving leaked rows is fiddly (pg_partman-class work) → YAGNI for v1, made observable instead |

## 10. Follow-ups (not in scope)

- Auto-remediate a non-empty `DEFAULT` partition (reattach leaked rows into a freshly created month
  leaf).
- A Grafana dashboard panel for the maintenance metrics (metric consts + alert ship now; the panel
  is optional).
- Generalise the maintainer if `event_outbox` (or another table) later needs the same
  create-ahead/retention treatment (the outbox pruning follow-up from the M5 §14 list).
