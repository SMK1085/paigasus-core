# SMA-660 measurements

Every run used `PAIGASUS_REQUIRE_DOCKER=1` and `--retries 0`, from the worktree.

## The four measured query strings (spec V2)

| Site | Prefix used | Measured `query` (first 200 chars) |
| -- | -- | -- |
| S1 `tenancy_events_pg.rs` detach | `select%` | `SELECT m.id FROM "membership" m\n WHERE m.id = $3\n    OR (m.principal_id = $1\n        AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)\n          OR m.project_id IN (SELECT id FROM "pro` |
| S2 `tenancy_events_pg.rs` restore | `select%` | `SELECT "organization"."id", "organization"."prn", "organization"."slug", "organization"."name", "organization"."status", "organization"."created_at", "organization"."updated_at", "organization"."creat` |
| S3 `authz_system_retirement_pg.rs` grant | `insert%` | `INSERT INTO "role_grant" ("id", "principal_id", "role_key", "scope_kind", "scope_node_prn", "scope_org_id", "scope_team_id", "scope_project_id", "linked_policy_id", "created_at") VALUES ($1, $2, $3, $` |
| S4 `authz_system_retirement_pg.rs` role insert | `insert%` | `INSERT INTO "role" ("key", "template_id", "scope_kinds", "description", "system", "created_at") VALUES ($1, $2, $3, $4, $5, $6) RETURNING "key", "template_id", "scope_kinds", "description", "system", ` |

All four agree with the spec's F3 table. Each measured query is the debug-formatted string
captured by the site's `PROBE` panic (task 4 report for S1/S2, task 5 report for S3/S4); the
`\n` and `\"` sequences above are exactly what the panic message printed, not an editing
artifact. None of the four sites needed a prefix change.

## Mutations, and what each one reddened

| # | Mutation | Expected | Observed |
| -- | -- | -- | -- |
| V3 | the wait's first check made unconditionally true | guard fails at case 1 (a test stops at its first panic) | Matches. Recipe: `if blocked > 0 {` changed to `if blocked >= 0 {` in `wait_until_blocked_by`. Compiled. `wait_until_blocked_by_reports_a_racer_that_never_blocks` FAILED at case 1's `expect_err`, panic message `a racer that takes the UPDATE path never blocks, so the wait must report it: ()`. Cases 2-4 were not executed in this run — the test function stops at its first panic, so this is a known and accepted limit of a single-function guard test, not an omission. The original mutation recipe from task 2's first attempt (`return Ok(());` as the first statement) failed to compile at all (`unreachable_code`/`unused_variables` under `-D warnings`); the coordinator ruled that recipe defective and supplied the `if blocked >= 0` recipe used here. |
| V4 | `query_prefix` neutralised to `%` at the bind site | guard case 3 alone fails | Matches exactly. Recipe: `let _ = query_prefix;` inserted as the first statement, and the query's bind values changed from `[blocker_pid.into(), query_prefix.into()]` to `[blocker_pid.into(), "%".into()]`. Compiled. FAILED at case 3's phase-two `expect_err`, panic message `a prefix that matches no statement must reach the deadline, even with the racer blocked: ()`. Cases 1, 2 and 4 did not fail (the test ran to that point and only then panicked). The original mutation recipe (bare `let query_prefix = "%";` shadowing the parameter) failed to compile (`unused_variables` under `-D warnings`); the coordinator ruled it defective and supplied the recipe used here. |
| V5 | `pg_policies.rs` bumps unconditionally | absorb test fails, and NOTHING else | Matches. Recipe: in `pg_policies.rs`, `if !matches!(outcome, PutOutcome::AbsorbedIdempotent) { self.bump_policy_gen_best_effort().await; }` replaced with an unconditional `self.bump_policy_gen_best_effort().await;`. The whole `authz_policy_store` binary was run with `--no-fail-fast` (17 tests) to confirm nothing else failed. Result: `17 tests run: 16 passed, 1 failed, 0 skipped` — exactly `concurrent_put_of_the_same_new_policy_id_is_idempotent_not_a_conflict` failed, panic message: `` assertion `left == right` failed: put must SKIP its policy_gen bump on the absorb path; a moved generation means racer B took the UPDATE path and the absorb never ran\n  left: 1\n right: 0 ``. The literal mutation left `outcome` unused, a compile error under `-D warnings`; the binding was renamed to `_outcome` (a `src/`-file edit made only to observe the mutation, discarded via `git checkout --` before the commit, never staged or committed) to let the crate compile. |
| V6 | `pg_blocking_pids` term neutralised | guard case 4 fails | Matches exactly, first attempt, no compile issue. Recipe: `BLOCKED_STATEMENTS_SQL`'s final line changed from `AND btrim(query) ILIKE $2 AND $1 = ANY(pg_blocking_pids(pid))` to `AND btrim(query) ILIKE $2 AND ($1 = ANY(pg_blocking_pids(pid)) OR true)`. FAILED at case 4, panic message: `a pid that blocks nobody must reach the deadline, even with a blocked racer running: ()`. |
| V7 S1 | `FOR UPDATE` deleted from `DETACH_LOCK_SQL` | S1 fails AT the wait | Matches. S1 (`a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report`) FAILED at the deadline sub-branch of `expect_racer_blocked` (`support/race.rs:129:5`) after the full 30 s `RACER_BLOCK_BUDGET`. Literal message: `the racer did not block on the peer's uncommitted statement (pid 59) within 30s while running a statement matching "select%"; non-idle backends: pid=59 state=idle in transaction wait=Client/ClientRead query=SELECT pg_backend_pid() AS pid; pid=60 state=active wait=Lock/transactionid query=DELETE FROM "membership" m\n WHERE m.principal_id = $1\n   AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)\n     OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2))`. Mechanism: with `FOR UPDATE` gone, the SELECT no longer blocks, so the racer proceeds to the cascade `DELETE`, which blocks on a different statement the `select%` prefix never matches, so the deadline fires. |
| V7 S2 | `.lock_shared()` deleted from `set_status_in` | S2 fails AT the wait | Matches, at the wait, via the OTHER branch of `expect_racer_blocked` (not the deadline branch — the resolution places no requirement on which sub-branch, only that it fail at the wait). S2 (`a_concurrent_org_archive_is_reflected_in_a_racing_team_set_status_event`) FAILED at `support/race.rs:126:9`. Literal message: `the racer finished before it blocked on the peer's uncommitted statement (pid 59), so the race never happened; the racer's own result: Ok(NodeView { node: Team { ... status: Active, ... }, effective_status: Active })`. Without the ancestor lock, `restore`'s org read never blocks, so `svc.restore` completes before `expect_racer_blocked` ever observes it blocked, and its own result carries the stale `effective_status: Active` the lock exists to prevent. |
| V7 S3 | `lock_role_in` removed | S3 fails AT the wait | Matches, same non-deadline branch as S2. S3 (`a_concurrent_grant_blocks_then_reports_unknown_role`) FAILED at `support/race.rs:126:9`. Literal message: `the racer finished before it blocked on the peer's uncommitted statement (pid 59), so the race never happened; the racer's own result: Ok(())`. With the role row never locked, the racing grant's INSERT succeeds immediately, so the racer finishes before the observer's first poll ever finds it blocked. |
| V7 S4 | `lock_policy_in` removed | S4 fails AT the wait | Matches, same non-deadline branch. S4 (`locking_the_policy_row_blocks_a_concurrent_role_insert`) FAILED at `support/race.rs:126:9`. Literal message: `the racer finished before it blocked on the peer's uncommitted statement (pid 60), so the race never happened; the racer's own result: Ok(Inserted)`. With the policy row never locked, `reconcile_role`'s INSERT succeeds at once. |
| V8 | old file + lock deleted + 1 s late racer | S1 and S2 PASS — the false green | Matches. Reconstructed the pre-task `tenancy_events_pg.rs` (`git show 16d383b6:...tenancy_events_pg.rs`), deleted the same lock at each site (`FOR UPDATE` for S1, `.lock_shared()` for S2), and inserted a marked 1 s `tokio::time::sleep` as the first statement of each racer's spawned task. Both PASSED alone: S1 in 1.974s, S2 in 1.967s — the old `!handle.is_finished()` check is satisfied by a racer that has not even started yet, so it reports success with the lock gone and no race having occurred. As a control on the other half, the same lock deletion plus the same late racer against the NEW (committed) test file correctly FAILED S1 at the full 30 s deadline (`support/race.rs:129:5`, same `DELETE FROM "membership"` message shape as the V7 S1 row above) — the new wait is not fooled by a late racer when the lock is genuinely missing. |
| V9 | new file + 1 s late racer, locks intact | all five tests pass | Matches, across all five rewritten tests. With production files clean (locks intact) and a marked 1 s `tokio::time::sleep` as the first statement of each racer, the `tenancy_events_pg` binary (S1, S2) ran 4/4 PASS in 2.257s, and the `authz_system_retirement_pg` binary (S3, S4) ran 18/18 PASS in 4.304s (S3 and S4 each took roughly 2.5s — the 1s sleep plus the poll interval before the racer reaches its blocking statement — well inside the 30s budget). `authz_policy_store`'s fifth rewritten test (the idempotent-put absorb test, task 3) is racer-A-as-raw-insert rather than a `tokio::spawn` racer with the same late-start shape as S1-S4, so V9 was not separately re-run against it in task 3's own report; its mutation coverage is V5 (see above), which is the site-specific analogue of the same "the assertion must actually bite" property. |

Quote the actual failure message for each V7 row: acceptance criterion 2 is about the MESSAGE,
not only about the failure. All four V7 messages are quoted verbatim above, transcribed
character-for-character from the corresponding task report's literal output block.

## Timing

Three consecutive full runs of the three binaries (`authz_policy_store`, `tenancy_events_pg`,
`authz_system_retirement_pg`; 39 tests total each run), from `rs/`:

| Run | nextest summary time | wall-clock (`time` builtin, `real`) | Result |
| -- | -- | -- | -- |
| 1 | 7.612s | 9.560s | 39 tests run: 39 passed, 0 skipped |
| 2 | 7.694s | 8.134s | 39 tests run: 39 passed, 0 skipped |
| 3 | 7.579s | 8.007s | 39 tests run: 39 passed, 0 skipped |

All three runs passed identically, with no order-dependence or flakiness at the new waits. The
nextest summary time and the wall-clock `time` differ because `time` also covers cargo's
up-to-date check and process startup; nextest's own summary is the tighter measure of test
execution time.

## Whole-crate verification (Step 2)

`moon run paigasus-iam-rs:test --force` ran the crate's entire suite: **1145 tests run, 1145
passed (1 flaky), 0 skipped**, in 96.090s (nextest summary), task total 3m 29s 277ms. Moon's own
exit code for the task was 0. The one flaky test was `paigasus-iam::authz_cache_redis
authz_cache_decision_get_of_missing_key_is_none`, in a binary this branch never touched. Its
first attempt failed with:

```
thread 'authz_cache_decision_get_of_missing_key_is_none' (84997529) panicked at crates/services/paigasus-iam/tests/authz_cache_redis.rs:134:63:
connect to redis: Backend(Incompatible type - Parse error at 1
Unexpected `84`
Unexpected `72`
)
```

and passed on its second attempt (`FLAKY 2/3`). This is a Redis connection/protocol hiccup in
`authz_cache_redis.rs`, unrelated to `authz_policy_store.rs`, `tenancy_events_pg.rs` or
`authz_system_retirement_pg.rs` — the only three binaries this branch touched. It matches the
documented pattern of this crate's Docker-backed suites being genuinely flaky under parallel
load. No failure appeared in any of the three binaries this branch touched; all their tests
passed on the first attempt, consistent with the three-run repeat above. `PAIGASUS_SKIP_DOCKER`
was never set for this run — Docker was reachable throughout, and the run used the crate's real
retry budget from `rs/.config/nextest.toml` (not `--retries 0`, since Step 2's brief command,
unlike Step 1's, is the plain `moon run` task as CI runs it).

`moon run paigasus-iam-rs:lint --force`: PASSED, exit code 0, in 315ms (task time; 1s 701ms wall
including its upstream `contracts:generate`/`paigasus-proto-rs:build`/
`paigasus-service-info-rs:build` dependencies). No warnings, no errors.

## `src/` cleanliness (Step 3)

`git status --short`, run after both Moon commands completed: clean except for this
measurements file itself (`?? docs/superpowers/plans/2026-09-19-sma-660-measurements.md`,
untracked because it had not yet been staged). No other file, tracked or untracked, was
modified by either Moon run.

`git diff --stat main...HEAD -- rs/crates/services/paigasus-iam/src/`: empty output. No file
under `rs/crates/services/paigasus-iam/src/` differs from `main` anywhere in this branch's
history.
