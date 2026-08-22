# SMA-559 — Guard `Migrator::up` with a Postgres advisory lock

**Issue:** [SMA-559](https://linear.app/smaschek/issue/SMA-559/) · **Status:** design approved
**Related:** SMA-500 (container images, surfaced this), SMA-513 (Helm chart, consumes the decision)

## 1. Problem

`paigasus-iam`'s composition root (`rs/crates/services/paigasus-iam/src/main.rs:109`) runs

```rust
Migrator::up(&db, None).await?;
```

unconditionally on every process start, before any listener binds, with nothing serialising it.
`sea-orm-migration` does not serialise concurrent `up()` itself. A rolling update or a
`replicas: 2` scale-out therefore starts a second IAM while the first may still be migrating.

The only mitigation today is documentation: `docs/ops/RUNBOOK-containers.md` §5 tells operators to
migrate with `replicas: 1` and `strategy.rollingUpdate.maxSurge: 0`, or to use a pre-install Job.
Nothing enforces that, and SMA-513's chart would have to honour it by hand.

## 2. What the risk actually is

The issue text says the worst case is "a half-applied migration". **On Postgres that is already
impossible**, and the spec records the corrected reading so the fix targets the real failure.

`sea-orm-migration-1.1.20`'s `exec_with_connection` (`src/migrator.rs:252-273`) special-cases
Postgres:

```rust
match db.get_database_backend() {
    DbBackend::Postgres => {
        let transaction = db.begin().await?;
        let manager = SchemaManager::new(&transaction);
        f(&manager).await?;
        transaction.commit().await
    }
    ...
}
```

and `exec_up` (`src/migrator.rs:360-401`) applies every pending migration *and* writes every
`seaql_migrations` bookkeeping row through that same `manager`. So one `Migrator::up` is one
transaction: Postgres' transactional DDL makes it atomic. It commits fully or not at all.

What concurrent starts genuinely risk:

* **Deadlock** between two migrating transactions taking the same objects' locks in
  data-dependent order.
* **Duplicate-object failure.** Replica B's `CREATE TABLE` blocks on replica A's uncommitted DDL,
  then fails with "already exists" once A commits. B's transaction rolls back — correct, but B's
  *boot* fails, and under Kubernetes that is a crash-loop rather than a converged rollout.
* **Long lock contention** on a large migration, with no operator-visible explanation.

All three are fixed by serialising the migration; none is fixed by documentation.

`m0008_partition_audit_log` already hand-rolled an advisory lock plus an "already partitioned?"
guard for exactly this reason. This design generalises that protection from one migration to the
whole run, so a future migration does not have to remember to hand-roll it again.

## 3. Design

### 3.1 Mechanism

New module `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`:

```rust
/// Namespaces the WHOLE migration run. Must never collide with `AUDIT_PARTITION_LOCK_KEY`
/// (5_580_467) — see §3.3 for why the two keys' ordering is load-bearing.
pub const MIGRATION_LOCK_KEY: i64 = 5_580_559;

pub async fn migrate_under_lock(
    db: &DatabaseConnection,
    wait: Duration,
) -> Result<(), MigrationLockError>;
```

Because `Migrator::up` accepts `&DatabaseTransaction` directly
(`sea-orm-migration/src/connection.rs:144`), the lock can be **transaction-scoped** and taken on
the very transaction the migration runs in. Postgres releases it on commit *or* rollback, so no
unlock path can be missed — including a panic or a dropped connection.

The loop is **do-while**: the deadline is checked *after* a failed attempt, never before the
first one, so `wait == 0` still tries exactly once rather than failing without ever asking
Postgres (see §3.4). Loop with `deadline = Instant::now() + wait`:

1. `let txn = db.begin().await?;`
2. `SELECT pg_try_advisory_xact_lock(MIGRATION_LOCK_KEY)` — the `try` variant never blocks, so the
   bounded thing is *our poll loop*, not a lock wait inside Postgres. This is what makes the
   deadline exact and independent of whether `lock_timeout` applies to advisory locks.
3. `false` → roll the (empty) transaction back, emit a throttled
   `tracing::info!` — "another replica is migrating; waiting" with elapsed and remaining — back
   off, and retry. Past the deadline → `Err(MigrationLockError::Contended { waited, key })`.
4. `true` → `Migrator::up(&txn, None).await?` then `txn.commit().await?`.

Rolling back between polls (rather than holding one transaction open across the whole wait) keeps
the session out of `idle in transaction`, so a deployment with
`idle_in_transaction_session_timeout` set cannot have its waiter killed mid-wait.

**Deliberately no `SET LOCAL lock_timeout` on the migration transaction.** That would newly bound
every existing migration's own table-lock waits, changing behaviour unrelated to this issue.
m0008 already sets its own where it wants one.

### 3.2 Second-runner semantics

Once the leader commits, the waiter acquires the lock, and `Migrator::up` finds
`seaql_migrations` complete and applies nothing. The waiter boots normally. This is the "blocks
until the first finishes and then finds nothing to do" behaviour AC 1 asks for.

### 3.3 Deadlock-freedom across the two advisory keys

The crate now has two advisory-lock keys, so the ordering is stated rather than assumed:

* A migrating transaction takes `MIGRATION_LOCK_KEY`, then — inside m0008 only —
  `AUDIT_PARTITION_LOCK_KEY`.
* `PgPartitionMaintainer` takes `AUDIT_PARTITION_LOCK_KEY` and **never** the migration key.

Every acquirer that holds both takes them in the same order, and the maintainer holds only the
second, so no cycle exists. A future component that needs both must take them in this order.

### 3.4 Config

A new `[migration]` section on `IamConfig`, matching the existing `[outbox]` / `[metrics]` /
`[authn]` style (there is no `[database]` section today — `database_url` is top-level):

```toml
[migration]
lock_wait_secs = 300    # IAM_MIGRATION__LOCK_WAIT_SECS
```

300s default: m0008 copies the entire `audit_log` table, which on a large deployment is genuinely
slow, so a hardcoded ceiling would be a boot-time landmine. `0` is **legal** and means "try once,
fail immediately if contended" — a deliberate fail-fast mode for an operator who would rather see
a crash than a wait. It is documented, not a `validate()` error.

Requires: the struct + `Deserialize`, an entry in config.rs's `Defaults` struct, and doc comments.

### 3.5 Composition root

`main.rs:109` becomes `migrate_under_lock(&db, config.migration.lock_wait()).await?`.

The helper's doc comment carries the intent that production code must not call `Migrator::up`
bare. No CI single-site gate: there is exactly one production call site, the gateway does not
migrate, and the five other call sites are integration tests that legitimately drive migrations
step by step — a gate would ship mostly allowlist, at the cost of a `ci.yml` `T=()` entry, the
CLAUDE.md marker block, and an `:affected-smoke` re-baseline.

## 4. Testing

`rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs`, Docker-backed through
`support::docker::start_or_skip` (the single-site skip-versus-panic policy —
`repo:iam-docker-policy-single-site` fails if a suite hand-rolls its own).

1. **Convergence — AC 1 and AC 2.** One raw Postgres container; two independent
   `DatabaseConnection`s to it; `tokio::join!` two `migrate_under_lock` calls. Assert both return
   `Ok`, that `seaql_migrations` holds exactly `Migrator::migrations().len()` rows, and that a
   schema probe confirms the tip shape. The whole join runs inside `tokio::time::timeout` — **the
   timeout is the deadlock assertion**, the same technique `outbox_retention_concurrency_pg.rs`
   uses.
2. **The lock is load-bearing.** An independent connection pinned to `max_connections(1)` /
   `min_connections(1)` holds `pg_advisory_lock(MIGRATION_LOCK_KEY)` — session-level and
   transaction-level advisory locks share one lock space, so they conflict. Then
   `migrate_under_lock(db, Duration::from_secs(1))` must return `Contended` **and leave the
   database unmigrated** (`seaql_migrations` absent). Release via `pg_advisory_unlock`; a
   subsequent call succeeds.

   This is the deterministic form of "prove the guard does something". The tempting alternative —
   run two *unguarded* migrations and assert breakage — is not deterministic: m0008 is already
   individually hardened, and whether a duplicate-object error or a deadlock surfaces depends on
   timing.
3. **Second runner is a no-op.** After a completed migration, another `migrate_under_lock` returns
   `Ok` and leaves the migration count unchanged.

## 5. Documentation

`docs/ops/RUNBOOK-containers.md`:

* **§5 bullet** — replace "runs `Migrator::up` on every process start, with no advisory lock
  around it … Migrate with a single replica" with: IAM serialises boot migrations with a Postgres
  advisory lock, so concurrent starts are safe by construction; single-replica migration is now a
  **recommendation** for a long migration, not a requirement; document `lock_wait_secs` and what a
  `Contended` boot failure means and how to respond to it.
* **Probe table, line 92** — a *waiting* replica's startup is the leader's migration time **plus**
  its own, so a generous `startupProbe.failureThreshold` matters more after this change, not less.

## 6. AC 4 — SMA-513

AC 4 asks that the choice be reflected in SMA-513's chart defaults. That cannot be done in code
here: SMA-513 is in Backlog and there is no Helm chart anywhere in the repo (no `Chart.yaml`, no
`deploy/`, no `charts/`). The decision is instead recorded where SMA-513 will read it:

* the runbook change in §5, and
* a comment on SMA-513 stating that the chart no longer needs `maxSurge: 0` or a pre-install
  migration Job, and should carry a generous `startupProbe.failureThreshold` instead.

## 7. Rejected

* **A boot metric.** A once-per-process event does not earn a metric family plus the
  `:observability-drift` surface it would add. The throttled `tracing` lines carry it.
* **`SET lock_timeout` + blocking `pg_advisory_lock`.** Simpler, but depends on `lock_timeout`
  applying to advisory-lock waits, and produces no "still waiting" log — the single thing that
  makes a stuck rollout legible.
* **A `paigasus-iam migrate` subcommand (issue Option 2).** No consumer: there is no chart to run
  it as a Job. `health::dispatch` also lives in the shared `paigasus-observability` crate, which
  the gateway uses and which has no migrations, so a migrate mode there would be dead for one of
  its two consumers. Deferred to its own issue when a chart wants it.
* **A `repo:migrator-single-site` CI gate.** See §3.5.
* **Blocking indefinitely, or failing fast on first contention.** Rejected in favour of the
  bounded, configurable wait — the former hangs opaquely outside Kubernetes, the latter turns
  every rolling update into a guaranteed crash-and-retry.
