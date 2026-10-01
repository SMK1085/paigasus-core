# SMA-709: the twin COLLECTION_ROWS floors track the 38-row table

- Linear: SMA-709 (follow-up named by the SMA-708 spec, §9)
- Path: bounded (two integer literals, comments and two messages in two files, plus two prose
  sites that name the floor)
- Date: 2026-09-27 (approved 2026-09-28)
- Base: `origin/main` at `83d446fc`; re-checked on 2026-09-28 against `origin/main` at `906de46c`
- Status: **APPROVED** by Sven on 2026-09-28. Decisions: floor = count − 1 (37) in both places
  (Q2); the gate-side floor keeps `infra` (rc 2) (Q1). Both decisions confirm the spec defaults,
  so the scope does not change.

## 1. Problem

`ci/release-plan/release_plan.py` has a module-level table `COLLECTION_ROWS`. Two independent
floors protect it against an emptied or gutted table:

| Floor | Location on `origin/main` | Value |
|---|---|---|
| in-process | `release_plan.py:1783-1784`, `self_test()`: `if len(COLLECTION_ROWS) < 14:` and the `the floor is 14` message | 14 |
| gate | `ci/actionlint/run.sh:4650`, check 11 (`release_plan_self_test()`, line 4625): `[ "$c" -ge 14 ]` on `--collection-count`, and the `expected at least 14` message | 14 |

The table holds 38 rows since SMA-708 (PR 326). Check 10's comment at `ci/actionlint/run.sh:4589-4590`
states the rule: "Re-baseline the floor whenever rows are added: set it just under the current
`--fixture-count`, never far below it." At 14, a deletion of any 24 rows reds nothing.

`ci/release-plan/README.md:153-154` (row 8 prose) and `ci/release-plan/run.sh:449` (row 8
comment) both say that the arity floor "returns 3 if `COLLECTION_ROWS` is short two or more rows"
(run.sh: "delete two rows from COLLECTION_ROWS and the floor fires"). At 14 this statement is
false. At 37 it is true, but only while the floor equals the count minus one. It goes false again
after the next row addition. §4.3 rewords both sites so that they do not depend on the count.

## 2. Measured facts (2026-09-27)

- `--collection-count` on `origin/main` prints `38`. `--fixture-count` prints `9`.
- The FIXTURES floor is 8 in both places (`self_test()` and check 11 `[ "$n" -ge 8 ]`). It already
  tracks its table (9 rows, one below). It is out of scope.
- M1. A scratch copy of `release_plan.py` with two rows removed (the SMA-708 row
  "a committed fixture repository starts no background git maintenance" and the SMA-688 row
  "sed parity: a quoted chain key fails --assert"): `--collection-count` prints `36`, and
  `--self-test` exits `0` with no `FAIL` line. This is the defect.
- The unmodified copy: `--self-test` exits `0`.
- No whole-line pin in `ci/affected-graph/ci_targets.py` refers to the check 11 floors. Check 10's
  floor line has such a pin (`ACTIONLINT_SH_INDENTED_CALL_SITES`, line 1030); check 11's floors
  do not.

## 3. Decision

Raise both floors to **37**, one below the current count, in one PR (approved by Sven on
2026-09-28, Q2). The gate-side floor keeps `infra` (rc 2) (approved on 2026-09-28, Q1). This is the issue's first
option and the convention that check 10 and the FIXTURES floor already follow: one legitimate row
removal stays possible, and a second removal reds.

The re-baseline rule applies when the row count changes in either direction: set both floors to
`--collection-count` minus one. After an intended one-row removal, the floors move down by one, so
one row of headroom stays.

The issue's second option ("record why 14 is intended") is rejected. No evidence in the repo says
that 14 is intended. The SMA-608 plan set 14 when the table held about 15 rows, and the floor
did not move when SMA-658, SMA-688 and SMA-708 added rows.

Rejected alternatives:

- **An exact count (`==`), not a floor.** Every new row would then red check 11 as infra until
  someone edits two files. The repo's convention is a floor with one row of headroom. An exact
  count also blocks a legitimate removal.
- **Floor = count (`-ge 38`, no headroom).** This option does not red on added rows, and it
  closes the one-row residual. The cost is that every deliberate removal must also lower both
  floors in the same commit. This is the same kind of intentional re-baseline that
  `EXPECTED_FINDING_KEYS` in `ci/affected-graph/ci_targets.py` requires. It is rejected because
  check 10 and the FIXTURES floor both use one row of headroom, and a different convention for
  one twin pair makes the rule harder to follow. It is recorded as open question Q2.
- **One shared constant.** The two floors are deliberately in two separately scheduled files, so
  one edit cannot remove both (`self_test()` comment, `release_plan.py:1778-1782`). A shared
  constant would defeat that.
- **A gate that asserts `floor == count - 1`, or a whole-line pin in `ci_targets.py`.** It needs a
  parse of both files and adds a control for a Low-priority drift. For check 11, the twin floor
  in the other file is the protection against a deleted or lowered floor, and the written rule in
  both comments is the only control against a floor that does not move. Check 10 has an extra
  whole-line pin; check 11 does not get one in this ticket.

## 4. Changes

### 4.1 `ci/release-plan/release_plan.py`

- `self_test()`: `< 14` becomes `< 37`. The message `the floor is 14` becomes `the floor is 37`.
- Extend the message so that it names the likely cause and the action, for example:
  `"FAIL COLLECTION_ROWS has only {n} row(s); the floor is 37 — something emptied or gutted the
  collection-layer table; if the removal is intended, lower both floors in the same commit
  (SMA-709)"`. Keep the `COLLECTION_ROWS has only {n} row(s); the floor is 37` substring, because
  T3 asserts it.
- Extend the comment at `release_plan.py:1778-1782` with the re-baseline rule in STE, for example:
  "The floor tracks the table (SMA-709). When the row count changes, set it to one below
  `--collection-count`. Change the twin in `ci/actionlint/run.sh` check 11 in the same commit."
- The existing sentence "Floored below the actual count so a legitimate row removal does not
  abort the gate as infra" stays.

### 4.2 `ci/actionlint/run.sh` (check 11)

- `[ "$c" -ge 14 ]` becomes `[ "$c" -ge 37 ]`. The message `expected at least 14` becomes
  `expected at least 37`, followed by `; if the removal is intended, lower both floors in the
  same commit (SMA-709)`. Keep the substring `reports $c collection rows, expected at least 37`,
  because T7 asserts it.
- Put one new comment line directly above the `[ "$c" -ge 37 ]` line (or extend the existing
  "The COLLECTION_ROWS twin" comment): the floor tracks the table, one below the current count;
  when the count changes, re-set both; the twin is in `release_plan.py` `self_test()` (SMA-709).
- The `infra` (rc 2) classification stays (Q1, decided on 2026-09-28).
- Fix the stale citation in the FIXTURES comment at `run.sh:4638-4641` ("Check 10's own floor is
  equally loose (150 against 155 actual — that citation read 20 against 84 …)"). Reword it so
  that it contains no numbers, for example: "Check 10's own floor follows the same one-row
  headroom rule." SMA-708 set the precedent: it corrected a stale row-count comment in the same
  edit so that the comment no longer states a row count.

### 4.3 The two prose sites that name the floor

- `ci/release-plan/README.md:153-154`: "returns 3 if `COLLECTION_ROWS` is short two or more
  rows" becomes "returns 3 when `COLLECTION_ROWS` falls below its floor".
- `ci/release-plan/run.sh:449`: "delete two rows from COLLECTION_ROWS and the floor fires" becomes
  "delete rows from COLLECTION_ROWS until it falls below its floor, and the floor fires".
- Neither site states a count or a floor value after the change, so neither site needs a
  re-baseline.

## 5. Constraints

- Both floor values change in the same commit. A PR with only one changed floor fails AC1.
- SPDX headers and all other text stay as they are.
- Conventional commit: `fix(ci): raise the COLLECTION_ROWS floors to track the table (SMA-709)`.
- Other open tickets can add rows to `COLLECTION_ROWS` at the same time. An addition keeps both
  floors valid. Before the merge, re-read `--collection-count` on the rebased branch. If the count
  is not 38, set both floors to the new count minus one.

## 6. Acceptance criteria

1. AC1. Both floors have the same value N, and N = `--collection-count` − 1 on the merged branch
   (37 when the count is 38).
2. AC2. Both failure messages name N and tell the author to lower both floors in the same commit
   if the removal is intended.
3. AC3. Both comments state the rule "the floor tracks the table, one below the count, re-set when
   the count changes", and each names the other file as its twin.
4. AC4. `release_plan.py --self-test` exits `0` on the branch.
5. AC5. `repo:actionlint` passes (check 11 included) in CI.
6. AC6. In-process mutation proof (§7, T2 to T4): a 36-row copy gives `--self-test` rc 3 with the
   floor text on the branch and rc 0 on `origin/main`; a 37-row copy gives rc 0.
7. AC7. Gate-side mutation proof (§7, T7): the real `release_plan_self_test()` from the branch
   `run.sh`, run against a 36-row copy, exits 2 with stderr
   `reports 36 collection rows, expected at least 37`. Against a 37-row copy the floor does not
   fire. The same 36-row run against `origin/main`'s `run.sh` does not fire the floor.
8. AC8. `README.md` and `ci/release-plan/run.sh` no longer state a row count for the floor.
9. AC9. `run.sh`'s check 11 FIXTURES comment no longer states "150 against 155" or any other
   count for check 10.

## 7. Test strategy

All mutations run on COPIES in a scratch directory, never on the tracked file (memory:
"mutation restore discards the fix").

Mutation method: remove rows BY LABEL with a small Python edit (parse the file text, drop the
tuple whose label string matches, write the copy). Do not delete by line number: the SMA-708 row
covers two physical lines (1744-1745), and a line-number cut can leave a `SyntaxError` (rc 1).
After each mutation, assert non-vacuity: `cmp -s <original> <copy>` must report a difference.

Command prefix: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`,
then `uv run --locked --project ci/release-plan --python '>=3.12' python3 <file> <flag>`.

- T1. Unmodified branch file: `--collection-count` prints 38; `--self-test` exits 0.
- T2. Copy with one row removed (the SMA-708 row): `--collection-count` prints 37 (this proves
  that the copy parses); `--self-test` exits 0.
- T3. Copy with two rows removed (the same two rows as M1): `--collection-count` prints 36;
  `--self-test` exits 3, and stderr contains `COLLECTION_ROWS has only 36 row(s); the floor is 37`.
  Assert the stderr text, not only rc 3 (the row 8 lesson: rc 3 can come from another control).
- T4. Red-first control: run T3 against `origin/main`'s file. It exits 0 (this is M1). T3 must go
  from 0 to 3 only because of the change.
- T5. The gate's own controls: `bash ci/release-plan/run.sh --self-test` and
  `bash ci/release-plan/run.sh --negative-control` both pass on the branch.
- T6. `repo:actionlint`. Local runs need Homebrew bash 5 and a healthy pipe (root `CLAUDE.md`,
  "This development Mac only"). Read the pipe preflight line first. If the host has the 512-byte
  pipe, run the gate in a Linux container (memory: "Small pipe: run gates in a Linux
  container"). If no local verdict is possible, the CI result is the verdict for this AC, and the
  implementer records that fact. A doc that names moon's CI report file needs the
  `moon-diagnosis` marker (actionlint check 12); this change must not add that file name to any
  doc.
- T7. Gate-side mutation proof. This runs the edited `run.sh` line, not hand-typed arithmetic.
  1. Make a scratch root that holds `ci/release-plan/pyproject.toml`, `ci/release-plan/uv.lock`
     and the mutant copy as `ci/release-plan/release_plan.py`.
  2. From that root, run `bash -c` with these stubs: `infra(){ echo "INFRA: $*" >&2; exit 2; }`,
     `fail(){ echo "FAIL: $*" >&2; }`, `release_plan_sh(){ :; }`, and `SELF_TESTS_RAN=0`.
  3. `eval` the output of `sed -n '/^release_plan_self_test() {/,/^}/p' <branch>/ci/actionlint/run.sh`,
     then call `release_plan_self_test`.
  4. The 36-row copy must give rc 2 and stderr containing
     `reports 36 collection rows, expected at least 37`.
  5. The 37-row copy must give rc 0.
  6. Red-first: repeat the 36-row run with `origin/main`'s `run.sh`. It must give rc 0.
  7. Assert that the `sed` extraction is not empty, so that a renamed function cannot make T7
     pass vacuously.
- T8. Gates that the diff selects and that need a specific bash on this Mac:
  `repo:ruff-ci` (selected by `release_plan.py`, needs bash 4+) and `repo:affected-smoke`
  (selected by `ci/release-plan/**`, needs system bash 3.2). Before the push, run the full
  `ci-targets` command from root `CLAUDE.md`, then re-run these gates directly with the correct
  bash where the `moon ci` bash was wrong for them.

## 8. Files expected to change

- `ci/release-plan/release_plan.py` (the `self_test()` floor, its message and comment)
- `ci/actionlint/run.sh` (check 11 floor, its message and comment; the stale check 10 citation in
  the FIXTURES comment)
- `ci/release-plan/README.md` (row 8 prose, §4.3)
- `ci/release-plan/run.sh` (row 8 comment, §4.3)

## 9. Out of scope

- The FIXTURES floor (8 against 9 rows). It already tracks its table.
- A gate or a `ci_targets.py` pin that enforces that any floor tracks its table.
- A change of the gate-side `infra` (rc 2) classification to `fail` (rc 1). Q1 is decided: keep
  `infra`.

## 10. Residual risk

- Nothing enforces the re-baseline rule. A future PR that changes the row count and does not move
  both floors opens the same gap again, one row at a time. The two comments are the only control.
- A removal of exactly one row still reds nothing. This is the intended headroom.
- The SMA-708 §9 residual ("A deletion of the new row reds nothing") is NARROWED, not closed. The
  gap goes from 24 rows to 1 row. A deletion of the SMA-708 row alone, or of any other single
  row, still reds nothing after this change.
- The gate-side floor uses `infra`, which exits the whole gate at once. An intended removal that
  forgets the floors therefore aborts `repo:actionlint` as rc 2, and the checks after check 11 do
  not run in that invocation. The new message tells the author what to do.

## 11. Open questions

- Q1. Should the gate-side collection floor keep `infra` (rc 2) now that it is tight? The likely
  trigger is now an author's edit, which is rc 1 (`fail`) under this repo's contract. Check 10
  also uses `infra`, so a change needs a reason and is probably a separate ticket. Default in
  this spec: keep `infra`.
  **ANSWERED (Sven, 2026-09-28): keep `infra` (rc 2).**
- Q2. One row of headroom (floor = count − 1) or no headroom (floor = count)? Default in this
  spec: one row, to match check 10 and the FIXTURES floor. No headroom closes the one-row
  residual but requires a floor edit on every deliberate removal.
  **ANSWERED (Sven, 2026-09-28): floor = count − 1 (37), one row of headroom, in both places.**

## Re-check against `origin/main` at `906de46c` (2026-09-28)

One commit after `83d446fc` touches the cited files: `67982bc4` (SMA-716) edits
`ci/affected-graph/ci_targets.py`. It does not add a pin for the check 11 floors, and
`ACTIONLINT_SH_INDENTED_CALL_SITES` is still at line 1030. The measured facts hold:
`--collection-count` prints `38`, `--fixture-count` prints `9`, and `--self-test` exits `0`.

Corrections made in this re-check:

- §1: check 10's re-baseline comment is at `run.sh:4589-4590`, not `4587`.
- §4.2: the FIXTURES comment starts at `run.sh:4638` ("Floor, not a count"), not `4639`.
- §7 T6: added the Linux-container fallback for a small-pipe host and the check 12 marker note.

All other citations are unchanged and correct: `release_plan.py:1778-1782` (comment),
`1783-1784` (floor and message), `1744-1745` (the SMA-708 row), `run.sh:4625`
(`release_plan_self_test()`), `4642` (FIXTURES floor), `4650` (collection floor),
`README.md:153-154` and `ci/release-plan/run.sh:449`.

## Challenge changelog

Verdict from the spec-challenger: **APPROVE WITH CHANGES** (0 blocker, 1 major, 8 minor,
3 questions).

Folded:

- MAJOR, the gate-side mutation proof never ran the edited line: added T7, which extracts and
  runs the real `release_plan_self_test()` from `run.sh` with stubs against 36-row and 37-row
  copies, with a red-first run against `origin/main`. Added AC7; AC6 now covers the in-process
  side only.
- The SMA-708 §9 residual: §10 now says it is narrowed from 24 rows to 1 row, not closed. Added
  "floor = count" to the rejected alternatives with a reason, and as Q2.
- The check 10 comparison: removed the claim that check 11 has the same control as check 10.
  §2 and §3 now say that check 10 has a `ci_targets.py` whole-line pin and check 11 has only the
  twin floor. Corrected the quotation of `run.sh:4587` ("current `--fixture-count`").
- The two prose sites: §4.3 rewords `README.md:153-154` and `ci/release-plan/run.sh:449` so that
  they name "its floor", not a count. §8 lists both files. AC8 added.
- The failure messages: both messages now add "if the removal is intended, lower both floors in
  the same commit (SMA-709)". AC2 updated.
- The re-baseline rule: it now says "when the row count changes", in both directions (§3, §4).
- The mutation method: §7 now requires removal by label, a `cmp -s` non-vacuity check, and a
  `--collection-count` of 37 in T2 as proof that the copy parses.
- Q1 (stale "150 against 155" citation): now fixed in this PR, without numbers (§4.2, AC9),
  per the SMA-708 precedent. Removed from §9.
- Gates selected by the diff: T8 adds `repo:ruff-ci` (bash 4+) and `repo:affected-smoke`
  (bash 3.2), and the full `ci-targets` run before the push.
- Line citations: the twin comment is at `release_plan.py:1778-1782`. The new `run.sh` comment
  goes directly above the `-ge` line, or extends the existing "The COLLECTION_ROWS twin" comment.
- Challenger question on the documentation ACs: answered by §4.3 and AC8.
- Challenger question on headroom after a removal: answered in §3 (the floor moves down, one row
  of headroom stays).

Rejected: none. Every finding was checked against the code at `83d446fc` and holds. The
challenger question on `infra` versus `fail` is not rejected; it stays open as Q1, because a
change would also affect check 10's convention.
