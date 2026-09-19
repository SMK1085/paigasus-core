# SMA-659: the authz_policy_store race tests wait on a condition, not on 200 ms

Linear: SMA-659. Blocks SMA-629 (PR 266, already merged as `7684b8f3`). Related: SMA-521.

## 1. Problem

Three tests in `rs/crates/services/paigasus-iam/tests/authz_policy_store.rs` make a Postgres
INSERT-vs-INSERT race:

| Test | Sleep | Late-B verdict in CI |
| -- | -- | -- |
| `concurrent_put_of_the_same_new_policy_id_with_different_content_is_a_conflict` | `:349` | `Ok(())` |
| `put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable` | `:434` | `got Updated` |
| `put_in_surfaces_a_different_content_savepoint_conflict_and_the_outer_txn_stays_usable` | `:509` | `Ok(Updated)` |

Each test holds racer A's INSERT open in an uncommitted transaction, spawns racer B (the real
`PgPolicyStore::put` or `put_in`), sleeps a fixed 200 ms, and then commits A. The sleep only
hopes that B has reached its INSERT. On a loaded CI runner B can still be before its existence
check when A commits. B then sees A's committed row and takes the UPDATE path. `put` returns
`Result<(), AuthzError>`, so test 1 shows `Ok(())`; the two `put_in` tests show `Updated`.

CI run 35454491833 has eight failure lines for these tests. All eight are one of the three
`Ok`/`Updated` verdicts above. None is an `Err(Backend(..))`. So the cause above explains every
observed failure. The nextest retry budget (SMA-521) does not help, because each retry meets
the same load.

Why B is now late on every attempt is NOT measured. Two candidates: B opens a new pool
connection (SCRAM authentication) before its existence check, and B's `validate_policy` can be
the first parse of the Cedar schema in the process (`OnceLock`,
`rs/crates/libs/paigasus-iam-core/src/authz/schema.rs:32-35`). Both are CPU work in a debug
build. The design does not depend on which one it is.

## 2. Facts the design depends on

Postgres behaviour (Postgres 16, READ COMMITTED; confirmed by the spec review, not measured):

- F1. B's existence check is `SELECT … LIMIT 1 FOR UPDATE`
  (`src/adapters/persistence/pg_policies.rs:290`). A's row is uncommitted, so it is not visible
  to B's snapshot, and the scan drops it before the lock step. So this statement does NOT wait
  on A.
- F2. B's INSERT is a plain `INSERT … RETURNING` on a SAVEPOINT (`pg_policies.rs:317`), with no
  `ON CONFLICT`. Its unique-index check finds A's in-progress entry and waits on A's top-level
  transaction id. In `pg_stat_activity` this is `wait_event_type = 'Lock'`,
  `wait_event = 'transactionid'`, and `pg_blocking_pids(B)` contains A's backend pid. The
  savepoint does not change this.
- F3. `pg_stat_activity` is a snapshot per transaction: inside one transaction every read shows
  the same data. So the observer must poll with autocommit statements.

Test harness (read from the code):

- F4. Each test starts its own Postgres container (`support::start_migrated_postgres`). Nothing
  else in the test holds or waits on A's transaction.
- F5. The pool is sqlx's default through SeaORM: `max_connections = 10`,
  `acquire_timeout = 30 s`. A, B and the observer need three connections.
- F6. `rs/.config/nextest.toml` sets no slow-test timeout, and gives every `paigasus-iam`
  integration test `retries = 2`.

## 3. Design

### 3.1 One helper, in the test file

Add to `authz_policy_store.rs`:

```rust
/// Upper bound on how long a racer may take to block on the other racer's uncommitted INSERT.
/// A LOAD BUDGET, not an expectation: the wait returns on the first observation.
const RACER_BLOCK_BUDGET: Duration = Duration::from_secs(30);

/// The backend pid of the connection that runs `txn` (`SELECT pg_backend_pid()`).
async fn backend_pid(txn: &impl ConnectionTrait) -> i32;

/// Waits until some backend runs an INSERT that is blocked by `blocker_pid` on a
/// `transactionid` lock, that is until racer B is inside its INSERT (F1, F2). Each poll is an
/// autocommit statement on `db` (F3). Returns `Err(message)` if `racer` finishes first or if
/// `budget` elapses first. Panics with `observer query failed: …` if the poll itself fails.
async fn wait_until_blocked_by<T>(db: &DatabaseConnection, blocker_pid: i32, racer: &JoinHandle<T>, budget: Duration) -> Result<(), String>;
```

The observer query:

```sql
SELECT count(*)::bigint AS n
FROM pg_stat_activity
WHERE wait_event_type = 'Lock' AND wait_event = 'transactionid'
  AND query ILIKE 'insert%'
  AND $1 = ANY(pg_blocking_pids(pid))
```

The `query ILIKE 'insert%'` term shows directly that B is inside an INSERT, so the predicate
does not rely on F1 alone.

The loop checks, in this order, every 10 ms:

1. If the observer query finds such a backend, return `Ok(())`.
2. If `racer.is_finished()`, return `Err("racer B finished before it blocked on racer A's
   uncommitted INSERT (pid {blocker_pid}), so the race never happened")`.
3. If the deadline has passed, return `Err("racer B did not block on racer A's uncommitted INSERT
   (pid {blocker_pid}) within {budget:?}; non-idle backends: {dump}")`. `{dump}` lists every
   non-idle backend from `pg_stat_activity` (pid, state, wait_event_type, wait_event,
   `left(query, 80)`), so a stuck pool, a different lock and a wrong predicate each give a
   different message.

A blocked racer cannot finish while A is uncommitted, so step 1 first cannot hide a finished
racer. Step 2 before step 3 gives the more exact message when both hold.

### 3.2 The three tests

In each test:

1. After A's INSERT on `txn_a`, read `let pid_a = backend_pid(&txn_a).await;`.
2. Replace the 200 ms sleep and its comment with the wait, and a comment that says what the wait
   proves (B passed its existence check and is inside its INSERT).
3. On `Err(e)`: if the racer is finished, await it and panic with `{e}` plus the racer's result
   (or its panic payload), so the reason B finished is in the message. Otherwise panic with `{e}`
   alone. A small shared function does this, so the three tests stay identical at this point.
4. Commit A, as now. Everything after is unchanged.

So the test fails at the wait with a message that says B did not block, and never reaches the
`Conflict`/`Updated` assertion (AC 2). The tests' intent does not change: B still takes the
INSERT path and meets a real unique violation. The wait now proves that, where the sleep only
hoped so.

### 3.3 A guard test for both failure paths

A wait that always returns `Ok` would pass all three tests while the race goes back to hope.
One test, one container, drives the helper into both error paths:

- `wait_until_blocked_by_reports_a_racer_that_never_blocks`:
  1. "Finished" path: begin A, INSERT, read `pid_a`, commit A FIRST, then spawn B's `put` with
     different content. B takes the UPDATE path and never blocks. Assert `Err` whose message
     contains `finished before it blocked`. Then await B and assert `Ok(())`, so the guard
     proves the UPDATE-path case and not some error.
  2. "Deadline" path: call the helper with a racer `tokio::spawn(std::future::pending::<()>())`,
     the same `pid_a`, and a 200 ms budget. Nothing blocks on it. Assert `Err` whose message
     contains `did not block`. Abort the pending task.

The three race tests catch the opposite defect: a predicate that is never true makes them fail
at the deadline.

### 3.4 Out of scope, with a follow-up

- `tests/tenancy_events_pg.rs:406` and `:492`, and `tests/authz_system_retirement_pg.rs:692`,
  use the same "sleep until the racer reaches the lock" pattern. Their checks
  (`!handle.is_finished()`, `timeout(..).expect_err`) are also true when the racer has not
  started yet, so under load they can pass without the race. They are not failing today.
- `concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict` (`:277`) says one
  racer absorbs a conflict, but does not assert it; with `tokio::join!` B can take the UPDATE
  path and the test stays green.

Both go into ONE follow-up Linear issue, SMA-660. The helper stays in this file for now. The follow-up
moves it to `tests/support/` when it gets a second user. The other fixed sleeps in this crate
(`relay_pg.rs`, `relay_nudge_pg.rs`, …) are not audited here.

## 4. Verification

Every local run below starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`, so
`cargo nextest` and `moon` resolve to the repository-pinned tools rather than to a global binary.
Every local run below also uses `PAIGASUS_REQUIRE_DOCKER=1` and `--retries 0`: a filtered run
without Docker skips silently, and a retry can turn a failing mutation green. Every mutation is
a marked insert and is undone by deleting the marked lines, never by `git checkout`.

- V1. `cargo nextest run -p paigasus-iam --test authz_policy_store` passes, the guard test
  included.
- V2. Deterministic reproduction of the CI failure. Insert
  `tokio::time::sleep(Duration::from_secs(1)).await;` as the first statement of racer B's spawned
  task in test 1 (test code only). Against the OLD file (200 ms sleep) test 1 must fail with
  `Ok(())`. Against the NEW file it must pass. This proves the fix removes the failure CI saw.
- V3. Mutation: make `wait_until_blocked_by` return `Ok(())` at once. The guard test must fail.
- V4. Stress (AC 3, local half).
  1. Baseline: run the OLD binary under the same load (below) and record the assertion-failure
     count. If it is zero, write that down: V4 then shows no regression, and V2 is the proof of
     the fix.
  2. Run the NEW binary 50 times in a row under load.
  Load: one `yes > /dev/null` per core. Counting rule: an assertion failure resets the count to
  zero. An infrastructure failure (container start or port publish timeout) is reported with its
  message, does not count as a run, and does not reset the count.
- V5. CI (AC 3, remote half). PR 266 is merged, so it cannot run again. This PR edits a
  `paigasus-iam` test file, so it selects `paigasus-iam-rs:test`. The evidence is the
  `nextest-junit` artifact (profile `iam`): the three race tests and the guard test must have no
  `flakyFailure` and no `rerunFailure` element. A green task alone is not enough (F6).
- V6. `moon run paigasus-iam-rs:lint` (clippy with `--all-targets`, so the test file is linted)
  and `cargo fmt --check` stay clean.

## 5. Acceptance criteria (from the issue)

1. No test in `authz_policy_store.rs` waits a fixed time for racer B. (§3.2)
2. If B does not block within the deadline, the test fails with a message that says so. (§3.1,
   §3.2, guarded by §3.3)
3. 50 local runs in a row under CPU load (V4), and `paigasus-iam-rs:test` green without a retry
   in CI on this PR (V5; the issue named PR 266, which is merged).
