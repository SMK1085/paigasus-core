# SMA-637: A negative control for `repo:next-env-drift` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `ci/next-env/run.sh` a `--negative-control` mode that proves the gate can still go red, and register it in the gate registries so that nobody can switch the control off with every check green.

**Architecture:** The control copies the real git index to a temporary file, plants a change in that copy for every app, and runs the whole gate in check mode as a child process with `GIT_INDEX_FILE` set to the copy. Row N1 plants drift (the child must exit 1); row N2 removes the file from the copy (the child must exit 2). The real index never changes. The moon task runs the control first, then the real run, in one `set -euo pipefail` block. `ci_targets.py` pins the three task lines, the inputs, the floor entry and the load-bearing lines of `run.sh`.

**Tech Stack:** bash (3.2 and 5.x compatible), git plumbing (`update-index --cacheinfo`, `hash-object -w`), `pnpm exec next typegen`, Python 3 (`ci/affected-graph/ci_targets.py`), moon 2.5.3 through proto.

**Spec:** `docs/superpowers/specs/2026-10-01-sma-637-next-env-negative-control-design.md` (APPROVED by Sven on 2026-10-01). Executors read the spec and this plan.

## Global Constraints

- Base: `origin/main` at `2c1e1171`. The spec was re-checked at that commit (spec §13). All line numbers below were read again on that commit for this plan.
- Exit codes do not change for the real run: 0 clean, 1 drift, 2 infrastructure.
- Control exit codes: 0 every row as expected; 1 a row gave a wrong rc (not an N1 rc 2) or a literal was missing; 2 setup failed, a precondition or post-condition failed, or the N1 child exited 2.
- If `GIT_INDEX_FILE` is set (also to an empty string) when the control starts, the control exits 2 with an `INFRA:` message (Sven, 2026-10-01).
- No new `deps` edges between `*-ts:typecheck`, `ts:lint`, `ts:fmt` and `repo:next-env-drift` (Sven, 2026-10-01). The longer delete window is accepted.
- `run.sh` stays bash 3.2 and bash 5.3.15 compatible: no `mapfile`, no `declare -A`, no `${var,,}`, no here-string (`<<<`), no heredoc, no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `sed … q`, `awk … exit`) (`repo:actionlint` check 13).
- Every git write in the control has the prefix `GIT_INDEX_FILE="$idx_…"`. No git command in the control writes the real index. `git hash-object -w` (one loose object per app) is the only write outside the temporary directory, and it is accepted (spec §5).
- The planted-line literal is `// SMA-637 negative control`. The literal is the same in `run.sh` and in this plan.
- No new or changed file contains the token of the moon CI report file name (the word that `repo:actionlint` check 12 scans for). Do not write it in a comment, a message or a commit body.
- New repo text is written in ASD-STE100 Simplified Technical English.
- Conventional commits with a workspace scope (`ci` or `ts`). Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never `--no-verify`, `--no-gpg-sign`, `--amend`, `git reset`, or a force push. Before each commit, `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control branch --show-current` must print `feature/sma-637-next-env-negative-control`.
- Mutation runs are authorized (Sven, 2026-09-28): a temporary edit, a run that must go red, then a restore with the Edit tool (never `git checkout --`). Confirm the restore with `git diff`. Never commit a mutation.
- Every shell command starts with `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`. Do not install host software.
- The Mac can be in the 512-byte small-pipe state (SMA-612). `repo:actionlint` needs bash 5 and a healthy pipe; if its preflight reports a small pipe, run it in a Linux container (memory "Small pipe: run gates in a Linux container").

## Deviations from the spec (decided by this plan)

1. **The N1 diff literal is `-// SMA-637 negative control`, not `+// …`.** The spec (§4.1) says `+`. MEASURED on 2026-10-01 in a scratch repository: `git diff` compares the index (old side) with the work tree (new side). The planted line is in the index copy and not in the regenerated file, so `git diff` prints it as a removed line: `-// marker`, rc 1. A `+` literal would make N1 fail on every run.
2. **The N1 marker check counts lines, not one match per app.** The marker text is the same for every app, so a per-app `grep -qF` of it proves nothing per app. The control counts the lines that contain the marker (`grep -cF`) and requires the count to equal the number of apps. The per-app message literal (`the committed <app>/next-env.d.ts does not match what Next generates`) stays a per-app `grep -qF`.
3. **The marker count uses `grep -cF`, not `grep -cxF`.** A host with `color.diff=always` wraps the diff line in ANSI codes. A whole-line match would then give a false red. A substring match still finds the marker (Review Focus 1).
4. **The control makes its temporary directory absolute itself** (`cd … && pwd -P`) rather than refusing a relative `TMPDIR`. The spec rule "`GIT_INDEX_FILE` must be an absolute path" is kept: the path is absolute by construction, and a `case` check asserts it.
5. **Additional pins and preconditions.** `NEXT_ENV_SH_CALL_SITES` also pins the `GIT_INDEX_FILE` refusal guard and the real-index post-condition line, because both carry an approval decision or a spec rule. The N2 row also asserts, before the child runs, that the index copy no longer lists each path (rc 2 if it does).
6. **`hash-object` gets `--no-filters`,** so a `.gitattributes` filter or `core.autocrlf` cannot change the planted blob.

## Review Focus

These are the five inputs that the spec implies but that no acceptance criterion exercises. Each one has a test in Task 1 Step 7 or Task 4.

1. **A host git config that forces colour** (`color.diff=always` or `color.ui=always`). A person expects the control to pass. The marker match is a substring match for this reason. Test: Task 1 Step 7 row RF1.
2. **The control started from another directory or through a relative path** (`cd ts && bash ../ci/next-env/run.sh --negative-control`). A person expects the same pass. `SELF` is resolved before the `cd` to the top level. Test: Task 1 Step 7 row RF2.
3. **`GIT_INDEX_FILE` already set, also to an empty string.** A person expects rc 2 with a clear message, and no change to any index or to the work tree. Test: Task 1 Step 7 row RF3.
4. **`TMPDIR` relative, or with a space in it.** A person expects a pass, not a git error about a bad index path. Test: Task 1 Step 7 row RF4.
5. **An unknown flag, for example `--self-test`, which the siblings have.** A person expects rc 2 with `unknown flag` and no `next typegen` run and no deleted file. Test: Task 1 Step 7 row RF5.

## Measurements taken for this plan (2026-10-01)

- `git diff --exit-code -- f` with a temporary index whose blob has one extra last line `// marker` prints `-// marker` and exits 1. The real index still exits 0. (Scratch repository, git on macOS.)
- `git rev-parse --git-path index` prints an ABSOLUTE path in this worktree (`/Users/…/.git/worktrees/sma-637-next-env-negative-control/index`) and a RELATIVE path (`.git/index`) in a plain checkout. The control copies it after the `cd` to the top level, so both forms work.
- Both committed `ts/apps/*/next-env.d.ts` files end with a newline, so the appended marker is its own line.
- `moon query projects`, `repo:next-env-drift`: `inputGlobs` sorted, without the implicit `.moon/*.{…}` entry, are `ci/next-env/**/*`, `ts/apps/*/app/**/*`, `ts/apps/*/next-env.d.ts`, `ts/apps/*/next.config.ts`, `ts/apps/*/package.json`, `ts/apps/*/tsconfig.json`; `inputFiles` is `ts/pnpm-lock.yaml`. That is the order of spec §4.3 item 2.
- `ci_targets.py` holds 67 `check_self_invocation(` CALLS (AST count; the `grep -c` count of 69 includes the `def` line and one docstring line). All 67 have exactly ten positional arguments and no keyword argument. 66 end with a moon-diagnosis haystack (`wired_moon_diagnosis`, `_md_broken`, `_md_commented`, `_md_guard_neutered` or the contamination `"".join(…)`), and one (in `collect_findings`) ends with `sh["moon_diagnosis"]`.
- `py/pyproject.toml` ignores `E501`, so longer call lines do not red `repo:ruff-ci`.
- The exact `run.sh` of Task 1 Step 3 was extracted from this plan to a scratch path and run against this worktree (no repo file changed): `--negative-control` rc 0 in about 2 s under `/bin/bash` 3.2, rc 0 under Homebrew bash 5.3.15, RF1 rc 0, RF3 (`GIT_INDEX_FILE=`) rc 2 with the `INFRA` message, a relative `TMPDIR` rc 0, M2a rc 1 (`FAIL [N1] expected rc 1, got 0`), M2c rc 1, M2e rc 2 (`INFRA [N1] …`). `git status` was clean after each run.

## File structure

| File | Change | Task |
|---|---|---|
| `ci/next-env/run.sh` | `SELF`, `discover_apps`, `real_run`, `negative_control`, flag parse, dispatch, header paragraph, NOTE replaced | 1 |
| `moon.yml` | `next-env-drift` script becomes three lines; `ci/next-env/**/*` added to `affected-smoke` inputs | 2 |
| `ci/affected-graph/ci_targets.py` | `REQUIRED_REPO_TASKS`, `SELF_TASK_EXPECTED_GLOBS`, `SELF_SCHEDULED_GATES`, `NEXT_ENV_SH_CALL_SITES`, `check_self_invocation` wiring, self-test fixture and battery, `main()` wiring, two stale citations | 2 |
| `ci/actionlint/run.sh` | `'ci/next-env/**/*'` in `T_AFFECTED_SMOKE_REQUIRED_INPUTS` | 2 |
| `ts/CLAUDE.md` | the "no negative control" sentence replaced | 3 |

`ci/CLAUDE.md` has no table of self-scheduled gates (checked: lines 145-230 are prose about the registration obligations, with named examples). Per spec §4.4, it gets no change.

---

### Task 1: The negative control in `ci/next-env/run.sh`

**Files:**
- Modify: `ci/next-env/run.sh` (whole file; 173 lines today)
- Test: the script itself (`--negative-control`, the real run, and the Review Focus rows)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: these exact lines, each once in `run.sh` (Task 2 pins them in `NEXT_ENV_SH_CALL_SITES`; leading whitespace does not matter, the pin strips it):
  - `SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"`
  - `--negative-control) MODE=negctl; shift ;;`
  - `check)  real_run ;;`
  - `negctl) negative_control ;;`
  - `*) echo "next-env gate: unknown mode '$MODE'" >&2; exit 2 ;;`
  - `if [ "${GIT_INDEX_FILE+set}" = set ]; then`
  - `GIT_INDEX_FILE="$idx_drift" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?`
  - `if [ "$rc_drift" -eq 2 ]; then`
  - `echo "INFRA [N1] child exited 2 (infrastructure), expected 1" >&2`
  - `if [ "$rc_drift" -ne 1 ]; then`
  - `n_marker="$(grep -cF -- '-// SMA-637 negative control' "$out_drift" || true)"`
  - `if [ "$n_marker" != "${#apps[@]}" ]; then`
  - `if ! grep -qF -- "the committed $a/next-env.d.ts does not match what Next generates" "$out_drift"; then`
  - `GIT_INDEX_FILE="$idx_untracked" "$BASH" "$SELF" >"$out_untracked" 2>&1 || rc_untracked=$?`
  - `if [ "$rc_untracked" -ne 2 ]; then`
  - `if ! grep -qF -- "'$a/next-env.d.ts' exists but is NOT tracked by git" "$out_untracked"; then`
  - `cmp -s "$NEGCTL_TMP/index-before" "$NEGCTL_TMP/index-after" || negctl_infra "the real index entries for next-env.d.ts changed during the control"`
  - `if [ "$fails" -ne 0 ]; then`
  - `if ! git diff --exit-code -- "$FILE"; then`
  - `check_app "$APP" || ec=$?`
  - `if [ "$ec" -gt "$rc" ]; then`
  - `rc="$ec"`
  - `exit "$rc"`
  - The pass line printed on stdout: `== next-env-drift negative control passed ==`

- [ ] **Step 1: Take the baseline of the real run**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/next-env/run.sh; echo "rc=$?"; git status --porcelain
```
Expected: two lines `next-env gate: ts/apps/<app>/next-env.d.ts matches 'next typegen' output.` (gateway-console, iam-console), `rc=0`, and no `git status` output. If `rc` is not 0, stop: the tree has real drift or `ts/node_modules` is missing (`pnpm -C ts install`). Do not continue on a red baseline.

- [ ] **Step 2: Run the control before it exists (red first)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/sma637-red.out" 2>&1; echo "rc=$?"; grep -cF '== next-env-drift negative control passed ==' "$TMPDIR/sma637-red.out"
```
Expected: the old script ignores the flag and runs the real gate, so `rc=0` and the count is `0`. The pass line is absent: this is the red state.

- [ ] **Step 3: Write the new `ci/next-env/run.sh`**

Replace the whole file with this content. Lines 1-99 of the old file stay the same except for two insertions: the new header paragraph (after line 21) and the `SELF=` line (before the `cd`). `check_app` does not change.

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Drift gate for the generated-but-tracked next-env.d.ts (SMA-519).
#
# next-env.d.ts is emitted by Next but committed, because tsconfig.json lists it in
# `include` and `typecheck` runs without a Next build. Nothing asserted the committed copy
# matched what Next actually emits, and it sat stale from the ts/ bootstrap in May until
# SMA-517: a newer Next also emits an `./.next/types/root-params.d.ts` reference.
#
# WHY THE BUILD DID NOT CATCH IT — iam-console-ts:build declares inputs
# ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts']. ts/pnpm-lock.yaml
# is NOT among them, so a Next upgrade never re-keys the task: the build stays cached, the
# file is never regenerated, and the drift is invisible. During SMA-517 a full `moon ci`
# reported the file clean and it took `--force` to surface. Hence this gate keys on the
# lockfile (see moon.yml), which is what actually drives this file's content.
#
# WHY DELETE-THEN-REGENERATE — diffing a file that nothing rewrote is a vacuous assertion:
# if a future Next stops emitting next-env.d.ts, a naive `typegen && git diff` would pass
# forever while guarding nothing. Removing it first makes the gate self-proving — an absent
# file is a loud failure, not a silent pass.
#
# THE NEGATIVE CONTROL (SMA-637) — `--negative-control` proves that this gate can still go red.
# It copies the real git index to a temporary file and changes the copy for every app: row N1
# appends one line to each staged next-env.d.ts, and row N2 removes each one from the copy. Then
# it runs this whole script in check mode as a CHILD PROCESS, with GIT_INDEX_FILE set to the
# copy. N1 must exit 1 and N2 must exit 2. A temporary index, because the control must not
# change the real index, the work tree or a commit. A child process, because a call of
# check_app alone would not prove the aggregation loop and the final `exit` in real_run.
# Exit codes: 0 every row as expected, 1 a row failed, 2 an infrastructure error.
set -euo pipefail

# Resolve this script's own absolute path BEFORE the cd below. The negative control runs this
# same file again as a child process.
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

cd "$(git rev-parse --show-toplevel)"

# If typegen dies before writing a file, restore it rather than leaving the tree broken. A
# DRIFTING file is deliberately left in place: it is the corrected content, ready to commit.
#
# RESTORE FROM A BACKUP, NEVER FROM THE INDEX. `git checkout -- "$f"` restores the INDEX copy,
# which silently discards any UNSTAGED working-tree content the file had before `rm`. That is a
# reachable data loss, not a hypothetical: this gate's own drift path leaves a regenerated
# next-env.d.ts unstaged, so a second run whose typegen failed would have destroyed it.
RESTORE_FILES=()
restore_if_absent() {
  local f
  for f in "${RESTORE_FILES[@]:-}"; do
    [ -n "$f" ] || continue
    if [ ! -f "$f" ] && [ -f "$f.next-env-bak" ]; then
      mv -f "$f.next-env-bak" "$f"
    fi
    rm -f "$f.next-env-bak"
  done
}
trap restore_if_absent EXIT

check_app() {
  local APP="$1"
  local FILE="$APP/next-env.d.ts"

  if [ ! -f "$FILE" ]; then
    echo "next-env gate: '$FILE' is missing from the working tree — it is a tracked file." >&2
    return 2
  fi

  # Present is not the same as TRACKED, and only tracked is meaningful here. `git diff` ignores
  # untracked paths, so after a `git rm --cached` typegen would recreate the file, the diff would
  # compare nothing, and this gate would report a clean pass forever. (CodeRabbit, SMA-519)
  if ! git ls-files --error-unmatch -- "$FILE" >/dev/null 2>&1; then
    echo "next-env gate: '$FILE' exists but is NOT tracked by git." >&2
    echo "  This gate compares generated output against the committed copy; with nothing" >&2
    echo "  committed that comparison is vacuous and would pass unconditionally." >&2
    return 2
  fi

  # Back up BEFORE removing; the EXIT trap restores from this copy, not from the index.
  if ! cp "$FILE" "$FILE.next-env-bak"; then
    echo "next-env gate: could not back up $FILE before regenerating it." >&2
    return 2
  fi
  rm -f "$FILE"
  RESTORE_FILES+=("$FILE")

  # `next typegen` regenerates route/page/layout types without a full production build
  # (~1.5s vs ~5s). It writes into "$APP"/.next/, which is why moon.yml orders this task after
  # iam-console-ts:build rather than letting the two race on that directory.
  if ! pnpm --dir "$APP" exec next typegen >/dev/null 2>&1; then
    echo "next-env gate: 'next typegen' failed in $APP." >&2
    pnpm --dir "$APP" exec next typegen >&2 || true
    return 2
  fi

  # Control: typegen must actually have produced the file. Without this the gate would go quietly
  # vacuous the day Next changes how this file is emitted.
  if [ ! -f "$FILE" ]; then
    echo "next-env gate: 'next typegen' completed but did not emit $FILE." >&2
    echo "  Next no longer generates this file the same way, so this gate is guarding nothing." >&2
    return 2
  fi

  if ! git diff --exit-code -- "$FILE"; then
    echo "" >&2
    echo "next-env gate: the committed $FILE does not match what Next generates." >&2
    echo "  The regenerated file has been left in your working tree — commit it." >&2
    return 1
  fi

  echo "next-env gate: $FILE matches 'next typegen' output."
  return 0
}

# SMA-512: this gate checked ONE hardcoded app until a second console zone landed, and it would
# have skipped the new one in silence. Discovery plus the subset assertion below is what makes a
# future third zone impossible to miss. It is a SUBSET assertion, not set-equality: every
# `ts/apps/*` directory that has a `package.json` must be in the discovered set.
#
# SMA-637: the --negative-control mode below proves that this gate can still go red.
discover_apps() {
  shopt -s nullglob
  apps=()
  # EXACT filenames, never a trailing wildcard. `next.config.[tjmc][sj]*` matched
  # `next.config.ts.bak` (the `*` swallows any suffix) and MISSED `next.config.mts` (`t` is not in
  # `[sj]`) — both measured. A stray backup file would have added a phantom entry, and an `.mts` app
  # would have been invisible here.
  #
  # An app directory is only a workspace member if it has a `package.json` — the same test the
  # liveness assertion below applies, and the one pnpm's own `apps/*` glob uses. Without this filter
  # a config-bearing directory that is NOT a workspace member reaches check_app, which then runs
  # `pnpm --dir` against a non-member and fails confusingly.
  for cfg in ts/apps/*/next.config.{js,mjs,cjs,ts,mts,cts}; do
    candidate="$(dirname "$cfg")"
    [ -f "$candidate/package.json" ] && apps+=("$candidate")
  done
  shopt -u nullglob

  if [ "${#apps[@]}" -eq 0 ]; then
    echo "next-env gate: no ts/apps/*/next.config.* found — this gate is guarding nothing." >&2
    exit 2
  fi

  # LIVENESS. A Next app directory with no discoverable config would otherwise be skipped without a
  # word, which is the exact defect this rewrite exists to remove. Only a directory with its own
  # package.json counts as a workspace member — the same test pnpm's own `apps/*` glob applies —
  # so a stale leftover directory (an old node_modules/.next from a rename, say) is not mistaken
  # for a missing app.
  dirs=()
  for d in ts/apps/*/; do
    [ -f "${d}package.json" ] && dirs+=("${d%/}")
  done
  missing=()
  # `"${dirs[@]:-}"` iterates ONCE with an empty `d` when `dirs` is empty, which printed a blank
  # row under the heading below. Unreachable today — `apps` needs a package.json AND a config, and
  # the exit above fires when `apps` is empty, so reaching here proves `dirs` is non-empty too —
  # but the `:-` idiom is wrong regardless, and a future reordering of the two blocks would make it
  # live. The length guard is the bash-3.2-safe form under `set -u`; the `apps` check above uses it
  # too.
  if [ "${#dirs[@]}" -gt 0 ]; then
    for d in "${dirs[@]}"; do
      found=0
      for a in "${apps[@]}"; do
        [ "$a" = "$d" ] && found=1
      done
      [ "$found" -eq 1 ] || missing+=("$d")
    done
  fi
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "next-env gate: these ts/apps/* directories have no discoverable next.config.*:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    echo "  Each would be skipped by this gate in silence. Add a config, or remove the directory." >&2
    exit 2
  fi
}

# Preserve the HIGHEST severity seen across apps: rc 2 (infrastructure failure — a missing or
# untracked file, or a broken typegen) outranks rc 1 (content drift), because an infrastructure
# failure must never be reported as if it were mere drift needing a commit.
real_run() {
  rc=0
  for APP in "${apps[@]}"; do
    ec=0
    check_app "$APP" || ec=$?
    if [ "$ec" -gt "$rc" ]; then
      rc="$ec"
    fi
  done
  exit "$rc"
}

# ---- The negative control (SMA-637) ------------------------------------------------------------
NEGCTL_TMP=""

# One EXIT handler in negctl mode: it removes the temporary files, then does what the check-mode
# handler does. It REPLACES `trap restore_if_absent EXIT`, so it must call restore_if_absent itself.
negctl_cleanup() {
  if [ -n "$NEGCTL_TMP" ]; then
    rm -rf "$NEGCTL_TMP"
  fi
  restore_if_absent
}

negctl_infra() {
  echo "next-env negative control: INFRA: $*" >&2
  exit 2
}

# The REAL index entries (mode, blob, stage, path) of every app's next-env.d.ts.
real_index_entries() {
  local a
  for a in "${apps[@]}"; do
    git ls-files -s -- "$a/next-env.d.ts" || negctl_infra "git ls-files -s failed for $a/next-env.d.ts"
  done
}

# The child has its own EXIT trap, which restores its backups. Prove that it did.
assert_no_backup() {
  local a
  for a in "${apps[@]}"; do
    if [ -e "$a/next-env.d.ts.next-env-bak" ]; then
      negctl_infra "[$1] the child left $a/next-env.d.ts.next-env-bak behind"
    fi
  done
}

negative_control() {
  local real_index idx_drift idx_untracked out_drift out_untracked
  local a f blob staged rc_drift rc_untracked n_marker fails fails_row

  # Refuse a caller's index (Sven, 2026-10-01): `git rev-parse --git-path index` would return
  # it, and the control would plant its rows in a copy of an index that is not the real one.
  if [ "${GIT_INDEX_FILE+set}" = set ]; then
    negctl_infra "GIT_INDEX_FILE is set ('${GIT_INDEX_FILE}'). The control copies the real index only. Unset GIT_INDEX_FILE and run again."
  fi

  NEGCTL_TMP="$(mktemp -d "${TMPDIR:-/tmp}/next-env-negctl.XXXXXX")" || negctl_infra "mktemp -d failed"
  trap negctl_cleanup EXIT
  # GIT_INDEX_FILE must be an absolute path: the child runs git from the top level, and a
  # relative TMPDIR would otherwise give a relative index path.
  NEGCTL_TMP="$(cd "$NEGCTL_TMP" && pwd -P)" || negctl_infra "cannot resolve the temporary directory"
  case "$NEGCTL_TMP" in
    /*) ;;
    *) negctl_infra "the temporary directory '$NEGCTL_TMP' is not an absolute path" ;;
  esac

  real_index="$(git rev-parse --git-path index)" || negctl_infra "git rev-parse --git-path index failed"
  [ -f "$real_index" ] || negctl_infra "the real index '$real_index' does not exist"
  real_index_entries > "$NEGCTL_TMP/index-before"
  fails=0

  # ---- N1 drift: the staged blob of every app gets one extra last line. ----
  idx_drift="$NEGCTL_TMP/index-drift"
  out_drift="$NEGCTL_TMP/out-drift"
  cp "$real_index" "$idx_drift" || negctl_infra "[N1] cannot copy the index to $idx_drift"
  for a in "${apps[@]}"; do
    f="$a/next-env.d.ts"
    git cat-file blob ":$f" > "$NEGCTL_TMP/planted" || negctl_infra "[N1] git cat-file blob :$f failed"
    printf '%s\n' '// SMA-637 negative control' >> "$NEGCTL_TMP/planted"
    blob="$(git hash-object -w --no-filters -- "$NEGCTL_TMP/planted")" || negctl_infra "[N1] git hash-object -w failed for $f"
    git cat-file -e "$blob" || negctl_infra "[N1] the planted blob $blob for $f is not in the object store"
    staged="$(git rev-parse ":$f")" || negctl_infra "[N1] git rev-parse :$f failed"
    [ "$blob" != "$staged" ] || negctl_infra "[N1] the planted blob for $f equals the index blob, so N1 would prove nothing"
    GIT_INDEX_FILE="$idx_drift" git update-index --cacheinfo "100644,$blob,$f" || negctl_infra "[N1] git update-index --cacheinfo failed for $f"
  done
  rc_drift=0
  GIT_INDEX_FILE="$idx_drift" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?
  assert_no_backup N1
  # An infrastructure failure must never be reported as drift (see real_run).
  if [ "$rc_drift" -eq 2 ]; then
    echo "INFRA [N1] child exited 2 (infrastructure), expected 1" >&2
    cat "$out_drift" >&2
    exit 2
  fi
  fails_row="$fails"
  if [ "$rc_drift" -ne 1 ]; then
    echo "FAIL [N1] expected rc 1, got $rc_drift" >&2
    fails=$((fails + 1))
  fi
  # The marker is the same for every app, so count it: one removed line per app.
  n_marker="$(grep -cF -- '-// SMA-637 negative control' "$out_drift" || true)"
  if [ "$n_marker" != "${#apps[@]}" ]; then
    echo "FAIL [N1] expected ${#apps[@]} diff lines '-// SMA-637 negative control', got $n_marker" >&2
    fails=$((fails + 1))
  fi
  for a in "${apps[@]}"; do
    if ! grep -qF -- "the committed $a/next-env.d.ts does not match what Next generates" "$out_drift"; then
      echo "FAIL [N1 $a] missing 'the committed $a/next-env.d.ts does not match what Next generates'" >&2
      fails=$((fails + 1))
    fi
  done
  if [ "$fails" -eq "$fails_row" ]; then
    echo "next-env negative control: PASS [N1] planted drift gave rc 1 and named every app"
  else
    cat "$out_drift" >&2
  fi

  # ---- N2 untracked: every app's file is removed from the index copy. ----
  idx_untracked="$NEGCTL_TMP/index-untracked"
  out_untracked="$NEGCTL_TMP/out-untracked"
  cp "$real_index" "$idx_untracked" || negctl_infra "[N2] cannot copy the index to $idx_untracked"
  for a in "${apps[@]}"; do
    f="$a/next-env.d.ts"
    GIT_INDEX_FILE="$idx_untracked" git update-index --force-remove -- "$f" || negctl_infra "[N2] git update-index --force-remove failed for $f"
    if GIT_INDEX_FILE="$idx_untracked" git ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
      negctl_infra "[N2] $f is still listed in the index copy"
    fi
  done
  rc_untracked=0
  GIT_INDEX_FILE="$idx_untracked" "$BASH" "$SELF" >"$out_untracked" 2>&1 || rc_untracked=$?
  assert_no_backup N2
  fails_row="$fails"
  if [ "$rc_untracked" -ne 2 ]; then
    echo "FAIL [N2] expected rc 2, got $rc_untracked" >&2
    fails=$((fails + 1))
  fi
  for a in "${apps[@]}"; do
    if ! grep -qF -- "'$a/next-env.d.ts' exists but is NOT tracked by git" "$out_untracked"; then
      echo "FAIL [N2 $a] missing ''$a/next-env.d.ts' exists but is NOT tracked by git'" >&2
      fails=$((fails + 1))
    fi
  done
  if [ "$fails" -eq "$fails_row" ]; then
    echo "next-env negative control: PASS [N2] an untracked file gave rc 2 and named every app"
  else
    cat "$out_untracked" >&2
  fi

  # Post-condition: the real index entries did not change.
  real_index_entries > "$NEGCTL_TMP/index-after"
  cmp -s "$NEGCTL_TMP/index-before" "$NEGCTL_TMP/index-after" || negctl_infra "the real index entries for next-env.d.ts changed during the control"

  if [ "$fails" -ne 0 ]; then
    echo "next-env-drift negative control: $fails check(s) failed" >&2
    exit 1
  fi
  echo "== next-env-drift negative control passed =="
  exit 0
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --negative-control) MODE=negctl; shift ;;
    *) echo "next-env gate: unknown flag: $1" >&2; exit 2 ;;
  esac
done

discover_apps
case "$MODE" in
  check)  real_run ;;
  negctl) negative_control ;;
  *) echo "next-env gate: unknown mode '$MODE'" >&2; exit 2 ;;
esac
```

Notes for the implementer:
- `real_index_entries > file` is NOT inside `$( )` on purpose. An `exit 2` inside a command substitution exits only the subshell (memory "errexit swallows nested exit codes"); with a redirect, `negctl_infra` exits the control itself.
- `git ls-files -s` output for each path is one line. Under `set -e`, a failing `git ls-files -s` goes to `negctl_infra`.
- Keep the old comment text in `discover_apps` as it is, except the one sentence that cited `lines 126 and 148` (now "the `apps` check above uses it too"), because those line numbers move.

- [ ] **Step 4: Check that every pinned line occurs exactly once**

Save as `$TMPDIR/sma637-pins.py`:
```python
import sys

PINS = [
    'SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"',
    '--negative-control) MODE=negctl; shift ;;',
    'check)  real_run ;;',
    'negctl) negative_control ;;',
    '*) echo "next-env gate: unknown mode \'$MODE\'" >&2; exit 2 ;;',
    'if [ "${GIT_INDEX_FILE+set}" = set ]; then',
    'GIT_INDEX_FILE="$idx_drift" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?',
    'if [ "$rc_drift" -eq 2 ]; then',
    'echo "INFRA [N1] child exited 2 (infrastructure), expected 1" >&2',
    'if [ "$rc_drift" -ne 1 ]; then',
    'n_marker="$(grep -cF -- \'-// SMA-637 negative control\' "$out_drift" || true)"',
    'if [ "$n_marker" != "${#apps[@]}" ]; then',
    'if ! grep -qF -- "the committed $a/next-env.d.ts does not match what Next generates" "$out_drift"; then',
    'GIT_INDEX_FILE="$idx_untracked" "$BASH" "$SELF" >"$out_untracked" 2>&1 || rc_untracked=$?',
    'if [ "$rc_untracked" -ne 2 ]; then',
    'if ! grep -qF -- "\'$a/next-env.d.ts\' exists but is NOT tracked by git" "$out_untracked"; then',
    'cmp -s "$NEGCTL_TMP/index-before" "$NEGCTL_TMP/index-after" || negctl_infra "the real index entries for next-env.d.ts changed during the control"',
    'if [ "$fails" -ne 0 ]; then',
    'if ! git diff --exit-code -- "$FILE"; then',
    'check_app "$APP" || ec=$?',
    'if [ "$ec" -gt "$rc" ]; then',
    'rc="$ec"',
    'exit "$rc"',
]
lines = [ln.strip() for ln in open(sys.argv[1], encoding="utf-8")]
bad = [(p, lines.count(p)) for p in PINS if lines.count(p) != 1]
for p, n in bad:
    print(f"count {n}: {p}")
print(f"{len(PINS)} pins, {len(bad)} bad")
sys.exit(1 if bad else 0)
```
Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && python3 "$TMPDIR/sma637-pins.py" ci/next-env/run.sh; echo "rc=$?"
```
Expected: `23 pins, 0 bad`, `rc=0`. Task 2 copies this exact list into `NEXT_ENV_SH_CALL_SITES`.

Also run the static bans:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && grep -nE 'mapfile|declare -A|<<<|<<[A-Za-z_'"'"'"-]|,,\}|\| *(grep -[a-zA-Z]*[qm]|head|sed -n .*q)' ci/next-env/run.sh; echo "matches rc=$? (1 means none)"; bash -n ci/next-env/run.sh && /bin/bash -n ci/next-env/run.sh && echo SYNTAX_OK
```
Expected: `matches rc=1 (1 means none)` and `SYNTAX_OK`.

- [ ] **Step 5: Run the control and the real run under bash 3.2 (AC 1, 3, 4)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && git ls-files -s -- 'ts/apps/*/next-env.d.ts' > "$TMPDIR/sma637-idx-before" && /bin/bash ci/next-env/run.sh --negative-control; echo "negctl rc=$?"; git ls-files -s -- 'ts/apps/*/next-env.d.ts' > "$TMPDIR/sma637-idx-after"; cmp "$TMPDIR/sma637-idx-before" "$TMPDIR/sma637-idx-after" && echo INDEX_SAME; git status --porcelain; git diff --cached --stat; ls ts/apps/*/next-env.d.ts.next-env-bak 2>/dev/null; /bin/bash ci/next-env/run.sh; echo "real rc=$?"
```
Expected:
- `next-env negative control: PASS [N1] …`, `next-env negative control: PASS [N2] …`, `== next-env-drift negative control passed ==`, `negctl rc=0`;
- `INDEX_SAME`;
- `git status --porcelain` lists only `M ci/next-env/run.sh` (the uncommitted Task 1 edit) and nothing under `ts/`;
- `git diff --cached --stat` prints nothing; no `.next-env-bak` file;
- the real run prints the two `matches` lines and `real rc=0`.

- [ ] **Step 6: The same under Homebrew bash 5 (AC 8)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /opt/homebrew/bin/bash --version | head -n1 && /opt/homebrew/bin/bash ci/next-env/run.sh --negative-control; echo "negctl rc=$?"; /opt/homebrew/bin/bash ci/next-env/run.sh; echo "real rc=$?"; git status --porcelain
```
Note: `| head -n1` here reads `--version` output, which is a short, complete write; it is not in a gate script. Expected: `negctl rc=0`, `real rc=0`, and only `M ci/next-env/run.sh` in `git status`. If the command does not finish in 3 minutes, check the host pipe state (SMA-612) and record it; the script itself has no here-string.

- [ ] **Step 7: Review Focus rows RF1 to RF5**

Run each row and compare with the expected result:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && W="$PWD" && \
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=color.diff GIT_CONFIG_VALUE_0=always /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/rf1.out" 2>&1; echo "RF1 rc=$?"; \
( cd ts && /bin/bash ../ci/next-env/run.sh --negative-control ) > "$TMPDIR/rf2.out" 2>&1; echo "RF2 rc=$?"; \
GIT_INDEX_FILE= /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/rf3a.out" 2>&1; echo "RF3a rc=$?"; \
GIT_INDEX_FILE="$TMPDIR/some-index" /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/rf3b.out" 2>&1; echo "RF3b rc=$?"; \
mkdir -p "$TMPDIR/sma637 space" && TMPDIR="$TMPDIR/sma637 space" /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/rf4a.out" 2>&1; echo "RF4a rc=$?"; \
mkdir -p "$W/.sma637-reltmp" && TMPDIR=.sma637-reltmp /bin/bash ci/next-env/run.sh --negative-control > "$TMPDIR/rf4b.out" 2>&1; echo "RF4b rc=$?"; rm -rf "$W/.sma637-reltmp"; \
/bin/bash ci/next-env/run.sh --self-test > "$TMPDIR/rf5.out" 2>&1; echo "RF5 rc=$?"; cat "$TMPDIR/rf5.out"; \
grep -F INFRA "$TMPDIR/rf3a.out" "$TMPDIR/rf3b.out"; git status --porcelain; ls ts/apps/*/next-env.d.ts.next-env-bak 2>/dev/null
```
Expected:
- `RF1 rc=0` (colour does not break the marker count);
- `RF2 rc=0`;
- `RF3a rc=2` and `RF3b rc=2`, each with `next-env negative control: INFRA: GIT_INDEX_FILE is set`;
- `RF4a rc=0`; `RF4b rc=0` (the relative `TMPDIR` resolves against the top level, which is the cwd of the script after its `cd`). Use the dedicated directory `.sma637-reltmp` and remove it with `rm -rf`, never `TMPDIR=.`: MEASURED while this plan was written, `next typegen` (node) also reads `TMPDIR` and writes a `node-compile-cache/` directory into it, so `TMPDIR=.` leaves an untracked `node-compile-cache/` at the top level;
- `RF5 rc=2` and `next-env gate: unknown flag: --self-test`, with no `matches` line (no typegen ran);
- `git status --porcelain` shows only `M ci/next-env/run.sh`; no backup file.

If a row differs, fix `run.sh` and run Steps 4 to 7 again.

- [ ] **Step 8: Behaviour mutations (AC 2), one at a time**

For each row: apply the edit with the Edit tool, run the command, compare, then revert the exact edit with the Edit tool and confirm with `git diff ci/next-env/run.sh` that only the Task 1 change is present (compare with `git diff --stat`: the same line counts as before the mutation). Never commit a mutation.

| ID | Edit in `ci/next-env/run.sh` | Command | Predicted |
|---|---|---|---|
| M2a | in `check_app`, `if ! git diff --exit-code -- "$FILE"; then` → `if ! git diff -- "$FILE"; then` | `/bin/bash ci/next-env/run.sh --negative-control` | rc 1, `FAIL [N1] expected rc 1, got 0` |
| M2b | delete the `if ! git ls-files --error-unmatch …` block (the `if` line to its `fi`, 6 lines) | same | rc 1, `FAIL [N2] expected rc 2, got 0` |
| M2c | in `real_run`, `check_app "$APP" \|\| ec=$?` → `check_app "$APP" \|\| true` | same | rc 1, `FAIL [N1] expected rc 1, got 0` and `FAIL [N2] expected rc 2, got 0` |
| M2d | in `real_run`, delete the line `exit "$rc"` | same | rc 1, both rows `got 0` |
| M2e | in `check_app`, `if ! pnpm --dir "$APP" exec next typegen >/dev/null 2>&1; then` → `if ! pnpm --dir "$APP/nonexistent" exec next typegen >/dev/null 2>&1; then` | same | rc 2, `INFRA [N1] child exited 2 (infrastructure), expected 1` |

M2e also runs a `pnpm --dir "$APP"` diagnostic call (the line below it). That is fine. After M2e, also run `git status --porcelain` and `ls ts/apps/*/next-env.d.ts*`: both `next-env.d.ts` files must be present (the child's trap restored them) and no backup file is left.

Record each row (command, predicted rc, actual rc, the line that fired) for the PR body under "Mutation proof". If the permission system refuses a mutation, restore the file, write the exact manual steps under "Mutation proof pending" and continue.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && git branch --show-current && git diff --stat && git add ci/next-env/run.sh && git commit -m "feat(ci): add a negative control to the next-env drift gate (SMA-637)

The control copies the real git index, plants a change for every app in
the copy, and runs the whole gate as a child process with GIT_INDEX_FILE
set to the copy. Row N1 (planted drift) must exit 1, and row N2 (file
removed from the copy) must exit 2. The real index does not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Expected: the branch line prints `feature/sma-637-next-env-negative-control`, and the commit succeeds. If signing fails with "failed to fill whole buffer" or "communication with agent failed", 1Password is locked: stop and report a blocker.

---

### Task 2: The moon task and the registrations

All edits of this task go into ONE commit. `check_self_scheduled_coverage` reds `repo:affected-smoke` as soon as `moon.yml` names `--negative-control` without a `SELF_SCHEDULED_GATES` entry, so the task script and the registry must land together.

**Files:**
- Modify: `moon.yml:133-171` (the `next-env-drift` task) and `moon.yml:257-263` (after the `ci/moon-diagnosis/**/*` input of `affected-smoke`)
- Modify: `ci/affected-graph/ci_targets.py` (see each step for the place)
- Modify: `ci/actionlint/run.sh:2164-2167` (after `'ci/moon-diagnosis/**/*'`)
- Test: `python3 ci/affected-graph/ci_targets.py --self-test`, `python3 ci/affected-graph/ci_targets.py`, `/bin/bash ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: the 23 pinned lines of Task 1 (exactly the `PINS` list of Task 1 Step 4).
- Produces: `NEXT_ENV_SH_CALL_SITES` (tuple of str), the parameter `next_env_sh_text` of `check_self_invocation` (the 11th and last positional parameter), the self-test fixture `wired_next_env`, the `sh` dict key `"next_env"`, and the `_CALL_SITE_SOURCE_KEYS` entry `"next_env"`.

- [ ] **Step 1: Write the moon task**

In `moon.yml`, in the `next-env-drift` task, replace the line `    script: 'ci/next-env/run.sh'` with:
```yaml
    # SMA-637 — the negative control runs FIRST and in the SAME block. `set -euo pipefail` is
    # REQUIRED: Moon does not enable errexit for `script:` blocks and takes the block's status
    # from its LAST command, so without it a failing control is masked by the passing real run.
    # These three lines are pinned by SELF_SCHEDULED_GATES. The control runs the whole gate as a
    # child process against a temporary copy of the git index (see ci/next-env/run.sh).
    script: |
      set -euo pipefail
      bash ci/next-env/run.sh --negative-control
      bash ci/next-env/run.sh
```
Keep `description`, the comment above `script:`, `toolchain`, `deps` and `inputs` without change.

In the `affected-smoke` task's `inputs`, directly after the line `      - 'ci/moon-diagnosis/**/*'`, add:
```yaml
      # SMA-637 — this task pins load-bearing lines inside ci/next-env/run.sh
      # (NEXT_ENV_SH_CALL_SITES), so a change under ci/next-env/ MUST re-key it. The broad
      # 'ci/**/*' entry already covers it for SCHEDULING; the narrow entry is kept because check
      # 8e in ci/actionlint/run.sh floors this array's length and the file's own policy is to
      # keep narrow globs rather than collapse them into the broad one.
      - 'ci/next-env/**/*'
```

- [ ] **Step 2: Run the guard to see it go red (red first)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/ci_targets.py > "$TMPDIR/sma637-ct-red.out" 2>&1; echo "rc=$?"; grep -n 'next-env' "$TMPDIR/sma637-ct-red.out"
```
Expected: `rc=1`, with a `selfsched-unregistered` row that names `next-env-drift` (the task script has `--negative-control` and no `SELF_SCHEDULED_GATES` key).

- [ ] **Step 3: `REQUIRED_REPO_TASKS`**

In `ci/affected-graph/ci_targets.py`, after the `"moon-diagnosis-exec",` entry of `REQUIRED_REPO_TASKS` (line 217), add:
```python
    # SMA-637. Same reasoning as the entries above: repo:next-env-drift now carries a
    # --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is
    # dropped from `T` and made CI-ineligible in the same edit — so without a floor entry the whole
    # gate, control included, could be switched off with every check green.
    "next-env-drift",
```

- [ ] **Step 4: `SELF_TASK_EXPECTED_GLOBS`**

After the `"moon-diagnosis-exec": (…),` entry of `SELF_TASK_EXPECTED_GLOBS` (ends at line 420), add:
```python
    # SMA-637. Six globs then one literal, in check_gate_inputs' comparison order (globs sorted,
    # then files sorted). MEASURED with moon 2.5.3's `moon query projects` on 2026-10-01.
    # ts/pnpm-lock.yaml is the entry SMA-519 exists for: the Next version drives this file's
    # content, so dropping it serves a cached PASS on a Next upgrade. The day an app with
    # next.config.mjs (or another extension) lands, moon.yml must add its glob, and this entry
    # must change in the same commit.
    "next-env-drift": (
        "ci/next-env/**/*",
        "ts/apps/*/app/**/*",
        "ts/apps/*/next-env.d.ts",
        "ts/apps/*/next.config.ts",
        "ts/apps/*/package.json",
        "ts/apps/*/tsconfig.json",
        "ts/pnpm-lock.yaml",
    ),
```

- [ ] **Step 5: `SELF_SCHEDULED_GATES`**

After the `"moon-diagnosis-exec": (…),` entry of `SELF_SCHEDULED_GATES` (ends at line 697), add:
```python
    # SMA-637. Three lines: a --negative-control and no --self-test (spec §8 leaves the self-test
    # out). `set -euo pipefail` is what makes a failing control propagate, since Moon takes a
    # `script:` block's status from its LAST command. Whole-line matched:
    # `bash ci/next-env/run.sh` is a strict PREFIX of the flagged line.
    "next-env-drift": (
        "set -euo pipefail",
        "bash ci/next-env/run.sh --negative-control",
        "bash ci/next-env/run.sh",
    ),
```

- [ ] **Step 6: `NEXT_ENV_SH_CALL_SITES`**

After the closing `)` of `MOON_DIAGNOSIS_SH_CALL_SITES` (line 1551) and before the two blank lines that precede `def read_input`, add:
```python

# SMA-637 — ci/next-env/run.sh's load-bearing lines. REACHABILITY IS NOT AUTOMATIC: this check
# only runs when repo:affected-smoke is scheduled, so moon.yml lists `ci/next-env/**/*` among its
# inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry.
#
# Matched as stripped WHOLE LINES, like the helm-render and moon-diagnosis haystacks and for both
# of their reasons: the real lines are indented inside functions and `case` arms, and a
# COMMENTED-OUT copy must not satisfy a pin. Every entry occurs EXACTLY ONCE in run.sh.
#
# The pins hold the flag parse and the dispatch arms, the GIT_INDEX_FILE refusal, both child
# runs with their rc guards, the N1 infrastructure arm, the literal checks, the real-index
# post-condition and the failure guard of the control, and the real run's drift assertion,
# aggregation and exit. The pins hold text; the control is what proves behaviour.
NEXT_ENV_SH_CALL_SITES = (
    # The flag parse, the self path and the dispatch arms.
    'SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"',
    '--negative-control) MODE=negctl; shift ;;',
    'check)  real_run ;;',
    'negctl) negative_control ;;',
    '*) echo "next-env gate: unknown mode \'$MODE\'" >&2; exit 2 ;;',
    # The control: the refusal of a caller's index (Sven, 2026-10-01).
    'if [ "${GIT_INDEX_FILE+set}" = set ]; then',
    # N1: the child run, the infrastructure arm, the rc guard and the literal checks.
    'GIT_INDEX_FILE="$idx_drift" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?',
    'if [ "$rc_drift" -eq 2 ]; then',
    'echo "INFRA [N1] child exited 2 (infrastructure), expected 1" >&2',
    'if [ "$rc_drift" -ne 1 ]; then',
    'n_marker="$(grep -cF -- \'-// SMA-637 negative control\' "$out_drift" || true)"',
    'if [ "$n_marker" != "${#apps[@]}" ]; then',
    'if ! grep -qF -- "the committed $a/next-env.d.ts does not match what Next generates" "$out_drift"; then',
    # N2: the child run, the rc guard and the literal check.
    'GIT_INDEX_FILE="$idx_untracked" "$BASH" "$SELF" >"$out_untracked" 2>&1 || rc_untracked=$?',
    'if [ "$rc_untracked" -ne 2 ]; then',
    'if ! grep -qF -- "\'$a/next-env.d.ts\' exists but is NOT tracked by git" "$out_untracked"; then',
    # The real-index post-condition and the failure guard.
    'cmp -s "$NEGCTL_TMP/index-before" "$NEGCTL_TMP/index-after" || negctl_infra "the real index entries for next-env.d.ts changed during the control"',
    'if [ "$fails" -ne 0 ]; then',
    # The real run: the drift assertion in check_app, and real_run's aggregation and exit.
    'if ! git diff --exit-code -- "$FILE"; then',
    'check_app "$APP" || ec=$?',
    'if [ "$ec" -gt "$rc" ]; then',
    'rc="$ec"',
    'exit "$rc"',
)
```

- [ ] **Step 7: Wire `check_self_invocation`**

1. Signature (line 1924): replace
   `    next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text,`
   with
   `    next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text, next_env_sh_text,`
2. Docstring: replace `release-plan, ruff, next-public-free, helm-render and moon-diagnosis-exec gates missing from` with `release-plan, ruff, next-public-free, helm-render, moon-diagnosis-exec and next-env-drift gates missing from`; replace `    Ten haystacks, matched THREE different ways.` with `    Eleven haystacks, matched THREE different ways.`; replace `    The ten texts are checked SEPARATELY` with `    The eleven texts are checked SEPARATELY`; replace `` `release_plan_sh_text`, `ruff_sh_text`, `next_public_free_sh_text`, `helm_render_sh_text` and `moon_diagnosis_sh_text` are REQUIRED`` with `` `release_plan_sh_text`, `ruff_sh_text`, `next_public_free_sh_text`, `helm_render_sh_text`, `moon_diagnosis_sh_text` and `next_env_sh_text` are REQUIRED``.
3. Body: after the moon-diagnosis block (lines 2040-2046, ends with `if site not in moon_diagnosis_lines` and `)`) and before `    return missing`, add:
```python
    # SMA-637 — stripped whole lines, like the helm-render haystack and for the same two reasons.
    next_env_lines = {line.strip() for line in next_env_sh_text.splitlines()}
    missing.extend(
        f"ci/next-env/run.sh: {site}"
        for site in NEXT_ENV_SH_CALL_SITES
        if site not in next_env_lines
    )
```

- [ ] **Step 8: Add the new argument to all 67 calls (mechanical)**

Do not edit 67 calls by hand. Save as `$TMPDIR/sma637-addarg.py`:
```python
"""Append the next-env haystack as the 11th argument of every check_self_invocation call."""
import ast
import sys

PATH = "ci/affected-graph/ci_targets.py"
src = open(PATH, "rb").read()
tree = ast.parse(src)
line_starts = [0]
for line in src.splitlines(keepends=True):
    line_starts.append(line_starts[-1] + len(line))
inserts = []
for node in ast.walk(tree):
    if isinstance(node, ast.Call) and getattr(node.func, "id", None) == "check_self_invocation":
        if len(node.args) != 10 or node.keywords:
            sys.exit(f"line {node.lineno}: expected 10 positional args, got {len(node.args)}")
        last = node.args[9]
        # end_col_offset is a UTF-8 BYTE offset, so work on bytes (the file has em dashes).
        end = line_starts[last.end_lineno - 1] + last.end_col_offset
        text = src[line_starts[last.lineno - 1] + last.col_offset:end].decode()
        arg = b', sh["next_env"]' if text == 'sh["moon_diagnosis"]' else b", wired_next_env"
        inserts.append((end, arg))
if len(inserts) != 67:
    sys.exit(f"expected 67 calls, found {len(inserts)}")
for end, arg in sorted(inserts, reverse=True):
    src = src[:end] + arg + src[end:]
open(PATH, "wb").write(src)
print(f"patched {len(inserts)} calls")
```
Run it BEFORE Step 9 adds the new battery (whose calls already have 11 arguments):
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && python3 "$TMPDIR/sma637-addarg.py" && grep -c 'wired_next_env' ci/affected-graph/ci_targets.py && grep -n 'sh\["next_env"\]' ci/affected-graph/ci_targets.py
```
Expected: `patched 67 calls`, a count of `66`, and one `sh["next_env"]` line in `collect_findings`.

- [ ] **Step 9: The self-test fixture, the floor fixture and the battery**

1. Directly after the `wired_moon_diagnosis = "".join(…)` block (lines 2888-2891), add:
```python
    # SMA-637 — the same shape for ci/next-env/run.sh, derived from the registry.
    wired_next_env = "".join(
        f"    {site}\n" for site in NEXT_ENV_SH_CALL_SITES
    )
```
2. In `tasks_fixture` (line 2420), replace
```python
                 # SMA-714 — a floor member too, for the same reason.
                 "moon-diagnosis-exec": True},
```
with
```python
                 # SMA-714 — a floor member too, for the same reason.
                 "moon-diagnosis-exec": True,
                 # SMA-637 — a floor member too, for the same reason.
                 "next-env-drift": True},
```
and in `aligned_t` (lines 2423-2426) replace `                 "moon-diagnosis-exec"]` with `                 "moon-diagnosis-exec", "next-env-drift"]`.
3. In the `_param_name` loop (lines 3213-3217), replace `        "helm_render_sh_text", "moon_diagnosis_sh_text",` with `        "helm_render_sh_text", "moon_diagnosis_sh_text", "next_env_sh_text",`.
4. After the last row of the moon-diagnosis battery (the `_md_guard_neutered` block, which ends with the `"satisfied the pin"` failure text and `)`), add:
```python

    # SMA-637 — the moon-diagnosis battery above, repeated for the next-env haystack: a deletion
    # row per pinned line, a contamination row, a commented-out row and a disabled-guard row.
    for _ne_site in NEXT_ENV_SH_CALL_SITES:
        _ne_broken = "".join(
            line for line in wired_next_env.splitlines(keepends=True)
            if line.strip() != _ne_site
        )
        if not check_self_invocation(
            wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
            wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
            wired_moon_diagnosis, _ne_broken,
        ):
            failures.append(
                f"check_self_invocation: missed {_ne_site!r} deleted from ci/next-env/run.sh"
            )
    # Contamination: a next-env site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_next_env, scripts, wired_actionlint, wired_release_parity,
        wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free,
        wired_helm_render, wired_moon_diagnosis + wired_next_env,
        "".join(line for line in wired_next_env.splitlines(keepends=True)
                if line.strip() != NEXT_ENV_SH_CALL_SITES[0]),
    ):
        failures.append(
            "check_self_invocation: a next-env site was satisfied by another haystack's text"
        )
    # Whole-LINE, not substring: a commented-out copy of a pinned line must report missing.
    _ne_commented = wired_next_env.replace(
        "negctl) negative_control ;;\n", "# negctl) negative_control ;;\n"
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
        wired_moon_diagnosis, _ne_commented,
    ):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT next-env line satisfied the pin "
            "(widened to substring matching)"
        )
    # The disabled guard. N1's rc comparison is what makes the drift row prove anything;
    # comparing rc_drift with ITSELF leaves every other pinned line byte-identical, so the control
    # would pass whatever rc the child gave.
    _ne_guard_neutered = wired_next_env.replace(
        'if [ "$rc_drift" -ne 1 ]; then\n', 'if [ "$rc_drift" -ne "$rc_drift" ]; then\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
        wired_moon_diagnosis, _ne_guard_neutered,
    ):
        failures.append(
            "check_self_invocation: a NEUTERED next-env comparison (always-false guard) "
            "satisfied the pin"
        )
```
The contamination row puts the full next-env text into two other haystacks (the run.sh text and the moon-diagnosis text) while the next-env haystack itself misses its first line. A check that read the haystacks as one would pass; the real check must report the missing line.

- [ ] **Step 10: `main()`, `_CALL_SITE_SOURCE_KEYS` and the report text**

1. `_CALL_SITE_SOURCE_KEYS` (lines 4019-4024): replace the comment line `# The nine shell sources check_self_invocation reads, keyed so collect_findings' signature does` with `# The ten shell sources check_self_invocation reads, keyed so collect_findings' signature does`, and replace `    "next_public_free", "helm_render", "moon_diagnosis",` with `    "next_public_free", "helm_render", "moon_diagnosis", "next_env",`. (Step 8 already added `sh["next_env"]` to the `collect_findings` call.)
2. In `main()`, after the `moon_diagnosis_sh = read_input(…)` call (lines 4294-4296), add:
```python
        next_env_sh = read_input(
            root / "ci" / "next-env" / "run.sh", "ci/next-env/run.sh"
        )
```
3. In the `sh` dict of the `collect_findings` call in `main()`, after `                "moon_diagnosis": moon_diagnosis_sh,`, add `                "next_env": next_env_sh,`.
4. In the `call-sites` report text (around line 4115), replace
`         "    RUFF_SH_CALL_SITES, HELM_RENDER_SH_CALL_SITES and MOON_DIAGNOSIS_SH_CALL_SITES in\n"`
with
`         "    RUFF_SH_CALL_SITES, HELM_RENDER_SH_CALL_SITES, MOON_DIAGNOSIS_SH_CALL_SITES and\n"`
`         "    NEXT_ENV_SH_CALL_SITES in\n"`
(two lines). Then, directly after the paragraph that starts `"    A row prefixed `ci/workflow-credentials/run.sh:` means the same for that gate's\n"` and ends `"    failure guard, or the report line.\n"`, add:
```python
         "    A row prefixed `ci/next-env/run.sh:` means one of the pinned lines of that gate is\n"
         "    gone: the flag parse or a dispatch arm, a line of the negative control (a child\n"
         "    run, an rc guard, a literal check, the GIT_INDEX_FILE refusal, the real-index\n"
         "    check or the failure guard), or a line of the real run (the git diff assertion,\n"
         "    the aggregation or the final exit). The control can then pass while it proves\n"
         "    nothing, or the real run can exit 0 on drift.\n"
```

- [ ] **Step 11: Correct the two stale citations**

Replace (line ~2215, in the `check_tailwind_guard_invocations` docstring)
`    \`ci/next-env/run.sh:113\` applies (finding C: a directory need not have a \`moon.yml\` to reach`
with
`    \`discover_apps\` in \`ci/next-env/run.sh\` applies (finding C: a directory need not have a \`moon.yml\` to reach`
and replace (line ~4299, in `main()`)
`        # whole branch exists to remove. \`package.json\` is the test ci/next-env/run.sh:113`
with
`        # whole branch exists to remove. \`package.json\` is the test discover_apps in ci/next-env/run.sh`
(the backslashes above only escape the backticks in this plan; the file has plain backticks). Then:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && grep -n 'next-env/run.sh:[0-9]' ci/affected-graph/ci_targets.py; echo "stale rc=$? (1 means none)"
```
Expected: `stale rc=1 (1 means none)` (AC 9).

- [ ] **Step 12: `T_AFFECTED_SMOKE_REQUIRED_INPUTS`**

In `ci/actionlint/run.sh`, directly after the line `  'ci/moon-diagnosis/**/*'` (line 2167), add:
```bash
  # SMA-637 — floors the input that makes NEXT_ENV_SH_CALL_SITES reachable. Without it, a PR
  # editing ci/next-env/** does not schedule repo:affected-smoke, and neither the
  # SELF_SCHEDULED_GATES nor the SELF_TASK_EXPECTED_GLOBS pin for that gate can fire.
  'ci/next-env/**/*'
```
Do not change the check 8e floor line (`-ge 20`); `ACTIONLINT_SH_CALL_SITES` pins it, and the array grows from 27 to 28 entries.

- [ ] **Step 13: Run the self-test and the guard (green)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/ci_targets.py --self-test; echo "self-test rc=$?"; python3 ci/affected-graph/ci_targets.py; echo "guard rc=$?"
```
Expected: `self-test rc=0` and `guard rc=0` (the guard prints `PASS  ci-targets …`). A `TypeError: check_self_invocation() missing 1 required positional argument` means Step 8 missed a call; fix it and run again.

- [ ] **Step 14: Run the affected-graph suite and the input-liveness gate**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/affected-graph/run.sh > "$TMPDIR/sma637-ag.out" 2>&1; echo "affected-smoke rc=$?"; tail -n 5 "$TMPDIR/sma637-ag.out"; python3 ci/affected-graph/task_inputs.py; echo "input-liveness rc=$?"
```
Expected: `affected-smoke rc=0` and `input-liveness rc=0`. `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2 (root `CLAUDE.md`). If it hangs, the host is in the small-pipe state with the wrong bash (memory "affected-smoke hang: shim bash"); record it.

- [ ] **Step 15: Run the moon task (AC 5)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run repo:next-env-drift --force; echo "rc=$?"; cat .moon/cache/states/repo/next-env-drift/stdout.log; git status --porcelain
```
Expected: `rc=0`; `stdout.log` holds the two `PASS [N…]` lines, `== next-env-drift negative control passed ==` and the two `matches` lines; `git status` shows only the uncommitted Task 2 files. The task builds both apps first (its `deps`), so it can take several minutes.

- [ ] **Step 16: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && git branch --show-current && git add moon.yml ci/affected-graph/ci_targets.py ci/actionlint/run.sh && git diff --cached --stat && git commit -m "feat(ci): register the next-env negative control in the gate registries (SMA-637)

The next-env-drift task now runs the negative control before the real
run. The commit adds the SELF_SCHEDULED_GATES, SELF_TASK_EXPECTED_GLOBS
and REQUIRED_REPO_TASKS entries, the NEXT_ENV_SH_CALL_SITES pin table
with its self-test battery, and the affected-smoke input with its
actionlint floor. Two stale run.sh line citations now name discover_apps.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Expected: three files staged; the commit succeeds.

---

### Task 3: The documentation

**Files:**
- Modify: `ts/CLAUDE.md:76-79`

**Interfaces:**
- Consumes: the behaviour of Task 1.
- Produces: nothing for other tasks.

- [ ] **Step 1: Replace the false sentence (AC 10)**

In `ts/CLAUDE.md`, replace
```
  the discovered set, and `ts/eslint.config.js` derives one block per app directory. The next-env
  gate still has **no negative control**. Its `deps` names one build per app by hand and nothing
```
with
```
  the discovered set, and `ts/eslint.config.js` derives one block per app directory. The next-env
  gate has a negative control since SMA-637: it plants a change in a temporary copy of the git
  index and runs the whole gate as a child process, which must exit 1 (drift) and 2 (untracked).
  Its `deps` names one build per app by hand and nothing
```
Keep the rest of the bullet (`asserts the list is complete, …`) as it is.

- [ ] **Step 2: Check the text**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && grep -n 'no negative control' ts/CLAUDE.md; echo "rc=$? (1 means none)"; sed -n 72,82p ts/CLAUDE.md
```
Expected: `rc=1 (1 means none)`, and the bullet reads as one paragraph.

- [ ] **Step 3: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && git branch --show-current && git add ts/CLAUDE.md && git commit -m "docs(ts): record the next-env negative control (SMA-637)

ts/CLAUDE.md said that the next-env gate has no negative control. That
is no longer true, so the sentence now names the control and SMA-637.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Verification: registry mutations, the gate graph, the critical path

No new file. Record every result (command, predicted rc, actual rc, the row that fired) for the PR body under "Mutation proof". A mutation is never committed.

**Files:**
- Test only. Temporary edits to `moon.yml`, `ci/affected-graph/ci_targets.py`, `ci/next-env/run.sh`, `.github/workflows/ci.yml` and `CLAUDE.md`, each restored with the Edit tool.

**Interfaces:**
- Consumes: everything of Tasks 1-3.
- Produces: the "Mutation proof" record and the critical-path time for the PR body.

- [ ] **Step 1: Registry mutations (AC 7), one at a time**

For each row: apply the edit with the Edit tool, run `python3 ci/affected-graph/ci_targets.py` (with the PATH prefix; it calls `moon query`), compare, revert the exact edit with the Edit tool, and confirm `git status --porcelain` is empty. Rows (a) to (d) and (g) change `moon.yml`, so `moon query` sees them.

| ID | Edit | Predicted |
|---|---|---|
| 7a | `moon.yml`: delete the line `      bash ci/next-env/run.sh --negative-control` | rc 1, `call-sites` row for `repo:next-env-drift` naming the missing line |
| 7b | `moon.yml`: delete the line `      set -euo pipefail` of the `next-env-drift` script | rc 1, `call-sites` row naming `set -euo pipefail` |
| 7c | `ci_targets.py`: delete the `"next-env-drift": (…),` entry of `SELF_SCHEDULED_GATES` (with its comment) | rc 1, `selfsched-unregistered` row `next-env-drift` (and a `pairing-orphan-globs` row) |
| 7d | `moon.yml`: delete `      - 'ts/pnpm-lock.yaml'` from the `next-env-drift` inputs | rc 1, `gate-inputs` row for `next-env-drift` |
| 7e | `ci/next-env/run.sh`: delete each line of `NEXT_ENV_SH_CALL_SITES` in turn (23 runs) | rc 1 each, `call-sites` row `ci/next-env/run.sh: <that line>` |
| 7f | `ci_targets.py`: delete the `"next-env-drift": (…),` entry of `SELF_TASK_EXPECTED_GLOBS` (with its comment) | rc 1, `pairing-unpinned` row `next-env-drift` |
| 7g | delete `:next-env-drift ` from `T=(…)` in `.github/workflows/ci.yml` AND from the `ci-targets` block in `CLAUDE.md` (keep the markers), add `    options:` / `      runInCI: false` to the `next-env-drift` task in `moon.yml`, AND delete the four `- 'repo:next-env-drift'` `deps` lines in `ts/apps/{iam,gateway}-console/moon.yml` (`test` and `test-e2e`) | rc 1, `floor` row `next-env-drift` |

CORRECTED during Task 4 (2026-10-01): without the four `deps` deletions, 7g gives rc 2, not rc 1. Moon 2.5.3 refuses the graph (`task_builder::dependency::run_in_ci_mismatch`: `gateway-console-ts:test` cannot depend on a task with `runInCI` disabled), so `moon query tasks` fails and `ci_targets.py` exits `FATAL` before any check runs. With the deletions, 7g gives rc 1 and only the `floor` row.

For 7e, the 23 deletions can run from a script against scratch copies, so that no repo file changes: copy `ci/next-env/run.sh` to `$TMPDIR/sma637-7e/run.sh`, delete one pinned line in the copy, and call `check_self_invocation` directly. Save as `$TMPDIR/sma637-7e.py`:
```python
import sys
sys.path.insert(0, "ci/affected-graph")
import ci_targets as ct  # noqa: E402

real = open("ci/next-env/run.sh", encoding="utf-8").read()
others = {k: "" for k in ("run", "actionlint", "release_parity", "workflow_credentials",
                          "release_plan", "ruff", "next_public_free", "helm_render",
                          "moon_diagnosis")}
bad = 0
for site in ct.NEXT_ENV_SH_CALL_SITES:
    mutated = "".join(ln for ln in real.splitlines(keepends=True) if ln.strip() != site)
    if mutated == real:
        print(f"NOT FOUND in run.sh: {site}")
        bad += 1
        continue
    rows = ct.check_self_invocation(
        others["run"], {}, others["actionlint"], others["release_parity"],
        others["workflow_credentials"], others["release_plan"], others["ruff"],
        others["next_public_free"], others["helm_render"], others["moon_diagnosis"], mutated,
    )
    want = f"ci/next-env/run.sh: {site}"
    if want not in rows:
        print(f"NOT REPORTED: {site}")
        bad += 1
print(f"7e: {len(ct.NEXT_ENV_SH_CALL_SITES)} sites, {bad} bad")
sys.exit(1 if bad else 0)
```
Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && python3 "$TMPDIR/sma637-7e.py"; echo "rc=$?"
```
Expected: `7e: 23 sites, 0 bad`, `rc=0`. Then do ONE 7e row for real in the repo (delete `if [ "$rc_drift" -ne 1 ]; then` from `run.sh` with the Edit tool, run `python3 ci/affected-graph/ci_targets.py`, predict rc 1 with that row, restore), so the production path through `main()` and `read_input` is proven too.

- [ ] **Step 2: The gate graph**

Run the gates that this change touches:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/affected-graph/run.sh > "$TMPDIR/sma637-ag2.out" 2>&1; echo "affected-smoke rc=$?"; python3 ci/affected-graph/task_inputs.py; echo "input-liveness rc=$?"; /opt/homebrew/bin/bash ci/ruff/run.sh > "$TMPDIR/sma637-ruff.out" 2>&1; echo "ruff-ci rc=$?"; /opt/homebrew/bin/bash ci/actionlint/run.sh > "$TMPDIR/sma637-al.out" 2>&1; echo "actionlint rc=$?"; grep -m1 -F 'pipe capacity' "$TMPDIR/sma637-al.out"; tail -n 15 "$TMPDIR/sma637-al.out"
```
Expected: `rc=0` for all four. For `repo:actionlint`, read the preflight line first. If it reports a small pipe (rc 2, `small`), run the gate in a Linux container instead, per the memory "Small pipe: run gates in a Linux container" (SMA-685). Check 8e, check 12 and check 13 must show no FAIL row. Then run the full graph of the root `CLAUDE.md` ("Before you push: the full gate graph") once with the PATH prefix and `--base origin/main`, and read the bash-split gates per the root `CLAUDE.md` standing rule. CI is the final verdict, and the CI Linux bash is the third bash of AC 8.

- [ ] **Step 3: The critical-path cost**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-637-next-env-negative-control && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && for i in 1 2 3; do s=$(python3 -c 'import time; print(time.time())'); /bin/bash ci/next-env/run.sh --negative-control >/dev/null 2>&1; e=$(python3 -c 'import time; print(time.time())'); python3 -c "print('negctl s=%.1f' % ($e - $s))"; s=$(python3 -c 'import time; print(time.time())'); /bin/bash ci/next-env/run.sh >/dev/null 2>&1; e=$(python3 -c 'import time; print(time.time())'); python3 -c "print('real s=%.1f' % ($e - $s))"; done
```
Record the three pairs. The control time is the time added before `iam-console-ts:test`, `iam-console-ts:test-e2e` and the gateway-console equivalents (spec §7; estimate about 3 s for two apps). After the PR CI run, also record the `repo:next-env-drift` duration from the CI log.

- [ ] **Step 4: Re-run the whole battery after any fix**

If any step of Task 4 needed a fix in Tasks 1-3, commit the fix (a new commit, never an amend), then run Task 1 Steps 4-8 and Task 4 Steps 1-2 again in full, not only the failed row (memory "Re-run a mutation battery whole"). At the end, confirm `git status --porcelain` is empty and `git diff origin/main --stat` lists only the five files of the file-structure table plus this plan and the spec.

---

## Self-review record

- Spec coverage: §4.1 → Task 1 Step 3 (with deviations 1-6 above); §4.2 → Task 2 Step 1; §4.3 items 1-6 → Task 2 Steps 5, 4, 3 and 9, 6-10, 11, 12; §4.4 → Task 3 and the header in Task 1 Step 3; §5 → the exit codes in Task 1 Step 3 and the rows of Task 1 Steps 5, 7, 8; AC 1, 3, 4 → Task 1 Step 5; AC 2 → Task 1 Step 8; AC 5 → Task 2 Step 15; AC 6 → Task 2 Steps 13-14; AC 7 → Task 4 Step 1; AC 8 → Task 1 Steps 5-6 and CI; AC 9 → Task 2 Step 11; AC 10 → Task 3 Step 2; §7 critical path → Task 4 Step 3.
- Names used across tasks: `NEXT_ENV_SH_CALL_SITES`, `next_env_sh_text`, `wired_next_env`, `sh["next_env"]`, `_CALL_SITE_SOURCE_KEYS` key `next_env`, task key `next-env-drift`, functions `discover_apps`, `real_run`, `negative_control`, `negctl_cleanup`, `negctl_infra`, variables `idx_drift`, `idx_untracked`, `out_drift`, `out_untracked`, `rc_drift`, `rc_untracked`, `n_marker`, `fails`.
- The 23 pins in Task 1 Step 4 and Task 2 Step 6 are the same list in the same order.
