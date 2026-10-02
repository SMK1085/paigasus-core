# SMA-637: A negative control for `repo:next-env-drift`

- **Linear:** [SMA-637](https://linear.app/smaschek/issue/SMA-637/ci-a-negative-control-for-reponext-env-drift)
- **Branch:** `feature/sma-637-next-env-negative-control`
- **Related:** SMA-512 (parameterized this gate, left the control out on purpose), SMA-601 (the
  "a gate that can lie" measurement), SMA-519 (the gate), SMA-502 / SMA-513 (the nearest sibling
  gates with a control).
- **Status:** APPROVED by Sven on 2026-10-01. Draft from an unattended Stage 0/1 run on
  2026-09-27. Stage 2 (challenge) ran on 2026-09-27: APPROVE WITH CHANGES, all findings folded
  (see §12). Re-checked against `origin/main` at `2c1e1171` on 2026-10-01 (see §13).

## 1. Problem

`ci/next-env/run.sh` (173 lines on `origin/main` at `2c1e1171`, unchanged since `83d446fc`) has no flag parsing. It has no
`--self-test` arm and no `--negative-control` arm. The root `moon.yml` runs it as a bare
`script: 'ci/next-env/run.sh'` (`moon.yml:138`).

The gate has three load-bearing assertions in `check_app`:

1. the file is tracked (`git ls-files --error-unmatch`, `run.sh:58`), else rc 2;
2. `next typegen` emitted the file again after the gate deleted it (`run.sh:84`), else rc 2;
3. `git diff --exit-code` finds no drift (`run.sh:90`), else rc 1.

The gate also has a fourth load-bearing part outside `check_app`: the aggregation loop
(`run.sh:165-173`) that keeps the highest rc and exits with it.

Nothing proves that assertion 3 can still fire, or that the loop passes the rc on. If a later edit
breaks either (for example, a `git diff` without `--exit-code`, a `check_app "$APP" || true`, or a
dropped `exit "$rc"`), the gate prints a pass for every app and CI stays green. That is the shape
SMA-601 measured for `ci/cargo-lock-integrity/run.sh`.

The comment at `run.sh:106-108` records that SMA-512 left the control out on purpose, because of
the registry cost. This issue pays that cost.

## 2. Facts (read on `origin/main` at `83d446fc`, 2026-09-27; re-checked at `2c1e1171`, 2026-10-01)

- `git log origin/main --grep SMA-637` finds no commit. The issue is not fixed.
- `ci/affected-graph/ci_targets.py` `check_self_scheduled_coverage` (`:2167`) reds
  `repo:affected-smoke` for any `repo:*` task whose resolved script contains `--self-test` or
  `--negative-control` and has no `SELF_SCHEDULED_GATES` key. `SELF_SCHEDULED_COVERAGE_EXEMPT`
  is empty and must stay empty.
- `check_registry_pairing` (`:2145-2165`) reds a `SELF_SCHEDULED_GATES` key that has neither a
  `SELF_TASK_EXPECTED_GLOBS` entry nor a `SELF_TASK_GLOBS_EXEMPT` entry, and reds a key in both.
- `REQUIRED_REPO_TASKS` (`ci_targets.py:186-218`) holds every sibling gate that has a
  `--negative-control`: `release-parity*`, `workflow-credentials`, `ruff-ci`, `next-public-free`,
  `helm-render`, `moon-diagnosis-exec` (SMA-714). Each entry's comment gives the same reason: `check_forward`'s `want` and `got`
  shrink together when a task is dropped from `T` and made CI-ineligible in one edit. Without the
  floor entry, the gate and its control can be switched off with every check green.
  `ci/CLAUDE.md:175-218` ("Registration is seven obligations") names this entry, and it is not
  self-enforcing.
- The task inputs today (`moon.yml:152-171`) are six globs and one literal:
  `ci/next-env/**/*`, `ts/apps/*/next-env.d.ts`, `ts/apps/*/package.json`,
  `ts/apps/*/next.config.ts`, `ts/apps/*/tsconfig.json`, `ts/apps/*/app/**/*`,
  `ts/pnpm-lock.yaml`.
- The nearest siblings (`next-public-free`, `helm-render`, `ruff-ci`, and since SMA-714
  `moon-diagnosis-exec`) each have a
  `SELF_SCHEDULED_GATES` entry, a `SELF_TASK_EXPECTED_GLOBS` entry, a `REQUIRED_REPO_TASKS` entry,
  and a `*_SH_CALL_SITES` tuple that pins the flag parse, the dispatch arms, the control's
  assertion lines and the real run's aggregation and exit lines in `run.sh`.
- `repo:affected-smoke` lists `ci/**/*` in its inputs, and `ci/actionlint/run.sh`
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` floors that entry (`:2147`). So an edit under
  `ci/next-env/` already schedules `repo:affected-smoke`. The siblings still add a narrow entry
  (`moon.yml:251`, `:257`, `:263`; `ci/actionlint/run.sh:2159`, `:2163`, `:2167`), because "the file's own policy
  is to keep narrow globs rather than collapse them into the broad one" (`moon.yml:244-250`).
- `ci_targets.py` cites `ci/next-env/run.sh:113` in two places (`:2215`, `:4299`). The cited line
  is now `:122`, so these citations are already stale. Line `:756` names the script but no line
  number, so it is not stale.
- `ts/CLAUDE.md:76-77` says "The next-env gate still has **no negative control**." This change
  makes that false.
- `iam-console-ts:test` and `:test-e2e` depend on `repo:next-env-drift`
  (`ts/apps/iam-console/moon.yml:300`, `:391`). The gateway app has the same edges
  (`ts/apps/gateway-console/moon.yml:276`, `:358`). So the gate is on the critical path of those
  chains.
- Nothing orders `*-ts:typecheck`, `ts:lint` or `ts:fmt` against `repo:next-env-drift`. The
  `typecheck` task depends on `contracts:generate` only.
- `lefthook.yml` has only `commit-msg` and `pre-push` hooks. No hook runs this gate, so no hook
  sets `GIT_INDEX_FILE` for it today.
- Moon resolves `bash` through `PATH`, and so does the `#!/usr/bin/env bash` shebang. On the
  development Mac that gives Homebrew bash 5.3.15, not `/bin/bash` 3.2 (memory:
  "affected-smoke hang: shim bash, don't prepend /bin").
- Probe (scratch repository, not this repo): a copy of the index with a planted blob for one
  path, used through `GIT_INDEX_FILE`, makes `git diff --exit-code -- <path>` return 1. The real
  index still returns 0. `git update-index --force-remove` on the copy makes
  `git ls-files --error-unmatch` return 1. The real index and the work tree did not change.

## 3. Approaches considered

**A. Temporary index plus a child process (recommended).** The control copies the real index to
a temporary file and plants a change in that copy for every app. It then runs the whole gate, in
check mode, as a child process with `GIT_INDEX_FILE` set to the copy. `next typegen` writes the
true content, and the planted index copy differs from it, so the child must exit exactly 1 and
name every app. The control uses the real app, the real `pnpm`, the real `next typegen`, the real
`git diff` line, the real discovery and the real aggregation and exit. It does not copy the app or
its `node_modules`. The real index does not change.

**A'. Temporary index, `check_app` called in a subshell.** The first draft of this spec. It
proves `check_app` only, not the aggregation loop or the final `exit`. It also needs a second
`EXIT` trap in the subshell. Replaced by A after the challenge (§12).

**B. Plant drift in a copied app tree.** Copy one app into a temporary git repository and change
its committed `next-env.d.ts`. This needs the app's `node_modules` and workspace links, so it is
slow and fragile. Rejected.

**C. A synthetic fixture with a fake `next`.** Put a stub `next` on `PATH` that writes a fixed
file. This proves the diff logic but not the real `pnpm exec next typegen` call. It is a
self-test, not a negative control. Rejected for this issue (see §8).

## 4. Design

### 4.1 `ci/next-env/run.sh`

Resolve the script's own absolute path before the `cd` at `:24`:

```bash
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
```

Add a flag parse and a mode dispatch at the end of the file, in the sibling form
(`ci/next-public/run.sh:512-525`):

```bash
MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --negative-control) MODE=negctl; shift ;;
    *) echo "next-env gate: unknown flag: $1" >&2; exit 2 ;;
  esac
done
```

The dispatch has exactly two arms, `check)` and `negctl)`, plus a `*)` arm that exits 2. A
`MODE` that matches no real arm must never exit 0 in silence.

Move the existing discovery and liveness code (`:109-160`) into a function `discover_apps` that
fills the global `apps` array. Both modes call it. Move the aggregation loop (`:165-173`) into a
function `real_run`, and keep its lines as they are: `rc=0`,
`check_app "$APP" || ec=$?`, the `-gt` comparison, `rc="$ec"`, and `exit "$rc"`.

The `EXIT` trap at `:44` (`restore_if_absent`) stays the only `EXIT` trap in check mode. In
`negctl` mode, one combined handler deletes the temporary index files and then calls
`restore_if_absent`. A second `trap … EXIT` must not replace the first one.

Add `negative_control`. It runs two rows. Each row runs the whole gate as a child process:

- **N1 drift.** Make a temporary index copy `$idx_drift` from `git rev-parse --git-path index`.
  For every app in `apps`: read the index blob `:<app>/next-env.d.ts`, append one line
  `// SMA-637 negative control`, write it with `git hash-object -w --stdin`, and assert
  `git cat-file -e "$blob"`. Put it in the copy with
  `GIT_INDEX_FILE="$idx_drift" git update-index --cacheinfo 100644,<blob>,<path>`.
  Then run
  `GIT_INDEX_FILE="$idx_drift" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?`.
  The row passes only when `rc_drift` is exactly 1 AND `$out_drift` contains, for every app,
  both the literal `-// SMA-637 negative control` and the literal
  `the committed <app>/next-env.d.ts does not match what Next generates`.
  The diff line starts with `-`, not `+`. `git diff` compares the index (the old side) with
  the work tree (the new side). The planted line is only in the index copy, so `git diff`
  prints it as a removed line. This was measured on 2026-10-01 (plan, correction 1).
- **N2 untracked.** Make a second copy `$idx_untracked`. For every app, remove the path with
  `GIT_INDEX_FILE="$idx_untracked" git update-index --force-remove -- <path>`. Then run the child
  with `GIT_INDEX_FILE="$idx_untracked"`. The row passes only when the rc is exactly 2 AND the
  output contains, for every app, the literal `'<app>/next-env.d.ts' exists but is NOT tracked by
  git`. This row costs no `typegen` call, because `check_app` returns before it.

Rules for both rows:

- `GIT_INDEX_FILE` must be an absolute path.
- Every git write in the control uses the `GIT_INDEX_FILE="$idx_…"` prefix. No git command in the
  control writes the real index.
- The two rows use different variable names (`$idx_drift`, `$idx_untracked`, `$out_drift`,
  `$out_untracked`), so every pinned line is unique in the file.
- Match output with `grep -qF -- "$lit" "$file"`. Do not pipe into `grep -q` (`repo:actionlint`
  check 13 bans it). Do not use a `<<<` here-string (the 512-byte pipe deadlock on the development
  Mac).
- The child has its own `EXIT` trap (`run.sh:44`), so the backup files are restored in the child.
  After each row, the control asserts that no `*.next-env-bak` file remains, else rc 2.
- Each row's output goes to its file only. The control prints that file to stderr only when the
  row does not pass. A passing row prints one line, so the buffered log of a later real-drift
  failure does not show the planted diff above the real one.

Row verdicts:

- The expected rc and every expected literal: PASS.
- N1 gives rc 2: `INFRA [N1] child exited 2 (infrastructure), expected 1`, print the child
  output, and the control exits 2. An infrastructure failure must never be reported as drift
  (the gate's own rule at `run.sh:162-164`). The helm-render control has a similar
  `INCONCLUSIVE` arm.
- Any other wrong rc (0, or 1 for N2, or any other value), or a missing literal:
  `FAIL [<row>] expected rc <n>, got <m>` or `FAIL [<row> <app>] missing '<literal>'`. Count it
  and continue. At the end the control exits 1 if any row failed.
- All rows pass: print `== next-env-drift negative control passed ==` and exit 0.

Preconditions and post-conditions, each rc 2 (`INFRA: …`):

- `GIT_INDEX_FILE` is unset at control start. If it is set (to any value, also an empty
  string), the control exits 2 with a clear `INFRA:` message, because
  `git rev-parse --git-path index` would then return a caller's temporary index. The control does
  not copy the caller's index. No hook sets the variable today (§2). Decided by Sven on
  2026-10-01 (§11 question 2).
- The planted blob differs from `git rev-parse :<path>` (index stage 0) for every app.
- `git ls-files -s -- <each app path>` in the real index is the same before and after the control.

Remove the NOTE at `:106-108` and replace it with one line that names SMA-637. Add a short
paragraph to the header comment on the control, on why it uses a temporary index, and on why it
runs the gate as a child process.

Keep the script bash 3.2 compatible and bash 5.3.15 compatible: no `mapfile`, no `declare -A`,
no `${var,,}`, no `<<<`.

### 4.2 `moon.yml`

Rewrite the task to the three-invocation form of the siblings:

```yaml
    script: |
      set -euo pipefail
      bash ci/next-env/run.sh --negative-control
      bash ci/next-env/run.sh
```

Keep the `deps`, the `toolchain` and every input without change. Add the sibling comment that
says why `set -euo pipefail` is required and that `SELF_SCHEDULED_GATES` pins the three lines.

Add `ci/next-env/**/*` to the inputs of `repo:affected-smoke`, next to `ci/next-public/**/*` and
`ci/helm-render/**/*`, with the sibling comment. This follows the file's own policy of narrow
globs (§2), although `ci/**/*` already covers scheduling.

### 4.3 `ci/affected-graph/ci_targets.py` and `ci/actionlint/run.sh`

1. `SELF_SCHEDULED_GATES["next-env-drift"]` with the three lines of §4.2, whole-line matched.
2. `SELF_TASK_EXPECTED_GLOBS["next-env-drift"]`, in `check_gate_inputs` comparison order
   (globs sorted, then files sorted). The expected value, to be MEASURED with
   `moon query projects` before it is written:
   `("ci/next-env/**/*", "ts/apps/*/app/**/*", "ts/apps/*/next-env.d.ts",
   "ts/apps/*/next.config.ts", "ts/apps/*/package.json", "ts/apps/*/tsconfig.json",
   "ts/pnpm-lock.yaml")`.
   An EXPECTED entry, not an exemption: the set is small and static, and dropping
   `ts/pnpm-lock.yaml` reopens the exact defect SMA-519 exists for. The cost is known: the day an
   app with `next.config.mjs` lands, the `moon.yml` comment already requires a new glob, and this
   entry must change in the same commit.
3. `"next-env-drift"` in `REQUIRED_REPO_TASKS`, with the sibling comment. Add it also to the
   self-test `tasks_fixture` and to `aligned_t` (`ci_targets.py:2405-2426`).
4. A `NEXT_ENV_SH_CALL_SITES` tuple, stripped whole lines, each verified to occur once in
   `run.sh`:
   - the `SELF=` line, the `--negative-control)` parse arm, the `check)` and `negctl)` dispatch
     arms and the `*)` dispatch arm that exits 2;
   - the N1 and N2 child invocation lines, the N1 and N2 rc comparisons, the N1 INFRA arm, the
     `grep -qF` literal checks, and the failure-count guard;
   - the real `git diff --exit-code` line in `check_app`;
   - in `real_run`: the `check_app "$APP" || ec=$?` loop call, the `-gt` comparison, the
     `rc="$ec"` line and the final `exit "$rc"`.
   Wire it into `check_self_invocation` as a new required parameter `next_env_sh_text`. The full
   wiring list, so the plan can size it:
   - the new parameter in the signature (`:1921-1925`), the docstring list (`:1961`), a
     `next_env_lines` whole-line set next to `moon_diagnosis_lines` (`:2041-2046`), and the
     `_param_name` introspection loop (`:3213-3217`);
   - every existing `check_self_invocation(` call gets the new argument (`grep -c
     'check_self_invocation('` gives 69 lines on `origin/main` at `2c1e1171`; that count includes
     the `def` line and a few f-string lines, so the plan must count the real calls);
   - a `wired_next_env` fixture next to `wired_moon_diagnosis` (`:2881-2891`);
   - in `main()`: a `read_input` call (`:4265-4296`), an `sh` dict key (`:4316-4324`) and the
     `collect_findings` call (`:4050-4053`);
   - the same deletion, contamination and commented-out battery that
     `NEXT_PUBLIC_FREE_SH_CALL_SITES` has (`:3446-3480`), plus the neutered-guard row
     (`:3481-3494`). The newest copy of this battery is the moon-diagnosis one (`:3547-3599`,
     SMA-714); use it as the template.
5. Correct the two stale `ci/next-env/run.sh:113` citations (`:2215`, `:4299`) to name the
   function `discover_apps` instead of a line number, so the next edit cannot make them stale
   again.
6. `ci/actionlint/run.sh`: add `'ci/next-env/**/*'` to `T_AFFECTED_SMOKE_REQUIRED_INPUTS`
   (next to `:2159`, `:2163`, `:2167`), with the sibling comment. Check 8e floors this array's
   length with `-ge 20` (`:5784`); the array has 27 entries today. The floor is a lower bound, so
   growth needs no floor change. Do not change the floor line: `ACTIONLINT_SH_CALL_SITES` pins
   it.

### 4.4 Documentation

- `ci/CLAUDE.md`: no entry names this gate today. Add nothing unless the plan finds a table of
  self-scheduled gates there.
- `ts/CLAUDE.md:76-77`: replace "The next-env gate still has **no negative control**." with one
  sentence that names the control and SMA-637. Keep the rest of the bullet.
- The header comment of `ci/next-env/run.sh` (§4.1).

## 5. Error handling

- Real run exit codes do not change: 0 clean, 1 drift, 2 infrastructure.
- Control: 0 all rows as expected; 1 a row gave a wrong rc other than an infrastructure rc, or a
  literal was missing; 2 the control could not set up (index copy, `hash-object`, `cat-file -e`,
  `update-index`, a precondition or post-condition in §4.1), or N1's child exited 2.
- An unknown flag exits 2. An unknown `MODE` exits 2.
- The work tree after the control: N1 leaves the regenerated file, which equals the committed
  content when there is no real drift. The real run that follows then reports any real drift as
  before. The real index is never written, and the post-condition proves it.
- `git hash-object -w` writes one loose, unreachable object per app into `.git/objects`. The
  planted content is deterministic, so every run gives the same object ID: at most one object
  per committed version of each file. `git gc` removes it. This is the only write outside the
  temporary files, and it is accepted.

## 6. Acceptance criteria

1. `bash ci/next-env/run.sh --negative-control` exits 0 on a clean tree and prints one pass line.
2. Behaviour mutations of `run.sh`, each measured:
   (a) delete `--exit-code` from the `git diff` line in `check_app`: the control exits 1 and names
   `N1`;
   (b) delete the tracked-file check: the control exits 1 and names `N2`;
   (c) change the `real_run` loop call to `check_app "$APP" || true`: the control exits 1;
   (d) delete `exit "$rc"` from `real_run`: the control exits 1;
   (e) make `typegen` fail (for example, a bad `pnpm --dir`): the control exits 2 with `INFRA`,
   not 1.
3. After the control, `git status --porcelain` and `git diff --cached --stat` show no change
   caused by the control, and no `*.next-env-bak` file remains.
4. `bash ci/next-env/run.sh` behaves as before: exit codes and messages do not change.
5. The `moon.yml` task has the three-line script, and `repo:next-env-drift` passes under
   `moon run repo:next-env-drift --force`.
6. `repo:affected-smoke` passes with the registry entries, the `REQUIRED_REPO_TASKS` entry, the
   new `repo:affected-smoke` input and the call-site tuple.
7. Registry mutations, each measured to red `repo:affected-smoke` (compile-clean, per the memory
   rule "a mutation must compile"): (a) delete the `--negative-control` line from `moon.yml`;
   (b) delete `set -euo pipefail`; (c) delete the `SELF_SCHEDULED_GATES` entry while the flag
   stays in `moon.yml`; (d) delete `ts/pnpm-lock.yaml` from the task inputs; (e) delete each
   `NEXT_ENV_SH_CALL_SITES` line from `run.sh` in turn; (f) delete
   `SELF_TASK_EXPECTED_GLOBS["next-env-drift"]` (the unpinned row of `check_registry_pairing`
   must red); (g) drop `:next-env-drift` from `T` and make it CI-ineligible in the same edit (the
   `REQUIRED_REPO_TASKS` floor must red).
8. The gate and the control run under system `/bin/bash` 3.2.57, under Homebrew bash 5.3.15 (the
   bash that Moon resolves on the development Mac, on a host whose pipe preflight passes), and
   under the CI Linux bash.
9. The two stale `run.sh:113` citations in `ci_targets.py` are gone.
10. `ts/CLAUDE.md` no longer says the gate has no negative control.

## 7. Test strategy

- Run the control and the real run locally under `/bin/bash` 3.2 and Homebrew bash 5.3.15
  (§6 items 1, 3, 4, 8).
- Mutation battery for §6 item 2, restoring each mutation by reverting the one inserted edit, not
  by `git checkout --` (memory: "mutation restore discards the fix"). Re-run the whole battery
  after any fix.
- `python3 ci/affected-graph/ci_targets.py --self-test` must pass with the new battery rows, then
  the mutation battery of §6 item 7 through `ci/affected-graph/run.sh`. Use system bash 3.2 for
  `repo:affected-smoke` (root `CLAUDE.md`).
- Measure the `SELF_TASK_EXPECTED_GLOBS` order from `moon query projects` before writing it.
- Record the added CI time on the CRITICAL PATH, not only the task time: the gate sits before
  `iam-console-ts:test`, `:test-e2e` and the gateway equivalents (§2). Estimate: one extra
  `next typegen` per app in N1 (about 1.5 s each, per the comment at `run.sh:73-74`), so about 3 s
  for two apps. N2 costs no `typegen`.
- Run the full gate graph of the root `CLAUDE.md` before push.

## 8. Out of scope

- A `--self-test` arm with a fixture table for discovery, the liveness assertion and the rc
  aggregation. The child-process design now proves the aggregation and the exit for the drift
  and untracked cases, but not the liveness assertion. It is a follow-up candidate.
- Widening the `next.config.*` input globs. The `moon.yml` comment defers that until an app needs
  it.
- Ordering `*-ts:typecheck`, `ts:lint` and `ts:fmt` against this gate (see §10). Sven decided on
  2026-10-01: no new `deps` edges (§11 question 1).

## 9. Files expected to change

- `ci/next-env/run.sh`
- `moon.yml` (the `next-env-drift` task, and one input of `repo:affected-smoke`)
- `ci/affected-graph/ci_targets.py`
- `ci/actionlint/run.sh` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`)
- `ts/CLAUDE.md`

## 10. Residual risk

- The control proves that the gate exits 1 on a planted index difference. It does not prove
  that `next typegen` output matches what `next build` writes. That was true before this change.
- `GIT_INDEX_FILE` must reach every git call in `check_app`. A future edit that adds
  `git -C <other dir>` or a `--git-dir` would bypass the copy. The N1 and N2 rows would then fail
  loudly, not pass silently, because the real index has no drift.
- The control is coupled to the index. A legitimate change of the diff line to
  `git diff HEAD --exit-code` ignores the planted index, so N1 reds. That is a known false red:
  whoever makes such a change must change the control in the same commit.
- `NEXT_ENV_SH_CALL_SITES` pins text, not behaviour. The control is what proves behaviour.
- The delete window doubles. Each app's `next-env.d.ts` is now absent twice per run (once in the
  N1 child, once in the real run), not once. Other readers of the file are not ordered against
  this task: `*-ts:typecheck`, `ts:fmt` and the Moon hasher. This race existed before; the
  control makes the window about twice as long. ACCEPTED by Sven on 2026-10-01 (§11 question 1).
- A control failure now also skips `iam-console-ts:test`, `:test-e2e` and the gateway
  equivalents, because they depend on this task.

## 11. Open questions

Both questions are ANSWERED (Sven, 2026-10-01). The answers equal the spec defaults, so the
scope did not change.

1. Is a second window per run with each `next-env.d.ts` absent acceptable, given that nothing
   orders `*-ts:typecheck`, `ts:lint` or `ts:fmt` against `repo:next-env-drift`? This spec
   accepts it as a residual risk (§10). The alternative is new `deps` edges, which lengthen the
   critical path for every PR that selects those tasks.
   **ANSWER: yes, accept the longer window. Add no new `deps` edges between `*-ts:typecheck`,
   `ts:lint` or `ts:fmt` and `repo:next-env-drift`.**
2. Should the control refuse to run when `GIT_INDEX_FILE` is already set (this spec: yes, rc 2),
   or should it copy whatever index the caller uses? No hook sets the variable today; SMA-371
   lefthook work could add a pre-commit hook later.
   **ANSWER: refuse. If `GIT_INDEX_FILE` is set when the control starts, the control exits rc 2
   (§4.1).**

## 12. Challenge changelog

**Verdict:** APPROVE WITH CHANGES (spec-challenger, 2026-09-27). No blocker.

**Folded (all checked against the repo on `main` at `4051df5e`):**

- MAJOR `REQUIRED_REPO_TASKS` missing: confirmed at `ci_targets.py:186-213` and
  `ci/CLAUDE.md:191-200`. Added as §4.3 item 3, §6 item 7(g), §9.
- MAJOR the control skipped `real_run`: switched to the child-process design (§3 A, §4.1). The
  `real_run` loop call, `-gt`, `rc="$ec"` and `exit "$rc"` are also pinned (§4.3 item 4). Added
  §6 item 2(c), 2(d).
- MAJOR infrastructure failures reported as rc 1: N1 rc 2 is now `INFRA` and control exit 2
  (§4.1, §5, §6 item 2(e)).
- MAJOR rows judged by rc alone: each row now also requires literals in its captured output, and
  a `git cat-file -e` precondition. `grep -qF` on a file, no pipe, no here-string (§4.1).
- MAJOR `GIT_INDEX_FILE` scope contradiction: every git write uses the prefix; a real-index
  post-condition gives rc 2 (§4.1).
- MAJOR open question 1 (call-site tuple in scope): removed; the tuple and the `*)` dispatch arm
  are in scope (§4.1, §4.3).
- MINOR wiring list incomplete: listed in §4.3 item 4, with the counted 65 calls.
- MINOR pinned lines not unique: per-row variable names (§4.1).
- MINOR stale-citation count: corrected to two (§2, §4.3 item 5, §6 item 9).
- MINOR `ts/CLAUDE.md` becomes false: added (§4.4, §6 item 10, §9).
- MINOR bash premise wrong: Moon resolves Homebrew bash 5.3.15; added to §2, §4.1, §6 item 8,
  §7; `<<<` forbidden.
- MINOR trap replacement: resolved by the child process; negctl mode uses one combined handler
  (§4.1).
- MINOR misleading log output: row output captured and printed only on failure (§4.1).
- MINOR critical path: measured on the critical path (§2, §7, §10).
- MINOR delete window doubles: added to §10 and open question 1.
- MINOR coupling to the index: added to §10.
- MINOR narrow reachability entry: added to `moon.yml` and `T_AFFECTED_SMOKE_REQUIRED_INPUTS`
  (§4.2, §4.3 item 6).
- MINOR "committed blob" ambiguous: the spec now says `:<path>` (index stage 0) everywhere.
- MINOR open question 3 (loose object): closed; the object ID is deterministic, so it is
  accepted (§5).
- MINOR AC 7 misses a mutation: added §6 item 7(f).
- QUESTION child-process design: accepted; it also closes the former open question 2 (every app
  runs, with one extra `typegen` per app in N1 and none in N2).
- QUESTION ordering of `typecheck`/`ts:lint`/`ts:fmt`: checked, nothing orders them; kept as open
  question 1.
- QUESTION inherited `GIT_INDEX_FILE`: checked, no hook sets it today; the control refuses it
  (§4.1), kept as open question 2 for Sven's decision.

**Rejected:** none.

## 13. Re-check against `origin/main` (2026-10-01)

Main moved from `83d446fc` to `2c1e1171` after the spec was written. SMA-714 added
`repo:moon-diagnosis-exec`, and SMA-709 and SMA-679 changed `ci/actionlint/run.sh` and
`ci/helm-render/`. Every cited path, line and registration table was read again.

**Unchanged:** `ci/next-env/run.sh` (173 lines, every cited line), `moon.yml:138` and `:152-171`
(the task inputs), `moon.yml:244-251` and `:257`, `ci/actionlint/run.sh:2147`, `:2159`, `:2163`,
`ci/next-public/run.sh:512-525`, `ts/CLAUDE.md:76-77`, the `ts/apps/*/moon.yml` `deps` lines,
`lefthook.yml` (only `commit-msg` and `pre-push`), the `typecheck` `deps`
(`contracts:generate` only), and `SELF_SCHEDULED_COVERAGE_EXEMPT` (still empty, now `:751`). No
commit on `origin/main` names SMA-637. The spec does not name the moon CI report file, so it
needs no moon-diagnosis marker (`repo:actionlint` check 12).

**Changed in this spec:**

- §1, §2: the base commit is now `2c1e1171`.
- §2: `check_self_scheduled_coverage` `:2029` -> `:2167`; `check_registry_pairing`
  `:2010-2026` -> `:2145-2165`; `REQUIRED_REPO_TASKS` `:186-213` -> `:186-218`, and it now also
  holds `moon-diagnosis-exec`; `ci/CLAUDE.md:175-200` -> `:175-218`.
- §2: `moon-diagnosis-exec` (SMA-714) is a new sibling with the same registry set, and its
  narrow `repo:affected-smoke` input is at `moon.yml:263` and `ci/actionlint/run.sh:2167`.
- §2, §4.3 item 5: the stale `run.sh:113` citations moved from `:2077`, `:4093` to `:2215`,
  `:4299`; the line-free mention moved from `:735` to `:756`.
- §4.3 item 3: `tasks_fixture` and `aligned_t` `:2267-2285` -> `:2405-2426`; both now also hold
  `moon-diagnosis-exec`.
- §4.3 item 4: the `_param_name` loop `:3066-3070` -> `:3213-3217`; the call count 65 -> 69
  matching lines (the plan must count real calls); `read_input` `:4067-4090` -> `:4265-4296`;
  `sh` dict `:4109-4116` -> `:4316-4324`; `collect_findings` call `:3849-3852` -> `:4050-4053`;
  next-public-free battery `:3299-3330` -> `:3446-3480`, neutered-guard row `:3334-3346` ->
  `:3481-3494`. The old `:3385-3398` row is now the moon-diagnosis battery (`:3547-3599`), named
  as the template. Added the signature, the docstring, the whole-line set and the `wired_*`
  fixture to the wiring list, because SMA-714's diff shows these four places too.
- §4.3 item 6: check 8e's floor is `-ge 20` at `ci/actionlint/run.sh:5784` (27 entries today), a
  lower bound pinned by `ACTIONLINT_SH_CALL_SITES`. The conditional "update the floor if it is a
  literal count" is resolved: no floor change.
- §4.1, §8, §10, §11: the two approval decisions are recorded.
