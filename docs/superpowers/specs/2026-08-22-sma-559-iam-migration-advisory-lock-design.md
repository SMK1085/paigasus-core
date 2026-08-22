# SMA-559 — Guard `Migrator::up` with a Postgres advisory lock

**Issue:** [SMA-559](https://linear.app/smaschek/issue/SMA-559/) · **Status:** approved, revised after two adversarial passes
**Related:** SMA-500 (container images, surfaced this), SMA-513 (Helm chart, consumes the decision),
[SMA-571](https://linear.app/smaschek/issue/SMA-571/) (bind-first readiness gating, split out of this)

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

`sea-orm-migration-1.1.20`'s `exec_with_connection` (`src/migrator.rs:261-273`) special-cases
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

### 2.1 What this lock does NOT subsume

`m0008_partition_audit_log` hand-rolled an advisory lock for a *pair* of races, and
`MIGRATION_LOCK_KEY` covers only one of them. `AUDIT_PARTITION_LOCK_KEY` guards:

1. migration-vs-migration on the `audit_log` swap (`m0008_partition_audit_log.rs:13-17`) — **this**
   one becomes redundant under `MIGRATION_LOCK_KEY`, and
2. migration-vs-**running replica**: `pg_partition_maintainer.rs:10-11` takes the same key so "the
   swap and a maintenance tick never overlap". During a rolling update the *old* replicas are
   still ticking `PgPartitionMaintainer` while the new replica migrates. Those old replicas never
   take `MIGRATION_LOCK_KEY`, so this design does nothing about that race.

**A migration that does DDL on a table a background maintainer also does DDL on still needs that
maintainer's key.** Do not read this design as licence to drop a hand-rolled advisory lock from a
future migration. `MIGRATION_LOCK_KEY` serialises *migration runs against each other*, nothing
more — and §3.3 shows this is not merely theoretical: it is what makes an m0008-class migration
still require a single-replica window (§5, §6).

## 3. Design

### 3.1 Mechanism

New module `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`:

```rust
/// Namespaces a whole migration RUN against another run. Must never collide with
/// `AUDIT_PARTITION_LOCK_KEY` (5_580_467) — see §2.1 for what this key does not cover and
/// §3.3 for why the two keys' ordering is load-bearing.
pub const MIGRATION_LOCK_KEY: i64 = 5_580_559;

/// What the call actually did. Returned rather than discarded so tests can assert the wait
/// happened instead of inferring it from wall clock — see §4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationLockOutcome {
    /// Time spent waiting for the lock, excluding the migration itself.
    pub waited: Duration,
    /// Failed acquisition attempts before the successful one. `0` = uncontended.
    pub polls: u32,
    /// Migrations actually applied. `0` on a warm boot, which is the §3.2 second-runner path.
    pub migrations_applied: usize,
}

pub async fn migrate_under_lock(
    db: &DatabaseConnection,
    wait: Duration,
) -> Result<MigrationLockOutcome, MigrationLockError>;
```

**The outcome type is load-bearing, not decoration.** A `Result<(), _>` makes the wait
unobservable, and a test that measures wall clock around the call cannot distinguish "waited for
the lock" from "ran the migration" — so it would pass with the guard deleted. `PgPartitionMaintainer::tick`
returns a `MaintenanceReport` for exactly this reason (`pg_partition_maintainer.rs:36-43`).

Because `Migrator::up` accepts `&DatabaseTransaction` directly
(`sea-orm-migration/src/connection.rs:144`), the lock is **transaction-scoped** and taken on the
very transaction the migration runs in. Postgres releases it on commit *or* rollback.

The loop is **do-while**: an attempt always happens before any give-up decision. Timing is
expressed as elapsed-against-budget throughout — `start: Instant` is the only clock state, and
§3.2's `next_poll` is the single authority for both the backoff and the give-up decision. There is
deliberately no second `deadline` representation.

1. `let txn = db.begin().await?;` — a failure here aborts boot immediately rather than counting
   against the budget: it means the pool is unusable, which no amount of waiting fixes.
2. `SELECT pg_try_advisory_xact_lock(MIGRATION_LOCK_KEY)` — the `try` variant never blocks, so the
   bounded thing is *our poll loop*, not a lock wait inside Postgres. That is what gives us
   somewhere to log from (§7 restates this against the `lock_timeout` alternative).
3. `false` → **explicitly** `txn.rollback().await`, then consult `next_poll(start.elapsed(), wait)`:
   `Retry(d)` → sleep `d`, increment `polls`, loop; `GiveUp` → emit `tracing::error!` and return
   `Err(MigrationLockError::Contended { waited, key })`.
4. `true` → `Migrator::up(&txn, None).await` — on `Err`, **explicitly** `txn.rollback().await` and
   return; on `Ok`, `txn.commit().await?` and return the outcome.

`migrations_applied` is derived by counting `seaql_migrations` rows inside the locked transaction
before and after, so a warm boot reports `0` without parsing sea-orm's log output.

**The terminal give-up must `tracing::error!` before returning.** `main.rs:29` surfaces a boot
error only through a bare `eprintln!("Error: {error:?}")`, which bypasses `paigasus_logging`
entirely — without this the structured pipeline gets the throttled `info!` waiting lines and then
silence at the moment that actually matters. `MigrationLockError` must also be
`std::error::Error + Send + Sync + 'static` to travel through `serve()`'s `anyhow::Result`.

**Concrete poll parameters** — unspecified values are how two implementers produce two loops:

* Backoff: fixed **1s**, clamped by `next_poll` to the remaining budget so the wait is honoured
  exactly rather than overshot by up to one interval.
* Log throttle: at most one `tracing::info!` every **15s**, carrying elapsed and remaining.
* **No jitter.** Jitter breaks up a thundering herd; here every loser waits and then finds nothing
  to do, so staggering buys nothing. (Contrast `OutboxConfig::wake_debounce_ms`,
  `config.rs:469-470`, where the herd is real.)

**Rolling back between polls** — rather than holding one transaction open across the whole wait —
keeps the session out of `idle in transaction`. Note the interaction with §3.6: this leaves the
pooled session plainly `idle` for ~1s per poll, so the GUC that must **not** be set aggressively
on this deployment is `idle_session_timeout`; the one §3.6 recommends,
`idle_in_transaction_session_timeout`, is unaffected by the poll loop and is the correct knob.

**Deliberately no `SET LOCAL lock_timeout` on the migration transaction.** Not because none is in
play — m0008 sets `lock_timeout = '5s'` (`m0008_partition_audit_log.rs:56`) and, since `SET LOCAL`
survives the enclosing transaction once its savepoint releases, m0009 and m0010 already run under
it. The reason is that setting one *here* would bound the earlier migrations too, changing
behaviour unrelated to this issue.

**Savepoint nesting, and the limit of the `Drop` claim.** Passing `&txn` means
`exec_with_connection` calls `SchemaManagerConnection::Transaction(t).begin()`
(`sea-orm-migration/src/connection.rs:60-65`), which is `DatabaseTransaction::begin` on the same
connection — a **SAVEPOINT**, not the top-level `db.begin()` §2 quotes. Atomicity is unaffected: a
savepoint release is not durable until the outer commit, and the advisory lock is held on the
*outer* transaction, which is what makes this correct. But on a migration error that inner
transaction is dropped by sea-orm-migration, and `DatabaseTransaction::Drop` calls
`start_rollback().expect(..)` (`sea-orm/src/database/transaction.rs:234-238`), which panics if the
connection mutex is contended. So the rule is: **rollback is explicit on every path we own**;
sea-orm-migration's inner drop is a residual we do not control.

### 3.2 The poll decision is a pure function

```rust
enum Poll { Retry(Duration), GiveUp }
fn next_poll(elapsed: Duration, wait: Duration) -> Poll;
```

This is the **sole** authority for backoff and give-up; `migrate_under_lock` keeps only the
database round-trip and the `start: Instant`. Following the repo's practice of pulling a decision
out of an I/O path so it can be asserted directly — `docker.rs`'s `env_flag`, and
`PgPartitionMaintainer::tick`'s `MaintenanceReport` "so tests can assert without scraping logs".

Unit cases (Docker-free): `next_poll(0s, 1s) == Retry(1s)`; `next_poll(900ms, 1s) == Retry(100ms)`
— the clamp; `next_poll(1s, 1s) == GiveUp`; `next_poll(2s, 1s) == GiveUp`. Note the do-while
property ("always attempt once") is **not** a property of `next_poll` — it belongs to
`migrate_under_lock`'s structure and is covered by test 3's `polls`/`waited` assertions instead.

### 3.3 Lock ordering, and the live interaction

* A migrating transaction takes `MIGRATION_LOCK_KEY`, then — inside m0008 only —
  `AUDIT_PARTITION_LOCK_KEY`.
* `PgPartitionMaintainer` takes `AUDIT_PARTITION_LOCK_KEY` and **never** the migration key.

The rule for future components: **advisory keys first, in the order MIGRATION → AUDIT, then
heavyweight table locks.** No cycle exists among the advisory keys.

**The interaction that survives this design.** m0008 issues a *blocking* `pg_advisory_xact_lock`
under `SET LOCAL lock_timeout = '5s'` (`m0008_partition_audit_log.rs:56-57`), and `lock_timeout`
does apply to advisory-lock waits. So during the one-time partition upgrade, an old replica's
`PgPartitionMaintainer` — which holds `AUDIT_PARTITION_LOCK_KEY` for a tick
(`pg_partition_maintainer.rs:45`, `LOCK_TIMEOUT = "5s"`) — aborts the **entire** migration
transaction, after the new replica has already spent up to `lock_wait_secs` winning
`MIGRATION_LOCK_KEY`. This is why §5 and §6 keep a single-replica window for m0008-class
migrations rather than declaring concurrent starts universally safe. Test 6 pins it.

`migrate_under_lock` does **not** retry a failed `Migrator::up`. The wait loop retries lock
*acquisition* only. A failed migration exits the process and the orchestrator's restart backoff is
the recovery — retrying in-process would re-run unknown DDL against unobserved state and hide a
genuinely broken migration.

### 3.4 Config

A new `[migration]` section on `IamConfig`, matching the existing `[outbox]` / `[metrics]` /
`[authn]` style (there is no `[database]` section today — `database_url` is top-level):

```toml
[migration]
lock_wait_secs = 120    # IAM_MIGRATION__LOCK_WAIT_SECS
```

`MigrationConfig::lock_wait()` returns `Duration::from_secs(self.lock_wait_secs)`.

**Validated `1..=3600`.**

* **`0` is rejected, not repurposed.** Everywhere else in this config surface `0` means
  *never / unbounded* — `OutboxRetentionConfig`'s doc is explicit that one sentinel meaning across
  a block is deliberate because "two different readings of `0` inside one table would be a trap"
  (`config.rs:504-505`), and `audit.retention.{denied,committed}_months` follow it
  (`config.rs:396-400`). An operator writing `lock_wait_secs = 0` to mean "don't time out my
  migration wait" must not get a guaranteed crash on every contended rollout. A caller wanting
  fail-fast writes `1`. This matches `jwks_ttl_secs`, `poll_interval_secs`, `interval_secs` and
  `refresh_interval_secs`, all of which reject `0` in `validate()`.
* **The ceiling is operational, not arithmetic.** (`checked_add` already makes an overflow panic
  unreachable, so "prevents a panic" would not justify a bound.) 3600 is the largest wait that any
  plausible probe budget can accommodate. Because the ceiling still exceeds what the shipped image
  is configured for, boot additionally emits a `tracing::warn!` when
  `lock_wait_secs + MIGRATION_BUDGET_SECS` exceeds the image's `--start-period` (§3.5) — a value
  the code knows as a constant — so an operator raising the wait past what the container tolerates
  is told at boot rather than discovering it during a rollout.

**Shape.** `#[serde(default)]` on the field plus `impl Default` on the type, following
`RetentionConfig` (`config.rs:374`) and `OutboxRetentionConfig` (`config.rs:493`), so an absent
`[migration]` block is valid config. Derives `Debug, Clone, Deserialize, Serialize, PartialEq, Eq`
— `Eq` is not optional here, see `MetricsConfig`'s doc (`config.rs:650-652`). Plus an entry in the
`Defaults` struct.

**`#[serde(default)]` does not save the struct literals.** It governs deserialization only; a new
`pub migration: MigrationConfig` field on `IamConfig` (`config.rs:14-29`) breaks every *exhaustive*
literal. Two are confirmed exhaustive and must be edited: `tests/support/mod.rs:444` and
**`src/service_info.rs:132` — which is in `src/`, not tests**. `tests/keycloak_e2e.rs:194` is a
third candidate to check. `tests/api_key_cache_connection.rs:42` is `base.clone()` plus field
mutation and is **not** at risk. (The `RetentionConfig` precedent does not generalise here: it
nests under `AuditConfig`, and those literals write `audit: AuditConfig::default()`.)

### 3.5 Composition root, and probe budgets

`main.rs:109` becomes `migrate_under_lock(&db, config.migration.lock_wait()).await?`.

A waiter now legitimately sits for up to `lock_wait_secs` with **no listener bound** — that is the
consequence of deferring bind-first to SMA-571 (§7). Two independent probe systems see it, and
they are **not** the same system:

* **Docker `HEALTHCHECK`** (`rs/Dockerfile:70-71`) — read by `docker run`, Compose, Swarm and
  `ci/images/run.sh`. Docker Engine does not restart an unhealthy container (Swarm does), so the
  practical effect outside Swarm is a misleading `unhealthy` status. `--start-period` moves
  60s → **180s** = the 120s default wait + a **60s migration budget**, and that budget is the
  assumption the number encodes, so it is stated rather than implied.
* **Kubernetes** — the kubelet **ignores the image's `HEALTHCHECK` entirely**. The rolling-update
  shape is governed by `startupProbe`, which is why §6 carries a formula and worked numbers rather
  than prose. Note a third, unbudgeted term: `AppState::new` (`main.rs:121` →
  `adapters/http/mod.rs:396-402`) reconciles policies and loads a snapshot *after* the migration
  and still before any bind.

**The invariant is enforced, not merely documented.** `ci/images/run.sh`'s `assert_pins`
(`:46-117`) already greps `rs/Dockerfile` for cross-file agreements — `FROM rust:` versus
`rust-toolchain.toml`'s channel, `ENV RUSTUP_TOOLCHAIN` versus both, the bookworm/noble glibc
ordering, and the final stage's permitted `COPY` set. A fourth check of the same shape parses
`--start-period=(\d+)s` and asserts it is at least the `lock_wait_secs` default plus the 60s
migration budget. This costs nothing in the Moon graph — `assert_pins` is not a `repo:*` task, so
none of the `T=()` / CLAUDE.md-marker / `:affected-smoke` costs apply.

One caveat to record: `images.yml`'s `pull_request` filter does not include `rs/crates/**`, so a
PR that changes only the `config.rs` default reds `main` rather than the PR. That is the
documented posture for that workflow (CLAUDE.md), not a new gap — but it means the check catches a
Dockerfile edit on the PR and a config-default edit one merge later.

The helper's doc comment carries the intent that production code must not call `Migrator::up`
bare. **No CI single-site gate:** there is exactly one production call site, and the gateway does
not migrate. The other eight call sites are all test code — `tests/audit_log_partition_pg.rs`
(:153, :176, :202, :262, :312) and `tests/outbox_dead_letter_columns_pg.rs` (:74, :93) drive
migrations step by step, and `tests/support/mod.rs:78` is `start_migrated_postgres`, the bulk
helper ~52 binaries depend on. A gate would ship as mostly allowlist.

`Migrator::down` stays unguarded; nothing calls it in production. See §8.

### 3.6 A stranded lock

The release property holds only when Postgres *observes* the connection ending. A pod SIGKILL'd on
a partitioned node leaves its backend alive holding the advisory lock — and, if it died inside
m0008, `ACCESS EXCLUSIVE` on `audit_log` — until TCP-level timeouts fire, which by default is
hours. Every subsequent replica then waits `lock_wait_secs` and fails to boot, indefinitely.

This is the one scenario where the design converts a transient problem into a standing outage, so
§5 carries the operator-facing remedy. Two corrections that matter for it to actually work:

* The stranded backend is **`idle in transaction`**, so `idle_session_timeout` does not apply to
  it. The GUC that does is **`idle_in_transaction_session_timeout`**.
* `tcp_keepalives_idle` alone only starts probing; **`tcp_user_timeout`** is what bounds a
  partitioned peer.

`MigrationLockError::Contended` must print the key in the form an operator can match against
`pg_locks`.

### 3.7 Pooler compatibility

Choosing a *transaction-scoped* lock is what makes this design safe behind a transaction-mode
pooler such as PgBouncer: the lock is acquired and released within one transaction, so it can
never be stranded on a server connection the pooler hands to someone else. (Contrast
`OutboxConfig::listen_database_url`, `config.rs:481-487`, where `LISTEN` forces a direct or
session-mode connection.)

Two caveats for the docs: a long single migration transaction can be killed by PgBouncer's
**`idle_transaction_timeout`** (not `query_wait_timeout`, which bounds a client waiting for a
server slot, nor `server_lifetime`, which applies to a *returned* connection); and the
session-level `pg_try_advisory_lock` used by test 2 is a test-only device that would not be
pooler-safe in production code.

### 3.8 Rollout ordering

**The rollout that introduces this lock is the one rollout it does not protect.** During that
upgrade the *old* replicas still call `Migrator::up(&db, None)` bare and ignore
`MIGRATION_LOCK_KEY` entirely, so a new replica holding the lock does not stop an old replica
restarting into an unguarded migration.

Therefore the relaxation in §5/§6 applies **from the release after** the one introducing the lock.
The introducing release keeps `replicas: 1` / `maxSurge: 0`. This must appear in both the runbook
bullet and the chart-facing block, because otherwise the documentation that relaxes the safety
rule ships exactly one release too early.

## 4. Testing

`rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs`, Docker-backed through
`support::docker::start_or_skip` (`repo:iam-docker-policy-single-site` fails if a suite hand-rolls
its own skip policy), plus unit tests for §3.2 and the config surface.

**Container sourcing — the trap that would make every test vacuous.** Tests 1-4 and 6 take the
*container* from `support::start_raw_postgres` and **discard its returned handle**, building their
own connections from `support::connection_url(&node)` via `Database::connect`. Two distinct
hazards:

* `start_raw_postgres` pins its pool to `max_connections(1)` / `min_connections(1)`
  (`tests/support/mod.rs:144-153`), so reusing that handle for both migrators makes the second
  `db.begin()` block on the *pool*, not the advisory lock — the test then either serialises
  trivially or trips sqlx's acquire timeout.
* `start_migrated_postgres` (`tests/support/mod.rs:73`, the helper ~52 binaries use) has **already
  run `Migrator::up` at `:78`**. Reaching for it makes both calls no-ops with every assertion
  passing trivially. Test 1 therefore opens with a **pre-assertion that `seaql_migrations` does not
  exist**, so this failure can never be silent.

`outbox_retention_concurrency_pg.rs:65-71` is the precedent. Tests use `#[tokio::test]`'s default
current-thread runtime, which does interleave the `tokio::join!` here.

1. **Convergence — AC 1 and AC 2.** Pre-assert `seaql_migrations` absent. Two independent
   `DatabaseConnection`s; `tokio::join!` two `migrate_under_lock` calls. Both `Ok`;
   `seaql_migrations` holds exactly `Migrator::migrations().len()` rows; `audit_log` is partitioned
   (`pg_class.relkind = 'p'`) — m0008's outcome, the migration most likely to break under
   concurrency. **Exactly one side reports `migrations_applied > 0`** and the other reports `0`,
   which is what distinguishes real serialisation from both-ran-and-one-lost. The whole join runs
   inside `tokio::time::timeout`: **the timeout is the deadlock assertion**. `lock_wait_secs` is
   generous (120s) — `outbox_retention_concurrency_pg.rs:176-182` records an in-container 5s
   `lock_timeout` inflating to 21.3s of wall clock under a full-crate run.
2. **The lock is load-bearing.** An independent connection pinned to `max_connections(1)` /
   `min_connections(1)` takes the key with **`pg_try_advisory_lock`** and `assert!(acquired)` —
   not `pg_advisory_lock`, which returns `void` and cannot assert its own setup. (Session- and
   transaction-level advisory locks conflict across sessions but are re-entrant within one, which
   is why the holder must be a separate connection.) Then `migrate_under_lock(db, 1s)` returns
   `Contended` **and leaves the database unmigrated** — `seaql_migrations` absent. Release with
   `pg_advisory_unlock` and `assert!(released)`; a subsequent call succeeds.

   This bites: delete the `pg_try_advisory_xact_lock` check and `Migrator::up`'s own `install()`
   creates `seaql_migrations`, failing the assertion. The tempting alternative — two *unguarded*
   migrations, assert breakage — is not deterministic: m0008 is already individually hardened, and
   whether a duplicate-object error or a deadlock surfaces depends on timing.
3. **A waiter genuinely waits, then succeeds.** A holder takes the lock and releases it after
   ~500ms; `migrate_under_lock(db, 10s)` returns `Ok` with **`outcome.polls >= 1`** and
   **`outcome.waited >= 500ms`**. Asserting on the outcome rather than wall clock is what makes
   this non-vacuous: an advisory lock does not block DDL, so with the guard deleted a bare
   `Migrator::up` would return `Ok` after ≥500ms too and a wall-clock assertion would still pass.
4. **The lock is released when the migration itself fails.** Drive `migrate_under_lock` into a
   failing `Migrator::up` (a pre-seeded conflicting object) and assert the error propagates. Then
   assert release **from an independent session** — `SELECT pg_try_advisory_lock(5580559)` returns
   `true`. Re-calling `migrate_under_lock` would fail identically on the still-present conflicting
   object and so proves nothing about the lock. This is the claim §3.1 uses to justify
   transaction-scoped over session-scoped.
5. **Second runner is a no-op.** After a completed migration, another `migrate_under_lock` returns
   `Ok` with `migrations_applied == 0`. A regression guard, not primary evidence.
6. **The m0008 / `lock_timeout` interaction (§3.3).** Hold `AUDIT_PARTITION_LOCK_KEY` (5_580_467)
   on an independent session; run `migrate_under_lock` against a raw database; assert it fails
   within ~5s with a lock-timeout `DbErr` and that `seaql_migrations` is absent. Cheap,
   deterministic, and it is the technical basis for the single-replica caveat §5 and §6 must carry
   — an untested claim there would be prose.

**Unit, no Docker:** §3.2's four `next_poll` cases; and the config surface, matching this file's
own convention (`config.rs:2593`, `:2049`, `:2103`, `:2886`) — reject `0`, reject `3601`, accept
`1` and `3600`, an absent `[migration]` block defaults to 120, and `IAM_MIGRATION__LOCK_WAIT_SECS`
reaches the field through `Env::prefixed("IAM_").split("__")` (`config.rs:955-957`).

## 5. Documentation

`docs/ops/RUNBOOK-containers.md`:

* **§5 bullet — two clauses, not one.** Concurrent starts are serialised *against each other* by
  an advisory lock, so a rolling update no longer risks a duplicate-object boot failure. **But** a
  migration doing DDL on a table a background maintainer also touches — m0008-class — still
  requires `replicas: 1` / `maxSurge: 0`, because an old replica's `PgPartitionMaintainer` holding
  `AUDIT_PARTITION_LOCK_KEY` past m0008's 5s `lock_timeout` aborts the whole migration (§3.3).
  A single-clause "it's just a recommendation now" would remove the operator's only protection
  against precisely the migration class that motivated m0008.
* **Rollout ordering (§3.8)** — the relaxation applies from the release *after* the one
  introducing the lock; the introducing release keeps `maxSurge: 0`.
* **The cost a long migration still carries.** The whole run is one transaction (§2), so
  m0008-class DDL holds `ACCESS EXCLUSIVE` on `audit_log` for its entire duration — **every
  running replica's audit writes block for that window.** An operator sizing `lock_wait_secs` for
  a big table is simultaneously sizing an audit-write stall. A large `audit_log` still warrants a
  maintenance window.
* **Stranded-lock recovery (§3.6)** — scoped to the current database, since §8 permits separate
  databases on one cluster:

  ```sql
  SELECT pid, granted, query_start
  FROM pg_locks l JOIN pg_stat_activity a USING (pid)
  WHERE l.locktype = 'advisory'
    AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
    AND ((l.classid::bigint << 32) | l.objid::bigint) = 5580559;
  ```

  The parentheses are load-bearing — Postgres gives `<<` and `|` equal precedence. `objsubid` is
  deliberately omitted: it is well-defined (`1` for the `bigint` form, `2` for two `int4`s), so
  adding it would narrow rather than harden, and the key arithmetic already identifies the lock.
  Remedy: `pg_terminate_backend(pid)`. **State the required role**: `query_start` is NULL for other
  users' backends without `pg_read_all_stats`, and `pg_terminate_backend` needs `pg_signal_backend`
  or superuser — the IAM application role typically has neither. Also recommend
  `idle_in_transaction_session_timeout` and `tcp_user_timeout` (§3.6), **not**
  `idle_session_timeout`, which the poll loop would trip (§3.1) and which does not apply to the
  stranded backend anyway.
* **Probe table, line 92** — a waiting replica's startup is the leader's migration time **plus**
  its own wait plus `AppState::new`, so `startupProbe.failureThreshold` matters more, not less.
* **Docker vs Kubernetes (§3.5)** — the image's `HEALTHCHECK` governs Compose/Swarm/`docker run`
  and `ci/images/run.sh`; the kubelet ignores it. Plus the pooler caveats from §3.7.

## 6. AC 4 — SMA-513

AC 4 asks that the choice be reflected in SMA-513's chart defaults. That cannot be satisfied here:
SMA-513 is in Backlog and there is no Helm chart anywhere in the repo (`**/Chart.yaml` → no
matches). **AC 4 is deferred with a written handoff, not satisfied** — and the handoff is a repo
artifact, because a Linear comment is invisible from a checkout.

`docs/ops/RUNBOOK-containers.md` gains a copyable chart-facing block with a formula and worked
numbers, not prose:

> `startupProbe.failureThreshold × periodSeconds  >  lock_wait_secs + migration budget + AppState::new`
>
> At the shipped default (`lock_wait_secs = 120`, 60s migration budget):
> `periodSeconds: 10`, `failureThreshold: 30` (= 300s).
>
> `strategy.rollingUpdate.maxSurge` is no longer forced to `0` — **except** on the release that
> introduces the lock (§3.8), and **except** for an m0008-class migration (§5).

A comment on SMA-513 points at it.

## 7. Rejected

* **Binding `/healthz` before migrating**, serving `/readyz` as not-ready-while-migrating. This is
  the standard pattern; it would dissolve the probe-budget coupling in §3.5 and additionally fix
  the *single*-replica slow-migration case, a real bug today with no concurrency involved.
  **Deferred to [SMA-571](https://linear.app/smaschek/issue/SMA-571/), not dismissed.**

  It is not a reordering. `AppState` provably cannot be built before the migration:
  `adapters/http/mod.rs:396` reconciles system policies **into** Postgres via
  `bootstrap::reconcile_starter`, and `:398-401` **reads** the policy store through
  `PolicySnapshot::new` → `load_and_compile`; both hit m0004's tables and fail with "relation does
  not exist" on a fresh database. So the listener must be *serving* before the state exists, which
  rules out the cheap variants — binding early and calling `axum::serve` later leaves connections
  accepted-but-unanswered (the probe times out; worse than connection-refused), and handing the
  listener between two `axum::serve` calls opens a port-free window. The viable mechanism is a
  deferred-router slot (`health_router()` is already stateless, `mod.rs:810`, so it can be served
  with a `fallback_service` over an `Arc<ArcSwapOption<Router>>`) — a novel request-path mechanism
  for this repo, earning its own spec and tests.

  **Honest limitation:** the port-free-window and accept-backlog arguments above are reasoned from
  the API contracts, **not measured**. SMA-571 should measure before committing to the mechanism.
* **A boot metric.** A once-per-process event does not earn a metric family plus the
  `:observability-drift` surface. The throttled `tracing` lines carry it.
* **`SET lock_timeout` + blocking `pg_advisory_lock`.** Not rejected for uncertainty —
  `lock_timeout` *does* apply to advisory waits and m0008 already relies on it. Rejected because it
  produces no "still waiting" log line, the single thing that makes a stuck rollout legible, and
  because it puts the deadline in Postgres' hands rather than ours.
* **A `paigasus-iam migrate` subcommand (issue Option 2).** No consumer: there is no chart to run
  it as a Job. `health::dispatch` also lives in the shared `paigasus-observability` crate, which
  the gateway uses and which has no migrations.
* **A `repo:migrator-single-site` Moon gate.** See §3.5. Note this rejection is specifically about
  a `repo:*` task; the `start_period` invariant *is* enforced, in `assert_pins`, at no graph cost.
* **Blocking indefinitely, or failing fast on first contention.** The former hangs opaquely outside
  Kubernetes; the latter turns every rolling update into a guaranteed crash-and-retry.
* **Retrying a failed `Migrator::up`.** See §3.3.

## 8. Out of scope, and known asymmetries

* **On `Contended` the process exits** via `serve()`'s `Err` path → `ExitCode::FAILURE` (1), and
  Kubernetes applies CrashLoopBackOff, up to 5 minutes between restarts. For an N-replica
  scale-out, convergence is the leader's migration plus each waiter's poll — waiters after the
  first apply nothing (§3.2), so the tail is poll-bound, not migration-bound.
* **`Migrator::down` stays unguarded.** Nothing calls it in production. `tests/audit_log_partition_pg.rs:215`
  drives it against a shared container, which is single-session in practice; a future
  operator-facing down-migration path would need the same guard, and "exactly one production call
  site" would stop being the reason to skip a gate.
* **Multiple IAM deployments sharing one database** (separate schemas via `search_path`) would
  serialise their migrations against each other — advisory locks are per-database, not per-schema.
  Not a supported topology. Separate databases on one cluster are unaffected, which is why §5's
  recovery query filters on `l.database`.
* **The knob is documented only in `RUNBOOK-containers.md`.** `RUNBOOK-observability.md` carries
  `IAM_AUDIT__RETENTION__*` and `IAM_OUTBOX__RETENTION__*` because those knobs have metrics and
  alerts; this one is boot-only with no metric (§7), so containers is the right and only home.
