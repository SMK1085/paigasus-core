# SMA-659 authz_policy_store race-test wait Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The three race tests in `authz_policy_store.rs` wait until racer B is provably blocked inside its INSERT on racer A, instead of sleeping 200 ms, and fail with a clear message when B does not block.

**Architecture:** One test-only helper, `wait_until_blocked_by`, polls `pg_stat_activity` (autocommit, every 10 ms, 30 s budget) for a backend that runs an INSERT and is blocked by A's backend pid on a `transactionid` lock. A thin wrapper, `expect_racer_blocked`, turns its `Err` into a panic that carries B's own result. A guard test drives both error paths. No production code changes.

**Tech Stack:** Rust 2024, tokio, SeaORM 2.0 (`ConnectionTrait::query_one_raw`), Postgres 16 in testcontainers, cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-659-authz-race-tests-design.md`

## Global Constraints

- Only `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` changes (plus this plan). No production code, no `tests/support/` change (the move is SMA-660).
- `RACER_BLOCK_BUDGET = 30 s`. Poll interval 10 ms. Guard deadline case uses a 200 ms budget.
- Error message substrings are asserted: `finished before it blocked`, `did not block`, `non-idle backends`. Keep them exact.
- Every local test run: `PAIGASUS_REQUIRE_DOCKER=1` and `--retries 0`. Docker must be running.
- Every mutation is a marked insert (`// SMA-659-MUTATION`) and is undone by deleting the marked lines. Never `git checkout --` a file with uncommitted work.
- Commits: conventional, scope `rs`, key at the END of the subject, e.g. `test(rs): … (SMA-659)`. End every message with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Add new commits; never `--amend`.
- Bash PATH prefix for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.

## Execution notes for subagents

- Your cwd may be pinned to the MAIN checkout. First action: `EnterWorktree` with path
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-659-authz-race-tests`, then
  confirm `git branch --show-current` prints `feature/sma-659-authz-race-tests`. Stop if it does not.
- Run every command in the foreground. Do not end your turn while a background job runs.
- The test command, from the worktree root:
  ```bash
  cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0
  ```
  Add `-E 'test(<name>)'` to run one test.

---

### Task 1: The wait helper and its guard test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` (imports at `:28-35`; new items after `seed_system_policy`, which ends at `:69`; new test at the end of the file)

**Interfaces:**
- Produces (used by Task 2):
  - `const RACER_BLOCK_BUDGET: Duration`
  - `async fn backend_pid(conn: &impl ConnectionTrait) -> i32`
  - `async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, budget: Duration) -> Result<(), String>`
  - `async fn expect_racer_blocked<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &mut JoinHandle<T>, describe: impl FnOnce(&T) -> String)`

- [ ] **Step 1: Write the guard test (it does not compile yet)**

Deviation from spec §3.3, on purpose: the spec's "finished" case begins A, INSERTs and commits it by hand. Here A's row is committed through `store.put`, and `pid_a` is any pool backend. The effect is the same (a committed row, and a pid that nothing waits on), with less code.

Append to the end of `authz_policy_store.rs`:

```rust
/// SMA-659 guard: [`wait_until_blocked_by`] must REPORT a racer that never blocks, on both of its
/// error paths. Without this, a wait that always returned `Ok` would keep the three race tests
/// green while the race went back to depending on timing — which is the defect SMA-659 removes.
/// (The opposite defect, a predicate that is never true, makes those three tests fail at the
/// deadline, so they guard it themselves.)
#[tokio::test]
async fn wait_until_blocked_by_reports_a_racer_that_never_blocks() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    // "Finished" path. A's row is COMMITTED before B starts, so B's existence check sees it and
    // B takes the UPDATE path: it never waits on any lock and simply finishes.
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc_a = valid_static_doc("guard-never-blocks", false, now);
    store.put(&doc_a).await.unwrap();
    let pid_a = backend_pid(&db).await;
    let mut doc_b = doc_a.clone();
    doc_b.description = "racer B's document".to_string();
    let store_b = store.clone();
    let put_b = tokio::spawn(async move { store_b.put(&doc_b).await });

    let err = wait_until_blocked_by(&db, pid_a, &put_b, RACER_BLOCK_BUDGET)
        .await
        .expect_err("a racer that takes the UPDATE path never blocks, so the wait must report it");
    assert!(err.contains("finished before it blocked"), "wrong message for a finished racer: {err}");
    // The guard proves the UPDATE-path case specifically, not some racer that died of an error.
    let result_b = put_b.await.unwrap();
    assert!(result_b.is_ok(), "racer B must have finished through the ordinary UPDATE path: {result_b:?}");

    // "Deadline" path. A racer that neither blocks nor finishes: the wait must give up at its
    // budget and say so, with the backend dump that tells the reader what was running instead.
    let pending = tokio::spawn(std::future::pending::<()>());
    let err = wait_until_blocked_by(&db, pid_a, &pending, Duration::from_millis(200))
        .await
        .expect_err("a racer that never blocks must hit the deadline");
    pending.abort();
    assert!(err.contains("did not block"), "wrong message at the deadline: {err}");
    assert!(err.contains("non-idle backends"), "the deadline message must carry the backend dump: {err}");
}
```

- [ ] **Step 2: Run it and confirm it fails to compile**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 -E 'test(wait_until_blocked_by_reports)'`
Expected: compile error, `cannot find function 'backend_pid'` / `wait_until_blocked_by` / `RACER_BLOCK_BUDGET`, and `Duration` not in scope.

- [ ] **Step 3: Add the imports**

Change the `sea_orm` import line (`:35`) and add two imports after it:

```rust
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement, TransactionTrait};
use std::time::Duration;
use tokio::task::JoinHandle;
```

- [ ] **Step 4: Add the helpers after `seed_system_policy` (after `:69`)**

```rust
/// Upper bound on how long racer B may take to block on racer A's uncommitted INSERT (SMA-659).
/// A LOAD BUDGET, not an expectation: the wait returns on the first observation, which on an idle
/// machine is the first or second poll.
const RACER_BLOCK_BUDGET: Duration = Duration::from_secs(30);

/// How often [`wait_until_blocked_by`] polls `pg_stat_activity`.
const RACER_BLOCK_POLL: Duration = Duration::from_millis(10);

/// Counts backends that are inside an INSERT and blocked by `$1` on a transaction-id lock. That is
/// exactly racer B once it is in its INSERT: its `SELECT … FOR UPDATE` existence check does not
/// wait on A's uncommitted, invisible row, but its unique-index check does wait on A's
/// transaction id (spec §2, F1–F2). The `ILIKE 'insert%'` term makes the observation itself show
/// that B is in the INSERT, rather than relying on F1 alone.
const BLOCKED_INSERTS_SQL: &str = "SELECT count(*)::bigint AS n FROM pg_stat_activity \
     WHERE wait_event_type = 'Lock' AND wait_event = 'transactionid' \
     AND query ILIKE 'insert%' AND $1 = ANY(pg_blocking_pids(pid))";

/// Every non-idle client backend but the one running this query, for a deadline message: a stuck
/// pool, a different lock and a wrong predicate each look different here.
const NON_IDLE_BACKENDS_SQL: &str = "SELECT coalesce(string_agg(format('pid=%s state=%s wait=%s/%s query=%s', \
     pid, state, wait_event_type, wait_event, left(query, 80)), '; ' ORDER BY pid), '(none)') AS dump \
     FROM pg_stat_activity \
     WHERE backend_type = 'client backend' AND state IS DISTINCT FROM 'idle' AND pid <> pg_backend_pid()";

/// The backend pid of the connection that runs `conn` — for a transaction, the backend that holds
/// its locks.
async fn backend_pid(conn: &impl ConnectionTrait) -> i32 {
    conn.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT pg_backend_pid() AS pid"))
        .await
        .expect("query pg_backend_pid()")
        .expect("pg_backend_pid() always returns a row")
        .try_get::<i32>("", "pid")
        .expect("pg_backend_pid() is an integer")
}

/// SMA-659: waits until racer B is inside its INSERT and blocked by `blocker_pid` (racer A's
/// backend), which replaces a fixed sleep that only HOPED B had got there. Each poll is an
/// autocommit statement on `db`: `pg_stat_activity` is a per-transaction snapshot, so a poll
/// inside one transaction would see the same data every time (spec §2, F3).
///
/// Checks, in this order, every [`RACER_BLOCK_POLL`]: B blocked → `Ok`; `racer` finished →
/// `Err` (a blocked racer cannot finish while A is uncommitted, so checking "blocked" first
/// never hides a finished one); `budget` elapsed → `Err` with a dump of the non-idle backends.
/// Panics with `observer query failed: …` if the poll itself fails, which is neither of the two
/// verdicts.
async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, budget: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let blocked = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, BLOCKED_INSERTS_SQL, [blocker_pid.into()]))
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
                "racer B finished before it blocked on racer A's uncommitted INSERT (pid {blocker_pid}), so the race never happened"
            ));
        }
        if std::time::Instant::now() >= deadline {
            let dump = match db.query_one_raw(Statement::from_string(DbBackend::Postgres, NON_IDLE_BACKENDS_SQL)).await {
                Ok(Some(row)) => row.try_get::<String>("", "dump").unwrap_or_else(|e| format!("(dump unreadable: {e})")),
                Ok(None) => "(dump returned no row)".to_string(),
                Err(e) => format!("(dump failed: {e})"),
            };
            return Err(format!(
                "racer B did not block on racer A's uncommitted INSERT (pid {blocker_pid}) within {budget:?}; non-idle backends: {dump}"
            ));
        }
        tokio::time::sleep(RACER_BLOCK_POLL).await;
    }
}

/// The race tests' call site for [`wait_until_blocked_by`] with [`RACER_BLOCK_BUDGET`]: panics
/// with the wait's reason when B does not block, so the test stops HERE and never reaches its
/// `Conflict`/`Updated` verdict, which would be meaningless without the race. When B has already
/// finished, B's own result is part of the message (through `describe`, since a racer's output
/// can hold a non-`Debug` transaction), so a `DbErr` or a panic inside B is not hidden behind
/// "finished".
async fn expect_racer_blocked<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &mut JoinHandle<T>, describe: impl FnOnce(&T) -> String) {
    let Err(reason) = wait_until_blocked_by(db, blocker_pid, racer, RACER_BLOCK_BUDGET).await else { return };
    if racer.is_finished() {
        let own = match racer.await {
            Ok(output) => describe(&output),
            Err(join_err) => format!("{join_err:?}"),
        };
        panic!("{reason}; racer B's own result: {own}");
    }
    racer.abort();
    panic!("{reason}");
}
```

Note: `expect_racer_blocked` has no caller until Task 2. The workspace denies warnings, and dead
code in a test target is a warning. If the build fails with `function 'expect_racer_blocked' is
never used`, add `#[allow(dead_code)] // SMA-659: used from Task 2` above it for this commit and
remove that line in Task 2.

- [ ] **Step 5: Run the guard test and confirm it passes**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 -E 'test(wait_until_blocked_by_reports)'`
Expected: `1 test run: 1 passed`. The test takes a few seconds (container start plus 200 ms).

- [ ] **Step 6: Mutation V3 — the guard must catch a wait that always says `Ok`**

Insert as the FIRST line of `wait_until_blocked_by`'s body:

```rust
    return Ok(()); // SMA-659-MUTATION
```

(An `unreachable_code` warning may break the build under `warnings = "deny"`. If it does, put
`#[allow(unreachable_code)] // SMA-659-MUTATION` on the line above the `async fn`.)

Run the Step 5 command. Expected: FAIL, panic at the first `expect_err` with
`a racer that takes the UPDATE path never blocks, so the wait must report it`.
Then delete every line that contains `SMA-659-MUTATION`. Confirm `grep -n SMA-659-MUTATION rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` prints nothing, and re-run Step 5: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "test(rs): add a bounded lock-wait probe for the authz race tests (SMA-659)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The three race tests wait on the probe, not on 200 ms

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` — the three tests
  `concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict` (sleep at
  `:347-349` before Task 1's insert shifted it), `put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable`
  (`:432-434`), `put_in_surfaces_a_different_content_savepoint_conflict_and_the_outer_txn_stays_usable`
  (`:507-509`), and each test's doc comment where it says the race is manufactured by committing
  "once B's put is in flight". Find them by name, not by line number.

**Interfaces:**
- Consumes: `backend_pid`, `expect_racer_blocked` from Task 1.

- [ ] **Step 1: Red — reproduce the CI failure with the OLD sleep (spec V2)**

In each of the three tests, insert as the FIRST statement inside racer B's `tokio::spawn(async move { … })` block:

```rust
        tokio::time::sleep(Duration::from_secs(1)).await; // SMA-659-MUTATION
```

For test 1 the spawn is a one-liner; expand it to
`tokio::spawn(async move {\n tokio::time::sleep(Duration::from_secs(1)).await; // SMA-659-MUTATION\n store_b.put(&doc_b).await\n })`.

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 -E 'test(concurrent_put_of_the_same_new_policy_id_with_different_content) | test(put_in_absorbs_a_same_content) | test(put_in_surfaces_a_different_content)'`
Expected: 3 FAILED, with exactly the CI verdicts:
- test 1: `the losing racer must see AuthzError::Conflict, not a silent Ok …: Ok(())`
- `put_in_absorbs…`: `expected AbsorbedIdempotent, got Updated`
- `put_in_surfaces…`: `… not a silent absorb: Ok(Updated)`

Record the three messages for the report. Keep the three marked lines in place for Step 4.

- [ ] **Step 2: Test 1 — replace the sleep**

In `concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict`:

a) Directly after racer A's `.insert(&txn_a).await.unwrap();`, add:

```rust
    let pid_a = backend_pid(&txn_a).await;
```

b) Change `let put_b = tokio::spawn(` to `let mut put_b = tokio::spawn(`.

c) Replace the three lines

```rust
    // Give racer B's task time to actually reach (and block inside) its own INSERT before A
    // commits — comfortably longer than a local Postgres round-trip even under load.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
```

with

```rust
    // Commit A only once B is provably blocked INSIDE its INSERT on A's uncommitted row — past its
    // existence check, so its INSERT must resolve into a real unique violation. A fixed sleep here
    // only hoped for that and failed under CI load (SMA-659): a late B saw A's committed row and
    // took the UPDATE path instead.
    expect_racer_blocked(&db, pid_a, &mut put_b, |r| format!("{r:?}")).await;
```

d) In the test's doc comment, replace `then commits A once B's \`put\` (the\n/// real, unmodified production method) is in flight,` with `then commits A once B's \`put\` (the\n/// real, unmodified production method) is observed blocked on it (\`expect_racer_blocked\`),`. Keep the rest of the comment.

- [ ] **Step 3: Tests 2 and 3 — replace the sleep**

In BOTH `put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable` and
`put_in_surfaces_a_different_content_savepoint_conflict_and_the_outer_txn_stays_usable`:

a) Directly after racer A's `.insert(&txn_a).await.unwrap();`, add:

```rust
    let pid_a = backend_pid(&txn_a).await;
```

b) Change `let put_b = tokio::spawn(` to `let mut put_b = tokio::spawn(`.

c) Replace the three lines

```rust
    // Give racer B's task time to actually reach (and block inside) its own savepoint INSERT
    // before A commits — comfortably longer than a local Postgres round-trip even under load.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
```

with

```rust
    // Commit A only once B is provably blocked INSIDE its savepoint INSERT on A's uncommitted row
    // (SMA-659 — a fixed sleep here only hoped for that, and a late B took the UPDATE path).
    expect_racer_blocked(&db, pid_a, &mut put_b, |(_, outcome)| format!("{outcome:?}")).await;
```

d) In `put_in_surfaces…`'s doc comment, replace `then commits A once B's \`put_in\` is in flight,` with
`then commits A once B's \`put_in\` is observed blocked on it (\`expect_racer_blocked\`),`.
`put_in_absorbs…`'s doc comment says only "manufactures the race deterministically"; leave it.

e) If Task 1 added `#[allow(dead_code)] // SMA-659: used from Task 2` above `expect_racer_blocked`, delete that line now.

- [ ] **Step 4: Green with the delay still in place (spec V2, second half)**

Run the Step 1 command again, with the three `SMA-659-MUTATION` lines still present.
Expected: 3 passed. B now starts 1 s late, and each test waits for it instead of committing A early.

- [ ] **Step 5: Remove the delay and run the whole binary**

Delete every line that contains `SMA-659-MUTATION`; for test 1 restore the one-line spawn
`tokio::spawn(async move { store_b.put(&doc_b).await })`. Confirm
`grep -n 'SMA-659-MUTATION\|from_millis(200)' rs/crates/services/paigasus-iam/tests/authz_policy_store.rs`
prints only the guard test's `Duration::from_millis(200)` budget line.

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0`
Expected: all tests pass (17 tests: the 16 existing ones plus the guard).

- [ ] **Step 6: Format and lint**

Run: `cd rs && cargo fmt -p paigasus-iam && cargo fmt --check` — expected: no output, rc 0.
Run (from the worktree root): `moon run paigasus-iam-rs:lint` — expected: passes (clippy with `--all-targets -D warnings`).

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
git commit -m "test(rs): make the authz_policy_store race tests wait for the racer to block (SMA-659)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Stress verification under CPU load (spec V4)

The controller runs this task, not an implementer subagent: it takes about 30–40 minutes and
changes no code.

**Files:** none changed. Results go into the PR body.

- [ ] **Step 1: Confirm a clean, committed tree**

Run: `git status --porcelain` — expected: empty. (Step 3 swaps the file and restores it with `git checkout`; that is safe only on a clean tree.)

- [ ] **Step 2: Start the load generator**

```bash
: > "$SCRATCH/load.pids"; N=$(sysctl -n hw.ncpu); for _ in $(seq "$N"); do nohup yes > /dev/null 2>&1 & echo $! >> "$SCRATCH/load.pids"; done
wc -l < "$SCRATCH/load.pids"   # expected: the core count
```

(`$SCRATCH` is the session scratchpad directory.)

- [ ] **Step 3: Baseline — the OLD file under load, 25 runs**

```bash
git show origin/main:rs/crates/services/paigasus-iam/tests/authz_policy_store.rs > rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
cd rs && for i in $(seq 25); do PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 > "$SCRATCH/base-$i.log" 2>&1; echo "base $i rc=$?"; done; cd ..
git checkout HEAD -- rs/crates/services/paigasus-iam/tests/authz_policy_store.rs
```

Classify every non-zero run from its log: an assertion failure (`must see AuthzError::Conflict`, `got Updated`) or an infrastructure failure (container start or port publish timeout). Record both counts. If the assertion count is 0, write that down: V4 then shows no regression only, and Task 2 Step 1/4 is the proof of the fix.

- [ ] **Step 4: The NEW file under load, 50 in a row**

```bash
cd rs && ok=0; run=0; while [ "$ok" -lt 50 ]; do run=$((run+1)); PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_policy_store --retries 0 > "$SCRATCH/new-$run.log" 2>&1; rc=$?; echo "new run $run rc=$rc ok=$ok"; if [ $rc -eq 0 ]; then ok=$((ok+1)); else break; fi; done; cd ..
```

On a failure, read the log. Counting rule (spec V4): an assertion failure or a `did not block`/`finished before it blocked` failure resets the count to zero and is a real finding — stop and report it. An infrastructure failure is reported with its message, does not count as a run, and does not reset the count: re-enter the loop with `ok` kept.

- [ ] **Step 5: Stop the load generator**

```bash
kill $(cat "$SCRATCH/load.pids")
```

- [ ] **Step 6: Record** the baseline counts, the 50-run result, and every infrastructure failure message, for the PR body.
