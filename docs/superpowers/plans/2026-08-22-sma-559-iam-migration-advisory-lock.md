# IAM Migration Advisory Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serialise `paigasus-iam`'s boot migration with a transaction-scoped Postgres advisory lock, so two replicas starting concurrently converge instead of deadlocking or crash-looping on a duplicate-object error.

**Architecture:** A new `adapters::persistence::migration_lock` module opens a transaction, takes `pg_try_advisory_xact_lock(5_580_559)` on it, and hands that same transaction to `Migrator::up` — which on Postgres already runs the whole migration set in one transaction. Acquisition is a bounded do-while poll loop whose timing decision lives in a pure `next_poll` function. A new `[migration]` config section carries the wait budget.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), sea-orm 1 / sea-orm-migration 1.1.20, figment config, tokio, testcontainers + `cargo nextest`, bash for the CI image pin check.

**Spec:** `docs/superpowers/specs/2026-08-22-sma-559-iam-migration-advisory-lock-design.md` — read it alongside this plan; every task argues from a numbered section of it.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- `[workspace.lints.rust] warnings = "deny"` — **dead code is a hard compile error on the lib target.** Every item added in Task 1 must be `pub` in a publicly-reachable module (`pub mod adapters` → `pub mod persistence` → `pub mod migration_lock`), or the crate will not build until its caller lands in Task 3.
- Conventional commits with a workspace scope, e.g. `feat(rs): …`, and the `(SMA-559)` suffix.
- Commit subjects start **lowercase** and are ≤100 chars. Never write `#NNN` in a commit body — it breaks commitlint's `footer-leading-blank`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`cargo nextest` resolve to the repo-pinned versions.
- Rust commands run from `rs/`. Formatting is `cargo fmt`; lints must pass `cargo clippy --workspace --all-targets -- -D warnings`.
- Docker-backed tests get their skip-vs-panic policy from `tests/support/docker.rs::start_or_skip` **only** — hand-rolling one reds `repo:iam-docker-policy-single-site`.
- Fixed constants, copied verbatim: `MIGRATION_LOCK_KEY = 5_580_559`; `AUDIT_PARTITION_LOCK_KEY = 5_580_467` (existing, do not change); default `lock_wait_secs = 120`; validated range `1..=3600`; `POLL_BACKOFF = 1s`; `LOG_THROTTLE = 15s`; `IMAGE_START_PERIOD_SECS = 180`; `MIGRATION_BUDGET_SECS = 60`.

---

### Task 1: The pure poll decision

Spec §3.2. This lands first and alone because it is the single authority for backoff and give-up, and it is the only part testable without Docker.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs` (add `pub mod migration_lock;` to the module list at :5-26, alphabetically after `migration`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum Poll { Retry(Duration), GiveUp }`, `pub fn next_poll(elapsed: Duration, wait: Duration) -> Poll`, `pub const POLL_BACKOFF: Duration`, `pub const LOG_THROTTLE: Duration`, `pub const MIGRATION_LOCK_KEY: i64`, `pub const IMAGE_START_PERIOD_SECS: u64`, `pub const MIGRATION_BUDGET_SECS: u64`.

- [ ] **Step 1: Create the module with the constants, the enum, and a stub**

Create `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Serialises `Migrator::up` across concurrently starting replicas (SMA-559).
//!
//! `sea-orm-migration` does not serialise concurrent `up()`. On Postgres it runs the whole
//! migration set in ONE transaction (`exec_with_connection`, `migrator.rs:261-273`), so a
//! half-applied migration is already impossible — but two migrating transactions can still
//! deadlock, and the loser can fail its boot on a duplicate-object error. This module takes a
//! transaction-scoped advisory lock on the very transaction `Migrator::up` runs in, so Postgres
//! releases it on commit or rollback and no unlock path exists to be missed.
//!
//! **This key serialises migration runs against each other and NOTHING ELSE.** A migration doing
//! DDL on a table a background maintainer also touches still needs that maintainer's own key —
//! see `AUDIT_PARTITION_LOCK_KEY` and spec §2.1. Do not read this module as licence to drop a
//! hand-rolled advisory lock from a future migration.

use std::time::Duration;

/// Namespaces a whole migration RUN against another run. Must never collide with
/// `AUDIT_PARTITION_LOCK_KEY` (5_580_467).
pub const MIGRATION_LOCK_KEY: i64 = 5_580_559;

/// Fixed backoff between acquisition attempts. No jitter: every loser simply waits and then
/// finds nothing to do, so staggering them buys nothing (contrast `OutboxConfig::wake_debounce_ms`,
/// where the herd is real).
pub const POLL_BACKOFF: Duration = Duration::from_secs(1);

/// At most one "still waiting" line per this interval.
pub const LOG_THROTTLE: Duration = Duration::from_secs(15);

/// The image's `HEALTHCHECK --start-period`, mirrored here so boot can warn when the configured
/// wait exceeds what the container tolerates. `ci/images/run.sh`'s `assert_pins` asserts this
/// constant and `rs/Dockerfile` agree.
pub const IMAGE_START_PERIOD_SECS: u64 = 180;

/// Budget for the migration itself, on top of the lock wait. `IMAGE_START_PERIOD_SECS` is
/// `lock_wait_secs` default + this.
pub const MIGRATION_BUDGET_SECS: u64 = 60;

/// What to do after a failed acquisition attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Sleep this long, then try again. Already clamped to the remaining budget.
    Retry(Duration),
    /// The budget is spent.
    GiveUp,
}

/// The sole authority for backoff and give-up.
///
/// Extracted from the async loop so the timing decision is testable without Docker — the same
/// discipline `docker::env_flag` and `PgPartitionMaintainer::tick`'s `MaintenanceReport` follow.
///
/// Note the do-while property ("always attempt at least once") is NOT expressed here: it belongs
/// to `migrate_under_lock`'s loop structure, because this function is only ever consulted AFTER
/// an attempt has already failed.
pub fn next_poll(elapsed: Duration, wait: Duration) -> Poll {
    let remaining = wait.saturating_sub(elapsed);
    if remaining.is_zero() {
        return Poll::GiveUp;
    }
    // Clamped, so the wait is honoured exactly rather than overshot by up to one interval.
    Poll::Retry(POLL_BACKOFF.min(remaining))
}
```

Add to `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`, after the `pub mod migration;` line:

```rust
pub mod migration_lock;
```

- [ ] **Step 2: Write the failing tests**

Append to `migration_lock.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_wait_retries_at_the_full_backoff() {
        assert_eq!(next_poll(Duration::ZERO, Duration::from_secs(1)), Poll::Retry(Duration::from_secs(1)));
    }

    /// The clamp: without it the loop overshoots the budget by up to one backoff interval, and
    /// the "deadline is honoured exactly" claim in the module doc is false.
    #[test]
    fn a_partial_budget_clamps_the_backoff_to_what_remains() {
        assert_eq!(next_poll(Duration::from_millis(900), Duration::from_secs(1)), Poll::Retry(Duration::from_millis(100)));
    }

    #[test]
    fn an_exactly_spent_budget_gives_up() {
        assert_eq!(next_poll(Duration::from_secs(1), Duration::from_secs(1)), Poll::GiveUp);
    }

    /// `saturating_sub`, not `checked_sub` + unwrap: an overshoot past the budget is normal (the
    /// sleep and the round-trip both take time) and must not panic.
    #[test]
    fn an_overshot_budget_gives_up_rather_than_panicking() {
        assert_eq!(next_poll(Duration::from_secs(2), Duration::from_secs(1)), Poll::GiveUp);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib migration_lock --no-tests=pass
```

Expected: FAIL to compile — `mod.rs` does not yet declare `migration_lock`, or the module body is incomplete. Fix forward until the four tests compile and run.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib migration_lock --no-tests=pass
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Verify the lints and formatting**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. If clippy reports `dead_code` on any item here, that item is not `pub` — fix the visibility, do not add an `#[allow]`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs
git commit -m "feat(rs): add the migration-lock poll decision as a pure function (SMA-559)"
```

---

### Task 2: The `[migration]` config section

Spec §3.4. Lands before Task 3 because `main.rs` will read `config.migration.lock_wait()`.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — the `IamConfig` struct (:14-29), `validate()` (:991), the `Defaults` struct (:668-679) and its `Default` impl (:769-783), plus a new `MigrationConfig` type and its tests
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:444` (exhaustive `IamConfig` literal)
- Modify: `rs/crates/services/paigasus-iam/src/service_info.rs:132` (exhaustive `IamConfig` literal — **this one is in `src/`, not tests**)
- Check: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:194` (a third candidate literal; add the field if the compiler demands it)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub struct MigrationConfig { pub lock_wait_secs: u64 }` with `Default` (120) and `pub fn lock_wait(&self) -> Duration`; `IamConfig.migration: MigrationConfig`.

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s existing `#[cfg(test)] mod tests`, following the shape of `validate_rejects_zero_retention_interval` (:2592):

```rust
    #[test]
    fn migration_lock_wait_defaults_to_120_when_the_block_is_absent() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"\n", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.migration.lock_wait_secs, 120);
            assert_eq!(cfg.migration.lock_wait(), std::time::Duration::from_secs(120));
            Ok(())
        });
    }

    /// `0` is REJECTED, not repurposed. Everywhere else in this file `0` means never/unbounded
    /// (see `OutboxRetentionConfig`'s doc), so an operator writing `0` here to mean "don't time
    /// out my migration wait" must not silently get a guaranteed crash on every contended
    /// rollout.
    #[test]
    fn validate_rejects_zero_migration_lock_wait() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!("{}\n[api_keys]\npepper = \"{}\"\n[migration]\nlock_wait_secs = 0", minimal_issuer_toml(), valid_pepper_b64()),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            let err = cfg.validate().expect_err("0 must be rejected");
            assert!(err.contains("migration.lock_wait_secs"), "unexpected error: {err}");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_migration_lock_wait_above_the_ceiling() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "iam.toml",
                &format!("{}\n[api_keys]\npepper = \"{}\"\n[migration]\nlock_wait_secs = 3601", minimal_issuer_toml(), valid_pepper_b64()),
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "3601 must be rejected");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_the_migration_lock_wait_boundaries() {
        for value in [1u64, 3600] {
            figment::Jail::expect_with(move |jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.create_file(
                    "iam.toml",
                    &format!("{}\n[api_keys]\npepper = \"{}\"\n[migration]\nlock_wait_secs = {value}", minimal_issuer_toml(), valid_pepper_b64()),
                )?;
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(cfg.migration.lock_wait_secs, value);
                cfg.validate().unwrap_or_else(|e| panic!("{value} must be accepted: {e}"));
                Ok(())
            });
        }
    }

    /// The `IAM_` env layer reaches a nested section through `split("__")` (see `figment()`).
    #[test]
    fn the_env_layer_reaches_migration_lock_wait_secs() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("IAM_MIGRATION__LOCK_WAIT_SECS", "300");
            jail.create_file("iam.toml", &format!("{}\n[api_keys]\npepper = \"{}\"\n", minimal_issuer_toml(), valid_pepper_b64()))?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert_eq!(cfg.migration.lock_wait_secs, 300);
            Ok(())
        });
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib migration --no-tests=pass
```

Expected: FAIL to compile — `IamConfig` has no field `migration`.

- [ ] **Step 3: Add the config type**

In `config.rs`, next to the other section structs, add:

```rust
/// Boot-migration serialisation (SMA-559) — the knob for
/// [`migrate_under_lock`](crate::adapters::persistence::migration_lock::migrate_under_lock).
/// Every field has a default, so an absent `[migration]` block is valid config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MigrationConfig {
    /// How long to wait for another replica's migration to finish before giving up and failing
    /// boot. Validated `1..=3600`.
    ///
    /// **`0` is rejected, not a sentinel.** Everywhere else in this config surface `0` means
    /// *never / unbounded* (see [`OutboxRetentionConfig`]); a second reading of `0` here would be
    /// exactly the trap that doc warns about. Write `1` for fail-fast.
    ///
    /// The ceiling is operational, not arithmetic: a wait the container's `HEALTHCHECK
    /// --start-period` cannot accommodate is not usable, so boot additionally warns when
    /// `lock_wait_secs + MIGRATION_BUDGET_SECS` exceeds `IMAGE_START_PERIOD_SECS`.
    pub lock_wait_secs: u64,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self { lock_wait_secs: 120 }
    }
}

impl MigrationConfig {
    /// The configured wait as a `Duration`.
    pub fn lock_wait(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.lock_wait_secs)
    }
}
```

Add the field to `IamConfig` (after `metrics`):

```rust
    #[serde(default)]
    pub migration: MigrationConfig,
```

Add to the `Defaults` struct (:668-679), after `metrics: MetricsConfig,`:

```rust
    migration: MigrationConfig,
```

and to its `Default` impl (:769-783), after `metrics: MetricsConfig::default(),`:

```rust
            migration: MigrationConfig::default(),
```

Add to `validate()` (`config.rs:991`), alongside the other range checks:

```rust
        if !(1..=3600).contains(&self.migration.lock_wait_secs) {
            return Err(format!(
                "migration.lock_wait_secs must be between 1 and 3600 (got {}); 0 is rejected rather than meaning \"never\" — write 1 for fail-fast",
                self.migration.lock_wait_secs
            ));
        }
```

- [ ] **Step 4: Fix the exhaustive struct literals**

`#[serde(default)]` governs deserialization only — it does **not** save an exhaustive struct literal. Add `migration: MigrationConfig::default(),` to each of:

- `rs/crates/services/paigasus-iam/tests/support/mod.rs:444` (`test_config_with`)
- `rs/crates/services/paigasus-iam/src/service_info.rs:132` (`iam_config_with_empty_authn`)
- `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:194` — only if the compiler demands it

Import `MigrationConfig` alongside the other `crate::config` / `paigasus_iam::config` imports in each file. `tests/api_key_cache_connection.rs:42` is `base.clone()` plus field mutation and needs **no** change.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib migration --no-tests=pass && cargo build --workspace --all-targets
```

Expected: the 5 new tests PASS, and the whole workspace including test targets compiles.

- [ ] **Step 6: Verify lints and formatting**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs rs/crates/services/paigasus-iam/src/service_info.rs rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
git commit -m "feat(rs): add the [migration] config section with a validated lock wait (SMA-559)"
```

---

### Task 3: `migrate_under_lock` and the composition root

Spec §3.1, §3.5. The lock itself, plus its only production call site.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs` (re-export)
- Modify: `rs/crates/services/paigasus-iam/src/main.rs:109` (the bare `Migrator::up`)

**Interfaces:**
- Consumes: Task 1's `next_poll`, `Poll`, `MIGRATION_LOCK_KEY`, `POLL_BACKOFF`, `LOG_THROTTLE`, `IMAGE_START_PERIOD_SECS`, `MIGRATION_BUDGET_SECS`. Task 2's `config.migration.lock_wait()`.
- Produces: `pub struct MigrationLockOutcome { pub waited: Duration, pub polls: u32, pub migrations_applied: u64 }`; `pub enum MigrationLockError { Contended { waited, key }, Db(DbErr), Migrate(DbErr) }`; `pub async fn migrate_under_lock(db: &DatabaseConnection, wait: Duration) -> Result<MigrationLockOutcome, MigrationLockError>`.

- [ ] **Step 1: Add the outcome type, the error type, and the function**

Add to `migration_lock.rs` (imports at the top of the file):

```rust
use std::time::Instant;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait};
use sea_orm_migration::MigratorTrait;

use super::Migrator;
```

and the body:

```rust
/// What a `migrate_under_lock` call actually did.
///
/// Returned rather than discarded because a `Result<(), _>` makes the wait unobservable: a test
/// measuring wall clock around the call cannot tell "waited for the lock" from "ran the
/// migration", so it would pass with the lock deleted. An advisory lock does not block DDL, so
/// that is not a theoretical concern. `PgPartitionMaintainer::tick` returns a `MaintenanceReport`
/// for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationLockOutcome {
    /// Time spent waiting for the lock, excluding the migration itself.
    pub waited: Duration,
    /// Failed acquisition attempts before the successful one. `0` = uncontended.
    pub polls: u32,
    /// Migrations actually applied. `0` on a warm boot.
    pub migrations_applied: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationLockError {
    #[error(
        "timed out after {waited:?} waiting for the migration advisory lock (key {key}); another \
         replica may still be migrating, or a lock may be stranded by a hard-killed pod — see \
         docs/ops/RUNBOOK-containers.md"
    )]
    Contended { waited: Duration, key: i64 },
    #[error("database error while acquiring the migration advisory lock: {0}")]
    Db(#[source] DbErr),
    #[error("migration failed under the advisory lock: {0}")]
    Migrate(#[source] DbErr),
}

/// How many migrations `seaql_migrations` records, or `0` when the table does not exist yet.
///
/// Two statements rather than one `COALESCE`: Postgres plans a whole statement up front, so a
/// `SELECT count(*) FROM seaql_migrations` guarded by a subquery still fails on a fresh database
/// where `Migrator::up`'s own `install()` has not run yet.
async fn applied_count<C: ConnectionTrait>(db: &C) -> Result<u64, DbErr> {
    let present = db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT to_regclass('public.seaql_migrations') IS NOT NULL AS present".to_string(),
        ))
        .await?
        .and_then(|r| r.try_get::<bool>("", "present").ok())
        .unwrap_or(false);
    if !present {
        return Ok(0);
    }
    let row = db
        .query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT count(*)::bigint AS n FROM seaql_migrations".to_string()))
        .await?;
    Ok(row.and_then(|r| r.try_get::<i64>("", "n").ok()).unwrap_or(0).max(0) as u64)
}

/// Run `Migrator::up` under a transaction-scoped advisory lock, waiting up to `wait` to acquire it.
///
/// **Production code must never call `Migrator::up` bare.** There is exactly one production call
/// site (`main.rs`) and this is it; the remaining call sites are integration tests that
/// deliberately drive migrations step by step.
///
/// The loop is do-while: an attempt always happens before any give-up decision, so even the
/// smallest legal `wait` still asks Postgres once.
pub async fn migrate_under_lock(db: &DatabaseConnection, wait: Duration) -> Result<MigrationLockOutcome, MigrationLockError> {
    let start = Instant::now();
    let mut polls: u32 = 0;
    let mut last_log = start;

    loop {
        // A failure here aborts boot rather than counting against the budget: it means the pool
        // is unusable, which no amount of waiting fixes.
        let txn = db.begin().await.map_err(MigrationLockError::Db)?;

        // Rollback is EXPLICIT on every path we own, INCLUDING the fallible reads below.
        // `DatabaseTransaction::Drop` calls `start_rollback().expect(..)`, which panics if the
        // connection mutex is contended — turning a migration error into a panic in the
        // composition root. A bare `?` here would leave `txn` live and hit exactly that.
        // (sea-orm-migration drops its own inner savepoint transaction on error; that residual
        // is not ours.) `db.begin()` above is the one exception: there is no transaction yet.
        let acquired = match txn
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT pg_try_advisory_xact_lock({MIGRATION_LOCK_KEY}) AS locked"),
            ))
            .await
        {
            Ok(row) => row.and_then(|r| r.try_get::<bool>("", "locked").ok()).unwrap_or(false),
            Err(e) => {
                let _ = txn.rollback().await;
                return Err(MigrationLockError::Db(e));
            }
        };

        if acquired {
            // Captured BEFORE the migration runs: `waited` is documented as excluding the
            // migration itself, and Task 4 asserts on it to prove a waiter genuinely waited.
            // Computing it after `commit` would silently fold migration time in and stop the
            // assertion discriminating.
            let waited = start.elapsed();
            let before = match applied_count(&txn).await {
                Ok(n) => n,
                Err(e) => {
                    let _ = txn.rollback().await;
                    return Err(MigrationLockError::Db(e));
                }
            };
            if let Err(e) = Migrator::up(&txn, None).await {
                let _ = txn.rollback().await;
                return Err(MigrationLockError::Migrate(e));
            }
            let after = match applied_count(&txn).await {
                Ok(n) => n,
                Err(e) => {
                    let _ = txn.rollback().await;
                    return Err(MigrationLockError::Db(e));
                }
            };
            txn.commit().await.map_err(MigrationLockError::Db)?;
            return Ok(MigrationLockOutcome { waited, polls, migrations_applied: after.saturating_sub(before) });
        }

        let _ = txn.rollback().await;

        match next_poll(start.elapsed(), wait) {
            Poll::GiveUp => {
                let waited = start.elapsed();
                // `main.rs` surfaces a boot error only through a bare `eprintln!`, which bypasses
                // `paigasus_logging` — without this the structured pipeline gets the waiting
                // lines and then silence at the moment that actually matters.
                tracing::error!(?waited, polls, key = MIGRATION_LOCK_KEY, "gave up waiting for the migration advisory lock");
                return Err(MigrationLockError::Contended { waited, key: MIGRATION_LOCK_KEY });
            }
            Poll::Retry(backoff) => {
                if last_log.elapsed() >= LOG_THROTTLE || polls == 0 {
                    tracing::info!(
                        elapsed = ?start.elapsed(),
                        remaining = ?wait.saturating_sub(start.elapsed()),
                        key = MIGRATION_LOCK_KEY,
                        "another replica is migrating; waiting for the migration advisory lock"
                    );
                    last_log = Instant::now();
                }
                polls = polls.saturating_add(1);
                tokio::time::sleep(backoff).await;
            }
        }
    }
}
```

Re-export from `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`, alongside `pub use migration::Migrator;`:

```rust
pub use migration_lock::{MigrationLockError, MigrationLockOutcome, migrate_under_lock};
```

- [ ] **Step 2: Verify it compiles**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam
```

Expected: success. If `thiserror` is unresolved, confirm `thiserror = { workspace = true }` is in `rs/crates/services/paigasus-iam/Cargo.toml` (it is, at :40).

- [ ] **Step 3: Wire the composition root**

In `rs/crates/services/paigasus-iam/src/main.rs`, replace line 109:

```rust
    Migrator::up(&db, None).await?;
```

with:

```rust
    // SMA-559: serialised against a concurrently starting replica by a transaction-scoped
    // advisory lock. A waiter blocks here with NO listener bound — see the probe-budget note in
    // docs/ops/RUNBOOK-containers.md, and SMA-571 for the bind-first fix that removes the
    // coupling entirely.
    if config.migration.lock_wait_secs + MIGRATION_BUDGET_SECS > IMAGE_START_PERIOD_SECS {
        tracing::warn!(
            lock_wait_secs = config.migration.lock_wait_secs,
            start_period_secs = IMAGE_START_PERIOD_SECS,
            "migration.lock_wait_secs plus the migration budget exceeds the container image's HEALTHCHECK start period — a waiting replica may be reported unhealthy before it finishes waiting"
        );
    }
    let migration = migrate_under_lock(&db, config.migration.lock_wait()).await?;
    tracing::info!(
        waited = ?migration.waited,
        polls = migration.polls,
        migrations_applied = migration.migrations_applied,
        "database migrations complete"
    );
```

Update the imports at `main.rs:12-13`: drop `Migrator` from the `paigasus_iam::adapters::persistence::{..}` list if nothing else in the file uses it, add `migrate_under_lock`, and add
`use paigasus_iam::adapters::persistence::migration_lock::{IMAGE_START_PERIOD_SECS, MIGRATION_BUDGET_SECS};`.
Remove `use sea_orm_migration::MigratorTrait;` if it becomes unused — an unused import is a hard error under `warnings = "deny"`.

- [ ] **Step 4: Verify the build, lints and formatting**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace --all-targets && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs rs/crates/services/paigasus-iam/src/main.rs
git commit -m "feat(rs): serialise iam's boot migration with a postgres advisory lock (SMA-559)"
```

---

### Task 4: The Docker-backed integration suite

Spec §4. Six tests. Read the container-sourcing preamble carefully — two different helpers make these tests silently vacuous.

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs`

**Interfaces:**
- Consumes: Task 3's `migrate_under_lock`, `MigrationLockError`, `MigrationLockOutcome`, `MIGRATION_LOCK_KEY`; `support::start_raw_postgres`, `support::connection_url`.
- Produces: nothing.

**Container sourcing — non-negotiable.** Every test below takes the *container* from `support::start_raw_postgres()` and **discards the `DatabaseConnection` it returns**, building its own via `Database::connect(support::connection_url(&node).await)`. Two hazards:

- `start_raw_postgres` pins its pool to `max_connections(1)`/`min_connections(1)` (`tests/support/mod.rs:144-153`). Reusing that handle for both migrators makes the second `db.begin()` block on the **pool**, not the advisory lock — the test then either serialises trivially or trips sqlx's acquire timeout.
- `start_migrated_postgres` (`tests/support/mod.rs:73`) has **already run `Migrator::up` at :78**. Using it makes every call a no-op with every assertion passing trivially. Test 1 opens with a pre-assertion against exactly this.

- [ ] **Step 1: Write the test file**

Create `rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The SMA-559 migration advisory lock, proven against a real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker; the skip-versus-panic decision lives once, in
//! `tests/support/docker.rs::start_or_skip`, reached through `support::start_raw_postgres`.
//!
//! Every test takes the CONTAINER from `start_raw_postgres` and discards its returned handle —
//! that handle is pinned to a single connection, so reusing it for two concurrent migrators
//! would make the second block on the pool rather than on the advisory lock. And nothing here
//! may use `start_migrated_postgres`, which has already migrated.

mod support;

use std::time::Duration;

use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::adapters::persistence::migration_lock::{MIGRATION_LOCK_KEY, MigrationLockError, migrate_under_lock};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

/// A fresh multi-connection pool onto the same container. NOT `start_raw_postgres`'s handle.
async fn connect(url: &str) -> DatabaseConnection {
    Database::connect(url.to_string()).await.expect("connect")
}

/// A pool pinned to ONE physical connection, so a session-level advisory lock taken on it stays
/// held across statements.
async fn connect_pinned(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.max_connections(1).min_connections(1);
    Database::connect(opts).await.expect("connect pinned")
}

async fn scalar_bool(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .and_then(|r| r.try_get::<bool>("", "v").ok())
        .expect("a bool column named v")
}

async fn migrations_table_exists(db: &DatabaseConnection) -> bool {
    scalar_bool(db, "SELECT to_regclass('public.seaql_migrations') IS NOT NULL AS v").await
}

async fn applied_migrations(db: &DatabaseConnection) -> i64 {
    db.query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT count(*)::bigint AS n FROM seaql_migrations".to_string()))
        .await
        .expect("query")
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .expect("a count")
}

/// AC 1 and AC 2: two migrators, one database, no deadlock, one converged schema.
///
/// `tokio::time::timeout` IS the deadlock assertion — the same technique
/// `outbox_retention_concurrency_pg.rs` uses.
#[tokio::test]
async fn two_concurrent_migrators_converge_without_deadlocking() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let a = connect(&url).await;
    let b = connect(&url).await;

    // Guards against someone swapping in `start_migrated_postgres`, which would make every
    // assertion below pass trivially.
    assert!(!migrations_table_exists(&a).await, "the container must start UNMIGRATED — did this switch to start_migrated_postgres?");

    let wait = Duration::from_secs(120);
    let joined = tokio::time::timeout(Duration::from_secs(300), async {
        tokio::join!(migrate_under_lock(&a, wait), migrate_under_lock(&b, wait))
    })
    .await
    .expect("concurrent migrations deadlocked");

    let (ra, rb) = joined;
    let oa = ra.expect("migrator A must exit cleanly");
    let ob = rb.expect("migrator B must exit cleanly");

    assert_eq!(applied_migrations(&a).await as usize, Migrator::migrations().len(), "the schema must be at the tip");
    assert!(scalar_bool(&a, "SELECT relkind = 'p' AS v FROM pg_class WHERE relname = 'audit_log'").await, "audit_log must be partitioned (m0008)");

    // Exactly one side did the work. This is what distinguishes real serialisation from
    // both-ran-and-one-lost.
    let workers = [&oa, &ob].iter().filter(|o| o.migrations_applied > 0).count();
    assert_eq!(workers, 1, "exactly one migrator should apply migrations, got {oa:?} and {ob:?}");
}

/// The guard is load-bearing: with the lock held, the migration must not proceed AT ALL.
///
/// Delete `pg_try_advisory_xact_lock` from `migrate_under_lock` and `Migrator::up`'s own
/// `install()` creates `seaql_migrations`, failing the second assertion.
#[tokio::test]
async fn a_held_lock_blocks_the_migration_and_leaves_the_database_untouched() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    let db = connect(&url).await;

    // `pg_try_advisory_lock`, not `pg_advisory_lock`: the latter returns void and so cannot
    // assert its own setup, and a holder that silently failed would make this test vacuous.
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let err = migrate_under_lock(&db, Duration::from_secs(1)).await.expect_err("must not migrate while the lock is held");
    assert!(matches!(err, MigrationLockError::Contended { .. }), "expected Contended, got {err:?}");
    assert!(!migrations_table_exists(&db).await, "a blocked migration must leave the database untouched");

    assert!(
        scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually release the lock"
    );

    migrate_under_lock(&db, Duration::from_secs(120)).await.expect("must migrate once the lock is free");
    assert_eq!(applied_migrations(&db).await as usize, Migrator::migrations().len());
}

/// The wait-then-acquire path AC 1 actually asks for.
///
/// Asserts on the OUTCOME, not wall clock: an advisory lock does not block DDL, so with the
/// guard deleted a bare `Migrator::up` would also return `Ok` after >= 500ms and a wall-clock
/// assertion would still pass.
#[tokio::test]
async fn a_waiter_waits_and_then_migrates() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    let db = connect(&url).await;

    assert!(scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await);

    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await);
    });

    let outcome = migrate_under_lock(&db, Duration::from_secs(30)).await.expect("must succeed once the holder releases");
    releaser.await.expect("releaser");

    assert!(outcome.polls >= 1, "the waiter must have polled at least once, got {outcome:?}");
    assert!(outcome.waited >= Duration::from_millis(500), "the waiter must have waited for the holder, got {outcome:?}");
    assert!(outcome.migrations_applied > 0, "the waiter must have applied the migrations, got {outcome:?}");
}

/// The claim that justifies transaction-scoped over session-scoped: a FAILED migration still
/// releases the lock.
///
/// Release is asserted from an INDEPENDENT session. Re-calling `migrate_under_lock` would fail
/// identically on the still-present conflicting object and so would prove nothing.
#[tokio::test]
async fn a_failed_migration_still_releases_the_lock() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let db = connect(&url).await;
    let observer = connect_pinned(&url).await;

    // Collides with m0001's first CREATE TABLE, so `Migrator::up` fails inside the lock.
    db.execute_unprepared("CREATE TABLE principal (bogus int);").await.expect("seed the conflict");

    let err = migrate_under_lock(&db, Duration::from_secs(10)).await.expect_err("the migration must fail");
    assert!(!matches!(err, MigrationLockError::Contended { .. }), "expected a migration error, not Contended: {err:?}");

    assert!(
        scalar_bool(&observer, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "a failed migration must still release the advisory lock"
    );
}

/// Warm boot: the second runner applies nothing. A regression guard, not primary evidence.
#[tokio::test]
async fn a_second_run_against_a_migrated_database_applies_nothing() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let db = connect(&url).await;

    let first = migrate_under_lock(&db, Duration::from_secs(120)).await.expect("first run");
    assert!(first.migrations_applied > 0);

    let second = migrate_under_lock(&db, Duration::from_secs(120)).await.expect("second run");
    assert_eq!(second.migrations_applied, 0, "a warm boot must apply nothing");
    assert_eq!(applied_migrations(&db).await as usize, Migrator::migrations().len());
}

/// Spec §3.3: the interaction this design does NOT fix, pinned so the runbook's single-replica
/// caveat is evidence rather than prose.
///
/// m0008 takes `AUDIT_PARTITION_LOCK_KEY` with a BLOCKING `pg_advisory_xact_lock` under
/// `SET LOCAL lock_timeout = '5s'`. An old replica's `PgPartitionMaintainer` holding that key
/// therefore aborts the entire migration — even though this replica won `MIGRATION_LOCK_KEY`.
#[tokio::test]
async fn a_held_audit_partition_lock_aborts_the_migration() {
    const AUDIT_PARTITION_LOCK_KEY: i64 = 5_580_467;

    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping migration lock test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let maintainer = connect_pinned(&url).await;
    let db = connect(&url).await;

    assert!(
        scalar_bool(&maintainer, &format!("SELECT pg_try_advisory_lock({AUDIT_PARTITION_LOCK_KEY}) AS v")).await,
        "the stand-in maintainer must hold the audit-partition key"
    );

    let err = migrate_under_lock(&db, Duration::from_secs(30)).await.expect_err("m0008 must abort under a held audit-partition lock");
    assert!(!matches!(err, MigrationLockError::Contended { .. }), "the migration lock was won; the failure must come from m0008: {err:?}");
    assert!(!migrations_table_exists(&db).await, "the whole migration transaction must roll back");

    assert!(scalar_bool(&maintainer, &format!("SELECT pg_advisory_unlock({AUDIT_PARTITION_LOCK_KEY}) AS v")).await);
}
```

- [ ] **Step 2: Run the suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test migration_lock_pg --no-tests=pass
```

`PAIGASUS_REQUIRE_DOCKER=1` turns the Docker skip into a panic — required for a FILTERED run, because `tests/docker_preflight.rs` (the canary that would otherwise catch an unreachable daemon) is not in this filter.

Expected: 6 PASS. If a test fails, do **not** relax an assertion — each one is load-bearing and the spec explains why. In particular, if `a_held_audit_partition_lock_aborts_the_migration` does not fail the migration, the §3.3 claim is wrong and the runbook wording in Task 6 must change with it; stop and report rather than deleting the test.

- [ ] **Step 3: Verify lints and formatting**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs
git commit -m "test(rs): prove concurrent iam migrations converge under the advisory lock (SMA-559)"
```

---

### Task 5: The container start period, enforced

Spec §3.5. A waiter sits with no listener bound, so the image's health-check start period must cover the wait plus the migration — and the invariant is asserted rather than merely written down.

**Files:**
- Modify: `rs/Dockerfile:70-71`
- Modify: `ci/images/run.sh` — `assert_pins`, inserting before the closing `echo "  pins OK: …"` at `:116`

**Interfaces:**
- Consumes: Task 1's `IMAGE_START_PERIOD_SECS`/`MIGRATION_BUDGET_SECS`, Task 2's default `lock_wait_secs = 120`.
- Produces: nothing.

- [ ] **Step 1: Raise the start period**

In `rs/Dockerfile`, replace lines 70-71:

```dockerfile
# 60s start period because IAM runs Migrator::up before it binds.
HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=3 \
```

with:

```dockerfile
# 180s start period = migration.lock_wait_secs (default 120) + a 60s migration budget. IAM runs
# Migrator::up before it binds, and since SMA-559 a replica that LOSES the migration-lock race
# waits for the leader with nothing bound at all. INVARIANT: start-period must stay >=
# lock_wait_secs + 60; ci/images/run.sh's assert_pins enforces it against config.rs's default.
# Note this governs docker run / Compose / Swarm and ci/images/run.sh only — the kubelet ignores
# a HEALTHCHECK entirely, so Kubernetes sizes startupProbe instead (docs/ops/RUNBOOK-containers.md).
HEALTHCHECK --interval=30s --timeout=3s --start-period=180s --retries=3 \
```

- [ ] **Step 2: Add the pin assertion**

In `ci/images/run.sh`'s `assert_pins`, immediately **before** the closing `echo "  pins OK: …"` line (`:116`):

```bash
  # SMA-559: a replica that loses the migration-lock race waits with NO listener bound, so the
  # image's start period must cover that wait plus the migration itself. A config default raised
  # without touching the Dockerfile would silently re-arm the restart-while-waiting bug.
  local start_period lock_wait budget required
  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" | head -1 | grep -oE '[0-9]+')"
  lock_wait="$(grep -oE 'lock_wait_secs: [0-9]+' "$ROOT/rs/crates/services/paigasus-iam/src/config.rs" | head -1 | grep -oE '[0-9]+')"
  budget="$(grep -oE 'MIGRATION_BUDGET_SECS: u64 = [0-9]+' "$ROOT/rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs" | head -1 | grep -oE '[0-9]+$')"
  if [ -z "$start_period" ] || [ -z "$lock_wait" ] || [ -z "$budget" ]; then
    echo "::error::could not read the start-period/lock-wait/migration-budget triple (start_period=${start_period:-<missing>} lock_wait=${lock_wait:-<missing>} budget=${budget:-<missing>}); one of the grep anchors moved." >&2
    return 1
  fi
  required=$((lock_wait + budget))
  if [ "$start_period" -lt "$required" ]; then
    echo "::error::rs/Dockerfile's HEALTHCHECK --start-period=${start_period}s is below migration.lock_wait_secs (${lock_wait}) + the migration budget (${budget}) = ${required}s." >&2
    echo "  A replica waiting on the SMA-559 migration lock binds no listener, so it would be reported unhealthy while correctly waiting. Raise the start period or lower the default wait." >&2
    return 1
  fi
```

and extend the final echo to mention it:

```bash
  echo "  pins OK: rustc ${channel}, bookworm builder, ubuntu ${ubuntu_from} == chisel release, no baked service config, start-period ${start_period}s >= ${required}s"
```

- [ ] **Step 3: Run the check and prove it bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/images/run.sh build 2>&1 | head -30
```

Expected: `pins OK: … start-period 180s >= 180s`. (If `build` is slow or Docker is unavailable, run only the pin function: `bash -c 'source ci/images/run.sh; assert_pins'` — confirm the script tolerates being sourced, and if it does not, temporarily set `--start-period=60s` and re-run the same command you used above to see it fail, then restore 180s.)

**Prove the guard bites before committing.** Temporarily change `--start-period=180s` to `--start-period=60s`, re-run, and confirm it fails with the new error. Restore 180s and re-run to confirm it passes. Do not skip this: a pin check that cannot fail is worth nothing.

- [ ] **Step 4: Commit**

```bash
git add rs/Dockerfile ci/images/run.sh
git commit -m "build(repo): cover the migration lock wait in the image start period (SMA-559)"
```

---

### Task 6: Runbook and the SMA-513 handoff

Spec §5, §6, §3.8. AC 3 and AC 4.

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md` — the probe table row at `:92`, and the §5 bullet at `:111-113`

**Interfaces:**
- Consumes: everything above.
- Produces: the AC 4 handoff artifact.

- [ ] **Step 1: Replace the §5 bullet**

Replace `docs/ops/RUNBOOK-containers.md:111-113`:

```markdown
- **IAM runs `Migrator::up` on every process start, with no advisory lock around it.** A rolling
  update or a scale-out risks concurrent migration. Migrate with a single replica —
  `replicas: 1` with `strategy.rollingUpdate.maxSurge: 0` — or use a pre-install migration Job.
```

with:

```markdown
- **IAM serialises its boot migration with a Postgres advisory lock (SMA-559), but that covers
  migrations against *each other* and nothing else.** Two replicas starting together now converge:
  the loser waits `migration.lock_wait_secs` (`IAM_MIGRATION__LOCK_WAIT_SECS`, default 120,
  validated 1–3600), then finds nothing to do. **Two exceptions keep `replicas: 1` /
  `strategy.rollingUpdate.maxSurge: 0` a requirement rather than a recommendation:**
  1. **The release that introduces the lock.** Old replicas still migrate unguarded, so the
     upgrade *to* the locking version is the one rollout the lock cannot protect. Relax only from
     the release after it.
  2. **A migration doing DDL on a table a background maintainer also touches** — the m0008 class.
     An old replica's `PgPartitionMaintainer` holds `AUDIT_PARTITION_LOCK_KEY`, which m0008 waits
     for under a 5s `lock_timeout`; hold it longer and the entire migration transaction aborts,
     even though that replica won the migration lock.

  A long migration also still warrants a maintenance window: the whole run is **one transaction**,
  so m0008-class DDL holds `ACCESS EXCLUSIVE` on `audit_log` for its full duration and **every
  running replica's audit writes block** for that window. Sizing `lock_wait_secs` for a large
  table is simultaneously sizing an audit-write stall.

  **Probe budgets.** A waiting replica has **no listener bound at all**, and the two probe systems
  are not the same system. The image's `HEALTHCHECK --start-period` (180s = the 120s default wait
  + a 60s migration budget, enforced by `ci/images/run.sh`'s `assert_pins`) governs
  `docker run`, Compose and Swarm; **the kubelet ignores a `HEALTHCHECK` entirely** and sizes
  `startupProbe` instead:

  ```
  startupProbe.failureThreshold × periodSeconds  >  lock_wait_secs + migration budget + AppState::new
  ```

  At the shipped default: `periodSeconds: 10`, `failureThreshold: 30` (300s). `AppState::new` is a
  third, unbudgeted term — it reconciles policies and loads a snapshot after the migration and
  still before any bind. Raising `lock_wait_secs` means raising both.

  **Chart defaults (handoff to SMA-513).** `strategy.rollingUpdate.maxSurge` need no longer be
  pinned to `0`, subject to the two exceptions above; set `startupProbe` from the formula; expose
  `IAM_MIGRATION__LOCK_WAIT_SECS`. SMA-571 (bind-first readiness gating) will make the
  `start-period` coupling vestigial.

  **Recovering a stranded lock.** A pod SIGKILL'd on a partitioned node leaves its backend holding
  the lock until TCP-level timeouts fire — by default, hours — and every later replica then waits
  and fails to boot. Find it (scoped to this database, since one cluster may host several):

  ```sql
  SELECT pid, granted, query_start
  FROM pg_locks l JOIN pg_stat_activity a USING (pid)
  WHERE l.locktype = 'advisory'
    AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
    AND ((l.classid::bigint << 32) | l.objid::bigint) = 5580559;
  ```

  The parentheses are load-bearing — Postgres gives `<<` and `|` equal precedence. Then
  `SELECT pg_terminate_backend(<pid>)`. **This needs privileges the IAM application role usually
  lacks**: `query_start` reads as NULL for other users' backends without `pg_read_all_stats`, and
  `pg_terminate_backend` needs `pg_signal_backend` or superuser — run it as an admin role. To
  bound it automatically, set `idle_in_transaction_session_timeout` (the stranded backend is
  *idle in transaction*, so `idle_session_timeout` does **not** apply — and setting that one
  aggressively would instead kill a healthy replica between poll attempts) and `tcp_user_timeout`
  (`tcp_keepalives_idle` alone only starts probing).

  Behind a transaction-mode pooler the lock is safe by construction — it is acquired and released
  within one transaction — but PgBouncer's `idle_transaction_timeout` can kill a long migration.
```

- [ ] **Step 2: Update the probe table row**

Replace the `startup` row at `docs/ops/RUNBOOK-containers.md:92`:

```markdown
| startup | `GET /healthz` | IAM migrates at boot — a `startupProbe` with a generous `failureThreshold` is required, or the kubelet kills it mid-migration |
```

with:

```markdown
| startup | `GET /healthz` | IAM migrates at boot, and since SMA-559 a replica that loses the migration-lock race also *waits* with nothing bound — budget `lock_wait_secs` + the migration + `AppState::new`, see §5 |
```

- [ ] **Step 3: Verify the docs render and nothing else regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "5580559\|lock_wait_secs\|maxSurge" docs/ops/RUNBOOK-containers.md
```

Expected: the new content is present, and no stale "with no advisory lock around it" text remains:

```bash
grep -c "no advisory lock around it" docs/ops/RUNBOOK-containers.md
```

Expected: `0`.

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md
git commit -m "docs(repo): document the iam migration lock and its two single-replica exceptions (SMA-559)"
```

---

### Task 7: Full-graph verification

Per-project Moon tasks do **not** run the repo-level gates. This is the run that matches CI.

**Files:** none.

- [ ] **Step 1: Run the full affected graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: all green. Moon attributes failures poorly — diagnose any "N failed" with:

```bash
jq '.actions[] | select(.status == "failed") | {label, status}' .moon/cache/ciReport.json
```

Two gates are worth anticipating:
- `:iam-docker-policy-single-site` — fails if `migration_lock_pg.rs` hand-rolled a Docker skip instead of going through `support::start_raw_postgres`. It does not; if this reds, the test file drifted.
- `:input-liveness` — fails if a declared glob matches zero tracked files. Task 4 adds a file, so this should stay green; if it reds, read `ci/affected-graph/task_inputs.py`'s message.

No new crate and no new dependency is added by this plan, so `:affected-smoke`'s `lockfile->all-lint` and `kernel->bindings` expected sets need **no** re-baselining.

- [ ] **Step 2: Commit any fixes**

If the graph required changes, commit them with a `fix(rs):` or `fix(repo):` scope and the `(SMA-559)` suffix.

---

## Self-Review

**Spec coverage.** §2/§2.1 → Task 1's module doc and Task 6's runbook exceptions. §3.1 → Task 3. §3.2 → Task 1. §3.3 → Task 4 test 6 and Task 6. §3.4 → Task 2. §3.5 → Tasks 3 and 5. §3.6 → Task 6's recovery block. §3.7 → Task 6's pooler note. §3.8 → Task 6 exception 1. §4 → Tasks 1, 2, 4. §5 → Task 6. §6 → Task 6's chart-defaults block. §7/§8 → recorded in the spec; no code.

**Not covered by design:** the Linear comment on SMA-513 pointing at the runbook block — a controller action after merge, not a task.

**Type consistency.** `migrations_applied` is `u64` in `MigrationLockOutcome` and compared against `Migrator::migrations().len()` (`usize`) only after an `as usize` cast on the SQL count. `next_poll(elapsed, wait)` takes `(Duration, Duration)` in Task 1 and is called as `next_poll(start.elapsed(), wait)` in Task 3. `lock_wait()` returns `Duration` in Task 2 and is passed as `wait` in Task 3. `MIGRATION_BUDGET_SECS`/`IMAGE_START_PERIOD_SECS` are `u64` in Task 1 and compared against `config.migration.lock_wait_secs` (`u64`) in Task 3.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
