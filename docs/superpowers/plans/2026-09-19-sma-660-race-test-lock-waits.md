# SMA-660 Race-Test Lock Waits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make four more `paigasus-iam` race tests wait on an observed lock rather than on a fixed sleep, and make the idempotent-`put` test assert that the absorb path ran.

**Architecture:** SMA-659's `wait_until_blocked_by` helper moves from `tests/authz_policy_store.rs` to a new `tests/support/race.rs`, and its `pg_stat_activity` predicate gains a per-site `query_prefix` parameter. Six tests then call it: the three SMA-659 tests unchanged in behaviour, the rewritten absorb test, and the four sites that sleep today. The change is test-only — no production code ships in this branch.

**Tech Stack:** Rust (edition 2024), SeaORM, `tokio`, `cargo nextest`, Postgres 16 in Docker via testcontainers, Moon.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-660-race-test-lock-waits-design.md` — read it before Task 1. The plan argues from it and cites its section numbers.

## Global Constraints

- **Work in the worktree.** Everything below runs from `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits`, on branch `feature/sma-660-race-test-lock-waits`. Do not `cd` to the main checkout.
- **Every shell command starts with** `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `cargo nextest` and `moon` resolve to the repository-pinned tools.
- **Every test run uses** `PAIGASUS_REQUIRE_DOCKER=1` and `--retries 0`. Without the first, a filtered run with no Docker daemon skips in silence and reads as a pass. Without the second, `rs/.config/nextest.toml`'s `retries = 2` can turn a failing mutation green.
- **Docker must be running.** Every test in this plan starts a Postgres container. If the daemon is unreachable the run fails loudly, which is the intended behaviour, not a defect to work around.
- **Every mutation must COMPILE.** This workspace denies warnings, so a mutation that leaves a binding unused or code unreachable dies at `rustc` (rc 101) instead of at the assertion it targets — and a compile failure proves only that warnings are denied, not that the test catches the defect. That is the claim this branch's pull request makes, so it must be demonstrated at the assertion. Measured three times on this plan: shadowing a parameter, adding an early `return`, and deleting a guard that leaves its binding unread all failed this way. If a mutation will not compile, adapt it (silence the binding with `_`, or make a condition unconditionally true) rather than accepting the compile error as the result.
- **When a mutation's expected result is "exactly N tests fail", pass `--no-fail-fast`.** nextest cancels the remaining tests at the first failure by default, so without it the run stops looking and the claim is unprovable.
- **A mutation in a file this branch EDITS is a marked insert**, undone by deleting the marked lines. Write the marker as `// MUTATION SMA-660 — delete this line` on each inserted line. Never restore such a file with `git checkout --`: it would also discard the uncommitted work under test.
- **A mutation in a PRODUCTION file is restored with `git checkout -- <path>`.** This is safe only because this branch changes no production file. Run `git status --short` before and after each one and confirm the path is clean.
- **No production file may appear in any commit.** The only permanent changes are under `rs/crates/services/paigasus-iam/tests/` and `docs/`.
- **SPDX header.** Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **Commits** are Conventional with the `rs` scope, e.g. `test(rs): …`, and end with the line `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` — **verbatim, whichever model writes the commit.** That trailer names the Claude Code session, not the subagent executing a task, so a subagent whose own environment names a different model still writes this line. It keeps one trailer across the branch. Put the issue key `(SMA-660)` in the subject. Do not write a bare `#NNN` line or a `token: value` line in the commit BODY — the local `commit-msg` hook rejects it.
- **Do not write the literal name of Moon's cached CI report file into any file in this repository.** A repository gate requires a special marker on any file that names it, and this plan and its commits do not carry that marker.
- **Record every measurement.** Tasks 2, 4 and 5 produce output that the pull request body must quote. Keep it as you go in `docs/superpowers/plans/2026-09-19-sma-660-measurements.md`, which is committed with Task 6.

---

### Task 1: Move the helper to `tests/support/race.rs` and parameterize its predicate

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/support/race.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (add `pub mod race;` next to the existing `pub mod docker;` at `:60-63`)
- Modify: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` (delete `:73-171`; re-point four call sites)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the module `support::race`, with exactly these public items, which Tasks 2–5 call:
  - `pub const RACER_BLOCK_BUDGET: Duration` (30 s)
  - `pub async fn backend_pid(conn: &impl ConnectionTrait) -> i32`
  - `pub async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, query_prefix: &str, budget: Duration) -> Result<(), String>`
  - `pub async fn expect_racer_blocked<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &mut JoinHandle<T>, query_prefix: &str, describe: impl FnOnce(&T) -> String)`

The existing `authz_policy_store.rs` tests are this task's control: it is a refactor, so they must pass before and after, unchanged in behaviour.

- [ ] **Step 1: Confirm the control is green before you touch anything**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0
```

Expected: PASS, 20-ish tests. If this is red before you start, stop and report — nothing below is meaningful against a red baseline.

- [ ] **Step 2: Create `tests/support/race.rs`**

This is `authz_policy_store.rs:73-171` moved verbatim, with four deliberate changes marked `SMA-660` in the comments: the prefix parameter, `btrim(query)`, the widened `wait_event` set, and the richer deadline message. Write the file exactly as below.

```rust
// SPDX-License-Identifier: Apache-2.0

//! Waiting for a racer to reach a lock, rather than sleeping and hoping it got there.
//!
//! SMA-659 added this to `tests/authz_policy_store.rs` for one INSERT-vs-INSERT race. SMA-660
//! moved it here, because four more race tests in this crate need it and one predicate term —
//! the statement the racer blocks INSIDE — differs per site.
//!
//! Every item is `#[allow(dead_code)]`: `support` compiles once per test binary (about 59 of
//! them) and most use none of this, which `clippy --all-targets -D warnings` would otherwise
//! report.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Upper bound on how long a racer may take to block on the peer's uncommitted statement.
/// A LOAD BUDGET, not an expectation: the wait returns on the first observation that it blocked.
#[allow(dead_code)]
pub const RACER_BLOCK_BUDGET: Duration = Duration::from_secs(30);

/// How often [`wait_until_blocked_by`] polls `pg_stat_activity`.
#[allow(dead_code)]
const RACER_BLOCK_POLL: Duration = Duration::from_millis(10);

/// Counts backends blocked by `$1` on a row lock while running a statement that starts with `$2`.
///
/// Three terms, three jobs (SMA-660 spec §3.2). `$1 = ANY(pg_blocking_pids(pid))` answers WHO
/// blocks the racer, and it is the only term that discriminates at the two `tenancy_events_pg`
/// sites. `btrim(query) ILIKE $2` shows the racer is inside the statement under test rather than
/// an earlier one — `btrim` because the match would otherwise depend on whether a production SQL
/// constant starts its raw string with a newline. `wait_event IN ('transactionid', 'tuple')`
/// covers both forms a row-lock waiter can show: a waiter takes a `tuple` lock before it waits on
/// the holder's transaction id, so a poll can land on either. The `'tuple'` half is NOT verified
/// by any test — see spec D5.
#[allow(dead_code)]
const BLOCKED_STATEMENTS_SQL: &str = "SELECT count(*)::bigint AS n FROM pg_stat_activity \
     WHERE wait_event_type = 'Lock' AND wait_event IN ('transactionid', 'tuple') \
     AND btrim(query) ILIKE $2 AND $1 = ANY(pg_blocking_pids(pid))";

/// Every non-idle client backend but the one running this query, for a deadline message: a stuck
/// pool, a different lock and a wrong predicate each look different here. 200 characters of
/// `query`, not 80: SeaORM emits a long column list before the table name, so 80 cannot tell two
/// SELECTs on different tables apart.
#[allow(dead_code)]
const NON_IDLE_BACKENDS_SQL: &str = "SELECT coalesce(string_agg(format('pid=%s state=%s wait=%s/%s query=%s', \
     pid, state, wait_event_type, wait_event, left(query, 200)), '; ' ORDER BY pid), '(none)') AS dump \
     FROM pg_stat_activity \
     WHERE backend_type = 'client backend' AND state IS DISTINCT FROM 'idle' AND pid <> pg_backend_pid()";

/// The backend pid of the connection that runs `conn` — for a transaction, the backend that holds
/// its locks.
#[allow(dead_code)]
pub async fn backend_pid(conn: &impl ConnectionTrait) -> i32 {
    conn.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT pg_backend_pid() AS pid"))
        .await
        .expect("query pg_backend_pid()")
        .expect("pg_backend_pid() always returns a row")
        .try_get::<i32>("", "pid")
        .expect("pg_backend_pid() is an integer")
}

/// Waits until the racer is inside a statement matching `query_prefix` and blocked by
/// `blocker_pid`, which replaces a fixed sleep that only HOPED it had got there. Each poll is an
/// autocommit statement on `db`: `pg_stat_activity` is a per-transaction snapshot, so a poll
/// inside one transaction would see the same data every time.
///
/// Checks, in this order, every [`RACER_BLOCK_POLL`]: racer blocked → `Ok`; `racer` finished →
/// `Err` (a blocked racer cannot finish while the peer is uncommitted, so checking "blocked"
/// first never hides a finished one); `budget` elapsed → `Err` with `query_prefix` and a dump of
/// the non-idle backends. Panics with `observer query failed: …` if the poll itself fails, which
/// is neither verdict. `budget` is not a hard wall-clock bound: the deadline is checked only
/// after a poll returns, so with an exhausted connection pool one poll can wait up to the pool's
/// own `acquire_timeout` (30 s) before the check runs and panics with `observer query failed`.
/// The worst case is therefore about `budget` plus that acquire timeout, and it is still bounded.
#[allow(dead_code)]
pub async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, query_prefix: &str, budget: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let blocked = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, BLOCKED_STATEMENTS_SQL, [blocker_pid.into(), query_prefix.into()]))
            .await
            .unwrap_or_else(|e| panic!("observer query failed: {e}"))
            .expect("count(*) always returns a row")
            .try_get::<i64>("", "n")
            .unwrap_or_else(|e| panic!("observer query failed: {e}"));
        if blocked > 0 {
            return Ok(());
        }
        if racer.is_finished() {
            return Err(format!(
                "the racer finished before it blocked on the peer's uncommitted statement (pid {blocker_pid}), so the race never happened"
            ));
        }
        if std::time::Instant::now() >= deadline {
            let dump = match db.query_one_raw(Statement::from_string(DbBackend::Postgres, NON_IDLE_BACKENDS_SQL)).await {
                Ok(Some(row)) => row.try_get::<String>("", "dump").unwrap_or_else(|e| format!("(dump unreadable: {e})")),
                Ok(None) => "(dump returned no row)".to_string(),
                Err(e) => format!("(dump failed: {e})"),
            };
            return Err(format!(
                "the racer did not block on the peer's uncommitted statement (pid {blocker_pid}) within {budget:?} \
                 while running a statement matching {query_prefix:?}; non-idle backends: {dump}"
            ));
        }
        tokio::time::sleep(RACER_BLOCK_POLL).await;
    }
}

/// The race tests' call site for [`wait_until_blocked_by`] with [`RACER_BLOCK_BUDGET`]: panics
/// with the wait's reason when the racer does not block, so the test stops HERE and never reaches
/// a verdict that would be meaningless without the race. When the racer has already finished, its
/// own result is part of the message (through `describe`, since a racer's output can hold a
/// non-`Debug` transaction), so an error or a panic inside the racer is not hidden behind
/// "finished".
#[allow(dead_code)]
pub async fn expect_racer_blocked<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &mut JoinHandle<T>, query_prefix: &str, describe: impl FnOnce(&T) -> String) {
    let Err(reason) = wait_until_blocked_by(db, blocker_pid, racer, query_prefix, RACER_BLOCK_BUDGET).await else {
        return;
    };
    if racer.is_finished() {
        let own = match racer.await {
            Ok(output) => describe(&output),
            Err(join_err) => format!("{join_err:?}"),
        };
        panic!("{reason}; the racer's own result: {own}");
    }
    racer.abort();
    panic!("{reason}");
}
```

- [ ] **Step 3: Declare the module**

In `rs/crates/services/paigasus-iam/tests/support/mod.rs`, directly after the existing `pub mod docker;` block (`:60-63`), add:

```rust
/// Waiting for a racer to reach a lock — see `support/race.rs`. `pub` for the same reason as
/// `docker` above: the test binaries reach it as `support::race::*` (SMA-660).
pub mod race;
```

- [ ] **Step 4: Delete the old helper and re-point its call sites**

In `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs`:

1. Delete lines 73-171 — everything from `/// Upper bound on how long racer B may take…` through the closing `}` of `expect_racer_blocked`, inclusive. Keep `seed_system_policy` above it and `#[tokio::test]` below it.
2. The three race tests call `expect_racer_blocked` at `:454`, `:540` and `:617`. Add the prefix argument to each. Example, at `:454`:

```rust
    expect_racer_blocked(&db, pid_a, &mut put_b, "insert%", |r| format!("{r:?}")).await;
```

becomes, with the module path:

```rust
    support::race::expect_racer_blocked(&db, pid_a, &mut put_b, "insert%", |r| format!("{r:?}")).await;
```

Do the same at `:540` and `:617`, keeping each site's existing `describe` closure exactly as it is.
3. The guard test `wait_until_blocked_by_reports_a_racer_that_never_blocks` calls `backend_pid` once and `wait_until_blocked_by` twice. Re-point all three and add `"insert%"` as the fourth argument to both waits:

```rust
    let pid_a = support::race::backend_pid(&db).await;
```

```rust
    let err = support::race::wait_until_blocked_by(&db, pid_a, &put_b, "insert%", support::race::RACER_BLOCK_BUDGET)
```

```rust
    let err = support::race::wait_until_blocked_by(&db, pid_a, &pending, "insert%", Duration::from_millis(200))
```

4. Remove now-unused imports from the file's `use` block. `tokio::task::JoinHandle` (`:37`) is used only by the deleted helper, so delete that line. `std::time::Duration` (`:36`) is still used by the guard test's `Duration::from_millis(200)`, so keep it. `ConnectionTrait` and `Statement` (`:35`) were used by the deleted `backend_pid`; check whether anything else in the file still uses them and delete only what is genuinely unused. `clippy -D warnings` in Step 6 is the arbiter.

- [ ] **Step 5: Run the control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0
```

Expected: PASS, the same test count as Step 1. A deadline failure in one of the three race tests means the predicate changed behaviour — most likely `btrim` or the prefix. Do not widen the predicate to clear it; read the deadline message, which now prints the prefix and the racer's real query.

- [ ] **Step 6: Lint and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: both clean. `--all-targets` is what lints the test files at all.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
git add rs/crates/services/paigasus-iam/tests/support/race.rs \
        rs/crates/services/paigasus-iam/tests/support/mod.rs \
        rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "$(cat <<'EOF'
test(rs): move the racer-block wait into tests/support and parameterize it (SMA-660)

The helper gets a second user, so it leaves authz_policy_store.rs. Its
pg_stat_activity predicate now takes the racer's own statement prefix, matches
a btrim'd query, and accepts the tuple form of a row-lock wait.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Guard the two new predicate terms

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` (the guard test, `:775` onward after Task 1's edit)

**Interfaces:**
- Consumes: `support::race::{backend_pid, wait_until_blocked_by, RACER_BLOCK_BUDGET}` from Task 1.
- Produces: nothing later tasks call. This task's output is confidence that Tasks 3–5's waits cannot silently pass.

Spec §3.5. Without this task, two one-line deletions — the `query_prefix` term and the `pg_blocking_pids` term — leave every test in this branch green.

- [ ] **Step 1: Write the two failing cases**

Append inside `wait_until_blocked_by_reports_a_racer_that_never_blocks`, after the existing "Deadline" block and before the test's closing `}`. Note the shared setup: cases 3 and 4 need a racer that is genuinely blocked, which neither existing case has.

```rust
    // Cases 3 and 4 need a racer that IS blocked, which neither case above has. Set one up once:
    // peer C holds an uncommitted INSERT of a fresh id, and racer D's real `put` of the same id
    // blocks inside its own INSERT on C's row.
    let doc_c = valid_static_doc("guard-blocked-racer", false, now);
    let txn_c = db.begin().await.unwrap();
    policy::ActiveModel {
        policy_id: Set(doc_c.policy_id.clone()),
        kind: Set("static".to_string()),
        source: Set(doc_c.source.clone()),
        description: Set(Some(doc_c.description.clone())),
        system: Set(doc_c.system),
        created_at: Set(doc_c.created_at),
        updated_at: Set(doc_c.updated_at),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(&txn_c)
    .await
    .unwrap();
    let pid_c = support::race::backend_pid(&txn_c).await;
    let store_d = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc_d = doc_c.clone();
    let racer_d = tokio::spawn(async move { store_d.put(&doc_d).await });

    // Case 3: the prefix is honoured. Phase one proves the racer really is blocked — without it,
    // phase two could report `did not block` simply because the racer had not arrived yet, which
    // is the timing-dependent assertion this whole issue removes.
    support::race::wait_until_blocked_by(&db, pid_c, &racer_d, "insert%", support::race::RACER_BLOCK_BUDGET)
        .await
        .expect("phase one: the racer must be blocked inside its INSERT before the prefix can be tested");
    let err = support::race::wait_until_blocked_by(&db, pid_c, &racer_d, "delete%", Duration::from_millis(200))
        .await
        .expect_err("a prefix that matches no statement must reach the deadline, even with the racer blocked");
    assert!(err.contains("did not block"), "wrong message for a non-matching prefix: {err}");
    assert!(err.contains("\"delete%\""), "the deadline message must name the prefix it looked for: {err}");

    // Case 4: the blocker pid is honoured. A third, idle transaction blocks nobody, so asking
    // about ITS pid must reach the deadline although a blocked racer exists.
    let txn_idle = db.begin().await.unwrap();
    let pid_idle = support::race::backend_pid(&txn_idle).await;
    let err = support::race::wait_until_blocked_by(&db, pid_idle, &racer_d, "insert%", Duration::from_millis(200))
        .await
        .expect_err("a pid that blocks nobody must reach the deadline, even with a blocked racer running");
    assert!(err.contains("did not block"), "wrong message for an unrelated blocker pid: {err}");

    // Teardown: release the racer, then drain both transactions so no task outlives the test.
    txn_c.commit().await.unwrap();
    racer_d.await.unwrap().expect("the racer must absorb the same-content conflict once C commits");
    txn_idle.rollback().await.unwrap();
```

The doc comment above the test says it guards two error paths. Replace that sentence so it names all four cases and says what cases 3 and 4 add:

```rust
/// SMA-659/SMA-660 guard: [`support::race::wait_until_blocked_by`] must REPORT a racer that never
/// blocks, and must not report one that blocks for the WRONG reason. Four cases: a racer that
/// finished, a racer that never blocks at all, a prefix matching no statement, and a blocker pid
/// that blocks nobody. Without the last two, deleting either the `query_prefix` term or the
/// `pg_blocking_pids` term from the predicate leaves every race test in this crate green — the
/// second matters most, because `pg_blocking_pids` is the only discriminating term at the two
/// `tenancy_events_pg` sites, whose prefix is a bare `select%`.
```

- [ ] **Step 2: Run the guard test and watch it pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store \
  -E 'test(wait_until_blocked_by_reports_a_racer_that_never_blocks)' --retries 0
```

Expected: PASS. A failure in phase one of case 3 means the setup is wrong (the racer is not blocked), not that the assertion is wrong — fix the setup.

- [ ] **Step 3: Mutation V4 — the prefix must be honoured**

**Every mutation in this task must COMPILE.** One that dies at `rustc` proves only that this workspace denies warnings; it does not prove the guard test catches the defect, which is the claim the pull request makes. So do not shadow the parameter and do not add an early `return` — both leave a parameter unused or code unreachable, and `-D warnings` rejects them at rc 101. (Measured: the first two recipes written here did exactly that.)

In `tests/support/race.rs`, inside `wait_until_blocked_by`, bind a literal into the statement instead. Two marked edits:

```rust
    let _ = query_prefix; // MUTATION SMA-660 — delete this line
```

and, on the `query_one_raw` line, replace `[blocker_pid.into(), query_prefix.into()]` with:

```rust
[blocker_pid.into(), "%".into()] // MUTATION SMA-660 — restore to: [blocker_pid.into(), query_prefix.into()]
```

Run the command from Step 2. **Expected: FAIL, case 3 alone**, at its phase-two `expect_err` — the wrong prefix `delete%` now matches the blocked racer's INSERT because the predicate compares against `%`. Cases 1, 2 and 4 must still pass, since `pg_blocking_pids` is untouched; if any of them fails too, the cases are less independent than this brief claims, so record that. Then restore both lines and re-run to confirm PASS.

- [ ] **Step 4: Mutation V6 — the blocker pid must be honoured**

In `tests/support/race.rs`, change `BLOCKED_STATEMENTS_SQL`'s last term by inserting a neutralised copy. Replace the constant's final line with the two lines below, keeping the original as a comment so the restore is mechanical:

```rust
     AND btrim(query) ILIKE $2 AND ($1 = ANY(pg_blocking_pids(pid)) OR true)"; // MUTATION SMA-660 — restore to: AND btrim(query) ILIKE $2 AND $1 = ANY(pg_blocking_pids(pid))";
```

Run the command from Step 2. **Expected: FAIL**, on case 4 (`a pid that blocks nobody must reach the deadline`). Record it. Restore the original line and re-run to confirm PASS.

- [ ] **Step 5: Mutation V3 — the wait must be load-bearing at all**

Again, it must compile. In `tests/support/race.rs`, make the first check unconditionally true instead of returning early — `blocked` is a `count(*)` and can never be negative, so the wait returns `Ok` on its first poll while still running the observer query:

```rust
        if blocked >= 0 { // MUTATION SMA-660 — restore to: if blocked > 0 {
```

Run the command from Step 2. **Expected: FAIL at case 1.** All four cases assert an `Err`, so all four are broken by this mutation — but a `#[tokio::test]` stops at its first panic, so one run can only ever show the first. Case 1 failing IS the evidence; do not split the test to observe the other three, which would quadruple this file's container starts to prove something the first failure already establishes. Record the output, restore the line, and re-run to confirm PASS.

- [ ] **Step 6: Lint, format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd ..
git status --short   # only tests/authz_policy_store.rs may be modified; race.rs must be clean
git add rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "$(cat <<'EOF'
test(rs): guard the racer wait's prefix and blocker-pid terms (SMA-660)

Two new guard cases drive a genuinely blocked racer: one asks with a prefix
that matches no statement, one with a pid that blocks nobody. Deleting either
predicate term left every race test in the crate green before this.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Make the idempotent-`put` test assert the absorb path

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` — the doc comment and body of `concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict`

**Locate by symbol, not by line number.** Task 1 deleted about 99 lines from the top of this file and Task 2 appended to the guard test, so every line number the spec and the issue quote for this file is stale. Find the test with `grep -n "concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict" rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` and work from there. Its doc comment is the `///` block immediately above its `#[tokio::test]`, ending at the line `/// must exist afterward.`

**Interfaces:**
- Consumes: `support::race::{backend_pid, expect_racer_blocked}` from Task 1.
- Produces: nothing later tasks call.

Spec §3.4 and §1.2. The test currently drives two `put` calls under `tokio::join!` and asserts only that both return `Ok` and one row exists — both of which are true when the second racer took the ordinary UPDATE path and the absorb never happened.

- [ ] **Step 1: Replace the test body**

Replace the whole function body — from `async fn concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict() {` through its closing `}` — with:

```rust
async fn concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let doc = valid_static_doc("racing-policy", false, now);

    // Racer A: insert directly (bypassing `PgPolicyStore::put`, mirroring `seed_system_policy`'s
    // established direct-entity pattern) and hold the transaction open. The content matches
    // `doc` on every field `policy_content_matches` compares — kind, source, description and
    // system — which is what makes B's conflict an absorb rather than a `Conflict`.
    let txn_a = db.begin().await.unwrap();
    policy::ActiveModel {
        policy_id: Set(doc.policy_id.clone()),
        kind: Set("static".to_string()),
        source: Set(doc.source.clone()),
        description: Set(Some(doc.description.clone())),
        system: Set(doc.system),
        created_at: Set(doc.created_at),
        updated_at: Set(doc.updated_at),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(&txn_a)
    .await
    .unwrap();
    let pid_a = support::race::backend_pid(&txn_a).await;

    // Racer B: the real `PgPolicyStore::put`. The test holds a CLONE of the same `Generations`
    // handle B bumps — every other store in this file takes a fresh `Generations::memory()`, and
    // copying that here would leave the assertion below reading a counter nothing touches.
    let gens = Generations::memory();
    let store_b = PgPolicyStore::new(db.clone(), gens.clone());
    let doc_b = doc.clone();
    let mut put_b = tokio::spawn(async move { store_b.put(&doc_b).await });
    let before = gens.policy_gen().await.unwrap();

    // Commit A only once B is provably blocked INSIDE its INSERT on A's uncommitted row — past
    // its existence check, so its INSERT must resolve into a real unique violation.
    support::race::expect_racer_blocked(&db, pid_a, &mut put_b, "insert%", |r| format!("{r:?}")).await;
    txn_a.commit().await.unwrap();

    let result_b = put_b.await.unwrap();
    assert!(result_b.is_ok(), "the unique-violation loser must absorb, not error: {result_b:?}");

    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let all = store.list_all().await.unwrap();
    let matches: Vec<_> = all.iter().filter(|d| d.policy_id == "racing-policy").collect();
    assert_eq!(matches.len(), 1, "exactly one row must exist after the race, not zero or two: {matches:?}");

    // THE assertion this test exists for. `put` bumps `policy_gen` unless its outcome is
    // `AbsorbedIdempotent`, and racer A was a raw entity insert that bumps nothing. So an
    // unmoved counter means B absorbed, and a counter that moved by one means B took the UPDATE
    // path — the silent failure this test could not see while it used `tokio::join!`.
    assert_eq!(
        gens.policy_gen().await.unwrap(),
        before,
        "put must SKIP its policy_gen bump on the absorb path; a moved generation means racer B took the UPDATE path and the absorb never ran"
    );
}
```

- [ ] **Step 2: Replace the doc comment**

Replace the whole `///` doc comment above the test (it currently starts `/// Boot-reliability fix (SMA-444 Task 17 review finding):` and ends `/// must exist afterward.`) with:

```rust
/// Boot-reliability fix (SMA-444 Task 17 review finding): `PgPolicyStore::put`'s existence
/// check and its INSERT aren't atomic, so two replicas booting concurrently against a fresh,
/// unseeded database can both observe `existing == None` for the same starter `policy_id` and
/// both attempt to insert it — the loser must hit a unique-constraint violation and absorb it as
/// an idempotent success, not fail its replica's `AppState::new`.
///
/// SMA-660 made this deterministic and gave it a real assertion. It used to drive two `put`
/// calls under `tokio::join!` and assert only that both returned `Ok` and one row survived —
/// both of which are ALSO true when racer A's whole `put` committed before B's existence check,
/// leaving B on the ordinary UPDATE path with no absorb anywhere in the test. It now holds A's
/// INSERT open in an uncommitted transaction, waits until B is provably blocked inside its own
/// INSERT, and then commits A, exactly as the two `put_in` race tests below do.
///
/// What it asserts that nothing else does: `put` SKIPS its `policy_gen` bump when the outcome is
/// `AbsorbedIdempotent`. The absorb itself is already covered at the `put_in` level by
/// `put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable`, which reads
/// the `PutOutcome` directly; `put` returns `Result<(), AuthzError>` and cannot, so the
/// generation counter is the only observable that tells the two paths apart here.
```

- [ ] **Step 3: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store \
  -E 'test(concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict)' --retries 0
```

Expected: PASS.

- [ ] **Step 4: Mutation V5 — the assertion must bite**

This one mutates a PRODUCTION file, so it is restored with `git checkout --`. In `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs`, replace the guard at `:257-259`:

```rust
        if !matches!(outcome, PutOutcome::AbsorbedIdempotent) {
            self.bump_policy_gen_best_effort().await;
        }
```

with an unconditional bump. `outcome` then has no reader, and `-D warnings` rejects that, so rename its binding to `_outcome` in the same mutation — the mutation must COMPILE or it proves only that this workspace denies warnings:

```rust
        let _outcome = self.put_in(&*tx, doc).await?;
        tx.commit().await.map_err(map_txn_err)?;
        self.bump_policy_gen_best_effort().await;
```

Then run the WHOLE binary, not just the one test, and **pass `--no-fail-fast`**. Without it nextest cancels the remaining tests at the first failure, so "exactly one test failed" would be unprovable — the run would simply stop looking:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 --no-fail-fast
```

**Expected: exactly ONE failure** — `concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict`, on its `policy_gen` assertion. Record the count and the message.

If a second test also fails, that test already covered this condition, the spec's §1.2 is wrong, and this task's premise needs re-examining — report it rather than adjusting the assertion.

Restore and confirm:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
git checkout -- rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs
git status --short   # the src/ path must be gone from the list
```

- [ ] **Step 5: Lint, format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd ..
git add rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "$(cat <<'EOF'
test(rs): assert that the idempotent put actually took the absorb path (SMA-660)

The join-based race asserted only two Oks and one row, which a racer on the
UPDATE path satisfies too. It now manufactures the race deterministically and
asserts that put skipped its policy_gen bump, which only the absorb path does.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Rewrite S1 and S2 in `tenancy_events_pg.rs`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs` — S1 inside `a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report` (around `:382-412` as the file stands now), S2 inside `a_concurrent_org_archive_is_reflected_in_a_racing_team_set_status_event` (around `:474-495`), and the `use std::time::Duration;` import at `:48`

**Every line number below is pre-edit.** Editing S1 shifts S2, so locate S2 by its test name, not by its quoted lines. The two anchors that never move are the `tokio::time::sleep(Duration::from_millis(300)).await;` line in each test and the `assert!(!handle.is_finished(), …)` line directly beneath it — those two lines, plus the comment paragraph above them, are what each site replaces.

**Interfaces:**
- Consumes: `support::race::{backend_pid, expect_racer_blocked}` from Task 1, plus `paigasus_iam::adapters::persistence::uow::recover_txn`.
- Produces: nothing later tasks call.

Spec §3.3. Both peers hold a `Box<dyn Transaction>` from `SeaOrmUnitOfWork::begin`, so the pid comes through `recover_txn` (spec F13). **Do not change how the peer transaction is opened.**

- [ ] **Step 1: Measure S1's real query string**

Before writing the final call, prove what the racer is actually running. In S1, replace the sleep and its assertion (`:402-407`, from the `// Give the spawned call real wall-clock time…` comment through the `assert!(!handle.is_finished(), …)` line) with a temporary probe:

```rust
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*peer_tx).unwrap()).await; // keep this line — step 2 needs it
    let probe = support::race::wait_until_blocked_by(&db, pid_peer, &handle, "zzz%", Duration::from_secs(2)).await; // MUTATION SMA-660 — delete this line
    panic!("PROBE S1: {probe:?}"); // MUTATION SMA-660 — delete this line
```

The probe takes `&handle`, so leave the racer's binding as the plain `let handle` it is today; Step 2 is what changes it to `let mut handle`.

Run it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_events_pg \
  -E 'test(a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report)' --retries 0
```

Expected: FAIL with `PROBE S1:` and a deadline message whose `non-idle backends:` dump contains the racer's query. **Record that query text in the measurements file.** It must start with `SELECT` (the spec's F3 says so, from `pg_memberships.rs:155-160`). If it starts with something else, the prefix in Step 2 changes to match what you measured — the measurement wins over the spec's table, and say so in the report.

- [ ] **Step 2: Write S1's real wait**

Delete the three lines marked `delete this line`, keep the `pid_peer` line (dropping its marker comment), and put the wait in place of the probe. The finished block reads:

```rust
    // Wait until this call is provably blocked INSIDE its own `DETACH_LOCK_SQL` — the
    // `SELECT … FOR UPDATE` at `pg_memberships.rs:155-160` — on the peer's uncommitted delete.
    // A fixed sleep here proved nothing: `!handle.is_finished()` is just as true for a racer
    // that has not started, so a late racer under CI load made this test pass without the race
    // ever happening (SMA-660).
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*peer_tx).unwrap()).await;
    support::race::expect_racer_blocked(&db, pid_peer, &mut handle, "select%", |r| format!("{r:?}")).await;
```

and the racer's binding at `:394` changes from `let handle = tokio::spawn(` to `let mut handle = tokio::spawn(`.

Note the `pid_peer` read must sit AFTER the peer's `detach_in` call (`:386`) — the transaction must already hold its locks — and before the wait.

- [ ] **Step 3: Run S1**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_events_pg \
  -E 'test(a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report)' --retries 0
```

Expected: PASS, in roughly the same time as before or faster — the wait returns on the first observation, where the sleep always cost 300 ms.

- [ ] **Step 4: Repeat Steps 1-3 for S2**

Same shape, at `:474-495`. The peer is `peer_tx` from `SeaOrmUnitOfWork::begin` again, holding an uncommitted org `set_status_in`; the racer is `svc.restore`, whose blocking statement is the org ancestor read `organization::Entity::find_by_id(..).lock_shared()` at `pg_teams.rs:266` — inside `set_status_in` (at `:242`), NOT inside `rename_in`. Probe it the same way, record the query, then write:

```rust
    // Wait until the racing `restore` is provably blocked INSIDE `set_status_in`'s org ancestor
    // read — the `.lock_shared()` at `pg_teams.rs:266` — on the peer's uncommitted org update.
    // Its earlier `lock_exclusive()` on the TEAM row (`:245`) is uncontested, so the ancestor
    // read is the only statement that can block on this peer. A fixed sleep proved nothing here
    // for the same reason as the test above (SMA-660).
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*peer_tx).unwrap()).await;
    support::race::expect_racer_blocked(&db, pid_peer, &mut handle, "select%", |r| format!("{r:?}")).await;
```

with `let mut handle = tokio::spawn(async move { svc.restore(team_id, &actor2).await });` at `:487`.

Run S2 alone and expect PASS.

- [ ] **Step 5: Remove the now-dead import and run the whole binary**

`use std::time::Duration;` at `:48` was used only by the two deleted sleeps. Delete the line if nothing else in the file uses `Duration`; grep first:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
grep -n "Duration" rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs
```

Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_events_pg --retries 0
```

Expected: PASS, whole binary.

- [ ] **Step 5b: COMMIT NOW, before any mutation**

Run the lint and format check from Step 10, then make Step 10's commit at this point rather than at the end of the task.

This ordering is not cosmetic, it is what makes the mutation steps safe. Steps 6-9 restore mutated files with `git checkout -- <path>`. While your new S1 and S2 are uncommitted, that command would restore `tenancy_events_pg.rs` to its PRE-TASK state and silently destroy your work. Once the new tests are committed, `git checkout --` restores exactly what you wrote, and the pre-task version is still reachable with `git show` when Step 8 needs it.

After this commit, Steps 6-9 must produce NO further commit: every mutation they make is reverted, so the tree returns to this commit each time.

- [ ] **Step 6: V7 — delete the lock S1 protects**

In `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs:160`, delete the ` FOR UPDATE` from `DETACH_LOCK_SQL` (the raw string then ends after the closing paren of line 159). Run S1 alone.

**Expected: FAIL at `expect_racer_blocked`**, with the deadline message — not later, at the `deleted.len()` assertion. Record the message. This is the evidence for acceptance criterion 2 at this site.

Restore: `git checkout -- rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs`, confirm with `git status --short`, re-run S1 and expect PASS.

- [ ] **Step 7: V7 — delete the lock S2 protects**

In `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_teams.rs:266`, delete the `.lock_shared()` call from `set_status_in`'s org ancestor read. Run S2 alone.

**Expected: FAIL at `expect_racer_blocked`.** Record the message. Restore with `git checkout --`, confirm clean, re-run and expect PASS.

- [ ] **Step 8: V8 — show the false green the old tests carried**

This is the evidence that the OLD tests could pass while the race did not happen, and it needs BOTH halves: the lock deleted AND the racer late. A late racer alone makes `!handle.is_finished()` more true, not less, so it would prove nothing on its own.

1. Materialize the PRE-TASK version of the test file into the working tree, without touching HEAD, the index, or the stash (the stash stack is shared across worktrees and another session may be using it — never `git stash` here):

```bash
# TASK4_BASE is the commit your Step 5b commit was made ON TOP OF
git show "$TASK4_BASE":rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs \
  > rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs
```

Your new tests are safe in the Step 5b commit, so this is reversible with a plain `git checkout -- rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs` when you are done.
2. Delete ` FOR UPDATE` from `DETACH_LOCK_SQL` as in Step 6, and insert into S1's spawned racer, as the first statement inside `tokio::spawn(async move {`:

```rust
        tokio::time::sleep(Duration::from_secs(1)).await; // MUTATION SMA-660 — delete this line
```

3. Run S1. **Expected: PASS.** That is the false green: the lock is gone, the race never happened, and the old test reports success. Record it.
4. Restore both files with `git checkout -- <path>` — the production file and the test file. Because of Step 5b, that returns the test file to YOUR version, not the pre-task one. Confirm `git status --short` is clean and `git log --oneline -1` still shows your Step 5b commit.
5. Repeat 2-4 for S2 with `.lock_shared()` and S2's racer. Expected: PASS on the old file both times.
6. Now the other half: with the NEW file and the locks still deleted, the same 1 s racer sleep must FAIL at the wait. That is Step 6 and Step 7's result plus a delay, so run one of the two — S1 — to confirm, and record it.

- [ ] **Step 9: V9 — a late racer must not break the new tests**

With production files clean and the new tests in place, insert the same 1 s sleep as the first statement of each spawned racer in S1 and S2, marked `// MUTATION SMA-660 — delete this line`. Run the whole binary.

**Expected: PASS.** The wait absorbs a late racer, which is the entire point of the change. Delete the marked lines and re-run to confirm PASS.

- [ ] **Step 10: Final check (the commit already happened at Step 5b)**

Confirm the tree is exactly your Step 5b commit with nothing left over from Steps 6-9:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd ..
git status --short   # no src/ path may appear
git add rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs
git commit -m "$(cat <<'EOF'
test(rs): make the two tenancy_events race tests wait for the racer to block (SMA-660)

Both slept 300 ms and then asserted the racer had not finished, which is also
true for a racer that never started. They now wait until pg_stat_activity shows
the racer blocked by the peer inside its own locking read.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

If Step 8 left a `wip:` commit, squash it into this one with `git rebase -i` or `git reset --soft HEAD~1` before committing, so the branch carries no WIP commit.

---

### Task 5: Rewrite S3 and S4 in `authz_system_retirement_pg.rs`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs` — S3 at `:645-647` (inside `a_concurrent_grant_blocks_then_reports_unknown_role`, `:604`), S4 at `:692-694` (inside `locking_the_policy_row_blocks_a_concurrent_role_insert`, `:668`)

**Every line number below is pre-edit**, and editing S3 shifts S4 — locate S4 by its test name. The stable anchor at each site is the three-line `tokio::time::timeout(Duration::from_millis(500), &mut handle)` / `.await` / `.expect_err("…")` block, together with the comment paragraph above it. Those are what each site replaces. Do not touch the LATER `tokio::time::timeout(Duration::from_secs(10), handle)` in either test — that one bounds a released waiter and stays.

**Interfaces:**
- Consumes: `support::race::{backend_pid, expect_racer_blocked}` from Task 1, plus `recover_txn`.
- Produces: nothing later tasks call.

Spec §3.3. Two differences from Task 4. The peer here is a `Box<dyn Transaction>` from `begin_retirement`, which carries a `SET LOCAL lock_timeout` — **do not replace it** to make the pid easier to read; use `recover_txn`. And this file has two connections: poll on `db_a`, the retirer's pool, which does not also hold the blocked racer.

- [ ] **Step 1: Measure S3's real query string**

Replace S3's `timeout(..).expect_err(..)` block (`:645-647`) with a probe:

```rust
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*tx).unwrap()).await;
    let probe = support::race::wait_until_blocked_by(&db_a, pid_peer, &handle, "zzz%", Duration::from_secs(2)).await; // MUTATION SMA-660 — delete this line
    panic!("PROBE S3: {probe:?}"); // MUTATION SMA-660 — delete this line
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_system_retirement_pg \
  -E 'test(a_concurrent_grant_blocks_then_reports_unknown_role)' --retries 0
```

Expected: FAIL with `PROBE S3:` and the dump. **Record the racer's query.** Per spec F3 it is the `INSERT INTO "role_grant"` from `grant_in` (`pg_role_grants.rs:214`), blocked by the FK check on the role row the retirer locked. If the measurement disagrees, the prefix follows the measurement.

- [ ] **Step 2: Write S3's real wait**

Delete the two marked lines and put the wait in their place:

```rust
    // Prove the grant actually BLOCKS, and prove it is blocked INSIDE its own INSERT (whose FK
    // check reads the role row this transaction holds FOR UPDATE) BY this transaction. The old
    // form — a 500 ms `timeout` that must expire — is also satisfied by a racer that has not
    // started yet, so under load it could pass with no race at all (SMA-660). Poll on `db_a`:
    // `pg_stat_activity` is instance-wide, and `db_b` is the pool holding the blocked racer.
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*tx).unwrap()).await;
    support::race::expect_racer_blocked(&db_a, pid_peer, &mut handle, "insert%", |r| format!("{r:?}")).await;
```

`handle` is already `let mut handle` at `:645`'s test, so no binding change is needed. The `pid_peer` read must come after `lock_role_in` (`:611`).

Run S3 alone. Expected: PASS, and faster than before — the old form always spent its full 500 ms.

- [ ] **Step 3: Repeat for S4**

Same shape inside `locking_the_policy_row_blocks_a_concurrent_role_insert` (`:668`), replacing `:692-694`. The peer is `tx` from `begin_retirement` holding `lock_policy_in`; the racer is `reconciler.reconcile_role`, whose blocking statement is its `INSERT INTO "role"` (`pg_system_roles.rs:45`) — its existence check at `:34` is a plain unlocked `find_by_id` and does not block. Probe, record, then:

```rust
    // Prove the role INSERT actually BLOCKS, and prove it is blocked inside that INSERT — whose
    // FK check reads the policy row this transaction holds FOR UPDATE — by this transaction.
    // `reconcile_role`'s own existence check is an unlocked `find_by_id`, so it never blocks and
    // the racer really does reach its INSERT. The old 500 ms `timeout` form was also satisfied
    // by a racer that had not started (SMA-660).
    let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*tx).unwrap()).await;
    support::race::expect_racer_blocked(&db_a, pid_peer, &mut handle, "insert%", |r| format!("{r:?}")).await;
```

Run S4 alone. Expected: PASS.

- [ ] **Step 4: Run the whole binary**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_system_retirement_pg --retries 0
```

Expected: PASS. Watch `a_contended_row_times_out_rather_than_hanging` (`:715`) in particular — it is untouched and must stay green; it asserts a real `lock_timeout` error rather than waiting on a racer.

Check whether `Duration` is still used in this file (it is — `begin_retirement(Duration::from_secs(5))` and the trailing `timeout(Duration::from_secs(10), handle)` both remain), so do NOT delete its import here.

- [ ] **Step 4b: COMMIT NOW, before any mutation**

Run Step 8's lint and format check, then make Step 8's commit at this point rather than at the end.

Same reason as Task 4's Step 5b, and it matters more here: Steps 5 and 6 mutate THIS TEST FILE (they comment out a `lock_*_in` call in the test body), and Step 7 inserts marked lines into it. While your S3/S4 rewrite is uncommitted, restoring any of that with `git checkout -- <path>` would destroy your own work. Once committed, `git checkout --` restores exactly what you wrote.

After this commit, Steps 5-7 must produce NO further commit: every mutation they make is reverted, so the tree returns to this commit each time.

- [ ] **Step 5: V7 — delete the lock S3 protects**

In S3's test body, comment out the `retirer.lock_role_in(&*tx, "legacy_auditor")` call at `:611` and its `.expect(..)`:

```rust
    // retirer.lock_role_in(&*tx, "legacy_auditor").await.unwrap().expect("seeded role row must be found"); // MUTATION SMA-660 — restore this line
```

Run S3 alone. **Expected: FAIL at `expect_racer_blocked`** with the deadline message, not at the later `UnknownRole` match. Record it. Restore the line and re-run to confirm PASS.

- [ ] **Step 6: V7 — delete the lock S4 protects**

Same, for `retirer.lock_policy_in(&*tx, "legacy_auditor")` inside `locking_the_policy_row_blocks_a_concurrent_role_insert`. Run S4 alone, **expect FAIL at the wait**, record, restore, re-run and expect PASS.

- [ ] **Step 7: V9 — a late racer must not break these tests**

Insert `tokio::time::sleep(Duration::from_secs(1)).await;` marked `// MUTATION SMA-660 — delete this line` as the first statement inside each of S3's and S4's `tokio::spawn(async move {` blocks. Run the whole binary. **Expected: PASS.** Delete the marked lines and re-run to confirm.

- [ ] **Step 8: Final check (the commit already happened at Step 4b)**

Confirm the tree is exactly your Step 4b commit with nothing left over from Steps 5-7:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd ..
git status --short   # no src/ path may appear
git add rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs
git commit -m "$(cat <<'EOF'
test(rs): make the two retirement lock tests wait for the racer to block (SMA-660)

Both asserted that a 500 ms timeout expires, which a racer that never started
also satisfies. They now wait until pg_stat_activity shows the racer blocked by
the retirement transaction inside its own INSERT.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Whole-crate verification and the measurements record

**Files:**
- Create: `docs/superpowers/plans/2026-09-19-sma-660-measurements.md`

**Interfaces:**
- Consumes: the recorded output of Tasks 2-5.
- Produces: the evidence the pull request body quotes.

- [ ] **Step 1: Run every affected binary together**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam \
  --test authz_policy_store --test tenancy_events_pg --test authz_system_retirement_pg --retries 0
```

Expected: PASS. Run it three times in a row and record the wall-clock time of each. The three runs are not a stress test; they are a cheap check that nothing here is order-dependent or flaky at the new waits.

- [ ] **Step 2: Run the crate's full test task the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
moon run paigasus-iam-rs:test --force
moon run paigasus-iam-rs:lint --force
```

Expected: both PASS. `--force` defeats the Moon cache, which would otherwise replay an older result. If `:test` fails in a binary this branch never touched, check whether it fails the same way on `main` before treating it as yours — `authz_policy_store.rs`'s neighbours include Docker-backed suites that are independently flaky under parallel load.

- [ ] **Step 3: Confirm no production file changed**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
git status --short
git diff --stat main...HEAD -- rs/crates/services/paigasus-iam/src/
```

Expected: `git status --short` clean, and the `src/` diff EMPTY. A non-empty `src/` diff means a mutation was never restored — find it and restore it before going further.

- [ ] **Step 4: Write the measurements file**

Create `docs/superpowers/plans/2026-09-19-sma-660-measurements.md` with this structure, filled in from what you recorded:

```markdown
# SMA-660 measurements

Every run used `PAIGASUS_REQUIRE_DOCKER=1` and `--retries 0`, from the worktree.

## The four measured query strings (spec V2)

| Site | Prefix used | Measured `query` (first 200 chars) |
| -- | -- | -- |
| S1 `tenancy_events_pg.rs` detach | `select%` | … |
| S2 `tenancy_events_pg.rs` restore | `select%` | … |
| S3 `authz_system_retirement_pg.rs` grant | `insert%` | … |
| S4 `authz_system_retirement_pg.rs` role insert | `insert%` | … |

State for each whether it agrees with the spec's F3 table. If one does not, say what changed.

## Mutations, and what each one reddened

| # | Mutation | Expected | Observed |
| -- | -- | -- | -- |
| V3 | the wait's first check made unconditionally true | guard fails at case 1 (a test stops at its first panic) | … |
| V4 | `query_prefix` neutralised to `%` at the bind site | guard case 3 alone fails | … |
| V5 | `pg_policies.rs` bumps unconditionally | absorb test fails, and NOTHING else | … |
| V6 | `pg_blocking_pids` term neutralised | guard case 4 fails | … |
| V7 S1 | `FOR UPDATE` deleted from `DETACH_LOCK_SQL` | S1 fails AT the wait | … |
| V7 S2 | `.lock_shared()` deleted from `set_status_in` | S2 fails AT the wait | … |
| V7 S3 | `lock_role_in` removed | S3 fails AT the wait | … |
| V7 S4 | `lock_policy_in` removed | S4 fails AT the wait | … |
| V8 | old file + lock deleted + 1 s late racer | S1 and S2 PASS — the false green | … |
| V9 | new file + 1 s late racer, locks intact | all five tests pass | … |

Quote the actual failure message for each V7 row: acceptance criterion 2 is about the MESSAGE,
not only about the failure.

## Timing

Three consecutive full runs of the three binaries: …
```

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-660-race-lock-waits
git add docs/superpowers/plans/2026-09-19-sma-660-measurements.md docs/superpowers/plans/2026-09-19-sma-660-race-test-lock-waits.md
git commit -m "$(cat <<'EOF'
docs(rs): record the SMA-660 plan and its measurements (SMA-660)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

## What CI will check that this plan cannot

Acceptance criterion 3's remote half is the `nextest-junit` artifact from `paigasus-iam-rs:test` (profile `iam`). The four rewritten tests, the absorb test and the guard test must carry no `flakyFailure` and no `rerunFailure` element. A green task alone is not enough: `rs/.config/nextest.toml` gives every test in this crate `retries = 2`, so a test that failed once and passed on retry still reports a green task. Check the artifact on the pull request before calling the work done.
