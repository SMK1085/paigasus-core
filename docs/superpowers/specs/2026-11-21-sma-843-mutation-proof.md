# SMA-843 mutation proof

Each row removes one piece of the guard code. The crate suite (`paigasus-agent-host-core`, 136
tests) and the `memory_pg` suite (12 tests, Docker) then run. Every row fails at least one test.
The unmutated tree passes both suites. The mutated line compiles under `-D warnings`.

Names: `cmd::` is `memory::commands::tests::`, `ws::` is `memory::workspace::tests::`, `fake::` is
`memory::fake::tests::`, and `pg::` is `memory_pg`.

| # | Mutation | Failing tests | Proof |
|---|---|---|---|
| 1 | Remove the `check_guards` call in the Postgres `apply` | pg::a_directory_rename_conflicts_on_a_file_added_before_the_commit, pg::postgres_passes_the_conformance_suite | pass |
| 2 | Remove the guard step of the fake `apply` | fake::the_fake_passes_the_conformance_suite, cmd::dir_delete_then_an_outsider_conflicts_at_commit, cmd::dir_rename_then_an_outsider_at_the_source_conflicts_at_commit, cmd::dir_rename_then_an_outsider_at_the_destination_conflicts_at_commit | pass |
| 3 | Drop the `path = $4` term in the Postgres guard read | pg::postgres_passes_the_conformance_suite (the `/done` file) | pass |
| 4 | Remove the scope columns from the guard read | pg::postgres_passes_the_conformance_suite. It fails at the first guard check that shares the database with another scope (a false conflict at `/tasks/b.md`), before check 8. | pass |
| 5 | Remove the guard of `delete` | cmd::dir_delete_then_an_outsider_conflicts_at_commit, cmd::an_early_conflict_under_a_dir_delete_drops_the_delete, cmd::a_create_after_a_dir_delete_where_the_file_is_deleted_drops_the_guard, cmd::a_guard_only_dir_delete_drops_on_an_early_conflict, cmd::a_create_after_a_dir_delete_joins_and_a_conflict_drops_the_guard | pass |
| 6 | Remove `old.dir` from the guards of `rename` | cmd::dir_rename_then_an_outsider_at_the_source_conflicts_at_commit, pg::a_directory_rename_conflicts_on_a_file_added_before_the_commit | pass |
| 7 | Remove `new.dir` from the guards of `rename` | cmd::dir_rename_then_an_outsider_at_the_destination_conflicts_at_commit | pass |
| 8 | Remove the guard copy in `take_changes` | cmd::dir_delete_then_an_outsider_conflicts_at_commit, cmd::dir_rename_then_an_outsider_at_the_source_conflicts_at_commit, cmd::dir_rename_then_an_outsider_at_the_destination_conflicts_at_commit, ws::a_guard_is_armed_and_reaches_the_commit_set, ws::a_later_stage_into_the_group_carries_the_guard, ws::a_conflict_leaves_unrelated_guards, pg::a_directory_rename_conflicts_on_a_file_added_before_the_commit | pass |
| 9 | Remove the guard renumbering in `join_group` | ws::a_joined_guard_drops_with_its_group_when_the_path_is_gone, cmd::a_create_after_a_dir_delete_where_the_file_is_deleted_drops_the_guard | pass |
| 10 | Remove the guard drop of the group of P (step 2) | ws::a_joined_guard_drops_with_its_group_when_the_path_is_gone, cmd::a_create_after_a_dir_delete_where_the_file_is_deleted_drops_the_guard, ws::a_conflict_on_one_path_of_a_staged_move_drops_the_whole_move, ws::a_later_stage_into_a_moved_path_joins_the_move, cmd::a_conflict_after_a_rename_keeps_the_source | pass |
| 11 | Remove step 3 (drop the groups of covering guards) | cmd::an_early_conflict_under_a_dir_delete_drops_the_delete, cmd::a_guard_only_dir_delete_drops_on_an_early_conflict, ws::a_guard_only_group_drops_on_an_early_conflict_under_it, ws::an_early_conflict_under_a_guard_drops_the_guard_group | pass |
| 12 | Remove step 4 (the re-sight mark) | cmd::a_directory_view_after_a_conflict_lets_the_retry_pass, ws::a_re_sight_mark_is_used_once, ws::a_re_sight_mark_lets_a_directory_view_end_the_loop | pass |
| 13 | Remove the `check_dir` calls of `delete` and `rename` | cmd::dir_rename_and_delete_refuse_an_outsider_at_command_time, cmd::check_dir_catches_an_outsider_after_a_buffered_delete, cmd::a_directory_view_after_a_conflict_lets_the_retry_pass, cmd::a_stale_cached_row_gives_the_dir_missing_text | pass |
| 14 | Remove the listing replace in `check_dir` | cmd::a_stale_cached_row_gives_the_dir_missing_text, ws::check_dir_records_a_never_seen_path_and_replaces_the_cache | pass |
| 15 | Use the cached listing for the destination of `rename` | cmd::a_file_at_the_destination_gives_already_exists | pass |
| 16 | Remove the join term for the groups that a new guard covers in `join_group` | cmd::a_dir_delete_after_a_buffered_move_drops_with_the_move, cmd::a_dir_rename_onto_a_moved_away_destination_drops_with_the_move | pass |

Notes:

- Row 13 did not compile at first, because `check_dir` became unused code. The mutation keeps a
  reference to it, so it compiles, and the result above is from that version.
- Rows 3 and 4 change the SQL so that the bind count stays valid (`$4::text = ''` and a `$1..$3`
  null check). The queries run, and the tests fail on the assertion, not on a SQL error.
- Rows 1 to 4 need Docker. The core suite passes for those rows, as expected.
- After the last row, `git diff --stat -- rs/` printed nothing.
- Row 16 came from the final review. It ran on the core suite with 140 tests. The mutation
  replaces the term with an empty `extend`, because a plain removal fails `unused_mut` under
  `-D warnings`. The core suite gave 138 passed and 2 failed. The reverse edit restored the line,
  and the core suite then gave 140 passed.
