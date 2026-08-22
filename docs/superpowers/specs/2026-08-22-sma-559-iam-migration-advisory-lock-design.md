# SMA-559 — Guard `Migrator::up` with a Postgres advisory lock

**Issue:** [SMA-559](https://linear.app/smaschek/issue/SMA-559/) · **Status:** design approved, revised after adversarial review
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
more.

## 3. Design

### 3.1 Mechanism

New module `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`:

```rust
/// Namespaces a whole migration RUN against another run. Must never collide with
/// `AUDIT_PARTITION_LOCK_KEY` (5_580_467) — see §2.1 for what this key does not cover and
/// §3.3 for why the two keys' ordering is load-bearing.
pub const MIGRATION_LOCK_KEY: i64 = 5_580_559;

pub async fn migrate_under_lock(
    db: &DatabaseConnection,
    wait: Duration,
) -> Result<(), MigrationLockError>;
```

Because `Migrator::up` accepts `&DatabaseTransaction` directly
(`sea-orm-migration/src/connection.rs:144`), the lock is **transaction-scoped** and taken on the
very transaction the migration runs in. Postgres releases it on commit *or* rollback, so no unlock
path exists to be missed — on any path where the backend actually observes the transaction ending.
(§3.6 covers the case where it does not.)

The loop is **do-while**: the deadline is checked *after* a failed attempt, never before the first
one. With `deadline = Instant::checked_add(Instant::now(), wait)`:

1. `let txn = db.begin().await?;`
2. `SELECT pg_try_advisory_xact_lock(MIGRATION_LOCK_KEY)` — the `try` variant never blocks, so the
   bounded thing is *our poll loop*, not a lock wait inside Postgres. That is what makes the
   deadline ours to control and gives us somewhere to log from (§7 restates this against the
   `lock_timeout` alternative).
3. `false` → **explicitly** `txn.rollback().await`, then back off and retry. Past the deadline →
   `Err(MigrationLockError::Contended { waited, key })`.
4. `true` → `Migrator::up(&txn, None).await` — on `Err`, **explicitly** `txn.rollback().await` and
   return; on `Ok`, `txn.commit().await?`.

Rolling back between polls (rather than holding one transaction open across the whole wait) keeps
the session out of `idle in transaction`, so a deployment with
`idle_in_transaction_session_timeout` set cannot have its waiter killed mid-wait.

**Rollback is explicit on every error path, never left to `Drop`.** `DatabaseTransaction::Drop`
calls `start_rollback().expect(..)` (`sea-orm/src/database/transaction.rs:234-238`), which
*panics* if the connection mutex is contended — turning a migration error into a panic in the
composition root.

**Concrete poll parameters** (unspecified values are how two implementers produce two different
loops):

* Backoff: fixed **1s**, with the final sleep clamped to `min(1s, deadline - now)` so the deadline
  is honoured exactly rather than overshot by up to one interval.
* Log throttle: at most one `tracing::info!` every **15s**, carrying elapsed and remaining.
* **No jitter.** Jitter exists to break up a thundering herd; here every loser simply waits and
  then finds nothing to do, so staggering them buys nothing. (Contrast `OutboxConfig`'s
  `wake_debounce_ms`, `config.rs:469-470`, where the herd is real.)

**Deliberately no `SET LOCAL lock_timeout` on the migration transaction.** That would newly bound
every existing migration's own table-lock waits, changing behaviour unrelated to this issue.

### 3.2 The poll decision is a pure function

The deadline arithmetic is extracted so it is testable without Docker:

```rust
enum Poll { Retry(Duration), GiveUp }
fn next_poll(elapsed: Duration, wait: Duration) -> Poll;
```

`migrate_under_lock` keeps only the database round-trip. This follows the repo's established
practice of pulling a decision out of an I/O path so it can be asserted directly — `docker.rs`'s
`env_flag`, and `PgPartitionMaintainer::tick` returning a `MaintenanceReport` "so tests can assert
without scraping logs". A named design decision with no coverage regresses silently.

### 3.3 Lock ordering

The crate now has two advisory keys, so the ordering is stated rather than assumed:

* A migrating transaction takes `MIGRATION_LOCK_KEY`, then — inside m0008 only —
  `AUDIT_PARTITION_LOCK_KEY`.
* `PgPartitionMaintainer` takes `AUDIT_PARTITION_LOCK_KEY` and **never** the migration key.

The rule, stated for future components: **advisory keys first, in the order MIGRATION → AUDIT,
then heavyweight table locks.** Every acquirer that holds both takes them in that order and the
maintainer holds only the second, so no cycle exists among the advisory keys.

**The known live interaction.** m0008 issues a *blocking* `pg_advisory_xact_lock` under
`SET LOCAL lock_timeout = '5s'` (`m0008_partition_audit_log.rs:56-57`), and `lock_timeout` does
apply to advisory-lock waits. So during the one-time partition upgrade, an old replica's
`PgPartitionMaintainer` holding `AUDIT_PARTITION_LOCK_KEY` for more than 5s aborts the **entire**
migration transaction — after the new replica has already spent up to `lock_wait_secs` winning
`MIGRATION_LOCK_KEY`.

`migrate_under_lock` does **not** retry a failed `Migrator::up`. The wait loop retries lock
*acquisition* only. A failed migration exits the process, and the orchestrator's restart backoff
is the recovery — retrying in-process would re-run an unknown amount of DDL against a database
whose state we did not observe, and would hide a genuinely broken migration behind a retry.

### 3.4 Config

A new `[migration]` section on `IamConfig`, matching the existing `[outbox]` / `[metrics]` /
`[authn]` style (there is no `[database]` section today — `database_url` is top-level):

```toml
[migration]
lock_wait_secs = 120    # IAM_MIGRATION__LOCK_WAIT_SECS
```

**Validated `1..=3600`.** Two reasons, both load-bearing:

* **`0` is rejected, not repurposed.** Everywhere else in this config surface `0` means
  *never / unbounded* — `OutboxRetentionConfig`'s doc is explicit that one sentinel meaning across
  a block is deliberate because "two different readings of `0` inside one table would be a trap"
  (`config.rs:504-505`), and `audit.retention.{denied,committed}_months` follow it
  (`config.rs:396-400`). An operator writing `lock_wait_secs = 0` to mean "don't time out my
  migration wait" must not get a guaranteed crash on every contended rollout. There is no
  fail-fast-via-zero mode; a caller wanting one writes `1`.
* **An upper bound stops a panic.** `Instant + Duration` panics on overflow, so an unvalidated
  `IAM_MIGRATION__LOCK_WAIT_SECS` near `u64::MAX` would panic at boot with a bare "overflow when
  adding duration to instant" rather than a config error. The implementation uses
  `checked_add` regardless.

This matches the crate's existing posture — `jwks_ttl_secs`, `poll_interval_secs`,
`interval_secs` and `refresh_interval_secs` all reject `0` in `validate()`.

**Shape.** `#[serde(default)]` on the field plus `impl Default` on the type, following
`RetentionConfig` (`config.rs:374`) and `OutboxRetentionConfig` (`config.rs:493`) — so an absent
`[migration]` block is valid config and the four hand-built `IamConfig` literals
(`tests/support/mod.rs:444`, `tests/keycloak_e2e.rs`, `tests/api_key_cache_connection.rs`,
`src/service_info.rs`) keep compiling. Derives `Debug, Clone, Deserialize, Serialize, PartialEq,
Eq` — `Eq` is not optional here, see `MetricsConfig`'s doc (`config.rs:650-652`). Plus an entry in
the `Defaults` struct.

### 3.5 Composition root and the container start period

`main.rs:109` becomes `migrate_under_lock(&db, config.migration.lock_wait()).await?`.

**`rs/Dockerfile` must change with it.** It ships
`HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=3` with the comment
"60s start period because IAM runs `Migrator::up` before it binds" (`rs/Dockerfile:70-71`). A
waiter now legitimately sits for `lock_wait_secs` with **no listener bound**, so a 60s start period
marks a correctly-waiting container unhealthy at ~150s and restarts it — making the fix
self-defeating in precisely the rolling-update shape it exists for. The smoke suite cannot catch
this: `ci/images/run.sh` starts a single IAM container, which is never contended.

`--start-period` moves to **180s**, and its comment must name the invariant:

> **`start_period` must exceed `lock_wait_secs` plus the expected migration time.** Raising
> `lock_wait_secs` without raising this re-arms the restart-while-waiting bug.

Raising the start period costs a healthy container nothing — Docker marks a container healthy on
the first successful probe regardless of the start period, which only suppresses *unhealthy*
marking — so `ci/images/run.sh`'s 60s `wait_healthy` budget is unaffected.

The helper's doc comment carries the intent that production code must not call `Migrator::up`
bare. **No CI single-site gate:** there is exactly one production call site, and the gateway does
not migrate. The other eight call sites are all test code — `tests/audit_log_partition_pg.rs`
(:153, :176, :202, :262, :312) and `tests/outbox_dead_letter_columns_pg.rs` (:74, :93) drive
migrations step by step, and `tests/support/mod.rs:78` is `start_migrated_postgres`, the bulk
helper ~52 binaries depend on. A gate would ship as mostly allowlist, at the cost of a `ci.yml`
`T=()` entry, the CLAUDE.md marker block, and an `:affected-smoke` re-baseline.

`Migrator::down` stays unguarded. Nothing calls it in production, so the risk is low today — but
the asymmetry is deliberate and recorded here: a future operator-facing down-migration path would
need the same guard, and "exactly one production call site" would no longer be the reason to skip
a gate.

### 3.6 A stranded lock

The "no unlock path can be missed" property holds only when Postgres *observes* the connection
ending. A pod SIGKILL'd on a partitioned node leaves its backend alive, holding the advisory lock
— and, if it died inside m0008, `ACCESS EXCLUSIVE` on `audit_log` — until TCP keepalives or
`tcp_user_timeout` fire, which by Postgres default is hours. Every subsequent replica then waits
`lock_wait_secs` and fails to boot, indefinitely.

This is the one scenario where the design converts a transient problem into a standing outage, so
it is documented rather than hidden: §5 carries the `pg_locks` query, the `pg_terminate_backend`
remedy, and the server settings that bound it. `MigrationLockError::Contended` must print the key
in the form an operator can actually match against `pg_locks`.

### 3.7 Pooler compatibility

Choosing a *transaction-scoped* lock is what makes this design safe behind a transaction-mode
pooler such as PgBouncer: the lock is acquired and released within one transaction, so it can
never be stranded on a server connection that the pooler hands to someone else. (Contrast
`OutboxConfig::listen_database_url`, `config.rs:481-487`, where `LISTEN` forces a direct or
session-mode connection.)

Two caveats belong in the docs: a long single migration transaction can exceed PgBouncer's
`query_wait_timeout` / `server_lifetime`; and the *session*-level `pg_advisory_lock` used by test
2 below is a test-only device that would not be pooler-safe in production code.

## 4. Testing

`rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs`, Docker-backed through
`support::docker::start_or_skip` (the single-site skip-versus-panic policy —
`repo:iam-docker-policy-single-site` fails if a suite hand-rolls its own), plus unit tests for
§3.2's pure function.

**A trap to state up front.** `support::start_raw_postgres` pins its pool to
`opts.max_connections(1).min_connections(1)` (`tests/support/mod.rs:144-153`). Reusing that one
handle for both migrators — the obvious reading of "one Postgres container" — makes the second
`db.begin()` block on the *pool*, not on the advisory lock: the test then either serialises
trivially (proving nothing) or trips sqlx's acquire timeout (flaky red). Every test below builds
its connections from `support::connection_url(&node)` via `Database::connect`, never from
`start_raw_postgres`'s returned handle. `outbox_retention_concurrency_pg.rs:65-71` is the
precedent.

1. **Convergence — AC 1 and AC 2.** Two independent `DatabaseConnection`s to one container;
   `tokio::join!` two `migrate_under_lock` calls. Assert both return `Ok`, and that
   `seaql_migrations` holds exactly `Migrator::migrations().len()` rows. The concrete schema
   assertion is that `audit_log` is *partitioned* (`pg_class.relkind = 'p'`) — m0008's outcome,
   i.e. the migration most likely to break under concurrency — rather than a vague "schema probe".
   The whole join runs inside `tokio::time::timeout`: **the timeout is the deadlock assertion**,
   the technique `outbox_retention_concurrency_pg.rs` uses. `lock_wait_secs` for this test is
   generous (120s), because that file records an in-container 5s `lock_timeout` inflating to 21.3s
   of wall clock under a full-crate run (:176-182) — a tight wait would be flaky in exactly the
   environment that must stay green.
2. **The lock is load-bearing.** An independent connection pinned to `max_connections(1)` /
   `min_connections(1)` takes the key with **`pg_try_advisory_lock`** and `assert!(acquired)` —
   not `pg_advisory_lock`, which returns `void` and so cannot assert its own setup. (Session-level
   and transaction-level advisory locks share one lock space across sessions, so they conflict;
   within one session they would be re-entrant, which is why the holder must be a separate
   connection.) Then `migrate_under_lock(db, 1s)` must return `Contended` **and leave the database
   unmigrated** — `seaql_migrations` absent. Release with `pg_advisory_unlock` and
   `assert!(released)`; a subsequent call then succeeds.

   This is the deterministic form of "prove the guard does something", and it bites: delete the
   `pg_try_advisory_xact_lock` check and `Migrator::up`'s own `install()` creates
   `seaql_migrations`, failing the assertion. The tempting alternative — run two *unguarded*
   migrations and assert breakage — is not deterministic: m0008 is already individually hardened,
   and whether a duplicate-object error or a deadlock surfaces depends on timing.
3. **A waiter genuinely waits, then succeeds.** A holder takes the lock and releases it after
   ~500ms; `migrate_under_lock(db, 10s)` must return `Ok` having waited a measurable, non-zero
   time. Test 2 only proves the give-up path — this is §3.2's wait-then-acquire behaviour, which
   is what AC 1 actually asks for.
4. **The lock is released when the migration itself fails.** Drive `migrate_under_lock` into a
   failing `Migrator::up` (a pre-seeded conflicting object), assert the error propagates, then
   assert a second call can still acquire the lock. This is the claim §3.1 uses to justify
   transaction-scoped over session-scoped; untested, it is an assumption.
5. **Second runner is a no-op.** After a completed migration, another `migrate_under_lock` returns
   `Ok` and leaves the migration count unchanged.

**Unit (no Docker):** `next_poll` — that `wait == 0`-adjacent minimums still attempt once, that the
final sleep is clamped to the remaining budget rather than overshooting, and that an elapsed time
past the deadline yields `GiveUp`.

## 5. Documentation

`docs/ops/RUNBOOK-containers.md`:

* **§5 bullet** — replace the "no advisory lock … migrate with a single replica" text with: IAM
  serialises boot migrations with a Postgres advisory lock, so concurrent starts are safe by
  construction; single-replica migration becomes a **recommendation** for a long migration, not a
  requirement.
* **The cost a long migration still carries.** §2 establishes the whole run is one transaction,
  which means m0008-class DDL holds `ACCESS EXCLUSIVE` on `audit_log` for its entire duration —
  **every running replica's audit writes block for that window.** An operator sizing
  `lock_wait_secs` for a big table is simultaneously sizing an audit-write stall, and must be told
  so. A large `audit_log` still warrants a maintenance window; that is the honest version of
  "single-replica migration is now a recommendation".
* **Stranded-lock recovery (§3.6)** — the diagnostic query, written so it does not depend on
  `pg_locks`' `objsubid` encoding:

  ```sql
  SELECT pid, granted, query_start
  FROM pg_locks l JOIN pg_stat_activity a USING (pid)
  WHERE l.locktype = 'advisory'
    AND ((l.classid::bigint << 32) | l.objid::bigint) = 5580559;
  ```

  plus `pg_terminate_backend(pid)` as the remedy, and a recommendation to set
  `tcp_keepalives_idle` / `idle_session_timeout` so a partitioned node's backend is reaped without
  manual intervention. The exact query is to be **verified against a live container** during
  implementation, not trusted from the doc.
* **Probe table, line 92** — a *waiting* replica's startup is the leader's migration time **plus**
  its own wait, so a generous `startupProbe.failureThreshold` matters more after this change, not
  less.
* **`rs/Dockerfile`'s `start_period` invariant** from §3.5, and the pooler caveats from §3.7.

## 6. AC 4 — SMA-513

AC 4 asks that the choice be reflected in SMA-513's chart defaults. That cannot be satisfied here:
SMA-513 is in Backlog and there is no Helm chart anywhere in the repo (`**/Chart.yaml` → no
matches). **AC 4 is therefore deferred with a written handoff, not satisfied** — and the handoff
must be a repo artifact, because a Linear comment is invisible to whoever writes the chart from a
checkout.

`docs/ops/RUNBOOK-containers.md` gains a copyable chart-facing block naming the concrete values:
`strategy.rollingUpdate.maxSurge` (no longer forced to `0`), `startupProbe.periodSeconds` and
`failureThreshold` derived from `lock_wait_secs`, `IAM_MIGRATION__LOCK_WAIT_SECS` itself, and the
`start_period` relationship from §3.5. A comment on SMA-513 points at it.

## 7. Rejected

* **Binding `/healthz` before migrating**, serving `/readyz` as not-ready-while-migrating, so a
  migrating replica is externally visible and no start-period tuning is needed. This is the
  standard pattern and would dissolve both the Dockerfile conflict (§3.5) and the
  `startupProbe.failureThreshold` sizing problem. Rejected for this issue because `main.rs:135-141`
  encodes a deliberate "fail with nothing bound" principle — an early return after a listener is
  live aborts in-flight requests instead of never having accepted one — so reversing it is a
  restructuring of the composition root, not a lock. Worth its own issue; flagged rather than
  silently dropped.
* **A boot metric.** A once-per-process event does not earn a metric family plus the
  `:observability-drift` surface it would add. The throttled `tracing` lines carry it.
* **`SET lock_timeout` + blocking `pg_advisory_lock`.** Not rejected for uncertainty —
  `lock_timeout` *does* apply to advisory-lock waits, and m0008 already relies on that
  (`m0008_partition_audit_log.rs:56-57`). Rejected because it produces no "still waiting" log
  line, which is the single thing that makes a stuck rollout legible, and because it puts the
  deadline in Postgres' hands rather than ours.
* **A `paigasus-iam migrate` subcommand (issue Option 2).** No consumer: there is no chart to run
  it as a Job. `health::dispatch` also lives in the shared `paigasus-observability` crate, which
  the gateway uses and which has no migrations, so a migrate mode there would be dead for one of
  its two consumers. Deferred to its own issue when a chart wants it.
* **A `repo:migrator-single-site` CI gate.** See §3.5.
* **Blocking indefinitely, or failing fast on first contention.** Rejected in favour of the
  bounded, configurable wait — the former hangs opaquely outside Kubernetes, the latter turns
  every rolling update into a guaranteed crash-and-retry.
* **Retrying a failed `Migrator::up`.** See §3.3.

## 8. Out of scope

* **Multiple IAM deployments sharing one database** (separate schemas via `search_path`) would
  serialise their migrations against each other, since advisory locks are per-database, not
  per-schema. Not a supported topology; noted so the constraint is not discovered by surprise.
  Separate databases on one cluster are unaffected.
