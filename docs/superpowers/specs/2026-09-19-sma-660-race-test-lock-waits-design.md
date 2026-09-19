# SMA-660: four more race tests wait on a condition, and the absorb path gets an assertion

Linear: SMA-660. Follows SMA-659 (PR 269, merged as `faece64a`), whose §3.4 names this work.

## 1. Problem

SMA-659 replaced a fixed 200 ms sleep in three `authz_policy_store.rs` race tests with
`wait_until_blocked_by`, a bounded poll of `pg_stat_activity`. Four more sites in
`rs/crates/services/paigasus-iam/tests/` have the same defect, and one test asserts less than
its doc comment claims.

### 1.1 The four sites that wait a fixed time

Each holds a peer's statement open in an uncommitted transaction, spawns the racer, waits a
fixed time, and then checks something that is **also true when the racer has not started yet**:

| # | Site | Test | Wait | Check after the wait |
| -- | -- | -- | -- | -- |
| S1 | `tenancy_events_pg.rs:406` | `a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report` | `sleep(300 ms)` | `assert!(!handle.is_finished())` |
| S2 | `tenancy_events_pg.rs:492` | `a_concurrent_org_archive_is_reflected_in_a_racing_team_set_status_event` | `sleep(300 ms)` | `assert!(!handle.is_finished())` |
| S3 | `authz_system_retirement_pg.rs:645` | `a_concurrent_grant_blocks_then_reports_unknown_role` (`:604`) | `timeout(500 ms, &mut handle)` | `.expect_err(..)` |
| S4 | `authz_system_retirement_pg.rs:692` | `locking_the_policy_row_blocks_a_concurrent_role_insert` (`:668`) | `timeout(500 ms, &mut handle)` | `.expect_err(..)` |

The issue names S1, S2 and S4. S3 is the same shape for the same reason and is in scope by
decision D1 below.

So under CI load each test can pass without the race happening, and a regression that removes
the lock under test can pass CI. None of the four fails today. The risk is a false green, not a
red.

### 1.2 `put`'s bump-skip on the absorb path has no assertion

`authz_policy_store.rs:379`, `concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict`,
says in its doc comment that one racer takes the INSERT path and the other absorbs the
unique-constraint violation. It asserts neither. It drives two `put` calls under `tokio::join!`,
so racer A's whole `put` can commit before B's existence check; B then takes the ordinary UPDATE
path, both calls return `Ok(())`, one row exists, and the test is green without ever reaching
`PutOutcome::AbsorbedIdempotent`.

State the gap exactly, because one part of it is already covered.
`put_in_absorbs_a_same_content_savepoint_conflict_and_the_outer_txn_stays_usable` (`:500`)
already drives the absorb deterministically and asserts `PutOutcome::AbsorbedIdempotent`
directly. What no test covers is the **`put` wrapper's** behaviour on that outcome: `put` skips
its `policy_gen` bump when, and only when, the outcome is `AbsorbedIdempotent`
(`pg_policies.rs:257-259`). That one condition is the marginal coverage this spec adds. V5
proves the claim is true rather than asserting it: if another test already covers that
condition, V5's mutation reds that test too and this paragraph is wrong.

## 2. Facts the design depends on

Read from the source at `faece64a`. Where a fact is reasoned rather than measured, it says so,
and §5 turns it into a measurement.

- **F1.** The SMA-659 helper lives in `authz_policy_store.rs:73-171`: `RACER_BLOCK_BUDGET`
  (30 s), `RACER_BLOCK_POLL` (10 ms), `BLOCKED_INSERTS_SQL`, `NON_IDLE_BACKENDS_SQL`,
  `backend_pid`, `wait_until_blocked_by`, `expect_racer_blocked`.
- **F2.** `BLOCKED_INSERTS_SQL` matches `wait_event_type = 'Lock' AND wait_event =
  'transactionid' AND query ILIKE 'insert%' AND $1 = ANY(pg_blocking_pids(pid))`. The
  `ILIKE 'insert%'` term exists to show that racer B is *past* its existence check and inside
  the statement that must block. That statement is site-specific, so the term is too.
- **F3.** The blocked statement per site, read from the production code:

  | Site | Racer | Statement that blocks | Blocked by |
  | -- | -- | -- | -- |
  | S1 | `detach_in` | `DETACH_LOCK_SQL`, a `SELECT m.id … FOR UPDATE` (`pg_memberships.rs:155-160`) | the peer's uncommitted `DELETE` of the cascade row |
  | S2 | `TeamService::restore` → `set_status_in` (`pg_teams.rs:242`) | the org ancestor read, `organization::Entity::find_by_id(..).lock_shared()` (`pg_teams.rs:266`) | the peer's uncommitted `UPDATE` of the org row |
  | S3 | `PgRoleGrantStore::grant` | `grant_in`'s `INSERT INTO "role_grant"` (`pg_role_grants.rs:214`), whose FK check reads the locked `role` row | the retirer's `lock_role_in` (`FOR UPDATE`) |
  | S4 | `PgSystemRoleReconciler::reconcile_role` | its `INSERT INTO "role"` (`pg_system_roles.rs:45`), whose FK check reads the locked `policy` row | the retirer's `lock_policy_in` (`FOR UPDATE`) |

  So the prefix is `select%` at S1 and S2, and `insert%` at S3 and S4. The issue's guess
  (`delete%` / `update%`) names the **peer's** statement, not the racer's, and would never
  match. §5's V2 measures the four query strings rather than trusting this table.
- **F4.** At S2, `set_status_in` locks the team row with `lock_exclusive()` (`pg_teams.rs:245`)
  before it reads the org ancestor (`:266`). That first lock is uncontested — the peer touches
  the org row, not the team row — so the ancestor read is the only statement that can block on
  the peer. (`rename_in` at `:185-231` carries the same two-read shape at `:188`/`:192`. It is
  not the racer here; do not cite it.)
- **F5.** At S4, `reconcile_role`'s existence check is a plain `find_by_id` on `&self.db`
  (`pg_system_roles.rs:34`), with no row lock. A plain read does not block on a `FOR UPDATE`
  holder, so the racer reaches its INSERT.
- **F6.** A row-lock waiter takes a `tuple` lock before it waits on the holder's transaction id.
  A poll can therefore observe `wait_event = 'tuple'` instead of `'transactionid'`.
  `pg_blocking_pids(pid)` reports the blocker in both forms. (Postgres behaviour, reasoned, not
  measured. §3.2 states what the design does with that, and D5 states why it is not verified.)
- **F7.** `put` bumps `policy_gen` unless the outcome is `AbsorbedIdempotent`
  (`pg_policies.rs:257-259`). `put_in` itself never bumps (`:276-368`, pinned by the assertion
  at `:767-771`).
- **F8.** `policy_content_matches` (`pg_policies.rs:105-111`) compares `kind`, `source`,
  `description` and `system`. It deliberately excludes `created_at` / `updated_at`, so two
  racers' timestamps may differ and the absorb still fires.
- **F9.** `support/mod.rs` is compiled per test binary via `mod support;`, and already carries
  `#[allow(dead_code)]` on items that only some binaries use. `clippy --all-targets -D warnings`
  lints the test files.
- **F10.** S1 and S2 run peer, racer and observer against one shared `DatabaseConnection`
  (sqlx default `max_connections = 10`). S3 and S4 use two: `db_a` for the retirer, `db_b` for
  the racer, both to the same container (`second_connection`, `authz_system_retirement_pg.rs:412`).
  `pg_stat_activity` and `pg_blocking_pids` are instance-wide, so the observer may poll on
  either.
- **F11.** S3's and S4's retirement transaction sets `SET LOCAL lock_timeout`
  (`pg_system_row_retirer.rs:104`). That bounds the **retirer's** own waits. The racer has no
  lock timeout, so it stays blocked until the retirer commits.
- **F12.** `rs/.config/nextest.toml` gives every `paigasus-iam` integration test `retries = 2`
  (SMA-521), and sets no slow-test timeout. A green task alone is not evidence that a test did
  not fail and get retried, and nothing terminates a slow test.
- **F13.** All four peers hold a `Box<dyn Transaction>`, not a raw `DatabaseTransaction`: S1 and
  S2 through `SeaOrmUnitOfWork::begin` (`tenancy_events_pg.rs:385`, `:478`), S3 and S4 through
  `begin_retirement` (`pg_system_row_retirer.rs:85`, which returns
  `Box::new(SeaOrmTransaction { txn })` at `:105`). `backend_pid` takes `&impl ConnectionTrait`,
  which `dyn Transaction` does not satisfy, and `SeaOrmTransaction`'s `txn` field is
  `pub(crate)` (`uow.rs:96`). The route out is `recover_txn` (`uow.rs:154`), a `pub fn` in the
  `pub mod uow` (`persistence/mod.rs:27`). It is not in that module's `pub use` list, so tests
  spell the full path. SMA-659's three sites had a raw `db.begin()`, so this case is new.
- **F14.** Two properties make §3.4's `policy_gen` assertion readable. `put` **awaits** the bump
  before it returns (`pg_policies.rs:258`), so a read after `put_b.await` is ordered against it.
  And `bump_policy_gen_best_effort` logs and swallows a failed bump (`:94-98`), so "the counter
  did not move" would be ambiguous on a fallible backend. `Generations::memory()`'s bump cannot
  fail (`adapters/authz/generation.rs:417`), which is what makes it unambiguous here.
- **F15.** The test container is `postgres:16-alpine` (`support/mod.rs:74`) and overrides
  neither `idle_in_transaction_session_timeout` nor `statement_timeout`. Both default to 0
  (disabled), so a peer transaction held open for the full budget is not killed by the server.

## 3. Design

### 3.1 The helper moves to `tests/support/race.rs`

A new file `rs/crates/services/paigasus-iam/tests/support/race.rs`, declared `pub mod race;` in
`support/mod.rs`, receives every item in F1 unchanged except for the changes in §3.2. Each item
carries `#[allow(dead_code)]`, for the reason F9 records: `support` compiles once per test
binary, and most binaries use none of these items.

`authz_policy_store.rs`, `tenancy_events_pg.rs` and `authz_system_retirement_pg.rs` call them as
`support::race::<item>`. Nothing else in `support/mod.rs` changes.

**Why a new file rather than `support/mod.rs`.** `mod.rs` is 950 lines and already holds three
unrelated concerns (Postgres container, mock IdP, HTTP harness). A fourth would make the file
the thing to read for every test that needs any one of them.

**Accepted cost.** `support` compiles into every test binary that declares `mod support;`, so
this file is compiled about 59 times per full test build. It is ~100 lines with no new
dependency, so the cost is accepted without measurement.

### 3.2 Signature and predicate changes

```rust
async fn wait_until_blocked_by<T>(
    db: &DatabaseConnection,
    blocker_pid: i32,
    racer: &JoinHandle<T>,
    query_prefix: &str,   // NEW
    budget: Duration,
) -> Result<(), String>;

async fn expect_racer_blocked<T>(
    db: &DatabaseConnection,
    blocker_pid: i32,
    racer: &mut JoinHandle<T>,
    query_prefix: &str,   // NEW
    describe: impl FnOnce(&T) -> String,
);
```

`BLOCKED_INSERTS_SQL` is renamed `BLOCKED_STATEMENTS_SQL` (its old name now describes one caller
of eight) and takes the prefix as `$2`:

```sql
SELECT count(*)::bigint AS n
FROM pg_stat_activity
WHERE wait_event_type = 'Lock' AND wait_event IN ('transactionid', 'tuple')
  AND btrim(query) ILIKE $2
  AND $1 = ANY(pg_blocking_pids(pid))
```

Four changes from SMA-659, each with its reason:

- **The prefix is a parameter** (F2, F3). The term's job is to show the racer is inside the
  statement under test, and that statement differs per site. A hardcoded `insert%` would make
  S1 and S2 wait out the full budget and fail, which is loud but wrong.
- **`btrim(query)`**, because the match is otherwise coupled to the leading whitespace of
  production SQL. `DETACH_LOCK_SQL` happens to start its raw string with `SELECT`
  (`pg_memberships.rs:155`) while its siblings in the same file start with a newline
  (`DETACH_PROJECT_SQL` at `:171`, `LIST_BY_ORG_SQL` at `:114`). A reformat would otherwise turn
  a passing site into a 30 s wait that reports a missing lock. `btrim` does not survive a
  rewrite into a CTE (`WITH RECURSIVE …`), which no term can; §3.3 step 2's comment names the
  targeted statement so that failure is attributable.
- **`wait_event` widens to `transactionid` or `tuple`** (F6). A poll that lands in the `tuple`
  moment would otherwise read as "not blocked". This widens what counts as blocked; it does not
  widen **who** blocks it, which is still `pg_blocking_pids(pid) @> blocker_pid`. See D5 for
  what is not proven about this term.
- **The deadline message prints `query_prefix`**, alongside the existing dump. Without it a
  prefix typo reads as "the lock is gone", which is the wrong thing to go and look at. The dump
  also widens from `left(query, 80)` to `left(query, 200)`: SeaORM emits a long column list
  before the table name, so 80 characters cannot tell S2's two SELECTs apart.

The three-step loop (blocked → `Ok`; racer finished → `Err`; deadline → `Err` with the non-idle
dump), the order of its checks, and the panic on a failed observer query all stay exactly as
SMA-659 wrote them.

### 3.3 The four rewritten sites

Each site changes in the same three places:

1. After the peer's blocking statement, read the peer's backend pid. The peer holds a
   `Box<dyn Transaction>` (F13), so this is
   `let pid_peer = support::race::backend_pid(paigasus_iam::adapters::persistence::uow::recover_txn(&*peer_tx).unwrap()).await;`
   **Do not change how the peer transaction is opened to make this simpler.** At S3 and S4 that
   would mean replacing `begin_retirement`, which silently drops the `SET LOCAL lock_timeout`
   (F11) the test's own premise rests on.
2. Replace the fixed wait with
   `support::race::expect_racer_blocked(&<observer db>, pid_peer, &mut handle, "<prefix>", |r| format!("{r:?}")).await;`
   plus a comment naming the statement that prefix targets and what the wait proves.
3. Commit the peer, as now. Everything after the commit is unchanged.

At S1 and S2 the racer handle becomes `let mut handle`. At S3 and S4 it already is. The four
racer output types are all `Debug`, so `format!("{r:?}")` compiles at each: `Vec<MembershipRecord>`,
`NodeView<Team>` (`ports.rs:57`), `()` and `RoleOutcome` (`authz/reconcile.rs:131`).

After this change `expect_racer_blocked` has **eight** call sites, each with its prefix:

| Call site | Prefix | Why |
| -- | -- | -- |
| `authz_policy_store.rs:454`, `:540`, `:617` (the three SMA-659 tests) | `insert%` | unchanged behaviour; they only gain the new argument |
| the rewritten absorb test (§3.4) | `insert%` | racer B's savepoint INSERT |
| S1, S2 | `select%` | F3 |
| S3, S4 | `insert%` | F3 |

The observer connection at S3 and S4 is **`db_a`**, the retirer's pool. Either works (F10), and
`db_a` is the one that does not also hold the blocked racer, so a poll never competes with the
racer for a connection.

**Two assertions are removed, not kept alongside.**

- S1 and S2 drop `assert!(!handle.is_finished(), "sanity: …")`. The wait proves strictly more:
  `!is_finished()` is also true for a racer that has not started, which is precisely the defect
  under repair. Leaving the weaker line in place next to the stronger one would suggest it
  carries weight it does not.
- S3 and S4 drop `timeout(500 ms, &mut handle).expect_err(..)`. That call has a second job: it
  polls the handle, so a racer that panics surfaces there rather than at the later `.await`.
  `expect_racer_blocked` covers that job through its "racer finished" arm, which awaits the racer
  and formats a `JoinError` into the panic message (`authz_policy_store.rs:162-167`).

Everything downstream of the commit at S3 and S4 is unchanged, including the
`timeout(10 s, handle)` that proves the racer **resumes** after the lock is released. That
timeout is a real upper bound on a released waiter, not a hope that a racer has started, so it
is not in scope.

### 3.4 The absorb path gets an assertion

`concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict` is rewritten into the
SMA-659 shape:

1. Build `doc`. Insert racer A's row directly through the `policy` entity inside
   `let txn_a = db.begin()`, with content that satisfies `policy_content_matches` (F8):
   the same `kind`, `source`, `description` and `system`. Do not commit.
2. `let pid_a = support::race::backend_pid(&txn_a).await;` — this peer IS a raw
   `DatabaseTransaction`, so F13's `recover_txn` route does not apply here.
3. Spawn racer B: the real `store_b.put(&doc)`. **`store_b` and the test must share ONE
   `Generations` handle** — `let gens = Generations::memory();` then
   `PgPolicyStore::new(db.clone(), gens.clone())`. Every other store in this file takes a fresh
   `Generations::memory()` (`:383-384`, `:447`, `:466`, `:478`); copying that pattern here gives
   the test a handle nothing bumps and makes step 6's assertion vacuously green. V5 is the
   control that catches exactly this slip.
4. `let before = gens.policy_gen().await.unwrap();`
5. `expect_racer_blocked(&db, pid_a, &mut put_b, "insert%", ..)` — B is past its existence check
   and inside its savepoint INSERT. Commit A. Await B.
6. Assert three things: B returned `Ok(())`; exactly one row carries the id; and
   `gens.policy_gen().await.unwrap() == before`.

The third assertion makes the absorb path observable through the `put` API, which returns
`Result<(), AuthzError>` and so cannot report its own `PutOutcome`. By F7, `put` bumps unless the
outcome is `AbsorbedIdempotent`. Racer A is a raw entity insert and bumps nothing. `Ok(())` rules
out `Conflict`, and `Inserted` is impossible because A's row wins the unique index. So `Ok` plus
an unmoved counter leaves only `AbsorbedIdempotent`, and a moved counter means B took the UPDATE
path — the exact failure this test could not previously see. F14 is what makes the read ordered
and the value unambiguous; the assertion message says so, so a future reader does not have to
re-derive it.

The doc comment loses its "a REAL race over the network, not a simulation" claim, which the
rewrite makes false, and its unasserted "one takes the INSERT path, the other absorbs", which
becomes an assertion. It gains the reasoning above and a pointer to `:500`, which covers the
`put_in`-level absorb (§1.2).

**What the rewrite gives up.** Both racers no longer go through the public `put`. That cost is
accepted: the pool race proved nothing it claimed, because the path it claimed to exercise was
whichever one the scheduler happened to pick.

### 3.5 The guard test, with four cases

`wait_until_blocked_by_reports_a_racer_that_never_blocks` (SMA-659 §3.3) keeps its home in
`authz_policy_store.rs`: it needs a migrated container and a `policy` table, and that file has
both. It calls the moved helper. It runs four cases against one container, in this order.

Cases 1 and 2 are SMA-659's, unchanged except for the new `"insert%"` argument:

1. **Finished.** Commit A first, spawn B's `put` with different content, assert `Err` containing
   `finished before it blocked`, then await B and assert `Ok(())`.
2. **Deadline.** Call the helper with `tokio::spawn(std::future::pending::<()>())`, the same
   `pid_a` and a 200 ms budget; assert `Err` containing `did not block`; abort the task.

Cases 3 and 4 are new, and both need a genuinely blocked racer. Set that up once: a fresh policy
id, a raw uncommitted `db.begin()` + entity insert as peer A, `pid_a`, and a spawned real
`put` of the same id, which blocks inside its INSERT.

3. **The prefix is honoured.** Two phases, and the order matters. First call the helper with
   `"insert%"` and the full `RACER_BLOCK_BUDGET`; assert `Ok`. That proves the racer IS blocked.
   Only then call it with `"delete%"` and a 200 ms budget and assert `Err` containing
   `did not block`. Without the first phase the second can pass because the racer had not
   arrived yet — the same timing-dependent assertion this spec exists to remove, rebuilt inside
   its own guard.
4. **The blocker pid is honoured.** With the same blocked racer, call the helper with an
   **unrelated** pid — the backend of a third, idle transaction opened for this case — and a
   200 ms budget. Assert `Err` containing `did not block`. Without this case,
   `AND $1 = ANY(pg_blocking_pids(pid))` can be deleted outright and every test in this spec
   still passes, because the racer is the only backend in a lock wait at all six race sites.
   That term is what answers *who* blocks the racer, and it is the only term that discriminates
   at S1 and S2 (D3).

Teardown for cases 3 and 4: commit or drop peer A, then await the racer so no task outlives the
test, and drop the third transaction.

### 3.6 Decisions

- **D1. S3 is in scope**, although the issue names only S1, S2 and S4. It is the same shape,
  and AC 1 says "no test in the crate". Approved in brainstorming.
- **D2. The absorb test is rewritten, not duplicated.** The alternative — keep the `join!` test
  as a smoke test and add a deterministic one — costs one more container start per run and
  keeps a test whose only honest assertion is "one row exists", which §3.4's step 6 also makes.
  Approved in brainstorming.
- **D3. The helper takes a prefix rather than dropping the `ILIKE` term.** The term earns its
  place at S3, S4 and the absorb test, where the racer runs a locking read *before* the INSERT
  under test and `insert%` is what proves it is past that read. At S1 and S2 it does **not**
  discriminate: both of `set_status_in`'s locking reads are SELECTs (F4), so `select%` cannot
  tell them apart, and `pg_blocking_pids` is what does the work there. The prefix is kept at all
  eight sites anyway, because a site-appropriate prefix costs one argument and makes the
  observation self-describing. Guard case 4 exists because `pg_blocking_pids` carries S1 and S2
  alone.
- **D4. `RACER_BLOCK_BUDGET` stays 30 s** for all eight call sites. It is a load budget, not an
  expectation: the wait returns on the first observation. Recorded cost: on a genuinely broken
  site the failure now takes 30 s rather than 500 ms, times the three attempts
  `retries = 2` gives it (F12), so about 90 s per broken site against `ci.yml`'s 30-minute job
  budget. The helper's own doc (`authz_policy_store.rs:116-119`) allows a further pool
  `acquire_timeout` of 30 s on a single poll, which is bounded but additive. That trade is
  accepted: a false green costs more than 90 s.
- **D5. The `'tuple'` half of the widened `wait_event` set is NOT verified**, and no listed
  verification would red if it were deleted. Producing the tuple-lock form on demand needs two
  waiters queued on one row, and that fixture would itself be timing-dependent — the defect this
  spec removes. The term is kept because F6 says a poll can land there and the cost of the extra
  value is one more `IN` member. Read it as defence against a rare poll, not as a tested path.

### 3.7 Out of scope

The remaining fixed sleeps in this crate's tests wait for a debounce timer, a poll tick, or a
deliberate hold, not for a racer to reach a lock, so AC 1 does not reach them. Listed by file,
not by line, so the list cannot rot: `relay_pg.rs`, `relay_nudge_pg.rs`, `boot_lifecycle_pg.rs`,
`boot_install_pg.rs`, `audit_e2e.rs`, `nats_publisher.rs`, `nats_permissions.rs`,
`authz_acceptance.rs`, `keycloak_e2e.rs`, `migration_lock_pg.rs`. The one in
`migration_lock_pg.rs` is a lock holder's deliberate hold rather than a timer, and it is out of
scope for the same reason: nothing waits on a racer arriving. This is the result of reading
every `sleep(` under `tests/` outside `support/`; it is a statement about those sites' purpose,
not a claim that each one is correct.

`authz_system_retirement_pg.rs:728` and `:883` use `lock_timeout` and assert a real error from
the database. They are not waits on a racer and do not change.

## 4. Files touched

| File | Change |
| -- | -- |
| `tests/support/race.rs` | new — the F1 items, with §3.2's signature and predicate |
| `tests/support/mod.rs` | `pub mod race;` and a line in the module doc |
| `tests/authz_policy_store.rs` | helper deleted; three call sites re-pointed and given `"insert%"`; guard test re-pointed and extended to four cases (§3.5); absorb test rewritten (§3.4) |
| `tests/tenancy_events_pg.rs` | S1, S2 rewritten (§3.3); `use std::time::Duration;` (`:48`) becomes unused once both sleeps go, so it is removed with them |
| `tests/authz_system_retirement_pg.rs` | S3, S4 rewritten (§3.3) |

No production code changes.

## 5. Verification

Every local run starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`, so
`cargo nextest` and `moon` resolve to the repository-pinned tools. Every local run uses
`PAIGASUS_REQUIRE_DOCKER=1` (a filtered run without Docker skips silently) and `--retries 0` (a
retry can turn a failing mutation green — F12).

**Restoring a mutation.** Two rules, because this PR touches test files and not production
files. A mutation in a file this PR **edits** (`tests/support/race.rs`, the three test files) is
a marked insert, undone by deleting the marked lines — never by `git checkout --`, which would
also discard the uncommitted work under test. A mutation in a **production** file (V5, V9) is
restored with `git checkout -- <path>`, which is safe precisely because this PR changes no
production file; confirm with `git status` that the path is clean before and after.

- **V1.** `cargo nextest run -p paigasus-iam --test authz_policy_store --test tenancy_events_pg
  --test authz_system_retirement_pg` passes, the guard test included.
- **V2. Measure the four query strings** rather than trust F3's table. Call
  `wait_until_blocked_by` directly at each of S1–S4 (not `expect_racer_blocked`, which
  hard-codes the budget) with a deliberately wrong prefix (`zzz%`) and a 2 s budget. The
  deadline message dumps every non-idle backend with `left(query, 200)`. Record the racer's
  actual query text per site in the implementation report, and confirm the prefix chosen in
  §3.3 matches it. If a measured string contradicts F3, the prefix changes and this spec's table
  is corrected — the measurement wins.
- **V3. The wait must be load-bearing.** Marked insert: `return Ok(());` as the first statement
  of `wait_until_blocked_by`. Guard cases 1, 3 and 4 must fail. (Case 2 asserts `Err` too, so it
  fails as well; naming all of them keeps the expected output exact.)
- **V4. The prefix must be honoured.** Marked insert: `let query_prefix = "%";` at the top of
  `wait_until_blocked_by`, which neutralises the term without deleting it. Guard case 3 must
  fail. V2 proves the prefix *values*; this proves the *plumbing*.
- **V5. The absorb assertion must bite.** Mutation in `pg_policies.rs:257-259`: bump
  unconditionally. The rewritten absorb test must fail on its `policy_gen` assertion, and no
  other test in the three binaries may fail — if another does, it already covered that
  condition and §1.2 is wrong.
- **V6. The blocker pid must be honoured.** Marked insert: replace the `$1 = ANY(...)` term's
  parameter so it always matches (`WHERE ... AND true`). Guard case 4 must fail, and only case 4.
- **V7. Each rewritten test must still catch the regression it exists to catch.** This is AC 2's
  real evidence, and it needs the production lock **deleted**, per site, one at a time:

  | Site | Delete | Expected |
  | -- | -- | -- |
  | S1 | `FOR UPDATE` from `DETACH_LOCK_SQL` (`pg_memberships.rs:160`) | S1 fails AT `expect_racer_blocked`, naming the missing lock wait — not later, at its outcome assertion |
  | S2 | `.lock_shared()` from `set_status_in`'s ancestor read (`pg_teams.rs:266`) | S2 fails at `expect_racer_blocked` |
  | S3 | the `lock_role_in` call in the test's retirement transaction | S3 fails at `expect_racer_blocked` |
  | S4 | the `lock_policy_in` call in the test's retirement transaction | S4 fails at `expect_racer_blocked` |

  Record the message for each. The four old tests carry this proof in their own doc comments
  (`tenancy_events_pg.rs:360-361`, `:445-447`), but the rewrite MOVES the failure point from the
  outcome assertion to the wait, so the old proof no longer applies to the new code.
- **V8. The false green, made visible.** This is the evidence that the OLD tests could pass
  while the race did not happen. It needs BOTH halves, because a late racer alone makes
  `!handle.is_finished()` *more* true, not less. At S1 and S2: delete the lock (V7's row) AND
  insert `tokio::time::sleep(Duration::from_secs(1)).await;` as the racer's first statement.
  Against the OLD file both tests must PASS — that is the false green. Against the NEW file both
  must fail at the wait. S3 and S4 have no comparable demonstration: their old
  `timeout(500 ms).expect_err` shape passes with a late racer either way, which is the defect
  itself, so V7 alone carries them.
- **V9. A late racer must not break the new tests.** Insert the same 1 s racer sleep, with the
  production locks INTACT, at each of S1–S4 and the absorb test. All five must pass: the wait
  absorbs a late racer, which is the whole point.
- **V10.** `moon run paigasus-iam-rs:lint` (clippy `--all-targets`, so the test files are linted)
  and `cargo fmt --check` stay clean. The `#[allow(dead_code)]` of §3.1 is what keeps this green
  for binaries that use none of `support::race`, and §4's removed `Duration` import is what
  keeps it green in `tenancy_events_pg.rs`.
- **V11. CI.** This PR edits `paigasus-iam` test files, so it selects `paigasus-iam-rs:test`. The
  evidence is the `nextest-junit` artifact (profile `iam`): the four rewritten tests, the absorb
  test and the guard test must carry no `flakyFailure` and no `rerunFailure` element. A green
  task alone is not enough (F12).

## 6. Acceptance criteria (from the issue)

1. **No test in the crate waits a fixed time for a racer to reach a lock.** §3.3 covers S1–S4,
   §3.4 covers the absorb test, and §3.7 states which remaining sleeps are not of this kind and
   why. Evidence: V1, V8, V9.
2. **Each rewritten test fails with a message that names the missing lock wait when the racer
   does not block.** `expect_racer_blocked` panics with the wait's own reason, and with the
   racer's result when it has finished (§3.2, §3.3). Evidence: V7 per site, plus V3, V4 and V6
   on the helper itself.
3. **The absorb path is asserted.** §3.4 step 6, scoped to `put`'s bump-skip by §1.2.
   Evidence: V5.
