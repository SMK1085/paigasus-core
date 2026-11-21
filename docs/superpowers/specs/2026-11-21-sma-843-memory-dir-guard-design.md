# SMA-843 — Memory store: a directory guard for `rename` and `delete`

Status: approved by Sven at Gate 1 (2026-11-21). Parent design:
`2026-11-20-sma-789-memory-store-design.md` (SMA-789).

## 1. Intent

The memory tool can `rename` or `delete` a directory. The workspace stages one change per file that
it sees under the directory. Today nothing checks the directory itself. So a file that another
writer adds under the directory during the run stays at the old path, and the run gets no conflict.

Sequence (from the issue):

1. Run A views `/tasks` and sees `/tasks/a.md`.
2. Run B creates `/tasks/b.md` and commits.
3. Run A renames `/tasks` to `/done` and commits.

Result today: `/done/a.md` exists and `/tasks/b.md` stays. A directory `delete` leaves `/tasks/b.md`
in the same way.

**Expected:** a directory `rename` or `delete` moves or deletes every file under the directory at
commit time, or the commit refuses with a conflict. A conflict drops the whole staged group.

## 2. What the code does today

- `commands.rs` `targets` reads the directory with `MemoryWorkspace::list`. `list` answers from the
  **cached** listing when the run listed the directory before. So in the sequence above, step 3
  sees only `/tasks/a.md`.
- `stage` checks the baseline of each staged path only (`baseline`). A path that is not staged is
  not checked.
- At commit, the Postgres `apply` checks the version of each change. It does not look at the
  directory.
- **The destination has the same gap.** Run A lists `/done` (empty). Run B creates `/done/z.md`. Run
  A renames `/tasks` to `/done`. Every target is `Put { Absent }`, so the commit passes, and the two
  directories merge with no error.

## 3. Goals

- G1. At commit, a directory `rename` or `delete` refuses with `Conflict` when the store holds a
  path under a guarded directory that no change of the set names. The guarded directories are the
  source of a `delete`, and the source and the destination of a `rename`.
- G2. At command time, the same case gives the model a refusal text in the normal case (an early
  check, as in §6.5 of the parent design). After the model views the directory again, a retry
  passes.
- G3. A conflict drops the whole group of the staged command, guards included. An early conflict on
  a path under an armed guard also drops that guard's group.
- G4. The tests of §7 pass on the fake and on Postgres. Each mutation in §7.4 makes a named test
  fail.
- G5. The parent spec states the new contract (§6.6 below). SMA-829 (S3) and SMA-790 (the run
  commit) get a note.

## 4. Non-goals and anti-goals

- Non-goal: the file form of `rename` (`/a.md` to `/b.md`). Its `Put { Absent }` and `Delete`
  version checks already cover it.
- Non-goal: a new conflict text. The commit-time conflict cannot reach the model (§6.12 of the
  parent design). The early conflict uses the existing text (§6.11).
- Non-goal: the S3 adapter. SMA-829 builds it.
- Non-goal: a metric for guard conflicts. §6.13 counters come through SMA-798.
- Anti-goal: a false conflict. A file that the change set itself writes under a guarded directory
  (for example a `create /tasks/new.md` after `delete /tasks`) must pass.
- Anti-goal: a guard that stays after its group drops, or after the model got an early conflict for
  a path under it. Either gives a false conflict at commit, and a commit-time conflict rolls back
  the whole run.
- Anti-goal: a conflict loop. A model that views the directory after a conflict must be able to
  retry.

## 5. Approaches

| | Check | Trade-off |
|---|---|---|
| A | Record the set of files that the run saw under each directory. Compare it with the store before the changes apply. | Earlier buffered changes in the same run (a `create` in `/tasks` before the rename) make the expected set hard to compute. More state goes into the change set. |
| **B (chosen by Sven)** | Record only the guarded directories. Under the scope lock, the store may hold no path under a guard that no change of the set names. Add an early check in `delete` and `rename`. | A simple rule. It also covers the destination gap. It extends the port contract, and SMA-829 must follow it. |
| C | Only the commit-time check of B. | Smaller, but the normal case then rolls back the whole run (§6.12) and the model never sees the conflict. |

## 6. Design

### 6.1 The commit set carries guards

- `MemoryCommitSet` gets a third field, `guards: BTreeSet<MemoryDir>`. `MemoryChangeSet` does not
  change. Only `take_changes` fills `guards`.
- `MemoryCommitSet::is_empty()` is true when the set has no change **and** no guard. Every caller
  that decides "nothing to apply" uses this method, not `changes.is_empty()`.
- The root has no special case. A root guard means "every path of the scope". The commands never
  make one, because `delete` and `rename` refuse the root.
- A guard covers the directory path itself as a file path, and every path under it. So the guard
  `/done` also covers a file `/done` (the file/directory invariant of parent §6.2).

### 6.2 The port rule (order-independent)

`apply` refuses with `MemoryApplyError::Conflict { path }` when, before the changes, the store holds
a path that a guard covers and that no change of the set names. `path` is the first such path: the
guards in `BTreeSet` byte order, and inside a guard the paths in byte order. So every adapter
reports the same path.

This rule does not depend on when the adapter checks. A `Delete` path is present before and gone
after. A present `Put { Absent }` path is already a per-change conflict. So a check after the
changes, against "no path under a guard that no `Put` names", gives the same result under the lock.

### 6.3 Postgres

- The guard check runs right after the scope row lock, **before** the first change. Order: lock,
  guards, the per-change checks (with the file cap), the store caps. So a guard conflict wins over
  every cap refusal.
- For each guard: read the paths with `path = $dir OR path LIKE $prefix ESCAPE '\'` (the `like_prefix`
  of `list`, with the same escaping), with every scope column bound, `ORDER BY path`. Filter in
  Rust against the set of the change paths. No array bind. The read is not strictly bounded by the
  file cap (an over-cap store can hold more files, parent §6.7). The builder may add
  `LIMIT k + 1`, where k is the number of change paths under the guard.
- A set with guards and no change now takes the scope row and the lock. The comment at
  `memory.rs:143` and the test `an_empty_change_set_takes_no_scope_row_and_no_lock` in
  `memory_pg.rs` change to "a set with no change and no guard".

The fake does the same on its copy, before the first change.

### 6.4 The workspace

**Guards belong to a group.**

- `stage` takes the guards of the command with its puts and removes. The guards join the new group
  of the command (the `groups` rule of parent §6.5). The workspace holds
  `dir_guards: Vec<(MemoryDir, u64)>`.
- `join_group` renumbers the guards of every joined group, like the paths. The `retain` that
  removes paths with no change does **not** remove guards. A guard whose changes merged away still
  guards (the guard-only case).
- A guard joins a group in two ways: through a staged path of the same command, or through a later
  stage that touches a path of the group.
- `take_changes` copies the guards of the workspace into the `MemoryCommitSet`, with no duplicates.

**The shared conflict function.** `baseline` and `check_dir` use one function for a conflict on a
path P. It:

1. drops the `known` entry of P and the cached listings that hold P (as today);
2. drops the group of P (as today);
3. **new:** when the store holds P, drops every group that has a guard covering P, also a
   guard-only group (found through `dir_guards`, not through a path);
4. **new:** marks P as "re-sight". The next fresh listing of a directory that holds P records P as
   a first sight with its stored version, even when P is under a `listed` directory. This removes
   the mark. So a `view` of the directory, then the retry, passes;
5. returns `WorkspaceError::Conflict { path: P, also }`. `also` holds the paths of every dropped
   group, in byte order.

**The early check: `check_dir`.** `pub(super) async fn check_dir(&mut self, dir: &MemoryDir)`. It
lists `dir` in the store (one fresh `store.list`). It compares the listing with what the run saw:

- a stored path that the run saw with another version, or saw as absent (through `known`, or a
  covering `listed` directory with no re-sight mark), is a conflict;
- a path that the run saw as present under `dir` (in `known`, version `Some`) and that the store no
  longer holds is a conflict;
- a stored path that the run never saw, or that has a re-sight mark, becomes a first sight
  (recorded in `known`, as `list` does), with no conflict.

It compares the versions of the listing entries. It does not do one `get` per file. It handles the
first conflict in byte order and returns it. When it passes, it **replaces** `listings[dir]` with
the fresh rows and adds `dir` to `listed`. So the next `list(dir)` uses exactly the checked rows.

**Fresh listing for the destination: `list_fresh`.** `pub(super)`. It drops `listings[dir]` and lists
again, with the normal `list` rules. It does not check versions.

### 6.5 The commands

- `delete` and `rename` first call `targets` as today. When the result is a directory operation
  (`delete` of more than the one-file case; `rename` with `as_file` `None`), the command calls
  `check_dir(&dir)` on the source, then calls `targets` again on the refreshed listing. The file
  form does not call `check_dir`, so it costs no extra store call.
- `rename` gets its destination check from `list_fresh(&new.dir)`, not the cache. A file under the
  destination, or the file `new.dir` itself, gives "Error: The destination {new} already exists".
  This is a failed precondition, not a write conflict: nothing is staged and no group drops.
- `delete` of a directory stages with the guard `dir`. `rename` of a directory stages with the
  guards `old.dir` and `new.dir`.

### 6.6 Notes for other issues and the parent spec

- **SMA-829 (S3).** S3 cannot make the rule atomic. It checks the rule of §6.2 before its first
  write, as best effort, and passes the sequential conformance tests. The S3 text of parent §6.8
  states the race. A comment on SMA-829 records this.
- **SMA-790 (run commit).** It calls `apply_in` for every commit set where `is_empty()` is false,
  also a set with only guards. A comment on SMA-790 records this.
- Parent spec updates:
  - §6.4: the commit set holds guards, and the port rule of §6.2.
  - §6.5: guards in a group, the new conflict steps, `check_dir`, the re-sight mark. The "short
    window" sentence: a guard is armed from the command until the commit, for every path under the
    directory, so a commit-time conflict is not only a "short window" case.
  - §6.8: the guard step of the Postgres `apply`, the new "empty set" rule, and the S3 note.
  - §6.12: a guard-only set is applied, and a guard conflict is a `Conflict`.
  - §7: the new tests.

## 7. Tests

### 7.1 Conformance (`conformance.rs`, added to `run_all`, port level)

1. `dir_rename_conflicts_on_a_file_added_under_the_source`: the issue sequence as a commit set with
   the guards `/tasks` and `/done`. Conflict at `/tasks/b.md`. The store is unchanged.
2. `dir_delete_conflicts_on_a_file_added_under_the_dir`: the same for `delete`.
3. `dir_rename_conflicts_on_a_file_added_under_the_destination`: conflict at `/done/z.md`. Also a
   file at the path `/done` itself: conflict at `/done`.
4. `a_guard_allows_the_paths_that_the_set_names`: passes. An outsider in a subdirectory
   `/tasks/sub/q.md` conflicts.
5. `a_guard_matches_segments_and_literal_wildcards`: no conflict for `/tasks-old/x` and `/axb/f`
   (guards `/tasks`, `/a_b`).
6. `a_guard_only_set_is_checked`: a set with a guard and no change conflicts on an outsider, and
   passes on an empty directory.
7. `a_guard_conflict_wins_over_a_cap_refusal`: an outsider plus a store-cap breach gives `Conflict`.
8. `a_guard_ignores_files_of_another_scope`: a file under `/tasks` in scope B does not conflict a
   guard in scope A. Control: the same file in scope A conflicts.
9. `guards_report_the_first_outsider_in_byte_order`: two guards, outsiders in both; the reported
   path is the first in byte order.

### 7.2 Workspace and commands (fake store)

The outsider write comes **after** the command and before `commit()` unless the test says so.

1. `delete /tasks`, then B adds `/tasks/b.md`: `commit()` gives `Conflict { /tasks/b.md }`, and the
   store is unchanged.
2. `rename /tasks /done`, then B adds `/tasks/b.md`: `commit()` gives `Conflict { /tasks/b.md }`.
3. `rename /tasks /done`, then B adds `/done/z.md`: `commit()` gives `Conflict { /done/z.md }`.
4. `delete /tasks`, then `create /tasks/a.md` (this joins the group). Control: a `#[cfg(test)]`
   accessor of the armed guards shows the guard `/tasks` before the conflict. Then B changes `a.md`,
   and the model writes `a.md` again: conflict. `take_changes` holds no guard and no change.
5. The challenge sequence: view `/tasks`; `delete /tasks`; `create /tasks/n.md`; B creates
   `/tasks/n.md`; the model writes `n.md` again: conflict, and `also` names `/tasks/a.md`.
   `take_changes` holds no guard. `commit()` passes.
6. Guard only: `create /t/x`, `delete /t`, B adds `/t/y`, then the model writes `/t/y`: conflict.
   `take_changes` holds no guard.
7. Command time (the outsider write **before** the command): the issue sequence. `rename` and
   `delete` refuse with the conflict text, and nothing is buffered.
8. Retry after a directory view: test 7, then `view /memories/tasks`, then the same `rename`:
   passes, and `commit()` moves both files.
9. The buffered-delete case that only `check_dir` catches: view `/tasks`, `delete /tasks/a.md`
   (buffered), B creates `/tasks/b.md`, then `rename /tasks /done`: conflict at `/tasks/b.md`.
10. The destination at command time: view `/done` (empty), B creates `/done/z.md`, `rename /tasks
    /done`: "The destination /memories/done already exists". Nothing is buffered.
11. The stale cached row (from the challenge): view root (empty); B creates `/tasks/x`; view
    `/tasks`; B deletes `/tasks/x`; `rename /tasks /done` gives the normal "does not exist" text of
    an empty directory, not "The path /memories/tasks/x does not exist".
12. `create /tasks/new.md` after `delete /tasks`: `commit()` passes.

### 7.3 Postgres (`tests/memory_pg.rs`)

`run_all` already runs the conformance suite. One more end-to-end test: A views `/tasks`, A renames
it, B creates `/tasks/b.md` and commits, A commits: `Conflict` at `/tasks/b.md`, and the store is
unchanged.

### 7.4 Proof that the tests bite

Make each mutation, run the tests, and record in the PR which named test fails. Every mutation must
fail at least one test. A mutation must compile (no unused-code error hides it).

| Mutation | Must fail |
|---|---|
| Remove the guard step of the Postgres `apply` | 7.1 tests 1, 2, 3, 6 on Postgres; 7.3 |
| Remove the guard step of the fake | 7.1 tests 1, 2, 3, 6 on the fake |
| Drop the `path = $dir` term | 7.1 test 3 (the `/done` file) |
| Remove the scope columns from the guard read | 7.1 test 8 |
| Remove the guard of `delete` | 7.2 test 1 |
| Remove `old.dir` from the guards of `rename` | 7.2 test 2 |
| Remove `new.dir` from the guards of `rename` | 7.2 test 3 |
| Remove the guard copy in `take_changes` | 7.2 tests 1, 2, 3 |
| Remove the guard renumbering in `join_group` | 7.2 test 4 |
| Remove the guard drop for the group of P (step 2) | 7.2 test 4 |
| Remove step 3 (drop the groups of covering guards) | 7.2 tests 5, 6 |
| Remove step 4 (the re-sight mark) | 7.2 test 8 |
| Remove the `check_dir` call | 7.2 tests 7, 9 |
| Remove the listing replace in `check_dir` | 7.2 test 11 |
| Use the cached listing for the destination | 7.2 test 10 |

## 8. Constraints

- The port trait does not change its methods. `MemoryApplyError` does not change.
- `MemoryCommitSet` gets a field, so every struct literal of it changes (tests included). The
  builder may add a constructor.
- Edition 2024, the workspace lints, an SPDX header on every new file.
- The Postgres tests are Docker-gated (`rs/CLAUDE.md`).

## 9. Left to the builder

- The exact signature that passes guards into `stage` (an extra argument, or a second method).
- The SQL of the guard read, inside the rule of §6.3.
- The data structure of the re-sight mark.

## 10. Calls I made (for Sven to check)

- The destination of a `rename` is also a guard (G1). The issue names only the source.
- The early check (G2) is in scope, so the model sees the conflict in the normal case.
- A guard conflict wins over every cap refusal (§6.3 order).
- The outsider path is the conflict path, so `MemoryApplyError` needs no new variant.
- A set with only guards is still applied and checked (§6.1).
- A destination that holds a file gives "already exists", not a conflict (§6.5).
- Guards are not checked again at each later `stage` call. A guard stays armed until the commit.
  So an outsider that arrives after the command and before the end of the run is found only at
  commit, and that rolls back the run (§6.12).

## 11. Stage 2 challenge (2026-11-21): what changed

Verdict: APPROVE WITH CHANGES. Folded in:

- BLOCKER: the tests did not prove that the commands arm guards. Added 7.2 tests 1-6 (the write
  after the command) and the mutation table of §7.4.
- MAJOR: an early conflict under an armed guard left the guard. Added conflict step 3 and tests 5-6.
- MAJOR: a conflict loop after a directory view. Added the re-sight mark (step 4) and test 8. The
  destination now gives "already exists" (test 10).
- MAJOR: the claim "the cached listing agrees with the store" was false. `check_dir` now replaces
  the cached rows. Test 11.
- MAJOR: the S3 note contradicted the conformance suite. The port rule is now order-independent
  (§6.2).
- MINOR, folded: guards on `MemoryCommitSet` with its own `is_empty`; the cap-order sentence and
  test 7.1-7; no root special case; the cross-scope test; the `path = dir` term; the byte-order
  rule; the full list of parent-spec updates and the `memory_pg.rs` test rename; the cap-bound
  sentence; no `check_dir` cost for the file form.

Not folded:

- A guard-conflict metric: out of scope. The §6.13 counters come through SMA-798.
- A guard check at each later `stage`: not done (a call for Sven, §10). The cost is one store list
  per guard per command.
