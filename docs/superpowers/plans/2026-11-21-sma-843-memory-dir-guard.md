# SMA-843 Memory directory guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A directory `rename` or `delete` of the memory tool moves or deletes every file under the directory at commit time, or refuses with a conflict.

**Architecture:** `MemoryCommitSet` carries guarded directories. The port rule says that `apply` refuses with `Conflict { path }` when the store holds a covered path that no change names. The fake and Postgres check it right after the scope lock, before the first change. The workspace keeps guards in the command groups. A fresh `check_dir` listing gives the early conflict, and a re-sight mark lets a directory `view` end a conflict loop.

**Tech Stack:** Rust 2024 (rust-version 1.95), tokio, sea-orm raw SQL on Postgres, testcontainers (Docker-gated), cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-21-sma-843-memory-dir-guard-design.md` (approved). The parent is `docs/superpowers/specs/2026-11-20-sma-789-memory-store-design.md`.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- The `MemoryStore` trait methods and `MemoryApplyError` do not change.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` pass (run from `rs/`).
- Put the proto shims on `PATH` first: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- The Postgres tests need Docker. Run them with `PAIGASUS_REQUIRE_DOCKER=1` so that a skip is a failure.
- Conventional commits with the scope `rs` (`feat(rs): …`, `test(rs): …`, `docs(rs): …`). The body ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`, `--amend` or `git reset`.
- The user-facing text in the spec and the doc comments uses Simplified Technical English (short sentences, active voice).

## Review Focus

1. A guard `/tasks` and a file `/tasks.md`: the file is not covered (segment match). Pinned in Task 1.
2. `rename /a /b` and then `rename /b /a` in one run: the commit passes (no false conflict on the run's own work). Pinned in Task 5.
3. Two directory commands on unrelated directories: a conflict under one drops only that group's guard. Pinned in Task 4.
4. A re-sight mark is used once. A path that changes again after the re-sight is a conflict again. Pinned in Task 4.
5. A `rename` of a directory whose subdirectory was listed separately before: the commit moves every file, and the moved files do not show at the old path in a later `view`. Pinned in Task 5.

---

### Task 1: Guard types

**Files:**
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/path.rs` (impl `MemoryDir`, near line 114)
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/change.rs` (`MemoryCommitSet`, line ~130)
- Modify every `MemoryCommitSet { … }` literal: `fake.rs`, `conformance.rs`, `workspace.rs`, `change.rs`, `rs/crates/services/paigasus-agent-host/tests/memory_pg.rs`

**Interfaces:**
- Produces: `MemoryDir::covers(&self, path: &MemoryPath) -> bool`: true when `path` is the directory path itself or `self.contains(path)`. The root covers every path.
- Produces: `MemoryCommitSet { pub scope: MemoryScope, pub changes: MemoryChangeSet, pub guards: BTreeSet<MemoryDir> }`.
- Produces: `MemoryCommitSet::new(scope: MemoryScope, changes: MemoryChangeSet) -> Self` (empty guards) and `MemoryCommitSet::is_empty(&self) -> bool` (no change and no guard).
- `MemoryDir` must derive `Ord` (it is in a `BTreeSet` already; check).

- [ ] **Step 1: Write the failing tests**

In `path.rs` tests: `a_dir_covers_itself_and_the_paths_under_it` — `/tasks` covers `/tasks` and `/tasks/a/b.md`; it does not cover `/tasks.md`, `/tasks-old/x` or `/task`; the root covers `/x`.
In `change.rs` tests: `a_commit_set_is_empty_only_with_no_change_and_no_guard` — `new(scope, empty)` is empty; with one guard it is not empty; with one change and no guard it is not empty.

- [ ] **Step 2: Run them and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core -E 'test(covers) | test(is_empty_only)'`
Expected: compile error (no `covers`, no `guards`).

- [ ] **Step 3: Implement `covers`, the `guards` field, `new` and `is_empty`; change every literal to `MemoryCommitSet::new(…)`**

- [ ] **Step 4: Run the crate tests**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core && cargo build -p paigasus-agent-host --tests`
Expected: all pass, and the service tests compile.

- [ ] **Step 5: Commit** — `feat(rs): add directory guards to the memory commit set (SMA-843)`

---

### Task 2: The port rule in the conformance suite and the fake

**Files:**
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/conformance.rs` (new checks, added to `run_all`)
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/fake.rs` (`apply`)

**Interfaces:**
- Consumes: Task 1.
- Produces: the nine `pub async fn` checks below, each `(store: &dyn MemoryStore)`, all called from `run_all`. A helper `guarded(scope, changes, guards: &[&str]) -> MemoryCommitSet`.

- [ ] **Step 1: Write the nine checks (spec §7.1)**

Each check uses a `fresh_scope()` and the `write`/`version_of`/`assert_conflict` helpers. "Unchanged" means: `listed(store, &s, "/")` and the content of each file are the same as before `apply`.
1. `dir_rename_conflicts_on_a_file_added_under_the_source`: write `/tasks/a.md`; set = `put_new("/done/a.md", …)`, `del("/tasks/a.md", v)`, guards `/tasks`, `/done`; write `/tasks/b.md`; `assert_conflict(…, "/tasks/b.md")`; unchanged.
2. `dir_delete_conflicts_on_a_file_added_under_the_dir`: write `/tasks/a.md`; set = `del` + guard `/tasks`; write `/tasks/b.md`; conflict at `/tasks/b.md`; unchanged.
3. `dir_rename_conflicts_on_a_file_added_under_the_destination`: the rename set; write `/done/z.md`; conflict at `/done/z.md`. Second scope: write the file `/done` instead; conflict at `/done`.
4. `a_guard_allows_the_paths_that_the_set_names`: write `/tasks/a.md`; set = `del /tasks/a.md`, `put_new("/tasks/new.md")`, guard `/tasks`; passes, and `listed(…, "/tasks") == ["/tasks/new.md"]`. Second scope: an outsider `/tasks/sub/q.md` conflicts at `/tasks/sub/q.md`.
5. `a_guard_matches_segments_and_literal_wildcards`: write `/tasks-old/x`, `/tasks.md`, `/axb/f`; a set with only the guards `/tasks` and `/a_b` passes.
6. `a_guard_only_set_is_checked`: a set with only the guard `/t` passes on an empty scope; after `write /t/y` it conflicts at `/t/y`.
7. `a_guard_conflict_wins_over_a_cap_refusal`: write `/tasks/b.md`; set = `put_new("/big", "12345")` and guard `/tasks`, caps `max_store_bytes: 1`; result is `Conflict { /tasks/b.md }`, not `CapExceeded`.
8. `a_guard_ignores_files_of_another_scope`: write `/tasks/b.md` in scope B; a guard-only set `/tasks` in scope A passes. Control: write `/tasks/b.md` in A; the same set conflicts.
9. `guards_report_the_first_outsider_in_byte_order`: write `/b/2`, `/b/1`, `/a/9`; guard-only set `/b`, `/a`: conflict at `/a/9`.

- [ ] **Step 2: Run the fake conformance test and see it fail**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core -E 'test(the_fake_passes_the_conformance_suite)'`
Expected: FAIL in check 1 ("expected a conflict at /tasks/b.md, got Ok(())").

- [ ] **Step 3: Implement the guard step in the fake `apply`**

Right after `before` is read and before the change loop: for each guard in `set.guards` order, take the scope paths in byte order that `guard.covers(path)`; the first path that `set.changes.get(path)` is `None` for gives `Err(Conflict { path })`.

- [ ] **Step 4: Run the crate tests**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(rs): check memory directory guards in the fake store (SMA-843)`

---

### Task 3: The guard step in the Postgres adapter

**Files:**
- Modify: `rs/crates/services/paigasus-agent-host/src/adapters/persistence/memory.rs` (`apply_body`, lines 142-214)
- Modify: `rs/crates/services/paigasus-agent-host/tests/memory_pg.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2 (`run_all` already holds the nine checks).
- Produces: `async fn check_guards(txn: &DatabaseTransaction, set: &MemoryCommitSet) -> Result<(), MemoryApplyError>` (private).

- [ ] **Step 1: Rename the empty-set test and add the end-to-end test**

In `memory_pg.rs`: rename `an_empty_change_set_takes_no_scope_row_and_no_lock` to `a_set_with_no_change_and_no_guard_takes_no_scope_row_and_no_lock` (body unchanged). Add `a_guard_only_set_takes_the_scope_lock`: a guard-only set on a never-written scope succeeds, and afterwards the `memory_scope` row exists.
Add `a_directory_rename_conflicts_on_a_file_added_before_the_commit`: workspace A over `PgMemoryStore` (an `Arc`), with `/tasks/a.md` written first; `execute` view `/memories/tasks`, then rename `/memories/tasks` to `/memories/done`; a direct write of `/tasks/b.md`; `ws.commit()` gives `Conflict { /tasks/b.md }`; the store still holds `/tasks/a.md` and `/tasks/b.md` and no `/done/a.md`. (This test also needs Task 5 for the guards. Mark it `#[ignore = "needs Task 5"]` now, and remove the ignore in Task 5.)

- [ ] **Step 2: Run the Postgres suite and see it fail**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-agent-host --test memory_pg`
Expected: FAIL in `postgres_passes_the_conformance_suite` (check 1) and in `a_guard_only_set_takes_the_scope_lock`.

- [ ] **Step 3: Implement `check_guards` and call it right after `before`**

Change the early return to `if set.is_empty()`. Update the comment above it to "A set with no change and no guard takes no scope row and no lock". `check_guards` runs, for each guard in `set.guards` order, `SELECT path FROM memory_file WHERE {SCOPE} AND (path = $4 OR path LIKE $5 ESCAPE '\') ORDER BY path`. `$4` is `guard.as_str()`, `$5` is `like_prefix(guard)`. For the root, `$4` matches no row. The first row whose path `set.changes.get(&path)` is `None` for gives `Conflict`. Add a unit test in `memory.rs` only if a new pure helper appears.

- [ ] **Step 4: Run the Postgres suite**

Run: the command of Step 2.
Expected: all pass, one ignored.

- [ ] **Step 5: Commit** — `feat(rs): check memory directory guards in the Postgres store (SMA-843)`

---

### Task 4: Guards in workspace groups, the conflict steps, `check_dir` and `list_fresh`

**Files:**
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/workspace.rs`

**Interfaces:**
- Consumes: Task 1, and the fake of Task 2.
- Produces:
  - `pub async fn stage_guarded(&mut self, puts: Vec<(MemoryPath, String)>, removes: Vec<MemoryPath>, guards: Vec<MemoryDir>) -> Result<(), WorkspaceError>`. `stage` calls it with no guards.
  - `pub(super) async fn check_dir(&mut self, dir: &MemoryDir) -> Result<(), WorkspaceError>` (spec §6.4).
  - `pub(super) async fn list_fresh(&mut self, dir: &MemoryDir) -> Result<Vec<(MemoryPath, u64)>, WorkspaceError>`.
  - `#[cfg(test)] pub(crate) fn armed_guards(&self) -> Vec<MemoryDir>` (byte order, no duplicates).
  - `take_changes` fills `MemoryCommitSet::guards`.

- [ ] **Step 1: Write the failing workspace tests (fake store, `stage_guarded` direct)**

- `a_guard_is_armed_and_reaches_the_commit_set`: `stage_guarded(vec![], vec![a], vec![/tasks])`; `armed_guards() == [/tasks]`; `take_changes().guards == {/tasks}`.
- `a_conflict_in_a_group_drops_its_guard`: a guarded stage of `/tasks/a`; another writer changes `/tasks/a`; `write(/tasks/a)` conflicts; `armed_guards()` is empty.
- `a_later_stage_into_the_group_carries_the_guard`: guarded delete of `/tasks/a`, then `write(/tasks/a)` (joins); another writer changes nothing; `take_changes().guards == {/tasks}` (the renumbering keeps it).
- `an_early_conflict_under_a_guard_drops_the_guard_group`: view `/tasks` (holds `a`); guarded delete of `/tasks/a`; `write(/tasks/n)`; another writer creates `/tasks/n`; `write(/tasks/n)` again conflicts, and `also` holds `/tasks/a`; `armed_guards()` is empty.
- `a_guard_only_group_drops_on_an_early_conflict_under_it`: `write(/t/x)`, guarded remove of `/t/x` (net nothing, guard `/t`); another writer creates `/t/y`; `write(/t/y)` conflicts; `armed_guards()` is empty.
- `a_conflict_leaves_unrelated_guards` (Review Focus 3): guards `/a` and `/b` in two groups; a conflict under `/a`; `armed_guards() == [/b]`.
- `check_dir_conflicts_on_a_new_path_under_a_listed_dir`: list `/tasks`; another writer adds `/tasks/b`; `check_dir(/tasks)` gives `Conflict { /tasks/b }`.
- `check_dir_conflicts_on_a_seen_path_that_is_gone`: list `/tasks` (holds `a`); another writer deletes `a`; `check_dir` gives `Conflict { /tasks/a }`.
- `check_dir_records_a_never_seen_path_and_replaces_the_cache`: the stale-row sequence of spec §7.2 test 11 at workspace level; after `check_dir` passes, `list(/tasks)` equals the store rows.
- `a_re_sight_mark_lets_a_directory_view_end_the_loop`: `check_dir` conflicts on `/tasks/b`; `list(/tasks)` (fresh, because the conflict dropped the cache); `check_dir(/tasks)` passes.
- `a_re_sight_mark_is_used_once` (Review Focus 4): continue the last test; another writer changes `/tasks/b` again; `check_dir` conflicts again.
- `list_fresh_ignores_the_cache`: list `/done` (empty); another writer adds `/done/z`; `list_fresh(/done)` returns `[/done/z]`.

- [ ] **Step 2: Run them and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core -E 'test(guard) | test(check_dir) | test(re_sight) | test(list_fresh)'`
Expected: compile errors, then FAILs.

- [ ] **Step 3: Implement**

- `dir_guards: Vec<(MemoryDir, u64)>` and `resight: BTreeSet<MemoryPath>` on the workspace.
- `stage_guarded` = today's `stage` body, and it pushes the guards with the new group in `join_group`. `join_group` renumbers the guards of joined groups. Its `retain` stays for paths only.
- One `fn conflict(&mut self, path: &MemoryPath, stored: bool) -> WorkspaceError` with the five steps of spec §6.4. `baseline` calls it with `stored = fetched.is_some()`. Step 3 drops each group that has a guard that `covers(path)`, and it removes those guards. `also` is the dropped paths in byte order, without `path`.
- `list` treats a path in `resight` as a first sight and removes it from `resight`. `check_dir` does the same.
- `take_changes` copies the guard dirs into a `BTreeSet`.

- [ ] **Step 4: Run the crate tests**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core`
Expected: all pass. The existing workspace and command tests stay green.

- [ ] **Step 5: Commit** — `feat(rs): keep memory directory guards in workspace groups (SMA-843)`

---

### Task 5: Wire the commands

**Files:**
- Modify: `rs/crates/libs/paigasus-agent-host-core/src/memory/commands.rs` (`delete` line 327, `rename` line 341, tests)
- Modify: `rs/crates/services/paigasus-agent-host/tests/memory_pg.rs` (remove the `#[ignore]` of Task 3)

**Interfaces:**
- Consumes: Task 4 (`stage_guarded`, `check_dir`, `list_fresh`).

- [ ] **Step 1: Write the failing command tests (spec §7.2, tests 1-12)**

Use the existing `committed_parts` and `other_writer` helpers and `run(ws, json!(…))`. The names:
1. `dir_delete_then_an_outsider_conflicts_at_commit`
2. `dir_rename_then_an_outsider_at_the_source_conflicts_at_commit`
3. `dir_rename_then_an_outsider_at_the_destination_conflicts_at_commit`
4. `a_create_after_a_dir_delete_joins_and_a_conflict_drops_the_guard` (control: `armed_guards()` holds `/tasks` before the conflict)
5. `an_early_conflict_under_a_dir_delete_drops_the_delete` (`also` names `/memories/tasks/a.md` in the text; `commit()` passes)
6. `a_guard_only_dir_delete_drops_on_an_early_conflict`
7. `dir_rename_and_delete_refuse_an_outsider_at_command_time` (nothing buffered)
8. `a_directory_view_after_a_conflict_lets_the_retry_pass` (`commit()` then moves both files)
9. `check_dir_catches_an_outsider_after_a_buffered_delete`
10. `a_file_at_the_destination_gives_already_exists` (text: `Error: The destination /memories/done already exists`; nothing buffered)
11. `a_stale_cached_row_gives_the_dir_missing_text` (text: `Error: The path /memories/tasks does not exist`)
12. `a_create_after_a_dir_delete_commits`
Plus Review Focus 2: `rename_there_and_back_commits` (`/a` to `/b` to `/a`; `commit()` passes; the store equals the start).
Plus Review Focus 5: `a_dir_rename_after_a_subdir_listing_moves_everything` (view `/tasks/sub`, view `/tasks`, rename `/tasks` to `/done`; `commit()` passes; `view /memories/tasks/sub` gives the missing text).

- [ ] **Step 2: Run them and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core -E 'test(dir_) | test(rename_there) | test(destination) | test(stale_cached) | test(guard_only)'`
Expected: FAILs (for example test 1: `commit()` is `Ok`).

- [ ] **Step 3: Implement (spec §6.5)**

- `delete`: when `files` is a directory operation (`files != [parsed.file]`), call `ws.check_dir(&parsed.dir)`, take `targets` again, and refuse with `missing_path` if it is now empty. Stage with `stage_guarded(vec![], files, vec![parsed.dir.clone()])`.
- `rename`: when `as_file` is `None`, `check_dir(&old.dir)` and take `sources` again. For the destination, use `ws.list_fresh(&new.dir)` in place of the cached list, and also `ws.exists` of `new.dir` as a file path when the dir is not the root. Stage with the guards `old.dir` and `new.dir` for a directory move.
- In `memory_pg.rs`, remove the `#[ignore]` from `a_directory_rename_conflicts_on_a_file_added_before_the_commit`.

- [ ] **Step 4: Run the full checks**

Run: `cd rs && cargo nextest run -p paigasus-agent-host-core && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-agent-host --test memory_pg && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all pass.

- [ ] **Step 5: Commit** — `feat(rs): guard directory rename and delete in the memory tool (SMA-843)`

---

### Task 6: The parent spec and the mutation proof

**Files:**
- Modify: `docs/superpowers/specs/2026-11-20-sma-789-memory-store-design.md` (§6.4, §6.5, §6.8, §6.12, §7)
- Create: `docs/superpowers/specs/2026-11-21-sma-843-mutation-proof.md`

- [ ] **Step 1: Update the parent spec text as spec §6.6 lists it.** Keep the STE style. Replace the "short window" sentence with the armed-guard text. Link the SMA-843 spec once.

- [ ] **Step 2: Run the 15 mutations of spec §7.4, one at a time**

For each row: make the mutation, run the named tests (`-p paigasus-agent-host-core`, and `memory_pg` for the Postgres rows), record the failing test names, then undo only that mutation with a reverse edit (not `git checkout`, so no other work is lost). A mutation that does not compile under `-D warnings` does not count: change it until it compiles (for example `let _ = …`), then run it.

- [ ] **Step 3: Write the result table into the proof file.** Columns: mutation, failing tests, pass/fail of the proof. Every row must show at least one failing test. If a row shows none, stop and report it. Do not change a test to make it fail.

- [ ] **Step 4: Check that the tree is clean of mutations**

Run: `git diff --stat -- rs/` shows no change, and the Step 4 command of Task 5 passes again.

- [ ] **Step 5: Commit** — `docs(rs): record the SMA-843 contract and mutation proof`
